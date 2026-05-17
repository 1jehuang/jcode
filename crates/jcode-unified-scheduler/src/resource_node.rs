//! **资源节点管理器** — 移植自 Parallax `node.py` + `node_management.py`
//!
//! ## 功能
//!
//! 1. **节点生命周期管理**: 注册、注销、心跳、健康检查
//! 2. **Roofline 性能模型**: 基于 Roofline 模型估算节点延迟
//!     - Compute-bound: FLOPs / TFLOPS
//!     - IO-bound: IO_bytes / bandwidth
//! 3. **网络感知**: RTT 缓存、对称查找
//! 4. **容量估算**: 基于显存预算计算可容纳的模型层数

use super::*;
use std::collections::HashMap;
use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::warn;

// ============================================================================
// 节点管理器
// ============================================================================

/// 节点管理器 — 管理所有算力节点的生命周期
#[derive(Debug)]
pub struct NodeManager {
    /// 所有已知节点 (node_id -> NodeInfo)
    nodes: HashMap<NodeId, Arc<NodeInfo>>,
    /// 最后注册的节点 (用于增量重平衡)
    last_registered: Option<NodeId>,
    /// 心跳超时时间
    heartbeat_timeout: Duration,
    /// 统计
    pub total_registered: AtomicU64,
    pub total_unregistered: AtomicU64,
    pub total_heartbeats: AtomicU64,
}

