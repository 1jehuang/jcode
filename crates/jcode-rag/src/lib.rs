//! RAG Toolchain Closed-Loop System - 检索增强生成 + 工具链闭环
//!
//! ## 核心设计哲学
//!
//! **"模型只负责局部手术，系统负责全局导航和术后护理"**
//!
//! 针对超大型项目（30万行代码，Python/Rust/TS 混合技术栈），纯靠 LLM 上下文窗口不可行。
//! 本系统将 LLM 的能力限制在**可控范围内**：
//! - LLM 只处理 <1000 行的精确上下文
//! - 系统负责全局索引、定位、验证、回滚
//!
//! ## 五层防御体系
//!
//! ### 架构概览
//! - Layer 5: Debugging (调试层) - 日志注入+断点管理
//! - Layer 4: Validation (验证层) - 多语言编译+测试集成
//! - Layer 3: Editing (编辑层) - Diff/SearchReplace 安全编辑器
//! - Layer 2: Retrieval (检索层) - 三引擎融合检索
//! - Layer 1: Indexing (感知层) - LSP+Ctags 全局符号索引
//!
//! ### 各层职责
//!
//! **Layer 1: 感知层 (Indexing)** - 全局符号感知
//! - LSP 符号表 (函数/类/变量/类型)
//! - Ctags 兼容索引 (跨语言引用)
//! - 文件依赖图 (import/include 关系)
//! - Vector DB 语义索引 (代码向量嵌入)
//!
//! **Layer 2: 检索层 (Retrieval)** - 精准定位
//! - grep/ripgrep: 字符串精确搜索
//! - LSP Goto Definition: 符号跳转
//! - Vector DB Cosine Search: 语义相似度匹配
//! - File Activity Tracker: 用户行为加权
//! - 三引擎融合排序 (BM25 + Semantic + Activity)
//!
//! **Layer 3: 编辑层 (Editing)** - 安全修改
//! - Diff-based Editing (仅生成差异，不直接覆写)
//! - Search/Replace with Preview (预览确认)
//! - Atomic Transactions (原子操作，支持回滚)
//! - Conflict Detection (冲突检测与合并)
//!
//! **Layer 4: 验证层 (Validation)** - 多语言集成
//! - Rust: cargo check/cargo test/cargo clippy
//! - Python: mypyr/pytest/flake8
//! - TypeScript: tsc/eslint/vitest
//! - Shell: bash -n (语法检查)
//! - stderr 捕获 + 错误分类 + 自动修复建议
//!
//! **Layer 5: 调试层 (Debugging)** - 可观测性
//! - Automated Log Injection (关键点日志注入)
//! - Breakpoint Management (断点设置与管理)
//! - Execution Tracing (执行路径追踪)
//! - State Snapshot (状态快照与对比)

// ============== 模块声明 ==============

pub mod indexing_layer;
pub mod retrieval_layer;
pub mod editing_layer;
pub mod validation_layer;
pub mod debugging_layer;

// 重新导出主要类型
pub use indexing_layer::{
    GlobalSymbolIndexer, IndexingConfig, SymbolInfo,
};
pub use retrieval_layer::{
    MultiEngineRetriever, RetrievalConfig, FusedSearchResult,
    StringSearchProvider,
};
pub use editing_layer::{
    SafeEditor, EditingConfig, EditTransaction,
};
pub use validation_layer::{
    MultiLanguageValidator, ValidationConfig, ValidationResultSummary,
};
pub use debugging_layer::{
    ObservabilityManager, DebuggingConfig, DebugSession,
};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::{debug, error, info, warn};

// ============== 核心数据结构 ==============

/// 手术请求 (Surgical Request) - 一次完整的代码修改请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurgicalRequest {
    /// 请求 ID
    pub request_id: String,
    
    /// 用户意图描述 (自然语言)
    pub intent: String,
    
    /// 目标文件/范围
    pub target: TargetScope,
    
    /// 优先级
    pub priority: Priority,
    
    /// 安全模式开关
    pub safety_mode: SafetyMode,
    
    /// 元数据
    pub created_at: DateTime<Utc>,
    pub requested_by: String, // user/session id
}

