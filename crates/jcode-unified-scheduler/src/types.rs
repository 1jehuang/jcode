//! **统一类型系统** — Ruflo + Parallax + JCode 三源融合的类型定义
//!
//! 包含:
//! - 任务相关类型 (来自 Ruflo GOAP)
//! - 资源/节点相关类型 (来自 Parallax)
//! - 调度结果/状态类型 (原创融合)

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use uuid::Uuid;

// ============================================================================
// 基础标识符
// ============================================================================

/// 任务 ID (UUID v4)
pub type TaskId = Uuid;

/// 节点 ID (UUID v4)
pub type NodeId = Uuid;

/// Pipeline ID (UUID v4)
pub type PipelineId = Uuid;

// ============================================================================
// 任务相关类型 (Ruflo GOAP 体系)
// ============================================================================

/// Agent 角色 — 对应 Ruflo 的专业化 Agent 分类
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// 协调者: 分析、设计、任务分解 (对应 Ruflo Architect/Cordinator)
    Coordinator,

    /// 工作者: 编码、实现、测试 (对应 Ruflo Coder/Worker)
    Worker,

    /// 专家: 特定领域搜索、审查等 (对应 Ruflo Specialist)
    Specialist(String),

    /// 通用: 任何任务都可以尝试 (兜底角色)
    General,
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentRole::Coordinator => write!(f, "coordinator"),
            AgentRole::Worker => write!(f, "worker"),
            AgentRole::Specialist(name) => write!(f, "specialist:{}", name),
            AgentRole::General => write!(f, "general"),
        }
    }
}

impl AgentRole {
    /// 获取角色的计算需求权重 (影响调度时的资源分配偏好)
    pub fn compute_weight(&self) -> f64 {
        match self {
            AgentRole::Coordinator => 0.3,  // 低 CPU, 高内存 (分析/规划)
            AgentRole::Worker => 1.0,       // 高 CPU + GPU (编码/编译)
            AgentRole::Specialist(_) => 0.7, // 中等
            AgentRole::General => 0.5,       // 中等偏低
        }
    }
}

/// 任务优先级 (数值越大越优先)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low = 1,
    Medium = 2,
    High = 3,
    Urgent = 4,
    Critical = 5,
}

impl TaskPriority {
    pub fn from_level(level: u8) -> Self {
        match level {
            0..=1 => TaskPriority::Low,
            2 => TaskPriority::Medium,
            3 => TaskPriority::High,
            4 => TaskPriority::Urgent,
            _ => TaskPriority::Critical,
        }
    }
}

impl std::fmt::Display for TaskPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskPriority::Low => write!(f, "low"),
            TaskPriority::Medium => write!(f, "medium"),
            TaskPriority::High => write!(f, "high"),
            TaskPriority::Urgent => write!(f, "urgent"),
            TaskPriority::Critical => write!(f, "critical"),
        }
    }
}

/// 任务状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 初始/已创建
    Pending,
    /// 已入队等待调度
    Queued,
    /// 依赖等待中
    WaitingForDependencies,
    /// 正在执行
    Running,
    /// 用户发起取消中
    Cancelling,
    /// 已完成
    Completed,
    /// 执行失败
    Failed,
    /// 已取消
    Cancelled,
    /// 被抢占 (高优先级任务挤掉了低优先级)
    Preempted,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::Queued => write!(f, "queued"),
            TaskStatus::WaitingForDependencies => write!(f, "waiting_deps"),
            TaskStatus::Running => write!(f, "running"),
            TaskStatus::Cancelling => write!(f, "cancelling"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
            TaskStatus::Cancelled => write!(f, "cancelled"),
            TaskStatus::Preempted => write!(f, "preempted"),
        }
    }
}

/// 动作状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Skipped,
}

/// GOAP 动作定义 — 对应 Ruflo Goal Module 中的原子动作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: uuid::Uuid,
    pub name: String,
    pub parameters: serde_json::Value,
    pub preconditions: Vec<WorldStateCondition>,
    pub effects: Vec<WorldStateEffect>,
    pub estimated_cost: f64,
    pub status: ActionStatus,
}

