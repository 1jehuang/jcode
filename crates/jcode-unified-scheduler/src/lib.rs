//! **Ruflo-Parallax 统一调度器** — JCode 的任务调度与算力调度融合引擎。
//!
//! ## 架构概览
//!
//! ```text
//! +----------------------------------------------------------+
//! |              UnifiedScheduler (统一调度器)                 |
//! |                                                          |
//! |  +-------------+  +-------------+  +--------------+     |
//! |  | TaskScheduler|  | ComputeScheduler|  | StateManager  |     |
//! |  | (Ruflo GOAP) |  |(Parallax DP) |  |  (统一状态)    |     |
//! |  +------+------+  +------+------+  +------+-------+     |
//! |         |                |                |              |
//! |         +--------+-------+----------------+              |
//! |                  ▼                                      |
//! |  +-----------------------------------------------+      |
//! |  |           UnifiedSchedulingQueue               |      |
//! |  |  +------+ +------+ +------+ +------+        |      |
//! |  |  |TaskA | |TaskB | |TaskC | |TaskD | ...     |      |
//! |  |  |7B/H  | |3B/M  | |1.5B/L| |14B/H |        |      |
//! |  |  +------+ +------+ +------+ +------+        |      |
//! |  +-----------------------------------------------+      |
//! +----------------------------------------------------------+
//! ```
//!
//! ## 模块说明
//!
//! | 模块 | 来源 | 功能 |
//! |------|------|------|
//! | `goap_planner` | Ruflo | GOAP A* 目标导向动作规划 |
//! | `layer_allocator` | Parallax Phase 1 | 模型层分配（贪心/DP/注水） |
//! | `request_router` | Parallax Phase 2 | DP 请求路由 |
//! | `resource_node` | Parallax | 节点管理 + Roofline 性能模型 |
//! | `unified_queue` | 原创 | 统一调度队列 |
//! | `water_filling` | Parallax | 注水负载均衡算法 |
//! | `types` | 融合 | 三源统一类型系统 |

pub mod types;
pub mod goap_planner;
pub mod layer_allocator;
pub mod request_router;
pub mod resource_node;
pub mod gpu_load_balancer;
pub mod gpu_discovery;
pub mod unified_queue;
pub mod water_filling;
pub mod topology_aware;
pub mod resource_tracker;
pub mod node_join_manager;
pub mod cross_region;
pub mod hierarchical_scheduler;
pub mod batch_node_operations;
pub mod gslb;
#[cfg(feature = "cross-region-sync")]
pub mod cross_region_sync;
#[cfg(feature = "cross-region-sync")]
pub mod conflict_resolution;

// 重导出核心类型 — 方便外部使用
pub use types::*;
pub use resource_node::*;
pub use unified_queue::UnifiedQueue;
pub use topology_aware::{HardwareTopology, TopologyAwareScheduler, NumaNode, GpuInfo};
pub use resource_tracker::{ResourceManager, ResourceRequirement, NodeResourceState, AllocationId};
pub use node_join_manager::{NodeJoinManager, NodeJoinState, ProbeResult, WarmupConfig};
pub use cross_region::{RegionManager, Region, Zone, RegionSummary, RoutingConfig, RoutingDecision};
pub use hierarchical_scheduler::{HierarchicalScheduler, ClusterGroup, ClusterGroupId, ClusterGroupType, HierarchicalSchedulerConfig};
pub use batch_node_operations::{BatchNodeManager, BatchOperationConfig, BatchOperationStatus};
pub use gslb::{GslbRouter, RegionalCluster, GslbStrategy, HealthStatus, ClientLocation};
#[cfg(feature = "cross-region-sync")]
pub use cross_region_sync::{CrossRegionReplicator, GossipProtocol, VectorClock, LwwRegister, OrSet, GSet, StateStore};
#[cfg(feature = "cross-region-sync")]
pub use conflict_resolution::{PNCounter, LwwMap, MVRegister, ConflictAwareSession, ResolutionStrategy, MergeResult};

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use dashmap::DashMap;
use indexmap::IndexMap;
use tokio::sync::{RwLock, Notify};
use tracing::{info, warn, debug, error, instrument};
use uuid::Uuid;

// ============================================================================
// UnifiedScheduler — 统一调度器主结构体
// ============================================================================

/// Ruflo-Parallax 统一调度器
///
/// 同时考虑:
/// - **任务需求**: 角色、模型大小、优先级、依赖关系 (来自 Ruflo)
/// - **算力资源**: CPU/GPU/内存/网络容量与实时负载 (来自 Parallax)
///
/// 做出全局最优的调度决策。
#[derive(Debug, Default)]
pub struct UnifiedScheduler {
    /// 全局唯一实例 ID
    pub id: uuid::Uuid,

    /// === Ruflo 子系统 ===
    /// GOAP 规划器 (A* 目标分解)
    goap_planner: Arc<RwLock<goap_planner::GoapPlanner>>,

    /// 已注册的任务定义库
    task_registry: Arc<DashMap<TaskId, ScheduledTask>>,

    /// DAG 依赖图 (邻接表): task_id -> [dependent_task_ids]
    dependency_graph: Arc<RwLock<IndexMap<TaskId, Vec<TaskId>>>>,

    /// === Parallax 子系统 ===
    /// 算力节点管理器
    node_manager: Arc<RwLock<resource_node::NodeManager>>,

    /// Phase 1: 层分配器
    layer_allocator: Arc<RwLock<Option<layer_allocator::LayerAllocator>>>,

    /// Phase 2: 请求路由器
    request_router: Arc<RwLock<Option<request_router::RequestRouter>>>,

    /// === 统一队列 ===
    /// 调度队列 (优先级排序)
    queue: Arc<RwLock<unified_queue::UnifiedQueue>>,

    /// === 配置 ===
    config: SchedulerConfig,

    /// 运行时状态
    state: Arc<RwLock<SchedulerState>>,

    /// 通知信号 — 新任务入队或资源变化时唤醒调度循环
    notify: Arc<Notify>,

    /// 统计指标 (使用原子操作和DashMap减少锁竞争)
    metrics: Arc<SchedulerMetrics>,

    /// === GPU推理子系统 ===
    /// GPU负载均衡器 (可选，仅在GPU可用时启用)
    gpu_balancer: Arc<RwLock<Option<gpu_load_balancer::GpuLoadBalancer>>>,
}

