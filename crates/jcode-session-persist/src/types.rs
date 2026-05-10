// ════════════════════════════════════════════════════════════════
// 会话持久化核心类型
// ════════════════════════════════════════════════════════════════

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// 会话状态 (三态模型，对应 Claude Code)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionState {
    /// 空闲，等待输入
    Idle,
    
    /// 执行中 (API 调用 / 工具执行)
    Running,
    
    /// 等待用户审批
    RequiresAction,
}

impl Default for SessionState {
    fn default() -> Self {
        Self::Idle
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::RequiresAction => write!(f, "requires_action"),
        }
    }
}

impl SessionState {
    pub fn can_transition_to(&self, target: &SessionState) -> bool {
        matches!(
            (self, target),
            (Self::Idle, Self::Running)
            | (Self::Running, Self::RequiresAction)
            | (Self::Running, Self::Idle)
            | (Self::RequiresAction, Self::Running)
            | (Self::RequiresAction, Self::Idle)
            | (Self::Running, Self::Running)  // 允许自循环
        )
    }

    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }
}

/// 事件类型 (对应 JSONL 每行的 type 字段)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    /// 用户消息 / 助手消息
    Message { role: String, content: MessageContent },
    
    /// 工具调用
    ToolCall { 
        id: String, 
        name: String, 
        input: serde_json::Value 
    },
    
    /// 工具结果
    ToolResult { 
        id: String, 
        output: String, 
        is_error: bool,
        duration_ms: u64,
    },
    
    /// 系统事件
    System { message: String },
    
    /// 错误事件
    Error { error: String, recoverable: bool },
    
    /// 压缩事件
    Compact { strategy: String, messages_removed: usize },
    
    /// 会话元数据变更
    MetadataChange { key: String, value: serde_json::Value },
    
    /// 成本更新
    CostUpdate { total_cost: f64, incremental_cost: f64 },
    
    /// Token 使用
    TokenUsage { 
        input_tokens: u32, 
        output_tokens: u32,
        cache_read: u32,
        cache_write: u32,
    },

    /// 状态转换
    StateTransition { from: String, to: String, reason: Option<String> },
}

/// 消息内容 (支持多模态)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    ContentList(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: Option<String>,
    pub content: Option<String>,
}

/// 会话事件 — JSONL 存储的每一条记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_type: EventType,
    pub session_id: String,
    /// 可选的事件序列号 (用于增量同步)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
}

/// 会话快照 — 完整状态的序列化点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSnapshot {
    pub role: String,
    pub content: String,
    pub token_count: Option<usize>,
}

/// 对话摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    /// 摘要文本
    pub summary_text: String,
    
    /// 关键决策列表
    pub key_decisions: Vec<String>,
    
    /// 待办事项
    pub pending_items: Vec<String>,
    
    /// 原始消息数
    pub original_message_count: usize,
    
    /// 摘要后消息数
    pub summarized_count: usize,
    
    /// 节省的 token 数估算
    pub estimated_tokens_saved: usize,
    
    /// 生成时间
    pub generated_at: chrono::DateTime<chrono::Utc>,
}
