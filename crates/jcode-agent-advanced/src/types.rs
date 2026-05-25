// ════════════════════════════════════════════════════════════════
// Agent 循环核心类型定义
// 对应 Claude Code: src/query.ts State 类型 (L201-217)
// ════════════════════════════════════════════════════════════════

use std::collections::HashMap;
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

use jcode_types::*;
use jcode_provider_core::models::*;

use crate::abort_controller::{AbortController, AbortSignal, AbortReason};
use crate::model_fallback::FallbackTrigger;

/// 工具执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub output: Option<String>,
    pub is_error: bool,
    pub duration_ms: u64,
}

impl ToolResult {
    pub fn success(tool_call_id: String, output: String) -> Self {
        Self {
            tool_call_id,
            output: Some(output),
            is_error: false,
            duration_ms: 0,
        }
    }
    
    pub fn error(tool_call_id: String, error: String) -> Self {
        Self {
            tool_call_id,
            output: Some(error),
            is_error: true,
            duration_ms: 0,
        }
    }
}

/// 助手消息 (完整响应)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub role: String,
    pub content: Vec<ContentBlock>,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
    pub usage: Option<UsageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// 模型标识符
pub type ModelId = String;

/// Agent 循环跨迭代状态 (对应 Claude Code State)
#[derive(Debug, Clone)]
pub struct AgentLoopState {
    /// 完整消息历史 (system + user + assistant + tool)
    pub messages: Vec<Message>,
    
    /// 工具使用上下文 (含 abort controller)
    pub tool_use_context: ToolUseContext,
    
    /// 自动压缩追踪状态
    pub auto_compact_tracking: Option<AutoCompactTrackingState>,
    
    /// maxOutputTokens 恢复计数器
    pub max_output_tokens_recovery_count: u32,
    
    /// 是否已尝试 reactive compact
    pub has_attempted_reactive_compact: bool,
    
    /// 动态覆盖的输出 token 限制
    pub max_output_tokens_override: Option<u32>,
    
    /// 待处理的工具摘要 Promise
    pub pending_tool_use_summary: Option<String>,
    
    /// Stop hook 是否激活
    pub stop_hook_active: bool,
    
    /// 当前轮次计数
    pub turn_count: u32,
    
    /// 上次 continue 的原因
    pub last_transition: Option<ContinueReason>,
}

/// 工具使用上下文 (含 abort 控制)
#[derive(Debug, Clone)]
pub struct ToolUseContext {
    /// Abort 控制器 — 流式响应中断的核心
    pub abort_controller: AbortController,
    
    /// 当前活跃的工具调用 ID 列表
    pub active_tool_ids: Vec<String>,
    
    /// 已完成的工具结果
    pub completed_results: HashMap<String, ToolResult>,
    
    /// 工具执行超时设置
    pub default_tool_timeout: Duration,
}

impl ToolUseContext {
    pub fn new() -> Self {
        Self {
            abort_controller: AbortController::new(),
            active_tool_ids: Vec::new(),
            completed_results: HashMap::new(),
            default_tool_timeout: Duration::from_secs(120),
        }
    }
    
    pub fn signal(&self) -> &AbortSignal {
        self.abort_controller.signal()
    }
    
    pub fn abort(&self, reason: AbortReason) {
        self.abort_controller.abort(reason);
    }
}

/// 自动压缩追踪状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoCompactTrackingState {
    /// 上次 compact 的消息数量
    pub last_compact_message_count: usize,
    
    /// 累计 compact 次数
    pub total_compacts: u32,
    
    /// 是否已触发过 snip compact
    pub snip_triggered: bool,
    
    /// 是否已触发过 collapse
    pub collapse_triggered: bool,
}

/// Continue 原因枚举 (对应 Claude Code needsFollowUp)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContinueReason {
    /// 有待执行的 tool_use blocks
    HasToolUses,
    /// 需要用户输入
    NeedsUserInput,
    /// 达到最大轮次限制
    MaxTurnsReached,
    /// Token 预算耗尽，需要压缩
    TokenBudgetExhausted,
    /// Stop hook 返回了非空结果
    StopHookResult,
    /// 模型请求重试 (网络错误/限流)
    RetryableError,
}

/// 终端状态 (循环退出原因)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TerminalState {
    /// 正常完成
    Completed { reason: String },
    /// 用户取消 / Abort
    Aborted { reason: AbortReason },
    /// 最大轮次达到
    MaxTurnsExceeded { count: u32 },
    /// 错误终止
    Error { message: String, recoverable: bool },
    /// 成本预算耗尽
    BudgetExceeded { cost: f64, limit: f64 },
}

