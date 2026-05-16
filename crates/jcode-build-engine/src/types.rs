//! # 编译引擎核心数据类型
//!
//! 定义编译引擎使用的所有数据结构，包括：
//! - 编译任务/请求/结果
//! - 节点资源信息
//! - 缓存配置和统计
//! - 算力市场（供需/定价）
//! - 任务分解与调度
//! - 环境与工具链规范
//! - WebSocket 事件

use serde::{Deserialize,Serialize};
use std::collections::HashMap;
use uuid::Uuid;
use chrono::{DateTime,Utc};

// ══════════════════════════════════════════════════════════════════
// 基础类型别名 (Newtype wrapper)
// ══════════════════════════════════════════════════════════════════

#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash,serde::Serialize,serde::Deserialize)]
pub struct BuildId(pub Uuid);

#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash,serde::Serialize,serde::Deserialize)]
pub struct ProjectId(pub Uuid);

#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash,serde::Serialize,serde::Deserialize)]
pub struct NodeId(pub Uuid);

#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash,serde::Serialize,serde::Deserialize)]
pub struct TaskId(pub Uuid);

impl BuildId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
    pub fn as_uuid(&self) -> &Uuid { &self.0 }
}
impl ProjectId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
}
impl NodeId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
}
impl TaskId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
}

impl Default for BuildId { fn default() -> Self { Self::new() } }
impl Default for ProjectId { fn default() -> Self { Self::new() } }
impl Default for NodeId { fn default() -> Self { Self::new() } }
impl Default for TaskId { fn default() -> Self { Self::new() } }

// -- Display impls --

impl std::fmt::Display for BuildId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ══════════════════════════════════════════════════════════════════
// 编译类型与状态枚举
// ═════════════════════════════════════════════════════════════════

/// 编译构建类型
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildType {
    /// 全量编译
    Full,
    /// 增量编译 (仅编译变更文件)
    Incremental,
    /// 测试编译 (编译 + 运行测试)
    Test,
    /// 清理编译 (clean + build)
    Clean,
    /// 自定义编译命令
    Custom(String),
}

impl Default for BuildType {
    fn default() -> Self { Self::Full }
}

impl std::fmt::Display for BuildType {
    fn fmt(&self,f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f,"full"),
            Self::Incremental => write!(f,"incremental"),
            Self::Test => write!(f,"test"),
            Self::Clean => write!(f,"clean"),
            Self::Custom(cmd) => write!(f,"custom:{}",cmd),
        }
    }
}

/// 编译状态机
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    /// 已排队，等待调度
    Queued,
    /// 调度中，等待分配节点
    Scheduling,
    /// 编译执行中
    Running,
    /// 编译成功
    Success,
    /// 编译失败
    Failed,
    /// 用户取消
    Cancelled,
    /// 编译超时
    Timeout,
    /// 部分成功 (多任务场景)
    PartialSuccess,
}

impl BuildStatus {
    /// 是否为终态（不可再变更的状态）
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Success | Self::Failed | Self::Cancelled | Self::Timeout | Self::PartialSuccess)
    }

    /// 是否为活跃态（正在处理中的状态）
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Scheduling | Self::Running)
    }
}

impl Default for BuildStatus {
    fn default() -> Self { Self::Queued }
}

impl std::fmt::Display for BuildStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Queued => write!(f,"queued"),
            Self::Scheduling => write!(f,"scheduling"),
            Self::Running => write!(f,"running"),
            Self::Success => write!(f,"success"),
            Self::Failed => write!(f,"failed"),
            Self::Cancelled => write!(f,"cancelled"),
            Self::Timeout => write!(f,"timeout"),
            Self::PartialSuccess => write!(f,"partial_success"),
        }
    }
}

/// 支持的编程语言
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgrammingLanguage {
    Rust,
    Go,
    Python,
    Java,
    JavaScript,
    TypeScript,
    Cpp,
    C,
    DotNet,
    Swift,
    Kotlin,
    Other(String),
}

