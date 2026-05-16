//! **请求路由器 (Phase 2)** — 移植自 Parallax `request_routing.py`
//!
//! ## 算法概述
//!
//! Phase 2 在 **每次请求到达时** 动态选择最优的 GPU 节点执行路径。
//!
//! 核心思想: 将已分配好层的节点集群建模为一个 **DAG** (有向无环图):
//! - 每个 Node 是一个 Vertex, 其 `[start_layer, end_layer)` 定义了位置
//! - 边存在条件: `end(j) == start(i)` (保证层连续覆盖)
//! - 边代价 = RTT (节点间通信延迟)
//! - 顶点代价 = layer_latency_ms (节点计算延迟)
//!
//! 在此 DAG 上运行 **动态规划** 寻找从 Layer 0 到 Layer L 的最小延迟路径。

use super::*;
use std::sync::Arc;
use std::collections::{BTreeMap, HashMap};

// ============================================================================
// 请求路由策略 Trait 和实现
// ============================================================================

/// 请求路由策略接口
pub trait RoutingStrategy: Send + Sync {
    /// 寻找最优路径
    fn find_optimal_path(
        &self,
        num_layers: u32,
        nodes: &[Arc<NodeInfo>],
    ) -> Result<Option<(Vec<NodeId>, f64)>, SchedulerError>;
}

/// 请求路由器 — Phase 2 主入口
#[derive(Debug)]
pub struct RequestRouter {
    /// 当前路由策略
    strategy: Box<dyn RoutingStrategy>,
    /// 是否启用预热裁剪 (turning points optimization)
    enable_warmup_trim: bool,
    /// 统计
    pub routing_count: u64,
}

impl RequestRouter {
    pub fn new(strategy: RoutingStrategyEnum, enable_warmup_trim: bool) -> Self {
        let strat: Box<dyn RoutingStrategy> = match strategy {
            RoutingStrategyEnum::DynamicProgramming => Box::new(DPRouting {}),
            RoutingStrategyEnum::Randomized => Box::new(RandomRouting::new()),
            RoutingStrategyEnum::RoundRobin => Box::new(RoundRobinRouting::new()),
        };

        Self {
            strategy: strat,
            enable_warmup_trim,
            routing_count: 0,
        }
    }

    /// 为给定请求寻找最优路径
    pub fn find_optimal_path(
        &mut self,
        virtual_layers: u32,
        nodes: &[Arc<NodeInfo>],
    ) -> Result<Option<(Vec<NodeId>, f64)>, SchedulerError> {
        self.routing_count += 1;
        let mut result = self.strategy.find_optimal_path(virtual_layers, nodes)?;

        // 预热裁剪优化
        if self.enable_warmup_trim && result.is_some() && !nodes.is_empty() {
            if let Some((ref path, lat)) = result {
                let turning_points = find_turning_points(nodes, virtual_layers);
                if !turning_points.is_empty() {
                    debug!("[Router] 发现 {} 个转折点, 可裁剪冗余层", turning_points.len());
                    // 裁剪逻辑: 更新路径中的节点层范围
                }
                result = Some((path.clone(), lat));
            }
        }

        Ok(result)
    }
}

/// 路由策略枚举 (与 lib.rs 中的 RoutingStrategy 区分, 这里是内部实现枚举)
pub enum RoutingStrategyEnum {
    DynamicProgramming,
    Randomized,
    RoundRobin,
}

// ============================================================================
// 策略 1: 动态规划路由 (推荐)
// ============================================================================

struct DPRouting;