/// 循环事件流 (AsyncGenerator 对应)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoopEvent {
    // === 阶段 1: API 调用相关 ===
    /// 开始发起 LLM 请求
    StreamRequestStart { model: String, input_tokens: u32 },
    
    /// 收到第一个 token (TTFT - Time To First Token)
    FirstTokenReceived { latency_ms: u64 },
    
    /// 流式 content block delta
    ContentBlockDelta { 
        index: usize, 
        text: String,
        is_thinking: bool,
    },
    
    /// 流式 tool_use 开始
    ToolUseStarted { id: String, name: String, input: serde_json::Value },
    
    /// 流式结束 (完整消息)
    MessageComplete { message: AssistantMessage },
    
    // === 阶段 2: 工具执行相关 ===
    /// 工具执行开始
    ToolExecutionStart { id: String, name: String },
    
    /// 工具执行进度更新
    ToolProgress { id: String, progress: f64, detail: Option<String> },
    
    /// 工具执行完成
    ToolResultReady { result: ToolResult },
    
    /// 工具执行失败
    ToolExecutionFailed { id: String, error: String, retryable: bool },
    
    // === 阶段 3: 系统事件 ===
    /// 正在执行 compact
    CompactStarted { strategy: CompactStrategy },
    
    /// Compact 完成
    CompactCompleted { messages_removed: usize, new_count: usize },
    
    /// 模型降级触发
    ModelFallbackTriggered { from: String, to: String, reason: FallbackTrigger },
    
    /// 重试开始
    RetryAttempt { attempt: u32, max: u32, delay_ms: u64 },
    
    /// Token 使用统计
    TokenUsageUpdate { input_tokens: u32, output_tokens: u32, cache_hit_tokens: u32 },
    
    /// 成本追踪更新
    CostUpdate { total_cost: f64, session_cost: f64 },
    
    /// 警告信息
    Warning { level: WarningLevel, message: String },
}

/// 警告级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarningLevel {
    Info,
    Warn,
    Error,
}

/// 压缩策略类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactStrategy {
    /// MicroCompact: 仅截断旧消息的文本内容
    Micro,
    /// SnipCompact: 移除中间的消息块
    Snip,
    /// Collapse: 用 LLM 摘要替换多条消息
    Collapse,
    /// AutoCompact: 自动选择最佳策略
    Auto,
}

// ════════════════════════════════════════════════════════════════
// Agent Loop 配置
// ════════════════════════════════════════════════════════════════

/// Agent 循环配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLoopConfig {
    /// 单次会话最大轮次 (默认 100)
    pub max_turns: u32,
    
    /// 并行工具调用最大并发数 (默认 5)
    pub max_parallel_tools: usize,
    
    /// 工具执行默认超时 (秒) (默认 120s)
    pub tool_timeout_secs: u64,
    
    /// API 调用超时 (秒) (默认 300s)
    pub api_timeout_secs: u64,
    
    /// 启用自动 compact
    pub enable_auto_compact: bool,
    
    /// 触发 auto-compact 的消息阈值
    pub auto_compact_threshold: usize,
    
    /// 最大输出 token 限制
    pub max_output_tokens: u32,
    
    /// 最大恢复尝试次数
    pub max_output_tokens_recoveries: u32,
    
    /// 启用模型降级
    pub enable_model_fallback: bool,
    
    /// 降级链配置 (按优先级排序的模型列表)
    pub fallback_models: Vec<ModelId>,
    
    /// 成本预算上限 (美元), None = 无限制
    pub cost_budget_usd: Option<f64>,
    
    /// 启用流式中断
    pub enable_streaming_abort: bool,
    
    /// Abort grace period (ms)
    pub abort_grace_period_ms: u64,
    
    /// 启用 split prompt 缓存优化
    pub enable_split_prompt_caching: bool,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 100,
            max_parallel_tools: MAX_PARALLEL_TOOLS,
            tool_timeout_secs: 120,
            api_timeout_secs: 300,
            enable_auto_compact: true,
            auto_compact_threshold: 50,  // 50 条消息触发 compact
            max_output_tokens: 16384,
            max_output_tokens_recoveries: 3,
            enable_model_fallback: true,
            fallback_models: Vec::new(),  // 从外部注入
            cost_budget_usd: None,
            enable_streaming_abort: true,
            abort_grace_period_ms: ABORT_GRACE_PERIOD_MS,
            enable_split_prompt_caching: true,
        }
    }
}
