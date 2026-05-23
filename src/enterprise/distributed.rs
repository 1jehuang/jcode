//! ## 任务 1.3: 分布式推理适配（基于 Parallax）
//!
//! 本模块将 Parallax 的两阶段调度能力适配到企业版，实现：
//! 1. **模型层拆分**: 将 72B 模型的 Transformer 层拆分到多台设备
//! 2. **流水线并行**: 多节点组成推理流水线，按层分配
//! 3. **动态节点加入/离开**: 适配网吧闲置电脑的动态上下线场景
//!
//! ### 复用已集成的 Parallax 能力
//!
//! - `jcode-unified-scheduler::layer_allocator`: 层分配器（水填+贪心/DP）
//! - `jcode-unified-scheduler::request_router`: 请求路由（DP动态规划）
//! - `jcode-unified-scheduler::resource_node`: 节点管理
//! - `jcode-unified-scheduler::water_filling`: 水填负载均衡

use crate::enterprise::config::EnterpriseConfig;
use jcode_unified_scheduler::{
    layer_allocator::LayerAllocator,
    request_router::RequestRouter,
    resource_node::NodeManager,
    water_filling::WaterFilling,
    types::*,
    AllocationStrategy, SchedulerConfig, SchedulerError, RoutingStrategy,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// 分布式推理调度器 — 封装 Parallax 两阶段逻辑
pub struct DistributedInferenceScheduler {
    /// Parallax 节点管理器
    node_manager: Arc<RwLock<NodeManager>>,
    /// Parallax Phase 1: 层分配器
    layer_allocator: Arc<RwLock<Option<LayerAllocator>>>,
    /// Parallax Phase 2: 请求路由器
    request_router: Arc<RwLock<Option<RequestRouter>>>,
    /// 注水算法
    water_filling: Arc<WaterFilling>,
    /// 配置
    config: Arc<EnterpriseConfig>,
    /// 已注册的节点数量
    node_count: Arc<std::sync::atomic::AtomicU32>,
}

impl DistributedInferenceScheduler {
    pub fn new(config: Arc<EnterpriseConfig>) -> Self {
        let scheduler_config = SchedulerConfig {
            allocation_strategy: match config.scheduling.allocation_strategy.as_str() {
                "greedy" => AllocationStrategy::Greedy,
                _ => AllocationStrategy::DynamicProgramming,
            },
            routing_strategy: match config.scheduling.routing_strategy.as_str() {
                "random" => RoutingStrategy::Randomized,
                "round_robin" => RoutingStrategy::RoundRobin,
                _ => RoutingStrategy::DynamicProgramming,
            },
            heartbeat_timeout_secs: config.scheduling.heartbeat_timeout_secs,
            min_bootstrap_nodes: config.scheduling.min_bootstrap_nodes,
            enable_goap: config.scheduling.enable_goap,
            ..SchedulerConfig::default()
        };

        Self {
            node_manager: Arc::new(RwLock::new(NodeManager::new())),
            layer_allocator: Arc::new(RwLock::new(None)),
            request_router: Arc::new(RwLock::new(None)),
            water_filling: Arc::new(WaterFilling::new(40, 0.001)),
            config,
            node_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }
    }

    /// 注册一个推理节点
    ///
    /// 对应场景：
    /// - 一台 128G 台式机：1 CPU 节点，128GB 内存
    /// - 一台网吧空闲电脑：1 CPU 节点，512GB 虚拟内存
    /// - 一台员工笔记本：1 CPU 节点，16-32GB 内存
    pub async fn register_node(
        &self,
        node_name: &str,
        memory_gb: f64,
        cpu_cores: u32,
        has_gpu: bool,
        vram_gb: f64,
    ) -> anyhow::Result<NodeId> {
        let mut manager = self.node_manager.write().await;

        let hardware_info = if has_gpu && vram_gb > 0.0 {
            NodeHardwareInfo {
                node_id: uuid::Uuid::new_v4(),
                num_gpus: 1,
                gpu_name: "CPU+GPU Hybrid".into(),
                tflops_fp16: vram_gb * 3.0, // 粗估
                memory_gb,
                memory_bandwidth_gbps: if memory_gb > 100.0 { 50.0 } else { 25.0 },
                device_type: "cpu".into(),
            }
        } else {
            // CPU-only 节点
            NodeHardwareInfo::cpu(node_name, cpu_cores, memory_gb)
        };

        let node_id = manager.register_node(hardware_info).await
            .map_err(|e| anyhow::anyhow!("注册节点失败: {}", e))?;

        self.node_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        info!("已注册节点: {} (id={}, memory={}GB, cpu={}核)", node_name, node_id, memory_gb, cpu_cores);

        // 如果启用了自适应调度，触发增量重平衡
        self.trigger_incremental_rebalance().await;

        Ok(node_id)
    }

    /// 注销节点
    pub async fn unregister_node(&self, node_id: NodeId) -> anyhow::Result<()> {
        let mut manager = self.node_manager.write().await;
        manager.unregister_node(&node_id).await
            .map_err(|e| anyhow::anyhow!("注销节点失败: {}", e))?;
        self.node_count.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        info!("节点已注销: {}", node_id);
        Ok(())
    }

    /// 触发增量重平衡（新节点加入时）
    async fn trigger_incremental_rebalance(&self) {
        let allocator = self.layer_allocator.read().await;
        if let Some(ref alloc) = *allocator {
            let nodes = {
                let mgr = self.node_manager.read().await;
                mgr.active_node_list()
            };
            if alloc.should_rebalance(&nodes).unwrap_or(false) {
                info!("触发全局重平衡...");
            }
        }
    }

    /// 为指定模型分配层到所有注册节点
    ///
    /// 核心逻辑：将模型的 num_layers 层分配到多台设备，
    /// 按可用内存比例分配（水填算法）。
    pub async fn allocate_model_layers(
        &self,
        model_name: &str,
        num_layers: u32,
    ) -> anyhow::Result<Vec<(NodeId, u32)>> {
        let nodes = {
            let mgr = self.node_manager.read().await;
            mgr.active_node_list().into_iter().map(|n| {
                (n.node_id, n.hardware.memory_gb, n.hardware.tflops_fp16)
            }).collect::<Vec<_>>()
        };

        if nodes.is_empty() {
            anyhow::bail!("没有可用的推理节点");
        }

        // 使用水填算法分配层
        let capacities: Vec<f64> = nodes.iter().map(|(_, mem, _)| {
            // 每 8GB 放 1 层（经验公式，根据实际测试调整）
            (mem / 8.0).ceil().min(num_layers as f64)
        }).collect();

        let powers: Vec<f64> = nodes.iter().map(|(_, _, tflops)| {
            tflops.max(0.1)
        }).collect();

        let result = self.water_filling.allocate(&capacities, &powers, num_layers as f64);

        let allocations: Vec<(NodeId, u32)> = nodes.iter().zip(result.allocations.iter())
            .map(|((node_id, _, _), &layers)| (*node_id, layers as u32))
            .collect();

        info!(
            "模型 '{}' 层分配完成: {} 层分配给 {} 个节点",
            model_name, num_layers, allocations.len()
        );

        for (node_id, layers) in &allocations {
            debug!("  节点 {} -> {} 层", node_id, layers);
        }

        Ok(allocations)
    }

    /// 获取集群资源摘要
    pub async fn get_cluster_summary(&self) -> anyhow::Result<ClusterResourceSummary> {
        let mgr = self.node_manager.read().await;
        Ok(mgr.cluster_summary())
    }

    /// 获取活跃节点数
    pub fn node_count(&self) -> u32 {
        self.node_count.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 判断是否可以使用分布式推理
    pub async fn route_request(&self, model: &str, layers: u32) -> anyhow::Result<InferenceRoute> {
        let a = self.allocate_model_layers(model, layers).await?;
        let nodes = { let m = self.node_manager.read().await; m.active_node_list_arc() };
        let r = self.request_router.read().await;
        let target = match r.as_ref() {
            Some(rr) => {
                match rr.find_optimal_path(layers as u32, &nodes) {
                    Ok(Some((path, _))) => path.last().copied(),
                    _ => nodes.first().map(|n| n.node_id),
                }
            }
            None => nodes.first().map(|n| n.node_id),
        };
        Ok(InferenceRoute { model_name: model.into(), target_node: target, layer_assignments: a, total_layers: layers })
    }

    pub fn can_use_distributed(&self) -> bool {
        self.node_count() >= 2
    }

    /// 动态负载均衡：根据实时负载重新分配任务
    pub async fn dynamic_load_balance(&self, current_route: &InferenceRoute) -> anyhow::Result<Option<InferenceRoute>> {
        let nodes = { let m = self.node_manager.read().await; m.active_node_list_arc() };

        if let Some(target_id) = current_route.target_node {
            if let Some(target_node) = nodes.iter().find(|n| n.node_id == target_id) {
                let load_ratio = target_node.current_requests as f64 / target_node.max_requests as f64;

                if load_ratio > 0.85 {
                    tracing::warn!(
                        "[LoadBalance] 节点{}负载过高({:.0}%)，寻找替代节点",
                        target_id,
                        load_ratio * 100.0
                    );

                    if let Some(lightest) = nodes.iter()
                        .filter(|n| !n.is_overloaded())
                        .min_by(|a, b| {
                            let load_a = a.current_requests as f64 / a.max_requests as f64;
                            let load_b = b.current_requests as f64 / b.max_requests as f64;
                            load_a.partial_cmp(&load_b).unwrap_or(std::cmp::Ordering::Equal)
                        })
                    {
                        tracing::info!(
                            "[LoadBalance] 切换到轻负载节点{} (负载{:.0}%)",
                            lightest.node_id,
                            lightest.current_requests as f64 / lightest.max_requests as f64 * 100.0
                        );

                        return Ok(Some(InferenceRoute {
                            model_name: current_route.model_name.clone(),
                            target_node: Some(lightest.node_id),
                            layer_assignments: current_route.layer_assignments.clone(),
                            total_layers: current_route.total_layers,
                        }));
                    }
                }
            }
        }

        Ok(None)
    }

    /// 故障转移：当节点失效时自动切换到备用节点
    pub async fn failover(&self, failed_node_id: jcode_unified_scheduler::NodeId) -> anyhow::Result<Option<InferenceRoute>> {
        let nodes = { let m = self.node_manager.read().await; m.active_node_list() };
        
        // 排除故障节点，选择下一个最佳节点
        let backup = nodes.iter()
            .filter(|n| n.node_id != failed_node_id && !n.is_overloaded())
            .max_by(|a, b| {
                let score_a = a.hardware.tflops_fp16 / (1.0 + a.current_requests as f64);
                let score_b = b.hardware.tflops_fp16 / (1.0 + b.current_requests as f64);
                score_a.partial_cmp(&score_b).unwrap_or(std::cmp::Ordering::Equal)
            });
        
        match backup {
            Some(node) => {
                tracing::warn!(
                    "[Failover] 从故障节点{}切换到备用节点{}",
                    failed_node_id,
                    node.node_id
                );
                
                Ok(Some(InferenceRoute {
                    model_name: "unknown".to_string(), // 需要调用方提供
                    target_node: Some(node.node_id),
                    layer_assignments: vec![],
                    total_layers: 0,
                }))
            }
            None => {
                tracing::error!("[Failover] 无可用备用节点！");
                Err(anyhow::anyhow!("无可用备用节点"))
            }
        }
    }

    /// 健康检查：定期检测节点状态
    pub async fn health_check_nodes(&self) -> Vec<NodeHealthStatus> {
        let nodes = { let m = self.node_manager.read().await; m.active_node_list() };
        let now = chrono::Utc::now();
        
        nodes.iter().map(|node| {
            let time_since_heartbeat = now.signed_duration_since(node.last_heartbeat);
            let is_healthy = time_since_heartbeat.num_seconds() < 30; // 30秒超时
            
            NodeHealthStatus {
                node_id: node.node_id,
                is_healthy,
                latency_ms: node.avg_layer_latency_ms.unwrap_or(0.0),
                load_ratio: node.current_requests as f64 / node.max_requests as f64,
                last_heartbeat: node.last_heartbeat,
            }
        }).collect()
    }
}

/// 节点健康状态
#[derive(Debug, Clone)]
pub struct NodeHealthStatus {
    pub node_id: jcode_unified_scheduler::NodeId,
    pub is_healthy: bool,
    pub latency_ms: f64,
    pub load_ratio: f64,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug,Clone)]
pub struct InferenceRoute {
    pub model_name: String,
    pub target_node: Option<jcode_unified_scheduler::NodeId>,
    pub layer_assignments: Vec<(jcode_unified_scheduler::NodeId, u32)>,
    pub total_layers: u32,
}