/// 目标范围定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TargetScope {
    /// 单个文件
    SingleFile { path: PathBuf },
    
    /// 多个文件 (批量操作)
    MultipleFiles { paths: Vec<PathBuf> },
    
    /// 整个项目
    EntireProject { root: PathBuf },
    
    /// 函数级别 (指定文件+函数名)
    Function { file: PathBuf, function_name: String },
    
    /// 区域级别 (指定文件+行范围)
    Region { file: PathBuf, start_line: usize, end_line: usize },
}

/// 优先级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

/// 安全模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafetyMode {
    /// 只读分析 (不修改任何东西)
    ReadOnly,
    
    /// 安全模式 (需要确认才能应用)
    Safe,
    
    /// 自由模式 (自动应用，但保留完整日志)
    AutoWithLogging,
    
    /// 危险模式 (允许破坏性修改，需明确授权)
    Dangerous,
}

/// 手术结果 (Surgical Result) - 一次操作的完整结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurgicalResult {
    /// 对应的请求 ID
    pub request_id: String,
    
    /// 是否成功
    pub success: bool,
    
    /// 执行阶段记录
    pub phases: Vec<PhaseResult>,
    
    /// 最终状态
    pub final_state: FinalState,
    
    /// 影响范围分析
    pub impact_analysis: ImpactAnalysis,
    
    /// 统计信息
    pub stats: ExecutionStats,
    
    /// 时间戳
    pub completed_at: DateTime<Utc>,
}

/// 各阶段执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseResult {
    /// 阶段名称 (Indexing/Retrieval/Editing/Validation/Debugging)
    pub phase: PhaseName,
    
    /// 是否通过
    pub passed: bool,
    
    /// 耗时 (毫秒)
    pub duration_ms: u64,
    
    /// 详细输出
    pub output: PhaseOutput,
    
    /// 警告信息
    pub warnings: Vec<String>,
    
    /// 错误信息 (如果有)
    pub errors: Vec<String>,
}

/// 阶段名称
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PhaseName {
    Indexing,
    Retrieval,
    Editing,
    Validation,
    Debugging,
}

/// 阶段输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PhaseOutput {
    // Layer 1 输出
    IndexingOutput {
        symbols_found: usize,
        files_indexed: usize,
        index_duration_ms: u64,
    },
    
    // Layer 2 输出
    RetrievalOutput {
        context_windows: Vec<ContextWindow>,
        relevance_scores: Vec<f64>,
        retrieval_duration_ms: u64,
    },
    
    // Layer 3 输出
    EditingOutput {
        diffs_generated: Vec<TextDiff>,
        files_modified: Vec<PathBuf>,
        edit_duration_ms: u64,
    },
    
    // Layer 4 输出
    ValidationOutput {
        compilation_results: Vec<CompilationResult>,
        test_results: Vec<TestResult>,
        validation_duration_ms: u64,
    },
    
    // Layer 5 输出
    DebuggingOutput {
        logs_injected: Vec<LogInjection>,
        breakpoints_set: Vec<BreakpointInfo>,
        traces_captured: Vec<ExecutionTrace>,
        debug_duration_ms: u64,
    },
}

/// 上下文窗口 (发送给 LLM 的内容)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextWindow {
    /// 窗口 ID
    pub id: String,
    
    /// 内容片段列表
    pub segments: Vec<ContextSegment>,
    
    /// 总 token 数估算
    pub total_tokens: usize,
    
    /// 来源分布 (用于调试)
    pub source_breakdown: SourceBreakdown,
}

/// 上下文片段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSegment {
    /// 片段 ID
    pub id: String,
    
    /// 来源文件
    pub file_path: PathBuf,
    
    /// 内容
    pub content: String,
    
    /// 起始行号
    pub start_line: usize,
    
    /// 结束行号
    pub end_line: usize,
    
    /// 语言
    pub language: String,
    
    /// 相关性分数
    pub relevance_score: f64,
    
    /// 来源类型
    pub source: RetrievalSource,
}

/// 来源类型 (这个片段是怎么找到的)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetrievalSource {
    /// 来自字符串搜索 (grep)
    StringMatch,
    
    /// 来自符号跳转 (LSP)
    SymbolReference,
    
    /// 来自语义搜索 (Vector DB)
    SemanticSimilarity,
    
    /// 来自文件活动追踪
    UserActivity,
    
    /// 用户手动指定
    ExplicitInclusion,
}