impl ProgrammingLanguage {
    /// 默认编译命令
    pub fn default_build_command(&self) -> &'static str {
        match self {
            Self::Rust => "cargo build --release",
            Self::Go => "go build ./...",
            Self::Python => "pip install -e . && pytest",
            Self::Java => "mvn compile",
            Self::JavaScript => "npm run build",
            Self::TypeScript => "tsc --build",
            Self::Cpp => "cmake --build build",
            Self::C => "make",
            Self::DotNet => "dotnet build",
            Self::Swift => "swift build",
            Self::Kotlin => "gradle buildKotlin",
            Self::Other(_) => "echo 'unknown language'",
        }
    }

    /// 语言显示名
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Go => "Go",
            Self::Python => "Python",
            Self::Java => "Java",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Cpp => "C++",
            Self::C => "C",
            Self::DotNet => ".NET",
            Self::Swift => "Swift",
            Self::Kotlin => "Kotlin",
            Self::Other(_) => "Other",
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 编译请求 / 响应
// ═════════════════════════════════════════════════════════════════

/// 编译请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRequest {
    #[serde(default)]
    pub id: Option<BuildId>,
    /// 项目 ID
    pub project_id: ProjectId,
    /// 编译类型
    #[serde(default)]
    pub build_type: BuildType,
    /// Git 分支/commit
    pub branch: Option<String>,
    pub commit: Option<String>,
    /// 自定义编译命令 (当 build_type=Custom 时使用)
    pub custom_command: Option<String>,
    /// 环境变量
    #[serde(default)]
    pub env_vars: HashMap<String,String>,
    /// 构建选项
    #[serde(default)]
    pub options: BuildOptions,
    /// 优先级 (0-100，越高越优先)
    #[serde(default = "default_priority")]
    pub priority: u8,
    /// 请求时间戳
    #[serde(default = "Utc::now")]
    pub requested_at: DateTime<Utc>,
    /// 用户 ID
    pub user_id: Option<String>,
    /// 标签 (用于过滤/分组)
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_priority() -> u8 { 50 }

/// 编译响应 (提交后立即返回)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildResponse {
    pub build_id: BuildId,
    pub status: BuildStatus,
    pub message: String,
    pub estimated_duration_secs: Option<u64>,
    pub queue_position: Option<usize>,
    pub created_at: DateTime<Utc>,
}

/// 编译选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildOptions {
    /// 是否 clean 后再构建
    #[serde(default)]
    pub clean: bool,
    /// 并行编译 jobs 数
    #[serde(default = "default_parallel_jobs")]
    pub parallel_jobs: Option<u32>,
    /// 是否启用缓存
    #[serde(default = "default_true")]
    pub cache_enabled: bool,
    /// 目标平台 triple (如 x86_64-pc-windows-msvc)
    pub target_triple: Option<String>,
    /// Release 模式
    #[serde(default = "default_true")]
    pub release: bool,
    /// 特性 flags
    #[serde(default)]
    pub features: Vec<String>,
    /// 是否运行测试
    #[serde(default)]
    pub run_tests: bool,
    /// 超时秒数 (0 表示无限制)
    #[serde(default)]
    pub timeout_secs: u64,
    /// Docker 镜像覆盖
    pub docker_image: Option<String>,
    /// 资源限制
    #[serde(default)]
    pub resource_limits: ResourceLimits,
}

fn default_parallel_jobs() -> Option<u32> { None }
fn default_true() -> bool { true }

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            clean: false,
            parallel_jobs: None,
            cache_enabled: true,
            target_triple: None,
            release: true,
            features: Vec::new(),
            run_tests: false,
            timeout_secs: 600,
            docker_image: None,
            resource_limits: ResourceLimits::default(),
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 资源限制
// ═════════════════════════════════════════════════════════════════