impl Action {
    /// 检查给定世界状态下此动作是否可执行
    pub fn is_executable_in(&self, world_state: &WorldState) -> bool {
        self.preconditions
            .iter()
            .all(|cond| cond.satisfied_by(world_state))
    }
}

/// 世界状态条件 — GOAP 前置条件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldStateCondition {
    pub key: String,
    pub operator: ConditionOp,
    pub value: WorldStateValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConditionOp {
    Equals,
    NotEquals,
    GreaterThan,
    LessThan,
    Contains,
    NotContains,
    Exists,
    NotExists,
}

impl WorldStateCondition {
    pub fn satisfied_by(&self, world_state: &WorldState) -> bool {
        let actual = match world_state.0.get(&self.key) {
            Some(v) => v,
            None => return matches!(self.operator, ConditionOp::NotExists),
        };
        match self.operator {
            ConditionOp::Equals => actual == &self.value,
            ConditionOp::NotEquals => actual != &self.value,
            ConditionOp::GreaterThan => actual > &self.value,
            ConditionOp::LessThan => actual < &self.value,
            ConditionOp::Contains | ConditionOp::Exists => true, // 简化
            ConditionOp::NotContains | ConditionOp::NotExists => false,
        }
    }
}

/// 世界状态效果 — GOAP 动作效果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldStateEffect {
    pub key: String,
    pub operation: EffectOp,
    pub value: WorldStateValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectOp {
    Set,
    Add,
    Remove,
    Increment,
    Decrement,
}

/// 世界状态值 (支持多种类型)
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum WorldStateValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<String>),
    Nil,
}

impl WorldStateValue {
    pub fn as_float(&self) -> Option<f64> {
        match self {
            WorldStateValue::Float(f) => Some(*f),
            WorldStateValue::Int(i) => Some(*i as f64),
            WorldStateValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

/// 完整的世界状态表示
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorldState(pub std::collections::HashMap<String, WorldStateValue>);

impl WorldState {
    pub fn new() -> Self {
        Self(std::collections::HashMap::new())
    }

    pub fn set(&mut self, key: impl Into<String>, value: WorldStateValue) {
        self.0.insert(key.into(), value);
    }

    pub fn get(&self, key: &str) -> Option<&WorldStateValue> {
        self.0.get(key)
    }

    /// 应用效果到世界状态
    pub fn apply_effect(&mut self, effect: &WorldStateEffect) {
        match effect.operation {
            EffectOp::Set => {
                self.0.insert(effect.key.clone(), effect.value.clone());
            }
            EffectOp::Add => {
                if let Some(WorldStateValue::List(list)) = self.0.get_mut(&effect.key) {
                    if let WorldStateValue::String(s) = &effect.value {
                        list.push(s.clone());
                    }
                }
            }
            EffectOp::Remove => {
                if let Some(WorldStateValue::List(list)) = self.0.get_mut(&effect.key) {
                    if let WorldStateValue::String(s) = &effect.value {
                        list.retain(|item| item != s);
                    }
                }
            }
            EffectOp::Increment => {
                if let Some(val) = self.0.get_mut(&effect.key) {
                    if let WorldStateValue::Int(i) = val {
                        *i += 1;
                    } else if let WorldStateValue::Float(f) = val {
                        *f += 1.0;
                    }
                }
            }
            EffectOp::Decrement => {
                if let Some(val) = self.0.get_mut(&effect.key) {
                    if let WorldStateValue::Int(i) = val {
                        *i -= 1;
                    } else if let WorldStateValue::Float(f) = val {
                        *f -= 1.0;
                    }
                }
            }
        }
    }
}

/// GOAP 规划结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoapPlan {
    /// 规划的总代价
    pub total_cost: f64,
    /// 动作步骤序列
    pub steps: Vec<GoapStep>,
    /// 最终达成目标的世界状态
    pub final_state: WorldState,
    /// 规划耗时 (毫秒)
    pub planning_time_ms: f64,
    /// 迭代次数
    pub iterations: usize,
}

/// GOAP 规划步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoapStep {
    pub step_number: usize,
    pub action_name: String,
    pub params: serde_json::Value,
    pub preconditions: Vec<String>,
    pub effects: Vec<String>,
    pub estimated_cost: f64,
}

// ============================================================================
// 调度任务 (统一任务描述)
// ============================================================================

/// 统一调度任务 — 融合 Ruflo 任务 + Parallax 请求信号
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// 任务 ID
    pub id: TaskId,