impl NodeManager {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            last_registered: None,
            heartbeat_timeout: Duration::from_secs(30),
            total_registered: AtomicU64::new(0),
            total_unregistered: AtomicU64::new(0),
            total_heartbeats: AtomicU64::new(0),
        }
    }

    /// 设置心跳超时
    pub fn set_heartbeat_timeout(&mut self, timeout_secs: u64) {
        self.heartbeat_timeout = Duration::from_secs(timeout_secs);
    }

    /// 注册新节点
    pub async fn register_node(&mut self, hardware: NodeHardwareInfo) -> Result<NodeId, SchedulerError> {
        let node_id = hardware.node_id;
        let now = chrono::Utc::now();

        let node = NodeInfo {
            node_id,
            hardware: hardware.clone(),
            status: NodeStatus::Standby,
            start_layer: None,
            end_layer: None,
            current_requests: 0,
            max_requests: Self::default_max_requests(&hardware),
            avg_layer_latency_ms: None,
            last_heartbeat: now,
            rtt_to_nodes: HashMap::new(),
            kvcache_mem_ratio: 0.3,
            param_mem_ratio: 0.5,
        };

        self.nodes.insert(node_id, Arc::new(node));
        self.last_registered = Some(node_id);
        self.total_registered.fetch_add(1, Ordering::Relaxed);

        Ok(node_id)
    }

    /// 注销节点
    pub async fn unregister_node(&mut self, node_id: &NodeId) -> Result<(), SchedulerError> {
        if let Some(mut node) = self.nodes.remove(node_id) {
            // 清除服务状态
            use std::sync::Arc;
            Arc::<NodeInfo>::make_mut(&mut node).clear_serving_state();
            Arc::<NodeInfo>::make_mut(&mut node).status = NodeStatus::Offline;
            self.total_unregistered.fetch_add(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(SchedulerError::NodeNotFound(*node_id))
        }
    }

    /// 更新节点心跳
    pub async fn update_heartbeat(
        &mut self,
        node_id: &NodeId,
        latency_ms: Option<f64>,
    ) -> Result<(), SchedulerError> {
        let node = self.nodes.get_mut(node_id).ok_or(SchedulerError::NodeNotFound(*node_id))?;
        
        let node_mut = Arc::make_mut(node);
        node_mut.last_heartbeat = chrono::Utc::now();

        if let Some(latency) = latency_ms {
            node_mut.set_layer_latency_ms(latency);
        }

        // 如果之前是离线状态, 恢复为 Standby
        if node_mut.status == NodeStatus::Offline {
            node_mut.status = NodeStatus::Standby;
        }

        self.total_heartbeats.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// 获取节点引用
    pub fn get_node(&self, node_id: &NodeId) -> Option<&Arc<NodeInfo>> {
        self.nodes.get(node_id)
    }

    /// 获取可变节点引用
    pub fn get_node_mut(&mut self, node_id: &NodeId) -> Option<&mut Arc<NodeInfo>> {
        self.nodes.get_mut(node_id)
    }

    /// 获取所有活跃节点信息 (克隆)
    pub fn active_nodes(&self) -> Vec<NodeInfo> {
        self.nodes.values().map(|n| (**n).clone()).collect()
    }

    /// 获取所有活跃节点 (Arc 引用)
    pub fn active_node_list(&self) -> Vec<&NodeInfo> {
        self.nodes.values().map(|n| n.as_ref()).collect()
    }

    /// 获取所有活跃节点 (Arc 引用)
    pub fn active_node_list_arc(&self) -> Vec<Arc<NodeInfo>> {
        self.nodes.values().cloned().collect()
    }

    /// 待命节点 (未分配任何层)
    pub fn standby_nodes(&self) -> Vec<&NodeInfo> {
        self.active_node_list()
            .into_iter()
            .filter(|n| n.status == NodeStatus::Standby && n.start_layer.is_none())
            .collect()
    }

    /// 是否存在完整的 pipeline
    pub fn has_full_pipeline(&self, total_layers: u32) -> bool {
        // 简化判断: 至少有一个节点从 0 开始, 一个到 L 结束
        let has_start = self.nodes.values().any(|n| n.start_layer == Some(0));
        let has_end = self
            .nodes
            .values()
            .any(|n| n.end_layer == Some(total_layers));
        has_start && has_end
    }

    /// 最后注册的节点
    pub fn last_registered_node(&self) -> Option<&NodeInfo> {
        self.last_registered.and_then(|id| self.nodes.get(&id).map(|n| n.as_ref()))
    }

    /// 心跳检查 — 将超时节点标记为离线
    pub async fn check_heartbeats(&mut self) -> Vec<NodeId> {
        let now = chrono::Utc::now();
        let mut expired = vec![];

        for (id, node) in &self.nodes {
            let elapsed = now.signed_duration_since(node.last_heartbeat);
            if elapsed > chrono::Duration::from_std(self.heartbeat_timeout).unwrap_or(chrono::TimeDelta::seconds(0)) {
                expired.push(*id);
            }
        }

        for id in &expired {
            if let Some(node) = self.nodes.get_mut(id) {
                Arc::make_mut(node).status = NodeStatus::Offline;
            }
        }

        if !expired.is_empty() {
            warn!(
                "[NodeManager] {} 个节点因心跳超时标记为离线",
                expired.len()
            );
        }

        expired
    }

    /// 获取集群资源摘要
    pub fn cluster_summary(&self) -> ClusterResourceSummary {
        let online: Vec<_> = self
            .nodes
            .values()
            .filter(|n| n.is_online())
            .collect();

        let total_gpus: u32 = online.iter().map(|n| n.hardware.num_gpus).sum();
        let total_tflops: f64 = online.iter().map(|n| n.hardware.tflops_fp16).sum();
        let total_memory: f64 = online.iter().map(|n| n.hardware.memory_gb).sum();
        let avg_load: f64 = if online.is_empty() {
            0.0
        } else {
            online.iter().map(|n| n.load_ratio()).sum::<f64>() / online.len() as f64
        };

        ClusterResourceSummary {
            total_nodes: self.nodes.len(),
            active_nodes: online.len(),
            total_gpus,
            total_tflops,
            total_memory_gb: total_memory,
            avg_load_ratio: avg_load,
            available_pipelines: 0, // 由 LayerAllocator 填充
        }
    }

    /// 默认最大请求数 (基于 KV Cache 预算)
    fn default_max_requests(hardware: &NodeHardwareInfo) -> u32 {
        // 粗估: 假设每个请求占用 ~500MB KV Cache (对于中等序列长度)
        let kv_memory_bytes = (hardware.memory_gb * 1024.0 * 1024.0 * 1024.0 * 0.3) as u64; // 30% 给 KV
        let per_request_estimate = 500_000_000; // 500MB per request (保守)
        let max = kv_memory_bytes / per_request_estimate;
        std::cmp::min(max as u32, 128) // 上限 128 并发
    }
}

impl Default for NodeManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// NodeInfo 扩展方法 (Roofline 相关)
// ============================================================================

impl NodeInfo {
    /// 清除服务状态
    pub fn clear_serving_state(&mut self) {
        self.start_layer = None;
        self.end_layer = None;
        self.current_requests = 0;
        self.avg_layer_latency_ms = None;
    }

    /// 设置层延迟测量值
    pub fn set_layer_latency_ms(&mut self, latency_ms: f64) {
        self.avg_layer_latency_ms = Some(latency_ms);
    }

    /// 添加请求
    pub fn add_request(&mut self) {
        self.current_requests += 1;
    }

    /// 移除请求
    pub fn remove_request(&mut self) {
        self.current_requests = self.current_requests.saturating_sub(1);
    }

    /// 每层 KV Cache 内存 (字节)
    pub fn per_decoder_layer_kv_cache(&self) -> Option<u64> {
        if self.num_current_layers() == 0 {
            return None;
        }

        let total_kv_bytes = (self.hardware.memory_gb * 1024.0 * 1024.0 * 1024.0 * self.kvcache_mem_ratio) as u64;
        Some(total_kv_bytes / self.num_current_layers() as u64)
    }

    /// 解码器层容量 (能装多少层)
    pub fn get_decoder_layer_capacity(&self, include_input_embed: bool, include_lm_head: bool) -> u32 {
        let available_bytes = (self.hardware.memory_gb * 1024.0 * 1024.0 * 1024.0 * self.param_mem_ratio) as u64;

        let embedding_reserve = if include_input_embed {
            // Embedding 参数: vocab_size * hidden_dim * bytes_per_element
            // 典型的 7B 模型: 32000 * 4096 * 2 ≈ 256 MB
            256_000_000u64
        } else {
            0
        };

        let lm_head_reserve = if include_lm_head {
            // LM Head: vocab_size * hidden_dim * bytes
            256_000_000u64
        } else {
            0
        };

        let usable = available_bytes.saturating_sub(embedding_reserve + lm_head_reserve);

        // 每层参数大小
        let bytes_per_layer = match self.hardware.device_type.as_str() {
            "mlx" => 50_000_000,  // Apple Silicon 量化
            _ => 100_000_000,      // FP16
        };

        (usable / bytes_per_layer) as u32
    }

    /// 更新 RTT 到另一个节点
    pub fn update_rtt_to(&mut self, target: &NodeId, rtt_ms: f64) {
        self.rtt_to_nodes.insert(*target, rtt_ms);
    }

    /// Roofline 模型估算单层延迟 (ms)
    ///
    /// Roofline Model:
    /// ```
    /// latency = max(compute_bound, io_bound)
    /// compute_bound = decoder_FLOPs / TFLOPS
    /// io_bound = decoder_IO_bytes / bandwidth
    /// ```
    pub fn roofline_layer_latency_ms(&self) -> f64 {
        // 典型 Transformer 层参数 (以 7B 为基准)
        // FLOPs: ~2 * hidden_dim^2 * seq_len * batch_size (近似)
        let flops_per_layer = 4_000_000_000f64;  // 4 GFLOPs (典型 7B 层, seq=512, batch=1)
        let io_bytes_per_layer = 2_000_000f64;    // 2 MB (激活值)

        let compute_bound = flops_per_layer / (self.hardware.tflops_fp16 * 1e9);
        let io_bound = io_bytes_per_layer / (self.hardware.memory_bandwidth_gbps * 1e6);

        // 取两者较大者
        compute_bound.max(io_bound) * 1000.0 // 转换为 ms
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_gpu_node(name: &str) -> NodeHardwareInfo {
        NodeHardwareInfo::gpu(name, 1, 80.0, 24.0, 900.0)
    }

    #[tokio::test]
    async fn test_register_and_unregister() {
        let mut mgr = NodeManager::new();
        let hw = create_gpu_node("RTX-4090");

        let id = mgr.register_node(hw).await.unwrap();
        assert_eq!(mgr.total_registered.load(Ordering::Relaxed), 1);
        assert_eq!(mgr.active_nodes().len(), 1);

        mgr.unregister_node(&id).await.unwrap();
        assert_eq!(mgr.active_nodes().len(), 0); // 离线节点不算 active
        assert_eq!(mgr.total_unregistered.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_heartbeat() {
        let mut mgr = NodeManager::new();
        let hw = create_gpu_node("RTX-4090");

        let id = mgr.register_node(hw).await.unwrap();

        // 正常心跳
        mgr.update_heartbeat(&id, Some(5.5)).await.unwrap();
        let node = mgr.get_node(&id).unwrap();
        assert_eq!(node.avg_layer_latency_ms, Some(5.5));

        // 多次心跳
        for i in 0..10 {
            mgr.update_heartbeat(&id, Some(5.0 + i as f64 * 0.1)).await.unwrap();
        }
        assert_eq!(mgr.total_heartbeats.load(Ordering::Relaxed), 11); // 含第一次 register
    }

    #[tokio::test]
    async fn test_cluster_summary() {
        let mut mgr = NodeManager::new();

        let hw1 = create_gpu_node("RTX-4090");  // 80 TFLOPS, 24GB
        let hw2 = create_gpu_node("RTX-3090");  // 71 TFLOPS, 24GB
        let hw3 = NodeHardwareInfo::gpu("M2-Ultra", 1, 27.0, 192.0, 800.0); // Apple Silicon

        mgr.register_node(hw1).await.ok();
        mgr.register_node(hw2).await.ok();
        mgr.register_node(hw3).await.ok();

        let summary = mgr.cluster_summary();
        assert_eq!(summary.total_nodes, 3);
        assert_eq!(summary.active_nodes, 3);
        assert!(summary.total_tflops > 170.0); // 80+71+27
        assert!(summary.total_memory_gb > 230.0); // 24+24+192
    }

    #[test]
    fn test_roofline_model() {
        let node = NodeInfo {
            node_id: uuid::Uuid::new_v4(),
            hardware: create_gpu_node("H100"),
            status: NodeStatus::Active,
            start_layer: Some(0),
            end_layer: Some(10),
            current_requests: 0,
            max_requests: 16,
            avg_layer_latency_ms: None,
            last_heartbeat: chrono::Utc::now(),
            rtt_to_nodes: std::collections::HashMap::new(),
            kvcache_mem_ratio: 0.3,
            param_mem_ratio: 0.5,
        };

        let roofline_lat = node.roofline_layer_latency_ms();
        assert!(roofline_lat > 0.0);
        assert!(roofline_lat < 100.0, "单层延迟应在合理范围内 (<100ms)");
        println!("Roofline 估算延迟: {:.3} ms/层", roofline_lat);

        // 有效延迟 (含负载补偿)
        assert_eq!(node.effective_layer_latency_ms(), roofline_lat); // 无负载时应等于 roofline

        // 模拟过载
        let overloaded = NodeInfo {
            current_requests: 100,
            max_requests: 1,
            ..node.clone()
        };
        assert!(overloaded.is_overloaded());
        assert_eq!(overloaded.effective_layer_latency_ms(), f64::INFINITY);
    }

    #[test]
    fn test_capacity_estimation() {
        let node = NodeInfo {
            node_id: uuid::Uuid::new_v4(),
            hardware: create_gpu_node("RTX-4090"), // 24 GB
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
        };

        // 基础容量 (不含 endpoint)
        let base_cap = node.get_decoder_layer_capacity(false, false);
        println!("基础容量: {} 层", base_cap);
        assert!(base_cap > 0);

        // 首节点 (需 Input Embedding)
        let first_cap = node.get_decoder_layer_capacity(true, false);
        assert!(first_cap <= base_cap, "首节点容量应 ≤ 基础容量");

        // 尾节点 (需 LM Head)
        let last_cap = node.get_decoder_layer_capacity(false, true);
        assert!(last_cap <= base_cap, "尾节点容量应 ≤ 基础容量");

        // 同时首尾 (极端情况)
        let both_cap = node.get_decoder_layer_capacity(true, true);
        assert!(both_cap <= first_cap.min(last_cap));
    }

    #[tokio::test]
    async fn test_heartbeat_expiry() {
        let mut mgr = NodeManager::new();
        mgr.set_heartbeat_timeout(0); // 立即超时

        let hw = create_gpu_node("Test");
        let id = mgr.register_node(hw).await.unwrap();

        // 心跳检查
        let expired = mgr.check_heartbeats().await;
        assert!(!expired.is_empty(), "应检测到过期节点");
        assert!(expired.contains(&id));

        let node = mgr.get_node(&id).unwrap();
        assert_eq!(node.status, NodeStatus::Offline);
    }
}