/// 计算资源限制
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// CPU 核心数上限
    #[serde(default)]
    pub cpu_cores: Option<f64>,
    /// 内存上限 (MB)
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// 磁盘空间上限 (MB)
    #[serde(default)]
    pub disk_mb: Option<u64>,
    /// GPU 数量
    #[serde(default)]
    pub gpu_count: Option<u32>,
    /// GPU 显存要求 (MB)
    #[serde(default)]
    pub gpu_memory_mb: Option<u64>,
    /// 网络带宽限制 (Mbps)
    #[serde(default)]
    pub network_bandwidth_mbps: Option<u64>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_cores: None,
            memory_mb: None,
            disk_mb: None,
            gpu_count: None,
            gpu_memory_mb: None,
            network_bandwidth_mbps: None,
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 编译进度 / 日志 / 结果
// ═════════════════════════════════════════════════════════════════

/// 编译进度
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProgress {
    pub build_id: BuildId,
    /// 0.0 ~ 1.0
    pub percentage: f32,
    /// 当前阶段描述
    pub current_phase: String,
    /// 已用时间(秒)
    pub elapsed_secs: u64,
    /// 剩余预估时间(秒)，None 表示无法估计
    pub remaining_secs: Option<u64>,
    /// 已编译文件数
    pub compiled_files: usize,
    /// 总文件数 (如果已知)
    pub total_files: Option<usize>,
    pub updated_at: DateTime<Utc>,
}

/// 编译日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildLogEntry {
    /// 日志序号
    pub sequence: u64,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 日志级别
    pub level: LogLevel,
    /// 来源 (compiler/linker/test等)
    pub source: String,
    /// 内容
    pub message: String,
    /// 关联的文件路径
    pub file_path: Option<String>,
    /// 行号
    pub line_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// 编译最终结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildResult {
    pub build_id: BuildId,
    pub status: BuildStatus,
    /// 执行节点
    pub executed_on: Option<NodeId>,
    /// 开始时间
    pub started_at: Option<DateTime<Utc>>,
    /// 结束时间
    pub finished_at: Option<DateTime<Utc>>,
    /// 总耗时(毫秒)
    pub duration_ms: u64,
    /// 编译产物列表
    #[serde(default)]
    pub artifacts: Vec<BuildArtifact>,
    /// 错误摘要
    pub error_summary: Option<ErrorSummary>,
    /// 统计信息
    pub stats: BuildStats,
    /// AI 修复建议 (可选)
    pub ai_suggestions: Option<Vec<AiFixSuggestion>>,
}

/// 编译产物
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildArtifact {
    pub name: String,
    pub path: String,
    /// 文件大小(bytes)
    pub size_bytes: u64,
    /// SHA256 hash
    pub sha256: Option<String>,
    /// 产物类型 (binary/library/package/etc)
    pub artifact_type: ArtifactType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactType {
    Binary,
    LibraryStatic,
    LibraryDynamic,
    Package,
    Archive,
    Intermediate,
    Other,
}

/// 错误摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorSummary {
    pub error_count: u32,
    pub warning_count: u32,
    /// 主要错误信息
    pub primary_error: Option<String>,
    /// 按文件分组的错误
    #[serde(default)]
    pub errors_by_file: HashMap<String,Vec<FileError>>,
}

/// 单个文件的错误详情
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileError {
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub severity: ErrorSeverity,
    pub code: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorSeverity {
    Error,
    Warning,
    Note,
    Help,
}

/// 编译统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildStats {
    /// 源代码文件总数
    pub source_files: usize,
    /// 成功编译的文件
    pub compiled_files: usize,
    /// 缓存命中次数
    pub cache_hits: usize,
    /// 缓存未命中次数
    pub cache_misses: usize,
    /// 使用的 CPU 秒数
    pub cpu_seconds: f64,
    /// 峰值内存 (MB)
    pub peak_memory_mb: u64,
    /// 网络下载量 (bytes)
    pub network_downloaded_bytes: u64,
}

impl Default for BuildStats {
    fn default() -> Self {
        Self {
            source_files: 0,
            compiled_files: 0,
            cache_hits: 0,
            cache_misses: 0,
            cpu_seconds: 0.0,
            peak_memory_mb: 0,
            network_downloaded_bytes: 0,
        }
    }
}