    /// 任务描述
    pub description: String,

    /// === 任务属性 (Ruflo) ===
    /// Agent 角色
    pub role: AgentRole,
    /// 优先级
    pub priority: TaskPriority,
    /// 所需模型 (如 "qwen-3.6-max", "deepseek-r1-72b")
    pub required_model: String,
    /// 依赖的任务 ID 列表 (DAG)
    pub dependencies: Vec<TaskId>,
    /// 高层目标 (可选, 触发 GOAP 自动分解)
    pub goal: Option<String>,
    /// GOAP 生成的动作序列
    pub actions: Vec<Action>,
    /// GOAP 规划结果
    pub plan: Option<GoapPlan>,

    /// === 资源需求 (Parallax) ===
    /// 是否需要推理 (true=走 Parallax 两阶段, false=走简单匹配)
    pub requires_inference: bool,
    /// 所需最小显存 (MB), 0=不限
    pub min_memory_mb: Option<u64>,
    /// 所需最小 TFLOPS, 0.0=不限
    pub min_tflops: Option<f64>,
    /// 最大可容忍延迟 (ms)
    pub max_latency_ms: Option<f64>,
    /// 估算 token 数 (用于 KV Cache 预算)
    pub estimated_tokens: Option<u64>,
    /// 批量大小 (用于推理)
    pub batch_size: Option<u32>,
    /// 最大序列长度 (用于推理)
    pub max_seq_len: Option<u32>,

    /// === 时间戳 ===
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub submitted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,

    /// === 状态 ===
    pub status: TaskStatus,
    /// 执行结果/错误信息
    pub result: Option<TaskResult>,
    /// 元数据 (自由格式)
    pub metadata: serde_json::Value,

    /// 重试计数
    pub retry_count: u32,
    /// 最大重试次数
    pub max_retries: u32,
}

/// 任务执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub assigned_nodes: Vec<NodeId>,
    pub actual_latency_ms: f64,
}

impl ScheduledTask {
    /// 创建简单任务
    pub fn simple(description: &str, role: AgentRole, model: &str) -> Self {
        Self {
            id: uuid::Uuid::nil(), // 由调度器分配
            description: description.to_string(),
            role,
            priority: TaskPriority::Medium,
            required_model: model.to_string(),
            dependencies: vec![],
            goal: None,
            actions: vec![],
            plan: None,
            requires_inference: true,
            min_memory_mb: None,
            min_tflops: None,
            max_latency_ms: None,
            estimated_tokens: None,
            batch_size: None,
            max_seq_len: None,
            created_at: Some(chrono::Utc::now()),
            submitted_at: None,
            started_at: None,
            completed_at: None,
            status: TaskStatus::Pending,
            result: None,
            metadata: serde_json::Value::Object(serde_json::Map::new()),
            retry_count: 0,
            max_retries: 3,
        }
    }

    /// 创建带高层目标的任务 (触发 GOAP)
    pub fn with_goal(goal: &str, role: AgentRole, model: &str, priority: TaskPriority) -> Self {
        let mut task = Self::simple(goal, role, model);
        task.goal = Some(goal.to_string());
        task.priority = priority;
        task.requires_inference = false; // GOAP 任务通常不需要推理
        task
    }

    /// 添加依赖
    pub fn depends_on(mut self, dep: TaskId) -> Self {
        self.dependencies.push(dep);
        self
    }

    /// 设置优先级
    pub fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = priority;
        self
    }

    /// 设置资源约束
    pub fn with_resources(
        mut self,
        memory_mb: u64,
        tflops: f64,
        max_latency_ms: f64,
    ) -> Self {
        self.min_memory_mb = Some(memory_mb);
        self.min_tflops = Some(tflops);
        self.max_latency_ms = Some(max_latency_ms);
        self
    }

    /// 所有依赖是否都已解决
    pub fn dependencies_met(
        &self,
        completed_tasks: &std::collections::HashSet<TaskId>,
    ) -> bool {
        self.dependencies.iter().all(|id| completed_tasks.contains(id))
    }
}

