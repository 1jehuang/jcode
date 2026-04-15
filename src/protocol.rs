//! Client-server protocol for jcode
//!
//! Uses newline-delimited JSON over Unix socket.
//! Server streams events back to clients during message processing.
//!
//! Socket types:
//! - Main socket: TUI/client communication with agent
//! - Agent socket: Inter-agent communication (AI-to-AI)

use serde::{Deserialize, Serialize};

use crate::bus::BatchProgress;
use crate::message::ToolCall;
use crate::plan::PlanItem;
use crate::side_panel::SidePanelSnapshot;

#[path = "protocol_memory.rs"]
mod memory_snapshots;

pub use memory_snapshots::{
    MemoryActivitySnapshot, MemoryPipelineSnapshot, MemoryStateSnapshot, MemoryStepResultSnapshot,
    MemoryStepStatusSnapshot,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMode {
    Insert,
    Append,
    Replace,
    #[default]
    Send,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommDeliveryMode {
    Notify,
    Interrupt,
    Wake,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionActivitySnapshot {
    pub is_processing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_name: Option<String>,
}

/// Client request to server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    /// Send a message to the agent
    #[serde(rename = "message")]
    Message {
        id: u64,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_reminder: Option<String>,
    },

    /// Cancel current generation
    #[serde(rename = "cancel")]
    Cancel { id: u64 },

    /// Move the currently executing tool to background
    #[serde(rename = "background_tool")]
    BackgroundTool { id: u64 },

    /// Soft interrupt: inject message at next safe point without cancelling
    #[serde(rename = "soft_interrupt")]
    SoftInterrupt {
        id: u64,
        content: String,
        /// If true, can skip remaining tools at injection point C
        #[serde(default)]
        urgent: bool,
    },

    /// Cancel all pending soft interrupts (remove from server queue before injection)
    #[serde(rename = "cancel_soft_interrupts")]
    CancelSoftInterrupts { id: u64 },

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_instance_id: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        client_has_local_history: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_session_takeover: bool,
    },

    /// Get full conversation history (for TUI sync on connect)
    #[serde(rename = "get_history")]
    GetHistory { id: u64 },

    /// Trigger server hot reload (build new version, restart)
    #[serde(rename = "reload")]
    Reload { id: u64 },

    /// Resume a specific session by ID
    #[serde(rename = "resume_session")]
    ResumeSession {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_instance_id: Option<String>,
        #[serde(default)]
        client_has_local_history: bool,
        #[serde(default)]
        allow_session_takeover: bool,
    },

    /// Deliver a scheduled task to a currently live session.
    #[serde(rename = "notify_session")]
    NotifySession {
        id: u64,
        session_id: String,
        message: String,
    },

    /// Inject externally transcribed text into a live TUI session.
    #[serde(rename = "transcript")]
    Transcript {
        id: u64,
        text: String,
        #[serde(default)]
        mode: TranscriptMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },

    /// Execute a shell command from `!cmd` in the active remote session.
    #[serde(rename = "input_shell")]
    InputShell { id: u64, command: String },

    /// Cycle the active model (direction: 1 for next, -1 for previous)
    #[serde(rename = "cycle_model")]
    CycleModel {
        id: u64,
        #[serde(default = "default_model_direction")]
        direction: i8,
    },

    #[serde(rename = "refresh_models")]
    RefreshModels { id: u64 },

    /// Set the active model by name
    #[serde(rename = "set_model")]
    SetModel { id: u64, model: String },

    /// Set or clear the session-scoped subagent model preference.
    #[serde(rename = "set_subagent_model")]
    SetSubagentModel {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },

    /// Launch a subagent immediately in the active session.
    #[serde(rename = "run_subagent")]
    RunSubagent {
        id: u64,
        prompt: String,
        subagent_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },

    /// Set reasoning effort for OpenAI models (none|low|medium|high|xhigh)
    #[serde(rename = "set_reasoning_effort")]
    SetReasoningEffort { id: u64, effort: String },

    /// Set service tier for OpenAI models (priority|fast|flex|off)
    #[serde(rename = "set_service_tier")]
    SetServiceTier { id: u64, service_tier: String },

    /// Set connection transport for OpenAI models (auto|https|websocket)
    #[serde(rename = "set_transport")]
    SetTransport { id: u64, transport: String },

    /// Set Copilot premium request conservation mode (0=normal, 1=one-per-session, 2=zero)
    #[serde(rename = "set_premium_mode")]
    SetPremiumMode { id: u64, mode: u8 },

    /// Toggle a runtime feature for this session
    #[serde(rename = "set_feature")]
    SetFeature {
        id: u64,
        feature: FeatureToggle,
        enabled: bool,
    },

    /// Set the compaction mode for this session
    #[serde(rename = "set_compaction_mode")]
    SetCompactionMode {
        id: u64,
        mode: crate::config::CompactionMode,
    },

    /// Split the current session — clone conversation into a new session
    #[serde(rename = "split")]
    Split { id: u64 },

    /// Trigger manual context compaction
    #[serde(rename = "compact")]
    Compact { id: u64 },

    /// Trigger immediate memory extraction for the current session
    #[serde(rename = "trigger_memory_extraction")]
    TriggerMemoryExtraction { id: u64 },

    /// Notify server that auth credentials changed (e.g., after login)
    #[serde(rename = "notify_auth_changed")]
    NotifyAuthChanged { id: u64 },

    /// Switch active Anthropic account label on the server session.
    /// This keeps account overrides and provider credential caches in sync.
    #[serde(rename = "switch_anthropic_account")]
    SwitchAnthropicAccount { id: u64, label: String },

    /// Switch active OpenAI account label on the server session.
    /// This keeps account overrides and provider credential caches in sync.
    #[serde(rename = "switch_openai_account")]
    SwitchOpenAiAccount { id: u64, label: String },

    /// Send stdin input to a running command that requested it
    #[serde(rename = "stdin_response")]
    StdinResponse {
        id: u64,
        /// Matches the request_id from StdinRequest
        request_id: String,
        /// The user's input (line of text)
        input: String,
    },

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
        #[serde(default)]
        append: bool,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<CommDeliveryMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wake: Option<bool>,
    },

    /// List agents and their activity
    #[serde(rename = "comm_list")]
    CommList { id: u64, session_id: String },

    /// List swarm channels and subscriber counts
    #[serde(rename = "comm_list_channels")]
    CommListChannels { id: u64, session_id: String },

    /// List members subscribed to a swarm channel
    #[serde(rename = "comm_channel_members")]
    CommChannelMembers {
        id: u64,
        session_id: String,
        channel: String,
    },

    /// Propose a swarm plan update
    #[serde(rename = "comm_propose_plan")]
    CommProposePlan {
        id: u64,
        session_id: String,
        items: Vec<PlanItem>,
    },

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

    /// Spawn a new agent session (coordinator only)
    #[serde(rename = "comm_spawn")]
    CommSpawn {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial_message: Option<String>,
    },

    /// Stop/destroy an agent session (coordinator only)
    #[serde(rename = "comm_stop")]
    CommStop {
        id: u64,
        session_id: String,
        target_session: String,
    },

    /// Assign a role to an agent (coordinator only)
    #[serde(rename = "comm_assign_role")]
    CommAssignRole {
        id: u64,
        session_id: String,
        target_session: String,
        role: String,
    },

    /// Get a summary of an agent's recent tool calls
    #[serde(rename = "comm_summary")]
    CommSummary {
        id: u64,
        session_id: String,
        target_session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },

    /// Read another agent's full conversation context
    #[serde(rename = "comm_read_context")]
    CommReadContext {
        id: u64,
        session_id: String,
        target_session: String,
    },

    /// Attach/resync this session with the swarm plan
    #[serde(rename = "comm_resync_plan")]
    CommResyncPlan { id: u64, session_id: String },

    /// Assign a task from the plan to a specific agent (coordinator only)
    #[serde(rename = "comm_assign_task")]
    CommAssignTask {
        id: u64,
        session_id: String,
        target_session: String,
        task_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },

    /// Subscribe to a named channel in the swarm
    #[serde(rename = "comm_subscribe_channel")]
    CommSubscribeChannel {
        id: u64,
        session_id: String,
        channel: String,
    },

    /// Unsubscribe from a named channel in the swarm
    #[serde(rename = "comm_unsubscribe_channel")]
    CommUnsubscribeChannel {
        id: u64,
        session_id: String,
        channel: String,
    },

    /// Wait until specified (or all) swarm members reach a target status
    #[serde(rename = "comm_await_members")]
    CommAwaitMembers {
        id: u64,
        session_id: String,
        /// Statuses that count as "done" (e.g. ["completed", "stopped"])
        target_status: Vec<String>,
        /// Specific session IDs to watch. If empty, watches all non-self members.
        #[serde(default)]
        session_ids: Vec<String>,
        /// Timeout in seconds (default 3600 = 1 hour)
        #[serde(default)]
        timeout_secs: Option<u64>,
    },
}

