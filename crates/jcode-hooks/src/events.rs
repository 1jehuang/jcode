// ════════════════════════════════════════════════════════════════
// Hook 事件类型定义 — 7 类 Hook 点
// ════════════════════════════════════════════════════════════════

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// 全局唯一事件 ID
pub type EventId = Uuid;

// ════════════════════════════════════════════════════════════════
// 事件类型枚举 (用于路由到正确的 Handler 链)
// ════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEventType {
    // ── Session 级别 ──
    /// 会话开始
    SessionStart,
    /// 会话结束
    SessionEnd,

    // ── Agent 执行 ──
    /// Agent 执行前 (可注入上下文/修改 prompt)
    PreAgentExecute,
    /// Agent 执行后 (可修改结果/记录指标)
    PostAgentExecute,

    // ── Prompt 注入 ──
    /// 在发送给 LLM 前, 可修改 system/user prompt
    PrePrompt,

    // ── 工具调用 ──
    /// 工具调用前 (权限检查、参数校验、日志)
    PreToolCall,
    /// 工具调用后 (结果处理、缓存、副作用)
    PostToolCall,

    // ── HTTP 请求 ──
    /// 发出 HTTP 请求前 (SSRF 检查、Header 注入)
    PreHttpRequest,
    /// 收到 HTTP 响应后 (响应处理、缓存)
    PostHttpResponse,

    // ── 安全检查 ──
    /// SSRF / 安全检查点
    SsrfCheck,

    // ── 自定义 ──
    /// 自定义事件 (携带 name + payload)
    Custom(String),
}

impl std::fmt::Display for HookEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionStart => write!(f, "session.start"),
            Self::SessionEnd => write!(f, "session.end"),
            Self::PreAgentExecute => write!(f, "agent.pre_execute"),
            Self::PostAgentExecute => write!(f, "agent.post_execute"),
            Self::PrePrompt => write!(f, "prompt.pre"),
            Self::PreToolCall => write!(f, "tool.pre_call"),
            Self::PostToolCall => write!(f, "tool.post_call"),
            Self::PreHttpRequest => write!(f, "http.pre_request"),
            Self::PostHttpResponse => write!(f, "http.post_response"),
            Self::SsrfCheck => write!(f, "security.ssrf_check"),
            Self::Custom(name) => write!(f, "custom.{}", name),
        }
    }
}

// ════════════════════════════════════════════════════════════════
// 事件基类 + 各类型的具体事件
// ════════════════════════════════════════════════════════════════

/// 所有 Hook 事件的基类
#[derive(Debug, Clone)]
pub struct HookEvent {
    pub id: EventId,
    pub event_type: HookEventType,
    pub timestamp: DateTime<Utc>,
    pub session_id: Option<String>,
}

impl HookEvent {
    pub fn new(event_type: HookEventType, session_id: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type,
            timestamp: Utc::now(),
            session_id,
        }
    }
}

// ─── Session Events ─────────────────────────────────

#[derive(Debug, Clone)]
pub struct SessionStartEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub session_id: String,
    pub user_id: Option<String>,
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionEndEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub session_id: String,
    pub reason: SessionEndReason,
    pub duration_secs: f64,
    pub total_turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionEndReason {
    Normal,
    Timeout,
    Error(String),
    UserDisconnect,
}

// ─── Agent Execution Events ─────────────────────────

#[derive(Debug, Clone)]
pub struct PreAgentExecuteEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub agent_id: String,
    pub turn_number: u32,
    pub user_message: String,
    /// 可修改的 system prompt (Handler 可以追加指令)
    pub system_prompt: String,
}

#[derive(Debug, Clone)]
pub struct PostAgentExecuteEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub agent_id: String,
    pub turn_number: u32,
    pub success: bool,
    pub tokens_used: u32,
    pub duration_ms: u64,
}

// ─── Prompt Injection Event ─────────────────────────

#[derive(Debug, Clone)]
pub struct PrePromptEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub messages: Vec<PromptMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: String,
    pub content: String,
}

// ─── Tool Call Events ───────────────────────────────

#[derive(Debug, Clone)]
pub struct PreToolCallEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub is_readonly: bool,
    /// Handler 可以设置此字段来阻止执行
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PostToolCallEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub tool_call_id: String,
    pub tool_name: String,
    pub success: bool,
    pub duration_ms: u64,
    pub output_preview: Option<String>,
}

// ─── HTTP Request/Response Events ─────────────────────

#[derive(Debug, Clone)]
pub struct PreHttpRequestEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PostHttpResponseEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub url: String,
    pub status_code: u16,
    pub response_size_bytes: usize,
    pub duration_ms: u64,
}

// ─── Security Events ─────────────────────────────────

#[derive(Debug, Clone)]
pub struct SsrfCheckEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub url: String,
    pub allowed: bool,
    pub block_reason: Option<String>,
}

// ─── Custom Event ───────────────────────────────────

#[derive(Debug, Clone)]
pub struct CustomEvent {
    #[allow(dead_code)]
    pub base: HookEvent,
    pub name: String,
    pub payload: serde_json::Value,
}

// ════════════════════════════════════════════════════════════════
// 枚举包装: 将所有具体事件统一为一个 enum
// ════════════════════════════════════════════════════════════════

/// 统一事件枚举 (用于 EventBus 内部传递)
pub enum HookEventData {
    SessionStart(SessionStartEvent),
    SessionEnd(SessionEndEvent),
    PreAgentExecute(PreAgentExecuteEvent),
    PostAgentExecute(PostAgentExecuteEvent),
    PrePrompt(PrePromptEvent),
    PreToolCall(PreToolCallEvent),
    PostToolCall(PostToolCallEvent),
    PreHttpRequest(PreHttpRequestEvent),
    PostHttpResponse(PostHttpResponseEvent),
    SsrfCheck(SsrfCheckEvent),
    Custom(CustomEvent),
}

impl HookEventData {
    pub fn event_type(&self) -> HookEventType {
        match self {
            Self::SessionStart(_) => HookEventType::SessionStart,
            Self::SessionEnd(_) => HookEventType::SessionEnd,
            Self::PreAgentExecute(_) => HookEventType::PreAgentExecute,
            Self::PostAgentExecute(_) => HookEventType::PostAgentExecute,
            Self::PrePrompt(_) => HookEventType::PrePrompt,
            Self::PreToolCall(_) => HookEventType::PreToolCall,
            Self::PostToolCall(_) => HookEventType::PostToolCall,
            Self::PreHttpRequest(_) => HookEventType::PreHttpRequest,
            Self::PostHttpResponse(_) => HookEventType::PostHttpResponse,
            Self::SsrfCheck(_) => HookEventType::SsrfCheck,
            Self::Custom(name) => HookEventType::Custom(name.name.clone()),
        }
    }

    pub fn base(&self) -> &HookEvent {
        match self {
            Self::SessionStart(e) => &e.base,
            Self::SessionEnd(e) => &e.base,
            Self::PreAgentExecute(e) => &e.base,
            Self::PostAgentExecute(e) => &e.base,
            Self::PrePrompt(e) => &e.base,
            Self::PreToolCall(e) => &e.base,
            Self::PostToolCall(e) => &e.base,
            Self::PreHttpRequest(e) => &e.base,
            Self::PostHttpResponse(e) => &e.base,
            Self::SsrfCheck(e) => &e.base,
            Self::Custom(e) => &e.base,
        }
    }
}