/// 实现 PartialOrd/Ord 以支持优先队列排序
impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for ScheduledTask {}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // 主键: 优先级 DESC
        match other.priority.cmp(&self.priority) {
            Ordering::Equal => {
                // 次键: 提交时间 ASC (FIFO)
                match (&self.submitted_at, &other.submitted_at) {
                    (Some(a), Some(b)) => a.cmp(b),
                    (Some(_), None) => Ordering::Greater,
                    (None, Some(_)) => Ordering::Less,
                    (None, None) => Ordering::Equal,
                }
            }
            ord => ord,
        }
    }
}

// ============================================================================
// 资源/算力相关类型 (Parallax 体系)
// ============================================================================

/// 节点硬件信息 — 对应 Parallax NodeHardwareInfo (静态属性)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHardwareInfo {
    pub node_id: NodeId,
    /// GPU 数量
    pub num_gpus: u32,
    /// GPU 型号名
    pub gpu_name: String,
    /// FP16 算力 (TFLOPS)
    pub tflops_fp16: f64,
    /// 显存总量 (GB)
    pub memory_gb: f64,
    /// 内存带宽 (GB/s)
    pub memory_bandwidth_gbps: f64,
    /// 设备类型: "cuda", "rocm", "mlx" (Apple Silicon), "cpu"
    pub device_type: String,
}

impl NodeHardwareInfo {
    /// 创建 GPU 节点的便捷构造函数
    pub fn gpu(gpu_name: &str, num_gpus: u32, tflops: f64, memory_gb: f64, bandwidth: f64) -> Self {
        Self {
            node_id: uuid::Uuid::new_v4(),
            num_gpus,
            gpu_name: gpu_name.to_string(),
            tflops_fp16: tflops,
            memory_gb,
            memory_bandwidth_gbps: bandwidth,
            device_type: "cuda".to_string(),
        }
    }

    /// 创建 Apple Silicon (MLX) 节点
    pub fn apple_chip(chip_name: &str, unified_memory_gb: f64, bandwidth: f64) -> Self {
        Self {
            node_id: uuid::Uuid::new_v4(),
            num_gpus: 1,
            gpu_name: chip_name.to_string(),
            tflops_fp16: bandwidth * 0.05, // 粗估
            memory_gb: unified_memory_gb,
            memory_bandwidth_gbps: bandwidth,
            device_type: "mlx".to_string(),
        }
    }

    /// 创建纯 CPU 节点
    pub fn cpu(cpu_name: &str, cores: u32, memory_gb: f64) -> Self {
        Self {
            node_id: uuid::Uuid::new_v4(),
            num_gpus: 0,
            gpu_name: cpu_name.to_string(),
            tflops_fp16: 0.01 * cores as f64, // 极粗估
            memory_gb,
            memory_bandwidth_gbps: 50.0, // DDR5 ~50 GB/s
            device_type: "cpu".to_string(),
        }
    }
}

/// 节点动态状态 — 对应 Parallax Node 类
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// 节点 ID
    pub node_id: NodeId,
    /// 硬件信息 (不可变)
    pub hardware: NodeHardwareInfo,
    /// 当前状态
    pub status: NodeStatus,

    // === Parallax 层分配 ===
    /// 起始层 (inclusive)
    pub start_layer: Option<u32>,
    /// 结束层 (exclusive)
    pub end_layer: Option<u32>,

    // === 负载追踪 ===
    /// 当前处理的请求数
    pub current_requests: u32,
    /// 最大并发请求数 (受 KV Cache 限制)
    pub max_requests: u32,

    // === 性能数据 (实时) ===
    /// 实测平均每层延迟 (ms)
    pub avg_layer_latency_ms: Option<f64>,
    /// 最后心跳时间
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    /// RTT 到其他节点 (node_id -> rtt_ms)
    pub rtt_to_nodes: std::collections::HashMap<NodeId, f64>,

    // === Roofline 参数 ===
    /// KV Cache 占内存比例 (默认 0.3)
    pub kvcache_mem_ratio: f64,
    /// 模型参数占用内存比例 (默认 0.5)
    pub param_mem_ratio: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeStatus {
    Standby,   // 待命 (空闲)
    Active,    // 活跃 (有层分配)
    Overloaded,// 过载
    Offline,   // 离线
    Degraded,  // 降级
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeStatus::Standby => write!(f, "standby"),
            NodeStatus::Active => write!(f, "active"),
            NodeStatus::Overloaded => write!(f, "overloaded"),
            NodeStatus::Offline => write!(f, "offline"),
            NodeStatus::Degraded => write!(f, "degraded"),
        }
    }
}

