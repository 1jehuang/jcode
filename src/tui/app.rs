use super::keybind::{
    CenteredToggleKeys, ModelSwitchKeys, OptionalBinding, ScrollKeys, WorkspaceNavigationKeys,
};
use super::markdown::IncrementalMarkdownRenderer;
use super::stream_buffer::StreamBuffer;
use crate::bus::{Bus, BusEvent, LoginCompleted, ToolEvent, ToolStatus};
use crate::compaction::CompactionEvent;
use crate::config::config;
use crate::id;
use crate::mcp::McpManager;
use crate::message::{
    ContentBlock, Message, Role, StreamEvent, TOOL_OUTPUT_MISSING_TEXT, ToolCall, ToolDefinition,
};
use crate::provider::Provider;
use crate::runtime_memory_log::RuntimeMemoryLogController;
use crate::session::{Session, StoredMessage};
use crate::skill::SkillRegistry;
use crate::tool::selfdev::ReloadContext;
use crate::tool::{Registry, ToolContext};
use anyhow::Result;
use auth::PendingLogin;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use debug::DebugTrace;
use futures::StreamExt;
use helpers::*;
use ratatui::DefaultTerminal;
use std::cell::RefCell;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;

mod auth;
mod auth_account_picker_saved_accounts;
mod catchup;
mod commands;
mod commands_improve;
mod commands_overnight;
mod commands_review;
mod conversation_state;
mod copy_selection;
mod debug;
mod dictation;
mod event_wrappers;
mod handterm_native_scroll;
mod helpers;
mod inline_interactive;
mod input;
mod input_help;
mod local;
mod misc_ui;
mod model_context;
mod navigation;
mod observe;
mod remote;
mod remote_notifications;
mod replay;
mod run_shell;
mod runtime_memory;
mod split_view;
mod state_ui;
mod state_ui_input_helpers;
mod state_ui_maintenance;
mod state_ui_messages;
mod state_ui_runtime;
mod state_ui_storage;
mod todos_view;
mod tui_lifecycle;
mod tui_lifecycle_runtime;
mod tui_state;
mod turn;
mod turn_memory;

pub(crate) use self::state_ui_storage::compact_display_messages_for_storage;

pub(crate) fn extract_input_shell_command(input: &str) -> Option<&str> {
    self::input::extract_input_shell_command(input)
}

#[derive(Debug, Clone)]
struct PendingRemoteMessage {
    content: String,
    images: Vec<(String, String)>,
    is_system: bool,
    system_reminder: Option<String>,
    auto_retry: bool,
    retry_attempts: u8,
    retry_at: Option<Instant>,
}

#[derive(Debug, Clone)]
struct PendingSplitPrompt {
    content: String,
    images: Vec<(String, String)>,
}

struct PendingLocalTransfer {
    receiver: mpsc::Receiver<anyhow::Result<PreparedTransferSession>>,
}

#[derive(Debug, Clone)]
struct LocalRewindUndoSnapshot {
    messages: Vec<StoredMessage>,
    provider_session_id: Option<String>,
    session_provider_session_id: Option<String>,
    visible_message_count: usize,
}