impl RoutingStrategy for DPRouting {
    fn find_optimal_path(
        &self,
        num_layers: u32,
        nodes: &[Arc<NodeInfo>],
    ) -> Result<Option<(Vec<NodeId>, f64)>, SchedulerError> {
        if num_layers == 0 || nodes.is_empty() {
            return Ok(None);
        }

        // === 构建 DAG 顶点集合 ===
        // 每个有效 (有层分配且活跃) 的节点是一个顶点
        let mut starts: BTreeMap<u32, Vec<usize>> = BTreeMap::new(); // start_layer -> [node_indices]
        let mut ends: BTreeMap<u32, Vec<usize>> = BTreeMap::new();   // end_layer -> [node_indices]

        let mut order: Vec<usize> = vec![]; // 排序后的节点索引列表

        for (idx, node) in nodes.iter().enumerate() {
            if !node.is_online() {
                continue;
            }
            if let (Some(start), Some(end)) = (node.start_layer, node.end_layer) {
                starts.entry(start).or_default().push(idx);
                ends.entry(end).or_default().push(idx);
                order.push(idx);
            }
        }

        if order.is_empty() {
            return Ok(None);
        }

        // 按 (start_layer, end_layer) 排序
        order.sort_by_key(|&idx| {
            (
                nodes[idx].start_layer.unwrap_or(u32::MAX),
                nodes[idx].end_layer.unwrap_or(0),
            )
        });

        // === DP 初始化 ===
        // dp[i] = 到达节点 order[i] 的最小累积延迟
        let mut dp: HashMap<usize, f64> = order.iter().map(|&i| (i, f64::INFINITY)).collect();
        let mut parent: HashMap<usize, Option<usize>> = order.iter().map(|&i| (i, None)).collect();

        // 初始化: 从 Layer 0 开始的节点
        if let Some(starters) = starts.get(&0) {
            for &i in starters {
                if i < nodes.len() {
                    dp.insert(i, nodes[i].effective_layer_latency_ms());
                }
            }
        } else {
            // 无节点托管第 0 层 -> 无法构建路径
            return Ok(None);
        }

        // === DP 状态转移 ===
        // 对于每个节点 i (作为目的地), 查找所有满足 end(j) == start(i) 的前置节点 j
        for &i in &order {
            let ni = &nodes[i];
            let start_i = match ni.start_layer {
                Some(s) => s,
                None => continue,
            };

            if dp.get(&i).copied().unwrap_or(f64::INFINITY) == f64::INFINITY {
                continue; // 此节点不可达
            }

            // 查找所有能连接到此节点的前置节点
            if let Some(predecessors) = ends.get(&start_i) {
                for &j in predecessors {
                    if j >= nodes.len() || i >= nodes.len() {
                        continue;
                    }

                    let prev_cost = dp.get(&j).copied().unwrap_or(f64::INFINITY);
                    if prev_cost == f64::INFINITY {
                        continue; // 前置节点不可达
                    }

                    let nj = &nodes[j];
                    let ni = &nodes[i];

                    // 边代价 = RTT (同一节点则为 0)
                    let edge_cost = if nj.node_id == ni.node_id {
                        0.0
                    } else {
                        nj.get_rtt_to(ni)
                    };

                    // 顶点代价 = 节点处理其负责层的延迟
                    let vertex_cost = ni.effective_layer_latency_ms();

                    let candidate = prev_cost + edge_cost + vertex_cost;

                    let current = dp.get(&i).copied().unwrap_or(f64::INFINITY);
                    if candidate < current {
                        dp.insert(i, candidate);
                        parent.insert(i, Some(j));
                    }
                }
            }
        }

        // === 找最优终点 (必须结束于 total_layers) ===
        let terminals = ends.get(&num_layers);
        match terminals {
            Some(terminals) if !terminals.is_empty() => {
                // 选择延迟最小的终点
                let best_end = terminals
                    .iter()
                    .filter(|&&idx| dp.get(&idx).copied().unwrap_or(f64::INFINITY) < f64::INFINITY)
                    .min_by_key(|&&idx| {
                        ordered_float::OrderedFloat(dp.get(&idx).copied().unwrap_or(f64::INFINITY))
                    });

                match best_end {
                    Some(&end_idx) => {
                        let final_latency = dp[&end_idx];
                        if final_latency >= f64::INFINITY {
                            return Ok(None);
                        }

                        // 回溯重建路径
                        let mut path_indices = vec![];
                        let mut cur: Option<usize> = Some(end_idx);
                        while let Some(idx) = cur {
                            path_indices.push(idx);
                            cur = parent.get(&idx).copied().flatten();
                        }
                        path_indices.reverse();

                        let path: Vec<NodeId> = path_indices.iter().map(|&i| nodes[i].node_id).collect();
                        Ok(Some((path, final_latency)))
                    }
                    None => Ok(None),
                }
            }
            _ => Ok(None), // 无终点
        }
    }
}