/// Server event sent to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[expect(
    clippy::large_enum_variant,
    reason = "wire protocol prioritizes straightforward serde payloads over boxing every larger event variant"
)]
pub enum ServerEvent {
    /// Acknowledgment of request
    #[serde(rename = "ack")]
    Ack { id: u64 },

    /// Streaming text delta
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    /// Replace the current turn's streamed text content
    /// Used when text-wrapped tool calls are recovered: the garbled text
    /// shown during streaming is replaced with the clean prefix text.
    #[serde(rename = "text_replace")]
    TextReplace { text: String },

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

    /// Batch tool progress update, including currently-running subcalls
    #[serde(rename = "batch_progress")]
    BatchProgress { progress: BatchProgress },

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

    /// Active transport/connection type for the current stream
    #[serde(rename = "connection_type")]
    ConnectionType { connection: String },

    /// Connection phase update (authenticating, connecting, waiting, etc.)
    #[serde(rename = "connection_phase")]
    ConnectionPhase { phase: String },

    /// Provider-supplied human-readable transport detail for the current stream.
    #[serde(rename = "status_detail")]
    StatusDetail { detail: String },

    /// Upstream provider info (e.g., which provider OpenRouter routed to)
    #[serde(rename = "upstream_provider")]
    UpstreamProvider { provider: String },