/// 调度器配置
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    // --- 队列配置 ---
    /// 最大并发执行任务数
    pub max_concurrent_tasks: usize,
    /// 队列最大长度 (0 = 无限)
    pub max_queue_size: usize,
    /// 单个任务最大等待时间 (毫秒), 超过则降级或拒绝
    pub max_wait_time_ms: u64,

    // --- GOAP (Ruflo) 配置 ---
    /// 启用 GOAP 规划器
    pub enable_goap: bool,
    /// 最大规划迭代次数
    pub max_planning_iterations: usize,
    /// 启发式函数权重 (越高越倾向于目标导向)
    pub heuristic_weight: f64,

    // --- Parallax Phase 1 配置 ---
    /// 分配策略: "greedy" | "dp"
    pub allocation_strategy: AllocationStrategy,
    /// 负载不均衡阈值 (变异系数 CV), 触发重平衡
    pub rebalance_threshold: f64,
    /// 注水算法最大迭代次数
    pub water_filling_max_iterations: usize,

    // --- Parallax Phase 2 配置 ---
    /// 路由策略: "dp" | "random" | "round_robin"
    pub routing_strategy: RoutingStrategy,
    /// 是否启用预热裁剪 (turning points)
    pub enable_warmup_trim: bool,

    // --- 资源管理 ---
    /// 心跳超时 (秒), 超过此时间的节点标记为离线
    pub heartbeat_timeout_secs: u64,
    /// 最小引导节点数 (至少需要多少节点才能开始调度)
    pub min_bootstrap_nodes: usize,

    // --- 自适应 ---
    /// 启用自适应策略切换
    pub adaptive_scheduling: bool,
    /// 性能采样窗口 (秒)
    pub performance_window_secs: u64,

    // --- GPU调度配置 ---
    /// GPU负载均衡策略: "balanced" | "latency" | "throughput" | "power"
    pub gpu_balance_strategy: String,
    /// 启用GPU推理 (如果硬件可用)
    pub enable_gpu_inference: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_tasks: 16,
            max_queue_size: 0,
            max_wait_time_ms: 30000,
            enable_goap: true,
            max_planning_iterations: 100,
            heuristic_weight: 1.5,
            allocation_strategy: AllocationStrategy::DynamicProgramming,
            rebalance_threshold: 0.3,
            water_filling_max_iterations: 40,
            routing_strategy: RoutingStrategy::DynamicProgramming,
            enable_warmup_trim: false,
            heartbeat_timeout_secs: 30,
            min_bootstrap_nodes: 1,
            adaptive_scheduling: true,
            performance_window_secs: 60,
            gpu_balance_strategy: "balanced".to_string(),
            enable_gpu_inference: true,
        }
    }
}

/// 分配策略枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AllocationStrategy {
    /// 贪心: 优先构建长流水线, 最小化阶段数 (快但不一定最优)
    Greedy,
    /// 动态规划: 平衡并发流水线数和延迟 (慢但更优)
    DynamicProgramming,
}

/// 路由策略枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RoutingStrategy {
    /// 动态规划路由: 每次请求实时计算最优路径 (推荐)
    DynamicProgramming,
    /// 随机选择可用路径 (用于基准测试)
    Randomized,
    /// 固定流水线轮询 (低开销但可能不均衡)
    RoundRobin,
}

/// 调度器状态机
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SchedulerState {
    /// 初始化中
    #[default]
    Initializing,
    /// 就绪, 等待任务
    Idle,
    /// 正在调度/执行
    Running,
    /// 暂停 (不再接受新任务, 但正在执行的不中断)
    Paused,
    /// 关闭中 (完成所有正在执行的任务后关闭)
    ShuttingDown,
    /// 已关闭
    Shutdown,
}

/// 调度器性能指标 (原子操作版本，无锁)
#[derive(Debug, Default)]
pub struct SchedulerMetrics {
    // --- 任务统计 ---
    /// 总提交任务数
    pub tasks_submitted: AtomicU64,
    /// 总完成任务数
    pub tasks_completed: AtomicU64,
    /// 总失败任务数
    pub tasks_failed: AtomicU64,
    /// 总取消任务数
    pub tasks_cancelled: AtomicU64,
    /// 当前队列中的任务数
    pub queue_length: AtomicU64,
    /// 当前正在执行的任务数
    pub running_count: AtomicU64,

    // --- 调度延迟 (微秒) ---
    /// 平均调度决策时间 (从入队到开始执行)
    pub avg_schedule_latency_us: AtomicU64,
    /// P99 调度延迟
    pub p99_schedule_latency_us: AtomicU64,
    /// 上一次调度延迟
    pub last_schedule_latency_us: AtomicU64,

    // --- 资源利用率 ---
    /// 平均 CPU 利用率 (0-10000, 表示 0%-100%)
    pub avg_cpu_utilization: AtomicU32,
    /// 平均 GPU 利用率 (0-10000)
    pub avg_gpu_utilization: AtomicU32,
    /// 平均内存利用率 (0-10000)
    pub avg_memory_utilization: AtomicU32,

    // --- Parallax 特有 ---
    /// Phase 1 (层分配) 执行次数
    pub phase1_allocations: AtomicU64,
    /// Phase 2 (请求路由) 执行次数
    pub phase2_routings: AtomicU64,
    /// 全局重平衡触发次数
    pub global_rebalances: AtomicU64,
    /// 平均流水线数量 (使用f64的原子表示，需要unsafe或Mutex)
    pub avg_pipeline_count: std::sync::RwLock<f64>,

    // --- GOAP 特有 ---
    /// GOAP 规划次数
    pub goap_plans_generated: AtomicU64,
    /// 平均规划耗时 (毫秒)
    pub avg_plan_time_ms: std::sync::RwLock<f64>,
    /// 规划失败次数
    pub goap_plan_failures: AtomicU64,

    // --- 时间戳 ---
    /// 指标采集时间
    pub collected_at: chrono::DateTime<chrono::Utc>,
}

impl SchedulerMetrics {
    /// Clone metrics snapshot for serialization/reporting
    pub fn snapshot(&self) -> SchedulerMetricsSnapshot {
        SchedulerMetricsSnapshot {
            tasks_submitted: self.tasks_submitted.load(Ordering::Relaxed),
            tasks_completed: self.tasks_completed.load(Ordering::Relaxed),
            tasks_failed: self.tasks_failed.load(Ordering::Relaxed),
            tasks_cancelled: self.tasks_cancelled.load(Ordering::Relaxed),
            queue_length: self.queue_length.load(Ordering::Relaxed),
            running_count: self.running_count.load(Ordering::Relaxed),
            avg_schedule_latency_us: self.avg_schedule_latency_us.load(Ordering::Relaxed),
            p99_schedule_latency_us: self.p99_schedule_latency_us.load(Ordering::Relaxed),
            last_schedule_latency_us: self.last_schedule_latency_us.load(Ordering::Relaxed),
            avg_cpu_utilization: self.avg_cpu_utilization.load(Ordering::Relaxed),
            avg_gpu_utilization: self.avg_gpu_utilization.load(Ordering::Relaxed),
            avg_memory_utilization: self.avg_memory_utilization.load(Ordering::Relaxed),
            phase1_allocations: self.phase1_allocations.load(Ordering::Relaxed),
            phase2_routings: self.phase2_routings.load(Ordering::Relaxed),
            global_rebalances: self.global_rebalances.load(Ordering::Relaxed),
            avg_pipeline_count: *self.avg_pipeline_count.read().unwrap(),
            goap_plans_generated: self.goap_plans_generated.load(Ordering::Relaxed),
            avg_plan_time_ms: *self.avg_plan_time_ms.read().unwrap(),
            goap_plan_failures: self.goap_plan_failures.load(Ordering::Relaxed),
            collected_at: self.collected_at,
        }
    }
}

