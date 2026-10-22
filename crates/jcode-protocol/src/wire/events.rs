//! Events emitted by the daemon wire protocol.

use super::*;

/// Credential-free usage attributed to an OAuth account on the daemon host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountUsageSnapshot {
    pub label: String,
    pub email: Option<String>,
    pub exhausted: bool,
    pub primary_label: Option<String>,
    pub five_hour_ratio: Option<f32>,
    pub secondary_label: Option<String>,
    pub seven_day_ratio: Option<f32>,
    pub resets_at: Option<String>,
    pub error: Option<String>,
}

#[cfg(test)]
mod account_usage_tests {
    use super::*;

    #[test]
    fn account_usage_roundtrips_as_lightweight_control() {
        let request = Request::AccountUsage { id: 71 };
        assert!(request.is_lightweight_control_request());
        assert_eq!(request.id(), 71);
        let decoded: Request =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert!(matches!(decoded, Request::AccountUsage { id: 71 }));
        let snapshot = AccountUsageSnapshot {
            label: "remote-account".into(),
            email: None,
            exhausted: false,
            primary_label: Some("5h".into()),
            five_hour_ratio: Some(0.35),
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: None,
        };
        let event = ServerEvent::AccountUsage {
            id: 71,
            anthropic: vec![snapshot.clone()],
            openai: Vec::new(),
        };
        let decoded: ServerEvent =
            serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
        let ServerEvent::AccountUsage {
            id,
            anthropic,
            openai,
        } = decoded
        else {
            panic!("wrong response")
        };
        assert_eq!(id, 71);
        assert_eq!(anthropic, vec![snapshot]);
        assert!(openai.is_empty());
    }
}

/// Server event sent to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[expect(
    clippy::large_enum_variant,
    reason = "wire protocol prioritizes straightforward serde payloads over boxing every larger event variant"
)]
pub enum ServerEvent {
    /// Serialized todo card snapshot containing todos, plan, and goals.
    #[serde(rename = "todos")]
    Todos { session_id: String, payload: String },
    #[serde(rename = "account_usage")]
    AccountUsage {
        id: u64,
        anthropic: Vec<AccountUsageSnapshot>,
        openai: Vec<AccountUsageSnapshot>,
    },
    #[serde(rename = "runtime_command_result")]
    RuntimeCommandResult {
        id: u64,
        command: String,
        data: serde_json::Value,
        message: String,
    },
    #[serde(rename = "tools")]
    Tools {
        id: u64,
        tools: Vec<SessionToolDefinition>,
    },
    #[serde(rename = "tool_call")]
    ToolCall {
        session_id: String,
        call_id: String,
        name: String,
        input: serde_json::Value,
    },
    /// An autonomous wake was requested. In external wake mode this event is
    /// emitted instead of starting or injecting into a turn.
    #[serde(rename = "wake_requested")]
    WakeRequested {
        session_id: String,
        reason: String,
        notification: String,
    },

    /// Acknowledgment of request
    #[serde(rename = "ack")]
    Ack { id: u64 },

    /// Authoritative worker routing after a successful session preference update.
    #[serde(rename = "agent_models_changed")]
    AgentModelsChanged {
        id: u64,
        session_id: String,
        #[serde(default)]
        overrides: std::collections::BTreeMap<String, String>,
    },

    /// Streaming text delta
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    /// Assistant text message boundary within a provider response.
    #[serde(rename = "text_done")]
    TextDone,

    /// Streaming reasoning/thinking delta (raw, unformatted model text).
    ///
    /// Unlike [`ServerEvent::TextDelta`], this carries the model's reasoning as
    /// raw text deltas so the client can render the in-progress line live
    /// (token-by-token) rather than waiting for a whole line to complete. The
    /// client is responsible for the dim+italic styling. Clients that predate
    /// this event simply ignore it (reasoning is still persisted as a
    /// history-only trace and shown when the message commits).
    #[serde(rename = "reasoning_delta")]
    ReasoningDelta { text: String },

    /// Reasoning/thinking finished for the current step. Lets the client close
    /// its live reasoning region (flush the partial line, add separators) before
    /// normal output or a tool call begins.
    #[serde(rename = "reasoning_done")]
    ReasoningDone {
        /// Wall-clock reasoning duration in seconds, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_secs: Option<f64>,
    },