/// 来源分布统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceBreakdown {
    pub string_match_count: usize,
    pub symbol_reference_count: usize,
    pub semantic_similarity_count: usize,
    pub user_activity_count: usize,
    pub explicit_count: usize,
}

/// Text Diff (编辑差异)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDiff {
    /// 文件路径
    pub file_path: PathBuf,
    
    /// 差异类型
    pub diff_type: DiffType,
    
    /// 原始内容 (旧版本)
    pub old_content: Option<String>,
    
    /// 新内容 (新版本)
    pub new_content: Option<String>,
    
    /// unified diff 格式
    pub unified_diff: String,
    
    /// 变更行数统计
    pub stats: DiffStats,
}

/// 差异类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffType {
    Add,       // 新增内容
    Modify,    // 修改内容
    Delete,    // 删除内容
    Move,      // 移动内容
    Rename,    // 重命名
}

/// 差异统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffStats {
    pub additions: usize,
    pub deletions: usize,
    pub modifications: usize,
    pub files_changed: usize,
}

/// 最终状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FinalState {
    /// 成功完成，已应用更改
    Applied {
        changes_applied: usize,
        files_affected: Vec<PathBuf>,
    },
    /// 已回滚 (验证失败或用户取消)
    RolledBack {
        reason: String,
        original_state_snapshot: String,
    },
    /// 失败 (无法恢复)
    Failed {
        error: String,
        partial_changes: Vec<PathBuf>,
    },
}

/// 影响范围分析
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalysis {
    /// 直接影响的文件
    pub directly_affected_files: Vec<PathBuf>,
    
    /// 可能间接受影响的文件 (通过依赖关系)
    pub potentially_affected_files: Vec<PathBuf>,
    
    /// 受影响的测试套件
    pub affected_tests: Vec<String>,
    
    /// 风险等级
    pub risk_level: RiskLevel,
    
    /// 建议的回归测试
    pub suggested_regression_tests: Vec<TestSuggestion>,
}

/// 风险等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,       // 仅影响单个函数
    Medium,    // 影响模块内多个函数
    High,      // 影响跨模块接口
    Critical,  // 可能影响编译或核心功能
}

/// 测试建议
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSuggestion {
    pub test_name: String,
    pub test_file: PathBuf,
    pub reason: String, // 为什么建议运行这个测试
    pub priority: TestPriority,
}

/// 测试优先级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestPriority {
    MustRun,     // 必须运行
    Recommended, // 建议运行
    Optional,   // 可选
}

/// 编译结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilationResult {
    /// 语言
    pub language: Language,
    
    /// 命令
    pub command: String,
    
    /// 是否成功
    pub success: bool,
    
    /// 退出码
    pub exit_code: i32,
    
    /// stdout 输出
    pub stdout: String,
    
    /// stderr 输出 (错误/警告)
    pub stderr: String,
    
    /// 耗时 (毫秒)
    pub duration_ms: u64,
    
    /// 解析后的错误列表
    pub parsed_errors: Vec<ParsedError>,
}

/// 语言枚举
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Java,
    Shell,
    Other(String),
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "rust"),
            Self::Python => write!(f, "python"),
            Self::TypeScript => write!(f, "typescript"),
            Self::JavaScript => write!(f, "javascript"),
            Self::Go => write!(f, "go"),
            Self::Java => write!(f, "java"),
            Self::Shell => write!(f, "shell"),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}

/// 测试结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// 测试框架
    pub framework: TestFramework,
    
    /// 测试名称/路径
    pub test_name: String,
    
    /// 是否通过
    pub passed: bool,
    
    /// 耗时 (毫秒)
    pub duration_ms: u64,
    
    /// stdout
    pub stdout: String,
    
    /// stderr
    pub stderr: String,
}

/// 测试框架
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestFramework {
    CargoTest,
    Pytest,
    Vitest,
    Jest,
    GoTest,
    JUnit,
    BashTest,
}

