//! Client-server protocol for jcode
//!
//! Uses newline-delimited JSON over Unix socket.
//! Server streams events back to clients during message processing.
//!
//! Socket types:
//! - Main socket: TUI/client communication with agent
//! - Agent socket: Inter-agent communication (AI-to-AI)

use serde::{Deserialize, Serialize};

use crate::message::ToolCall;

/// A message in conversation history (for sync)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_data: Option<ToolCall>,
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

    /// Soft interrupt: inject message at next safe point without cancelling
    #[serde(rename = "soft_interrupt")]
    SoftInterrupt {
        id: u64,
        content: String,
        /// If true, can skip remaining tools at injection point C
        #[serde(default)]
        urgent: bool,
    },

    /// Clear conversation history
    #[serde(rename = "clear")]
    Clear { id: u64 },

    /// Health check
    #[serde(rename = "ping")]
    Ping { id: u64 },

    /// Get current state (debug)
    #[serde(rename = "state")]
    GetState { id: u64 },

    /// Execute a debug command (debug socket only)
    #[serde(rename = "debug_command")]
    DebugCommand {
        id: u64,
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },

    /// Execute a client debug command (forwarded to TUI)
    #[serde(rename = "client_debug_command")]
    ClientDebugCommand { id: u64, command: String },

    /// Response from TUI for client debug command
    #[serde(rename = "client_debug_response")]
    ClientDebugResponse { id: u64, output: String },

    /// Subscribe to events (for TUI clients)
    #[serde(rename = "subscribe")]
    Subscribe {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selfdev: Option<bool>,
    },

    /// Get full conversation history (for TUI sync on connect)
    #[serde(rename = "get_history")]
    GetHistory { id: u64 },

    /// Trigger server hot reload (build new version, restart)
    #[serde(rename = "reload")]
    Reload { id: u64 },

    /// Resume a specific session by ID
    #[serde(rename = "resume_session")]
    ResumeSession { id: u64, session_id: String },

    /// Cycle the active model (direction: 1 for next, -1 for previous)
    #[serde(rename = "cycle_model")]
    CycleModel {
        id: u64,
        #[serde(default = "default_model_direction")]
        direction: i8,
    },

    /// Set the active model by name
    #[serde(rename = "set_model")]
    SetModel { id: u64, model: String },

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

    // === Agent communication ===
    /// Share context with other agents
    #[serde(rename = "comm_share")]
    CommShare {
        id: u64,
        session_id: String,
        key: String,
        value: String,
    },

    /// Read shared context from other agents
    #[serde(rename = "comm_read")]
    CommRead {
        id: u64,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },

    /// Send a message to other agents
    #[serde(rename = "comm_message")]
    CommMessage {
        id: u64,
        from_session: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_session: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<String>,
    },

    /// List agents and their activity
    #[serde(rename = "comm_list")]
    CommList { id: u64, session_id: String },

    /// Approve a plan proposal (coordinator only)
    #[serde(rename = "comm_approve_plan")]
    CommApprovePlan {
        id: u64,
        session_id: String,
        proposer_session: String,
    },

    /// Reject a plan proposal (coordinator only)
    #[serde(rename = "comm_reject_plan")]
    CommRejectPlan {
        id: u64,
        session_id: String,
        proposer_session: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
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
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_read_input: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_creation_input: Option<u64>,
    },

    /// Upstream provider info (e.g., which provider OpenRouter routed to)
    #[serde(rename = "upstream_provider")]
    UpstreamProvider { provider: String },

    /// Swarm status update (subagent/session lifecycle info)
    #[serde(rename = "swarm_status")]
    SwarmStatus { members: Vec<SwarmMemberStatus> },

    /// Soft interrupt message was injected at a safe point
    #[serde(rename = "soft_interrupt_injected")]
    SoftInterruptInjected {
        /// The injected message content
        content: String,
        /// Which injection point: "A" (after stream), "B" (no tools),
        /// "C" (between tools), "D" (after all tools)
        point: String,
        /// Number of tools skipped (only for urgent interrupt at point C)
        #[serde(skip_serializing_if = "Option::is_none")]
        tools_skipped: Option<usize>,
    },

    /// Relevant memory was injected into the conversation
    #[serde(rename = "memory_injected")]
    MemoryInjected {
        /// Number of memories injected
        count: usize,
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

    /// Response for debug command
    #[serde(rename = "debug_response")]
    DebugResponse { id: u64, ok: bool, output: String },

    /// Client debug command forwarded from debug socket to TUI
    #[serde(rename = "client_debug_request")]
    ClientDebugRequest { id: u64, command: String },

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
        /// Available models for this provider
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        available_models: Vec<String>,
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
        /// Whether this session is in canary/self-dev mode
        #[serde(skip_serializing_if = "Option::is_none")]
        is_canary: Option<bool>,
        /// Server binary version string (e.g. "v0.1.123 (abc1234)")
        #[serde(skip_serializing_if = "Option::is_none")]
        server_version: Option<String>,
        /// Server name for multi-server support (e.g. "blazing")
        #[serde(skip_serializing_if = "Option::is_none")]
        server_name: Option<String>,
        /// Server icon for display (e.g. "🔥")
        #[serde(skip_serializing_if = "Option::is_none")]
        server_icon: Option<String>,
        /// Whether a newer server binary is available on disk
        #[serde(skip_serializing_if = "Option::is_none")]
        server_has_update: Option<bool>,
    },

    /// Server is reloading (clients should reconnect)
    #[serde(rename = "reloading")]
    Reloading {
        /// New socket path to connect to (if different)
        #[serde(skip_serializing_if = "Option::is_none")]
        new_socket: Option<String>,
    },

    /// Progress update during server reload
    #[serde(rename = "reload_progress")]
    ReloadProgress {
        /// Step name (e.g., "git_pull", "cargo_build", "exec")
        step: String,
        /// Human-readable message
        message: String,
        /// Whether this step succeeded (None = in progress)
        #[serde(skip_serializing_if = "Option::is_none")]
        success: Option<bool>,
        /// Output from the step (stdout/stderr)
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },

    /// Model changed (response to cycle_model)
    #[serde(rename = "model_changed")]
    ModelChanged {
        id: u64,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Notification from another agent (file conflict, message, shared context)
    #[serde(rename = "notification")]
    Notification {
        /// Session ID of the agent that triggered the notification
        from_session: String,
        /// Friendly name of the agent (e.g., "fox")
        #[serde(skip_serializing_if = "Option::is_none")]
        from_name: Option<String>,
        /// Type of notification
        notification_type: NotificationType,
        /// Human-readable message describing what happened
        message: String,
    },

    /// Response to comm_read request
    #[serde(rename = "comm_context")]
    CommContext {
        id: u64,
        /// Shared context entries
        entries: Vec<ContextEntry>,
    },

    /// Response to comm_list request
    #[serde(rename = "comm_members")]
    CommMembers { id: u64, members: Vec<AgentInfo> },
}

