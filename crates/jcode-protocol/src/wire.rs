use super::*;

/// Serde default for boolean fields that should default to `true` when absent,
/// so older clients that omit the field keep their previous (unconditional)
/// behavior.
fn default_true() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// SDK-owned session tool declaration. Parameters is a JSON schema object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Replaces the session SDK overlay. None inherits normal selection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionToolConfig {
    #[serde(default)]
    pub enabled: Option<Vec<String>>,
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub custom: Vec<SessionToolDefinition>,
}

/// Client request to server
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    #[serde(rename = "configure_tools")]
    ConfigureTools { id: u64, tools: SessionToolConfig },
    #[serde(rename = "list_tools")]
    ListTools { id: u64 },
    #[serde(rename = "tool_result")]
    ToolResult {
        id: u64,
        call_id: String,
        output: String,
        #[serde(default)]
        error: Option<String>,
    },
    /// Send a message to the agent
    #[serde(rename = "message")]
    Message {
        id: u64,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<(String, String)>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_reminder: Option<String>,
        /// Skill selected by the client for this and subsequent turns. The
        /// daemon resolves the name against its own skill registry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_skill: Option<String>,
        /// Append the user message as context only. The daemon persists it and
        /// acknowledges it without starting a model turn.
        #[serde(default, skip_serializing_if = "is_false")]
        no_reply: bool,
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<(String, String)>,
        /// If true, can skip remaining tools at injection point C
        #[serde(default)]
        urgent: bool,
        /// Opaque caller id, echoed in `soft_interrupt_injected` and accepted
        /// by a filtered `cancel_soft_interrupts`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },

    /// Cancel pending soft interrupts (remove from server queue before injection).
    ///
    /// Without `client_ids` every queued interrupt is dropped and the request
    /// is acknowledged. With `client_ids` only the matching ones are dropped,
    /// and the daemon answers with `soft_interrupts_cancelled`.
    #[serde(rename = "cancel_soft_interrupts")]
    CancelSoftInterrupts {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_ids: Option<Vec<String>>,
    },

    /// Clear conversation history
    #[serde(rename = "clear")]
    Clear { id: u64 },

    /// Rewind conversation history to the given 1-based message index.
    #[serde(rename = "rewind")]
    Rewind { id: u64, message_index: usize },

    /// Undo the most recent rewind, if one is available.
    #[serde(rename = "rewind_undo")]
    RewindUndo { id: u64 },

    /// Health check
    #[serde(rename = "ping")]
    Ping { id: u64 },

    /// Get current state (debug)
    #[serde(rename = "state")]
    GetState { id: u64 },

    /// Fetch the attached session's todo card without locking its agent.
    #[serde(rename = "get_todos")]
    GetTodos { id: u64 },

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
        /// Full system prompt override for a new session only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        /// Opt in to PDF panel payloads. Older clients only accept Markdown.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        supports_pdf_panels: bool,
        /// Desktop can render user_response tool calls as inline answers.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        supports_user_response: bool,
        /// The client has no account-router countdown of its own (Desktop,
        /// SDK). On a usage/rate-limit failure the server switches to the next
        /// pool-approved account and retries the turn itself.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        supports_server_failover: bool,
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
        /// Prefix of the transcript the client already rendered locally. When
        /// it matches the server's view, History carries only the messages
        /// after it (`messages_base_count`), instead of the full transcript.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history_base: Option<crate::HistoryBase>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_session_takeover: bool,
        /// Legacy ownership hint, retained for wire compatibility. Disconnects
        /// only mark a session crashed when they interrupt unfinished processing,
        /// regardless of this flag. Idle/completed sessions close normally.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        crash_on_disconnect: bool,
        /// Keep an already-running turn alive when this transport disconnects.
        /// Opt-in for remote clients only. Idle sessions still close normally,
        /// and reattachment uses persisted history plus future live events, not
        /// replay of missed deltas. This does not survive daemon termination.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        continue_on_disconnect: bool,
        /// Terminal-identifying env vars (tmux/zellij/kitty/DISPLAY/...) captured
        /// from the connecting client so the server can route spawn/focus hooks
        /// to the client's terminal instead of its own stale startup env (#405).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        terminal_env: Vec<(String, String)>,
    },

    /// Declare that this client is intentionally detaching before its transport
    /// closes. Retained for compatibility with older servers that use
    /// `crash_on_disconnect` for idle sessions too.
    #[serde(rename = "prepare_disconnect")]
    PrepareDisconnect { id: u64 },

    /// Get full conversation history (for TUI sync on connect)
    #[serde(rename = "get_history")]
    GetHistory { id: u64 },

    /// Get only provider/model metadata and available models.
    #[serde(rename = "get_model_catalog")]
    GetModelCatalog {
        id: u64,
        /// Older clients cannot decode new event variants. Only clients that
        /// explicitly opt in receive incremental model_usage_updated events.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        subscribe_usage_updates: bool,
    },

    /// Get a bounded view of compacted historical messages for lazy transcript expansion.
    #[serde(rename = "get_compacted_history")]
    GetCompactedHistory {
        id: u64,
        /// Number of leading compacted messages the client wants rendered before the live tail.
        visible_messages: usize,
    },

    /// Trigger server hot reload (build new version, restart)
    #[serde(rename = "reload")]
    Reload {
        id: u64,
        /// When `true` (the default for backward compatibility), the server
        /// reloads unconditionally. When `false`, the server only reloads if it
        /// detects a strictly-newer reload candidate binary, so callers like
        /// `jcode server reload` can request a graceful upgrade without risking
        /// a downgrade (e.g. a newer self-dev daemon next to an older release).
        #[serde(default = "default_true")]
        force: bool,
    },

    /// Optional local packaged-runtime upgrade. Distinct from `reload` so old
    /// daemons reject it without invoking the legacy interrupting reload path.
    /// The server validates this explicit executable and only replaces itself
    /// if it is strictly newer and an atomic idle reservation succeeds.
    #[serde(rename = "upgrade_runtime")]
    UpgradeRuntime { id: u64, candidate: String },

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

    /// Resume/continue every live session that was interrupted and would
    /// auto-continue on a reload (e.g. crashed/errored mid-turn). This is the
    /// on-demand equivalent of the automatic post-reload recovery sweep.
    #[serde(rename = "resume_all_sessions")]
    ResumeAllSessions { id: u64 },

    /// Deliver a scheduled task to a currently live session.
    #[serde(rename = "notify_session")]
    NotifySession {
        id: u64,
        session_id: String,
        message: String,
    },

    /// The user pressed something in an agent-mounted applet instance. The
    /// server stores `state`, then wakes the agent (or resolves a waiting
    /// `applet` tool call).
    #[serde(rename = "applet_action")]
    AppletAction {
        id: u64,
        session_id: String,
        instance: String,
        action: jcode_applet_types::Action,
        #[serde(default)]
        state: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_key: Option<String>,
    },

    /// The user closed an agent-mounted applet instance. No agent wake.
    #[serde(rename = "close_applet")]
    CloseApplet {
        id: u64,
        session_id: String,
        instance: String,
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
        /// Ask the server to persist the confirmed cycle as its own config
        /// default. Default false keeps older clients unchanged.
        #[serde(default, skip_serializing_if = "is_false")]
        persist_default: bool,
    },

    #[serde(rename = "refresh_models")]
    RefreshModels { id: u64 },

    /// Set the active model by name.
    ///
    /// A legacy/desktop compatibility shape (`{"type":"set_route","model":...}`)
    /// is also accepted, but it is normalized into this variant inside
    /// [`crate::decode_request`] rather than via a serde `alias`. A serde alias
    /// would make this variant *also* answer to the `set_route` tag, and serde's
    /// internally-tagged enums pick the first matching variant by tag (not by
    /// fields), so it would shadow the structured [`Request::SetRoute`] variant
    /// below and make every structured route switch fail with
    /// `missing field \`model\``.
    #[serde(rename = "set_model")]
    SetModel {
        id: u64,
        model: String,
        /// Ask the server to persist the confirmed switch as its own config
        /// default (the server owns config.toml that new sessions read).
        /// Default false keeps older clients and failover/auth switches from
        /// overriding the user's configured default.
        #[serde(default, skip_serializing_if = "is_false")]
        persist_default: bool,
    },

    /// Set a session context budget override. Null or zero clears the override.
    #[serde(rename = "set_context_cap")]
    SetContextCap {
        id: u64,
        session_id: String,
        max_context_tokens: Option<usize>,
    },

    /// Set the active model by structured route identity.
    #[serde(rename = "set_route")]
    SetRoute {
        id: u64,
        selection: jcode_provider_core::RouteSelection,
        /// Automatic retries must revalidate the server's current account pool.
        #[serde(default, skip_serializing_if = "is_false")]
        automatic_failover: bool,
        /// Ask the server to persist the confirmed switch as its own config
        /// default. Only user-initiated picker selections set this; failover
        /// and auth switches must not override the configured default.
        #[serde(default, skip_serializing_if = "is_false")]
        persist_default: bool,
    },

    /// Set or clear the session-scoped subagent model preference.
    #[serde(rename = "set_subagent_model")]
    SetSubagentModel {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },

    /// Set routing for a worker role. None clears to the global default.
    /// With `global`, the server updates its own configuration instead of the
    /// session, so remote clients never write a default only they can see.
    #[serde(rename = "set_agent_model")]
    SetAgentModel {
        id: u64,
        target: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        global: bool,
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

    /// Set reasoning effort for providers that expose it (OpenAI: none|minimal|low|medium|high|xhigh|max; Anthropic: none|low|medium|high|xhigh|max; DeepSeek: none|low|medium|high|max)
    #[serde(rename = "set_reasoning_effort")]
    SetReasoningEffort {
        id: u64,
        effort: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_session_id: Option<String>,
    },

    /// Set service tier for OpenAI models (priority|fast|flex|off)
    #[serde(rename = "set_service_tier")]
    SetServiceTier { id: u64, service_tier: String },

    /// Set connection transport for OpenAI models (auto|https|websocket)
    #[serde(rename = "set_transport")]
    SetTransport { id: u64, transport: String },

    /// Non-presentation command executed against the attached live runtime.
    #[serde(rename = "runtime_command")]
    RuntimeCommand {
        id: u64,
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },

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
        mode: jcode_config_types::CompactionMode,
    },

    /// Set or clear the active session's custom display title.
    #[serde(rename = "rename_session")]
    RenameSession {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },

    /// Replace the active session's process-local environment overlay.
    #[serde(rename = "set_session_env")]
    SetSessionEnv {
        id: u64,
        env: std::collections::HashMap<String, String>,
    },

    /// Bookmark (`saved: true`, optional label) or unbookmark the active
    /// session. Routed through the daemon so its in-memory session, which
    /// owns later writes, does not overwrite the flag.
    #[serde(rename = "set_session_saved")]
    SetSessionSaved {
        id: u64,
        saved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },

    /// Write the active session to disk now, even before its first message.
    /// Out-of-process tools such as `jcode cloud move` read the session file,
    /// so a fresh session must exist on disk with its model and account.
    #[serde(rename = "persist_session")]
    PersistSession { id: u64 },

    /// Split the current session — clone conversation into a new session
    #[serde(rename = "split")]
    Split { id: u64 },

    /// Transfer the current session into a compacted handoff session
    #[serde(rename = "transfer")]
    Transfer { id: u64 },

    /// Trigger manual context compaction
    #[serde(rename = "compact")]
    Compact { id: u64 },

    /// Trigger immediate memory extraction for the current session
    #[serde(rename = "trigger_memory_extraction")]
    TriggerMemoryExtraction { id: u64 },

    /// Notify server that auth credentials changed (e.g., after login)
    #[serde(rename = "notify_auth_changed")]
    NotifyAuthChanged {
        id: u64,
        /// Optional runtime provider identity whose credentials changed. Older
        /// clients omit this and get the legacy generic refresh behavior.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// Typed auth lifecycle event for new clients. The legacy `provider`
        /// string is retained for old clients, while this payload gives the
        /// server enough context to activate the intended runtime/catalog
        /// profile deterministically.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<AuthChanged>,
        /// First-run onboarding may ask the server to choose the strongest
        /// available route across all authenticated providers. Normal re-auth,
        /// account switching, and older clients leave this false.
        #[serde(default, skip_serializing_if = "is_false")]
        prefer_strongest: bool,
    },

    /// Fetch daemon-owned OAuth account usage without acquiring the agent lock.
    #[serde(rename = "account_usage")]
    AccountUsage { id: u64 },

    /// Switch active Anthropic account label on the server session.
    /// This keeps account overrides and provider credential caches in sync.
    #[serde(rename = "switch_anthropic_account")]
    SwitchAnthropicAccount { id: u64, label: String },

    /// Switch active OpenAI account label on the server session.
    /// This keeps account overrides and provider credential caches in sync.
    #[serde(rename = "switch_openai_account")]
    SwitchOpenAiAccount { id: u64, label: String },

    /// Invalidate daemon-local usage and quota cooldown state after a banked reset.
    /// This never redeems a reset or switches accounts. `None` pins the default
    /// account scope, not whichever account is active when the request arrives.
    #[serde(rename = "invalidate_openai_usage")]
    InvalidateOpenAiUsage {
        id: u64,
        account_label: Option<String>,
    },

    /// Invalidate daemon-local usage and quota cooldown state after a Claude
    /// session-limit reset. Like the OpenAI variant it never claims a reset.
    /// `None` pins the default account scope.
    #[serde(rename = "invalidate_anthropic_usage")]
    InvalidateAnthropicUsage {
        id: u64,
        account_label: Option<String>,
    },

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
        /// Sender-provided one-line summary. Receiving UIs render long
        /// message bodies collapsed to this with an expand control.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tldr: Option<String>,
        /// Cross-swarm target: a swarm label or swarm id other than the
        /// sender's own. When set, the message is a cross-swarm DM delivered
        /// to `to_session` inside that swarm, or to its coordinator when
        /// `to_session` is omitted. Channels and broadcasts never cross swarms.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_swarm: Option<String>,
        /// Refuse an untargeted send (no `to_session`, `channel`, or
        /// `to_swarm`) unless the sender is the swarm coordinator, so a worker
        /// that forgets `to_session` gets an error instead of broadcasting.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        require_target: bool,
    },

    /// List every live swarm (id, label, coordinator, member count) so agents
    /// can discover cross-swarm DM targets.
    #[serde(rename = "comm_list_swarms")]
    CommListSwarms { id: u64, session_id: String },

    /// Set (or clear, with an empty label) the human-readable label of the
    /// caller's swarm. Labels are unique across swarms.
    #[serde(rename = "comm_set_swarm_label")]
    CommSetSwarmLabel {
        id: u64,
        session_id: String,
        label: String,
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
        /// Exact coordinator todo lane to seed before the worker starts.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lane_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial_message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_nonce: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_mode: Option<String>,
        /// Optional per-spawn model override. Takes precedence over
        /// `agents.swarm_model` config. Supports explicit auth-route prefixes
        /// (e.g. `openai-api:gpt-5.5`) and the `inherit`/`coordinator`
        /// sentinels to force coordinator inheritance past a config pin.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Optional reasoning effort for the spawned agent (e.g. `none`,
        /// `low`, `medium`, `high`, `xhigh`, `max`). Unset = provider default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        /// Optional short human-readable label for the spawned agent shown in
        /// swarm UI (gallery chips, member lists). Overrides the task label
        /// otherwise derived from the first line of `initial_message`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },

    /// List models/routes available for spawning swarm agents
    #[serde(rename = "comm_list_models")]
    CommListModels { id: u64, session_id: String },

    /// Stop/destroy an agent session (coordinator only)
    #[serde(rename = "comm_stop")]
    CommStop {
        id: u64,
        session_id: String,
        target_session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        force: Option<bool>,
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

    /// Get a lightweight status snapshot for an agent, even while it is busy
    #[serde(rename = "comm_status")]
    CommStatus {
        id: u64,
        session_id: String,
        target_session: String,
    },

    /// Submit a structured swarm completion/progress report for this session
    #[serde(rename = "comm_report")]
    CommReport {
        id: u64,
        session_id: String,
        /// Completion status to record for this member. Defaults to ready.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        /// Main report body.
        message: String,
        /// Optional validation/testing summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        validation: Option<String>,
        /// Optional blockers/follow-up summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follow_up: Option<String>,
        /// Reporter-provided one-line summary. Receiving UIs render long
        /// report bodies collapsed to this with an expand control.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tldr: Option<String>,
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

    /// Get a lightweight summary of the current swarm plan graph
    #[serde(rename = "comm_plan_status")]
    CommPlanStatus { id: u64, session_id: String },

    /// Assign a task from the plan to a specific agent (coordinator only)
    #[serde(rename = "comm_assign_task")]
    CommAssignTask {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_session: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },

    /// Assign the next runnable unassigned task from the plan (coordinator only)
    #[serde(rename = "comm_assign_next")]
    CommAssignNext {
        id: u64,
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_session: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefer_spawn: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawn_if_needed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// Optional model override for workers spawned by this assignment
        /// (same semantics as CommSpawn::model).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Optional reasoning effort for workers spawned by this assignment
        /// (same semantics as CommSpawn::effort).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },

    /// Control an existing assigned task lifecycle (coordinator only)
    #[serde(rename = "comm_task_control")]
    CommTaskControl {
        id: u64,
        session_id: String,
        action: String,
        task_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_session: Option<String>,
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
        /// Whether to wait for all matching members or wake when any member matches.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        /// Timeout in seconds (default 3600 = 1 hour)
        #[serde(default)]
        timeout_secs: Option<u64>,
        /// Run the wait as a detached background watcher instead of blocking the
        /// requesting turn. Defaults to true so the agent stays responsive.
        #[serde(default = "default_true")]
        background: bool,
        /// When backgrounded, surface a notification card on completion.
        #[serde(default = "default_true")]
        notify: bool,
        /// When backgrounded, wake an idle requesting agent with the result (or
        /// soft-interrupt it if busy). Defaults to true.
        #[serde(default = "default_true")]
        wake: bool,
    },
}

mod events;
pub use events::{AccountUsageSnapshot, ServerEvent};

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::SetSessionEnv { id, .. } = self {
            return f
                .debug_struct("SetSessionEnv")
                .field("id", id)
                .field("env", &"[REDACTED]")
                .finish();
        }
        match serde_json::to_value(self) {
            Ok(value) => std::fmt::Debug::fmt(&value, f),
            Err(_) => f.write_str("Request([unavailable])"),
        }
    }
}

#[cfg(test)]
mod session_env_tests {
    use super::*;

    #[test]
    fn session_environment_roundtrips_without_debug_secrets() {
        let request = Request::SetSessionEnv {
            id: 42,
            env: [("TOKEN".into(), "private-secret".into())].into(),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["type"], "set_session_env");
        assert_eq!(json["env"]["TOKEN"], "private-secret");
        let decoded: Request = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.id(), 42);
        assert!(!format!("{decoded:?}").contains("private-secret"));
        assert!(!format!("{decoded:?}").contains("TOKEN"));
    }
}