/// 解析后的错误 (结构化错误信息)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedError {
    /// 错误级别
    pub severity: ErrorSeverity,
    
    /// 错误消息
    pub message: String,
    
    /// 文件路径
    pub file_path: Option<PathBuf>,
    
    /// 行号
    pub line: Option<usize>,
    
    /// 列号
    pub column: Option<usize>,
    
    /// 错误码/类型
    pub code: Option<String>,
    
    /// 是否可自动修复
    pub auto_fixable: bool,
    
    /// 修复建议 (如果可自动修复)
    pub fix_suggestion: Option<String>,
}

/// 错误严重程度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

/// 日志注入点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogInjection {
    /// 注入 ID
    pub id: String,
    
    /// 注入位置 (文件:行)
    pub location: InjectionLocation,
    
    /// 日志级别
    pub level: LogLevel,
    
    /// 日志模板 (支持变量替换)
    pub template: String,
    
    /// 条件触发 (可选)
    pub condition: Option<String>, // 表达式
    
    /// 是否激活
    pub is_active: bool,
}

/// 注入位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionLocation {
    pub file_path: PathBuf,
    pub line: usize,
    pub insert_before: bool, // true=在行前插入, false=在行后插入
}

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// 断点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointInfo {
    /// 断点 ID
    pub id: String,
    
    /// 位置
    pub location: BreakpointLocation,
    
    /// 断点条件 (可选)
    pub condition: Option<String>,
    
    /// 命中次数
    pub hit_count: u64,
    
    /// 是否启用
    pub enabled: bool,
}

/// 断点位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointLocation {
    pub file_path: PathBuf,
    pub line: usize,
}

/// 执行追踪
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    /// 追踪 ID
    pub trace_id: String,
    
    /// 执行步骤列表
    pub steps: Vec<ExecutionStep>,
    
    /// 总耗时
    pub total_duration_ms: u64,
}

/// 执行步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStep {
    /// 步骤序号
    pub step_number: usize,
    
    /// 类型
    pub step_type: StepType,
    
    /// 描述
    pub description: String,
    
    /// 位置
    pub location: Option<PathLocation>,
    
    /// 数据快照 (变量值等)
    pub data_snapshot: Option<serde_json::Value>,
    
    /// 时间戳
    pub timestamp_ms: u64,
}

/// 步骤类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepType {
    FunctionEnter,
    FunctionExit,
    LineExecuted,
    BranchTaken,
    VariableChanged,
    ErrorOccurred,
    LogEmitted,
}

/// 位置信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathLocation {
    pub file_path: PathBuf,
    pub line: usize,
    pub column: Option<usize>,
}

/// 执行统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStats {
    /// 总耗时 (毫秒)
    pub total_duration_ms: u64,
    
    /// 各阶段耗时
    pub phase_durations: HashMap<PhaseName, u64>,
    
    /// Token 使用量 (LLM API 调用)
    pub tokens_consumed: usize,
    
    /// 文件 I/O 操作次数
    pub file_io_operations: usize,
    
    /// 进程启动次数 (编译器/测试运行器)
    pub process_launches: usize,
}

// ============== RAG Orchestrator 核心逻辑 ==============

/// RAG 工具链闭环系统 - 手术总指挥官
pub struct RagToolchainOrchestrator {
    /// 配置
    config: OrchestratorConfig,
    
    /// 各层处理器
    indexing_layer: Arc<dyn IndexingLayer>,
    retrieval_layer: Arc<dyn RetrievalLayer>,
    editing_layer: Arc<dyn EditingLayer>,
    validation_layer: Arc<dyn ValidationLayer>,
    debugging_layer: Arc<dyn DebuggingLayer>,
    
    /// 历史记录
    history: Arc<RwLock<Vec<SurgicalResult>>>,
}

/// 编排器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// 最大上下文窗口大小 (tokens)
    pub max_context_window_tokens: usize,
    
    /// 默认安全模式
    pub default_safety_mode: SafetyMode,
    
    /// 是否自动提交 (false 则只生成 diff 不实际写入)
    pub auto_commit: bool,
    
    /// 并发限制
    pub max_concurrent_surgeries: usize,
    
    /// 重试策略
    pub retry_policy: RetryPolicy,
}