    /// Swarm status update (subagent/session lifecycle info)
    #[serde(rename = "swarm_status")]
    SwarmStatus { members: Vec<SwarmMemberStatus> },

    /// Full swarm plan snapshot for synchronization and UI rendering.
    #[serde(rename = "swarm_plan")]
    SwarmPlan {
        swarm_id: String,
        version: u64,
        items: Vec<PlanItem>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        participants: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Plan proposal payload delivered to the coordinator.
    #[serde(rename = "swarm_plan_proposal")]
    SwarmPlanProposal {
        swarm_id: String,
        proposer_session: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        proposer_name: Option<String>,
        items: Vec<PlanItem>,
        summary: String,
        proposal_key: String,
    },

    /// Soft interrupt message was injected at a safe point
    #[serde(rename = "soft_interrupt_injected")]
    SoftInterruptInjected {
        /// The injected message content
        content: String,
        /// Optional display role override for the injected content (e.g. "system")
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_role: Option<String>,
        /// Which injection point: "A" (after stream), "B" (no tools),
        /// "C" (between tools), "D" (after all tools)
        point: String,
        /// Number of tools skipped (only for urgent interrupt at point C)
        #[serde(skip_serializing_if = "Option::is_none")]
        tools_skipped: Option<usize>,
    },

    /// Current turn was interrupted by explicit user cancel.
    ///
    /// This is rendered as a system/status notice (not assistant content),
    /// so it does not blend into streaming model output.
    #[serde(rename = "interrupted")]
    Interrupted,

    /// Relevant memory was injected into the conversation
    #[serde(rename = "memory_injected")]
    MemoryInjected {
        /// Number of memories injected
        count: usize,
        /// Exact memory content that was injected
        #[serde(default)]
        prompt: String,
        /// Display-only version of the injected memory content.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_prompt: Option<String>,
        /// Character length of injected content
        #[serde(default)]
        prompt_chars: usize,
        /// Age of the precomputed memory payload at injection time
        #[serde(default)]
        computed_age_ms: u64,
    },

    /// Memory activity state update for remote clients.
    #[serde(rename = "memory_activity")]
    MemoryActivity { activity: MemoryActivitySnapshot },

    /// Context compaction occurred (background summary or emergency drop)
    #[serde(rename = "compaction")]
    Compaction {
        /// What triggered it: "background", "hard_compact", "auto_recovery"
        trigger: String,
        /// Token count before compaction
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_tokens: Option<u64>,
        /// Token estimate after compaction was applied
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_tokens: Option<u64>,
        /// Approximate tokens saved by this compaction event
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens_saved: Option<u64>,
        /// Time spent compacting in the background (0 for synchronous emergency compaction)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// Number of messages dropped (for hard/emergency compaction)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        messages_dropped: Option<usize>,
        /// Number of messages summarized or compacted by this event
        #[serde(default, skip_serializing_if = "Option::is_none")]
        messages_compacted: Option<usize>,
        /// Character count of the persisted summary after compaction
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary_chars: Option<usize>,
        /// Count of recent messages still kept verbatim after compaction
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_messages: Option<usize>,
    },

    /// Message/turn completed
    #[serde(rename = "done")]
    Done { id: u64 },

    /// Error occurred
    #[serde(rename = "error")]
    Error {
        id: u64,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_secs: Option<u64>,
    },

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

    /// MCP status update (sent after background MCP connections complete)
    #[serde(rename = "mcp_status")]
    McpStatus {
        /// Server names with tool counts in "name:count" format
        servers: Vec<String>,
    },

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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<crate::session::RenderedImage>,
        /// Provider name (e.g. "anthropic", "openai")
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_name: Option<String>,
        /// Model name (e.g. "claude-sonnet-4-20250514")
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_model: Option<String>,
        /// Available models for this provider
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        available_models: Vec<String>,
        /// Route metadata for available models
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        available_model_routes: Vec<crate::provider::ModelRoute>,
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
        /// Whether the session was interrupted mid-generation (crashed/disconnected while processing)
        #[serde(skip_serializing_if = "Option::is_none")]
        was_interrupted: Option<bool>,
        /// Last observed actual connection type for this session (e.g. websocket, https/sse)
        #[serde(skip_serializing_if = "Option::is_none")]
        connection_type: Option<String>,
        /// Last observed provider-supplied status detail for this session.
        #[serde(skip_serializing_if = "Option::is_none")]
        status_detail: Option<String>,
        /// Upstream provider (e.g., which provider OpenRouter routed to, or calculated preference)
        #[serde(skip_serializing_if = "Option::is_none")]
        upstream_provider: Option<String>,
        /// Reasoning effort for OpenAI models
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// Service tier override for OpenAI models
        #[serde(skip_serializing_if = "Option::is_none")]
        service_tier: Option<String>,
        /// Session-scoped preferred model for subagents.
        #[serde(skip_serializing_if = "Option::is_none")]
        subagent_model: Option<String>,
        /// Session-scoped automatic review toggle.
        #[serde(skip_serializing_if = "Option::is_none")]
        autoreview_enabled: Option<bool>,
        /// Session-scoped automatic judge toggle.
        #[serde(skip_serializing_if = "Option::is_none")]
        autojudge_enabled: Option<bool>,
        /// Active compaction mode for this session
        #[serde(default)]
        compaction_mode: crate::config::CompactionMode,
        /// Current live processing state for this session, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        activity: Option<SessionActivitySnapshot>,
        /// Session-scoped side panel pages and active focus state
        #[serde(default, skip_serializing_if = "crate::side_panel::snapshot_is_empty")]
        side_panel: SidePanelSnapshot,
    },