impl NodeInfo {
    pub fn is_overloaded(&self) -> bool {
        self.status == NodeStatus::Overloaded
            || self.current_requests >= self.max_requests
    }

    pub fn is_online(&self) -> bool {
        self.status != NodeStatus::Offline
    }

    /// 节点承载的层数
    pub fn num_current_layers(&self) -> u32 {
        match (self.start_layer, self.end_layer) {
            (Some(start), Some(end)) => end.saturating_sub(start),
            _ => 0,
        }
    }

    /// 是否托管指定层
    pub fn hosts_layer(&self, layer_id: u32) -> bool {
        match (self.start_layer, self.end_layer) {
            (Some(start), Some(end)) => layer_id >= start && layer_id < end,
            _ => false,
        }
    }

    /// 获取到另一个节点的 RTT
    pub fn get_rtt_to(&self, other: &NodeInfo) -> f64 {
        if self.node_id == other.node_id {
            0.0
        } else {
            self.rtt_to_nodes
                .get(&other.node_id)
                .copied()
                .unwrap_or(f64::INFINITY)
        }
    }

    /// 负载率 (0.0 - 1.0+)
    pub fn load_ratio(&self) -> f64 {
        if self.max_requests == 0 {
            0.0
        } else {
            self.current_requests as f64 / self.max_requests as f64
        }
    }

    /// 有效层延迟 (实测值 + 负载补偿)
    pub fn effective_layer_latency_ms(&self) -> f64 {
        if self.is_overloaded() {
            f64::INFINITY
        } else if let Some(avg) = self.avg_layer_latency_ms {
            // 负载补偿: 每增加 1 个请求, 延迟增加 5% * 负载率
            avg * (1.0 + 0.05 * self.load_ratio())
        } else {
            // 回退到 Roofline 估算
            self.roofline_estimate_ms()
        }
    }

    /// Roofline 模型估算延迟
    pub fn roofline_estimate_ms(&self) -> f64 {
        // 简化的 Roofline 模型:
        // latency = max(compute_bound, io_bound)
        // compute_bound = FLOPs / TFLOPS
        // io_bound = IO_bytes / bandwidth
        //
        // 这里用一个粗略的经验公式:
        // 基础延迟 ≈ 1/TFLOPS_per_layer * factor
        let base = 1.0 / (self.hardware.tflops_fp16.max(0.001));
        let layers = self.num_current_layers().max(1) as f64;
        base * layers * 1000.0 // 转换为 ms
    }
}

/// Pipeline 定义 — 一组连续的节点链, 覆盖完整的模型层范围
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: PipelineId,
    /// 组成 pipeline 的节点 ID 序列 (有序)
    pub node_ids: Vec<NodeId>,
    /// 覆盖的层范围 [start, end)
    pub layer_range: (u32, u32),
    /// 预估端到端延迟 (ms)
    pub estimated_latency_ms: f64,
    /// 吞吐能力 (requests/sec)
    pub throughput: f64,
}

impl Pipeline {
    /// 检查 pipeline 是否完整 (覆盖全部层)
    pub fn is_complete(&self, total_layers: u32) -> bool {
        self.layer_range.0 == 0 && self.layer_range.1 >= total_layers
    }

    /// pipeline 中的阶段数 (= 节点数)
    pub fn num_stages(&self) -> usize {
        self.node_ids.len()
    }
}

/// 集群资源摘要
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterResourceSummary {
    pub total_nodes: usize,
    pub active_nodes: usize,
    pub total_gpus: u32,
    pub total_tflops: f64,
    pub total_memory_gb: f64,
    pub avg_load_ratio: f64,
    pub available_pipelines: usize,
}