/// 重试策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// 最大重试次数
    pub max_attempts: usize,
    
    /// 重试间隔 (毫秒)
    pub retry_delay_ms: u64,
    
    /// 是否启用指数退避
    pub exponential_backoff: bool,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_context_window_tokens: 8000, // ~8K tokens (GPT-4 的一半)
            default_safety_mode: SafetyMode::Safe,
            auto_commit: false, // 默认不自动提交，需人工确认
            max_concurrent_surgeries: 3,
            retry_policy: RetryPolicy {
                max_attempts: 3,
                retry_delay_ms: 1000,
                exponential_backoff: true,
            },
        }
    }
}

impl RagToolchainOrchestrator {
    /// 创建新的编排器
    pub fn new(
        config: OrchestratorConfig,
        indexing: Arc<dyn IndexingLayer>,
        retrieval: Arc<dyn RetrievalLayer>,
        editing: Arc<dyn EditingLayer>,
        validation: Arc<dyn ValidationLayer>,
        debugging: Arc<dyn DebuggingLayer>,
    ) -> Self {
        Self {
            config,
            indexing_layer: indexing,
            retrieval_layer: retrieval,
            editing_layer: editing,
            validation_layer: validation,
            debugging_layer: debugging,
            history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 执行一次完整的手术流程
    pub async fn execute_surgery(&self, request: &SurgicalRequest) -> Result<SurgicalResult> {
        let start_time = std::time::Instant::now();
        
        info!(
            request_id = %request.request_id,
            intent = %request.intent,
            safety_mode = ?request.safety_mode,
            "🏥 Starting surgical procedure"
        );

        let mut phases = Vec::new();
        let mut final_state = FinalState::Failed {
            error: "Procedure not started".to_string(),
            partial_changes: Vec::new(),
        };

        // ========== Phase 1: 感知层 (Indexing) ==========
        info!(phase = "Indexing", "Phase 1/5: Building global awareness");
        
        let phase1_result = self.indexing_layer.build_context_index(request).await?;
        phases.push(phase1_result.clone());
        
        if !phase1_result.passed {
            final_state = FinalState::Failed {
                error: format!("Indexing phase failed: {:?}", phase1_result.errors),
                partial_changes: Vec::new(),
            };
            return Ok(self.finalize_result(request, phases, final_state, start_time).await);
        }

        // ========== Phase 2: 检索层 (Retrieval) ==========
        info!(phase = "Retrieval", "Phase 2/5: Locating surgical site");
        
        let phase2_result = self.retrieval_layer.retrieve_relevant_context(request, &phase1_result.output).await?;
        phases.push(phase2_result.clone());
        
        if !phase2_result.passed {
            final_state = FinalState::Failed {
                error: format!("Retrieval phase failed: {:?}", phase2_result.errors),
                partial_changes: Vec::new(),
            };
            return Ok(self.finalize_result(request, phases, final_state, start_time).await);
        }

        // ========== Phase 3: 编辑层 (Editing) ==========
        info!(phase = "Editing", "Phase 3/5: Performing precise incision");
        
        let phase3_result = self.editing_layer.generate_safe_edits(request, &phase2_result.output).await?;
        phases.push(phase3_result.clone());
        
        if !phase3_result.passed && request.safety_mode != SafetyMode::Dangerous {
            final_state = FinalState::RolledBack {
                reason: "Editing phase failed or user cancelled".to_string(),
                original_state_snapshot: "N/A".to_string(), // TODO: 实现快照
            };
            return Ok(self.finalize_result(request, phases, final_state, start_time).await);
        }

        // ========== Phase 4: 验证层 (Validation) ==========
        info!(phase = "Validation", "Phase 4/5: Post-op health check");
        
        let phase4_result = self.validation_layer.validate_changes(request, &phase3_result.output).await?;
        phases.push(phase4_result.clone());
        
        if !phase4_result.passed {
            // 验证失败 → 自动回滚
            warn!(phase = "Validation", "Validation failed, initiating rollback");
            
            let rollback_result = self.editing_layer.rollback_changes(request, &phase3_result.output).await?;
            phases.push(rollback_result);
            
            final_state = FinalState::RolledBack {
                reason: format!("Validation failed: {:?}", phase4_result.errors),
                original_state_snapshot: "N/A".to_string(),
            };
            
            return Ok(self.finalize_result(request, phases, final_state, start_time).await);
        }

        // ========== Phase 5: 调试层 (Debugging) ==========
        info!(phase = "Debugging", "Phase 5/5: Injecting observability");
        
        let phase5_result = self.debugging_layer.inject_debug_info(request, &phases).await?;
        phases.push(phase5_result);

        // ========== 成功！ ==========
        final_state = FinalState::Applied {
            changes_applied: self.count_total_changes(&phases),
            files_affected: self.collect_affected_files(&phases),
        };

        let result = self.finalize_result(request, phases.clone(), final_state, start_time).await;
        
        info!(
            request_id = %request.request_id,
            success = result.success,
            phases = phases.len(),
            duration_ms = result.stats.total_duration_ms,
            "✅ Surgical procedure completed successfully"
        );

        Ok(result)
    }

    /// 构建最终结果
    async fn finalize_result(
        &self,
        request: &SurgicalRequest,
        phases: Vec<PhaseResult>,
        final_state: FinalState,
        start_time: std::time::Instant,
    ) -> SurgicalResult {
        let total_duration = start_time.elapsed().as_millis() as u64;
        
        // 计算各阶段耗时
        let mut phase_durations = HashMap::new();
        for phase in &phases {
            phase_durations.insert(phase.phase, phase.duration_ms);
        }

        // 收集所有警告和错误
        let all_warnings: Vec<&String> = phases.iter()
            .flat_map(|p| p.warnings.iter())
            .collect();
        
        let all_errors: Vec<&String> = phases.iter()
            .flat_map(|p| p.errors.iter())
            .collect();

        // 记录到历史
        {
            let mut history = self.history.write();
            history.push(SurgicalResult {
                request_id: request.request_id.clone(),
                success: matches!(final_state, FinalState::Applied { .. }),
                phases: phases.clone(),
                final_state: final_state.clone(),
                impact_analysis: ImpactAnalysis {
                    directly_affected_files: self.collect_affected_files(&phases),
                    potentially_affected_files: Vec::new(), // TODO: 实现
                    affected_tests: Vec::new(), // TODO: 从 validation 层获取
                    risk_level: RiskLevel::Medium, // TODO: 基于 impact 分析计算
                    suggested_regression_tests: Vec::new(),
                },
                stats: ExecutionStats {
                    total_duration_ms: total_duration,
                    phase_durations: phase_durations.clone(),
                    tokens_consumed: 0, // TODO: 从 LLM 调用统计
                    file_io_operations: 0, // TODO: 统计
                    process_launches: 0, // TODO: 从 validation 层统计
                },
                completed_at: Utc::now(),
            });
        }

        SurgicalResult {
            request_id: request.request_id.clone(),
            success: matches!(&final_state, FinalState::Applied { .. }),
            phases: phases.clone(),
            final_state: final_state.clone(),
            impact_analysis: ImpactAnalysis {
                directly_affected_files: self.collect_affected_files(&phases),
                potentially_affected_files: Vec::new(),
                affected_tests: Vec::new(),
                risk_level: RiskLevel::Medium,
                suggested_regression_tests: Vec::new(),
            },
            stats: ExecutionStats {
                total_duration_ms: total_duration,
                phase_durations: phase_durations.clone(),
                tokens_consumed: 0,
                file_io_operations: 0,
                process_launches: 0,
            },
            completed_at: Utc::now(),
        }
    }

    /// 辅助方法: 统计总变更数
    fn count_total_changes(&self, phases: &[PhaseResult]) -> usize {
        phases.iter()
            .filter(|p| matches!(p.phase, PhaseName::Editing))
            .filter(|p| p.passed)
            .map(|p| {
                if let PhaseOutput::EditingOutput { ref diffs_generated, .. } = p.output {
                    diffs_generated.len()
                } else {
                    0
                }
            })
            .sum()
    }

    /// 辅助方法: 收集受影响文件
    fn collect_affected_files(&self, phases: &[PhaseResult]) -> Vec<PathBuf> {
        let mut files = HashSet::new();
        
        for phase in phases {
            if let PhaseOutput::EditingOutput { ref files_modified, .. } = phase.output {
                for f in files_modified {
                    files.insert(f.clone());
                }
            }
        }
        
        files.into_iter().collect()
    }
}

use std::time::Instant;

// ============== Trait 定义 (各层接口) ==============

/// Layer 1: 感知层 trait
#[async_trait::async_trait]
pub trait IndexingLayer: Send + Sync {
    async fn build_context_index(&self, request: &SurgicalRequest) -> Result<PhaseResult>;
    
    async fn get_project_stats(&self) -> Result<ProjectIndexStats>;
}

/// 项目索引统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectIndexStats {
    pub total_symbols: usize,
    pub total_files: usize,
    pub languages_detected: Vec<String>,
    pub index_build_time: DateTime<Utc>,
}

/// Layer 2: 检索层 trait
#[async_trait::async_trait]
pub trait RetrievalLayer: Send + Sync {
    async fn retrieve_relevant_context(
        &self, 
        request: &SurgicalRequest, 
        indexing_output: &PhaseOutput,
    ) -> Result<PhaseResult>;
    