impl Default for CacheStats {
    fn default() -> Self {
        Self {
            total_entries: 0,
            total_size_bytes: 0,
            hit_count: 0,
            miss_count: 0,
            hit_rate: 0.0,
            evicted_count: 0,
            last_access: None,
        }
    }
}

/// AI 修复建议
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiFixSuggestion {
    pub id: Uuid,
    pub error_pattern: String,
    pub description: String,
    /// 修复代码 diff
    pub fix_diff: Option<String>,
    /// 置信度 0~1
    pub confidence: f32,
    /// 自动应用是否安全
    pub safe_to_auto_apply: bool,
}

// ══════════════════════════════════════════════════════════════════
// 缓存相关类型
// ═════════════════════════════════════════════════════════════════

/// 缓存配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// 本地缓存最大大小(MB)
    #[serde(default = "default_cache_size")]
    pub max_local_size_mb: u64,
    /// 分布式缓存是否启用
    #[serde(default)]
    pub distributed_enabled: bool,
    /// 缓存 TTL (小时)
    #[serde(default = "default_cache_ttl")]
    pub ttl_hours: u64,
    /// LRU 淘汰策略阈值
    #[serde(default = "default_evict_threshold")]
    pub eviction_threshold: f64,
    /// 压缩缓存
    #[serde(default = "default_true")]
    pub compress: bool,
}

fn default_cache_size() -> u64 { 10_240 }  // 10 GB
fn default_cache_ttl() -> u64 { 168 }       // 7 天
fn default_evict_threshold() -> f64 { 0.9 } // 90% 时开始淘汰

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_local_size_mb: 10_240,
            distributed_enabled: false,
            ttl_hours: 168,
            eviction_threshold: 0.9,
            compress: true,
        }
    }
}

/// 缓存统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub total_entries: u64,
    pub total_size_bytes: u64,
    pub hit_count: u64,
    pub miss_count: u64,
    pub hit_rate: f64,
    pub evicted_count: u64,
    /// 最近访问时间
    pub last_access: Option<DateTime<Utc>>,
}

impl CacheStats {
    pub fn recalc_hit_rate(&mut self) {
        let total = self.hit_count + self.miss_count;
        self.hit_rate = if total > 0 {
            self.hit_count as f64 / total as f64
        } else {
            0.0
        };
    }
}

/// 缓存清理请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCleanRequest {
    /// 清理特定项目的缓存
    pub project_id: Option<ProjectId>,
    /// 清理特定语言的缓存
    pub language: Option<ProgrammingLanguage>,
    /// 清理超过指定时间的缓存
    pub older_than_hours: Option<u64>,
    /// 最大清理条目数 (0 = 无限)
    #[serde(default)]
    pub max_entries: u64,
}

/// 缓存清理结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCleanResult {
    pub entries_removed: u64,
    pub bytes_freed: u64,
    pub duration_ms: u64,
}

// ══════════════════════════════════════════════════════════════════
// 节点资源类型
// ═════════════════════════════════════════════════════════════════

/// 计算资源快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeResource {
    /// CPU 使用率 0.0~1.0
    pub cpu_usage: f64,
    /// 可用内存 MB
    pub available_memory_mb: u64,
    /// 总内存 MB
    pub total_memory_mb: u64,
    /// 可用磁盘 MB
    pub available_disk_mb: u64,
    /// 总磁盘 MB
    pub total_disk_mb: u64,
    /// GPU 信息
    #[serde(default)]
    pub gpus: Vec<GpuInfo>,
    /// 负载均衡系数 (1.0 = 正常负载)
    pub load_factor: f64,
}

/// GPU 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub device_id: u32,
    pub name: String,
    /// 总显存 MB
    pub total_memory_mb: u64,
    /// 已用显存 MB
    pub used_memory_mb: u64,
    /// GPU 使用率 0.0~1.0
    pub utilization: f64,
}