/// Serializable snapshot of scheduler metrics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SchedulerMetricsSnapshot {
    pub tasks_submitted: u64,
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub tasks_cancelled: u64,
    pub queue_length: u64,
    pub running_count: u64,
    pub avg_schedule_latency_us: u64,
    pub p99_schedule_latency_us: u64,
    pub last_schedule_latency_us: u64,
    pub avg_cpu_utilization: u32,
    pub avg_gpu_utilization: u32,
    pub avg_memory_utilization: u32,
    pub phase1_allocations: u64,
    pub phase2_routings: u64,
    pub global_rebalances: u64,
    pub avg_pipeline_count: f64,
    pub goap_plans_generated: u64,
    pub avg_plan_time_ms: f64,
    pub goap_plan_failures: u64,
    #[serde(with = "chrono::serde::ts_seconds")]
    pub collected_at: chrono::DateTime<chrono::Utc>,
}

impl UnifiedScheduler {
    /// 创建新的统一调度器实例
    pub async fn new(config: SchedulerConfig) -> Result<Self, SchedulerError> {
        let scheduler_id = uuid::Uuid::new_v4();
        info!("[UnifiedScheduler] 创建新实例 id={}", scheduler_id);

        let scheduler = Self {
            id: scheduler_id,
            goap_planner: Arc::new(RwLock::new(goap_planner::GoapPlanner::new())),
            task_registry: Arc::new(DashMap::new()),
            dependency_graph: Arc::new(RwLock::new(IndexMap::new())),
            node_manager: Arc::new(RwLock::new(NodeManager::new())),
            layer_allocator: Arc::new(RwLock::new(None)),
            request_router: Arc::new(RwLock::new(None)),
            queue: Arc::new(RwLock::new(unified_queue::UnifiedQueue::new(
                config.max_queue_size,
            ))),
            config,
            state: Arc::new(RwLock::new(SchedulerState::Initializing)),
            notify: Arc::new(Notify::new()),
            metrics: Arc::new(SchedulerMetrics {
                collected_at: chrono::Utc::now(),
                ..Default::default()
            }),
            gpu_balancer: Arc::new(RwLock::new(None)),  // Will be initialized if GPU available
        };

        // 初始化子系统
        scheduler.init_subsystems().await?;

        // 尝试初始化GPU负载均衡器 (非阻塞，失败不影响主流程)
        scheduler.try_init_gpu_balancer().await;

        Ok(scheduler)
    }

    /// 尝试初始化GPU负载均衡器 (可选功能)
    async fn try_init_gpu_balancer(&self) {
        use gpu_load_balancer::{GpuTopology, GpuLoadBalancer, GpuLoadBalanceStrategy};

        match GpuTopology::discover() {
            Ok(topology) => {
                let strategy = match self.config.gpu_balance_strategy.as_str() {
                    "latency" => GpuLoadBalanceStrategy::LatencyOptimized,
                    "throughput" => GpuLoadBalanceStrategy::ThroughputOptimized,
                    "power" => GpuLoadBalanceStrategy::PowerOptimized,
                    _ => GpuLoadBalanceStrategy::Balanced,
                };

                let balancer = GpuLoadBalancer::new(topology, strategy);
                *self.gpu_balancer.write().await = Some(balancer);
                info!("[UnifiedScheduler] GPU load balancer initialized");
            }
            Err(e) => {
                info!("[UnifiedScheduler] GPU not available ({}), running CPU-only mode", e);
            }
        }
    }

    /// 初始化各子系统 (GOAP + Parallax)
    async fn init_subsystems(&self) -> Result<(), SchedulerError> {
        info!("[UnifiedScheduler] 初始化子系统中...");

        // 1. 初始化 GOAP 规划器 (Ruflo)
        if self.config.enable_goap {
            let mut planner = self.goap_planner.write().await;
            planner.set_max_iterations(self.config.max_planning_iterations);
            planner.set_heuristic_weight(self.config.heuristic_weight);
            info!(
                "[UnifiedScheduler] GOAP 规划器已启用 (max_iter={}, weight={})",
                self.config.max_planning_iterations, self.config.heuristic_weight
            );
        }

        // 2. 初始化 Parallax 层分配器
        {
            let mut allocator = self.layer_allocator.write().await;
            *allocator = Some(layer_allocator::LayerAllocator::new(
                self.config.allocation_strategy,
                self.config.rebalance_threshold,
                self.config.water_filling_max_iterations,
            ));
        }

        // 3. 初始化请求路由器
        {
            let mut router = self.request_router.write().await;
            let routing_enum = match self.config.routing_strategy {
                RoutingStrategy::DynamicProgramming => request_router::RoutingStrategyEnum::DynamicProgramming,
                RoutingStrategy::Randomized => request_router::RoutingStrategyEnum::Randomized,
                RoutingStrategy::RoundRobin => request_router::RoutingStrategyEnum::RoundRobin,
            };
            *router = Some(request_router::RequestRouter::new(
                routing_enum,
                self.config.enable_warmup_trim,
            ));
        }

        // 4. 设置状态为就绪
        {
            let mut state = self.state.write().await;
            *state = SchedulerState::Idle;
        }

        info!("[UnifiedScheduler] 所有子系统初始化完成");
        Ok(())
    }

    // ========================================================================
    // 公开 API: 任务管理 (Ruflo 接口)
    // ========================================================================