/// A shared context entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    pub key: String,
    pub value: String,
    pub from_session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
}

/// Info about an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    /// Files this agent has touched
    pub files_touched: Vec<String>,
}

/// Swarm member status for lifecycle updates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmMemberStatus {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    /// Lifecycle status (ready, running, completed, failed, stopped, etc.)
    pub status: String,
    /// Optional detail (task, error, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Type of notification from another agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum NotificationType {
    /// Another agent touched a file you've worked with
    #[serde(rename = "file_conflict")]
    FileConflict {
        path: String,
        /// What the other agent did: "read", "wrote", "edited"
        operation: String,
    },
    /// Another agent shared context
    #[serde(rename = "shared_context")]
    SharedContext { key: String, value: String },
    /// Direct message from another agent
    #[serde(rename = "message")]
    Message {
        /// Message scope: "dm", "channel", or "broadcast"
        #[serde(skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
        /// Channel name for channel messages (e.g. "parser")
        #[serde(skip_serializing_if = "Option::is_none")]
        channel: Option<String>,
    },
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Request::Message { id, .. } => *id,
            Request::Cancel { id } => *id,
            Request::SoftInterrupt { id, .. } => *id,
            Request::Clear { id } => *id,
            Request::Ping { id } => *id,
            Request::GetState { id } => *id,
            Request::DebugCommand { id, .. } => *id,
            Request::ClientDebugCommand { id, .. } => *id,
            Request::ClientDebugResponse { id, .. } => *id,
            Request::Subscribe { id, .. } => *id,
            Request::GetHistory { id } => *id,
            Request::Reload { id } => *id,
            Request::ResumeSession { id, .. } => *id,
            Request::CycleModel { id, .. } => *id,
            Request::SetModel { id, .. } => *id,
            Request::AgentRegister { id, .. } => *id,
            Request::AgentTask { id, .. } => *id,
            Request::AgentCapabilities { id } => *id,
            Request::AgentContext { id } => *id,
            Request::CommShare { id, .. } => *id,
            Request::CommRead { id, .. } => *id,
            Request::CommMessage { id, .. } => *id,
            Request::CommList { id, .. } => *id,
            Request::CommApprovePlan { id, .. } => *id,
            Request::CommRejectPlan { id, .. } => *id,
        }
    }
}

fn default_model_direction() -> i8 {
    1
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