/// 节点容量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapacity {
    /// 最大并发任务数
    pub max_concurrent_tasks: u32,
    /// 最大 CPU 核心
    pub max_cpu_cores: f64,
    /// 最大内存 MB
    pub max_memory_mb: u64,
    /// 最大磁盘 MB
    pub max_disk_mb: u64,
    /// 是否支持 GPU
    #[serde(default)]
    pub supports_gpu: bool,
    /// 支持的语言列表
    #[serde(default)]
    pub supported_languages: Vec<ProgrammingLanguage>,
}

/// 节点状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    Online,
    Offline,
    Busy,
    Draining,      // 正在排空中 (不再接受新任务)
    Maintenance,   // 维护模式
    Unknown,
}

impl NodeStatus {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Online)
    }
}

impl Default for NodeStatus {
    fn default() -> Self { Self::Unknown }
}

/// 节点完整信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: NodeId,
    pub hostname: String,
    /// IP 地址列表
    #[serde(default)]
    pub addresses: Vec<String>,
    pub status: NodeStatus,
    pub capacity: NodeCapacity,
    pub current_resource: ComputeResource,
    /// 当前运行的任务数
    pub active_tasks: u32,
    /// 最后心跳时间
    pub last_heartbeat: DateTime<Utc>,
    /// 节点标签 (用于选择策略)
    #[serde(default)]
    pub labels: HashMap<String,String>,
    /// 节点地理位置
    pub region: Option<String>,
    /// 区域可用区
    pub zone: Option<String>,
    /// 节点启动时间
    pub started_at: DateTime<Utc>,
    /// 版本
    #[serde(default)]
    pub version: String,
}

// ══════════════════════════════════════════════════════════════════
// 算力市场类型 (供需 / 定价)
// ═════════════════════════════════════════════════════════════════

/// 供需条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupplyDemandEntry {
    /// 区域标识
    pub region: String,
    /// 可用算力 (标准化单位)
    pub supply: f64,
    /// 算力需求
    pub demand: f64,
    /// 供需比 (>1 供大于求, <1 供不应求)
    pub ratio: f64,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
}

impl SupplyDemandEntry {
    pub fn new(region: impl Into<String>, supply: f64, demand: f64) -> Self {
        let ratio = if demand > 0.0 { supply / demand } else { f64::INFINITY };
        Self {
            region: region.into(),
            supply,
            demand,
            ratio,
            timestamp: Utc::now(),
        }
    }
}

/// 定价规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingRule {
    pub rule_id: Uuid,
    pub name: String,
    /// 基础价格 (每CPU秒)
    pub base_price_per_cpu_sec: f64,
    /// 供需调节因子
    #[serde(default = "default_one")]
    pub supply_demand_factor: f64,
    /// 紧急加价倍率
    #[serde(default = "default_one")]
    pub urgency_multiplier: f64,
    /// 区域调节因子
    #[serde(default)]
    pub regional_adjustments: HashMap<String,f64>,
    /// 是否激活
    #[serde(default = "default_true")]
    pub active: bool,
}

fn default_one() -> f64 { 1.0 }

/// 动态定价结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicPrice {
    /// 最终价格 (每CPU秒)
    pub price_per_cpu_sec: f64,
    /// 预估总价格
    pub estimated_total_price: f64,
    /// 货币单位
    pub currency: String,
    /// 使用的定价规则
    pub rule_name: String,
    /// 价格有效时间窗口
    pub valid_until: DateTime<Utc>,
}

// ══════════════════════════════════════════════════════════════════
// 任务分解与依赖
// ═════════════════════════════════════════════════════════════════

/// 任务分解策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDecompositionStrategy {
    /// 按语言模块分解
    ByModule,
    /// 按文件粒度分解
    ByFile,
    /// 按目标(target)分解
    ByTarget,
    /// 按依赖图层级分解
    ByDependencyLayer,
    /// 不分解 (单任务)
    Monolithic,
}

