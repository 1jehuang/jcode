#![allow(dead_code)]

use super::keybind::{CenteredToggleKeys, ModelSwitchKeys, OptionalBinding, ScrollKeys};
use super::markdown::IncrementalMarkdownRenderer;
use super::stream_buffer::StreamBuffer;
use crate::bus::{Bus, BusEvent, LoginCompleted, ToolEvent, ToolStatus};
use crate::compaction::CompactionEvent;
use crate::config::config;
use crate::id;
use crate::mcp::McpManager;
use crate::message::{
    ContentBlock, Message, Role, StreamEvent, TOOL_OUTPUT_MISSING_TEXT, ToolCall,
};
use crate::provider::Provider;
use crate::session::Session;
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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;

mod auth;
mod commands;
mod conversation_state;
mod copy_selection;
mod debug;
mod dictation;
mod event_wrappers;
mod helpers;
mod input;
mod local;
mod misc_ui;
mod model_context;
mod navigation;
mod observe;
mod picker;
mod remote;
mod replay;
mod run_shell;
mod state_ui;
mod tui_lifecycle;
mod tui_state;
mod turn;

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
    Reconnecting { attempt: u32 },
}

impl RemoteStartupPhase {
    pub(crate) fn header_label(&self) -> String {
        match self {
            Self::StartingServer => "starting server…".to_string(),
            Self::Connecting => "connecting to server…".to_string(),
            Self::LoadingSession => "loading session…".to_string(),
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
    let mut notes = String::new();

    let tasks =
        crate::background::global().persisted_detached_running_tasks_for_session(session_id);
    if !tasks.is_empty() {
        let task_list = tasks
            .iter()
            .map(|task| format!("{} ({})", task.task_id, task.tool_name))
            .collect::<Vec<_>>()
            .join(", ");

        notes.push_str(&format!(
            "\nPersisted background task(s) for this session are still running: {}. Do not rerun those commands. Check them first with the `bg` tool (`bg action=\"list\"`, `bg action=\"status\" task_id=...`, or `bg action=\"output\" task_id=...`).",
            task_list
        ));
    }

    let pending_awaits = crate::server::pending_await_members_for_session(session_id);
    if !pending_awaits.is_empty() {
        let await_list = pending_awaits
            .iter()
            .map(|state| {
                let watch = if state.requested_ids.is_empty() {
                    "entire swarm".to_string()
                } else {
                    state.requested_ids.join(", ")
                };
                let remaining_secs = state.remaining_timeout().as_secs();
                format!(
                    "{} -> [{}], {}s remaining",
                    watch,
                    state.target_status.join(", "),
                    remaining_secs
                )
            })
            .collect::<Vec<_>>()
            .join("; ");

        notes.push_str(&format!(
            "\nPersisted `communicate await_members` wait(s) are still pending: {}. If you still need those coordination points after reload, rerun the same `communicate` call with action `await_members` to resume them with the remaining timeout instead of starting over.",
            await_list
        ));
    }

    notes
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
    pub(crate) fn alt_is_active(&self, now: Instant) -> bool {
        self.alt_active
            || self
                .alt_pulse_until
                .map(|expires_at| expires_at > now)
                .unwrap_or(false)
    }

    pub(crate) fn shift_is_active(&self, now: Instant) -> bool {
        self.alt_is_active(now)
            && (self.shift_active
                || self
                    .shift_pulse_until
                    .map(|expires_at| expires_at > now)
                    .unwrap_or(false))
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
    Run,
    Plan,
}

impl ImproveMode {
    pub(super) fn status_label(self) -> &'static str {
        match self {
            Self::Run => "active improvement loop",
            Self::Plan => "plan-only",
        }
    }
}

/// State for an in-progress OAuth/API-key login flow triggered by `/login`.

/// TUI Application state
pub struct App {
    provider: Arc<dyn Provider>,
    registry: Registry,
    skills: SkillRegistry,
    mcp_manager: Arc<RwLock<McpManager>>,
    messages: Vec<Message>,
    session: Session,
    display_messages: Vec<DisplayMessage>,
    display_messages_version: u64,
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
    // Total session token usage (accumulated across all turns)
    total_input_tokens: u64,
    total_output_tokens: u64,
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
    // Accurate TPS tracking: only counts actual token streaming time, not tool execution
    /// Set when first TextDelta arrives in a streaming response
    streaming_tps_start: Option<Instant>,
    /// Accumulated streaming-only time across agentic loop iterations
    streaming_tps_elapsed: Duration,
    /// Accumulated output tokens across all API calls in a turn.
    ///
    /// Providers may emit repeated cumulative usage snapshots for a single API call,
    /// so we accumulate per-call deltas to avoid double counting.
    streaming_total_output_tokens: u64,
    // Current status
    status: ProcessingStatus,
    // Subagent status (shown during Task tool execution)
    subagent_status: Option<String>,
    // Batch progress (shown during batch tool execution)
    batch_progress: Option<crate::bus::BatchProgress>,
    processing_started: Option<Instant>,
    // When the last API response completed (for cache TTL tracking)
    last_api_completed: Option<Instant>,
    // Input tokens from the last completed turn (for cache TTL display)
    last_turn_input_tokens: Option<u64>,
    // Pending turn to process (allows UI to redraw before processing starts)
    pending_turn: bool,
    // Local session file write to flush once the first "sending" frame is visible.
    session_save_pending: bool,
    // Tool calls detected during streaming (shown in real-time with details)
    streaming_tool_calls: Vec<ToolCall>,
    // Provider-specific session ID for conversation resume
    provider_session_id: Option<String>,
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
    // Interactive background client update is currently running
    background_update_in_progress: bool,
    // Reload the updated client once the current turn is idle
    pending_background_update_reload: Option<String>,
    // Restart: if set, exec into current binary with this session ID (no build)
    restart_requested: Option<String>,
    // Pasted content storage (displayed as placeholders, expanded on submit)
    pasted_contents: Vec<String>,
    // Pending pasted images (media_type, base64_data) attached to next message
    pending_images: Vec<(String, String)>,
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
    remote_provider_name: Option<String>,
    remote_provider_model: Option<String>,
    remote_startup_phase: Option<RemoteStartupPhase>,
    remote_startup_phase_started: Option<Instant>,
    remote_reasoning_effort: Option<String>,
    remote_service_tier: Option<String>,
    remote_transport: Option<String>,
    remote_compaction_mode: Option<crate::config::CompactionMode>,
    remote_available_models: Vec<String>,
    remote_model_routes: Vec<crate::provider::ModelRoute>,
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
    // Remember tool call ids that already have outputs
    tool_result_ids: HashSet<String>,
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
    last_side_panel_refresh: Option<Instant>,
    // Most recently focused side panel page, used to restore visibility when toggled off.
    last_side_panel_focus_id: Option<String>,
    // Pin read images to side pane
    pin_images: bool,
    // Show a native terminal scrollbar in the chat viewport.
    chat_native_scrollbar: bool,
    // Show a native terminal scrollbar in the side panel.
    side_panel_native_scrollbar: bool,
    // Interactive model/provider picker
    picker_state: Option<super::PickerState>,
    // Pending model switch from picker (for remote mode async processing)
    pending_model_switch: Option<String>,
    // Pending account switch from inline picker (for remote mode async processing)
    pending_account_picker_selection: Option<crate::tui::AccountPickerSelection>,
    // Keybindings for model switching
    model_switch_keys: ModelSwitchKeys,
    // Keybindings for effort switching
    effort_switch_keys: super::keybind::EffortSwitchKeys,
    // Keybindings for scrolling
    scroll_keys: ScrollKeys,
    // Keybinding for centered-mode toggle
    centered_toggle_keys: CenteredToggleKeys,
    // Optional configured keybinding for external dictation
    dictation_key: OptionalBinding,
    // Active external dictation session, if one is running
    dictation_session: Option<dictation::ActiveDictation>,
    // Whether an external dictation command is currently running
    dictation_in_flight: bool,
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
    // Message to interleave during processing (set via Ctrl+Enter in queue mode)
    interleave_message: Option<String>,
    // Message sent as soft interrupt but not yet injected (shown in queue preview until injected)
    pending_soft_interrupts: Vec<String>,
    // Whether the current remote turn should trigger autoreview after completion.
    autoreview_after_current_turn: bool,
    // Startup message to preload into the next spawned split window.
    pending_split_startup_message: Option<String>,
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
    /// Last mouse scroll event timestamp (for trackpad velocity detection)
    last_mouse_scroll: Option<Instant>,
    /// Scroll offset for changelog overlay (None = not visible)
    changelog_scroll: Option<usize>,
    help_scroll: Option<usize>,
    /// Session picker overlay (None = not visible)
    session_picker_overlay: Option<RefCell<super::session_picker::SessionPicker>>,
    /// Login picker overlay (None = not visible)
    login_picker_overlay: Option<RefCell<super::login_picker::LoginPicker>>,
    /// Account picker overlay (None = not visible)
    account_picker_overlay: Option<RefCell<super::account_picker::AccountPicker>>,
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
}

#[cfg(test)]
mod tests;