    async fn search_symbol(&self, name: &str, language: Option<Language>) -> Result<Vec<SymbolMatch>>;
    
    async fn search_code_pattern(&self, pattern: &str, language: Option<Language>) -> Result<Vec<PatternMatch>>;
    
    async fn find_similar_code(&self, code: &str, language: Language, top_k: usize) -> Result<Vec<SimilarCodeMatch>>;
}

/// 符号匹配结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolMatch {
    pub symbol_name: String,
    pub kind: String,
    pub file_path: PathBuf,
    pub line: usize,
    pub definition: Option<String>,
}

/// 模式匹配结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternMatch {
    pub file_path: PathBuf,
    pub line: usize,
    pub matched_text: String,
    pub context_before: String,
    pub context_after: String,
}

/// 相似代码匹配
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarCodeMatch {
    pub file_path: PathBuf,
    pub similarity: f64,
    pub snippet: String,
}

/// Layer 3: 编辑层 trait
#[async_trait::async_trait]
pub trait EditingLayer: Send + Sync {
    async fn generate_safe_edits(
        &self, 
        request: &SurgicalRequest, 
        retrieval_output: &PhaseOutput,
    ) -> Result<PhaseResult>;
    
    async fn apply_edits(&self, edits: &[TextDiff]) -> Result<ApplyResult>;
    