    /// Side panel state changed for the active session
    #[serde(rename = "side_panel_state")]
    SidePanelState { snapshot: SidePanelSnapshot },

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
        provider_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Reasoning effort changed (response to set_reasoning_effort)
    #[serde(rename = "reasoning_effort_changed")]
    ReasoningEffortChanged {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Service tier changed (response to set_service_tier)
    #[serde(rename = "service_tier_changed")]
    ServiceTierChanged {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        service_tier: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Transport changed (response to set_transport)
    #[serde(rename = "transport_changed")]
    TransportChanged {
        id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        transport: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Compaction mode changed (response to set_compaction_mode)
    #[serde(rename = "compaction_mode_changed")]
    CompactionModeChanged {
        id: u64,
        mode: crate::config::CompactionMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Available models updated (pushed after auth changes)
    #[serde(rename = "available_models_updated")]
    AvailableModelsUpdated {
        available_models: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        available_model_routes: Vec<crate::provider::ModelRoute>,
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

    /// External transcript text targeted at the active TUI input.
    #[serde(rename = "transcript")]
    Transcript { text: String, mode: TranscriptMode },

    /// Completed `!cmd` shell execution for a connected remote client.
    #[serde(rename = "input_shell_result")]
    InputShellResult {
        result: crate::message::InputShellResult,
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

    /// Response to comm_list_channels request
    #[serde(rename = "comm_channels")]
    CommChannels {
        id: u64,
        channels: Vec<SwarmChannelInfo>,
    },

    /// Response to comm_summary request
    #[serde(rename = "comm_summary_response")]
    CommSummaryResponse {
        id: u64,
        session_id: String,
        tool_calls: Vec<ToolCallSummary>,
    },

    /// Response to comm_read_context request
    #[serde(rename = "comm_context_history")]
    CommContextHistory {
        id: u64,
        session_id: String,
        messages: Vec<HistoryMessage>,
    },

    /// Response to comm_spawn request
    #[serde(rename = "comm_spawn_response")]
    CommSpawnResponse {
        id: u64,
        session_id: String,
        new_session_id: String,
    },

    /// Response to comm_await_members request
    #[serde(rename = "comm_await_members_response")]
    CommAwaitMembersResponse {
        id: u64,
        /// Whether the condition was met (false = timed out)
        completed: bool,
        /// Final status of each watched member
        members: Vec<AwaitedMemberStatus>,
        /// Human-readable summary
        summary: String,
    },

    /// Response to split request — new session created with cloned conversation
    #[serde(rename = "split_response")]
    SplitResponse {
        id: u64,
        new_session_id: String,
        new_session_name: String,
    },

    /// Response to compact request — context compaction status
    #[serde(rename = "compact_result")]
    CompactResult {
        id: u64,
        /// Human-readable status message
        message: String,
        /// Whether compaction was started successfully
        success: bool,
    },

    /// A running command is waiting for stdin input from the user
    #[serde(rename = "stdin_request")]
    StdinRequest {
        /// Unique request ID for matching the response
        request_id: String,
        /// The last line(s) of output (the prompt, e.g. "Password: ")
        prompt: String,
        /// Whether the input should be masked (password field)
        #[serde(default)]
        is_password: bool,
        /// Tool call ID this is associated with
        tool_call_id: String,
    },
}

/// Summary of a tool call for the comm_summary response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub tool_name: String,
    pub brief_output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmChannelInfo {
    pub channel: String,
    pub member_count: usize,
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
    /// Current lifecycle status (ready, running, completed, failed, stopped, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Optional status detail (current task, error, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Role: "agent", "coordinator", "worktree_manager"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Swarm member status for lifecycle updates
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwarmMemberStatus {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    /// Lifecycle status (ready, running, completed, failed, stopped, etc.)
    pub status: String,
    /// Optional detail (task, error, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Role: "agent", "coordinator", "worktree_manager"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Status of a member being awaited by comm_await_members
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwaitedMemberStatus {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    pub status: String,
    /// Whether this member reached the target status
    pub done: bool,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
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

/// Runtime feature names that can be toggled per session
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FeatureToggle {
    Memory,
    Swarm,
    Autoreview,
    Autojudge,
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Request::Message { id, .. } => *id,
            Request::Cancel { id } => *id,
            Request::BackgroundTool { id } => *id,
            Request::SoftInterrupt { id, .. } => *id,
            Request::CancelSoftInterrupts { id } => *id,
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
            Request::NotifySession { id, .. } => *id,
            Request::Transcript { id, .. } => *id,
            Request::InputShell { id, .. } => *id,
            Request::CycleModel { id, .. } => *id,
            Request::RefreshModels { id } => *id,
            Request::SetModel { id, .. } => *id,
            Request::SetSubagentModel { id, .. } => *id,
            Request::RunSubagent { id, .. } => *id,
            Request::SetReasoningEffort { id, .. } => *id,
            Request::SetServiceTier { id, .. } => *id,
            Request::SetTransport { id, .. } => *id,
            Request::SetPremiumMode { id, .. } => *id,
            Request::SetFeature { id, .. } => *id,
            Request::SetCompactionMode { id, .. } => *id,
            Request::Split { id } => *id,
            Request::Compact { id } => *id,
            Request::TriggerMemoryExtraction { id } => *id,
            Request::NotifyAuthChanged { id } => *id,
            Request::SwitchAnthropicAccount { id, .. } => *id,
            Request::SwitchOpenAiAccount { id, .. } => *id,
            Request::StdinResponse { id, .. } => *id,
            Request::AgentRegister { id, .. } => *id,
            Request::AgentTask { id, .. } => *id,
            Request::AgentCapabilities { id } => *id,
            Request::AgentContext { id } => *id,
            Request::CommShare { id, .. } => *id,
            Request::CommRead { id, .. } => *id,
            Request::CommMessage { id, .. } => *id,
            Request::CommList { id, .. } => *id,
            Request::CommListChannels { id, .. } => *id,
            Request::CommChannelMembers { id, .. } => *id,
            Request::CommProposePlan { id, .. } => *id,
            Request::CommApprovePlan { id, .. } => *id,
            Request::CommRejectPlan { id, .. } => *id,
            Request::CommSpawn { id, .. } => *id,
            Request::CommStop { id, .. } => *id,
            Request::CommAssignRole { id, .. } => *id,
            Request::CommSummary { id, .. } => *id,
            Request::CommReadContext { id, .. } => *id,
            Request::CommResyncPlan { id, .. } => *id,
            Request::CommAssignTask { id, .. } => *id,
            Request::CommSubscribeChannel { id, .. } => *id,
            Request::CommUnsubscribeChannel { id, .. } => *id,
            Request::CommAwaitMembers { id, .. } => *id,
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
#[path = "protocol_tests.rs"]
mod tests;