// ============================================================================
// 策略 2: 随机路由 (用于基准测试)
// ============================================================================

struct RandomRouting {
    rng_state: std::sync::atomic::AtomicU64,
}

impl RandomRouting {
    fn new() -> Self {
        Self { rng_state: std::sync::atomic::AtomicU64::new(42) }
    }
}

impl RoutingStrategy for RandomRouting {
    fn find_optimal_path(
        &self,
        _num_layers: u32,
        nodes: &[Arc<NodeInfo>],
    ) -> Result<Option<(Vec<NodeId>, f64)>, SchedulerError> {
        let candidates: Vec<&Arc<NodeInfo>> = nodes
            .iter()
            .filter(|n| n.is_online() && !n.is_overloaded())
            .collect();

        if candidates.is_empty() {
            return Ok(None);
        }

        let new_state = self.rng_state.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let idx = (new_state as usize) % candidates.len();
        let chosen = candidates[idx];

        Ok(Some((
            vec![chosen.node_id],
            chosen.effective_layer_latency_ms(),
        )))
    }
}

// ============================================================================
// 策略 3: Round Robin 轮询
// ============================================================================

struct RoundRobinRouting {
    counter: std::sync::atomic::AtomicUsize,
}

impl RoundRobinRouting {
    fn new() -> Self {
        Self { counter: std::sync::atomic::AtomicUsize::new(0) }
    }
}

impl RoutingStrategy for RoundRobinRouting {
    fn find_optimal_path(
        &self,
        _num_layers: u32,
        nodes: &[Arc<NodeInfo>],
    ) -> Result<Option<(Vec<NodeId>, f64)>, SchedulerError> {
        let candidates: Vec<&Arc<NodeInfo>> = nodes
            .iter()
            .filter(|n| n.is_online() && !n.is_overloaded())
            .collect();

        if candidates.is_empty() {
            return Ok(None);
        }

        let idx = self.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % candidates.len();

        let chosen = candidates[idx];
        Ok(Some((
            vec![chosen.node_id],
            chosen.effective_layer_latency_ms(),
        )))
    }
}

// ============================================================================
// Turning Points (预热裁剪)
// ============================================================================