    /// Replace the current turn's streamed text content
    /// Used when text-wrapped tool calls are recovered: the garbled text
    /// shown during streaming is replaced with the clean prefix text.
    #[serde(rename = "text_replace")]
    TextReplace { text: String },

    /// Source-aware correction of a completed provider message. Unlike the
    /// legacy response-wide replacement, preserve everything before an exact
    /// trailing old_text match (including already committed messages).
    #[serde(rename = "text_suffix_replace")]
    TextSuffixReplace { old_text: String, new_text: String },

    /// Tool call started
    #[serde(rename = "tool_start")]
    ToolStart { id: String, name: String },

    /// Tool input delta (streaming JSON)
    #[serde(rename = "tool_input")]
    ToolInput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        delta: String,
    },

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

    /// Rendered images produced during the live turn, including image-bearing
    /// tool results and provider-native image generation. Lets remote clients
    /// render them inline immediately instead of waiting for History reload.
    #[serde(rename = "side_pane_images")]
    SidePaneImages {
        session_id: String,
        images: Vec<jcode_session_types::RenderedImage>,
    },

    /// Image generated by a provider-native image generation tool.
    #[serde(rename = "generated_image")]
    GeneratedImage {
        id: String,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata_path: Option<String>,
        output_format: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        revised_prompt: Option<String>,
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
        /// Authoritative session context budget after applying its override.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effective_budget: Option<usize>,
    },

    /// Prompt-shape signature for the API request that will later report token
    /// usage. Remote clients use this to diagnose KV-cache misses.
    #[serde(rename = "kv_cache_request")]
    KvCacheRequest {
        system_static_hash: u64,
        tools_hash: u64,
        messages_hash: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        message_hashes: Vec<u64>,
        message_count: usize,
        tool_count: usize,
        #[serde(default)]
        system_static_chars: usize,
        #[serde(default)]
        tools_json_chars: usize,
        #[serde(default)]
        messages_json_chars: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ephemeral_hash: Option<u64>,
        #[serde(default)]
        ephemeral_chars: usize,
        #[serde(default)]
        ephemeral_message_count: usize,
    },

    /// Daemon-classified KV (prompt) cache miss for the request that just
    /// completed. Emitted after `tokens`. `reason` is a stable snake_case id
    /// (e.g. `prefix_changed`, `tools_changed`, `expired`, `model_switch`).
    #[serde(rename = "kv_cache_miss")]
    KvCacheMiss {
        reason: String,
        /// True when the harness itself changed the cached prefix.
        harness_caused: bool,
        missed_tokens: u64,
        expected_tokens: u64,
        read_tokens: u64,
        /// Documented intentional invalidation that explains the miss.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        documented_cause: Option<String>,
        /// Ready-to-display one-line summary.
        message: String,
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

    /// Effective tier reported for a request, alongside its configured tier.
    #[serde(rename = "service_tier_reported")]
    ServiceTierReported {
        requested: Option<String>,
        tier: String,
    },

    /// Provider has finished the visible assistant message, but the turn may still be
    /// finalizing bookkeeping such as session IDs or completion trailers.
    ///
    /// `stop_reason` carries the provider's own reason when it supplied one
    /// (e.g. Anthropic `end_turn`, `tool_use`, `max_tokens`). It must be
    /// forwarded rather than dropped: `max_tokens` is the only signal that a
    /// turn was truncated by the output budget, and headless consumers
    /// (`run --ndjson`) have no other way to detect it.
    #[serde(rename = "message_end")]
    MessageEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },

    /// A transient transport fault interrupted the provider stream mid-response
    /// and the provider is retrying the request from the top. The client must
    /// discard all partial output from the current attempt (streamed text,
    /// reasoning, in-progress tool calls) so the replayed response renders
    /// cleanly instead of duplicating.
    #[serde(rename = "retry_rollback")]
    RetryRollback { attempt: u32, max: u32 },

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<PlanGraphStatus>,
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
        /// Caller ids of the queued messages combined into this injection, in
        /// queue order. Messages queued without an id contribute nothing.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        client_ids: Vec<String>,
    },

    /// Reply to a `cancel_soft_interrupts` that named `client_ids`. Decided
    /// under the queue lock, so an id is never both cancelled and injected.
    #[serde(rename = "soft_interrupts_cancelled")]
    SoftInterruptsCancelled {
        id: u64,
        /// Requested ids that were still queued and are now removed.
        cancelled: Vec<String>,
        /// Requested ids that were not queued: already injected, or unknown.
        not_queued: Vec<String>,
    },

    /// Reply to `background_tool`.
    #[serde(rename = "background_tool_result")]
    BackgroundToolResult {
        id: u64,
        /// A tool was executing and has been signalled to move to the
        /// background. A tool finishing in the same instant completes normally.
        moved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
    },

    /// Soft interrupts still queued when a turn ended. Sent immediately before
    /// the turn's terminal `done`/`error` on the same ordered stream.
    #[serde(rename = "soft_interrupt_queue")]
    SoftInterruptQueue { pending: usize },

    /// Structured abnormal turn outcome, emitted before the terminal Done/Error.
    #[serde(rename = "turn_stopped")]
    TurnStopped {
        reason: TurnStopReason,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_stop_reason: Option<String>,
    },

    /// Current turn was interrupted by explicit user cancel.
    ///
    /// This is rendered as a system/status notice (not assistant content),
    /// so it does not blend into streaming model output.
    #[serde(rename = "interrupted")]
    Interrupted,

    /// The provider ended the turn without any visible assistant output,
    /// typically a model-side guardrail/refusal stop (e.g. Anthropic
    /// `stop_reason: "refusal"`), or a reasoning-only response with no final
    /// text. Rendered as a system notice so the user learns why no response
    /// arrived instead of the turn ending silently.
    #[serde(rename = "provider_guardrail")]
    ProviderGuardrail {
        /// Raw provider stop reason, when known (e.g. "refusal").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
        /// Human-readable explanation for display.
        message: String,
    },

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

    /// A context-only user message was appended and persisted. This is distinct
    /// from `done`: no model turn was started and no turn boundary should be
    /// emitted to API clients.
    #[serde(rename = "context_message_added")]
    ContextMessageAdded { id: u64 },

    /// Another client attached to this session started a turn with this
    /// message. Sent to every other live attachment so each window shows the
    /// prompt (or the auto-poke notice) instead of only the sender seeing it.
    #[serde(rename = "peer_user_message")]
    PeerUserMessage { content: String },

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
    Pong {
        id: u64,
        /// Native SSH protocol v1 supports opt-in disconnected turn continuation.
        /// Omitted by older daemons, which a new SSH bridge must reject.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        native_ssh_protocol: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
        /// Authoritative connected non-headless users. None on older daemons.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_connected_session_ids: Option<Vec<String>>,
    },

    /// Current state (debug)
    #[serde(rename = "state")]
    State {
        id: u64,
        session_id: String,
        message_count: usize,
        is_processing: bool,
        /// Soft interrupts queued and not yet injected. Omitted by older daemons.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pending_soft_interrupts: Option<usize>,
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

    /// Server requests that this client/session close itself.
    #[serde(rename = "session_close_requested")]
    SessionCloseRequested { reason: String },

    /// Session display title changed.
    #[serde(rename = "session_renamed")]
    SessionRenamed {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        display_title: String,
    },

    /// Full conversation history (response to GetHistory)
    #[serde(rename = "history")]
    History {
        id: u64,
        session_id: String,
        messages: Vec<HistoryMessage>,
        /// When set, `messages` holds only the messages after the first
        /// `messages_base_count` messages, which the client already has (it
        /// sent a matching `history_base`). Absent means a full transcript.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        messages_base_count: Option<usize>,
        /// `images` was omitted because it matches the client's advertised
        /// `history_base.images_fingerprint`; keep the local image set.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        images_unchanged: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<jcode_session_types::RenderedImage>,
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
        available_model_routes: Vec<jcode_provider_core::ModelRoute>,
        /// Authoritative service tiers from the live provider, including an empty unsupported list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        available_service_tiers: Option<Vec<String>>,
        /// Connected MCP server names
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mcp_servers: Vec<String>,
        /// Available skill names
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skills: Vec<String>,
        /// Total session token usage (input, output)
        #[serde(skip_serializing_if = "Option::is_none")]
        total_tokens: Option<(u64, u64)>,
        /// Detailed persisted token usage totals for diagnostics and cache stats.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_usage_totals: Option<TokenUsageTotals>,
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
        /// Server-owned reload recovery directive for this session, if a reconnect should continue automatically.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reload_recovery: Option<ReloadRecoverySnapshot>,
        /// Last observed actual connection type for this session (e.g. websocket, https/sse)
        #[serde(skip_serializing_if = "Option::is_none")]
        connection_type: Option<String>,
        /// Last observed provider-supplied status detail for this session.
        #[serde(skip_serializing_if = "Option::is_none")]
        status_detail: Option<String>,
        /// Upstream provider (e.g., which provider OpenRouter routed to, or calculated preference)
        #[serde(skip_serializing_if = "Option::is_none")]
        upstream_provider: Option<String>,
        /// Server-resolved billing credential for this session: `Oauth`
        /// (subscription) vs `ApiKey` (cost-based), or `None` when the active
        /// provider has no OAuth-vs-API-key distinction. Lets remote clients
        /// render usage/billing without re-deriving it from the provider name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolved_credential: Option<jcode_provider_core::ResolvedCredential>,
        /// Authoritative selected model-provider OAuth account email, shared
        /// only while daemon usage telemetry is enabled. Never a credential.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_oauth_email: Option<ProviderOAuthEmail>,
        /// Authoritative live provider OAuth account label, independent of telemetry.
        /// None means the provider has no known OAuth account label.
        #[serde(default)]
        provider_oauth_account_label: Option<String>,
        /// Reasoning effort for providers that expose it
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// Service tier override for OpenAI models
        #[serde(skip_serializing_if = "Option::is_none")]
        service_tier: Option<String>,
        /// Session-scoped preferred model for subagents.
        #[serde(skip_serializing_if = "Option::is_none")]
        subagent_model: Option<String>,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        agent_model_overrides: std::collections::BTreeMap<String, String>,
        /// Session-scoped automatic review toggle.
        #[serde(skip_serializing_if = "Option::is_none")]
        autoreview_enabled: Option<bool>,
        /// Session-scoped automatic judge toggle.
        #[serde(skip_serializing_if = "Option::is_none")]
        autojudge_enabled: Option<bool>,
        /// Active compaction mode for this session
        #[serde(default)]
        compaction_mode: jcode_config_types::CompactionMode,
        /// Current live processing state for this session, if known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        activity: Option<SessionActivitySnapshot>,
        /// Session-scoped side panel pages and active focus state
        #[serde(default, skip_serializing_if = "snapshot_is_empty")]
        side_panel: SidePanelSnapshot,
        /// Session-scoped agent applet instances.
        #[serde(default, skip_serializing_if = "applets_is_empty")]
        applets: jcode_applet_types::AgentApplets,
    },

    /// Expanded compacted-history window (response to GetCompactedHistory).
    #[serde(rename = "compacted_history")]
    CompactedHistory {
        id: u64,
        session_id: String,
        messages: Vec<HistoryMessage>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<jcode_session_types::RenderedImage>,
        compacted_total: usize,
        compacted_visible: usize,
        compacted_remaining: usize,
        #[serde(default)]
        compacted_hidden_prompts: usize,
    },

    /// Side panel state changed for the active session
    #[serde(rename = "side_panel_state")]
    SidePanelState { snapshot: SidePanelSnapshot },

    /// Agent applet instances changed for the active session (full snapshot).
    #[serde(rename = "applet_state")]
    AppletState {
        session_id: String,
        snapshot: jcode_applet_types::AgentApplets,
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

    /// Session context cap changed, including the authoritative budget.
    #[serde(rename = "context_cap_changed")]
    ContextCapChanged {
        id: u64,
        session_id: String,
        /// Normalized override, null when cleared.
        max_context_tokens: Option<usize>,
        effective_budget: usize,
    },

    /// Model changed (response to cycle_model)
    #[serde(rename = "model_changed")]
    ModelChanged {
        id: u64,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_name: Option<String>,
        /// The server's resolved context window for the active route, in tokens.
        ///
        /// A remote client cannot derive this itself: its local provider is an
        /// inert placeholder with no model catalog, so asking it returns the
        /// generic 200K default. The server owns the real provider, so it
        /// reports the number directly. Measured on a pinned
        /// `stealth/space-bunny-alpha@Stealth` route whose catalog entry carries
        /// context_length 1000000, the panel showed 200000 with this absent.
        ///
        /// Omitted when unknown, so older peers stay compatible.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Credential the switched-to route will bill against (OAuth vs API
        /// key). Lets clients update the auth badge on an OAuth<->API switch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolved_credential: Option<jcode_provider_core::ResolvedCredential>,
        /// Effective OAuth identity for the selected model-provider account.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_oauth_email: Option<ProviderOAuthEmail>,
        /// Authoritative live provider OAuth account label, independent of telemetry.
        /// None means the provider has no known OAuth account label.
        #[serde(default)]
        provider_oauth_account_label: Option<String>,
        /// Effort the switched-to model runs with. A switch can clear an
        /// effort the new model does not advertise, so clients must not keep
        /// showing the old one. Always serialized (`null` = no effort) so a
        /// client can tell "cleared" from an older server that omits it.
        #[serde(default)]
        reasoning_effort: Option<String>,
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
        /// Accepted but waiting for access to session configuration, not applied.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pending: bool,
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
        mode: jcode_config_types::CompactionMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Usage delta for a route, independent of catalog availability or Agent locks.
    #[serde(rename = "model_usage_updated")]
    ModelUsageUpdated {
        route: jcode_provider_core::ModelRoute,
    },

    /// Available models updated (pushed after auth changes)
    #[serde(rename = "available_models_updated")]
    AvailableModelsUpdated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_model: Option<String>,
        available_models: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        available_model_routes: Vec<jcode_provider_core::ModelRoute>,
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
    InputShellResult { result: InputShellResult },

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

    /// Response to comm_list_swarms and comm_set_swarm_label requests
    #[serde(rename = "comm_swarms")]
    CommSwarms { id: u64, swarms: Vec<SwarmInfo> },

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

    /// Response to comm_status request
    #[serde(rename = "comm_status_response")]
    CommStatusResponse {
        id: u64,
        snapshot: AgentStatusSnapshot,
    },

    /// Response to comm_report request
    #[serde(rename = "comm_report_response")]
    CommReportResponse {
        id: u64,
        status: String,
        message: String,
    },

    /// Response to comm_plan_status request
    #[serde(rename = "comm_plan_status_response")]
    CommPlanStatusResponse { id: u64, summary: PlanGraphStatus },

    /// Response to comm_assign_task request
    #[serde(rename = "comm_assign_task_response")]
    CommAssignTaskResponse {
        id: u64,
        task_id: String,
        target_session: String,
    },

    /// Response to comm_task_control request
    #[serde(rename = "comm_task_control_response")]
    CommTaskControlResponse {
        id: u64,
        action: String,
        task_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_session: Option<String>,
        status: String,
        summary: PlanGraphStatus,
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

    /// Response to comm_list_models request
    #[serde(rename = "comm_list_models_response")]
    CommListModelsResponse {
        id: u64,
        /// The coordinator's currently active model (spawn default when no
        /// override is configured or requested).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_model: Option<String>,
        /// The configured `agents.swarm_model` pin, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        configured_swarm_model: Option<String>,
        /// All model routes known to the server (model + provider + auth
        /// method + availability + rough cost estimate).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        model_routes: Vec<jcode_provider_core::ModelRoute>,
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
        /// True when the wait was handed off to a detached background watcher.
        /// In that case `members`/`completed` describe the current snapshot, not
        /// a final result; completion is delivered later via notify/wake.
        #[serde(default)]
        background_started: bool,
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

    /// Response to resume_all_sessions — summary of which sessions were continued.
    #[serde(rename = "resume_all_result")]
    ResumeAllResult {
        id: u64,
        /// Number of live sessions that were continued by this request.
        resumed: usize,
        /// Number of live sessions inspected but skipped (idle/complete/busy).
        skipped: usize,
        /// Friendly names (or short ids) of the sessions that were continued.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        resumed_sessions: Vec<String>,
        /// Human-readable summary suitable for direct display.
        message: String,
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

    /// The agent hit a decision point and offers the user options
    /// (raised by the `ask_user` tool). The client renders a chooser;
    /// free-form input is always valid alongside the listed options.
    #[serde(rename = "decision_request")]
    DecisionRequest {
        /// Unique request ID for matching the response
        request_id: String,
        /// What is being decided, phrased for the user
        question: String,
        /// 2-5 offered options
        options: Vec<DecisionOption>,
        /// Tool call ID this is associated with
        tool_call_id: String,
    },
}

/// One offered choice in a decision request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionOption {
    /// Short label shown in the chooser
    pub label: String,
    /// Optional one-line explanation of what this option implies
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The user's answer to a decision request: either a 1-based index into
/// the offered options, or free-form text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecisionChoice {
    /// The user picked a listed option
    Option { index: u32 },
    /// The user typed a free-form answer instead
    Text { text: String },
    /// The user dismissed the chooser without answering
    Dismissed,
}