/// 任务依赖关系
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDependency {
    pub task_id: TaskId,
    /// 依赖的任务 ID 列表
    pub depends_on: Vec<TaskId>,
    /// 依赖类型
    pub dep_type: DependencyType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyType {
    /// 强依赖 (必须完成才能开始)
    Hard,
    /// 弱依赖 (建议先完成但非必须)
    Soft,
    /// 数据依赖 (需要前者的输出作为输入)
    Data,
}

/// 分解后的编译子任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledTask {
    pub task_id: TaskId,
    pub parent_build_id: BuildId,
    /// 任务描述
    pub name: String,
    /// 命令
    pub command: String,
    /// 工作目录
    pub working_dir: String,
    /// 所需资源
    pub resource_requirements: ResourceLimits,
    /// 预估耗时(秒)
    pub estimated_duration_secs: u64,
    /// 任务状态
    #[serde(default)]
    pub status: BuildStatus,
    /// 依赖列表
    #[serde(default)]
    pub dependencies: Vec<TaskDependency>,
    /// 重试计数
    #[serde(default)]
    pub retry_count: u32,
    /// 最大重试次数
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// 创建时间
    pub created_at: DateTime<Utc>,
}

fn default_max_retries() -> u32 { 3 }

// ══════════════════════════════════════════════════════════════════
// 调度决策类型
// ═════════════════════════════════════════════════════════════════

/// 调度计划
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulePlan {
    pub plan_id: Uuid,
    pub build_id: BuildId,
    /// 任务到节点的映射
    pub assignments: Vec<TaskAssignment>,
    /// 预计总耗时(秒)
    pub estimated_total_secs: u64,
    /// 生效时间
    pub scheduled_at: DateTime<Utc>,
}

/// 任务分配 (哪个任务在哪个节点上执行)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub task_id: TaskId,
    pub node_id: NodeId,
    /// 优先级
    pub priority: u8,
    /// 预计开始时间
    pub estimated_start: DateTime<Utc>,
    /// 预计结束时间
    pub estimated_end: DateTime<Utc>,
}

/// 调度决策结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScheduleDecision {
    /// 立即调度
    ScheduleNow(SchedulePlan),
    /// 排队等待 (所有节点忙碌)
    Queue {
        queue_position: usize,
        estimated_wait_secs: u64,
    },
    /// 拒绝 (资源不足或非法请求)
    Reject {
        reason: String,
        suggested_alternatives: Option<Vec<String>>,
    },
}

/// 健康检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub component: String,
    pub healthy: bool,
    /// 响应时间(毫秒)
    pub latency_ms: u64,
    pub message: String,
    pub checked_at: DateTime<Utc>,
}

impl HealthCheckResult {
    pub fn healthy(component: impl Into<String>, latency_ms: u64) -> Self {
        Self {
            component: component.into(),
            healthy: true,
            latency_ms,
            message: "OK".to_string(),
            checked_at: Utc::now(),
        }
    }

    pub fn unhealthy(component: impl Into<String>, reason: impl Into<String>, latency_ms: u64) -> Self {
        Self {
            component: component.into(),
            healthy: false,
            latency_ms,
            message: reason.into(),
            checked_at: Utc::now(),
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 环境 / 容器 / 工具链规范
// ═════════════════════════════════════════════════════════════════

/// 环境规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentSpec {
    pub env_id: Uuid,
    pub name: String,
    /// 操作系统镜像
    pub os_image: String,
    /// 已安装的工具链
    #[serde(default)]
    pub toolchains: Vec<ToolchainSpec>,
    /// 预装的环境变量
    #[serde(default)]
    pub environment_variables: HashMap<String,String>,
    /// 资源默认配置
    #[serde(default)]
    pub default_resources: ResourceLimits,
}

/// 容器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    pub container_id: Option<Uuid>,
    /// Docker 镜像
    pub image: String,
    /// 资源限制
    pub limits: ResourceLimits,
    /// 挂载卷
    #[serde(default)]
    pub volume_mounts: Vec<VolumeMount>,
    /// 网络 mode
    #[serde(default = "default_network_mode")]
    pub network_mode: String,
    /// 是否特权容器
    #[serde(default)]
    pub privileged: bool,
    /// 超时秒数
    #[serde(default = "default_container_timeout")]
    pub timeout_secs: u64,
    /// 环境变量
    #[serde(default)]
    pub extra_env: HashMap<String,String>,
}

fn default_network_mode() -> String { "bridge".to_string() }
fn default_container_timeout() -> u64 { 600 }

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            container_id: None,
            image: "ubuntu:22.04".to_string(),
            limits: ResourceLimits::default(),
            volume_mounts: Vec::new(),
            network_mode: default_network_mode(),
            privileged: false,
            timeout_secs: 600,
            extra_env: HashMap::new(),
        }
    }
}

