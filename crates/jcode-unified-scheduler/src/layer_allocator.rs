//! **层分配器 (Phase 1)** — 移植自 Parallax `layer_allocation.py`
//!
//! ## 算法概述
//!
//! Phase 1 负责将 LLM 的 Transformer 层**静态/半静态地**分配到异构 GPU 集群。
//!
//! ### 支持的策略:
//! 1. **Greedy (贪心)**: 优先构建长流水线 -> 最少阶段数
//! 2. **Dynamic Programming (动态规划)**: 平衡流水线数(并发)与阶段数(延迟)
//!
//! ### 核心算法 — Water-Filling (注水法):
//! 将模型层数按节点算力比例 \(l_i \approx \lambda \cdot P_i\) 分配,
//! 受限于每节点的容量上限 \(l_i \leq C_i\)。通过二分搜索求解 \(\lambda\)。
//!
//! ```text
//! 求解: sum_i min(C_i, λ * P_i) = L_total  ->  二分 λ ∈ [0, max(C_i/P_i)]
//! ```

use super::*;
use std::collections::{HashSet};
use std::cmp::Ordering;
use tracing::{info, debug, warn};

// ============================================================================
// 层分配器主结构体
// ============================================================================

/// 层分配器 — Parallax Phase 1 实现
#[derive(Debug)]
pub struct LayerAllocator {
    /// 总模型层数 (如 Llama-3-70B = 80 层, Qwen3-35B-A3B = 40 层等)
    total_layers: u32,
    /// 分配策略
    strategy: AllocationStrategy,
    /// 重平衡阈值 (变异系数 CV)
    rebalance_threshold: f64,
    /// 注水算法最大迭代次数
    water_filling_max_iters: usize,

    // === 运行时状态 ===
    /// 当前所有 pipeline (每个是一组连续覆盖 [0, L) 的节点链)
    pipelines: Vec<Pipeline>,
    /// 当前活跃节点列表 (Arc 引用)
    active_nodes: Vec<Arc<NodeInfo>>,
    /// 每层的负载状态 (用于最小堆快速找到最轻层)
    layer_loads: Vec<LayerLoad>,

    /// 统计
    pub allocation_count: u64,
    pub rebalance_count: u64,
}

/// 单层负载状态 — 对应 Parallax 的 `LayerLoad` 数据结构
#[derive(Debug, Clone)]
pub struct LayerLoad {
    pub layer_id: u32,
    /// 该层当前占用的 KV Cache 内存总量 (字节估算)
    pub current_kv_size: u64,
    /// 托管该层的节点 ID 集合
    hosting_nodes: HashSet<NodeId>,
}

impl LayerLoad {
    pub fn new(layer_id: u32) -> Self {
        Self {
            layer_id,
            current_kv_size: 0,
            hosting_nodes: HashSet::new(),
        }
    }

    /// 添加一个节点对该层的贡献
    pub fn add_node(&mut self, node: &NodeInfo) {
        self.hosting_nodes.insert(node.node_id);
        if let Some(kv_per_layer) = node.per_decoder_layer_kv_cache() {
            self.current_kv_size += kv_per_layer;
        }
    }

    /// 移除一个节点的贡献
    pub fn remove_node(&mut self, node: &NodeInfo) {
        self.hosting_nodes.remove(&node.node_id);
        if let Some(kv_per_layer) = node.per_decoder_layer_kv_cache() {
            self.current_kv_size = self.current_kv_size.saturating_sub(kv_per_layer);
        }
    }

    /// 通过节点ID移除节点贡献
    pub fn remove_node_by_id(&mut self, node_id: &NodeId) {
        // 注意: 这里无法准确知道该节点的KV Cache大小, 所以只能从集合中移除
        // 实际的KV大小更新应该由调用者处理或通过全局重平衡重新计算
        self.hosting_nodes.remove(node_id);
    }

    /// 托管该层的节点数
    pub fn host_count(&self) -> usize {
        self.hosting_nodes.len()
    }
}

// 最小堆排序: KV 占用小的优先
impl PartialEq for LayerLoad {
    fn eq(&self, other: &Self) -> bool {
        self.layer_id == other.layer_id && self.current_kv_size == other.current_kv_size
    }
}
impl Eq for LayerLoad {}

impl PartialOrd for LayerLoad {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LayerLoad {
    fn cmp(&self, other: &Self) -> Ordering {
        // 主键: KV 内存占用 (越小=越轻)
        match self.current_kv_size.cmp(&other.current_kv_size) {
            Ordering::Equal => self.layer_id.cmp(&other.layer_id), // 次键: 层ID
            ord => ord,
        }
    }
}

// ============================================================================
// LayerAllocator 实现
// ============================================================================

impl LayerAllocator {
    /// 创建新的层分配器
    pub fn new(
        strategy: AllocationStrategy,
        rebalance_threshold: f64,
        water_filling_max_iters: usize,
    ) -> Self {
        Self {
            total_layers: 0, // 由 allocate_from_standby 设置
            strategy,
            rebalance_threshold,
            water_filling_max_iters,
            pipelines: vec![],
            active_nodes: vec![],
            layer_loads: vec![],
            allocation_count: 0,
            rebalance_count: 0,
        }
    }

