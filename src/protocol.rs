//! Client-server protocol for jcode
//!
//! Uses newline-delimited JSON over Unix socket.
//! Server streams events back to clients during message processing.
//!
//! Socket types:
//! - Main socket: TUI/client communication with agent
//! - Agent socket: Inter-agent communication (AI-to-AI)

use serde::{Deserialize, Serialize};

/// A message in conversation history (for sync)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<String>>,
}

/// Client request to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    /// Send a message to the agent
    #[serde(rename = "message")]
    Message { id: u64, content: String },

    /// Cancel current generation
    #[serde(rename = "cancel")]
    Cancel { id: u64 },

    /// Clear conversation history
    #[serde(rename = "clear")]
    Clear { id: u64 },

    /// Health check
    #[serde(rename = "ping")]
    Ping { id: u64 },

    /// Get current state (debug)
    #[serde(rename = "state")]
    GetState { id: u64 },

    /// Subscribe to events (for TUI clients)
    #[serde(rename = "subscribe")]
    Subscribe { id: u64 },

    /// Get full conversation history (for TUI sync on connect)
    #[serde(rename = "get_history")]
    GetHistory { id: u64 },

    /// Trigger server hot reload (build new version, restart)
    #[serde(rename = "reload")]
    Reload { id: u64 },

    /// Resume a specific session by ID
    #[serde(rename = "resume_session")]
    ResumeSession { id: u64, session_id: String },

    // === Agent-to-agent communication ===

    /// Register as an external agent
    #[serde(rename = "agent_register")]
    AgentRegister {
        id: u64,
        agent_name: String,
        capabilities: Vec<String>,
    },

    /// Send a task to jcode agent
    #[serde(rename = "agent_task")]
    AgentTask {
        id: u64,
        from_agent: String,
        task: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<serde_json::Value>,
        /// Whether to wait for completion or return immediately
        #[serde(default)]
        async_: bool,
    },

    /// Query jcode agent's capabilities
    #[serde(rename = "agent_capabilities")]
    AgentCapabilities { id: u64 },

    /// Get conversation context (for handoff between agents)
    #[serde(rename = "agent_context")]
    AgentContext { id: u64 },
}

/// Server event sent to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    /// Acknowledgment of request
    #[serde(rename = "ack")]
    Ack { id: u64 },

    /// Streaming text delta
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    /// Tool call started
    #[serde(rename = "tool_start")]
    ToolStart { id: String, name: String },

    /// Tool input delta (streaming JSON)
    #[serde(rename = "tool_input")]
    ToolInput { delta: String },

    /// Tool call ended, now executing
    #[serde(rename = "tool_exec")]
    ToolExec { id: String, name: String },

    /// Tool execution completed
    #[serde(rename = "tool_done")]
    ToolDone {
        id: String,
        name: String,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Token usage update
    #[serde(rename = "tokens")]
    TokenUsage {
        input: u64,
        output: u64,
    },

    /// Message/turn completed
    #[serde(rename = "done")]
    Done { id: u64 },

    /// Error occurred
    #[serde(rename = "error")]
    Error { id: u64, message: String },

    /// Pong response
    #[serde(rename = "pong")]
    Pong { id: u64 },

    /// Current state (debug)
    #[serde(rename = "state")]
    State {
        id: u64,
        session_id: String,
        message_count: usize,
        is_processing: bool,
    },

    /// Session ID assigned
    #[serde(rename = "session")]
    SessionId { session_id: String },

    /// Full conversation history (response to GetHistory)
    #[serde(rename = "history")]
    History {
        id: u64,
        session_id: String,
        messages: Vec<HistoryMessage>,
        /// Provider name (e.g. "anthropic", "openai")
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_name: Option<String>,
        /// Model name (e.g. "claude-sonnet-4-20250514")
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_model: Option<String>,
        /// Connected MCP server names
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mcp_servers: Vec<String>,
        /// Available skill names
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skills: Vec<String>,
        /// Total session token usage (input, output)
        #[serde(skip_serializing_if = "Option::is_none")]
        total_tokens: Option<(u64, u64)>,
        /// All session IDs on the server
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        all_sessions: Vec<String>,
        /// Number of connected clients
        #[serde(skip_serializing_if = "Option::is_none")]
        client_count: Option<usize>,
    },

    /// Server is reloading (clients should reconnect)
    #[serde(rename = "reloading")]
    Reloading {
        /// New socket path to connect to (if different)
        #[serde(skip_serializing_if = "Option::is_none")]
        new_socket: Option<String>,
    },
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Request::Message { id, .. } => *id,
            Request::Cancel { id } => *id,
            Request::Clear { id } => *id,
            Request::Ping { id } => *id,
            Request::GetState { id } => *id,
            Request::Subscribe { id } => *id,
            Request::GetHistory { id } => *id,
            Request::Reload { id } => *id,
            Request::ResumeSession { id, .. } => *id,
            Request::AgentRegister { id, .. } => *id,
            Request::AgentTask { id, .. } => *id,
            Request::AgentCapabilities { id } => *id,
            Request::AgentContext { id } => *id,
        }
    }
}

/// Encode an event as a newline-terminated JSON string
pub fn encode_event(event: &ServerEvent) -> String {
    let mut json = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    json.push('\n');
    json
}

/// Decode a request from a JSON string
pub fn decode_request(line: &str) -> Result<Request, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_roundtrip() {
        let req = Request::Message {
            id: 1,
            content: "hello".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id(), 1);
    }

    #[test]
    fn test_event_roundtrip() {
        let event = ServerEvent::TextDelta {
            text: "hello".to_string(),
        };
        let json = encode_event(&event);
        let decoded: ServerEvent = serde_json::from_str(json.trim()).unwrap();
        match decoded {
            ServerEvent::TextDelta { text } => assert_eq!(text, "hello"),
            _ => panic!("wrong event type"),
        }
    }
}