    /// 提交一个新任务到调度器
    ///
    /// 流程:
    /// 1. 验证任务合法性
    /// 2. 如果启用了 GOAP, 先进行目标分解
    /// 3. 解析依赖关系, 构建 DAG
    /// 4. 插入统一调度队列
    /// 5. 唤醒调度循环
    #[instrument(skip(self, task))]
    pub async fn submit_task(
        &self,
        mut task: ScheduledTask,
    ) -> Result<TaskId, SchedulerError> {
        // 检查状态
        let state = self.state.read().await;
        if *state == SchedulerState::Shutdown || *state == SchedulerState::ShuttingDown {
            return Err(SchedulerError::Shutdown);
        }
        drop(state);

        // 检查队列容量
        if self.config.max_queue_size > 0 {
            let queue = self.queue.read().await;
            if queue.len() >= self.config.max_queue_size {
                return Err(SchedulerError::QueueFull(queue.len()));
            }
        }

        // 分配 ID 和时间戳
        if task.id.is_nil() {
            task.id = uuid::Uuid::new_v4();
        }
        task.submitted_at = Some(chrono::Utc::now());
        task.status = TaskStatus::Queued;

        // 如果是高层目标且启用了 GOAP -> 进行自动分解
        if self.config.enable_goap && task.goal.is_some() && task.actions.is_empty() {
            debug!(
                "[UnifiedScheduler] 任务 {} 是高层目标, 触发 GOAP 规划...",
                task.id
            );
            let planner = self.goap_planner.write().await;
            match planner.plan(&task).await {
                Ok(ref plan) => { info!(
                        "[UnifiedScheduler] GOAP 规划成功: {} 个步骤",
                        plan.steps.len()
                    );
                    task.plan = Some(plan.clone());
                    // 将计划步骤转为 actions
                    task.actions = plan
                        .steps
                        .iter()
                        .map(|s| Action {
                            id: uuid::Uuid::new_v4(),
                            name: s.action_name.clone(),
                            parameters: s.params.clone(),
                            preconditions: s.preconditions.iter().map(|pc| WorldStateCondition {
                                key: pc.clone(), operator: ConditionOp::Exists, value: WorldStateValue::Bool(true)
                            }).collect(),
                            effects: s.effects.iter().map(|eff| WorldStateEffect {
                                key: eff.clone(), operation: EffectOp::Set, value: WorldStateValue::Bool(true)
                            }).collect(),
                            estimated_cost: s.estimated_cost,
                            status: ActionStatus::Pending,
                        })
                        .collect();
                }
                Err(e) => {
                    warn!("[UnifiedScheduler] GOAP 规划失败: {:?}, 使用原始任务", e);
                    self.metrics.goap_plan_failures.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        // 注册任务到 registry
        self.task_registry.insert(task.id, task.clone());

        // 更新依赖图
        if !task.dependencies.is_empty() {
            let mut dep_graph: tokio::sync::RwLockWriteGuard<'_, IndexMap<TaskId, Vec<TaskId>>> = self.dependency_graph.write().await;
            dep_graph.insert(task.id, task.dependencies.clone());
            debug!(
                "[UnifiedScheduler] 任务 {} 有 {} 个依赖",
                task.id,
                task.dependencies.len()
            );
        }

        // 提取 task.id 用于后续
        let submitted_task_id = task.id;
        let role = task.role.clone();
        let priority = task.priority;
        let required_model = task.required_model.clone();

        // 入队
        {
            let start = std::time::Instant::now();
            let mut queue = self.queue.write().await;
            queue.push(task)?;

            // 记录调度延迟 (使用原子操作)
            let elapsed_us = start.elapsed().as_micros() as u64;
            self.metrics.tasks_submitted.fetch_add(1, Ordering::Relaxed);
            
            // 更新平均延迟 (简单的移动平均)
            let old_avg = self.metrics.avg_schedule_latency_us.load(Ordering::Relaxed);
            let new_avg = (old_avg + elapsed_us) / 2;
            self.metrics.avg_schedule_latency_us.store(new_avg, Ordering::Relaxed);
            self.metrics.last_schedule_latency_us.store(elapsed_us, Ordering::Relaxed);
        }

        info!(
            "[UnifiedScheduler] 任务已提交: id={}, role={:?}, priority={:?}, model={}",
            submitted_task_id, role, priority, required_model
        );

        // 唤醒调度循环
        self.notify.notify_one();

        Ok(submitted_task_id)
    }

    /// 批量提交任务
    #[instrument(skip(self, tasks))]
    pub async fn submit_batch(
        &self,
        tasks: Vec<ScheduledTask>,
    ) -> Result<Vec<Result<TaskId, SchedulerError>>, SchedulerError> {
        let mut results = Vec::with_capacity(tasks.len());
        for task in tasks {
            results.push(self.submit_task(task).await);
        }
        Ok(results)
    }

    /// 取消任务 (及所有下游依赖任务)
    #[instrument(skip(self))]
    pub async fn cancel_task(&self, task_id: &TaskId) -> Result<(), SchedulerError> {
        // 取消自身
        {
            let mut task = self.get_task_mut(task_id).await?;
            if task.status == TaskStatus::Running {
                task.status = TaskStatus::Cancelling;
            } else {
                task.status = TaskStatus::Cancelled;
                task.completed_at = Some(chrono::Utc::now());
            }
        }

        // 迭代取消所有下游任务（避免 async fn 递归导致的 E0733）
        let downstream = self.get_downstream_tasks(task_id).await?;
        let mut to_cancel = downstream;
        while let Some(dep_id) = to_cancel.pop() {
            if let Ok(mut task) = self.get_task_mut(&dep_id).await {
                task.status = TaskStatus::Cancelled;
                task.completed_at = Some(chrono::Utc::now());
            }
            let mut queue = self.queue.write().await;
            queue.remove(&dep_id);
        }

        // 从队列移除
        {
            let mut queue = self.queue.write().await;
            queue.remove(task_id);
        }

        self.metrics.tasks_cancelled.fetch_add(1, Ordering::Relaxed);

        info!("[UnifiedScheduler] 任务 {} 已取消", task_id);
        Ok(())
    }

    /// 获取任务状态
    pub async fn get_task_status(&self, task_id: &TaskId) -> Result<TaskStatus, SchedulerError> {
        let task = self.get_task(task_id).await?;
        Ok(task.status)
    }

    /// 等待任务完成并返回结果
    ///
    /// 这是一个阻塞方法，会轮询任务状态直到完成、失败或超时。
    /// 用于同步API调用场景，需要等待推理完成后返回响应。
    pub async fn wait_for_completion(
        &self,
        task_id: &TaskId,
        timeout_ms: u64,
    ) -> Result<Option<TaskResult>, SchedulerError> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);

        loop {
            // 检查超时
            if start.elapsed() > timeout {
                return Err(SchedulerError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("Task {} timed out after {}ms", task_id, timeout_ms),
                )));
            }

            // 获取任务状态
            let task = self.get_task(task_id).await?;

            match task.status {
                TaskStatus::Completed => {
                    return Ok(task.result.clone());
                }
                TaskStatus::Failed => {
                    return Ok(Some(TaskResult {
                        success: false,
                        output: None,
                        error: Some("Task failed".to_string()),
                        duration_ms: task.started_at.and_then(|s| task.completed_at.map(|c| {
                            (c - s).num_milliseconds() as u64
                        })).unwrap_or(0),
                        assigned_nodes: vec![],
                        actual_latency_ms: 0.0,
                    }));
                }
                TaskStatus::Cancelled => {
                    return Ok(Some(TaskResult {
                        success: false,
                        output: None,
                        error: Some("Task cancelled".to_string()),
                        duration_ms: 0,
                        assigned_nodes: vec![],
                        actual_latency_ms: 0.0,
                    }));
                }
                _ => {
                    // 仍在执行中，等待一小段时间后重试
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
        }
    }

    // ========================================================================
    // 公开 API: 资源管理 (Parallax 接口)
    // ========================================================================

    /// 注册一个新的算力节点
    ///
    /// 对应 Parallax 中的节点加入 (Node Join)。
    /// 新节点会被纳入统一的资源池, 参与后续的层分配和请求路由。
    #[instrument(skip(self, hardware))]
    pub async fn register_node(
        &self,
        hardware: NodeHardwareInfo,
    ) -> Result<NodeId, SchedulerError> {
        let gpu_name = hardware.gpu_name.clone();
        let num_gpus = hardware.num_gpus;
        let memory_gb = hardware.memory_gb;
        let tflops = hardware.tflops_fp16;

        let mut manager = self.node_manager.write().await;
        let node_id = manager.register_node(hardware).await?;

        // 触发增量重平衡 (如果开启了自适应调度)
        if self.config.adaptive_scheduling {
            drop(manager);
            self.trigger_incremental_rebalance().await?;
        }

        info!(
            "[UnifiedScheduler] 节点已注册: id={}, gpu={}x{}, mem={}GB, tflops={:.1}",
            node_id,
            num_gpus,
            gpu_name,
            memory_gb,
            tflops
        );
        Ok(node_id)
    }

    /// 注销一个算力节点
    ///
    /// 对应 Parallax 中的节点离开 (Node Leave)。
    /// 会释放该节点的所有层分配, 并检查是否需要全局重平衡。
    #[instrument(skip(self))]
    pub async fn unregister_node(&self, node_id: &NodeId) -> Result<(), SchedulerError> {
        // 1. 从 LayerAllocator 中移除节点并触发重平衡
        {
            let mut allocator = self.layer_allocator.write().await;
            if let Some(ref mut alloc) = *allocator {
                match alloc.remove_node_and_rebalance(*node_id) {
                    Ok(_) => {
                        info!("[UnifiedScheduler] 节点 {} 已从层分配器中移除", node_id);
                    }
                    Err(e) => {
                        warn!("[UnifiedScheduler] 从层分配器移除节点 {} 失败: {:?}", node_id, e);
                        // 继续执行, 因为可能节点本来就不在分配器中
                    }
                }
            }
        }

        // 2. 从 NodeManager 中注销节点
        {
            let mut manager = self.node_manager.write().await;
            manager.unregister_node(node_id).await?;
        }

        info!("[UnifiedScheduler] 节点已注销: {}", node_id);

        // 3. 检查是否还需要额外的重平衡
        self.check_and_rebalance().await?;

        Ok(())
    }

    /// 获取当前所有活跃节点
    pub async fn get_active_nodes(&self) -> Vec<NodeInfo> {
        let manager = self.node_manager.read().await;
        manager.active_nodes()
    }

    /// 获取集群总资源概况
    pub async fn get_cluster_summary(&self) -> ClusterResourceSummary {
        let manager = self.node_manager.read().await;
        manager.cluster_summary()
    }

    /// 节点心跳更新
    ///
    /// 定期调用以保持节点在线状态, 并上报实时性能数据。
    #[instrument(skip(self))]
    pub async fn node_heartbeat(
        &self,
        node_id: &NodeId,
        latency_ms: Option<f64>,
    ) -> Result<(), SchedulerError> {
        let mut manager = self.node_manager.write().await;
        manager.update_heartbeat(node_id, latency_ms).await
    }

    // ========================================================================
    // 核心调度循环
    // ========================================================================

    /// 启动调度循环
    ///
    /// 这是一个持续运行的异步循环:
    /// 1. 等待通知 (新任务/资源变化/定时器)
    /// 2. 从队列取出可执行任务
    /// 3. 进行资源匹配 (Parallax Phase 1 + Phase 2)
    /// 4. 分配并执行任务
    /// 5. 更新状态和指标
    pub async fn run(&self) -> Result<(), SchedulerError> {
        {
            let mut state = self.state.write().await;
            *state = SchedulerState::Running;
        }
        info!("[UnifiedScheduler] 调度循环启动 id={}", self.id);

        loop {
            // 检查是否应该停止
            {
                let state = self.state.read().await;
                match *state {
                    SchedulerState::Shutdown => {
                        info!("[UnifiedScheduler] 收到关闭信号, 退出调度循环");
                        break;
                    }
                    SchedulerState::ShuttingDown => {
                        // 等待正在执行的任务完成 (使用原子操作)
                        if self.metrics.running_count.load(Ordering::Relaxed) == 0 {
                            info!("[UnifiedScheduler] 所有任务已完成, 关闭");
                            break;
                        }
                    }
                    SchedulerState::Paused => {
                        drop(state);
                        // 暂停状态下等待唤醒
                        self.notify.notified().await;
                        continue;
                    }
                    _ => {}
                }
            }

            // 尝试调度一批任务
            match self.schedule_batch().await {
                Ok(count) => {
                    if count > 0 {
                        debug!("[UnifiedScheduler] 本次调度了 {} 个任务", count);
                    }
                }
                Err(e) => {
                    error!("[UnifiedScheduler] 调度出错: {:?}", e);
                }
            }

            // 等待下一个事件或超时 (避免忙轮询)
            tokio::select! {
                _ = self.notify.notified() => {},
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
            }
        }

        // 标记为 shutdown
        {
            let mut state = self.state.write().await;
            *state = SchedulerState::Shutdown;
        }

        Ok(())
    }

    /// 优雅关闭
    ///
    /// 不再接受新任务, 等待正在执行的任务完成后关闭。
    pub async fn shutdown(&self) -> Result<(), SchedulerError> {
        info!("[UnifiedScheduler] 发起优雅关闭...");
        {
            let mut state = self.state.write().await;
            *state = SchedulerState::ShuttingDown;
        }
        self.notify.notify_one();

        // 等待实际关闭 (由 run() 循环处理)
        loop {
            {
                let state = self.state.read().await;
                if *state == SchedulerState::Shutdown {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        info!("[UnifiedScheduler] 已完全关闭");
        Ok(())
    }

    /// 暂停调度
    pub async fn pause(&self) -> Result<(), SchedulerError> {
        let mut state = self.state.write().await;
        *state = SchedulerState::Paused;
        info!("[UnifiedScheduler] 调度器已暂停");
        Ok(())
    }

    /// 恢复调度
    pub async fn resume(&self) -> Result<(), SchedulerError> {
        let mut state = self.state.write().await;
        *state = SchedulerState::Idle;
        self.notify.notify_one();
        info!("[UnifiedScheduler] 调度器已恢复");
        Ok(())
    }

    /// 强制暂停 (立即中断所有运行中的任务)
    pub async fn force_pause(&self) -> Result<(), SchedulerError> {
        // TODO: 实现强制暂停逻辑 (通过 CancellationToken 中断任务)
        self.pause().await
    }

    // ========================================================================
    // 内部调度方法
    // ========================================================================

    /// Try GPU-aware scheduling for inference tasks
    async fn try_gpu_scheduling(
        &self,
        task: &ScheduledTask,
    ) -> Result<Option<(Vec<NodeId>, f64)>, SchedulerError> {
        use gpu_load_balancer::{GpuInferenceRequest, Precision};

        let mut balancer_guard = self.gpu_balancer.write().await;
        if let Some(balancer) = balancer_guard.as_mut() {
            // Estimate model size based on model name
            let model_size = Self::estimate_model_size(&task.required_model);

            let request = GpuInferenceRequest {
                request_id: task.id.to_string(),
                model_name: task.required_model.clone(),
                model_size_bytes: model_size,
                batch_size: task.batch_size.unwrap_or(1),
                seq_len: task.max_seq_len.unwrap_or(512),
                precision: Precision::FP16,
                max_latency_ms: self.config.max_wait_time_ms,
                priority: task.priority as u32,
            };

            if let Some(decision) = balancer.schedule(&request) {
                info!(
                    "[GPU Scheduler] Task {} scheduled to GPUs {:?}, est. latency {:.1}ms",
                    task.id, decision.gpu_ids, decision.estimated_latency_ms
                );

                // Record scheduling decision in metrics
                self.metrics.phase2_routings.fetch_add(1, Ordering::Relaxed);

                return Ok(Some((
                    decision.gpu_ids.iter().map(|&id| Uuid::from_fields_le(id, 0, 0, &[0;8])).collect(),
                    decision.estimated_latency_ms,
                )));
            }
        }

        // GPU scheduling not available or failed
        Ok(None)
    }

    /// Estimate model size in bytes based on model name
    fn estimate_model_size(model_name: &str) -> u64 {
        let model_lower = model_name.to_lowercase();
        // Rough estimates for common models (FP16 = 2 bytes per param)
        if model_lower.contains("72b") || model_lower.contains("70b") {
            72_000_000_000u64 * 2 // ~144GB
        } else if model_lower.contains("32b") || model_lower.contains("35b") {
            32_000_000_000u64 * 2 // ~64GB
        } else if model_lower.contains("13b") || model_lower.contains("14b") {
            13_000_000_000u64 * 2 // ~26GB
        } else if model_lower.contains("7b") {
            7_000_000_000u64 * 2  // ~14GB
        } else {
            1_000_000_000u64 * 2  // Default ~2GB for smaller models
        }
    }

    /// 批量调度: 尝试将尽可能多的就绪任务分配到可用资源
    async fn schedule_batch(&self) -> Result<usize, SchedulerError> {
        let start = std::time::Instant::now();
        let mut scheduled_count = 0usize;

        loop {
            // 1. 从队列取下一个可执行任务 (依赖已满足)
            let next_task = {
                let mut queue = self.queue.write().await;
                let running_count = self.metrics.running_count.load(Ordering::Relaxed);
                if running_count >= self.config.max_concurrent_tasks as u64 {
                    None // 达到最大并发限制
                } else {
                    queue.pop_ready(&self.task_registry)?
                }
            };

            let task = match next_task {
                Some(t) => t,
                None => break, // 无更多可执行任务
            };

            // 2. 资源匹配 (Parallax 两阶段)
            let assignment = self.match_resource(&task).await?;

            match assignment {
                Some((node_ids, estimated_latency)) => {
                    // 匹配成功 -> 分配任务到目标节点
                    debug!(
                        "[UnifiedScheduler] 任务 {} 分配到节点 {:?}, 预估延迟 {:.1}ms",
                        task.id, node_ids, estimated_latency
                    );

                    // 更新任务状态为 Running
                    self.transition_task(&task.id, TaskStatus::Running).await?;

                    // 更新节点负载
                    {
                        let mut manager = self.node_manager.write().await;
                        for nid in &node_ids {
                            if let Some(node) = manager.get_node_mut(nid) {
                                std::sync::Arc::make_mut(node).add_request();
                            }
                        }
                    }

                    scheduled_count += 1;

                    // 更新指标
                    {
                        self.metrics.running_count.fetch_add(1, Ordering::Relaxed);
                        self.metrics.phase2_routings.fetch_add(1, Ordering::Relaxed);
                    }
                }
                None => {
                    // 无可用资源 -> 放回队列 (优先级不变)
                    debug!(
                        "[UnifiedScheduler] 任务 {} 暂无可用资源, 放回队列",
                        task.id
                    );
                    {
                        let mut queue = self.queue.write().await;
                        queue.push(task)?;
                    }
                    break; // 既然最高优先级的任务都无法满足, 后续也不必尝试了
                }
            }
        }

        let elapsed = start.elapsed();
        if scheduled_count > 0 {
            self.metrics.last_schedule_latency_us.store(elapsed.as_micros() as u64, Ordering::Relaxed);
            // EMA 更新平均延迟
            let alpha = 0.3;
            let old_avg = self.metrics.avg_schedule_latency_us.load(Ordering::Relaxed);
            let new_avg = (alpha * elapsed.as_micros() as f64 + (1.0 - alpha) * old_avg as f64) as u64;
            self.metrics.avg_schedule_latency_us.store(new_avg, Ordering::Relaxed);
        }

        Ok(scheduled_count)
    }

    /// 核心资源匹配: 结合任务需求和算力资源做最优决策
    ///
    /// 这是统一调度的关键方法, 融合了 Ruflo 和 Parallax:
    /// - 输入: 任务需求 (角色/模型大小/优先级)
    /// - 处理: Phase 1 层分配 + Phase 2 请求路由
    /// - 输出: 最优节点路径 + 预估延迟
    async fn match_resource(
        &self,
        task: &ScheduledTask,
    ) -> Result<Option<(Vec<NodeId>, f64)>, SchedulerError> {
        #[allow(unused_variables)]
        let _ = task;

        // === GPU-Aware Scheduling (if enabled and task requires inference) ===
        if self.config.enable_gpu_inference && task.requires_inference {
            if let Some(decision) = self.try_gpu_scheduling(task).await? {
                return Ok(Some(decision));
            }
            // Fall back to CPU scheduling if GPU scheduling fails
        }

        let nodes: Vec<Arc<NodeInfo>> = { let mgr = self.node_manager.read().await; mgr.active_nodes().into_iter().map(Arc::new).collect() };

        if nodes.is_empty() {
            return Ok(None); // 无可用节点
        }

        // 对于非推理类任务 (如代码生成/文件操作), 使用简化的资源匹配
        if !task.requires_inference {
            return self.match_simple_task(&nodes.iter().cloned().collect::<Vec<_>>(), task).await;
        }

        // ===== Parallax 两阶段调度 =====

        // Phase 1: 层分配 (静态/半静态)
        let allocator_guard = self.layer_allocator.read().await;
        let allocator = allocator_guard.as_ref().ok_or(SchedulerError::NotInitialized)?;

        // 检查是否需要重新分配
        if allocator.should_rebalance(&nodes.iter().map(|n| n.as_ref()).collect::<Vec<_>>())? {
            drop(allocator_guard);
            // 需要 -> 执行全局重平衡
            self.execute_global_rebalance().await?;
        } else {
            drop(allocator_guard);
        }

        // Phase 2: 请求路由 (动态, 每个请求独立计算最优路径)
        let mut router_guard = self.request_router.write().await;
        let router = router_guard.as_mut().ok_or(SchedulerError::NotInitialized)?;

        // 根据模型大小确定需要的 "层数" (这里抽象化: 大模型=多层数, 小模型=少层数)
        let virtual_layers = self.model_to_virtual_layers(&task.required_model);

        let result = router.find_optimal_path(virtual_layers, &nodes.to_vec())?;

        // 更新 Phase 1 计数
        {
            self.metrics.phase1_allocations.fetch_add(1, Ordering::Relaxed);
        }

        Ok(result)
    }

    /// 非推理任务的简化资源匹配
    async fn match_simple_task(
        &self,
        nodes: &[Arc<NodeInfo>],
        _task: &ScheduledTask,
    ) -> Result<Option<(Vec<NodeId>, f64)>, SchedulerError> {
        // 简单策略: 选择负载最低的节点
        let best = nodes
            .iter()
            .filter(|n| !n.is_overloaded())
            .min_by(|a, b| {
                let load_a = a.current_requests as f64 / a.max_requests as f64;
                let load_b = b.current_requests as f64 / b.max_requests as f64;
                load_a.partial_cmp(&load_b).unwrap_or(std::cmp::Ordering::Equal)
            });

        match best {
            Some(node) => Ok(Some((vec![node.node_id], 1.0))),
            None => Ok(None), // 所有节点都过载
        }
    }

    /// 将模型名称转换为虚拟层数 (用于调度算法的输入)
    fn model_to_virtual_layers(&self, model: &str) -> u32 {
        // 根据模型参数量估算虚拟层数
        // 这里的映射关系可以更精确地基于实际的模型参数量
        let model_lower = model.to_lowercase();
        if model_lower.contains("72b") || model_lower.contains("70b") {
            80
        } else if model_lower.contains("32b") || model_lower.contains("35b") {
            40
        } else if model_lower.contains("14b") || model_lower.contains("13b") {
            20
        } else if model_lower.contains("7b") || model_lower.contains("8b") {
            12
        } else if model_lower.contains("3b") || model_lower.contains("1.5b") {
            6
        } else if model_lower.contains("0.5b")
            || model_lower.contains("tiny")
            || model_lower.contains("small")
        {
            3
        } else {
            12 // 默认中等规模
        }
    }

    // ========================================================================
    // 重平衡逻辑 (Parallax)
    // ========================================================================

    /// 检查是否需要全局重平衡, 如需要则执行
    async fn check_and_rebalance(&self) -> Result<bool, SchedulerError> {
        let allocator = self.layer_allocator.read().await;
        let allocator = match allocator.as_ref() {
            Some(a) => a,
            None => return Ok(false),
        };
        let active_nodes = {
            let mgr = self.node_manager.read().await;
            mgr.active_nodes()
        };
        let active_nodes_refs: Vec<&NodeInfo> = active_nodes.iter().collect();
        let needs = allocator.should_rebalance(&active_nodes_refs)?;
        let _ = allocator;

        if needs {
            self.execute_global_rebalance().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 执行全局重平衡
    async fn execute_global_rebalance(&self) -> Result<(), SchedulerError> {
        info!("[UnifiedScheduler] 开始全局重平衡...");

        {
            let mut allocator = self.layer_allocator.write().await;
            if let Some(ref mut alloc) = *allocator {
                alloc.global_rebalance()?;
            }
        }

        {
            self.metrics.global_rebalances.fetch_add(1, Ordering::Relaxed);
        }

        info!("[UnifiedScheduler] 全局重平衡完成");
        Ok(())
    }

    /// 增量重平衡 (新节点加入时触发)
    async fn trigger_incremental_rebalance(&self) -> Result<(), SchedulerError> {
        let new_node = {
            let mgr = self.node_manager.read().await;
            mgr.last_registered_node().cloned()
        };

        if let Some(node) = new_node {
            let mut allocator = self.layer_allocator.write().await;
            if let Some(ref mut alloc) = *allocator {
                alloc.dynamic_join(&node)?;
            }
        }

        Ok(())
    }

    // ========================================================================
    // 辅助方法
    // ========================================================================

    async fn get_task(&self, task_id: &TaskId) -> Result<ScheduledTask, SchedulerError> {
        self.task_registry
            .get(task_id)
            .map(|r| r.value().clone())
            .ok_or(SchedulerError::TaskNotFound(*task_id))
    }

    async fn get_task_mut(
        &self,
        task_id: &TaskId,
    ) -> Result<dashmap::mapref::one::RefMut<'_, TaskId, ScheduledTask>, SchedulerError> {
        self.task_registry
            .get_mut(task_id)
            .ok_or(SchedulerError::TaskNotFound(*task_id))
    }

    async fn transition_task(
        &self,
        task_id: &TaskId,
        new_status: TaskStatus,
    ) -> Result<(), SchedulerError> {
        // 更新任务状态
        {
            let mut task = self.task_registry.get_mut(task_id)
                .ok_or(SchedulerError::TaskNotFound(*task_id))?;
            task.status = new_status;
            match new_status {
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled => {
                    task.completed_at = Some(chrono::Utc::now());
                }
                TaskStatus::Running => {
                    task.started_at = Some(chrono::Utc::now());
                }
                _ => {}
            }
        }

        // 更新全局指标 (使用原子操作，无锁)
        match new_status {
            TaskStatus::Running => {
                self.metrics.running_count.fetch_add(1, Ordering::Relaxed);
            }
            TaskStatus::Completed => {
                self.metrics.running_count.fetch_sub(1, Ordering::Relaxed);
                self.metrics.tasks_completed.fetch_add(1, Ordering::Relaxed);
            }
            TaskStatus::Failed => {
                self.metrics.running_count.fetch_sub(1, Ordering::Relaxed);
                self.metrics.tasks_failed.fetch_add(1, Ordering::Relaxed);
            }
            TaskStatus::Cancelled => {
                self.metrics.running_count.fetch_sub(1, Ordering::Relaxed);
                self.metrics.tasks_cancelled.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }

        Ok(())
    }

    /// 获取某个任务的所有下游依赖者
    async fn get_downstream_tasks(&self, task_id: &TaskId) -> Result<Vec<TaskId>, SchedulerError> {
        let graph = self.dependency_graph.read().await;
        let mut downstream = Vec::new();
        let mut to_visit = vec![*task_id];
        let mut visited = std::collections::HashSet::new();

        while let Some(current) = to_visit.pop() {
            if visited.insert(current) {
                if let Some(deps) = graph.get(&current) {
                    for dep in deps {
                        if !visited.contains(dep) {
                            downstream.push(*dep);
                            to_visit.push(*dep);
                        }
                    }
                }
            }
        }

        Ok(downstream)
    }

    /// 获取当前性能指标快照
    pub async fn get_metrics(&self) -> SchedulerMetricsSnapshot {
        let mut m = self.metrics.snapshot();
        m.queue_length = {
            let q = self.queue.read().await;
            q.len() as u64
        };
        m.collected_at = chrono::Utc::now();
        m
    }

    /// Get GPU statistics (if GPU balancer is available)
    pub async fn get_gpu_stats(&self) -> Option<gpu_load_balancer::GpuStats> {
        let balancer_guard = self.gpu_balancer.read().await;
        balancer_guard.as_ref().map(|balancer| balancer.get_stats())
    }

    /// Get GPU utilization for Prometheus export
    pub async fn get_gpu_prometheus_metrics(&self) -> Vec<(String, f64)> {
        let mut metrics = Vec::new();

        if let Some(stats) = self.get_gpu_stats().await {
            metrics.push(("carpai_gpu_total".to_string(), stats.total_gpus as f64));
            metrics.push(("carpai_gpu_active".to_string(), stats.active_gpus as f64));
            metrics.push(("carpai_gpu_avg_utilization".to_string(), stats.avg_utilization / 100.0));
            metrics.push(("carpai_gpu_vram_total_bytes".to_string(), stats.total_vram_bytes as f64));
            metrics.push(("carpai_gpu_vram_used_bytes".to_string(), stats.used_vram_bytes as f64));
            metrics.push(("carpai_gpu_vram_usage_percent".to_string(), stats.vram_usage_percent()));
            metrics.push(("carpai_gpu_pending_requests".to_string(), stats.pending_requests as f64));
        }

        metrics
    }

    /// 获取调度器状态
    pub async fn get_state(&self) -> SchedulerState {
        *self.state.read().await
    }
}

// 由于 SchedulerError 在 async context 中使用, 这里提供一个辅助函数来处理 metrics 写锁
#[allow(dead_code)]
fn block_on_metrics() -> Result<std::sync::RwLockWriteGuard<'static, SchedulerMetrics>, SchedulerError> {
    Err(SchedulerError::NotInitialized)
}

// ============================================================================
// 错误类型
// ============================================================================

/// 统一调度器错误类型
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("任务未找到: {0}")]
    TaskNotFound(TaskId),

    #[error("节点未找到: {0}")]
    NodeNotFound(NodeId),

    #[error("队列已满 (当前长度: {0})")]
    QueueFull(usize),

    #[error("依赖循环检测")]
    CycleDetected(Vec<TaskId>),

    #[error("资源不足: 无法找到匹配 {required} 的节点")]
    InsufficientResources { required: String },

    #[error("层分配失败: {0}")]
    AllocationFailed(String),

    #[error("路由失败: 无可用路径")]
    NoRouteAvailable,

    #[error("GOAP 规划失败: {0}")]
    GoapPlanningFailed(String),

    #[error("调度器尚未初始化")]
    NotInitialized,

    #[error("调度器已关闭")]
    Shutdown,

    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("序列化错误: {0}")]
    Serialization(#[from] serde_json::Error),
}



// ============================================================================
// 测试模块
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scheduler_creation() {
        let config = SchedulerConfig::default();
        let scheduler = UnifiedScheduler::new(config).await.unwrap();
        assert_eq!(scheduler.get_state().await, SchedulerState::Idle);
    }

    #[tokio::test]
    async fn test_submit_and_cancel_task() {
        let config = SchedulerConfig::default();
        let scheduler = UnifiedScheduler::new(config).await.unwrap();

        let task = ScheduledTask::simple("测试任务", AgentRole::Worker, "qwen-3.6-max");
        let id = scheduler.submit_task(task).await.unwrap();
        assert_eq!(scheduler.get_task_status(&id).await.unwrap(), TaskStatus::Queued);

        scheduler.cancel_task(&id).await.unwrap();
        assert_eq!(scheduler.get_task_status(&id).await.unwrap(), TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_node_registration() {
        let config = SchedulerConfig::default();
        let scheduler = UnifiedScheduler::new(config).await.unwrap();

        let hw = NodeHardwareInfo::gpu("RTX-4090", 1, 82.0, 24.0, 1008.0);
        let node_id = scheduler.register_node(hw).await.unwrap();
        assert!(!node_id.is_nil());

        let nodes = scheduler.get_active_nodes().await;
        assert_eq!(nodes.len(), 1);
    }

    #[tokio::test]
    async fn test_goap_planning_integration() {
        let mut config = SchedulerConfig::default();
        config.enable_goap = true;
        let scheduler = UnifiedScheduler::new(config).await.unwrap();

        let task = ScheduledTask::with_goal(
            "部署应用到生产环境",
            AgentRole::Coordinator,
            "qwen-3.6-max",
            TaskPriority::High,
        );
        let id = scheduler.submit_task(task).await.unwrap();

        let task = scheduler.get_task(&id).await.unwrap();
        assert!(!task.actions.is_empty(), "GOAP 应该生成了 action 步骤");
    }
}