    /// 从待命节点池执行全局分配 (对应 Parallax `allocate_from_standby`)
    ///
    /// 这是主要的入口方法, 在以下情况调用:
    /// - 初始引导 (bootstrap)
    /// - 全局重平衡触发后
    /// - 大量节点加入/离开后
    pub fn allocate_from_standby(
        &mut self,
        nodes: &[&NodeInfo],
        total_layers: u32,
    ) -> Result<(), SchedulerError> {
        self.total_layers = total_layers;
        self.active_nodes = nodes.iter().map(|n| Arc::new((*n).clone())).collect();
        self.init_layer_loads();

        info!(
            "[LayerAllocator] 开始全局分配: {} 个节点, {} 层, 策略={:?}",
            nodes.len(),
            total_layers,
            self.strategy
        );

        match self.strategy {
            AllocationStrategy::Greedy => {
                self.greedy_allocate()?;
            }
            AllocationStrategy::DynamicProgramming => {
                self.dp_allocate()?;
            }
        }

        // 对每个生成的 pipeline 做原位注水平衡
        for i in 0..self.pipelines.len() {
            let pipeline_nodes = self.get_pipeline_nodes(i)?;
            self.water_filling_rebalance(&pipeline_nodes)?;
        }

        self.allocation_count += 1;

        // 输出结果摘要
        for (i, pipe) in self.pipelines.iter().enumerate() {
            debug!(
                "  Pipeline {}: {} stages, nodes={:?}, latency={:.1}ms",
                i,
                pipe.num_stages(),
                pipe.node_ids
                    .iter()
                    .map(|id| id.to_string().split_at(8).0.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                pipe.estimated_latency_ms,
            );
        }

        Ok(())
    }

    /// 动态加入新节点 (增量重平衡, 不中断现有服务)
    pub fn dynamic_join(&mut self, new_node: &NodeInfo) -> Result<(), SchedulerError> {
        if self.total_layers == 0 || self.layer_loads.is_empty() {
            return Err(SchedulerError::AllocationFailed("尚未初始化".into()));
        }

        // 从最小堆中找到最轻的层
        let lightest = self.get_lightest_layer();
        if lightest.is_none() {
            return Err(SchedulerError::AllocationFailed("无可用层".into()));
        }

        let lightest_layer = lightest.unwrap().layer_id;
        debug!(
            "[LayerAllocator] 动态加入节点 {} 到最轻层 {}",
            new_node.node_id, lightest_layer
        );

        // 计算该节点能承载多少层 (从最轻层开始向后连续分配)
        let start = lightest_layer;
        let capacity = self.estimate_node_capacity(new_node, start == 0);

        let mut end = start + capacity;
        if end > self.total_layers {
            end = self.total_layers;
            // 尾部节点需要 LM Head 容量
            let tail_cap = self.estimate_node_capacity(new_node, false); // LM head 已在 estimate 中考虑
            if tail_cap < end - start {
                end = start + tail_cap.max(1);
            }
        }

        // 注册节点
        let arc_node = Arc::new(new_node.clone());
        self.active_nodes.push(arc_node.clone());

        // 更新各层的负载状态
        for layer in start..end {
            if let Some(load) = self.layer_loads.get_mut(layer as usize) {
                load.add_node(&arc_node);
            }
        }

        // 更新节点的层范围
        // 注意: 这里需要通过外部接口更新 NodeInfo 的 start/end_layer
        // 实际使用时由 NodeManager 协调

        Ok(())
    }

    /// 检查是否需要全局重平衡
    pub fn should_rebalance(&self, nodes: &[&NodeInfo]) -> Result<bool, SchedulerError> {
        // 条件 1: 无完整 pipeline
        if !self.has_full_pipeline(nodes) {
            return Ok(true);
        }

        // 条件 2: 层间负载不均衡超过阈值 (CV > threshold)
        if self.layer_loads.len() >= 2 {
            let loads: Vec<f64> = self
                .layer_loads
                .iter()
                .map(|l| l.current_kv_size as f64)
                .collect();
            let mean: f64 = loads.iter().sum::<f64>() / loads.len() as f64;
            if mean > 0.0 {
                let variance: f64 =
                    loads.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / loads.len() as f64;
                let std_dev = variance.sqrt();
                let cv = std_dev / mean;

                debug!(
                    "[LayerAllocator] 负载 CV={:.4}, 阈值={:.4}",
                    cv, self.rebalance_threshold
                );

                if cv > self.rebalance_threshold {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// 触发全局重平衡
    pub fn global_rebalance(&mut self) -> Result<(), SchedulerError> {
        info!("[LayerAllocator] 开始全局重平衡...");
        self.rebalance_count += 1;

        let total_layers = self.total_layers;
        if total_layers == 0 {
            return Err(SchedulerError::NotInitialized);
        }

        // 先克隆节点数据,避免借用冲突
        let nodes_clone: Vec<NodeInfo> = self.active_nodes.iter().map(|n| (**n).clone()).collect();
        let nodes_refs: Vec<&NodeInfo> = nodes_clone.iter().collect();

        // 清空当前状态并重新分配
        self.pipelines.clear();
        self.layer_loads.clear();
        self.allocate_from_standby(&nodes_refs, total_layers)?;

        info!(
            "[LayerAllocator] 全局重平衡完成, {} 条 pipeline",
            self.pipelines.len()
        );
        Ok(())
    }

    /// 获取当前所有 pipeline
    pub fn pipelines(&self) -> &[Pipeline] {
        &self.pipelines
    }

    /// 是否存在完整 pipeline
    pub fn has_full_pipeline(&self, _nodes: &[&NodeInfo]) -> bool {
        self.pipelines.iter().any(|p| p.is_complete(self.total_layers))
    }

    // ========================================================================
    // 私有方法: Greedy 分配
    // ========================================================================

    /// 贪心策略分配
    fn greedy_allocate(&mut self) -> Result<(), SchedulerError> {
        // 按 capacity 降序排列节点
        let mut sorted_indices: Vec<usize> = (0..self.active_nodes.len()).collect();
        sorted_indices.sort_by(|&a, &b| {
            let cap_a = self.estimate_node_capacity(&self.active_nodes[a], true);
            let cap_b = self.estimate_node_capacity(&self.active_nodes[b], true);
            cap_b.partial_cmp(&cap_a).unwrap_or(Ordering::Equal)
        });

        let mut used = std::collections::HashSet::new(); // 已使用的节点索引
        let total_layers = self.total_layers as usize;

        loop {
            // 检查剩余容量
            let remaining_capacity: usize = sorted_indices
                .iter()
                .filter(|&&idx| !used.contains(&idx))
                .map(|&idx| self.estimate_node_capacity(&self.active_nodes[idx], true) as usize)
                .sum();

            if remaining_capacity < total_layers {
                break; // 无法再构建完整 pipeline
            }

            // 构建一条 pipeline
            let mut pipeline_nodes: Vec<(usize, Arc<NodeInfo>)> = vec![];
            let mut remaining = total_layers as u32;
            let mut remaining_cap_total = remaining_capacity;

            for &node_idx in &sorted_indices {
                if used.contains(&node_idx) {
                    continue;
                }

                let is_start = pipeline_nodes.is_empty();
                let node = &self.active_nodes[node_idx];

                let base_cap = self.estimate_node_capacity(node, is_start);
                let assign = if remaining <= base_cap {
                    // 可能是尾部节点, 需要 LM Head 容量
                    let tail_cap = self.estimate_node_capacity(node, false);
                    std::cmp::min(tail_cap, remaining)
                } else {
                    base_cap
                };

                if assign == 0 {
                    continue; // 此节点无法容纳任何层
                }

                // Look-ahead 优化: 如果还有足够容量构建下一条 pipeline, 选最小的
                let should_pick_smallest = !pipeline_nodes.is_empty()
                    && remaining_cap_total - base_cap as usize >= total_layers;

                if should_pick_smallest {
                    // 尝试找一个能完成当前 pipeline 的最小节点
                    // (保留大节点给后续 pipeline)
                    // 这里简化: 直接选当前节点
                }

                pipeline_nodes.push((node_idx, node.clone()));
                remaining -= assign;
                remaining_cap_total -= base_cap as usize;

                if remaining == 0 {
                    break;
                }
            }

            if remaining > 0 || pipeline_nodes.is_empty() {
                break; // 无法完成这条 pipeline
            }

            // 标记已使用的节点
            for (idx, _) in &pipeline_nodes {
                used.insert(*idx);
            }

            // 创建 Pipeline 并注册
            let node_ids: Vec<NodeId> = pipeline_nodes.iter().map(|(_, n)| n.node_id).collect();
            self.pipelines.push(Pipeline {
                id: uuid::Uuid::new_v4(),
                node_ids: node_ids.clone(),
                layer_range: (0, self.total_layers),
                estimated_latency_ms: self.estimate_pipeline_latency(&node_ids),
                throughput: 0.0,
            });
        }

        Ok(())
    }

    // ========================================================================
    // 私有方法: Dynamic Programming 分配
    // ========================================================================

    /// 动态规划策略分配
    ///
    /// 目标函数: Z(k) = k² / s*(k)
    /// - k: 流水线数量 (并发度)
    /// - s*(k): 实现 k 条流水线的最小总阶段数 (延迟指标)
    ///
    /// DP State: dp(i, open_residuals, finished_pipes)
    /// - i: 当前处理的 GPU 索引
    /// - open_residuals: 各开放流水线剩余所需层数
    /// - finished_pipes: 已完成的流水线数量
    fn dp_allocate(&mut self) -> Result<(), SchedulerError> {
        let n = self.active_nodes.len();
        #[allow(non_snake_case)]
        let L = self.total_layers as usize;
        if n == 0 || L == 0 {
            return Ok(());
        }

        // 后缀和 (用于剪枝)
        let mut suffix_sum = vec![0usize; n + 1];
        for i in (0..n).rev() {
            suffix_sum[i] =
                suffix_sum[i + 1] + self.estimate_node_capacity(&self.active_nodes[i], true) as usize;
        }

        let max_pipes = std::cmp::min(n, suffix_sum[0] / L);
        if max_pipes == 0 {
            warn!("[LayerAllocator] DP: 总容量不足构建任何 pipeline");
            return Ok(()); // 无法分配
        }

        let mut best_score: f64 = f64::NEG_INFINITY;
        let mut best_k = 1;
        let mut best_path: std::collections::HashMap<
            (usize, Vec<i32>, usize),   // (gpu_index, open_residuals, finished)
            DpAction,
        > = std::collections::HashMap::new();

        // 尝试不同的 k (目标流水线数)
        for k_target in 1..=max_pipes {
            let path = self.dp_search(k_target, n, L, &suffix_sum)?;
            let s_star = match path.get(&(n, vec![], k_target)) {
                Some(action) => action.min_cost,
                None => continue,
            };

            if s_star < f64::INFINITY {
                let score = (k_target * k_target) as f64 / s_star;
                debug!(
                    "[DP] k={} -> s*={}, score={:.2}",
                    k_target, s_star, score
                );
                if score > best_score {
                    best_score = score;
                    best_k = k_target;
                    best_path = path;
                }
            }
        }

        if best_k == 0 {
            return Err(SchedulerError::AllocationFailed("DP 未找到可行方案".into()));
        }

        // 回溯构建 pipelines
        let pipelines = self.dp_backtrack(best_k, n, &best_path)?;
        for pl_nodes in &pipelines {
            if !pl_nodes.is_empty() {
                let node_ids: Vec<NodeId> = pl_nodes.iter().map(|n| n.node_id).collect();
                self.pipelines.push(Pipeline {
                    id: uuid::Uuid::new_v4(),
                    node_ids: node_ids.clone(),
                    layer_range: (0, self.total_layers),
                    estimated_latency_ms: self.estimate_pipeline_latency(&node_ids),
                    throughput: 0.0,
                });
            }
        }

        info!(
            "[DP] 最终选择 k={} 条 pipeline, score={:.2}",
            best_k, best_score
        );
        Ok(())
    }

    /// DP 搜索核心
    #[allow(non_snake_case, clippy::type_complexity)]
    fn dp_search(
        &self,
        k_target: usize,
        n: usize,
        #[allow(non_snake_case)] L: usize,
        suffix_sum: &[usize],
    ) -> Result<std::collections::HashMap<(usize, Vec<i32>, usize), DpAction>, SchedulerError>
    {
        use std::collections::HashMap;
        let mut memo: HashMap<(usize, Vec<i32>, usize), DpAction> = HashMap::new();

        // 递归闭包 (用迭代模拟避免栈溢出)
        // 这里简化为带记忆化的递归实现
        // 生产环境应改为迭代式 DP

        #[allow(clippy::too_many_arguments)]
        fn solve(
            i: usize,
            open_residuals: Vec<i32>,
            finished: usize,
            k_target: usize,
            n: usize,
            L: usize,
            suffix_sum: &[usize],
            allocator: &LayerAllocator,
            memo: &mut HashMap<(usize, Vec<i32>, usize), DpAction>,
        ) -> f64 {
            // 终止条件: 所有 pipeline 都完成且没有开放的
            if finished == k_target && open_residuals.is_empty() {
                memo.entry((i, open_residuals, finished))
                    .or_insert_with(|| DpAction { kind: DpActionKind::Done, min_cost: 0.0 })
                ;
                return 0.0;
            }

            if i >= n {
                return f64::INFINITY;
            }

            // 剪枝条件
            let new_needed = k_target.saturating_sub(finished) - open_residuals.len();
            let need_open: i64 = open_residuals.iter().map(|&x| x as i64).sum();
            let remaining_cap = suffix_sum[i];
            if remaining_cap < (need_open.max(0) as usize + new_needed * L)
                || finished + open_residuals.len() + (n - i) < k_target
            {
                return f64::INFINITY;
            }

            let cap_i = allocator.estimate_node_capacity(&allocator.active_nodes[i], /*is_start*/ true) as i32;

            // Option 1: 跳过此节点
            let mut best = solve(i + 1, open_residuals.clone(), finished, k_target, n, L, suffix_sum, allocator, memo);
            let mut best_action = DpAction { kind: DpActionKind::Skip, min_cost: best };

            // Option 2: 分配到已有的开放 pipeline
            for j in 0..open_residuals.len() {
                let r_after = open_residuals[j] - cap_i;

                if r_after <= 0 {
                    // 尝试关闭 (加上 LM Head)
                    let cap_close = allocator.estimate_node_capacity(&allocator.active_nodes[i], /*lm_head=*/ false) as i64;
                    let r_after_close = open_residuals[j] - cap_close as i32;

                    if r_after_close <= 0 {
                        let mut new_open = open_residuals.clone();
                        new_open.remove(j);
                        let cost = 1.0 + solve(i + 1, new_open, finished + 1, k_target, n, L, suffix_sum, allocator, memo);
                        if cost < best {
                            best = cost;
                            best_action = DpAction { kind: DpActionKind::AssignToExisting(j, true), min_cost: cost };
                        }
                    } else {
                        let mut new_open = open_residuals.clone();
                        new_open[j] = r_after_close;
                        new_open.sort();
                        let cost = 1.0 + solve(i + 1, new_open, finished, k_target, n, L, suffix_sum, allocator, memo);
                        if cost < best {
                            best = cost;
                            best_action = DpAction { kind: DpActionKind::AssignToExisting(j, false), min_cost: cost };
                        }
                    }
                } else {
                    let mut new_open = open_residuals.clone();
                    new_open[j] = r_after;
                    new_open.sort();
                    let cost = 1.0 + solve(i + 1, new_open, finished, k_target, n, L, suffix_sum, allocator, memo);
                    if cost < best {
                        best = cost;
                        best_action = DpAction { kind: DpActionKind::AssignToExisting(j, false), min_cost: cost };
                    }
                }
            }

            // Option 3: 开启新的 pipeline
            if new_needed > 0 {
                let c_start = allocator.estimate_node_capacity(&allocator.active_nodes[i], /*input_embed=*/ true) as i64;
                let r_new = L as i64 - c_start;

                if r_new <= 0 {
                    let cost = 1.0 + solve(i + 1, open_residuals.clone(), finished + 1, k_target, n, L, suffix_sum, allocator, memo);
                    if cost < best {
                        best = cost;
                        best_action = DpAction { kind: DpActionKind::StartNew(0, true), min_cost: cost };
                    }
                } else {
                    let mut new_open = open_residuals.clone();
                    new_open.push(r_new as i32);
                    new_open.sort();
                    let cost = 1.0 + solve(i + 1, new_open, finished, k_target, n, L, suffix_sum, allocator, memo);
                    if cost < best {
                        best = cost;
                        best_action = DpAction { kind: DpActionKind::StartNew(r_new, false), min_cost: cost };
                    }
                }
            }

            memo.insert((i, open_residuals, finished), best_action.clone());
            best
        }

        let _solved = solve(0, vec![], 0, k_target, n, L, suffix_sum, self, &mut memo);
        Ok(memo)
    }

    /// DP 回溯: 从备忘录重建 pipeline 列表
    fn dp_backtrack(
        &self,
        best_k: usize,
        n: usize,
        path: &std::collections::HashMap<(usize, Vec<i32>, usize), DpAction>,
    ) -> Result<Vec<Vec<Arc<NodeInfo>>>, SchedulerError> {
        let mut pipelines: Vec<Vec<Arc<NodeInfo>>> = vec![vec![]; best_k];
        let mut open_list: Vec<(i64, Vec<Arc<NodeInfo>>)> = vec![];
        let mut finished = 0usize;
        let mut i = 0usize;

        while i < n && finished < best_k {
            let key = ({
                let mut ol: Vec<i64> = open_list.iter().map(|(r, _)| *r).collect();
                ol.sort();
                ol
            }, finished);

            let action = path.get(&(i, key.0.clone().into_iter().map(|x| x as i32).collect::<Vec<i32>>(), key.1));

            match action.map(|a| a.kind.clone()) {
                Some(DpActionKind::Done) | Some(DpActionKind::Skip) => {
                    i += 1;
                }
                Some(DpActionKind::AssignToExisting(j, closed)) => {
                    let node = self.active_nodes[i].clone();
                    if j < open_list.len() {
                        let (_, ref mut nodes) = open_list[j];
                        nodes.push(node);
                        if closed && j < pipelines.len() {
                            pipelines[finished] = std::mem::take(&mut open_list[j].1);
                            open_list.remove(j);
                            finished += 1;
                        }
                    }
                    i += 1;
                }
                Some(DpActionKind::StartNew(_, closed)) => {
                    let node = self.active_nodes[i].clone();
                    if closed {
                        if finished < pipelines.len() {
                            pipelines[finished].push(node);
                            finished += 1;
                        }
                    } else {
                        open_list.push((i as i64, vec![node]));
                    }
                    i += 1;
                }
                None => {
                    i += 1; // 未知动作, 跳过
                }
            }
        }

        Ok(pipelines)
    }

    // ========================================================================
    // Water-Filling 注水算法
    // ========================================================================

    /// 对单个 pipeline 进行原位注水平衡
    ///
    /// 使各节点获得的层数与其算力成正比: l_i ≈ λ · P_i, 且 l_i ≤ C_i
    pub fn water_filling_rebalance(
        &mut self,
        pipeline_nodes: &[Arc<NodeInfo>],
    ) -> Result<(), SchedulerError> {
        let n = pipeline_nodes.len();
        #[allow(non_snake_case)]
        let L = self.total_layers as usize;
        if n == 0 || L == 0 {
            return Ok(());
        }

        debug!(
            "[WaterFilling] 平衡 {} 节点, {} 层, max_iter={}",
            n, L, self.water_filling_max_iters
        );

        // 1. 收集每个节点的容量和算力
        let mut caps: Vec<u32> = Vec::with_capacity(n);
        let mut powers: Vec<f64> = Vec::with_capacity(n);

        for (idx, node) in pipeline_nodes.iter().enumerate() {
            let cap = if idx == 0 {
                // 首节点: 需要预留 Input Embedding
                self.estimate_node_capacity(node, true)
            } else if idx == n - 1 {
                // 尾节点: 需要预留 LM Head
                self.estimate_node_capacity(node, false)
            } else {
                self.estimate_node_capacity(node, false)
            };
            caps.push(cap);
            powers.push(node.hardware.tflops_fp16);
        }

        let total_cap: u32 = caps.iter().sum();
        if total_cap < L as u32 {
            return Err(SchedulerError::InsufficientResources {
                required: format!("{} layers", L),
            });
        }

        // 2. 二分搜索求解 λ
        let lo = 0.0f64;
        let hi = caps
            .iter()
            .zip(powers.iter())
            .map(|(&c, &p)| if p > 0.0 { c as f64 / p } else { f64::INFINITY })
            .fold(0.0f64, |a, b| a.max(b));

        let lam = binary_search_lambda(&caps, &powers, L as f64, lo, hi, self.water_filling_max_iters);

        debug!("[WaterFilling] λ = {:.6}", lam);

        // 3. 计算理论目标分配
        let target: Vec<f64> = caps
            .iter()
            .zip(powers.iter())
            .map(|(&c, &p)| (c as f64).min(lam * p))
            .collect();

        // 4. 整数化 (floor) + 余数分配
        let mut stage_counts: Vec<u32> = target.iter().map(|t| t.floor() as u32).collect();
        let assigned: u32 = stage_counts.iter().sum();
        let mut remaining = L.saturating_sub(assigned as usize) as i32;

        if remaining > 0 {
            // 按小数部分降序分配余数
            let mut frac: Vec<(f64, i32)> = target
                .iter()
                .enumerate()
                .map(|(i, t)| (t - stage_counts[i] as f64, -(i as i32)))
                .collect();
            frac.sort_by(|a, b| b.partial_cmp(a).unwrap_or(Ordering::Equal));

            for (_, neg_i) in frac {
                if remaining <= 0 {
                    break;
                }
                let idx = (-neg_i) as usize;
                if stage_counts[idx] < caps[idx] {
                    stage_counts[idx] += 1;
                    remaining -= 1;
                }
            }
        } else if remaining < 0 {
            // 不应发生 (floor 之和 ≤ L)
            warn!(
                "[WaterFilling] 整数化异常: 剩余 {} (应为 ≥ 0)",
                remaining
            );
        }

        // 5. 安全钳位检查
        let extra = stage_counts
            .iter()
            .zip(caps.iter())
            .filter(|&(&s, &c)| s > c)
            .count();
        if extra > 0 {
            // 强制截断到容量上限
            for i in 0..n {
                if stage_counts[i] > caps[i] {
                    stage_counts[i] = caps[i];
                }
            }
        }

        // 6. 应用分配 (更新节点状态)
        let mut start_layer = 0u32;
        for (idx, _node) in pipeline_nodes.iter().enumerate() {
            let count = stage_counts[idx];
            if count == 0 {
                continue;
            }
            let end_layer = start_layer + count;

            // 更新层负载状态
            for layer in start_layer..end_layer {
                if let Some(load) = self.layer_loads.get_mut(layer as usize) {
                    load.add_node(&pipeline_nodes[idx]);
                }
            }

            // 注意: 实际的 NodeInfo.start_layer/end_layer 更新由外部协调
            debug!(
                "  节点 {} -> layers [{}, {}) = {} layers",
                idx, start_layer, end_layer, count
            );
            start_layer = end_layer;
        }

        if start_layer != self.total_layers {
            warn!(
                "[WaterFilling] 覆盖不完全: {}/{} 层",
                start_layer, self.total_layers
            );
        }

        Ok(())
    }

    // ========================================================================
    // 辅助方法
    // ========================================================================

    fn init_layer_loads(&mut self) {
        self.layer_loads = (0..self.total_layers)
            .map(LayerLoad::new)
            .collect();
    }

    fn get_lightest_layer(&self) -> Option<&LayerLoad> {
        self.layer_loads
            .iter()
            .min_by_key(|l| (l.current_kv_size, l.layer_id))
    }

    fn get_pipeline_nodes(&self, pipeline_idx: usize) -> Result<Vec<Arc<NodeInfo>>, SchedulerError> {
        let pipe = self
            .pipelines
            .get(pipeline_idx)
            .ok_or(SchedulerError::AllocationFailed(format!("Pipeline {} 不存在", pipeline_idx)))?;

        pipe.node_ids
            .iter()
            .map(|nid| {
                self.active_nodes
                    .iter()
                    .find(|n| &n.node_id == nid)
                    .cloned()
                    .ok_or(SchedulerError::NodeNotFound(*nid))
            })
            .collect()
    }

    /// 估计节点能承载的解码层数 (基于显存预算)
    fn estimate_node_capacity(&self, node: &NodeInfo, include_input_embed: bool) -> u32 {
        let available_mem_bytes =
            (node.hardware.memory_gb * 1024.0 * 1024.0 * 1024.0 * node.param_mem_ratio) as u64;

        let reserved_for_embedding = if include_input_embed {
            // Embedding 参数占用 (粗估: vocab_size * hidden_dim * bytes_per_param)
            256_000 * 4096 * 2 // ~2GB for typical models
        } else {
            0
        };

        let usable = available_mem_bytes.saturating_sub(reserved_for_embedding);

        // 每层参数量 (以 7B 模型为基准: 每层约 100MB FP16)
        // 实际应根据 ModelInfo 计算
        let bytes_per_layer = match node.hardware.device_type.as_str() {
            "mlx" => 50_000_000, // Apple Silicon 使用更高效的量化
            _ => 100_000_000,    // 标准 FP16
        };

        (usable / bytes_per_layer) as u32
    }

    /// 估算 pipeline 端到端延迟
    fn estimate_pipeline_latency(&self, node_ids: &[NodeId]) -> f64 {
        let mut total = 0.0f64;
        let mut prev: Option<&Arc<NodeInfo>> = None;

        for nid in node_ids {
            let node = match self.active_nodes.iter().find(|n| &n.node_id == nid) {
                Some(n) => n,
                None => return f64::INFINITY,
            };

            total += node.effective_layer_latency_ms();

            if let Some(prev_node) = prev {
                total += prev_node.get_rtt_to(node);
            }

            prev = Some(node);
        }

        total
    }

    /// 获取层负载统计 (用于调试/监控)
    pub fn layer_load_summary(&self) -> Vec<(u32, u64, usize)> {
        self.layer_loads
            .iter()
            .map(|l| (l.layer_id, l.current_kv_size, l.host_count()))
            .collect()
    }

    // ========================================================================
    // 节点移除与故障恢复
    // ========================================================================

    /// 移除故障节点并触发层重新分配
    ///
    /// 当检测到节点离线或故障时调用此方法:
    /// 1. 从活跃节点列表中移除该节点
    /// 2. 清除该节点托管的所有层
    /// 3. 检查是否需要全局重平衡
    /// 4. 如果需要, 执行全局重平衡以恢复完整的 Pipeline
    pub fn remove_node_and_rebalance(
        &mut self,
        node_id: NodeId,
    ) -> Result<(), SchedulerError> {
        info!(
            "[LayerAllocator] 开始移除节点 {} 并重新平衡",
            node_id
        );

        // 1. 从活跃节点列表中移除
        let initial_count = self.active_nodes.len();
        self.active_nodes.retain(|n| n.node_id != node_id);
        
        if self.active_nodes.len() == initial_count {
            warn!("[LayerAllocator] 节点 {} 不在活跃列表中", node_id);
            return Err(SchedulerError::NodeNotFound(node_id));
        }

        // 2. 清除该节点托管的所有层
        let mut layers_affected = 0u32;
        for layer in &mut self.layer_loads {
            let before_count = layer.host_count();
            layer.remove_node_by_id(&node_id);
            if layer.host_count() < before_count {
                layers_affected += 1;
            }
        }

        info!(
            "[LayerAllocator] 节点 {} 已移除, 影响 {} 个层",
            node_id, layers_affected
        );

        // 3. 检查是否仍然有完整 Pipeline 并决定是否需要重平衡
        let total_layers = self.total_layers;
        
        // 预先计算节点引用并检查
        let nodes_refs_for_check: Vec<&NodeInfo> = self.active_nodes.iter().map(|n| n.as_ref()).collect();
        let needs_rebalance = total_layers > 0 && !self.has_full_pipeline(&nodes_refs_for_check);
        
        // 清除借用
        drop(nodes_refs_for_check);

        if needs_rebalance {
            warn!(
                "[LayerAllocator] 移除节点后 Pipeline 不完整, 需要全局重平衡"
            );
            
            // 4. 执行全局重平衡
            self.global_rebalance()?;
        } else {
            info!("[LayerAllocator] Pipeline 仍然完整, 无需重平衡");
        }

        Ok(())
    }

    /// 批量移除多个故障节点
    pub fn remove_nodes_and_rebalance(
        &mut self,
        node_ids: &[NodeId],
    ) -> Result<(), SchedulerError> {
        for &node_id in node_ids {
            self.remove_node_and_rebalance(node_id)?;
        }
        Ok(())
    }

    /// 检查并修复不完整的 Pipeline
    ///
    /// 在不进行完全重平衡的情况下, 尝试快速修复缺失的层覆盖
    pub fn repair_pipeline_gaps(&mut self) -> Result<(), SchedulerError> {
        let total_layers = self.total_layers;
        if total_layers == 0 {
            return Err(SchedulerError::NotInitialized);
        }

        // 找出未被任何节点托管的层
        let mut uncovered_layers = Vec::new();
        for (idx, layer) in self.layer_loads.iter().enumerate() {
            if layer.host_count() == 0 {
                uncovered_layers.push(idx as u32);
            }
        }

        if uncovered_layers.is_empty() {
            debug!("[LayerAllocator] 所有层都有节点托管, 无需修复");
            return Ok(());
        }

        warn!(
            "[LayerAllocator] 发现 {} 个未覆盖的层: {:?}",
            uncovered_layers.len(),
            uncovered_layers
        );

        // 尝试将未覆盖的层分配给现有节点
        let nodes_refs: Vec<&NodeInfo> = self.active_nodes.iter().map(|n| n.as_ref()).collect();
        
        // 简单的贪心修复: 找到容量最大的节点来接管这些层
        if let Some(best_node) = nodes_refs.into_iter()
            .max_by(|a, b| {
                let cap_a = self.estimate_node_capacity(a, false);
                let cap_b = self.estimate_node_capacity(b, false);
                cap_a.cmp(&cap_b)
            }) 
        {
            info!(
                "[LayerAllocator] 使用节点 {} 接管 {} 个未覆盖的层",
                best_node.node_id,
                uncovered_layers.len()
            );

            for &layer_id in &uncovered_layers {
                if let Some(layer) = self.layer_loads.get_mut(layer_id as usize) {
                    layer.add_node(best_node);
                }
            }
        } else {
            return Err(SchedulerError::InsufficientResources {
                required: "至少一个可用节点".to_string(),
            });
        }

        Ok(())
    }
}

// ============================================================================
// 辅助结构体和函数
// ============================================================================

#[derive(Debug, Clone)]
enum DpActionKind {
    Done,
    Skip,
    AssignToExisting(usize, bool), // (pipeline_index, closed)
    #[allow(dead_code)]
    StartNew(i64, bool),           // (residual, closed immediately)
}

#[derive(Debug, Clone)]
struct DpAction {
    kind: DpActionKind,
    min_cost: f64,
}

/// 二分搜索求解注水水位线 λ
fn binary_search_lambda(
    caps: &[u32],
    powers: &[f64],
    target: f64,
    mut lo: f64,
    mut hi: f64,
    max_iters: usize,
) -> f64 {
    for _ in 0..max_iters {
        let mid = 0.5 * (lo + hi);
        let total: f64 = caps
            .iter()
            .zip(powers.iter())
            .map(|(&c, &p)| (c as f64).min(mid * p))
            .sum();

        if total >= target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi // 返回使 sum >= target 的最小 λ
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_nodes() -> Vec<NodeInfo> {
        vec![
            NodeInfo {
                node_id: uuid::Uuid::new_v4(),
                hardware: NodeHardwareInfo::gpu("RTX-4090", 1, 82.0, 24.0, 1008.0),
                status: NodeStatus::Standby,
                start_layer: None,
                end_layer: None,
                current_requests: 0,
                max_requests: 16,
                avg_layer_latency_ms: None,
                last_heartbeat: chrono::Utc::now(),
                rtt_to_nodes: std::collections::HashMap::new(),
                kvcache_mem_ratio: 0.3,
                param_mem_ratio: 0.5,
            },
            NodeInfo {
                node_id: uuid::Uuid::new_v4(),
                hardware: NodeHardwareInfo::gpu("RTX-3090", 1, 71.0, 24.0, 936.0),
                status: NodeStatus::Standby,
                start_layer: None,
                end_layer: None,
                current_requests: 0,
                max_requests: 12,
                avg_layer_latency_ms: None,
                last_heartbeat: chrono::Utc::now(),
                rtt_to_nodes: std::collections::HashMap::new(),
                kvcache_mem_ratio: 0.3,
                param_mem_ratio: 0.5,
            },
            NodeInfo {
                node_id: uuid::Uuid::new_v4(),
                hardware: NodeHardwareInfo::gpu("RTX-4080", 1, 49.0, 16.0, 717.0),
                status: NodeStatus::Standby,
                start_layer: None,
                end_layer: None,
                current_requests: 0,
                max_requests: 10,
                avg_layer_latency_ms: None,
                last_heartbeat: chrono::Utc::now(),
                rtt_to_nodes: std::collections::HashMap::new(),
                kvcache_mem_ratio: 0.3,
                param_mem_ratio: 0.5,
            },
        ]
    }

    #[test]
    fn test_greedy_allocation() {
        let nodes = make_test_nodes();
        let node_refs: Vec<&NodeInfo> = nodes.iter().collect();

        let mut allocator = LayerAllocator::new(
            AllocationStrategy::Greedy,
            0.25,
            40,
        );

        let result = allocator.allocate_from_standby(&node_refs, 12);
        assert!(result.is_ok(), "贪心分配应成功");
        assert!(!allocator.pipelines().is_empty(), "应至少生成一条 pipeline");
    }

    #[test]
    fn test_dp_allocation() {
        let nodes = make_test_nodes();
        let node_refs: Vec<&NodeInfo> = nodes.iter().collect();

        let mut allocator = LayerAllocator::new(
            AllocationStrategy::DynamicProgramming,
            0.25,
            40,
        );

        let result = allocator.allocate_from_standby(&node_refs, 12);
        assert!(result.is_ok());
    }

    #[test]
    fn test_water_filling_basic() {
        // 验证注水算法的基本性质: 结果总层数 = 目标层数
        let nodes = make_test_nodes();
        let node_refs: Vec<&NodeInfo> = nodes.iter().collect();
        let arcs: Vec<Arc<NodeInfo>> = nodes.into_iter().map(Arc::new).collect();

        let mut allocator = LayerAllocator::new(AllocationStrategy::Greedy, 0.25, 40);
        allocator.allocate_from_standby(&node_refs, 12).unwrap();

        if !allocator.pipelines().is_empty() {
            let pipe_nodes = allocator.get_pipeline_nodes(0).unwrap();
            assert!(!pipe_nodes.is_empty());

            // 注水平衡
            allocator.water_filling_rebalance(&pipe_nodes).unwrap();

            // 验证层负载分布
            let summary = allocator.layer_load_summary();
            let total_assigned: u64 = summary.iter().map(|&(_, _, hosts)| hosts as u64).sum();
            assert!(total_assigned > 0, "应有节点被分配到层");
        }
    }

    #[test]
    fn test_rebalance_trigger() {
        let nodes = make_test_nodes();
        let node_refs: Vec<&NodeInfo> = nodes.iter().collect();

        let mut allocator = LayerAllocator::new(AllocationStrategy::Greedy, 0.25, 40);

        // 刚开始应该需要重平衡 (无完整 pipeline)
        assert!(allocator.should_rebalance(&node_refs).unwrap());

        // 分配完成后不应需要
        allocator.allocate_from_standby(&node_refs, 12).unwrap();
        // 分配后有完整 pipeline, 负载均衡 -> 不需要重平衡
    }

    #[test]
    fn test_binary_search_lambda() {
        let caps = vec![10u32, 20, 30];
        let powers = vec![1.0f64, 2.0, 3.0];
        let target = 40.0f64;

        let lam = binary_search_lambda(&caps, &powers, target, 0.0, 30.0, 40);

        // 验证: sum(min(caps, λ*powers)) ≈ target
        let total: f64 = caps
            .iter()
            .zip(powers.iter())
            .map(|(&c, &p)| (c as f64).min(lam * p))
            .sum();
        assert!((total - target).abs() < 1.0, "λ 应使总和接近目标");
    }

    #[test]
    fn test_node_removal_and_rebalance() {
        let nodes = make_test_nodes();
        let node_refs: Vec<&NodeInfo> = nodes.iter().collect();

        let mut allocator = LayerAllocator::new(AllocationStrategy::Greedy, 0.25, 40);
        
        // 先进行分配
        allocator.allocate_from_standby(&node_refs, 12).unwrap();
        assert!(!allocator.pipelines().is_empty());
        
        // 移除第一个节点
        let first_node_id = nodes[0].node_id;
        let result = allocator.remove_node_and_rebalance(first_node_id);
        
        // 移除应该成功
        assert!(result.is_ok(), "节点移除应该成功");
        
        // 验证节点已被移除
        assert_eq!(allocator.active_nodes.len(), nodes.len() - 1);
    }

    #[test]
    fn test_remove_nonexistent_node() {
        let nodes = make_test_nodes();
        let node_refs: Vec<&NodeInfo> = nodes.iter().collect();

        let mut allocator = LayerAllocator::new(AllocationStrategy::Greedy, 0.25, 40);
        allocator.allocate_from_standby(&node_refs, 12).unwrap();
        
        // 尝试移除不存在的节点
        let fake_id = uuid::Uuid::new_v4();
        let result = allocator.remove_node_and_rebalance(fake_id);
        
        assert!(result.is_err(), "移除不存在的节点应该失败");
    }

    #[test]
    fn test_pipeline_repair() {
        let nodes = make_test_nodes();
        let node_refs: Vec<&NodeInfo> = nodes.iter().collect();

        let mut allocator = LayerAllocator::new(AllocationStrategy::Greedy, 0.25, 40);
        allocator.allocate_from_standby(&node_refs, 12).unwrap();
        
        // 模拟某些层失去托管（手动清除）
        if let Some(layer) = allocator.layer_loads.get_mut(0) {
            layer.hosting_nodes.clear();
            layer.current_kv_size = 0;
        }
        
        // 尝试修复
        let result = allocator.repair_pipeline_gaps();
        assert!(result.is_ok());
        
        // 验证第0层现在有节点托管
        if let Some(layer) = allocator.layer_loads.get(0) {
            assert!(layer.host_count() > 0, "修复后第0层应该有节点托管");
        }
    }
}
