//! Server-to-client events: replies and streaming.

use serde::{Deserialize, Serialize};

/// Curated event surface. Internally-tagged on `"ev"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "ev", rename_all = "snake_case")]
pub enum ApiEvent {
    /// Handshake accepted. Sent in reply to `Hello`.
    HelloOk {
        version: u32,
        /// Server name and version, e.g. "jcode/0.55.1".
        server: String,
        /// Optional capability strings for additive feature discovery.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
    },

    /// Generic success acknowledgment for requests without a richer reply.
    Ok,

    /// Request failed.
    Error { code: ErrorCode, message: String },

    /// Reply to `ListSessions`.
    Sessions { sessions: Vec<SessionInfo> },

    /// Reply to `CreateSession` / `AttachSession`.
    Attached { session: SessionInfo },

    /// Reply to `GetHistory`.
    History {
        session_id: String,
        messages: Vec<HistoryMessage>,
    },

    /// Reply to `Ping`.
    Pong,

    // --- Streaming events (carry session_id, not tied to a request id) ---
    /// Assistant text delta.
    TextDelta { session_id: String, text: String },

    /// Model reasoning delta (render dim/italic; safe to ignore).
    ReasoningDelta { session_id: String, text: String },

    /// Reasoning finished for the current step.
    ReasoningDone {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_secs: Option<f64>,
    },

    /// Tool call streaming lifecycle.
    ToolStart {
        session_id: String,
        call_id: String,
        name: String,
    },
    ToolInputDelta {
        session_id: String,
        call_id: String,
        delta: String,
    },
    ToolExec {
        session_id: String,
        call_id: String,
        name: String,
    },
    ToolDone {
        session_id: String,
        call_id: String,
        name: String,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Token usage update for the attached session.
    TokenUsage {
        session_id: String,
        input: u64,
        output: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_read_input: Option<u64>,
    },

    /// The turn finished; the agent is idle.
    TurnDone { session_id: String },

    /// The harness needs a permission decision from the user.
    PermissionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        description: String,
    },

    /// Session-level status change (idle, generating, tool_running, ...).
    SessionStatus { session_id: String, status: String },

    /// Forward-compatibility catch-all: clients must skip this silently.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedVersion,
    UnknownRequest,
    UnknownSession,
    InvalidRequest,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryMessage {
    /// "user" | "assistant" | "tool".
    pub role: String,
    pub content: String,
}
