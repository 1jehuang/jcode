use crate::message::ToolCall;
use serde::{Deserialize, Serialize};

/// Debug events broadcast by local harnesses via debug socket.
/// These expose the full internal state for debugging/comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebugEvent {
    /// Full state snapshot (sent on connect)
    StateSnapshot {
        display_messages: Vec<DebugMessage>,
        streaming_text: String,
        streaming_tool_calls: Vec<ToolCall>,
        input: String,
        cursor_pos: usize,
        is_processing: bool,
        scroll_offset: usize,
        status: String,
        provider_name: String,
        provider_model: String,
        mcp_servers: Vec<String>,
        skills: Vec<String>,
        session_id: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
        queued_messages: Vec<String>,
    },

    /// Text delta appended to streaming_text
    TextDelta { text: String },

    /// Tool started
    ToolStart { id: String, name: String },

    /// Tool input delta
    ToolInput { delta: String },

    /// Tool about to execute
    ToolExec { id: String, name: String },

    /// Tool completed
    ToolDone {
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },

    /// Message added to display_messages
    MessageAdded { message: DebugMessage },

    /// Streaming text cleared (turn complete)
    StreamingCleared,

    /// Processing state changed
    ProcessingChanged { is_processing: bool },

    /// Status changed
    StatusChanged { status: String },

    /// Token usage update
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    },

    /// Input changed (user typing)
    InputChanged { input: String, cursor_pos: usize },

    /// Scroll offset changed
    ScrollChanged { offset: usize },

    /// Message queued
    MessageQueued { content: String },

    /// Queued message sent
    QueuedMessageSent { index: usize },

    /// Session ID set
    SessionId { id: String },

    /// Thinking started
    ThinkingStart,

    /// Thinking ended
    ThinkingEnd,

    /// Compaction occurred
    Compaction { trigger: String, pre_tokens: u64 },

    /// Error occurred
    Error { message: String },
}

/// Simplified message for debug serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub duration_secs: Option<f32>,
    pub title: Option<String>,
    pub tool_data: Option<ToolCall>,
}