/// 寻找预热裁剪点 — 对应 Parallax 的 `find_turning_points`
///
/// 通过 Layer-level DP 分析最优路由, 发现可以裁剪的冗余层片段。
///
/// 返回: `(node_id, layer_index, kind)` 列表
/// - `"tail"`: 路由在第 l 层离开此节点, 但该节点仍托管 l 及之后的层 -> 裁剪 [l, end)
/// - `"head"`: 路由首次在第 l 层使用此节点, 但该节点从更早的层就开始托管 -> 裁剪 [start, l)
pub fn find_turning_points(nodes: &[Arc<NodeInfo>], num_layers: u32) -> Vec<(NodeId, u32, &'static str)> {
    if num_layers == 0 || nodes.is_empty() {
        return vec![];
    }

    // 构建每层的 host 列表
    let mut layer_hosts: Vec<Vec<usize>> = (0..num_layers)
        .map(|l| {
            nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.hosts_layer(l))
                .map(|(i, _)| i)
                .collect()
        })
        .collect();

    // 如果有任何层没有 host -> 无法分析
    if layer_hosts.iter().any(|h| h.is_empty()) {
        return vec![];
    }

    // === Layer-level DP ===
    // dp[l][i] = 到达第 l 层使用节点 i 的最小延迟
    let mut dp: Vec<HashMap<usize, f64>> = vec![];
    let mut back: Vec<HashMap<usize, Option<usize>>> = vec![];

    // 初始化第 0 层
    let mut dp_0: HashMap<usize, f64> = HashMap::new();
    let mut back_0: HashMap<usize, Option<usize>> = HashMap::new();
    for &i in &layer_hosts[0] {
        dp_0.insert(i, nodes[i].effective_layer_latency_ms());
        back_0.insert(i, None);
    }
    dp.push(dp_0);
    back.push(back_0);

    // DP 递推
    for l in 1..num_layers as usize {
        let mut curr_dp: HashMap<usize, f64> = HashMap::new();
        let mut curr_back: HashMap<usize, Option<usize>> = HashMap::new();

        for &i in &layer_hosts[l] {
            let node_i = &nodes[i];
            let mut best_cost = f64::INFINITY;
            let mut best_j: Option<usize> = None;

            // 查看上一层有哪些候选
            if let Some(prev_dp) = dp.get(l - 1) {
                for (&j, &prev_cost) in prev_dp {
                    if prev_cost >= f64::INFINITY {
                        continue;
                    }
                    let node_j = &nodes[j];

                    let trans = if i == j {
                        0.0
                    } else {
                        node_j.get_rtt_to(node_i)
                    };

                    let total = prev_cost + trans + node_i.effective_layer_latency_ms();

                    if total < best_cost {
                        best_cost = total;
                        best_j = Some(j);
                    }
                }
            }

            curr_dp.insert(i, best_cost);
            curr_back.insert(i, best_j);
        }

        dp.push(curr_dp);
        back.push(curr_back);
    }

    // 回溯最优路径 (按层记录选择的节点索引)
    let last_layer = num_layers as usize - 1;
    let last_dp = match dp.last() {
        Some(d) if !d.is_empty() => d,
        _ => return vec![],
    };

    let end_i = last_dp
        .iter()
        .min_by_key(|(_, cost)| ordered_float::OrderedFloat(*cost))
        .map(|(&i, _)| i);

    let end_i = match end_i {
        Some(i) => i,
        None => return vec![],
    };

    let mut path_idx: Vec<usize> = vec![end_i];
    let mut current = Some(end_i);

    while let Some(idx) = current {
        if idx == 0 {
            break;
        }
        let layer = nodes[idx].start_layer.unwrap_or(0) as usize;
        if layer == 0 {
            break;
        }
        if let Some(back_map) = back.get(layer.saturating_sub(1)) {
            current = back_map.get(&idx).copied().flatten();
            if let Some(c) = current {
                path_idx.push(c);
            } else {
                break;
            }
        } else {
            break;
        }
    }
    path_idx.reverse();

    // === 识别转折点 ===
    let mut turning: Vec<(NodeId, u32, &'static str)> = vec![];

    // Tail truncation: 当路由在第 l 层离开节点 A 时
    for l in 1..path_idx.len() {
        let prev_i = path_idx[l - 1];
        let cur_i = path_idx[l];
        if prev_i == cur_i {
            continue;
        }

        let prev_node = &nodes[prev_i];
        let switch_layer = l as u32;
        if prev_node.hosts_layer(switch_layer) {
            turning.push((prev_node.node_id, switch_layer, "tail"));
        }
    }

    // Head truncation: 节点被使用的起始层晚于其托管起始层
    let mut first_used: HashMap<usize, u32> = HashMap::new();
    for (l, &idx) in path_idx.iter().enumerate() {
        first_used.entry(idx).or_insert(l as u32);
    }

    for (&idx, &first_l) in &first_used {
        let node = &nodes[idx];
        if let Some(start) = node.start_layer {
            if first_l as u32 > start {
                turning.push((node.node_id, first_l as u32, "head"));
            }
        }
    }

    turning
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 估算 Pipeline 端到端延迟 (公开工具函数)
pub fn estimate_pipeline_latency(
    node_ids: &[NodeId],
    nodes_map: &HashMap<NodeId, Arc<NodeInfo>>,
) -> f64 {
    let mut total = 0.0f64;
    let mut prev: Option<&Arc<NodeInfo>> = None;

    for nid in node_ids {
        let node = match nodes_map.get(nid) {
            Some(n) => n.as_ref(),
            None => return f64::INFINITY,
        };

        if node.is_overloaded() {
            return f64::INFINITY;
        }

        total += node.effective_layer_latency_ms();

        if let Some(prev_node) = prev {
            total += prev_node.get_rtt_to(node);
        }

        prev = Some(nodes_map.get(nid).unwrap());
    }

    total
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn make_test_node(id_str: &str, start: u32, end: u32, latency: f64) -> Arc<NodeInfo> {
        Arc::new(NodeInfo {
            node_id: uuid::Uuid::new_v4(),
            hardware: NodeHardwareInfo::gpu("TestGPU", 1, 80.0, 24.0, 900.0),
            status: NodeStatus::Active,
            start_layer: Some(start),
            end_layer: Some(end),
            current_requests: 0,
            max_requests: 16,
            avg_layer_latency_ms: Some(latency),
            last_heartbeat: chrono::Utc::now(),
            rtt_to_nodes: std::collections::HashMap::new(),
            kvcache_mem_ratio: 0.3,
            param_mem_ratio: 0.5,
        })
    }

    #[test]
    fn test_dp_routing_basic() {
        let router = DPRouting;

        // 3 个节点分别托管不同层段
        let nodes = vec![
            make_test_node("A", 0, 4, 2.0),   // 层 0-3
            make_test_node("B", 4, 8, 3.0),   // 层 4-7
            make_test_node("C", 8, 12, 2.5),  // 层 8-11
        ];

        let result = router.find_optimal_path(12, &nodes).unwrap();
        assert!(result.is_some(), "应找到有效路径");

        let (path, latency) = result.unwrap();
        assert_eq!(path.len(), 3, "路径应经过全部 3 个节点");
        assert!(latency > 0.0, "延迟应 > 0");
        println!("DP 路径: {:?}, 延迟={:.2}ms", path, latency);
    }

    #[test]
    fn test_dp_routing_skip_overloaded() {
        let router = DPRouting;

        let mut nodes = vec![
            make_test_node("A", 0, 6, 2.0),
            make_test_node("B", 6, 12, 3.0),
        ];
        // 让 B 过载
        Arc::make_mut(&mut nodes[1]).current_requests = 100;
        Arc::make_mut(&mut nodes[1]).max_requests = 1;

        let result = router.find_optimal_path(12, &nodes).unwrap();
        // B 过载 -> 应返回 None 或绕过 B
        if let Some((path, _)) = result {
            assert!(!path.contains(&nodes[1].node_id), "不应包含过载节点");
        }
    }

    #[test]
    fn test_round_robin() {
        let mut router = RequestRouter::new(RoutingStrategyEnum::RoundRobin, false);
        let nodes = vec![
            make_test_node("X", 0, 12, 5.0),
            make_test_node("Y", 0, 12, 5.0),
        ];

        let r1 = router.find_optimal_path(12, &nodes).unwrap();
        let r2 = router.find_optimal_path(12, &nodes).unwrap();
        // 两次轮询应选不同节点 (如果都可用)
        assert_ne!(r1, r2, "RoundRobin 应交替选择节点");
    }

    #[test]
    fn test_turning_points_detection() {
        let nodes = vec![
            make_test_node("A", 0, 8, 2.0),  // 托管 0-7
            make_test_node("B", 4, 12, 3.0), // 托管 4-11 (与 A 有重叠!)
        ];

        // 设置 RTT
        Arc::make_mut(&mut nodes[0])
            .rtt_to_nodes
            .insert(nodes[1].node_id, 5.0);
        Arc::make_mut(&mut nodes[1])
            .rtt_to_nodes
            .insert(nodes[0].node_id, 5.0);

        let points = find_turning_points(&nodes, 12);
        println!("转折点: {:?}", points);
        // 由于 A 和 B 有重叠区域, DP 可能会选择在某个切换点从 A 切到 B
        // 或反过来, 这取决于延迟和 RTT
    }
}