#[derive(Debug, Clone)]
struct PendingRemoteRewindNotice {
    undo: bool,
    message_index: Option<usize>,
    changed_messages: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KvCacheRequestSignature {
    system_static_hash: u64,
    tools_hash: u64,
    messages_hash: u64,
    message_count: usize,
    tool_count: usize,
}

#[derive(Debug, Clone)]
struct KvCacheBaseline {
    input_tokens: u64,
    completed_at: Instant,
    provider: String,
    model: String,
    upstream_provider: Option<String>,
    signature: Option<KvCacheRequestSignature>,
}

#[derive(Debug, Clone)]
struct PendingKvCacheRequest {
    turn_number: usize,
    call_index: u16,
    provider: String,
    model: String,
    upstream_provider: Option<String>,
    signature: Option<KvCacheRequestSignature>,
    baseline_messages_prefix_matches: Option<bool>,
    baseline: Option<KvCacheBaseline>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KvCacheMissReason {
    ProviderSwitch,
    ModelSwitch,
    UpstreamSwitch,
    Expired,
    HarnessSystemChanged,
    HarnessToolsChanged,
    HarnessPrefixChanged,
    ZeroRead,
    LowRead,
    Unknown,
}

impl KvCacheMissReason {
    fn label(self) -> &'static str {
        match self {
            Self::ProviderSwitch => "provider switch",
            Self::ModelSwitch => "model switch",
            Self::UpstreamSwitch => "upstream switch",
            Self::Expired => "expired",
            Self::HarnessSystemChanged => "harness: system changed",
            Self::HarnessToolsChanged => "harness: tools changed",
            Self::HarnessPrefixChanged => "harness: prefix changed",
            Self::ZeroRead => "zero read",
            Self::LowRead => "low read",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
struct KvCacheMissSample {
    turn_number: usize,
    call_index: u16,
    missed_tokens: u64,
    reason: KvCacheMissReason,
}

struct PendingSessionPickerLoad {
    receiver: mpsc::Receiver<
        anyhow::Result<(
            Vec<super::session_picker::ServerGroup>,
            Vec<super::session_picker::SessionInfo>,
        )>,
    >,
}

struct PendingModelPickerLoad {
    request_id: u64,
    signature: ModelPickerCacheSignature,
    picker_started: Instant,
    receiver: mpsc::Receiver<anyhow::Result<ModelPickerRoutesResult>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelPickerCacheSignature {
    is_remote: bool,
    provider_name: String,
    current_model: String,
    config_default_model: Option<String>,
    reasoning_effort: Option<String>,
    available_efforts: Vec<String>,
    simplified_model_picker: bool,
    catalog_revision: u64,
    remote_provider_name: Option<String>,
    remote_available_len: usize,
    remote_available_first: Option<String>,
    remote_available_last: Option<String>,
    remote_routes_len: usize,
    remote_routes_first: Option<String>,
    remote_routes_last: Option<String>,
}

#[derive(Debug, Clone)]
struct ModelPickerCache {
    signature: ModelPickerCacheSignature,
    entries: Vec<super::PickerEntry>,
    route_count: usize,
    model_count: usize,
}

struct ModelPickerRoutesResult {
    routes: Vec<crate::provider::ModelRoute>,
    routes_ms: u128,
}

#[derive(Debug, Clone)]
struct PreparedTransferSession {
    session_id: String,
    session_name: String,
}

#[derive(Debug, Clone)]
struct PendingProviderFailover {
    prompt: crate::provider::ProviderFailoverPrompt,
    deadline: Instant,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SessionPickerMode {
    #[default]
    Resume,
    CatchUp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingCatchupResume {
    pub target_session_id: String,
    pub source_session_id: Option<String>,
    pub queue_position: Option<(usize, usize)>,
    pub show_brief: bool,
}

#[derive(Clone, Debug)]
pub(super) struct RemoteResumeActivity {
    pub session_id: String,
    pub observed_at: Instant,
    pub current_tool_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingReloadReconnectStatus {
    AwaitingHistory { session_id: Option<String> },
}

const MEMORY_INJECTION_SUPPRESSION_SECS: u64 = 90;

/// Current processing status
#[derive(Clone, Default, Debug)]
pub enum ProcessingStatus {
    #[default]
    Idle,
    /// Sending request to API (with optional connection phase detail)
    Sending,
    /// Connection phase update from transport layer
    Connecting(crate::message::ConnectionPhase),
    /// Model is reasoning/thinking (real-time duration tracking)
    Thinking(Instant),
    /// Receiving streaming response
    Streaming,
    /// Executing a tool
    RunningTool(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RemoteStartupPhase {
    StartingServer,
    Connecting,
    LoadingSession,
    WaitingForReload,
    Reconnecting { attempt: u32 },
}

impl RemoteStartupPhase {
    pub(crate) fn header_label(&self) -> String {
        match self {
            Self::StartingServer => "starting server…".to_string(),
            Self::Connecting => "connecting to server…".to_string(),
            Self::LoadingSession => "loading session…".to_string(),
            Self::WaitingForReload => "waiting for reload…".to_string(),
            Self::Reconnecting { attempt } => format!("reconnecting ({attempt})…"),
        }
    }

    pub(crate) fn header_label_with_elapsed(&self, elapsed: Duration) -> String {
        let base = self.header_label();
        if elapsed < Duration::from_secs(1) {
            return base;
        }

        let elapsed_str = if elapsed.as_secs() < 60 {
            format!("{}s", elapsed.as_secs())
        } else {
            format!("{}m {}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
        };

        format!("{base} {elapsed_str}")
    }
}

/// A message in the conversation for display
#[derive(Clone)]
pub struct DisplayMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub duration_secs: Option<f32>,
    pub title: Option<String>,
    /// Full tool call data (for role="tool" messages)
    pub tool_data: Option<ToolCall>,
}

pub(super) fn reload_persisted_background_tasks_note(session_id: &str) -> String {
    crate::tool::selfdev::persisted_background_tasks_note(session_id)
}

#[derive(Clone, Default)]
pub struct CopyBadgeUiState {
    pub alt_active: bool,
    pub shift_active: bool,
    pub alt_pulse_until: Option<Instant>,
    pub shift_pulse_until: Option<Instant>,
    pub key_active: Option<(char, Instant)>,
    pub copied_feedback: Option<CopyBadgeFeedback>,
}

#[derive(Clone)]
pub struct CopyBadgeFeedback {
    pub key: char,
    pub success: bool,
    pub expires_at: Instant,
}

impl CopyBadgeUiState {
    fn pulse_active(expires_at: Option<Instant>, now: Instant) -> bool {
        expires_at.is_some_and(|expires_at| expires_at > now)
    }

    pub(crate) fn alt_is_active(&self, now: Instant) -> bool {
        self.alt_active || Self::pulse_active(self.alt_pulse_until, now)
    }

    pub(crate) fn shift_is_active(&self, now: Instant) -> bool {
        self.alt_is_active(now)
            && (self.shift_active || Self::pulse_active(self.shift_pulse_until, now))
    }

    pub(crate) fn key_is_active(&self, key: char, now: Instant) -> bool {
        self.shift_is_active(now)
            && self
                .key_active
                .as_ref()
                .map(|(active_key, expires_at)| {
                    active_key.eq_ignore_ascii_case(&key) && *expires_at > now
                })
                .unwrap_or(false)
    }

    pub(crate) fn feedback_for_key(&self, key: char, now: Instant) -> Option<bool> {
        self.copied_feedback.as_ref().and_then(|feedback| {
            if feedback.key.eq_ignore_ascii_case(&key) && feedback.expires_at > now {
                Some(feedback.success)
            } else {
                None
            }
        })
    }
}

/// Result from running the TUI
#[derive(Debug, Default)]
pub struct RunResult {
    /// Session ID to reload (hot-reload, no rebuild)
    pub reload_session: Option<String>,
    /// Session ID to rebuild (full git pull + cargo build + tests)
    pub rebuild_session: Option<String>,
    /// Session ID to update (download from GitHub releases and reload)
    pub update_session: Option<String>,
    /// Session ID to restart (exec into current binary, no build)
    pub restart_session: Option<String>,
    /// Exit code to use (for canary wrapper communication)
    pub exit_code: Option<i32>,
    /// The session ID that was active (for resume hints on exit)
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendAction {
    Submit,
    Queue,
    Interleave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImproveMode {
    ImproveRun,
    ImprovePlan,
    RefactorRun,
    RefactorPlan,
}

impl ImproveMode {
    pub(super) fn status_label(self) -> &'static str {
        match self {
            Self::ImproveRun => "active improvement loop",
            Self::ImprovePlan => "improvement plan-only",
            Self::RefactorRun => "active refactor loop",
            Self::RefactorPlan => "refactor plan-only",
        }
    }

    pub(super) fn is_improve(self) -> bool {
        matches!(self, Self::ImproveRun | Self::ImprovePlan)
    }

    pub(super) fn is_refactor(self) -> bool {
        matches!(self, Self::RefactorRun | Self::RefactorPlan)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MouseScrollTarget {
    Chat,
    SidePane,
    HelpOverlay,
    ChangelogOverlay,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CompactedHistoryLazyState {
    pub total_messages: usize,
    pub visible_messages: usize,
    pub remaining_messages: usize,
    pub pending_request_visible: Option<usize>,
}

/// State for an in-progress OAuth/API-key login flow triggered by `/login`.
/// TUI Application state
pub struct App {
    provider: Arc<dyn Provider>,
    registry: Registry,
    skills: Arc<SkillRegistry>,
    mcp_manager: Arc<RwLock<McpManager>>,
    messages: Vec<Message>,
    session: Session,
    display_messages: Vec<DisplayMessage>,
    display_messages_version: u64,
    display_user_message_count: usize,
    display_edit_tool_message_count: usize,
    compacted_history_lazy: CompactedHistoryLazyState,
    input: String,
    cursor_pos: usize,
    scroll_offset: usize,
    /// Pauses auto-scroll when user scrolls up during streaming
    auto_scroll_paused: bool,
    active_skill: Option<String>,
    is_processing: bool,
    streaming_text: String,
    should_quit: bool,
    // Message queueing
    queued_messages: Vec<String>,
    hidden_queued_system_messages: Vec<String>,
    current_turn_system_reminder: Option<String>,
    // Live token usage (per turn)
    streaming_input_tokens: u64,
    streaming_output_tokens: u64,
    streaming_cache_read_tokens: Option<u64>,
    streaming_cache_creation_tokens: Option<u64>,
    // Upstream provider (e.g., which provider OpenRouter routed to)
    upstream_provider: Option<String>,
    // Active stream connection type (websocket/https/etc.)
    connection_type: Option<String>,
    // Provider-supplied human-readable transport detail for the current stream
    status_detail: Option<String>,
    // Total session token usage (accumulated across all turns)
    total_input_tokens: u64,
    total_output_tokens: u64,
    // Total session KV cache usage for turns where the provider reported cache telemetry.
    total_cache_reported_input_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_creation_tokens: u64,
    total_cache_optimal_input_tokens: u64,
    last_cache_reported_input_tokens: Option<u64>,
    last_cache_read_tokens: Option<u64>,
    last_cache_optimal_input_tokens: Option<u64>,
    cache_next_optimal_input_tokens: Option<u64>,
    kv_cache_baseline: Option<KvCacheBaseline>,
    pending_kv_cache_request: Option<PendingKvCacheRequest>,
    kv_cache_turn_number: Option<usize>,
    kv_cache_turn_call_index: u16,
    kv_cache_miss_samples: Vec<KvCacheMissSample>,
    // Total cost in USD (for API-key providers)
    total_cost: f32,
    // Cached pricing (input $/1M tokens, output $/1M tokens)
    cached_prompt_price: Option<f32>,
    cached_completion_price: Option<f32>,
    // Context limit tracking (for compaction warning)
    context_limit: u64,
    context_warning_shown: bool,
    // Context info (what's loaded in system prompt)
    context_info: crate::prompt::ContextInfo,
    // Track last streaming activity for "stale" detection
    last_stream_activity: Option<Instant>,
    // Provider has emitted MessageEnd, but the turn is still finalizing bookkeeping.
    stream_message_ended: bool,
    // Server-reported processing snapshot captured from resume/history before live events arrive.
    remote_resume_activity: Option<RemoteResumeActivity>,
    // Reload reconnect is waiting for server history before deciding whether to continue.
    pending_reload_reconnect_status: Option<PendingReloadReconnectStatus>,
    // Accurate TPS tracking: only counts actual token streaming time, not tool execution
    /// Set when first TextDelta arrives in a streaming response
    streaming_tps_start: Option<Instant>,
    /// Accumulated streaming-only time across agentic loop iterations
    streaming_tps_elapsed: Duration,
    /// Whether incoming output-token deltas should contribute to TPS.
    ///
    /// This is enabled only while user-visible assistant text is streaming, and stays
    /// enabled briefly after message end so late final usage snapshots still count.
    streaming_tps_collect_output: bool,
    /// Accumulated output tokens across all API calls in a turn.
    ///
    /// Providers may emit repeated cumulative usage snapshots for a single API call,
    /// so we accumulate per-call deltas to avoid double counting.
    streaming_total_output_tokens: u64,
    /// Latest visible-output token snapshot used for TPS display.
    ///
    /// We update this only when newly visible output tokens are observed. That keeps the
    /// displayed TPS anchored to the latest real token sample instead of decaying on every
    /// redraw while no new usage data has arrived.
    streaming_tps_observed_output_tokens: u64,
    /// Streaming-only elapsed time corresponding to streaming_tps_observed_output_tokens.
    streaming_tps_observed_elapsed: Duration,
    // Current status
    status: ProcessingStatus,
    // Subagent status (shown during Task tool execution)
    subagent_status: Option<String>,
    // Batch progress (shown during batch tool execution)
    batch_progress: Option<crate::bus::BatchProgress>,
    processing_started: Option<Instant>,
    // User-visible turn timer. Preserved across synthetic auto-poke follow-ups so elapsed time
    // reflects the original user turn rather than only the latest poke resend.
    visible_turn_started: Option<Instant>,
    // When the last API response completed (for cache TTL tracking)
    last_api_completed: Option<Instant>,
    // Provider/model that produced the last completed API response. A warm cache is only
    // meaningful for the same provider and model; switching either should make cache state cold.
    last_api_completed_provider: Option<String>,
    last_api_completed_model: Option<String>,
    // Input tokens from the last completed turn (for cache TTL display)
    last_turn_input_tokens: Option<u64>,
    // Pending turn to process (allows UI to redraw before processing starts)
    pending_turn: bool,
    // When armed by /poke, automatically continue prompting until todos are complete.
    auto_poke_incomplete_todos: bool,
    // Pending cross-provider resend after a failover warning/countdown.
    pending_provider_failover: Option<PendingProviderFailover>,
    // Local session file write to flush once the first "sending" frame is visible.
    session_save_pending: bool,
    // Tool calls detected during streaming (shown in real-time with details)
    streaming_tool_calls: Vec<ToolCall>,
    // Provider-specific session ID for conversation resume
    provider_session_id: Option<String>,
    // One-step undo snapshot captured before the most recent local rewind.
    rewind_undo_snapshot: Option<LocalRewindUndoSnapshot>,
    // Cancel flag for interrupting generation
    cancel_requested: bool,
    // Quit confirmation: tracks when first Ctrl+C was pressed
    quit_pending: Option<Instant>,
    // Debounce redraw storms while the terminal is being resized.
    last_resize_redraw: Option<Instant>,
    // Cached MCP server names and tool counts (updated on connect/disconnect)
    mcp_server_names: Vec<(String, usize)>,
    // Semantic stream buffer for chunked output
    stream_buffer: StreamBuffer,
    // Track thinking start time for extended thinking display
    thinking_start: Option<Instant>,
    // Whether we've inserted the current turn's thought line
    thought_line_inserted: bool,
    // Buffer for accumulating thinking content during a thinking session
    thinking_buffer: String,
    // Whether we've emitted the 💭 prefix for the current thinking session
    thinking_prefix_emitted: bool,
    // Hot-reload: if set, exec into new binary with this session ID (no rebuild)
    reload_requested: Option<String>,
    // Hot-rebuild: if set, do full git pull + cargo build + tests then exec
    rebuild_requested: Option<String>,
    // Update: if set, check for and download update from GitHub releases then exec
    update_requested: Option<String>,
    // Interactive background client maintenance action currently running
    background_client_action: Option<crate::bus::ClientMaintenanceAction>,
    // Reload the updated/rebuilt client once the current turn is idle
    pending_background_client_reload: Option<(String, crate::bus::ClientMaintenanceAction)>,
    // Restart: if set, exec into current binary with this session ID (no build)
    restart_requested: Option<String>,
    // Pasted content storage (displayed as placeholders, expanded on submit)
    pasted_contents: Vec<String>,
    // Pending pasted images (media_type, base64_data) attached to next message
    pending_images: Vec<(String, String)>,
    // One-shot flag: the next submitted prompt is routed to a new headed session.
    route_next_prompt_to_new_session: bool,
    // Restore-time flag: auto-submit restored input after startup.
    submit_input_on_startup: bool,
    // Inline UI state for copy badges ([Alt] [⇧] [S])
    copy_badge_ui: CopyBadgeUiState,
    // Modal in-app selection/copy state for the chat viewport.
    copy_selection_mode: bool,
    copy_selection_anchor: Option<crate::tui::CopySelectionPoint>,
    copy_selection_cursor: Option<crate::tui::CopySelectionPoint>,
    copy_selection_pending_anchor: Option<crate::tui::CopySelectionPoint>,
    copy_selection_dragging: bool,
    copy_selection_goal_column: Option<usize>,
    // Debug socket broadcast channel (if enabled)
    debug_tx: Option<tokio::sync::broadcast::Sender<super::backend::DebugEvent>>,
    // Remote provider info (set when running in remote mode)
    remote_client_instance_id: String,
    remote_provider_name: Option<String>,
    remote_provider_model: Option<String>,
    remote_startup_phase: Option<RemoteStartupPhase>,
    remote_startup_phase_started: Option<Instant>,
    remote_reasoning_effort: Option<String>,
    remote_service_tier: Option<String>,
    remote_transport: Option<String>,
    remote_compaction_mode: Option<crate::config::CompactionMode>,
    remote_available_entries: Vec<String>,
    remote_model_options: Vec<crate::provider::ModelRoute>,
    pending_remote_model_refresh_snapshot: Option<(Vec<String>, Vec<crate::provider::ModelRoute>)>,
    // Remote MCP servers and skills (set from server in remote mode)
    remote_mcp_servers: Vec<String>,
    remote_skills: Vec<String>,
    // Total session token usage (from server in remote mode)
    remote_total_tokens: Option<(u64, u64)>,
    // Whether the remote session is canary/self-dev (from server)
    remote_is_canary: Option<bool>,
    // Remote server version (from server)
    remote_server_version: Option<String>,
    // Whether the remote server has a newer binary available
    remote_server_has_update: Option<bool>,
    // Auto-reload server when stale (set on first connect if server_has_update)
    pending_server_reload: bool,
    // Remote server short name (e.g., "running", "blazing")
    remote_server_short_name: Option<String>,
    // Remote server icon (e.g., "🔥", "🌫️")
    remote_server_icon: Option<String>,
    // Current message request ID (for remote mode - to match Done events)
    current_message_id: Option<u64>,
    // Whether running in remote mode
    is_remote: bool,
    // Remote rewind/undo request waiting for the server's replacement History payload.
    pending_remote_rewind_notice: Option<PendingRemoteRewindNotice>,
    // Server was just spawned - allow initial connection retries in run_remote
    server_spawning: bool,
    // Whether running in replay mode (readonly playback of a saved session)
    pub is_replay: bool,
    // Suppress terminal title updates for off-screen/silent replay instances.
    suppress_terminal_title_updates: bool,
    /// Override for elapsed time during headless video replay.
    pub replay_elapsed_override: Option<Duration>,
    /// Sim-time at which processing started (video replay only)
    replay_processing_started_ms: Option<f64>,
    // Remember tool call ids that have appeared in the provider transcript
    tool_call_ids: HashSet<String>,
    // Remember tool call ids that already have outputs
    tool_result_ids: HashSet<String>,
    // Number of provider messages already indexed for missing tool-output repair
    tool_output_scan_index: usize,
    // Current session ID (from server in remote mode)
    remote_session_id: Option<String>,
    // All sessions on the server (remote mode only)
    remote_sessions: Vec<String>,
    remote_side_pane_images: Vec<crate::session::RenderedImage>,
    // Swarm member status snapshots (remote mode only)
    remote_swarm_members: Vec<crate::protocol::SwarmMemberStatus>,
    // Latest swarm plan snapshot (local or remote server event stream)
    swarm_plan_items: Vec<crate::plan::PlanItem>,
    swarm_plan_version: Option<u64>,
    swarm_plan_swarm_id: Option<String>,
    // Number of connected clients (remote mode only)
    remote_client_count: Option<usize>,
    // Build version tracking for auto-migration
    known_stable_version: Option<String>,
    // Last time we checked for stable version
    last_version_check: Option<Instant>,
    // Pending migration to new stable version
    pending_migration: Option<String>,
    // Session to resume on connect (remote mode)
    resume_session_id: Option<String>,
    // Exit code to use when quitting (for canary wrapper communication)
    requested_exit_code: Option<i32>,
    // Memory feature toggle for this session
    memory_enabled: bool,
    // Automatic end-of-turn review toggle for this session
    autoreview_enabled: bool,
    // Automatic end-of-turn judge toggle for this session
    autojudge_enabled: bool,
    // Last requested `/improve` mode for this session.
    improve_mode: Option<ImproveMode>,
    // Suppress duplicate memory injection messages for near-identical prompts.
    last_injected_memory_signature: Option<(String, Instant)>,
    // Swarm feature toggle for this session
    swarm_enabled: bool,
    // Diff display mode (toggle with Shift+Tab)
    diff_mode: crate::config::DiffDisplayMode,
    // Center all content (from config)
    pub(crate) centered: bool,
    // Diagram display mode (from config)
    diagram_mode: crate::config::DiagramDisplayMode,
    // Whether the pinned diagram pane has focus
    diagram_focus: bool,
    // Selected diagram index in pinned mode (most recent = 0)
    diagram_index: usize,
    // Diagram scroll offsets in cells (only used when focused)
    diagram_scroll_x: i32,
    diagram_scroll_y: i32,
    // Diagram pane width ratio (percentage)
    diagram_pane_ratio: u8,
    // Animation state for smooth pane ratio transitions
    diagram_pane_ratio_from: u8,
    diagram_pane_ratio_target: u8,
    diagram_pane_anim_start: Option<Instant>,
    // Whether the pinned diagram pane is visible
    diagram_pane_enabled: bool,
    // Position of pinned diagram pane (side or top)
    diagram_pane_position: crate::config::DiagramPanePosition,
    // Diagram zoom percentage (100 = normal)
    diagram_zoom: u8,
    // Last diagram hash that was actually visible in the pinned pane.
    // Used to detect identity/layout changes that should reset back to fit.
    last_visible_diagram_hash: Option<u64>,
    // Whether the user is dragging the diagram pane border
    diagram_pane_dragging: bool,
    // Scroll offset for pinned diff pane
    diff_pane_scroll: usize,
    diff_pane_scroll_x: i32,
    diff_pane_focus: bool,
    diff_pane_auto_scroll: bool,
    side_panel: crate::side_panel::SidePanelSnapshot,
    observe_mode_enabled: bool,
    observe_page_markdown: String,
    observe_page_updated_at_ms: u64,
    split_view_enabled: bool,
    split_view_markdown: String,
    split_view_updated_at_ms: u64,
    split_view_rendered_display_version: u64,
    split_view_rendered_streaming_hash: u64,
    todos_view_enabled: bool,
    todos_view_markdown: String,
    todos_view_updated_at_ms: u64,
    todos_view_rendered_hash: u64,
    last_side_panel_refresh: Option<Instant>,
    // Most recently persisted focus target for dictation routing.
    last_client_focus_recorded_at: Option<Instant>,
    last_client_focus_session_id: Option<String>,
    // Most recently focused side panel page, used to restore visibility when toggled off.
    last_side_panel_focus_id: Option<String>,
    // Pin read images to side pane
    pin_images: bool,
    // Show a native terminal scrollbar in the chat viewport.
    chat_native_scrollbar: bool,
    // Show a native terminal scrollbar in the side panel.
    side_panel_native_scrollbar: bool,
    // Passive inline UI (informational blocks shown above input).
    inline_view_state: Option<super::InlineViewState>,
    // Interactive model/provider picker
    inline_interactive_state: Option<super::InlineInteractiveState>,
    // Cached model picker entries. Building these can require hydrating large provider catalogs.
    model_picker_cache: Option<ModelPickerCache>,
    model_picker_catalog_revision: u64,
    pending_model_picker_load: Option<PendingModelPickerLoad>,
    model_picker_load_request_id: u64,
    // Pending model switch from picker (for remote mode async processing)
    pending_model_switch: Option<String>,
    // Pending account switch from inline picker (for remote mode async processing)
    pending_account_picker_action: Option<crate::tui::AccountPickerAction>,
    // Keybindings for model switching
    model_switch_keys: ModelSwitchKeys,
    // Keybindings for effort switching
    effort_switch_keys: super::keybind::EffortSwitchKeys,
    // Keybindings for scrolling
    scroll_keys: ScrollKeys,
    // Keybinding for centered-mode toggle
    centered_toggle_keys: CenteredToggleKeys,
    // Keybindings for Niri-style workspace navigation
    workspace_navigation_keys: WorkspaceNavigationKeys,
    // Optional configured keybinding for external dictation
    dictation_key: OptionalBinding,
    // Active external dictation session, if one is running
    dictation_session: Option<dictation::ActiveDictation>,
    // Whether an external dictation command is currently running
    dictation_in_flight: bool,
    // Ownership token for the current dictation request.
    dictation_request_id: Option<String>,
    // Session that owned the current dictation request when it was started.
    dictation_target_session_id: Option<String>,
    // Keep the current chat viewport while typing instead of snapping to bottom.
    typing_scroll_lock: bool,
    // Scroll bookmark: stashed scroll position for quick teleport back
    scroll_bookmark: Option<usize>,
    // Stashed input: saved via Ctrl+S for later retrieval
    stashed_input: Option<(String, usize)>,
    // Undo history for in-progress input editing (Ctrl+Z)
    input_undo_stack: Vec<(String, usize)>,
    // Short-lived notice for status feedback (model switch, cycle diff mode, etc.)
    status_notice: Option<(String, Instant)>,
    // Experimental feature warnings already shown in this session.
    experimental_feature_warnings_seen: HashSet<String>,
    // Active first-use experimental warning for the currently running tool.
    active_experimental_feature_notice: Option<String>,
    // Message to interleave during processing (set via Ctrl+Enter in queue mode)
    interleave_message: Option<String>,
    // Message sent as soft interrupt but not yet injected (shown in queue preview until injected)
    pending_soft_interrupts: Vec<String>,
    // Soft interrupts written to the socket but not yet acknowledged by the server.
    pending_soft_interrupt_requests: Vec<(u64, String)>,
    // Whether the current remote turn should trigger autoreview after completion.
    autoreview_after_current_turn: bool,
    // Whether the current remote turn should trigger autojudge after completion.
    autojudge_after_current_turn: bool,
    // Startup message to preload into the next spawned split window.
    pending_split_startup_message: Option<String>,
    // Parent/original session that feedback flows should report back to after a split launch.
    pending_split_parent_session_id: Option<String>,
    // Startup user prompt to auto-submit in the next spawned split window.
    pending_split_prompt: Option<PendingSplitPrompt>,
    // Optional model override to apply before opening the next spawned split window.
    pending_split_model_override: Option<String>,
    // Optional provider key override to persist into the next spawned split window.
    pending_split_provider_key_override: Option<String>,
    // Human-friendly label for the next spawned split window flow.
    pending_split_label: Option<String>,
    // Timestamp for showing a temporary client-side running state while a split launch is in flight.
    pending_split_started_at: Option<Instant>,
    // Ask the remote followup loop to issue a split request once idle.
    pending_split_request: bool,
    // Ask the followup loop to issue a transfer request once idle.
    pending_transfer_request: bool,
    // Local transfer preparation currently running in the background.
    pending_local_transfer: Option<PendingLocalTransfer>,
    // Queue mode: if true, Enter during processing queues; if false, Enter queues to send next
    // Toggle with Ctrl+Tab or Ctrl+T
    queue_mode: bool,
    // Automatically reload the remote server when a newer server binary is detected.
    auto_server_reload: bool,
    // After an interrupt, wait one redraw before auto-dispatching queued followups so
    // the queued preview can render in the interrupted state first.
    pending_queued_dispatch: bool,
    // Tab completion state: (base_input, suggestion_index)
    // base_input is the original input before cycling, suggestion_index is current position
    tab_completion_state: Option<(String, usize)>,
    // Time when app started (for startup animations)
    app_started: Instant,
    // Optional client runtime memory logger for low-overhead attribution journaling.
    runtime_memory_log: Option<RuntimeMemoryLogController>,
    // Binary modification time when client started (for smart reload detection)
    client_binary_mtime: Option<std::time::SystemTime>,
    // Rate limit state: when rate limit resets (if rate limited)
    rate_limit_reset: Option<Instant>,
    // Message being sent when rate limit hit (to auto-retry in remote mode)
    rate_limit_pending_message: Option<PendingRemoteMessage>,
    // Last turn-level stream error (used by /fix to choose recovery actions)
    last_stream_error: Option<String>,
    // Store reload info to pass to agent after reconnection (remote mode)
    reload_info: Vec<String>,
    // Debug trace for scripted testing
    debug_trace: DebugTrace,
    // Incremental markdown renderer for streaming text (uses RefCell for interior mutability)
    streaming_md_renderer: RefCell<IncrementalMarkdownRenderer>,
    /// Ambient mode system prompt override (when running as visible ambient cycle)
    ambient_system_prompt: Option<String>,
    /// Pending login flow: if set, next input is intercepted as OAuth code or API key
    pending_login: Option<PendingLogin>,
    /// Pending account picker follow-up input (new label or setting value)
    pending_account_input: Option<auth::PendingAccountInput>,
    /// One-shot flag: force the next paint to clear the terminal first.
    /// Needed after native terminal scrolls mutate the screen outside ratatui's diff model.
    force_full_redraw: bool,
    /// Last mouse scroll event timestamp (for trackpad velocity detection)
    last_mouse_scroll: Option<Instant>,
    /// Active smooth-scroll target for queued mouse-wheel motion.
    mouse_scroll_target: Option<MouseScrollTarget>,
    /// Remaining queued mouse-wheel lines. Positive = down, negative = up.
    mouse_scroll_queue: i16,
    /// Scroll offset for changelog overlay (None = not visible)
    changelog_scroll: Option<usize>,
    help_scroll: Option<usize>,
    /// Session picker overlay (None = not visible)
    session_picker_overlay: Option<RefCell<super::session_picker::SessionPicker>>,
    session_picker_mode: SessionPickerMode,
    pending_session_picker_load: Option<PendingSessionPickerLoad>,
    catchup_return_stack: Vec<String>,
    pending_catchup_resume: Option<PendingCatchupResume>,
    in_flight_catchup_resume: Option<PendingCatchupResume>,
    /// Login picker overlay (None = not visible)
    login_picker_overlay: Option<RefCell<super::login_picker::LoginPicker>>,
    /// Account picker overlay (None = not visible)
    account_picker_overlay: Option<RefCell<super::account_picker::AccountPicker>>,
    /// Usage overlay (None = not visible)
    usage_overlay: Option<RefCell<super::usage_overlay::UsageOverlay>>,
    /// Whether a usage refresh request is currently in flight.
    usage_report_refreshing: bool,
}

/// A placeholder provider for remote mode (never actually called)
struct NullProvider;

#[async_trait::async_trait]
impl Provider for NullProvider {
    fn name(&self) -> &str {
        "remote"
    }
    fn model(&self) -> String {
        "unknown".to_string()
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _session_id: Option<&str>,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Err(anyhow::anyhow!(
            "NullProvider cannot be used for completion"
        ))
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(NullProvider)
    }
}

impl App {
    const AUTO_RETRY_BASE_DELAY_SECS: u64 = 2;
    const AUTO_RETRY_MAX_ATTEMPTS: u8 = 3;
    const INPUT_UNDO_LIMIT: usize = 128;
    const CLIENT_FOCUS_RECORD_DEBOUNCE: Duration = Duration::from_secs(2);
    const KV_CACHE_OPTIMAL_OK_PCT: u8 = 85;
    const KV_CACHE_MIN_MISSED_TOKENS: u64 = 1_024;
    const KV_CACHE_MAX_MISS_SAMPLES: usize = 12;

    pub(super) fn begin_kv_cache_request(
        &mut self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
    ) {
        let turn_number = self
            .display_messages
            .iter()
            .filter(|message| message.role == "user")
            .count()
            .max(1);
        if self.kv_cache_turn_number == Some(turn_number) {
            self.kv_cache_turn_call_index = self.kv_cache_turn_call_index.saturating_add(1).max(1);
        } else {
            self.kv_cache_turn_number = Some(turn_number);
            self.kv_cache_turn_call_index = 1;
        }

        let baseline = self.kv_cache_baseline.clone();
        let signature = Self::kv_cache_request_signature(messages, tools, system_static);
        let baseline_messages_prefix_matches = baseline
            .as_ref()
            .and_then(|baseline| baseline.signature.as_ref())
            .map(|previous| Self::kv_cache_messages_prefix_matches(messages, previous));

        self.pending_kv_cache_request = Some(PendingKvCacheRequest {
            turn_number,
            call_index: self.kv_cache_turn_call_index,
            provider: self.kv_cache_provider_name(),
            model: self.kv_cache_provider_model(),
            upstream_provider: self.upstream_provider.clone(),
            signature: Some(signature),
            baseline_messages_prefix_matches,
            baseline,
        });
    }

    pub(super) fn record_completed_stream_cache_usage(&mut self) {
        let has_cache_telemetry = self.streaming_cache_read_tokens.is_some()
            || self.streaming_cache_creation_tokens.is_some();
        if self.streaming_input_tokens == 0 {
            return;
        }

        let optimal_input_tokens = self.cache_next_optimal_input_tokens;
        self.cache_next_optimal_input_tokens = Some(self.streaming_input_tokens);

        let request = self
            .pending_kv_cache_request
            .take()
            .unwrap_or_else(|| self.fallback_pending_kv_cache_request());

        self.record_kv_cache_miss_sample(&request);

        if !has_cache_telemetry {
            self.kv_cache_baseline = Some(KvCacheBaseline {
                input_tokens: self.streaming_input_tokens,
                completed_at: Instant::now(),
                provider: request.provider,
                model: request.model,
                upstream_provider: request.upstream_provider,
                signature: request.signature,
            });
            return;
        }

        self.total_cache_reported_input_tokens = self
            .total_cache_reported_input_tokens
            .saturating_add(self.streaming_input_tokens);
        if let Some(optimal) = optimal_input_tokens {
            self.total_cache_optimal_input_tokens = self
                .total_cache_optimal_input_tokens
                .saturating_add(optimal);
        }
        self.total_cache_read_tokens = self
            .total_cache_read_tokens
            .saturating_add(self.streaming_cache_read_tokens.unwrap_or(0));
        self.total_cache_creation_tokens = self
            .total_cache_creation_tokens
            .saturating_add(self.streaming_cache_creation_tokens.unwrap_or(0));
        self.last_cache_reported_input_tokens = Some(self.streaming_input_tokens);
        self.last_cache_read_tokens = Some(self.streaming_cache_read_tokens.unwrap_or(0));
        self.last_cache_optimal_input_tokens = optimal_input_tokens;

        self.log_kv_cache_usage_summary(&request, optimal_input_tokens);

        self.kv_cache_baseline = Some(KvCacheBaseline {
            input_tokens: self.streaming_input_tokens,
            completed_at: Instant::now(),
            provider: request.provider,
            model: request.model,
            upstream_provider: request.upstream_provider,
            signature: request.signature,
        });
    }

    fn log_kv_cache_usage_summary(
        &self,
        request: &PendingKvCacheRequest,
        optimal_input_tokens: Option<u64>,
    ) {
        let input_tokens = self.streaming_input_tokens;
        let read_tokens = self.streaming_cache_read_tokens.unwrap_or(0);
        let creation_tokens = self.streaming_cache_creation_tokens.unwrap_or(0);
        let read_pct = ratio_pct(read_tokens, input_tokens);
        let creation_pct = ratio_pct(creation_tokens, input_tokens);
        let optimal_read_pct = optimal_input_tokens.map(|optimal| ratio_pct(read_tokens, optimal));
        let session_read_pct = ratio_pct(
            self.total_cache_read_tokens,
            self.total_cache_reported_input_tokens,
        );
        let session_optimal_read_pct = if self.total_cache_optimal_input_tokens > 0 {
            Some(ratio_pct(
                self.total_cache_read_tokens,
                self.total_cache_optimal_input_tokens,
            ))
        } else {
            None
        };
        let miss = self
            .kv_cache_miss_samples
            .last()
            .filter(|sample| {
                sample.turn_number == request.turn_number && sample.call_index == request.call_index
            })
            .map(|sample| {
                format!(
                    "{}:{}",
                    sample.missed_tokens,
                    sample.reason.label().replace(' ', "_")
                )
            })
            .unwrap_or_else(|| {
                if request.baseline.is_none() {
                    "warmup:no_baseline".to_string()
                } else {
                    "none".to_string()
                }
            });
        let baseline_age_secs = request
            .baseline
            .as_ref()
            .map(|baseline| baseline.completed_at.elapsed().as_secs());
        let baseline_input_tokens = request
            .baseline
            .as_ref()
            .map(|baseline| baseline.input_tokens);
        let current_signature = request.signature.as_ref();
        let baseline_signature = request
            .baseline
            .as_ref()
            .and_then(|baseline| baseline.signature.as_ref());
        let system_static_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.system_static_hash != baseline.system_static_hash);
        let tools_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.tools_hash != baseline.tools_hash);
        let message_prefix_hash_changed = current_signature
            .zip(baseline_signature)
            .map(|(current, baseline)| current.messages_hash != baseline.messages_hash);
        let current_message_count = current_signature.map(|signature| signature.message_count);
        let baseline_message_count = baseline_signature.map(|signature| signature.message_count);
        let current_tool_count = current_signature.map(|signature| signature.tool_count);
        let baseline_tool_count = baseline_signature.map(|signature| signature.tool_count);

        crate::logging::info(&format!(
            "KV_CACHE_USAGE: turn={} call={} provider={} upstream={:?} model={} \
             input={} cache_read={} cache_write={} read_pct={} write_pct={} \
             optimal_input={:?} optimal_read_pct={:?} miss={} \
             session_input={} session_read={} session_write={} session_read_pct={} \
             session_optimal_input={} session_optimal_read_pct={:?} \
             baseline_input={:?} baseline_age_secs={:?} prefix_matches={:?} \
             system_changed={:?} tools_changed={:?} message_prefix_changed={:?} \
             message_count={:?} baseline_message_count={:?} tool_count={:?} baseline_tool_count={:?}",
            request.turn_number,
            request.call_index,
            request.provider,
            request.upstream_provider,
            request.model,
            input_tokens,
            read_tokens,
            creation_tokens,
            read_pct,
            creation_pct,
            optimal_input_tokens,
            optimal_read_pct,
            miss,
            self.total_cache_reported_input_tokens,
            self.total_cache_read_tokens,
            self.total_cache_creation_tokens,
            session_read_pct,
            self.total_cache_optimal_input_tokens,
            session_optimal_read_pct,
            baseline_input_tokens,
            baseline_age_secs,
            request.baseline_messages_prefix_matches,
            system_static_hash_changed,
            tools_hash_changed,
            message_prefix_hash_changed,
            current_message_count,
            baseline_message_count,
            current_tool_count,
            baseline_tool_count,
        ));
    }

    fn fallback_pending_kv_cache_request(&self) -> PendingKvCacheRequest {
        PendingKvCacheRequest {
            turn_number: self
                .display_messages
                .iter()
                .filter(|message| message.role == "user")
                .count()
                .max(1),
            call_index: 1,
            provider: self.kv_cache_provider_name(),
            model: self.kv_cache_provider_model(),
            upstream_provider: self.upstream_provider.clone(),
            signature: None,
            baseline_messages_prefix_matches: None,
            baseline: self.kv_cache_baseline.clone(),
        }
    }

    fn record_kv_cache_miss_sample(&mut self, request: &PendingKvCacheRequest) {
        let Some(baseline) = request.baseline.as_ref() else {
            return;
        };
        let expected_tokens = baseline.input_tokens;
        if expected_tokens == 0 {
            return;
        }

        let read_tokens = self.streaming_cache_read_tokens.unwrap_or(0);
        let missed_tokens = expected_tokens.saturating_sub(read_tokens);
        if missed_tokens < Self::KV_CACHE_MIN_MISSED_TOKENS {
            return;
        }

        let optimal_pct = ratio_pct(read_tokens, expected_tokens);
        let reason =
            self.classify_kv_cache_miss_reason(request, baseline, read_tokens, optimal_pct);
        if optimal_pct >= Self::KV_CACHE_OPTIMAL_OK_PCT
            && !matches!(
                reason,
                KvCacheMissReason::ProviderSwitch
                    | KvCacheMissReason::ModelSwitch
                    | KvCacheMissReason::UpstreamSwitch
                    | KvCacheMissReason::Expired
                    | KvCacheMissReason::HarnessSystemChanged
                    | KvCacheMissReason::HarnessToolsChanged
                    | KvCacheMissReason::HarnessPrefixChanged
            )
        {
            return;
        }

        self.kv_cache_miss_samples.push(KvCacheMissSample {
            turn_number: request.turn_number,
            call_index: request.call_index,
            missed_tokens,
            reason,
        });
        if self.kv_cache_miss_samples.len() > Self::KV_CACHE_MAX_MISS_SAMPLES {
            let overflow = self.kv_cache_miss_samples.len() - Self::KV_CACHE_MAX_MISS_SAMPLES;
            self.kv_cache_miss_samples.drain(0..overflow);
        }
    }

    fn classify_kv_cache_miss_reason(
        &self,
        request: &PendingKvCacheRequest,
        baseline: &KvCacheBaseline,
        read_tokens: u64,
        optimal_pct: u8,
    ) -> KvCacheMissReason {
        if baseline.provider != request.provider {
            return KvCacheMissReason::ProviderSwitch;
        }
        if baseline.model != request.model {
            return KvCacheMissReason::ModelSwitch;
        }
        if baseline.upstream_provider.is_some()
            && request.upstream_provider.is_some()
            && baseline.upstream_provider != request.upstream_provider
        {
            return KvCacheMissReason::UpstreamSwitch;
        }

        if let Some(ttl_secs) =
            crate::tui::cache_ttl_for_provider_model(&baseline.provider, Some(&baseline.model))
            && baseline.completed_at.elapsed() >= Duration::from_secs(ttl_secs)
        {
            return KvCacheMissReason::Expired;
        }

        if let (Some(previous), Some(current)) = (&baseline.signature, &request.signature) {
            if previous.system_static_hash != current.system_static_hash {
                return KvCacheMissReason::HarnessSystemChanged;
            }
            if previous.tools_hash != current.tools_hash
                || previous.tool_count != current.tool_count
            {
                return KvCacheMissReason::HarnessToolsChanged;
            }
        }

        if request.baseline_messages_prefix_matches == Some(false) {
            return KvCacheMissReason::HarnessPrefixChanged;
        }

        if self.streaming_cache_read_tokens.is_none() {
            return KvCacheMissReason::Unknown;
        }
        if read_tokens == 0 {
            return KvCacheMissReason::ZeroRead;
        }
        if optimal_pct < Self::KV_CACHE_OPTIMAL_OK_PCT {
            return KvCacheMissReason::LowRead;
        }
        KvCacheMissReason::Unknown
    }

    fn kv_cache_provider_name(&self) -> String {
        if self.is_remote || self.is_replay {
            self.remote_provider_name
                .clone()
                .unwrap_or_else(|| self.provider.name().to_string())
        } else {
            self.provider.name().to_string()
        }
    }

    fn kv_cache_provider_model(&self) -> String {
        if self.is_remote || self.is_replay {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| self.provider.model())
        } else {
            self.provider.model()
        }
    }

    fn kv_cache_request_signature(
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
    ) -> KvCacheRequestSignature {
        KvCacheRequestSignature {
            system_static_hash: stable_hash_str(system_static),
            tools_hash: stable_hash_json(tools),
            messages_hash: stable_hash_json(messages),
            message_count: messages.len(),
            tool_count: tools.len(),
        }
    }

    fn kv_cache_messages_prefix_matches(
        messages: &[Message],
        previous: &KvCacheRequestSignature,
    ) -> bool {
        if previous.message_count > messages.len() {
            return false;
        }
        stable_hash_json(&messages[..previous.message_count]) == previous.messages_hash
    }
}

fn stable_hash_str(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn stable_hash_json<T: serde::Serialize + ?Sized>(value: &T) -> u64 {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    stable_hash_str(&encoded)
}

fn ratio_pct(numerator: u64, denominator: u64) -> u8 {
    if denominator == 0 {
        0
    } else {
        ((numerator as f32 / denominator as f32) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }
}

#[cfg(test)]
mod tests;