    async fn rollback_changes(&self, request: &SurgicalRequest, edit_output: &PhaseOutput) -> Result<PhaseResult>;
    
    async fn preview_diff(&self, diff: &TextDiff) -> Result<PreviewResult>;
}

/// 应用结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResult {
    pub success: bool,
    pub applied_count: usize,
    pub failed_items: Vec<String>,
}

/// 预览结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewResult {
    pub unified_diff: String,
    pub estimated_risk: RiskLevel,
}

/// Layer 4: 验证层 trait
#[async_trait::async_trait]
pub trait ValidationLayer: Send + Sync {
    async fn validate_changes(
        &self, 
        request: &SurgicalRequest, 
        edit_output: &PhaseOutput,
    ) -> Result<PhaseResult>;
    
    async fn run_compilation(&self, language: Language, files: &[PathBuf]) -> Result<Vec<CompilationResult>>;
    
    async fn run_tests(&self, framework: TestFramework, tests: &[PathBuf]) -> Result<Vec<TestResult>>;
    
    async fn generate_regression_test_suite(&self, changes: &[TextDiff]) -> Result<Vec<TestSuggestion>>;
}

/// Layer 5: 调试层 trait
#[async_trait::async_trait]
pub trait DebuggingLayer: Send + Sync {
    async fn inject_debug_info(
        &self, 
        request: &SurgicalRequest, 
        phases: &[PhaseResult],
    ) -> Result<PhaseResult>;
    
    async fn set_breakpoint(&self, bp: BreakpointInfo) -> Result<()>;
    
    async fn remove_breakpoint(&self, bp_id: &str) -> Result<()>;
    
    async fn capture_execution_trace(&self, trace_config: TraceConfig) -> Result<ExecutionTrace>;
}

/// 追踪配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceConfig {
    pub include_stdlib: bool,
    pub max_depth: usize,
    pub focus_files: Vec<PathBuf>,
}