/// 卷挂载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    pub source: String,
    pub destination: String,
    #[serde(default)]
    pub read_only: bool,
}

/// 工具链规格
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainSpec {
    pub language: ProgrammingLanguage,
    /// 版本 (如 "1.78", "1.21", "3.12")
    pub version: String,
    /// Docker 镜像名
    pub image: String,
    /// 安装路径
    pub install_path: String,
    /// 可执行文件路径
    pub binary_path: String,
    /// 验证命令 (用于检查安装是否正确)
    pub verify_command: String,
    /// 是否预拉取
    #[serde(default)]
    pub pre_pulled: bool,
}

// ══════════════════════════════════════════════════════════════════
// WebSocket 事件类型
// ═════════════════════════════════════════════════════════════════

/// WebSocket 消息事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsEvent {
    /// 事件类型
    #[serde(rename = "type")]
    pub event_type: WsMessageType,
    /// 关联的 build_id
    pub build_id: BuildId,
    /// 事件载荷 (根据 event_type 不同而不同)
    pub payload: serde_json::Value,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
}

/// WebSocket 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsMessageType {
    /// 编译状态变更
    StatusChanged,
    /// 进度更新
    Progress,
    /// 日志输出
    Log,
    /// 编译完成 (终态)
    Completed,
    /// 队列位置更新
    QueueUpdate,
    /// 错误通知
    Error,
    /// 取消确认
    Cancelled,
    /// 心跳
    Heartbeat,
}

impl WsEvent {
    /// 创建状态变更事件
    pub fn status_changed(build_id: BuildId, status: BuildStatus) -> Self {
        Self {
            event_type: WsMessageType::StatusChanged,
            build_id,
            payload: serde_json::json!({ "status": status }),
            timestamp: Utc::now(),
        }
    }

    /// 创建进度事件
    pub fn progress(build_id: BuildId, progress: BuildProgress) -> Self {
        Self {
            event_type: WsMessageType::Progress,
            build_id,
            payload: serde_json::to_value(progress).unwrap_or_default(),
            timestamp: Utc::now(),
        }
    }

    /// 创建日志事件
    pub fn log(build_id: BuildId, entry: BuildLogEntry) -> Self {
        Self {
            event_type: WsMessageType::Log,
            build_id,
            payload: serde_json::to_value(entry).unwrap_or_default(),
            timestamp: Utc::now(),
        }
    }

    /// 创建完成事件
    pub fn completed(build_id: BuildId, result: &BuildResult) -> Self {
        Self {
            event_type: WsMessageType::Completed,
            build_id,
            payload: serde_json::to_value(result).unwrap_or_default(),
            timestamp: Utc::now(),
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 队列管理类型
// ═════════════════════════════════════════════════════════════════

/// 队列项
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub request: BuildRequest,
    pub queued_at: DateTime<Utc>,
    /// 重试次数
    pub retry_count: u32,
}

/// 队列统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStats {
    pub pending_count: usize,
    pub running_count: usize,
    pub completed_today: u64,
    pub failed_today: u64,
    pub avg_wait_time_secs: f64,
    pub avg_build_time_secs: f64,
}
