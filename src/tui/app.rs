#![allow(dead_code)]

use super::keybind::{CenteredToggleKeys, ModelSwitchKeys, ScrollKeys};
use super::markdown::IncrementalMarkdownRenderer;
use super::stream_buffer::StreamBuffer;
use crate::bus::{BackgroundTaskStatus, Bus, BusEvent, LoginCompleted, ToolEvent, ToolStatus};
use crate::compaction::CompactionEvent;
use crate::config::config;
use crate::id;
use crate::mcp::McpManager;
use crate::message::{
    ContentBlock, Message, Role, StreamEvent, ToolCall, TOOL_OUTPUT_MISSING_TEXT,
};
use crate::provider::Provider;
use crate::session::Session;
use crate::skill::SkillRegistry;
use crate::tool::selfdev::ReloadContext;
use crate::tool::{Registry, ToolContext};
use anyhow::Result;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use futures::StreamExt;
use ratatui::{layout::Rect, DefaultTerminal};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;

#[derive(Debug, Clone)]
struct PendingRemoteMessage {
    content: String,
    images: Vec<(String, String)>,
    is_system: bool,
}

const MEMORY_INJECTION_SUPPRESSION_SECS: u64 = 90;

#[cfg(target_os = "macos")]
fn ctrl_bracket_fallback_to_esc(code: &mut KeyCode, modifiers: &mut KeyModifiers) {
    if !modifiers.contains(KeyModifiers::CONTROL) {
        return;
    }
    match code {
        KeyCode::Esc => {
            *code = KeyCode::Char('[');
        }
        KeyCode::Char('5') => {
            // Legacy tty mapping for Ctrl+]
            *code = KeyCode::Char(']');
        }
        _ => {}
    }
}

#[cfg(not(target_os = "macos"))]
fn ctrl_bracket_fallback_to_esc(_code: &mut KeyCode, _modifiers: &mut KeyModifiers) {}

/// Debug command file path
fn debug_cmd_path() -> PathBuf {
    if let Ok(path) = std::env::var("JCODE_DEBUG_CMD_PATH") {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join("jcode_debug_cmd")
}

/// Debug response file path
fn debug_response_path() -> PathBuf {
    if let Ok(path) = std::env::var("JCODE_DEBUG_RESPONSE_PATH") {
        return PathBuf::from(path);
    }
    std::env::temp_dir().join("jcode_debug_response")
}

/// Parse rate limit reset time from error message
/// Returns the Duration until rate limit resets, if this is a rate limit error
fn parse_rate_limit_error(error: &str) -> Option<Duration> {
    let error_lower = error.to_lowercase();

    // Check if this is a rate limit error
    if !error_lower.contains("rate limit")
        && !error_lower.contains("rate_limit")
        && !error_lower.contains("429")
        && !error_lower.contains("too many requests")
        && !error_lower.contains("hit your limit")
    {
        return None;
    }

    // Try to extract time from common patterns

    // Pattern: "retry after X seconds" or "retry in X seconds"
    if let Some(idx) = error_lower.find("retry") {
        let after = &error_lower[idx..];
        for word in after.split_whitespace() {
            if let Ok(secs) = word
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
            {
                if secs > 0 && secs < 86400 {
                    return Some(Duration::from_secs(secs));
                }
            }
        }
    }

    // Pattern: "resets Xam" or "resets Xpm" (clock time like "resets 5am")
    if let Some(idx) = error_lower.find("resets") {
        let after = &error_lower[idx..];
        for word in after.split_whitespace() {
            let word = word.trim_matches(|c: char| c == '·' || c == ' ');
            // Check for time like "5am", "12pm", "5:30am"
            if word.ends_with("am") || word.ends_with("pm") {
                if let Some(duration) = parse_clock_time_to_duration(word) {
                    return Some(duration);
                }
            }
        }
    }

    // Pattern: "reset in X seconds"
    if let Some(idx) = error_lower.find("reset") {
        let after = &error_lower[idx..];
        for word in after.split_whitespace() {
            if let Ok(secs) = word
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
            {
                if secs > 0 && secs < 86400 {
                    return Some(Duration::from_secs(secs));
                }
            }
        }
    }

    // No default - only auto-retry if we know the actual reset time
    None
}

fn is_context_limit_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("context length")
        || lower.contains("context window")
        || lower.contains("maximum context")
        || lower.contains("max context")
        || lower.contains("token limit")
        || lower.contains("too many tokens")
        || lower.contains("prompt is too long")
        || lower.contains("input is too long")
        || lower.contains("request too large")
        || lower.contains("length limit")
        || lower.contains("maximum tokens")
        || (lower.contains("exceeded") && lower.contains("tokens"))
}

/// Parse a clock time like "5am" or "12:30pm" and return duration until that time
fn parse_clock_time_to_duration(time_str: &str) -> Option<Duration> {
    let time_lower = time_str.to_lowercase();
    let is_pm = time_lower.ends_with("pm");
    let time_part = time_lower.trim_end_matches("am").trim_end_matches("pm");

    // Parse hour (and optional minutes)
    let (hour, minute) = if time_part.contains(':') {
        let parts: Vec<&str> = time_part.split(':').collect();
        if parts.len() != 2 {
            return None;
        }
        let h: u32 = parts[0].parse().ok()?;
        let m: u32 = parts[1].parse().ok()?;
        (h, m)
    } else {
        let h: u32 = time_part.parse().ok()?;
        (h, 0)
    };

    // Convert to 24-hour format
    let hour_24 = if is_pm && hour != 12 {
        hour + 12
    } else if !is_pm && hour == 12 {
        0
    } else {
        hour
    };

    if hour_24 >= 24 || minute >= 60 {
        return None;
    }

    // Get current time and calculate duration until target time
    let now = chrono::Local::now();
    let today = now.date_naive();

    // Try today first, then tomorrow if the time has passed
    let target_time = chrono::NaiveTime::from_hms_opt(hour_24, minute, 0)?;
    let mut target_datetime = today.and_time(target_time);

    // If target time is in the past, use tomorrow
    if target_datetime <= now.naive_local() {
        target_datetime = (today + chrono::Duration::days(1)).and_time(target_time);
    }

    let duration_secs = (target_datetime - now.naive_local()).num_seconds();
    if duration_secs > 0 {
        Some(Duration::from_secs(duration_secs as u64))
    } else {
        None
    }
}

fn format_cache_footer(read_tokens: Option<u64>, write_tokens: Option<u64>) -> Option<String> {
    let _ = (read_tokens, write_tokens);
    None
}

/// Format token count for display (e.g., 63000 -> "63K")
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}k", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

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

/// Result from running the TUI
#[derive(Debug, Default)]
pub struct RunResult {
    /// Session ID to reload (hot-reload, no rebuild)
    pub reload_session: Option<String>,
    /// Session ID to rebuild (full git pull + cargo build + tests)
    pub rebuild_session: Option<String>,
    /// Session ID to update (download from GitHub releases and reload)
    pub update_session: Option<String>,
    /// Exit code to use (for canary wrapper communication)
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendAction {
    Submit,
    Queue,
    Interleave,
}

#[derive(Debug, Clone, Serialize)]
struct DebugSnapshot {
    state: serde_json::Value,
    frame: Option<crate::tui::visual_debug::FrameCapture>,
    recent_messages: Vec<DebugMessage>,
    queued_messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DebugMessage {
    role: String,
    content: String,
    tool_calls: Vec<String>,
    duration_secs: Option<f32>,
    title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DebugAssertion {
    field: String,
    op: String,
    value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
struct DebugAssertResult {
    ok: bool,
    field: String,
    op: String,
    expected: serde_json::Value,
    actual: serde_json::Value,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct DebugStepResult {
    step: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DebugScript {
    steps: Vec<String>,
    assertions: Vec<DebugAssertion>,
    wait_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct DebugRunReport {
    ok: bool,
    steps: Vec<DebugStepResult>,
    assertions: Vec<DebugAssertResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct ScrollTestConfig {
    width: Option<u16>,
    height: Option<u16>,
    step: Option<usize>,
    max_steps: Option<usize>,
    padding: Option<usize>,
    diagrams: Option<usize>,
    include_frames: Option<bool>,
    include_paused: Option<bool>,
    diagram: Option<String>,
    diagram_mode: Option<crate::config::DiagramDisplayMode>,
    expect_inline: Option<bool>,
    expect_pane: Option<bool>,
    expect_widget: Option<bool>,
    require_no_anomalies: Option<bool>,
}

#[derive(Debug, Clone)]
struct ScrollTestExpectations {
    expect_inline: bool,
    expect_pane: bool,
    expect_widget: bool,
    require_no_anomalies: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ScrollSuiteConfig {
    widths: Option<Vec<u16>>,
    heights: Option<Vec<u16>>,
    diagram_modes: Option<Vec<crate::config::DiagramDisplayMode>>,
    diagrams: Option<usize>,
    step: Option<usize>,
    max_steps: Option<usize>,
    padding: Option<usize>,
    include_frames: Option<bool>,
    include_paused: Option<bool>,
    diagram: Option<String>,
    require_no_anomalies: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct DebugEvent {
    at_ms: u64,
    kind: String,
    detail: String,
}

struct DebugTrace {
    enabled: bool,
    started_at: Instant,
    events: Vec<DebugEvent>,
}

impl DebugTrace {
    fn new() -> Self {
        Self {
            enabled: false,
            started_at: Instant::now(),
            events: Vec::new(),
        }
    }

    fn record(&mut self, kind: &str, detail: String) {
        if !self.enabled {
            return;
        }
        let at_ms = self.started_at.elapsed().as_millis() as u64;
        self.events.push(DebugEvent {
            at_ms,
            kind: kind.to_string(),
            detail,
        });
    }
}

#[derive(Clone)]
struct ScrollTestState {
    display_messages: Vec<DisplayMessage>,
    display_messages_version: u64,
    scroll_offset: usize,
    auto_scroll_paused: bool,
    is_processing: bool,
    streaming_text: String,
    queued_messages: Vec<String>,
    interleave_message: Option<String>,
    pending_soft_interrupts: Vec<String>,
    input: String,
    cursor_pos: usize,
    status: ProcessingStatus,
    processing_started: Option<Instant>,
    status_notice: Option<(String, Instant)>,
    diagram_mode: crate::config::DiagramDisplayMode,
    diagram_focus: bool,
    diagram_index: usize,
    diagram_scroll_x: i32,
    diagram_scroll_y: i32,
    diagram_pane_ratio: u8,
    diagram_pane_ratio_from: u8,
    diagram_pane_ratio_target: u8,
    diagram_pane_anim_start: Option<Instant>,
    diagram_pane_enabled: bool,
    diagram_pane_position: crate::config::DiagramPanePosition,
    diagram_zoom: u8,
}

fn rect_from_capture(rect: super::visual_debug::RectCapture) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn rect_contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x.saturating_add(inner.width) <= outer.x.saturating_add(outer.width)
        && inner.y.saturating_add(inner.height) <= outer.y.saturating_add(outer.height)
}

fn point_in_rect(col: u16, row: u16, rect: Rect) -> bool {
    col >= rect.x
        && row >= rect.y
        && col < rect.x.saturating_add(rect.width)
        && row < rect.y.saturating_add(rect.height)
}

fn parse_area_spec(spec: &str) -> Option<Rect> {
    let mut parts = spec.split('+');
    let size = parts.next()?;
    let x = parts.next()?;
    let y = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (w, h) = size.split_once('x')?;
    Some(Rect {
        width: w.parse::<u16>().ok()?,
        height: h.parse::<u16>().ok()?,
        x: x.parse::<u16>().ok()?,
        y: y.parse::<u16>().ok()?,
    })
}

/// State for an in-progress OAuth/API-key login flow triggered by `/login`.
#[derive(Debug, Clone)]
enum PendingLogin {
    /// Waiting for user to paste Claude OAuth code (verifier needed for token exchange)
    Claude {
        verifier: String,
        redirect_uri: Option<String>,
    },
    /// Waiting for user to paste Claude OAuth code for a specific named account
    ClaudeAccount {
        verifier: String,
        label: String,
        redirect_uri: Option<String>,
    },
    /// Waiting for user to paste OpenRouter API key
    OpenRouter,
    /// GitHub Copilot device flow in progress (polling in background)
    Copilot,
    /// Interactive provider selection (user picks a number)
    ProviderSelection,
}

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
    processing_started: Option<Instant>,
    // Pending turn to process (allows UI to redraw before processing starts)
    pending_turn: bool,
    // Tool calls detected during streaming (shown in real-time with details)
    streaming_tool_calls: Vec<ToolCall>,
    // Provider-specific session ID for conversation resume
    provider_session_id: Option<String>,
    // Cancel flag for interrupting generation
    cancel_requested: bool,
    // Quit confirmation: tracks when first Ctrl+C was pressed
    quit_pending: Option<Instant>,
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
    // Pasted content storage (displayed as placeholders, expanded on submit)
    pasted_contents: Vec<String>,
    // Pending pasted images (media_type, base64_data) attached to next message
    pending_images: Vec<(String, String)>,
    // Debug socket broadcast channel (if enabled)
    debug_tx: Option<tokio::sync::broadcast::Sender<super::backend::DebugEvent>>,
    // Remote provider info (set when running in remote mode)
    remote_provider_name: Option<String>,
    remote_provider_model: Option<String>,
    remote_available_models: Vec<String>,
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
    // Remote server short name (e.g., "running", "blazing")
    remote_server_short_name: Option<String>,
    // Remote server icon (e.g., "🔥", "🌫️")
    remote_server_icon: Option<String>,
    // Current message request ID (for remote mode - to match Done events)
    current_message_id: Option<u64>,
    // Whether running in remote mode
    is_remote: bool,
    // Whether running in replay mode (readonly playback of a saved session)
    pub is_replay: bool,
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
    // Scroll offset for pinned diff pane
    diff_pane_scroll: usize,
    diff_pane_focus: bool,
    diff_pane_auto_scroll: bool,
    // Pin read images to side pane
    pin_images: bool,
    // Interactive model/provider picker
    picker_state: Option<super::PickerState>,
    // Pending model switch from picker (for remote mode async processing)
    pending_model_switch: Option<String>,
    // Keybindings for model switching
    model_switch_keys: ModelSwitchKeys,
    // Keybindings for effort switching
    effort_switch_keys: super::keybind::EffortSwitchKeys,
    // Keybindings for scrolling
    scroll_keys: ScrollKeys,
    // Keybinding for centered-mode toggle
    centered_toggle_keys: CenteredToggleKeys,
    // Scroll bookmark: stashed scroll position for quick teleport back
    scroll_bookmark: Option<usize>,
    // Short-lived notice for status feedback (model switch, cycle diff mode, etc.)
    status_notice: Option<(String, Instant)>,
    // Message to interleave during processing (set via Shift+Enter)
    interleave_message: Option<String>,
    // Message sent as soft interrupt but not yet injected (shown in queue preview until injected)
    pending_soft_interrupts: Vec<String>,
    // Queue mode: if true, Enter during processing queues; if false, Enter queues to send next
    // Toggle with Ctrl+Tab or Ctrl+T
    queue_mode: bool,
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
    /// Scroll offset for changelog overlay (None = not visible)
    changelog_scroll: Option<usize>,
}

impl ScrollTestState {
    fn capture(app: &App) -> Self {
        Self {
            display_messages: app.display_messages.clone(),
            display_messages_version: app.display_messages_version,
            scroll_offset: app.scroll_offset,
            auto_scroll_paused: app.auto_scroll_paused,
            is_processing: app.is_processing,
            streaming_text: app.streaming_text.clone(),
            queued_messages: app.queued_messages.clone(),
            interleave_message: app.interleave_message.clone(),
            pending_soft_interrupts: app.pending_soft_interrupts.clone(),
            input: app.input.clone(),
            cursor_pos: app.cursor_pos,
            status: app.status.clone(),
            processing_started: app.processing_started,
            status_notice: app.status_notice.clone(),
            diagram_mode: app.diagram_mode,
            diagram_focus: app.diagram_focus,
            diagram_index: app.diagram_index,
            diagram_scroll_x: app.diagram_scroll_x,
            diagram_scroll_y: app.diagram_scroll_y,
            diagram_pane_ratio: app.diagram_pane_ratio,
            diagram_pane_ratio_from: app.diagram_pane_ratio_from,
            diagram_pane_ratio_target: app.diagram_pane_ratio_target,
            diagram_pane_anim_start: app.diagram_pane_anim_start,
            diagram_pane_enabled: app.diagram_pane_enabled,
            diagram_pane_position: app.diagram_pane_position,
            diagram_zoom: app.diagram_zoom,
        }
    }

    fn restore(self, app: &mut App) {
        app.display_messages = self.display_messages;
        app.display_messages_version = self.display_messages_version;
        app.scroll_offset = self.scroll_offset;
        app.auto_scroll_paused = self.auto_scroll_paused;
        app.is_processing = self.is_processing;
        app.streaming_text = self.streaming_text;
        app.queued_messages = self.queued_messages;
        app.interleave_message = self.interleave_message;
        app.pending_soft_interrupts = self.pending_soft_interrupts;
        app.input = self.input;
        app.cursor_pos = self.cursor_pos;
        app.status = self.status;
        app.processing_started = self.processing_started;
        app.status_notice = self.status_notice;
        app.diagram_mode = self.diagram_mode;
        app.diagram_focus = self.diagram_focus;
        app.diagram_index = self.diagram_index;
        app.diagram_scroll_x = self.diagram_scroll_x;
        app.diagram_scroll_y = self.diagram_scroll_y;
        app.diagram_pane_ratio = self.diagram_pane_ratio;
        app.diagram_pane_ratio_from = self.diagram_pane_ratio_from;
        app.diagram_pane_ratio_target = self.diagram_pane_ratio_target;
        app.diagram_pane_anim_start = self.diagram_pane_anim_start;
        app.diagram_pane_enabled = self.diagram_pane_enabled;
        app.diagram_pane_position = self.diagram_pane_position;
        app.diagram_zoom = self.diagram_zoom;
    }
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
    async fn begin_remote_send(
        &mut self,
        remote: &mut super::backend::RemoteConnection,
        content: String,
        images: Vec<(String, String)>,
        is_system: bool,
    ) -> Result<u64> {
        let msg_id = remote
            .send_message_with_images(content.clone(), images.clone())
            .await?;
        self.current_message_id = Some(msg_id);
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.processing_started = Some(Instant::now());
        self.last_stream_activity = Some(Instant::now());
        self.streaming_tps_start = None;
        self.streaming_tps_elapsed = Duration::ZERO;
        self.streaming_total_output_tokens = 0;
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.rate_limit_pending_message = Some(PendingRemoteMessage {
            content,
            images,
            is_system,
        });
        remote.reset_call_output_tokens_seen();
        Ok(msg_id)
    }

    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let skills = SkillRegistry::load().unwrap_or_default();
        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
        let mut session = Session::create(None, None);
        session.model = Some(provider.model());
        let display = config().display.clone();
        let features = config().features.clone();
        let context_limit = provider.context_window() as u64;

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let provider_clone = Arc::clone(&provider);
            handle.spawn(async move {
                let _ = provider_clone.prefetch_models().await;
            });
        }

        // Pre-compute context info so it shows on startup
        let available_skills: Vec<crate::prompt::SkillInfo> = skills
            .list()
            .iter()
            .map(|s| crate::prompt::SkillInfo {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect();
        let (_, context_info) = crate::prompt::build_system_prompt_with_context(
            None,
            &available_skills,
            session.is_canary,
        );

        Self {
            provider,
            registry,
            skills,
            mcp_manager,
            messages: Vec::new(),
            session,
            display_messages: Vec::new(),
            display_messages_version: 0,
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            auto_scroll_paused: false,
            active_skill: None,
            is_processing: false,
            streaming_text: String::new(),
            should_quit: false,
            queued_messages: Vec::new(),
            streaming_input_tokens: 0,
            streaming_output_tokens: 0,
            streaming_cache_read_tokens: None,
            streaming_cache_creation_tokens: None,
            upstream_provider: None,
            connection_type: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost: 0.0,
            cached_prompt_price: None,
            cached_completion_price: None,
            context_limit,
            context_warning_shown: false,
            context_info,
            last_stream_activity: None,
            streaming_tps_start: None,
            streaming_tps_elapsed: Duration::ZERO,
            streaming_total_output_tokens: 0,
            status: ProcessingStatus::default(),
            subagent_status: None,
            processing_started: None,
            pending_turn: false,
            streaming_tool_calls: Vec::new(),
            provider_session_id: None,
            cancel_requested: false,
            quit_pending: None,
            mcp_server_names: Vec::new(), // Vec<(name, tool_count)>
            stream_buffer: StreamBuffer::new(),
            thinking_start: None,
            thought_line_inserted: false,
            thinking_buffer: String::new(),
            thinking_prefix_emitted: false,
            reload_requested: None,
            rebuild_requested: None,
            update_requested: None,
            pasted_contents: Vec::new(),
            pending_images: Vec::new(),
            debug_tx: None,
            remote_provider_name: None,
            remote_provider_model: None,
            remote_available_models: Vec::new(),
            remote_mcp_servers: Vec::new(),
            remote_skills: Vec::new(),
            remote_total_tokens: None,
            remote_is_canary: None,
            remote_server_version: None,
            remote_server_has_update: None,
            remote_server_short_name: None,
            remote_server_icon: None,
            current_message_id: None,
            is_remote: false,
            is_replay: false,
            replay_elapsed_override: None,
            replay_processing_started_ms: None,
            tool_result_ids: HashSet::new(),
            remote_session_id: None,
            remote_sessions: Vec::new(),
            remote_swarm_members: Vec::new(),
            swarm_plan_items: Vec::new(),
            swarm_plan_version: None,
            swarm_plan_swarm_id: None,
            known_stable_version: crate::build::read_stable_version().ok().flatten(),
            last_version_check: Some(Instant::now()),
            pending_migration: None,
            remote_client_count: None,
            resume_session_id: None,
            requested_exit_code: None,
            memory_enabled: features.memory,
            last_injected_memory_signature: None,
            swarm_enabled: features.swarm,
            diff_mode: display.diff_mode,
            centered: display.centered,
            diagram_mode: display.diagram_mode,
            diagram_focus: false,
            diagram_index: 0,
            diagram_scroll_x: 0,
            diagram_scroll_y: 0,
            diagram_pane_ratio: 40,
            diagram_pane_ratio_from: 40,
            diagram_pane_ratio_target: 40,
            diagram_pane_anim_start: None,
            diagram_pane_enabled: true,
            diagram_pane_position: crate::config::DiagramPanePosition::default(),
            diagram_zoom: 100,
            diff_pane_scroll: 0,
            diff_pane_focus: false,
            diff_pane_auto_scroll: true,
            pin_images: display.pin_images,
            picker_state: None,
            pending_model_switch: None,
            model_switch_keys: super::keybind::load_model_switch_keys(),
            effort_switch_keys: super::keybind::load_effort_switch_keys(),
            centered_toggle_keys: super::keybind::load_centered_toggle_key(),
            scroll_keys: super::keybind::load_scroll_keys(),
            scroll_bookmark: None,
            status_notice: None,
            interleave_message: None,
            pending_soft_interrupts: Vec::new(),
            queue_mode: display.queue_mode,
            tab_completion_state: None,
            app_started: Instant::now(),
            client_binary_mtime: std::env::current_exe()
                .ok()
                .and_then(|p| std::fs::metadata(&p).ok())
                .and_then(|m| m.modified().ok()),
            rate_limit_reset: None,
            rate_limit_pending_message: None,
            last_stream_error: None,
            reload_info: Vec::new(),
            debug_trace: DebugTrace::new(),
            streaming_md_renderer: RefCell::new(IncrementalMarkdownRenderer::new(None)),
            ambient_system_prompt: None,
            pending_login: None,
            changelog_scroll: None,
        }
    }

    /// Configure ambient mode: override system prompt and queue an initial message.
    pub fn set_ambient_mode(&mut self, system_prompt: String, initial_message: String) {
        self.ambient_system_prompt = Some(system_prompt);
        crate::tool::ambient::register_ambient_session(self.session.id.clone());
        self.queued_messages.push(initial_message);
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.processing_started = Some(Instant::now());
        self.pending_turn = true;
    }

    /// Queue a startup message that should be auto-sent when the TUI starts.
    pub fn queue_startup_message(&mut self, message: String) {
        if message.trim().is_empty() {
            return;
        }
        self.queued_messages.push(message);
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.processing_started = Some(Instant::now());
        self.pending_turn = true;
    }

    /// Create an App instance for remote mode (connecting to server)
    pub fn new_for_remote(resume_session: Option<String>) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(NullProvider);
        let registry = Registry::empty();
        let mut app = Self::new(provider, registry);
        app.is_remote = true;

        // Load session to get canary status (for "client self-dev" badge)
        if let Some(ref session_id) = resume_session {
            if let Ok(session) = Session::load(session_id) {
                app.session = session;
            }
        }

        app.resume_session_id = resume_session;
        app
    }

    /// Create an App instance for replay mode (playing back a saved session)
    pub fn new_for_replay(session: crate::session::Session) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(NullProvider);
        let registry = Registry::empty();
        let mut app = Self::new(provider, registry);
        app.is_remote = false;
        app.is_replay = true;
        let model_name = session.model.clone().unwrap_or_default();
        let session_name = session.short_name.clone().unwrap_or_default();

        // Set provider/model info so status widgets show correct values
        let effective_model = if model_name.is_empty() {
            // Try to infer model from message content (e.g., usage events)
            // Default to a sensible value for demo purposes
            "claude-sonnet-4-20250514".to_string()
        } else {
            model_name
        };
        app.remote_provider_model = Some(effective_model.clone());
        // Infer provider name from model string
        let provider_name = if effective_model.contains("claude")
            || effective_model.contains("opus")
            || effective_model.contains("sonnet")
            || effective_model.contains("haiku")
        {
            "anthropic"
        } else if effective_model.contains("gpt")
            || effective_model.contains("o1")
            || effective_model.contains("o3")
            || effective_model.contains("o4")
        {
            "openai"
        } else if effective_model.contains('/') {
            "openrouter"
        } else {
            "claude"
        };
        app.remote_provider_name = Some(provider_name.to_string());

        app.session = session;
        if !session_name.is_empty() {
            let icon = crate::id::session_icon(&session_name);
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::SetTitle(format!("{} replay: {}", icon, session_name))
            );
        }
        app
    }

    /// Get the current session ID
    pub fn session_id(&self) -> &str {
        &self.session.id
    }

    fn update_terminal_title(&self) {
        let session_id = if self.is_remote {
            self.remote_session_id
                .as_deref()
                .unwrap_or(&self.session.id)
        } else {
            &self.session.id
        };
        let session_name = crate::id::extract_session_name(session_id)
            .map(|s| s.to_string())
            .unwrap_or_else(|| session_id.to_string());
        let session_icon = crate::id::session_icon(&session_name);
        let is_canary = if self.is_remote {
            self.remote_is_canary.unwrap_or(false)
        } else {
            self.session.is_canary
        };
        let suffix = if is_canary { " [self-dev]" } else { "" };
        let server_name = self.remote_server_short_name.as_deref().unwrap_or("jcode");
        let server_icon = self.remote_server_icon.as_deref().unwrap_or("");
        let icons = if server_icon.is_empty() {
            session_icon.to_string()
        } else {
            format!("{}{}", server_icon, session_icon)
        };
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(format!(
                "{} {} {}{}",
                super::ui::capitalize(server_name),
                super::ui::capitalize(&session_name),
                icons,
                suffix
            ))
        );
    }

    fn reconnect_target_session_id(&self) -> Option<String> {
        self.remote_session_id
            .clone()
            .or_else(|| self.resume_session_id.clone())
    }

    /// Check if the selected reload candidate is newer than startup.
    /// Candidate selection matches `/reload` so the `cli↑` badge and reload target stay aligned.
    fn has_newer_binary(&self) -> bool {
        let Some(startup_mtime) = self.client_binary_mtime else {
            return false;
        };

        let is_selfdev_session = if self.is_remote {
            self.remote_is_canary.unwrap_or(false)
        } else {
            self.session.is_canary
        };

        let Some((candidate, _label)) = crate::build::client_update_candidate(is_selfdev_session)
        else {
            return false;
        };

        std::fs::metadata(&candidate)
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|mtime| mtime > startup_mtime)
    }

    /// Initialize MCP servers (call after construction)
    pub async fn init_mcp(&mut self) {
        // Always register the MCP management tool so agent can connect servers
        let mcp_tool = crate::tool::mcp::McpManagementTool::new(Arc::clone(&self.mcp_manager))
            .with_registry(self.registry.clone());
        self.registry
            .register("mcp".to_string(), Arc::new(mcp_tool))
            .await;

        let manager = self.mcp_manager.read().await;
        let server_count = manager.config().servers.len();
        if server_count > 0 {
            drop(manager);

            // Log configured servers
            crate::logging::info(&format!("MCP: Found {} server(s) in config", server_count));

            let (successes, failures) = {
                let manager = self.mcp_manager.write().await;
                let result = manager.connect_all().await.unwrap_or((0, Vec::new()));
                // Cache server names with tool counts
                let servers = manager.connected_servers().await;
                let all_tools = manager.all_tools().await;
                self.mcp_server_names = servers
                    .into_iter()
                    .map(|name| {
                        let count = all_tools.iter().filter(|(s, _)| s == &name).count();
                        (name, count)
                    })
                    .collect();
                result
            };

            // Show connection results
            if successes > 0 {
                let msg = format!("MCP: Connected to {} server(s)", successes);
                crate::logging::info(&msg);
                self.set_status_notice(&format!("mcp: {} connected", successes));
            }

            if !failures.is_empty() {
                for (name, error) in &failures {
                    let msg = format!("MCP '{}' failed: {}", name, error);
                    self.push_display_message(DisplayMessage::error(msg));
                }
                if successes == 0 {
                    self.set_status_notice("MCP: all connections failed");
                }
            }

            // Register MCP server tools
            let tools = crate::mcp::create_mcp_tools(Arc::clone(&self.mcp_manager)).await;
            for (name, tool) in tools {
                self.registry.register(name, tool).await;
            }
        }

        // Register self-dev tools if this is a canary session
        if self.session.is_canary {
            self.registry.register_selfdev_tools().await;
        }
    }

    /// Restore a previous session (for hot-reload)
    pub fn restore_session(&mut self, session_id: &str) {
        if let Ok(session) = Session::load(session_id) {
            // Count stats before restoring
            let mut user_turns = 0;
            let mut assistant_turns = 0;
            let mut total_chars = 0;

            // Convert session messages to display messages (including tools)
            for item in crate::session::render_messages(&session) {
                if item.role == "user" {
                    user_turns += 1;
                } else if item.role == "assistant" {
                    assistant_turns += 1;
                }
                total_chars += item.content.len();

                self.push_display_message(DisplayMessage {
                    role: item.role,
                    content: item.content,
                    tool_calls: item.tool_calls,
                    duration_secs: None,
                    title: None,
                    tool_data: item.tool_data,
                });
            }

            // Don't restore provider_session_id - Claude sessions don't persist across
            // process restarts. The messages are restored, so Claude will get full context.
            self.provider_session_id = None;
            self.session = session;
            self.replace_provider_messages(self.session.messages_for_provider());
            // Clear the saved provider_session_id since it's no longer valid
            self.session.provider_session_id = None;
            let mut restored_model = false;
            if let Some(model) = self.session.model.clone() {
                if let Err(e) = self.provider.set_model(&model) {
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!("⚠ Failed to restore model '{}': {}", model, e),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                } else {
                    restored_model = true;
                }
            }

            let active_model = self.provider.model();
            if restored_model || self.session.model.is_none() {
                self.session.model = Some(active_model.clone());
            }
            self.update_context_limit_for_model(&active_model);
            // Mark session as active now that it's being used again
            self.session.mark_active();
            crate::logging::info(&format!("Restored session: {}", session_id));

            // Build stats message
            let total_turns = user_turns + assistant_turns;
            let estimated_tokens = total_chars / 4; // Rough estimate: ~4 chars per token
            let stats = if total_turns > 0 {
                format!(
                    " ({} turns, ~{}k tokens)",
                    total_turns,
                    estimated_tokens / 1000
                )
            } else {
                String::new()
            };

            // Check for reload info to show what triggered the reload
            let reload_info = if let Ok(jcode_dir) = crate::storage::jcode_dir() {
                let info_path = jcode_dir.join("reload-info");
                if info_path.exists() {
                    let info = std::fs::read_to_string(&info_path).ok();
                    let _ = std::fs::remove_file(&info_path); // Clean up
                    info
                } else {
                    None
                }
            } else {
                None
            };

            // Build the reload message based on what triggered it
            // Extract build hash for the AI notification
            let is_reload = reload_info.is_some();
            let (message, build_hash) = if let Some(info) = reload_info {
                if let Some(hash) = info.strip_prefix("reload:") {
                    let h = hash.trim().to_string();
                    (
                        format!("✓ Reloaded with build {}. Session restored{}", h, stats),
                        h,
                    )
                } else if let Some(hash) = info.strip_prefix("rebuild:") {
                    let h = hash.trim().to_string();
                    (
                        format!("✓ Rebuilt and reloaded ({}). Session restored{}", h, stats),
                        h,
                    )
                } else {
                    (
                        format!("✓ JCode reloaded. Session restored{}", stats),
                        "unknown".to_string(),
                    )
                }
            } else {
                (
                    format!("✓ JCode reloaded. Session restored{}", stats),
                    "unknown".to_string(),
                )
            };

            // Add success message with stats (only if there's actual content or a reload happened)
            if total_turns > 0 || is_reload {
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: message,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }

            // Queue an automatic message to notify the AI that reload completed
            // Only do this for the session that actually initiated the reload
            // (identified by matching ReloadContext). Other sessions that are
            // restored after a server restart were idle and shouldn't be prompted.
            let reload_ctx = ReloadContext::load_for_session(session_id).ok().flatten();
            if let Some(ctx) = reload_ctx {
                let task_info = ctx
                    .task_context
                    .map(|t| format!("\nTask context: {}", t))
                    .unwrap_or_default();

                let continuation_msg = format!(
                    "[SYSTEM: Reload succeeded. Build {} → {}.{}\nSession restored with {} turns.\nIMPORTANT: The reload is done. You MUST immediately continue your work. Do NOT ask the user what to do next. Do NOT summarize what happened. Just pick up exactly where you left off and keep going.]",
                    ctx.version_before,
                    ctx.version_after,
                    task_info,
                    total_turns
                );

                crate::logging::info(&format!(
                    "Queuing reload continuation message ({} chars)",
                    continuation_msg.len()
                ));
                self.queued_messages.push(continuation_msg);
                // Trigger processing so the queued message gets sent to the LLM.
                // Without this, the local event loop waits for user input since
                // process_queued_messages only runs inside process_turn_with_input.
                self.is_processing = true;
                self.status = ProcessingStatus::Sending;
                self.processing_started = Some(Instant::now());
                self.pending_turn = true;
            }
        } else {
            crate::logging::error(&format!("Failed to restore session: {}", session_id));

            // Check if this was a reload that failed - inject failure message if so
            if let Ok(Some(ctx)) = ReloadContext::load_for_session(session_id) {
                let task_info = ctx
                    .task_context
                    .map(|t| format!(" You were working on: {}", t))
                    .unwrap_or_default();

                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!(
                        "⚠ Reload failed. Session could not be restored. Previous version: {}, Target version: {}.{}\n\
                         Starting fresh session. You may need to re-examine your changes.",
                        ctx.version_before,
                        ctx.version_after,
                        task_info
                    ),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
        }
    }

    /// Check for and process debug commands from file
    /// Commands: "message:<text>", "reload", "state", "quit"
    fn scroll_max_estimate(&self) -> usize {
        let renderer_max = super::ui::last_max_scroll();
        if renderer_max > 0 {
            renderer_max
        } else {
            self.display_messages
                .len()
                .saturating_mul(100)
                .saturating_add(self.streaming_text.len())
        }
    }

    fn diagram_available(&self) -> bool {
        self.diagram_mode == crate::config::DiagramDisplayMode::Pinned
            && self.diagram_pane_enabled
            && !crate::tui::mermaid::get_active_diagrams().is_empty()
    }

    fn normalize_diagram_state(&mut self) {
        if self.diagram_mode != crate::config::DiagramDisplayMode::Pinned {
            self.diagram_focus = false;
            self.diagram_index = 0;
            self.diagram_scroll_x = 0;
            self.diagram_scroll_y = 0;
            return;
        }
        if !self.diagram_pane_enabled {
            self.diagram_focus = false;
        }

        let diagram_count = crate::tui::mermaid::get_active_diagrams().len();
        if diagram_count == 0 {
            self.diagram_focus = false;
            self.diagram_index = 0;
            self.diagram_scroll_x = 0;
            self.diagram_scroll_y = 0;
            return;
        }

        if self.diagram_index >= diagram_count {
            self.diagram_index = 0;
            self.diagram_scroll_x = 0;
            self.diagram_scroll_y = 0;
        }
    }

    fn set_diagram_focus(&mut self, focus: bool) {
        if self.diagram_focus == focus {
            return;
        }
        self.diagram_focus = focus;
        self.diff_pane_focus = false;
        if focus {
            self.set_status_notice("Focus: diagram (hjkl pan, [/] zoom, +/- resize)");
        } else {
            self.set_status_notice("Focus: chat");
        }
    }

    fn diff_pane_visible(&self) -> bool {
        self.diff_mode.is_pinned()
    }

    fn set_diff_pane_focus(&mut self, focus: bool) {
        if self.diff_pane_focus == focus {
            return;
        }
        self.diff_pane_focus = focus;
        self.diagram_focus = false;
        if focus {
            self.set_status_notice("Focus: diffs (j/k scroll, Esc to return)");
        } else {
            self.set_status_notice("Focus: chat");
        }
    }

    fn handle_diff_pane_focus_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        if !self.diff_pane_focus || modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }

        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.diff_pane_scroll = self.diff_pane_scroll.saturating_add(3);
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.diff_pane_scroll = self.diff_pane_scroll.saturating_sub(3);
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('d') | KeyCode::PageDown => {
                self.diff_pane_scroll = self.diff_pane_scroll.saturating_add(20);
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('u') | KeyCode::PageUp => {
                self.diff_pane_scroll = self.diff_pane_scroll.saturating_sub(20);
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.diff_pane_scroll = 0;
                self.diff_pane_auto_scroll = false;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.diff_pane_scroll = usize::MAX;
                self.diff_pane_auto_scroll = true;
            }
            KeyCode::Esc => {
                self.set_diff_pane_focus(false);
            }
            _ => {}
        }

        true
    }

    fn cycle_diagram(&mut self, direction: i32) {
        let diagrams = crate::tui::mermaid::get_active_diagrams();
        let count = diagrams.len();
        if count == 0 {
            return;
        }
        let current = self.diagram_index.min(count - 1);
        let next = if direction < 0 {
            if current == 0 {
                count - 1
            } else {
                current - 1
            }
        } else {
            if current + 1 >= count {
                0
            } else {
                current + 1
            }
        };
        self.diagram_index = next;
        self.diagram_scroll_x = 0;
        self.diagram_scroll_y = 0;
        self.set_status_notice(format!("Diagram {}/{}", next + 1, count));
    }

    fn pan_diagram(&mut self, dx: i32, dy: i32) {
        self.diagram_scroll_x = (self.diagram_scroll_x + dx).max(0);
        self.diagram_scroll_y = (self.diagram_scroll_y + dy).max(0);
    }

    const DIAGRAM_PANE_ANIM_DURATION: f32 = 0.15;

    fn animated_diagram_pane_ratio(&self) -> u8 {
        let Some(start) = self.diagram_pane_anim_start else {
            return self.diagram_pane_ratio_target;
        };
        let elapsed = start.elapsed().as_secs_f32();
        let t = (elapsed / Self::DIAGRAM_PANE_ANIM_DURATION).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t); // smoothstep
        let from = self.diagram_pane_ratio_from as f32;
        let to = self.diagram_pane_ratio_target as f32;
        (from + (to - from) * t).round() as u8
    }

    fn adjust_diagram_pane_ratio(&mut self, delta: i8) {
        let (min_ratio, max_ratio) = match self.diagram_pane_position {
            crate::config::DiagramPanePosition::Side => (25i16, 70i16),
            crate::config::DiagramPanePosition::Top => (20i16, 60i16),
        };
        let current_target = self.diagram_pane_ratio_target;
        let next = (current_target as i16 + delta as i16).clamp(min_ratio, max_ratio) as u8;
        if next != current_target {
            self.diagram_pane_ratio_from = self.animated_diagram_pane_ratio();
            self.diagram_pane_ratio_target = next;
            self.diagram_pane_anim_start = Some(Instant::now());
            self.set_status_notice(format!("Diagram pane: {}%", next));
        }
    }

    fn adjust_diagram_zoom(&mut self, delta: i8) {
        let next = (self.diagram_zoom as i16 + delta as i16).clamp(50, 200) as u8;
        if next != self.diagram_zoom {
            self.diagram_zoom = next;
            self.set_status_notice(format!("Diagram zoom: {}%", next));
        }
    }

    fn toggle_diagram_pane(&mut self) {
        if self.diagram_mode != crate::config::DiagramDisplayMode::Pinned {
            self.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        }
        super::markdown::set_diagram_mode_override(Some(self.diagram_mode));
        self.diagram_pane_enabled = !self.diagram_pane_enabled;
        if !self.diagram_pane_enabled {
            self.diagram_focus = false;
        }
        let status = if self.diagram_pane_enabled {
            "Diagram pane: ON"
        } else {
            "Diagram pane: OFF"
        };
        self.set_status_notice(status);
    }

    fn toggle_diagram_pane_position(&mut self) {
        use crate::config::DiagramPanePosition;
        self.diagram_pane_position = match self.diagram_pane_position {
            DiagramPanePosition::Side => DiagramPanePosition::Top,
            DiagramPanePosition::Top => DiagramPanePosition::Side,
        };
        let (min_ratio, max_ratio) = match self.diagram_pane_position {
            DiagramPanePosition::Side => (25u8, 70u8),
            DiagramPanePosition::Top => (20u8, 60u8),
        };
        self.diagram_pane_ratio_target = self.diagram_pane_ratio_target.clamp(min_ratio, max_ratio);
        self.diagram_pane_anim_start = None;
        let label = match self.diagram_pane_position {
            DiagramPanePosition::Side => "side",
            DiagramPanePosition::Top => "top",
        };
        self.set_status_notice(format!("Diagram pane: {}", label));
    }

    fn pop_out_diagram(&mut self) {
        let diagrams = super::mermaid::get_active_diagrams();
        let total = diagrams.len();
        if total == 0 {
            self.set_status_notice("No diagrams to open");
            return;
        }
        let index = self.diagram_index.min(total - 1);
        let diagram = &diagrams[index];
        if let Some(path) = super::mermaid::get_cached_path(diagram.hash) {
            if path.exists() {
                match open::that_detached(&path) {
                    Ok(_) => self.set_status_notice(format!(
                        "Opened diagram {}/{} in viewer",
                        index + 1,
                        total
                    )),
                    Err(e) => self.set_status_notice(format!("Failed to open: {}", e)),
                }
            } else {
                self.set_status_notice("Diagram image not found on disk");
            }
        } else {
            self.set_status_notice("Diagram not cached");
        }
    }

    fn handle_diagram_ctrl_key(&mut self, code: KeyCode, diagram_available: bool) -> bool {
        if diagram_available {
            match code {
                KeyCode::Left => {
                    self.cycle_diagram(-1);
                    return true;
                }
                KeyCode::Right => {
                    self.cycle_diagram(1);
                    return true;
                }
                KeyCode::Char('h') => {
                    self.set_diagram_focus(false);
                    return true;
                }
                KeyCode::Char('l') => {
                    self.set_diagram_focus(true);
                    return true;
                }
                _ => {}
            }
        }
        if self.diff_pane_visible() {
            match code {
                KeyCode::Char('l') => {
                    self.set_diff_pane_focus(true);
                    return true;
                }
                KeyCode::Char('h') => {
                    self.set_diff_pane_focus(false);
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    fn ctrl_prompt_rank(code: &KeyCode, modifiers: KeyModifiers) -> Option<usize> {
        if !modifiers.contains(KeyModifiers::CONTROL)
            || modifiers.contains(KeyModifiers::ALT)
            || modifiers.contains(KeyModifiers::SHIFT)
        {
            return None;
        }
        match code {
            KeyCode::Char(c) if c.is_ascii_digit() && *c != '0' => Some((*c as u8 - b'0') as usize),
            _ => None,
        }
    }

    fn jump_diagram(&mut self, index: usize) {
        let total = crate::tui::mermaid::get_active_diagrams().len();
        if total == 0 {
            return;
        }
        let target = index.min(total - 1);
        self.diagram_index = target;
        self.diagram_scroll_x = 0;
        self.diagram_scroll_y = 0;
        self.set_status_notice(format!("Pinned {}/{}", target + 1, total));
    }

    fn handle_diagram_focus_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        diagram_available: bool,
    ) -> bool {
        if !diagram_available || !self.diagram_focus || modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }

        match code {
            KeyCode::Char('h') | KeyCode::Left => self.pan_diagram(-4, 0),
            KeyCode::Char('l') | KeyCode::Right => self.pan_diagram(4, 0),
            KeyCode::Char('k') | KeyCode::Up => self.pan_diagram(0, -3),
            KeyCode::Char('j') | KeyCode::Down => self.pan_diagram(0, 3),
            KeyCode::Char('+') | KeyCode::Char('=') => self.adjust_diagram_pane_ratio(5),
            KeyCode::Char('-') | KeyCode::Char('_') => self.adjust_diagram_pane_ratio(-5),
            KeyCode::Char(']') => self.adjust_diagram_zoom(10),
            KeyCode::Char('[') => self.adjust_diagram_zoom(-10),
            KeyCode::Char('o') => self.pop_out_diagram(),
            KeyCode::Esc => {
                self.set_diagram_focus(false);
            }
            _ => {}
        }

        true
    }

    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        self.normalize_diagram_state();
        let diagram_available = self.diagram_available();
        let layout = super::ui::last_layout_snapshot();
        let mut over_diagram = false;
        let mut over_diff_pane = false;
        if let Some(layout) = layout {
            if let Some(diagram_area) = layout.diagram_area {
                over_diagram = point_in_rect(mouse.column, mouse.row, diagram_area);
            }
            if let Some(diff_area) = layout.diff_pane_area {
                over_diff_pane = point_in_rect(mouse.column, mouse.row, diff_area);
            }
            if diagram_available && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                if over_diagram {
                    self.set_diagram_focus(true);
                } else {
                    self.set_diagram_focus(false);
                }
            }
        }

        let mut handled_scroll = false;
        if diagram_available
            && over_diagram
            && matches!(
                mouse.kind,
                MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::ScrollLeft
                    | MouseEventKind::ScrollRight
            )
        {
            if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.adjust_diagram_zoom(10),
                    MouseEventKind::ScrollDown => self.adjust_diagram_zoom(-10),
                    _ => {}
                }
                self.set_diagram_focus(true);
                handled_scroll = true;
            } else if self.diagram_focus {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.pan_diagram(0, -1),
                    MouseEventKind::ScrollDown => self.pan_diagram(0, 1),
                    MouseEventKind::ScrollLeft => self.pan_diagram(-1, 0),
                    MouseEventKind::ScrollRight => self.pan_diagram(1, 0),
                    _ => {}
                }
                handled_scroll = true;
            }
        }

        if !handled_scroll
            && over_diff_pane
            && self.diff_pane_visible()
            && matches!(
                mouse.kind,
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
            )
        {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.diff_pane_scroll = self.diff_pane_scroll.saturating_sub(3);
                    self.diff_pane_auto_scroll = false;
                }
                MouseEventKind::ScrollDown => {
                    self.diff_pane_scroll = self.diff_pane_scroll.saturating_add(3);
                }
                _ => {}
            }
            handled_scroll = true;
        }

        if handled_scroll {
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_up(3);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down(3);
            }
            _ => {}
        }
    }

    fn scroll_up(&mut self, amount: usize) {
        let max_scroll = super::ui::last_max_scroll();
        let max = if max_scroll > 0 {
            max_scroll
        } else {
            self.scroll_max_estimate()
        };
        if !self.auto_scroll_paused {
            let current_abs = max.saturating_sub(self.scroll_offset);
            self.scroll_offset = current_abs.saturating_sub(amount);
        } else {
            self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        }
        self.auto_scroll_paused = true;
    }

    fn scroll_down(&mut self, amount: usize) {
        if !self.auto_scroll_paused {
            return;
        }
        let max_scroll = super::ui::last_max_scroll();
        let max = if max_scroll > 0 {
            max_scroll
        } else {
            self.scroll_max_estimate()
        };
        self.scroll_offset = (self.scroll_offset + amount).min(max);
        if self.scroll_offset >= max {
            self.follow_chat_bottom();
        }
    }

    /// Resume follow mode and keep the viewport pinned to the latest content.
    fn follow_chat_bottom(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll_paused = false;
    }

    fn debug_scroll_up(&mut self, amount: usize) {
        self.scroll_up(amount);
    }

    fn debug_scroll_down(&mut self, amount: usize) {
        self.scroll_down(amount);
    }

    fn debug_scroll_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll_paused = true;
    }

    fn debug_scroll_bottom(&mut self) {
        self.follow_chat_bottom();
    }

    fn build_scroll_test_content(
        diagrams: usize,
        padding: usize,
        override_diagram: Option<&str>,
    ) -> String {
        let mut out = String::new();
        let intro_lines = padding.max(4);
        for i in 0..intro_lines {
            out.push_str(&format!(
                "Intro line {:02} - quick brown fox jumps over the lazy dog.\n",
                i + 1
            ));
        }

        let diagram_templates = [
            r#"flowchart TD
    A[Start] --> B{Decision}
    B -->|Yes| C[Process 1]
    B -->|No| D[Process 2]
    C --> E[Merge]
    D --> E
    E --> F[End]"#,
            r#"sequenceDiagram
    participant U as User
    participant A as App
    participant S as Service
    U->>A: Scroll request
    A->>S: Render diagram
    S-->>A: PNG
    A-->>U: Draw frame"#,
            r#"stateDiagram-v2
    [*] --> Idle
    Idle --> Scrolling: input
    Scrolling --> Rendering: diagram
    Rendering --> Idle: frame drawn"#,
        ];

        for idx in 0..diagrams {
            let diagram =
                override_diagram.unwrap_or(diagram_templates[idx % diagram_templates.len()]);
            out.push_str("```mermaid\n");
            out.push_str(diagram);
            out.push_str("\n```\n");

            for j in 0..padding {
                out.push_str(&format!(
                    "After diagram {} line {:02} - stretch content for scrolling.\n",
                    idx + 1,
                    j + 1
                ));
            }
        }

        out
    }

    fn capture_scroll_test_step(
        &mut self,
        terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
        label: &str,
        mode: &str,
        scroll_offset: usize,
        max_scroll: usize,
        include_frames: bool,
        expectations: &ScrollTestExpectations,
    ) -> Result<serde_json::Value, String> {
        self.scroll_offset = scroll_offset;
        self.auto_scroll_paused = mode == "paused";
        if let Err(e) = terminal.draw(|f| crate::tui::ui::draw(f, self)) {
            return Err(format!("draw error ({}): {}", label, e));
        }

        let frame = super::visual_debug::latest_frame();
        let (frame_id, anomalies, image_regions, normalized_frame) = match frame {
            Some(ref frame) => {
                let normalized = if include_frames {
                    Some(super::visual_debug::normalize_frame(frame))
                } else {
                    None
                };
                (
                    Some(frame.frame_id),
                    frame.anomalies.clone(),
                    frame.image_regions.clone(),
                    normalized,
                )
            }
            None => (None, Vec::new(), Vec::new(), None),
        };

        let user_scroll = scroll_offset.min(max_scroll);
        let scroll_top = if self.auto_scroll_paused && user_scroll > 0 {
            user_scroll
        } else {
            max_scroll
        };

        let mermaid_stats = crate::tui::mermaid::debug_stats_json();
        let mermaid_state = serde_json::to_value(crate::tui::mermaid::debug_image_state()).ok();
        let active_diagrams = crate::tui::mermaid::get_active_diagrams();

        let (diagram_area_capture, diagram_widget_present, diagram_mode_label) = match frame {
            Some(ref frame) => {
                let widget_present = frame
                    .info_widgets
                    .as_ref()
                    .map(|info| info.placements.iter().any(|p| p.kind == "diagrams"))
                    .unwrap_or(false);
                let mode = frame
                    .state
                    .diagram_mode
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", self.diagram_mode));
                (frame.layout.diagram_area, widget_present, mode)
            }
            None => (None, false, format!("{:?}", self.diagram_mode)),
        };

        let diagram_area_rect = diagram_area_capture.map(rect_from_capture);
        let diagram_area_json = diagram_area_capture.map(|rect| {
            serde_json::json!({
                "x": rect.x,
                "y": rect.y,
                "width": rect.width,
                "height": rect.height,
            })
        });

        let mut diagram_rendered_in_pane = false;
        if let (Some(area), Some(state)) = (
            diagram_area_rect,
            mermaid_state.as_ref().and_then(|v| v.as_array()),
        ) {
            for entry in state {
                let last_area = entry
                    .get("last_area")
                    .and_then(|v| v.as_str())
                    .and_then(parse_area_spec);
                if let Some(render_area) = last_area {
                    if rect_contains(area, render_area) {
                        diagram_rendered_in_pane = true;
                        break;
                    }
                }
            }
        }

        let active_hashes: Vec<String> = active_diagrams
            .iter()
            .map(|d| format!("{:016x}", d.hash))
            .collect();
        let inline_placeholders = image_regions.len();

        let mut problems: Vec<String> = Vec::new();
        if expectations.require_no_anomalies && !anomalies.is_empty() {
            problems.push(format!("anomalies: {}", anomalies.join("; ")));
        }
        if expectations.expect_pane {
            if diagram_area_rect.is_none() {
                problems.push("missing pinned diagram area".to_string());
            }
            if active_hashes.is_empty() {
                problems.push("no active diagrams registered".to_string());
            }
            if !diagram_rendered_in_pane {
                problems.push("diagram not rendered in pinned pane".to_string());
            }
        }
        if expectations.expect_inline {
            if inline_placeholders == 0 {
                problems.push("expected inline diagram placeholders but none found".to_string());
            }
        } else if inline_placeholders > 0 {
            problems.push("unexpected inline diagram placeholders".to_string());
        }
        if expectations.expect_widget && !diagram_widget_present {
            problems.push("expected diagram widget but none present".to_string());
        }

        let checks_ok = problems.is_empty();

        Ok(serde_json::json!({
            "label": label,
            "mode": mode,
            "scroll_offset": scroll_offset,
            "scroll_top": scroll_top,
            "max_scroll": max_scroll,
            "frame_id": frame_id,
            "anomalies": anomalies,
            "image_regions": image_regions,
            "mermaid_stats": mermaid_stats,
            "mermaid_state": mermaid_state,
            "diagram": {
                "mode": diagram_mode_label,
                "area": diagram_area_json,
                "active_diagrams": active_hashes,
                "widget_present": diagram_widget_present,
                "inline_placeholders": inline_placeholders,
                "rendered_in_pane": diagram_rendered_in_pane,
            },
            "checks": {
                "ok": checks_ok,
                "problems": problems,
                "expectations": {
                    "expect_inline": expectations.expect_inline,
                    "expect_pane": expectations.expect_pane,
                    "expect_widget": expectations.expect_widget,
                    "require_no_anomalies": expectations.require_no_anomalies,
                }
            },
            "frame": normalized_frame,
        }))
    }

    fn run_scroll_test(&mut self, raw: Option<&str>) -> String {
        let cfg: ScrollTestConfig = if let Some(raw) = raw {
            if raw.trim().is_empty() {
                ScrollTestConfig {
                    width: None,
                    height: None,
                    step: None,
                    max_steps: None,
                    padding: None,
                    diagrams: None,
                    include_frames: None,
                    include_paused: None,
                    diagram: None,
                    diagram_mode: None,
                    expect_inline: None,
                    expect_pane: None,
                    expect_widget: None,
                    require_no_anomalies: None,
                }
            } else {
                match serde_json::from_str(raw) {
                    Ok(cfg) => cfg,
                    Err(e) => return format!("scroll-test parse error: {}", e),
                }
            }
        } else {
            ScrollTestConfig {
                width: None,
                height: None,
                step: None,
                max_steps: None,
                padding: None,
                diagrams: None,
                include_frames: None,
                include_paused: None,
                diagram: None,
                diagram_mode: None,
                expect_inline: None,
                expect_pane: None,
                expect_widget: None,
                require_no_anomalies: None,
            }
        };

        let diagram_mode = cfg.diagram_mode.unwrap_or(self.diagram_mode);
        let expectations = ScrollTestExpectations {
            expect_inline: cfg
                .expect_inline
                .unwrap_or(diagram_mode != crate::config::DiagramDisplayMode::Pinned),
            expect_pane: cfg
                .expect_pane
                .unwrap_or(diagram_mode == crate::config::DiagramDisplayMode::Pinned),
            expect_widget: cfg.expect_widget.unwrap_or(false),
            require_no_anomalies: cfg.require_no_anomalies.unwrap_or(true),
        };

        let width = cfg.width.unwrap_or(100).max(40);
        let height = cfg.height.unwrap_or(40).max(20);
        let step = cfg.step.unwrap_or(5).max(1);
        let max_steps = cfg.max_steps.unwrap_or(16).max(4).min(100);
        let padding = cfg.padding.unwrap_or(12).max(4);
        let diagrams = cfg.diagrams.unwrap_or(2).clamp(1, 3);
        let include_frames = cfg.include_frames.unwrap_or(true);
        let include_paused = cfg.include_paused.unwrap_or(true);
        let diagram_override = cfg.diagram.as_deref();

        let saved_state = ScrollTestState::capture(self);
        let saved_diagram_override = super::markdown::get_diagram_mode_override();
        let saved_active_diagrams = crate::tui::mermaid::snapshot_active_diagrams();
        let was_visual_debug = super::visual_debug::is_enabled();
        super::visual_debug::enable();

        self.diagram_mode = diagram_mode;
        super::markdown::set_diagram_mode_override(Some(diagram_mode));

        let test_content = Self::build_scroll_test_content(diagrams, padding, diagram_override);
        self.display_messages = vec![
            DisplayMessage {
                role: "user".to_string(),
                content: "Scroll test: render mermaid + text".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
            DisplayMessage {
                role: "assistant".to_string(),
                content: test_content,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
        ];
        self.bump_display_messages_version();
        self.follow_chat_bottom();
        self.is_processing = false;
        self.clear_streaming_render_state();
        self.queued_messages.clear();
        self.interleave_message = None;
        self.pending_soft_interrupts.clear();
        self.status = ProcessingStatus::Idle;
        self.processing_started = None;
        self.status_notice = None;

        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut errors: Vec<String> = Vec::new();
        let mut steps: Vec<serde_json::Value> = Vec::new();

        let backend = TestBackend::new(width, height);
        let mut terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(e) => {
                saved_state.restore(self);
                super::markdown::set_diagram_mode_override(saved_diagram_override);
                crate::tui::mermaid::restore_active_diagrams(saved_active_diagrams);
                if !was_visual_debug {
                    super::visual_debug::disable();
                }
                return format!("scroll-test terminal error: {}", e);
            }
        };

        // Baseline render (bottom) for metrics
        self.follow_chat_bottom();
        if let Err(e) = terminal.draw(|f| crate::tui::ui::draw(f, self)) {
            errors.push(format!("baseline draw error: {}", e));
        }

        // Derive scroll positions using the latest frame
        let baseline_frame = super::visual_debug::latest_frame();
        let (visible_height, total_lines, image_regions) = if let Some(frame) = baseline_frame {
            let visible_height = frame
                .layout
                .messages_area
                .map(|r| r.height as usize)
                .unwrap_or(height as usize);
            let total_lines = frame.layout.estimated_content_height.max(1);
            (visible_height, total_lines, frame.image_regions)
        } else {
            (height as usize, 1usize, Vec::new())
        };

        let max_scroll = total_lines.saturating_sub(visible_height);

        let mut positions: Vec<(String, usize)> = Vec::new();
        positions.push(("bottom".to_string(), max_scroll));
        positions.push(("middle".to_string(), max_scroll / 2));
        positions.push(("top".to_string(), 0));

        for (idx, region) in image_regions.iter().enumerate() {
            let img_top = region.abs_line_idx;
            let img_bottom = region.abs_line_idx + region.height as usize;
            positions.push((format!("image{}_top", idx + 1), img_top));
            positions.push((
                format!("image{}_bottom", idx + 1),
                img_bottom.saturating_sub(visible_height),
            ));
            positions.push((format!("image{}_off_top", idx + 1), img_bottom));
            if img_top > 0 {
                positions.push((format!("image{}_pre", idx + 1), img_top.saturating_sub(2)));
            }
        }

        if max_scroll > 0 {
            let mut cursor = 0usize;
            while cursor <= max_scroll && positions.len() < max_steps {
                positions.push((format!("step_{}", cursor), cursor));
                cursor = cursor.saturating_add(step);
                if cursor == 0 {
                    break;
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        let mut ordered: Vec<(String, usize)> = Vec::new();
        for (label, scroll_top) in positions {
            let clamped = scroll_top.min(max_scroll);
            if seen.insert(clamped) {
                ordered.push((label, clamped));
            }
        }

        if ordered.len() > max_steps {
            ordered.truncate(max_steps);
        }

        for (label, scroll_top) in &ordered {
            let offset = max_scroll.saturating_sub(*scroll_top);
            match self.capture_scroll_test_step(
                &mut terminal,
                label,
                "normal",
                offset,
                max_scroll,
                include_frames,
                &expectations,
            ) {
                Ok(step) => steps.push(step),
                Err(e) => errors.push(e),
            }
        }

        if include_paused {
            for (label, scroll_top) in &ordered {
                let offset = (*scroll_top).min(max_scroll);
                let paused_label = format!("{}_paused", label);
                match self.capture_scroll_test_step(
                    &mut terminal,
                    &paused_label,
                    "paused",
                    offset,
                    max_scroll,
                    include_frames,
                    &expectations,
                ) {
                    Ok(step) => steps.push(step),
                    Err(e) => errors.push(e),
                }
            }
        }

        let mermaid_scroll_sim =
            serde_json::to_value(crate::tui::mermaid::debug_test_scroll(None)).ok();

        let mut step_failures: Vec<String> = Vec::new();
        for step in &steps {
            let checks = step.get("checks");
            let ok = checks
                .and_then(|c| c.get("ok"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !ok {
                let label = step.get("label").and_then(|v| v.as_str()).unwrap_or("step");
                let problems = checks
                    .and_then(|c| c.get("problems"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_else(|| "unknown failure".to_string());
                step_failures.push(format!("{}: {}", label, problems));
            }
        }

        let report = serde_json::json!({
            "ok": errors.is_empty() && step_failures.is_empty(),
            "config": {
                "width": width,
                "height": height,
                "step": step,
                "max_steps": max_steps,
                "padding": padding,
                "diagrams": diagrams,
                "include_frames": include_frames,
                "include_paused": include_paused,
                "diagram_override": diagram_override,
                "diagram_mode": format!("{:?}", diagram_mode),
                "expectations": {
                    "expect_inline": expectations.expect_inline,
                    "expect_pane": expectations.expect_pane,
                    "expect_widget": expectations.expect_widget,
                    "require_no_anomalies": expectations.require_no_anomalies,
                },
            },
            "layout": {
                "total_lines": total_lines,
                "visible_height": visible_height,
                "max_scroll": max_scroll,
            },
            "steps": steps,
            "mermaid_scroll_sim": mermaid_scroll_sim,
            "errors": errors,
            "problems": step_failures,
        });

        saved_state.restore(self);
        super::markdown::set_diagram_mode_override(saved_diagram_override);
        crate::tui::mermaid::restore_active_diagrams(saved_active_diagrams);
        if !was_visual_debug {
            super::visual_debug::disable();
        }

        serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
    }

    fn run_scroll_suite(&mut self, raw: Option<&str>) -> String {
        let cfg: ScrollSuiteConfig = if let Some(raw) = raw {
            if raw.trim().is_empty() {
                ScrollSuiteConfig {
                    widths: None,
                    heights: None,
                    diagram_modes: None,
                    diagrams: None,
                    step: None,
                    max_steps: None,
                    padding: None,
                    include_frames: None,
                    include_paused: None,
                    diagram: None,
                    require_no_anomalies: None,
                }
            } else {
                match serde_json::from_str(raw) {
                    Ok(cfg) => cfg,
                    Err(e) => return format!("scroll-suite parse error: {}", e),
                }
            }
        } else {
            ScrollSuiteConfig {
                widths: None,
                heights: None,
                diagram_modes: None,
                diagrams: None,
                step: None,
                max_steps: None,
                padding: None,
                include_frames: None,
                include_paused: None,
                diagram: None,
                require_no_anomalies: None,
            }
        };

        let widths = cfg.widths.unwrap_or_else(|| vec![80, 100, 120]);
        let heights = cfg.heights.unwrap_or_else(|| vec![24, 40]);
        let diagram_modes = cfg.diagram_modes.unwrap_or_else(|| vec![self.diagram_mode]);
        let diagrams = cfg.diagrams.unwrap_or(2).clamp(1, 3);
        let step = cfg.step.unwrap_or(5).max(1);
        let max_steps = cfg.max_steps.unwrap_or(12).max(4).min(100);
        let padding = cfg.padding.unwrap_or(12).max(4);
        let include_frames = cfg.include_frames.unwrap_or(false);
        let include_paused = cfg.include_paused.unwrap_or(true);
        let diagram_override = cfg.diagram.as_deref();
        let require_no_anomalies = cfg.require_no_anomalies.unwrap_or(true);

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        let mut total = 0usize;
        let max_cases = 12usize;

        for mode in &diagram_modes {
            for width in &widths {
                for height in &heights {
                    if total >= max_cases {
                        break;
                    }
                    total += 1;
                    let mode_str = match mode {
                        crate::config::DiagramDisplayMode::None => "none",
                        crate::config::DiagramDisplayMode::Margin => "margin",
                        crate::config::DiagramDisplayMode::Pinned => "pinned",
                    };
                    let case_label = format!("{}x{}_{}", width, height, mode_str);
                    let cfg_json = serde_json::json!({
                        "width": width,
                        "height": height,
                        "step": step,
                        "max_steps": max_steps,
                        "padding": padding,
                        "diagrams": diagrams,
                        "include_frames": include_frames,
                        "include_paused": include_paused,
                        "diagram": diagram_override,
                        "diagram_mode": mode_str,
                        "require_no_anomalies": require_no_anomalies,
                    });
                    let cfg_str = cfg_json.to_string();
                    let report_str = self.run_scroll_test(Some(&cfg_str));
                    let report_value: serde_json::Value = serde_json::from_str(&report_str)
                        .unwrap_or_else(
                            |_| serde_json::json!({"ok": false, "error": "invalid report json"}),
                        );
                    let ok = report_value
                        .get("ok")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if !ok {
                        failures.push(case_label.clone());
                    }
                    results.push(serde_json::json!({
                        "name": case_label,
                        "config": cfg_json,
                        "report": report_value,
                    }));
                }
                if total >= max_cases {
                    break;
                }
            }
            if total >= max_cases {
                break;
            }
        }

        let report = serde_json::json!({
            "ok": failures.is_empty(),
            "summary": {
                "total": total,
                "failed": failures.len(),
                "failures": failures,
                "max_cases": max_cases,
            },
            "cases": results,
        });

        serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
    }

    fn handle_debug_command(&mut self, cmd: &str) -> String {
        let cmd = cmd.trim();
        if cmd == "frame" {
            return self.handle_debug_command("screen-json");
        }
        if cmd == "frame-normalized" {
            return self.handle_debug_command("screen-json-normalized");
        }
        if cmd == "enable" || cmd == "debug-enable" {
            super::visual_debug::enable();
            return "Visual debugging enabled.".to_string();
        }
        if cmd == "disable" || cmd == "debug-disable" {
            super::visual_debug::disable();
            return "Visual debugging disabled.".to_string();
        }
        if cmd == "status" {
            let enabled = super::visual_debug::is_enabled();
            let overlay = super::visual_debug::overlay_enabled();
            return serde_json::json!({
                "visual_debug_enabled": enabled,
                "visual_debug_overlay": overlay
            })
            .to_string();
        }
        if cmd == "overlay" || cmd == "overlay:status" {
            let overlay = super::visual_debug::overlay_enabled();
            return serde_json::json!({
                "visual_debug_overlay": overlay
            })
            .to_string();
        }
        if cmd == "overlay:on" || cmd == "overlay:enable" {
            super::visual_debug::set_overlay(true);
            return "Visual debug overlay enabled.".to_string();
        }
        if cmd == "overlay:off" || cmd == "overlay:disable" {
            super::visual_debug::set_overlay(false);
            return "Visual debug overlay disabled.".to_string();
        }
        if cmd.starts_with("message:") {
            let msg = cmd.strip_prefix("message:").unwrap_or("");
            // Inject the message respecting queue mode (like keyboard Enter)
            self.input = msg.to_string();
            match self.send_action(false) {
                SendAction::Submit => {
                    self.submit_input();
                    self.debug_trace
                        .record("message", format!("submitted:{}", msg));
                    format!("OK: submitted message '{}'", msg)
                }
                SendAction::Queue => {
                    self.queue_message();
                    self.debug_trace
                        .record("message", format!("queued:{}", msg));
                    format!(
                        "OK: queued message '{}' (will send after current turn)",
                        msg
                    )
                }
                SendAction::Interleave => {
                    let expanded = self.expand_paste_placeholders(&self.input.clone());
                    self.pasted_contents.clear();
                    self.pending_images.clear();
                    self.input.clear();
                    self.cursor_pos = 0;
                    self.interleave_message = Some(expanded);
                    self.debug_trace
                        .record("message", format!("interleave:{}", msg));
                    format!("OK: interleave message '{}' (injecting now)", msg)
                }
            }
        } else if cmd == "reload" {
            // Trigger reload
            self.input = "/reload".to_string();
            self.submit_input();
            self.debug_trace.record("reload", "triggered".to_string());
            "OK: reload triggered".to_string()
        } else if cmd == "state" {
            // Return current state as JSON for easier parsing
            serde_json::json!({
                "processing": self.is_processing,
                "messages": self.messages.len(),
                "display_messages": self.display_messages.len(),
                "input": self.input,
                "cursor_pos": self.cursor_pos,
                "scroll_offset": self.scroll_offset,
                "queued_messages": self.queued_messages.len(),
                "provider_session_id": self.provider_session_id,
                "model": self.provider.name(),
                "diagram_mode": format!("{:?}", self.diagram_mode),
                "diagram_focus": self.diagram_focus,
                "diagram_index": self.diagram_index,
                "diagram_scroll": [self.diagram_scroll_x, self.diagram_scroll_y],
                "diagram_pane_ratio": self.diagram_pane_ratio_target,
                "diagram_pane_enabled": self.diagram_pane_enabled,
                "diagram_pane_position": format!("{:?}", self.diagram_pane_position),
                "diagram_zoom": self.diagram_zoom,
                "diagram_count": crate::tui::mermaid::get_active_diagrams().len(),
                "version": env!("JCODE_VERSION"),
            })
            .to_string()
        } else if cmd == "swarm" || cmd == "swarm-status" {
            if self.is_remote {
                serde_json::json!({
                    "session_count": self.remote_sessions.len(),
                    "client_count": self.remote_client_count,
                    "members": self.remote_swarm_members,
                })
                .to_string()
            } else {
                serde_json::json!({
                    "session_count": 1,
                    "client_count": null,
                    "members": vec![crate::protocol::SwarmMemberStatus {
                        session_id: self.session.id.clone(),
                        friendly_name: Some(self.session.display_name().to_string()),
                        status: match &self.status {
                            ProcessingStatus::Idle => "ready".to_string(),
                            ProcessingStatus::Sending | ProcessingStatus::Connecting(_) => "running".to_string(),
                            ProcessingStatus::Thinking(_) => "thinking".to_string(),
                            ProcessingStatus::Streaming => "running".to_string(),
                            ProcessingStatus::RunningTool(_) => "running".to_string(),
                        },
                        detail: self.subagent_status.clone(),
                        role: None,
                    }],
                })
                .to_string()
            }
        } else if cmd == "snapshot" {
            let snapshot = self.build_debug_snapshot();
            serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("wait:") {
            let raw = cmd.strip_prefix("wait:").unwrap_or("0");
            if let Ok(ms) = raw.parse::<u64>() {
                return self.apply_wait_ms(ms);
            }
            format!("ERR: invalid wait '{}'", raw)
        } else if cmd == "wait" {
            if self.is_processing {
                "wait: processing".to_string()
            } else {
                "wait: idle".to_string()
            }
        } else if cmd == "last_response" {
            // Get last assistant message
            self.display_messages
                .iter()
                .rev()
                .find(|m| m.role == "assistant" || m.role == "error")
                .map(|m| format!("last_response: [{}] {}", m.role, m.content))
                .unwrap_or_else(|| "last_response: none".to_string())
        } else if cmd == "history" {
            // Return all messages as JSON
            let msgs: Vec<serde_json::Value> = self
                .display_messages
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                        "tool_calls": m.tool_calls,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&msgs).unwrap_or_else(|_| "[]".to_string())
        } else if cmd == "screen" {
            // Capture current visual state
            use super::visual_debug;
            visual_debug::enable(); // Ensure enabled
                                    // Force a frame dump to file and return path
            match visual_debug::dump_to_file() {
                Ok(path) => format!("screen: {}", path.display()),
                Err(e) => format!("screen error: {}", e),
            }
        } else if cmd == "screen-json" {
            use super::visual_debug;
            visual_debug::enable();
            visual_debug::latest_frame_json()
                .unwrap_or_else(|| "screen-json: no frames captured".to_string())
        } else if cmd == "screen-json-normalized" {
            use super::visual_debug;
            visual_debug::enable();
            visual_debug::latest_frame_json_normalized()
                .unwrap_or_else(|| "screen-json-normalized: no frames captured".to_string())
        } else if cmd == "layout" {
            use super::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "terminal_size": frame.terminal_size,
                    "layout": frame.layout,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                None => "layout: no frames captured".to_string(),
            }
        } else if cmd == "margins" {
            use super::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "margins": frame.layout.margins,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                None => "margins: no frames captured".to_string(),
            }
        } else if cmd == "widgets" || cmd == "info-widgets" {
            use super::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "info_widgets": frame.info_widgets,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                None => "widgets: no frames captured".to_string(),
            }
        } else if cmd == "render-stats" {
            use super::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "render_timing": frame.render_timing,
                    "render_order": frame.render_order,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                None => "render-stats: no frames captured".to_string(),
            }
        } else if cmd == "render-order" {
            use super::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&frame.render_order)
                    .unwrap_or_else(|_| "[]".to_string()),
                None => "render-order: no frames captured".to_string(),
            }
        } else if cmd == "anomalies" {
            use super::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&frame.anomalies)
                    .unwrap_or_else(|_| "[]".to_string()),
                None => "anomalies: no frames captured".to_string(),
            }
        } else if cmd == "theme" {
            use super::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&frame.theme)
                    .unwrap_or_else(|_| "null".to_string()),
                None => "theme: no frames captured".to_string(),
            }
        } else if cmd == "mermaid:stats" {
            let stats = super::mermaid::debug_stats();
            serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:memory" {
            let profile = super::mermaid::debug_memory_profile();
            serde_json::to_string_pretty(&profile).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:memory-bench" {
            let result = super::mermaid::debug_memory_benchmark(40);
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("mermaid:memory-bench ") {
            let raw_iterations = cmd
                .strip_prefix("mermaid:memory-bench ")
                .unwrap_or("")
                .trim();
            let iterations = match raw_iterations.parse::<usize>() {
                Ok(v) => v,
                Err(_) => return "Invalid iterations (expected integer)".to_string(),
            };
            let result = super::mermaid::debug_memory_benchmark(iterations);
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:cache" {
            let entries = super::mermaid::debug_cache();
            serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
        } else if cmd == "mermaid:evict" || cmd == "mermaid:clear-cache" {
            match super::mermaid::clear_cache() {
                Ok(_) => "mermaid: cache cleared".to_string(),
                Err(e) => format!("mermaid: cache clear failed: {}", e),
            }
        } else if cmd == "markdown:stats" {
            let stats = super::markdown::debug_stats();
            serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("assert:") {
            let raw = cmd.strip_prefix("assert:").unwrap_or("");
            self.handle_assertions(raw)
        } else if cmd.starts_with("run:") {
            let raw = cmd.strip_prefix("run:").unwrap_or("");
            self.handle_script_run(raw)
        } else if cmd.starts_with("inject:") {
            let raw = cmd.strip_prefix("inject:").unwrap_or("");
            let (role, content) = if let Some((r, c)) = raw.split_once(':') {
                let role = match r {
                    "user" | "assistant" | "system" | "tool" | "error" | "meta" => r,
                    _ => "assistant",
                };
                if role == "assistant" && r != "assistant" {
                    ("assistant", raw)
                } else {
                    (role, c)
                }
            } else {
                ("assistant", raw)
            };

            self.push_display_message(DisplayMessage {
                role: role.to_string(),
                content: content.to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            format!("OK: injected {} message ({} chars)", role, content.len())
        } else if cmd == "scroll-test" || cmd.starts_with("scroll-test:") {
            let raw = cmd.strip_prefix("scroll-test:");
            self.run_scroll_test(raw)
        } else if cmd == "scroll-suite" || cmd.starts_with("scroll-suite:") {
            let raw = cmd.strip_prefix("scroll-suite:");
            self.run_scroll_suite(raw)
        } else if cmd == "quit" {
            self.should_quit = true;
            "OK: quitting".to_string()
        } else if cmd == "trace-start" {
            self.debug_trace.enabled = true;
            self.debug_trace.started_at = Instant::now();
            self.debug_trace.events.clear();
            "OK: trace started".to_string()
        } else if cmd == "trace-stop" {
            self.debug_trace.enabled = false;
            "OK: trace stopped".to_string()
        } else if cmd == "trace" {
            serde_json::to_string_pretty(&self.debug_trace.events)
                .unwrap_or_else(|_| "[]".to_string())
        } else if cmd.starts_with("scroll:") {
            let dir = cmd.strip_prefix("scroll:").unwrap_or("");
            match dir {
                "up" => {
                    self.debug_scroll_up(5);
                    format!("scroll: up to {}", self.scroll_offset)
                }
                "down" => {
                    self.debug_scroll_down(5);
                    format!("scroll: down to {}", self.scroll_offset)
                }
                "top" => {
                    self.debug_scroll_top();
                    "scroll: top".to_string()
                }
                "bottom" => {
                    self.debug_scroll_bottom();
                    "scroll: bottom".to_string()
                }
                _ => format!("scroll error: unknown direction '{}'", dir),
            }
        } else if cmd.starts_with("keys:") {
            let keys_str = cmd.strip_prefix("keys:").unwrap_or("");
            let mut results = Vec::new();
            for key_spec in keys_str.split(',') {
                match self.parse_and_inject_key(key_spec.trim()) {
                    Ok(desc) => {
                        self.debug_trace.record("key", format!("{}", desc));
                        results.push(format!("OK: {}", desc));
                    }
                    Err(e) => results.push(format!("ERR: {}", e)),
                }
            }
            results.join("\n")
        } else if cmd == "input" {
            format!("input: {:?}", self.input)
        } else if cmd.starts_with("set_input:") {
            let new_input = cmd.strip_prefix("set_input:").unwrap_or("");
            self.input = new_input.to_string();
            self.cursor_pos = self.input.len();
            self.debug_trace
                .record("input", format!("set:{}", self.input));
            format!("OK: input set to {:?}", self.input)
        } else if cmd == "submit" {
            if self.input.is_empty() {
                "submit error: input is empty".to_string()
            } else {
                self.submit_input();
                self.debug_trace.record("input", "submitted".to_string());
                "OK: submitted".to_string()
            }
        } else if cmd == "record-start" {
            use super::test_harness;
            test_harness::start_recording();
            "OK: event recording started".to_string()
        } else if cmd == "record-stop" {
            use super::test_harness;
            test_harness::stop_recording();
            "OK: event recording stopped".to_string()
        } else if cmd == "record-events" {
            use super::test_harness;
            test_harness::get_recorded_events_json()
        } else if cmd == "clock-enable" {
            use super::test_harness;
            test_harness::enable_test_clock();
            "OK: test clock enabled".to_string()
        } else if cmd == "clock-disable" {
            use super::test_harness;
            test_harness::disable_test_clock();
            "OK: test clock disabled".to_string()
        } else if cmd.starts_with("clock-advance:") {
            use super::test_harness;
            let ms_str = cmd.strip_prefix("clock-advance:").unwrap_or("0");
            match ms_str.parse::<u64>() {
                Ok(ms) => {
                    test_harness::advance_clock(std::time::Duration::from_millis(ms));
                    format!("OK: clock advanced {}ms", ms)
                }
                Err(_) => "clock-advance error: invalid ms value".to_string(),
            }
        } else if cmd == "clock-now" {
            use super::test_harness;
            format!("clock: {}ms", test_harness::now_ms())
        } else if cmd.starts_with("replay:") {
            use super::test_harness;
            let json = cmd.strip_prefix("replay:").unwrap_or("[]");
            match test_harness::EventPlayer::from_json(json) {
                Ok(mut player) => {
                    player.start();
                    let mut results = Vec::new();
                    while let Some(event) = player.next_event() {
                        results.push(format!("{:?}", event));
                    }
                    format!(
                        "replay: {} events processed, {} remaining",
                        results.len(),
                        player.remaining()
                    )
                }
                Err(e) => format!("replay error: {}", e),
            }
        } else if cmd.starts_with("bundle-start:") {
            let name = cmd.strip_prefix("bundle-start:").unwrap_or("test");
            std::env::set_var("JCODE_TEST_BUNDLE", name);
            format!("OK: test bundle '{}' started", name)
        } else if cmd == "bundle-save" {
            use super::test_harness::TestBundle;
            let name = std::env::var("JCODE_TEST_BUNDLE").unwrap_or_else(|_| "unnamed".to_string());
            let bundle = TestBundle::new(&name);
            let path = TestBundle::default_path(&name);
            match bundle.save(&path) {
                Ok(_) => format!("OK: bundle saved to {}", path.display()),
                Err(e) => format!("bundle-save error: {}", e),
            }
        } else if cmd.starts_with("script:") {
            let raw = cmd.strip_prefix("script:").unwrap_or("{}");
            match serde_json::from_str::<super::test_harness::TestScript>(raw) {
                Ok(script) => self.handle_test_script(script),
                Err(e) => format!("script error: {}", e),
            }
        } else if cmd == "version" {
            format!("version: {}", env!("JCODE_VERSION"))
        } else if cmd == "help" {
            "Debug commands:\n\
                 - message:<text> - inject and submit a message\n\
                 - inject:<role>:<text> - inject display message without sending\n\
                 - reload - trigger /reload\n\
                 - state - get basic state info\n\
                 - snapshot - get combined state + frame snapshot JSON\n\
                 - assert:<json> - run assertions (see docs)\n\
                 - run:<json> - run scripted steps + assertions\n\
                 - trace-start - start recording trace events\n\
                 - trace-stop - stop recording trace events\n\
                 - trace - dump trace events JSON\n\
                 - quit - exit the TUI\n\
                 - last_response - get last assistant message\n\
                 - history - get all messages as JSON\n\
                 - screen - dump visual debug frames\n\
                 - screen-json - dump latest visual frame JSON\n\
                 - screen-json-normalized - dump normalized frame (for diffs)\n\
                 - frame - alias for screen-json\n\
                 - frame-normalized - alias for screen-json-normalized\n\
                 - layout - dump latest layout JSON\n\
                 - margins - dump layout margins JSON\n\
                 - widgets - dump info widget summary/placements\n\
                 - render-stats - dump render timing + order JSON\n\
                 - render-order - dump render order list\n\
                 - anomalies - dump visual debug anomalies\n\
                 - theme - dump current palette snapshot\n\
                 - mermaid:stats - dump mermaid debug stats\n\
                 - mermaid:cache - list mermaid cache entries\n\
                 - mermaid:evict - clear mermaid cache\n\
                 - markdown:stats - dump markdown debug stats\n\
                 - overlay:on/off/status - toggle overlay boxes\n\
                 - enable/disable/status - control visual debug capture\n\
                 - wait - check if processing\n\
                 - wait:<ms> - block until idle or timeout\n\
                 - scroll:<up|down|top|bottom> - control scroll\n\
                 - scroll-test[:<json>] - run offscreen scroll+diagram test\n\
                 - scroll-suite[:<json>] - run scroll+diagram test suite\n\
                 - keys:<keyspec> - inject key events (e.g. keys:ctrl+r)\n\
                 - input - get current input buffer\n\
                 - set_input:<text> - set input buffer\n\
                 - submit - submit current input\n\
                 - record-start - start event recording\n\
                 - record-stop - stop event recording\n\
                 - record-events - get recorded events JSON\n\
                 - clock-enable - enable deterministic test clock\n\
                 - clock-disable - disable test clock\n\
                 - clock-advance:<ms> - advance test clock\n\
                 - clock-now - get current clock time\n\
                 - replay:<json> - replay recorded events\n\
                 - bundle-start:<name> - start test bundle\n\
                 - bundle-save - save test bundle\n\
                 - script:<json> - run test script\n\
                 - version - get version\n\
                 - help - show this help"
                .to_string()
        } else {
            format!("ERROR: unknown command '{}'. Use 'help' for list.", cmd)
        }
    }

    async fn handle_debug_command_remote(
        &mut self,
        cmd: &str,
        remote: &mut super::backend::RemoteConnection,
    ) -> String {
        let cmd = cmd.trim();
        if cmd.starts_with("message:") {
            let msg = cmd.strip_prefix("message:").unwrap_or("");
            self.input = msg.to_string();
            let result = self
                .handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), remote)
                .await;
            if let Err(e) = result {
                return format!("ERR: {}", e);
            }
            self.debug_trace
                .record("message", format!("submitted:{}", msg));
            return format!("OK: queued message '{}'", msg);
        }
        if cmd == "reload" {
            self.input = "/reload".to_string();
            let result = self
                .handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), remote)
                .await;
            if let Err(e) = result {
                return format!("ERR: {}", e);
            }
            self.debug_trace.record("reload", "triggered".to_string());
            return "OK: reload triggered".to_string();
        }
        if cmd == "state" {
            return serde_json::json!({
                "processing": self.is_processing,
                "messages": self.messages.len(),
                "display_messages": self.display_messages.len(),
                "input": self.input,
                "cursor_pos": self.cursor_pos,
                "scroll_offset": self.scroll_offset,
                "queued_messages": self.queued_messages.len(),
                "provider_session_id": self.provider_session_id,
                "provider_name": self.remote_provider_name.clone(),
                "model": self
                    .remote_provider_model
                    .as_deref()
                    .unwrap_or(self.provider.name()),
                "diagram_mode": format!("{:?}", self.diagram_mode),
                "diagram_focus": self.diagram_focus,
                "diagram_index": self.diagram_index,
                "diagram_scroll": [self.diagram_scroll_x, self.diagram_scroll_y],
                "diagram_pane_ratio": self.diagram_pane_ratio_target,
                "diagram_pane_enabled": self.diagram_pane_enabled,
                "diagram_pane_position": format!("{:?}", self.diagram_pane_position),
                "diagram_zoom": self.diagram_zoom,
                "diagram_count": crate::tui::mermaid::get_active_diagrams().len(),
                "remote": true,
                "server_version": self.remote_server_version.clone(),
                "server_has_update": self.remote_server_has_update,
                "version": env!("JCODE_VERSION"),
                "diagram_mode": format!("{:?}", self.diagram_mode),
            })
            .to_string();
        }
        if cmd.starts_with("keys:") {
            let keys_str = cmd.strip_prefix("keys:").unwrap_or("");
            let mut results = Vec::new();
            for key_spec in keys_str.split(',') {
                match self
                    .parse_and_inject_key_remote(key_spec.trim(), remote)
                    .await
                {
                    Ok(desc) => {
                        self.debug_trace.record("key", format!("{}", desc));
                        results.push(format!("OK: {}", desc));
                    }
                    Err(e) => results.push(format!("ERR: {}", e)),
                }
            }
            return results.join("\n");
        }
        if cmd == "submit" {
            if self.input.is_empty() {
                return "submit error: input is empty".to_string();
            }
            let result = self
                .handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), remote)
                .await;
            if let Err(e) = result {
                return format!("ERR: {}", e);
            }
            self.debug_trace.record("input", "submitted".to_string());
            return "OK: submitted".to_string();
        }
        if cmd.starts_with("run:") || cmd.starts_with("script:") {
            return "ERR: script/run not supported in remote debug mode".to_string();
        }
        self.handle_debug_command(cmd)
    }

    /// Check for new stable version and trigger migration if at safe point
    fn check_stable_version(&mut self) {
        // Only check every 5 seconds to avoid excessive file reads
        let should_check = self
            .last_version_check
            .map(|t| t.elapsed() > Duration::from_secs(5))
            .unwrap_or(true);

        if !should_check {
            return;
        }

        self.last_version_check = Some(Instant::now());

        // Don't migrate if we're a canary session (we test changes, not receive them)
        if self.session.is_canary {
            return;
        }

        // Read current stable version
        let current_stable = match crate::build::read_stable_version() {
            Ok(Some(v)) => v,
            _ => return,
        };

        // Check if it changed
        let version_changed = self
            .known_stable_version
            .as_ref()
            .map(|v| v != &current_stable)
            .unwrap_or(true);

        if !version_changed {
            return;
        }

        // New stable version detected
        self.known_stable_version = Some(current_stable.clone());

        // Check if we're at a safe point to migrate
        let at_safe_point = !self.is_processing && self.queued_messages.is_empty();

        if at_safe_point {
            // Trigger migration
            self.pending_migration = Some(current_stable);
        }
    }

    /// Execute pending migration to new stable version
    fn execute_migration(&mut self) -> bool {
        if let Some(ref version) = self.pending_migration.take() {
            let stable_binary = match crate::build::stable_binary_path() {
                Ok(p) if p.exists() => p,
                _ => return false,
            };

            // Save session before migration
            if let Err(e) = self.session.save() {
                let msg = format!("Failed to save session before migration: {}", e);
                crate::logging::error(&msg);
                self.push_display_message(DisplayMessage::error(msg));
                self.set_status_notice("Migration aborted");
                return false;
            }

            // Request reload to stable version
            self.reload_requested = Some(self.session.id.clone());

            // The actual exec happens in main.rs when run() returns
            // We store the binary path in an env var for the reload handler
            std::env::set_var("JCODE_MIGRATE_BINARY", stable_binary);

            crate::logging::info(&format!("Migrating to stable version {}...", version));
            self.set_status_notice(format!("Migrating to stable {}...", version));
            self.should_quit = true;
            return true;
        }
        false
    }

    fn build_debug_snapshot(&self) -> DebugSnapshot {
        let frame = crate::tui::visual_debug::latest_frame();
        let recent_messages = self
            .display_messages
            .iter()
            .rev()
            .take(20)
            .map(|msg| DebugMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls: msg.tool_calls.clone(),
                duration_secs: msg.duration_secs,
                title: msg.title.clone(),
            })
            .collect::<Vec<_>>();
        DebugSnapshot {
            state: serde_json::json!({
                "processing": self.is_processing,
                "messages": self.messages.len(),
                "display_messages": self.display_messages.len(),
                "input": self.input,
                "cursor_pos": self.cursor_pos,
                "scroll_offset": self.scroll_offset,
                "queued_messages": self.queued_messages.len(),
                "provider_session_id": self.provider_session_id,
                "model": self.provider.name(),
                "diagram_mode": format!("{:?}", self.diagram_mode),
                "diagram_pane_enabled": self.diagram_pane_enabled,
                "diagram_pane_position": format!("{:?}", self.diagram_pane_position),
                "diagram_zoom": self.diagram_zoom,
                "version": env!("JCODE_VERSION"),
            }),
            frame,
            recent_messages,
            queued_messages: self.queued_messages.clone(),
        }
    }

    fn eval_assertions(&self, assertions: &[DebugAssertion]) -> Vec<DebugAssertResult> {
        let snapshot = self.build_debug_snapshot();
        let mut results = Vec::new();
        for assertion in assertions {
            let actual = self.lookup_snapshot_value(&snapshot, &assertion.field);
            let expected = assertion.value.clone();
            let op = assertion.op.as_str();
            let ok = match op {
                "eq" => actual == expected,
                "ne" => actual != expected,
                "contains" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(b)) => a.contains(b),
                    (serde_json::Value::Array(a), _) => a.contains(&expected),
                    _ => false,
                },
                "not_contains" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(b)) => !a.contains(b),
                    (serde_json::Value::Array(a), _) => !a.contains(&expected),
                    _ => true,
                },
                "exists" => actual != serde_json::Value::Null,
                "not_exists" => actual == serde_json::Value::Null,
                "gt" => match (&actual, &expected) {
                    (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                        a.as_f64().unwrap_or(0.0) > b.as_f64().unwrap_or(0.0)
                    }
                    _ => false,
                },
                "gte" => match (&actual, &expected) {
                    (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                        a.as_f64().unwrap_or(0.0) >= b.as_f64().unwrap_or(0.0)
                    }
                    _ => false,
                },
                "lt" => match (&actual, &expected) {
                    (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                        a.as_f64().unwrap_or(0.0) < b.as_f64().unwrap_or(0.0)
                    }
                    _ => false,
                },
                "lte" => match (&actual, &expected) {
                    (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                        a.as_f64().unwrap_or(0.0) <= b.as_f64().unwrap_or(0.0)
                    }
                    _ => false,
                },
                "len" => match &actual {
                    serde_json::Value::String(s) => expected
                        .as_u64()
                        .map(|e| s.len() as u64 == e)
                        .unwrap_or(false),
                    serde_json::Value::Array(a) => expected
                        .as_u64()
                        .map(|e| a.len() as u64 == e)
                        .unwrap_or(false),
                    serde_json::Value::Object(o) => expected
                        .as_u64()
                        .map(|e| o.len() as u64 == e)
                        .unwrap_or(false),
                    _ => false,
                },
                "len_gt" => match &actual {
                    serde_json::Value::String(s) => expected
                        .as_u64()
                        .map(|e| s.len() as u64 > e)
                        .unwrap_or(false),
                    serde_json::Value::Array(a) => expected
                        .as_u64()
                        .map(|e| a.len() as u64 > e)
                        .unwrap_or(false),
                    _ => false,
                },
                "len_lt" => match &actual {
                    serde_json::Value::String(s) => expected
                        .as_u64()
                        .map(|e| (s.len() as u64) < e)
                        .unwrap_or(false),
                    serde_json::Value::Array(a) => expected
                        .as_u64()
                        .map(|e| (a.len() as u64) < e)
                        .unwrap_or(false),
                    _ => false,
                },
                "matches" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(pattern)) => {
                        regex::Regex::new(pattern)
                            .map(|re| re.is_match(a))
                            .unwrap_or(false)
                    }
                    _ => false,
                },
                "not_matches" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(pattern)) => {
                        regex::Regex::new(pattern)
                            .map(|re| !re.is_match(a))
                            .unwrap_or(true)
                    }
                    _ => true,
                },
                "starts_with" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(b)) => {
                        a.starts_with(b)
                    }
                    _ => false,
                },
                "ends_with" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(b)) => a.ends_with(b),
                    _ => false,
                },
                "is_empty" => match &actual {
                    serde_json::Value::String(s) => s.is_empty(),
                    serde_json::Value::Array(a) => a.is_empty(),
                    serde_json::Value::Object(o) => o.is_empty(),
                    serde_json::Value::Null => true,
                    _ => false,
                },
                "is_not_empty" => match &actual {
                    serde_json::Value::String(s) => !s.is_empty(),
                    serde_json::Value::Array(a) => !a.is_empty(),
                    serde_json::Value::Object(o) => !o.is_empty(),
                    serde_json::Value::Null => false,
                    _ => true,
                },
                "is_true" => actual == serde_json::Value::Bool(true),
                "is_false" => actual == serde_json::Value::Bool(false),
                _ => false,
            };
            let message = if ok {
                "ok".to_string()
            } else {
                format!(
                    "expected {} {} {:?}, got {:?}",
                    assertion.field, op, expected, actual
                )
            };
            results.push(DebugAssertResult {
                ok,
                field: assertion.field.clone(),
                op: assertion.op.clone(),
                expected,
                actual,
                message,
            });
        }
        results
    }

    fn handle_assertions(&mut self, raw: &str) -> String {
        let parsed: Result<Vec<DebugAssertion>, _> = serde_json::from_str(raw);
        let assertions = match parsed {
            Ok(a) => a,
            Err(e) => {
                return format!("assert parse error: {}", e);
            }
        };
        let results = self.eval_assertions(&assertions);
        serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string())
    }

    fn handle_script_run(&mut self, raw: &str) -> String {
        let parsed: Result<DebugScript, _> = serde_json::from_str(raw);
        let script = match parsed {
            Ok(s) => s,
            Err(e) => return format!("run parse error: {}", e),
        };

        let mut steps = Vec::new();
        let mut ok = true;
        for step in &script.steps {
            let detail = self.execute_script_step(step);
            let step_ok = !detail.starts_with("ERR");
            if !step_ok {
                ok = false;
            }
            steps.push(DebugStepResult {
                step: step.clone(),
                ok: step_ok,
                detail,
            });
        }

        if let Some(wait_ms) = script.wait_ms {
            let _ = self.apply_wait_ms(wait_ms);
        }

        let assertions = self.eval_assertions(&script.assertions);
        if assertions.iter().any(|a| !a.ok) {
            ok = false;
        }

        let report = DebugRunReport {
            ok,
            steps,
            assertions,
        };

        serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
    }

    fn handle_test_script(&mut self, script: super::test_harness::TestScript) -> String {
        use super::test_harness::TestStep;

        let mut results = Vec::new();
        for step in &script.steps {
            let step_result = match step {
                TestStep::Message { content } => {
                    self.input = content.clone();
                    self.submit_input();
                    format!("message: {}", content)
                }
                TestStep::SetInput { text } => {
                    self.input = text.clone();
                    self.cursor_pos = self.input.len();
                    format!("set_input: {}", text)
                }
                TestStep::Submit => {
                    if !self.input.is_empty() {
                        self.submit_input();
                        "submit: OK".to_string()
                    } else {
                        "submit: skipped (empty)".to_string()
                    }
                }
                TestStep::WaitIdle { timeout_ms } => {
                    let _ = self.apply_wait_ms(timeout_ms.unwrap_or(30000));
                    "wait_idle: done".to_string()
                }
                TestStep::Wait { ms } => {
                    std::thread::sleep(std::time::Duration::from_millis(*ms));
                    format!("wait: {}ms", ms)
                }
                TestStep::Checkpoint { name } => format!("checkpoint: {}", name),
                TestStep::Command { cmd } => {
                    format!("command: {} (nested commands not supported)", cmd)
                }
                TestStep::Keys { keys } => {
                    let mut key_results = Vec::new();
                    for key_spec in keys.split(',') {
                        match self.parse_and_inject_key(key_spec.trim()) {
                            Ok(desc) => key_results.push(format!("OK: {}", desc)),
                            Err(e) => key_results.push(format!("ERR: {}", e)),
                        }
                    }
                    format!("keys: {}", key_results.join(", "))
                }
                TestStep::Scroll { direction } => {
                    match direction.as_str() {
                        "up" => self.debug_scroll_up(5),
                        "down" => self.debug_scroll_down(5),
                        "top" => self.debug_scroll_top(),
                        "bottom" => self.debug_scroll_bottom(),
                        _ => {}
                    }
                    format!("scroll: {}", direction)
                }
                TestStep::Assert { assertions } => {
                    let parsed: Vec<DebugAssertion> = assertions
                        .iter()
                        .filter_map(|a| serde_json::from_value(a.clone()).ok())
                        .collect();
                    let results = self.eval_assertions(&parsed);
                    let passed = results.iter().all(|r| r.ok);
                    format!(
                        "assert: {} ({}/{})",
                        if passed { "PASS" } else { "FAIL" },
                        results.iter().filter(|r| r.ok).count(),
                        results.len()
                    )
                }
                TestStep::Snapshot { name } => format!("snapshot: {}", name),
            };
            results.push(step_result);
        }

        serde_json::json!({
            "script": script.name,
            "steps": results,
            "completed": true
        })
        .to_string()
    }

    fn apply_wait_ms(&mut self, wait_ms: u64) -> String {
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        while Instant::now() < deadline {
            if !self.is_processing {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.debug_trace.record("wait", format!("{}ms", wait_ms));
        format!("waited {}ms", wait_ms)
    }

    fn lookup_snapshot_value(&self, snapshot: &DebugSnapshot, field: &str) -> serde_json::Value {
        let parts: Vec<&str> = field.split('.').collect();
        if parts.is_empty() {
            return serde_json::Value::Null;
        }
        match parts[0] {
            "state" => Self::lookup_json_path(&snapshot.state, &parts[1..]),
            "frame" => {
                if let Some(frame) = &snapshot.frame {
                    let value = serde_json::to_value(frame).unwrap_or(serde_json::Value::Null);
                    Self::lookup_json_path(&value, &parts[1..])
                } else {
                    serde_json::Value::Null
                }
            }
            "recent_messages" => {
                let value = serde_json::to_value(&snapshot.recent_messages)
                    .unwrap_or(serde_json::Value::Null);
                Self::lookup_json_path(&value, &parts[1..])
            }
            "queued_messages" => {
                let value = serde_json::to_value(&snapshot.queued_messages)
                    .unwrap_or(serde_json::Value::Null);
                Self::lookup_json_path(&value, &parts[1..])
            }
            _ => serde_json::Value::Null,
        }
    }

    fn lookup_json_path(value: &serde_json::Value, parts: &[&str]) -> serde_json::Value {
        let mut current = value;
        for part in parts {
            if let Ok(index) = part.parse::<usize>() {
                if let Some(v) = current.get(index) {
                    current = v;
                    continue;
                }
            }
            if let Some(v) = current.get(part) {
                current = v;
                continue;
            }
            return serde_json::Value::Null;
        }
        current.clone()
    }

    fn execute_script_step(&mut self, step: &str) -> String {
        let trimmed = step.trim();
        if trimmed.is_empty() {
            return "ERR: empty step".to_string();
        }
        if trimmed.starts_with("keys:") {
            let keys_str = trimmed.strip_prefix("keys:").unwrap_or("");
            let mut results = Vec::new();
            for key_spec in keys_str.split(',') {
                match self.parse_and_inject_key(key_spec.trim()) {
                    Ok(desc) => {
                        self.debug_trace.record("key", desc.clone());
                        results.push(format!("OK: {}", desc));
                    }
                    Err(e) => results.push(format!("ERR: {}", e)),
                }
            }
            return results.join("\n");
        }
        if trimmed.starts_with("set_input:") {
            let new_input = trimmed.strip_prefix("set_input:").unwrap_or("");
            self.input = new_input.to_string();
            self.cursor_pos = self.input.len();
            self.debug_trace
                .record("input", format!("set:{}", self.input));
            return format!("OK: input set to {:?}", self.input);
        }
        if trimmed == "submit" {
            if self.input.is_empty() {
                return "ERR: input is empty".to_string();
            }
            self.submit_input();
            self.debug_trace.record("input", "submitted".to_string());
            return "OK: submitted".to_string();
        }
        if trimmed.starts_with("message:") {
            let msg = trimmed.strip_prefix("message:").unwrap_or("");
            self.input = msg.to_string();
            self.submit_input();
            self.debug_trace
                .record("message", format!("submitted:{}", msg));
            return format!("OK: queued message '{}'", msg);
        }
        if trimmed.starts_with("scroll:") {
            let dir = trimmed.strip_prefix("scroll:").unwrap_or("");
            return match dir {
                "up" => {
                    self.debug_scroll_up(5);
                    format!("scroll: up to {}", self.scroll_offset)
                }
                "down" => {
                    self.debug_scroll_down(5);
                    format!("scroll: down to {}", self.scroll_offset)
                }
                "top" => {
                    self.debug_scroll_top();
                    "scroll: top".to_string()
                }
                "bottom" => {
                    self.debug_scroll_bottom();
                    "scroll: bottom".to_string()
                }
                _ => format!("ERR: unknown scroll '{}'", dir),
            };
        }
        if trimmed == "reload" {
            self.input = "/reload".to_string();
            self.submit_input();
            self.debug_trace.record("reload", "triggered".to_string());
            return "OK: reload triggered".to_string();
        }
        if trimmed == "snapshot" {
            let snapshot = self.build_debug_snapshot();
            return serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string());
        }
        if trimmed.starts_with("wait:") {
            let raw = trimmed.strip_prefix("wait:").unwrap_or("0");
            if let Ok(ms) = raw.parse::<u64>() {
                return self.apply_wait_ms(ms);
            }
            return format!("ERR: invalid wait '{}'", raw);
        }
        if trimmed == "wait" {
            return if self.is_processing {
                "wait: processing".to_string()
            } else {
                "wait: idle".to_string()
            };
        }
        format!("ERR: unknown step '{}'", trimmed)
    }

    fn check_debug_command(&mut self) -> Option<String> {
        let cmd_path = debug_cmd_path();
        if let Ok(cmd) = std::fs::read_to_string(&cmd_path) {
            // Remove command file immediately
            let _ = std::fs::remove_file(&cmd_path);
            let cmd = cmd.trim();

            self.debug_trace
                .record("cmd", format!("{}", cmd.to_string()));

            let response = self.handle_debug_command(cmd);

            // Write response
            let _ = std::fs::write(debug_response_path(), &response);
            return Some(response);
        }
        None
    }

    async fn check_debug_command_remote(
        &mut self,
        remote: &mut super::backend::RemoteConnection,
    ) -> Option<String> {
        let cmd_path = debug_cmd_path();
        if let Ok(cmd) = std::fs::read_to_string(&cmd_path) {
            // Remove command file immediately
            let _ = std::fs::remove_file(&cmd_path);
            let cmd = cmd.trim();

            self.debug_trace
                .record("cmd", format!("{}", cmd.to_string()));

            let response = self.handle_debug_command_remote(cmd, remote).await;

            // Write response
            let _ = std::fs::write(debug_response_path(), &response);
            return Some(response);
        }
        None
    }

    fn parse_key_spec(&self, key_spec: &str) -> Result<(KeyCode, KeyModifiers), String> {
        let key_spec = key_spec.to_lowercase();
        let parts: Vec<&str> = key_spec.split('+').collect();

        let mut modifiers = KeyModifiers::empty();
        let mut key_part = "";

        for part in &parts {
            match *part {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                _ => key_part = part,
            }
        }

        let key_code = match key_part {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            s if s.len() == 1 => KeyCode::Char(s.chars().next().unwrap()),
            s if s.starts_with('f') && s.len() <= 3 => {
                if let Ok(n) = s[1..].parse::<u8>() {
                    KeyCode::F(n)
                } else {
                    return Err(format!("Invalid function key: {}", s));
                }
            }
            _ => return Err(format!("Unknown key: {}", key_part)),
        };

        Ok((key_code, modifiers))
    }

    /// Parse a key specification and inject it as an event
    fn parse_and_inject_key(&mut self, key_spec: &str) -> Result<String, String> {
        let (key_code, modifiers) = self.parse_key_spec(key_spec)?;
        let key_event = crossterm::event::KeyEvent::new(key_code, modifiers);
        self.handle_key_event(key_event);
        Ok(format!("injected {:?} with {:?}", key_code, modifiers))
    }

    async fn parse_and_inject_key_remote(
        &mut self,
        key_spec: &str,
        remote: &mut super::backend::RemoteConnection,
    ) -> Result<String, String> {
        let (key_code, modifiers) = self.parse_key_spec(key_spec)?;
        self.handle_remote_key(key_code, modifiers, remote)
            .await
            .map_err(|e| format!("{}", e))?;
        Ok(format!("injected {:?} with {:?}", key_code, modifiers))
    }

    /// Check for selfdev signal files (rebuild-signal)
    /// These are written by the selfdev tool to trigger restarts
    fn check_selfdev_signals(&mut self) {
        // Only check in canary sessions
        if !self.session.is_canary {
            return;
        }

        let jcode_dir = match crate::storage::jcode_dir() {
            Ok(dir) => dir,
            Err(_) => return,
        };

        // Check for rebuild signal
        let rebuild_path = jcode_dir.join("rebuild-signal");
        if rebuild_path.exists() {
            if let Ok(_hash) = std::fs::read_to_string(&rebuild_path) {
                // Remove signal file
                let _ = std::fs::remove_file(&rebuild_path);
                // Save session and trigger exit with code 42 (reload requested)
                self.session.provider_session_id = self.provider_session_id.clone();
                let _ = self.session.save();
                self.requested_exit_code = Some(42);
                self.should_quit = true;
            }
        }
    }

    /// Run the TUI application
    /// Returns Some(session_id) if hot-reload was requested
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<RunResult> {
        let mut event_stream = EventStream::new();
        let mut redraw_period = super::redraw_interval(&self);
        let mut redraw_interval = interval(redraw_period);
        // Subscribe to bus for background task completion notifications
        let mut bus_receiver = Bus::global().subscribe();

        loop {
            let desired_redraw = super::redraw_interval(&self);
            if desired_redraw != redraw_period {
                redraw_period = desired_redraw;
                redraw_interval = interval(redraw_period);
            }

            // Draw UI
            terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;

            if self.should_quit {
                break;
            }

            // Process pending turn OR wait for input/redraw
            if self.pending_turn {
                self.pending_turn = false;
                // Process turn while still handling input
                self.process_turn_with_input(&mut terminal, &mut event_stream)
                    .await;
            } else {
                // Wait for input or redraw tick
                tokio::select! {
                    _ = redraw_interval.tick() => {
                        // Flush stream buffer on timeout
                        if self.stream_buffer.should_flush() {
                            if let Some(chunk) = self.stream_buffer.flush() {
                                self.streaming_text.push_str(&chunk);
                            }
                        }
                        self.poll_compaction_completion();
                        // Check for debug commands
                        self.check_debug_command();
                        // Check for selfdev signals (rebuild)
                        self.check_selfdev_signals();
                        // Check for new stable version (auto-migration)
                        self.check_stable_version();
                        // Execute pending migration if ready
                        if self.pending_migration.is_some() && !self.is_processing {
                            self.execute_migration();
                        }
                        // Check for rate limit expiry - auto-retry pending message
                        if let Some(reset_time) = self.rate_limit_reset {
                            if Instant::now() >= reset_time {
                                self.rate_limit_reset = None;
                                let queued_count = self.queued_messages.len();
                                let msg = if queued_count > 0 {
                                    format!("✓ Rate limit reset. Retrying... (+{} queued)", queued_count)
                                } else {
                                    "✓ Rate limit reset. Retrying...".to_string()
                                };
                                self.push_display_message(DisplayMessage::system(msg));
                                self.pending_turn = true;
                            }
                        }
                    }
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key))) => {
                                if key.kind == KeyEventKind::Press {
                                    self.handle_key(key.code, key.modifiers)?;
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                            }
                            Some(Ok(Event::Mouse(mouse))) => {
                                self.handle_mouse_event(mouse);
                            }
                            _ => {}
                        }
                        while crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
                            if let Ok(ev) = crossterm::event::read() {
                                match ev {
                                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                                        self.handle_key(key.code, key.modifiers)?;
                                    }
                                    Event::Paste(text) => {
                                        self.handle_paste(text);
                                    }
                                    Event::Mouse(mouse) => {
                                        self.handle_mouse_event(mouse);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    // Handle background task completion notifications
                    bus_event = bus_receiver.recv() => {
                        match bus_event {
                            Ok(BusEvent::BackgroundTaskCompleted(task)) => {
                            // Only show notifications for tasks from this session
                            if task.session_id == self.session.id {
                                let status_str = match task.status {
                                    BackgroundTaskStatus::Completed => "✓ completed",
                                    BackgroundTaskStatus::Failed => "✗ failed",
                                    BackgroundTaskStatus::Running => "running",
                                };
                                let notification = format!(
                                    "[Background Task Completed]\n\
                                     Task: {} ({})\n\
                                     Status: {}\n\
                                     Duration: {:.1}s\n\
                                     Exit code: {}\n\n\
                                     Output preview:\n{}\n\n\
                                     Use `bg action=\"output\" task_id=\"{}\"` for full output.",
                                    task.task_id,
                                    task.tool_name,
                                    status_str,
                                    task.duration_secs,
                                    task.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "N/A".to_string()),
                                    task.output_preview,
                                    task.task_id,
                                );
                                self.push_display_message(DisplayMessage::system(notification.clone()));
                                // If not currently processing, inject as a message for the agent
                                if !self.is_processing {
                                    self.add_provider_message(Message {
                                        role: Role::User,
                                        content: vec![ContentBlock::Text {
                                            text: notification,
                                            cache_control: None,
                                        }],
                                        timestamp: Some(chrono::Utc::now()),
                                    });
                                    self.session.add_message(Role::User, vec![ContentBlock::Text {
                                        text: format!("[Background task {} completed]", task.task_id),
                                        cache_control: None,
                                    }]);
                                    let _ = self.session.save();
                                }
                            }
                            }
                            Ok(BusEvent::UsageReport(results)) => {
                                self.handle_usage_report(results);
                            }
                            Ok(BusEvent::LoginCompleted(login)) => {
                                self.handle_login_completed(login);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Extract memories from session before exiting (don't block on failure)
        self.extract_session_memories().await;

        Ok(RunResult {
            reload_session: self.reload_requested.take(),
            rebuild_session: self.rebuild_requested.take(),
            update_session: self.update_requested.take(),
            exit_code: self.requested_exit_code,
        })
    }

    /// Run the TUI in remote mode, connecting to a server
    pub async fn run_remote(mut self, mut terminal: DefaultTerminal) -> Result<RunResult> {
        use super::backend::RemoteConnection;

        let mut event_stream = EventStream::new();
        let mut redraw_period = super::redraw_interval(&self);
        let mut redraw_interval = interval(redraw_period);
        let mut reconnect_attempts = 0u32;
        let mut disconnect_msg_idx: Option<usize> = None;
        let mut disconnect_start: Option<std::time::Instant> = None;

        'outer: loop {
            let session_to_resume = self.reconnect_target_session_id();

            let mut remote = match RemoteConnection::connect_with_session(
                session_to_resume.as_deref(),
            )
            .await
            {
                Ok(r) => {
                    if let Some(idx) = disconnect_msg_idx.take() {
                        if idx < self.display_messages.len() {
                            self.display_messages.remove(idx);
                        }
                    }
                    disconnect_start = None;
                    r
                }
                Err(e) => {
                    if reconnect_attempts == 0 {
                        return Err(anyhow::anyhow!(
                            "Failed to connect to server. Is `jcode serve` running? Error: {}",
                            e
                        ));
                    }
                    reconnect_attempts += 1;

                    let elapsed = disconnect_start
                        .get_or_insert_with(std::time::Instant::now)
                        .elapsed();
                    let elapsed_str = if elapsed.as_secs() < 60 {
                        format!("{}s", elapsed.as_secs())
                    } else {
                        format!("{}m {}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
                    };

                    let session_name = self
                        .remote_session_id
                        .as_ref()
                        .and_then(|id| crate::id::extract_session_name(id))
                        .or_else(|| {
                            self.resume_session_id
                                .as_ref()
                                .and_then(|id| crate::id::extract_session_name(id))
                        });
                    let resume_hint = if let Some(name) = &session_name {
                        format!("  Resume later: jcode --resume {}", name)
                    } else {
                        String::new()
                    };

                    let msg_content = format!(
                        "⚡ Connection lost — retrying ({})\n  {}\n{}",
                        elapsed_str, e, resume_hint,
                    );

                    if let Some(idx) = disconnect_msg_idx {
                        if idx < self.display_messages.len() {
                            self.display_messages[idx].content = msg_content;
                        }
                    } else {
                        self.push_display_message(DisplayMessage {
                            role: "system".to_string(),
                            content: msg_content,
                            tool_calls: Vec::new(),
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                        disconnect_msg_idx = Some(self.display_messages.len() - 1);
                    }
                    terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;

                    let backoff = Duration::from_secs((1u64 << reconnect_attempts.min(5)).min(30));
                    let sleep = tokio::time::sleep(backoff);
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => break,
                            event = event_stream.next() => {
                                if let Some(Ok(Event::Key(key))) = event {
                                    if key.kind == KeyEventKind::Press {
                                        if key.code == KeyCode::Char('c')
                                            && key.modifiers.contains(KeyModifiers::CONTROL)
                                        {
                                            break 'outer;
                                        }
                                        if let Some(amount) = self
                                            .scroll_keys
                                            .scroll_amount(key.code.clone(), key.modifiers)
                                        {
                                            if amount < 0 {
                                                self.scroll_up((-amount) as usize);
                                            } else {
                                                self.scroll_down(amount as usize);
                                            }
                                            terminal
                                                .draw(|frame| crate::tui::ui::draw(frame, &self))?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
            };

            crate::logging::info(&format!(
                "Reload check: session_to_resume={:?}, remote_session_id={:?}, reconnect_attempts={}",
                session_to_resume, self.remote_session_id, reconnect_attempts
            ));
            let has_reload_ctx_for_session = session_to_resume
                .as_deref()
                .and_then(|sid| {
                    let result = ReloadContext::peek_for_session(sid);
                    crate::logging::info(&format!(
                        "Reload peek_for_session({}) = {:?}",
                        sid,
                        result.as_ref().map(|r| r.is_some())
                    ));
                    result.ok().flatten()
                })
                .is_some();

            // Check for per-session client-reload-pending marker (written when a
            // client re-exec breaks out before queuing the continuation message).
            let has_client_reload_marker = session_to_resume
                .as_deref()
                .and_then(|sid| {
                    let jcode_dir = crate::storage::jcode_dir().ok()?;
                    let marker = jcode_dir.join(format!("client-reload-pending-{}", sid));
                    if marker.exists() {
                        let info = std::fs::read_to_string(&marker).ok()?;
                        let _ = std::fs::remove_file(&marker);
                        crate::logging::info(&format!(
                            "Found client-reload-pending marker for {}, injecting reload info",
                            sid
                        ));
                        if self.reload_info.is_empty() {
                            for line in info.lines() {
                                let trimmed = line.trim();
                                if !trimmed.is_empty() {
                                    self.reload_info.push(trimmed.to_string());
                                }
                            }
                        }
                        Some(())
                    } else {
                        None
                    }
                })
                .is_some();

            // Show reconnection message if applicable
            if reconnect_attempts > 0 {
                if self.reload_info.is_empty() {
                    if let Ok(jcode_dir) = crate::storage::jcode_dir() {
                        let info_path = jcode_dir.join("reload-info");
                        if info_path.exists() {
                            if let Ok(info) = std::fs::read_to_string(&info_path) {
                                let _ = std::fs::remove_file(&info_path);
                                let trimmed = info.trim();
                                if let Some(hash) = trimmed.strip_prefix("reload:") {
                                    self.reload_info
                                        .push(format!("Reloaded with build {}", hash.trim()));
                                } else if let Some(hash) = trimmed.strip_prefix("rebuild:") {
                                    self.reload_info
                                        .push(format!("Rebuilt and reloaded ({})", hash.trim()));
                                } else if !trimmed.is_empty() {
                                    self.reload_info.push(trimmed.to_string());
                                }
                            }
                        }
                    }
                }

                // Check if client also needs to reload (newer binary available)
                if self.has_newer_binary() {
                    self.push_display_message(DisplayMessage::system(
                        "Server reloaded. Reloading client with newer binary...".to_string(),
                    ));
                    terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;
                    let session_id = self
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
                    // Save a per-session marker so the re-exec'd client knows to
                    // send a reload continuation message.  Without this, the
                    // continuation is lost because we break out before queuing it,
                    // and the re-exec'd client connects fresh (reconnect_attempts=0)
                    // with no reload-info file (already consumed above).
                    if has_reload_ctx_for_session || !self.reload_info.is_empty() {
                        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
                            let marker =
                                jcode_dir.join(format!("client-reload-pending-{}", session_id));
                            let info = if self.reload_info.is_empty() {
                                "reload".to_string()
                            } else {
                                self.reload_info.join("\n")
                            };
                            let _ = std::fs::write(&marker, &info);
                            crate::logging::info(&format!(
                                "Wrote client-reload-pending marker for {} before re-exec",
                                session_id
                            ));
                        }
                    }
                    self.reload_requested = Some(session_id);
                    self.should_quit = true;
                    break 'outer;
                }

                // Build success message with reload info if available
                let reload_details = if !self.reload_info.is_empty() {
                    format!("\n  {}", self.reload_info.join("\n  "))
                } else if has_reload_ctx_for_session {
                    "\n  Reload context restored".to_string()
                } else {
                    String::new()
                };

                self.push_display_message(DisplayMessage::system(format!(
                    "✓ Reconnected successfully.{}",
                    reload_details
                )));
            }

            // Queue message to notify the agent about reload completion.
            // Only queue a continuation for the session that actually initiated
            // the reload (has_reload_ctx_for_session) or was explicitly marked
            // for continuation (has_client_reload_marker). Other sessions that
            // reconnect after a server restart should NOT get a continuation
            // prompt - they were idle and have nothing to continue.
            let should_queue_reload_continuation =
                has_reload_ctx_for_session || has_client_reload_marker;
            crate::logging::info(&format!(
                "Reload continuation check: should_queue={}, reload_info_empty={}, has_ctx={}, has_marker={}",
                should_queue_reload_continuation,
                self.reload_info.is_empty(),
                has_reload_ctx_for_session,
                has_client_reload_marker
            ));
            if should_queue_reload_continuation {
                let reload_ctx = session_to_resume.as_deref().and_then(|sid| {
                    let result = ReloadContext::load_for_session(sid);
                    crate::logging::info(&format!(
                        "Reload load_for_session({}) = {:?}",
                        sid,
                        result.as_ref().map(|r| r.is_some())
                    ));
                    result.ok().flatten()
                });

                if let Some(ctx) = reload_ctx {
                    let task_info = ctx
                        .task_context
                        .map(|t| format!("\nTask context: {}", t))
                        .unwrap_or_default();

                    let continuation_msg = format!(
                        "[SYSTEM: Reload succeeded. Build {} → {}.{}\nIMPORTANT: The reload is done. You MUST immediately continue your work. Do NOT ask the user what to do next. Do NOT summarize what happened. Just pick up exactly where you left off and keep going.]",
                        ctx.version_before,
                        ctx.version_after,
                        task_info
                    );

                    crate::logging::info(&format!(
                        "Queuing reload continuation message ({} chars)",
                        continuation_msg.len()
                    ));
                    self.queued_messages.push(continuation_msg);
                } else {
                    crate::logging::warn(
                        "Reload context expected but not found (race condition?), skipping continuation"
                    );
                }
                self.reload_info.clear();
            }

            // Reset reconnect counter after handling reconnection
            reconnect_attempts = 0;

            let mut bus_receiver_remote = Bus::global().subscribe();

            // Main event loop
            loop {
                let desired_redraw = super::redraw_interval(&self);
                if desired_redraw != redraw_period {
                    redraw_period = desired_redraw;
                    redraw_interval = interval(redraw_period);
                }

                terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;

                if self.should_quit {
                    break 'outer;
                }

                tokio::select! {
                    _ = redraw_interval.tick() => {
                        // Flush stream buffer
                        if self.stream_buffer.should_flush() {
                            if let Some(chunk) = self.stream_buffer.flush() {
                                self.streaming_text.push_str(&chunk);
                            }
                        }
                        // Check for debug commands (remote mode)
                        let _ = self.check_debug_command_remote(&mut remote).await;
                        if let Some(reset_time) = self.rate_limit_reset {
                            if Instant::now() >= reset_time {
                                self.rate_limit_reset = None;
                                if !self.is_processing {
                                    if let Some(pending) = self.rate_limit_pending_message.clone() {
                                        self.push_display_message(DisplayMessage::system(format!(
                                            "✓ Rate limit reset. Retrying...{}",
                                            if pending.is_system { " (system message)" } else { "" }
                                        )));
                                        let _ = self
                                            .begin_remote_send(
                                                &mut remote,
                                                pending.content,
                                                pending.images,
                                                pending.is_system,
                                            )
                                            .await;
                                        continue;
                                    }
                                }
                            }
                        }
                        // Process queued messages (e.g. reload continuation)
                        if !self.is_processing && !self.queued_messages.is_empty() {
                            let messages = std::mem::take(&mut self.queued_messages);
                            let combined = messages.join("\n\n");
                            crate::logging::info(&format!("Sending queued continuation message ({} chars)", combined.len()));
                            for msg in &messages {
                                self.push_display_message(DisplayMessage::user(msg.clone()));
                            }
                            if self
                                .begin_remote_send(&mut remote, combined, vec![], true)
                                .await
                                .is_err()
                            {
                                crate::logging::error("Failed to send queued continuation message");
                            }
                        }
                        // Stall detection: if processing for too long with no server events,
                        // cancel and reset so the user isn't stuck forever.
                        const STALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
                        if self.is_processing {
                            let stalled = self.last_stream_activity
                                .map(|t| t.elapsed() > STALL_TIMEOUT)
                                .unwrap_or_else(|| {
                                    self.processing_started
                                        .map(|t| t.elapsed() > STALL_TIMEOUT)
                                        .unwrap_or(false)
                                });
                            if stalled {
                                crate::logging::warn(&format!(
                                    "Stream stall detected: no server events for {:?}, cancelling",
                                    self.last_stream_activity.map(|t| t.elapsed())
                                        .or(self.processing_started.map(|t| t.elapsed()))
                                ));
                                let _ = remote.cancel().await;
                                self.is_processing = false;
                                self.status = ProcessingStatus::Idle;
                                self.current_message_id = None;
                                self.processing_started = None;
                                self.last_stream_activity = None;
                                self.rate_limit_pending_message = None;
                                if !self.streaming_text.is_empty() {
                                    let content = self.take_streaming_text();
                                    self.push_display_message(DisplayMessage {
                                        role: "assistant".to_string(),
                                        content,
                                        tool_calls: vec![],
                                        duration_secs: None,
                                        title: None,
                                        tool_data: None,
                                    });
                                }
                                self.push_display_message(DisplayMessage::system(
                                    "⚠ Stream stalled (no response for 5 minutes). Processing cancelled. You can resend your message.".to_string()
                                ));
                            }
                        }
                    }
                    event = remote.next_event() => {
                        match event {
                            None => {
                                self.rate_limit_pending_message = None;
                                self.is_processing = false;
                                self.current_message_id = None;
                                self.last_stream_activity = None;
                                disconnect_start = Some(std::time::Instant::now());
                                self.push_display_message(DisplayMessage {
                                    role: "system".to_string(),
                                    content: "⚡ Connection lost — reconnecting…".to_string(),
                                    tool_calls: Vec::new(),
                                    duration_secs: None,
                                    title: None,
                                    tool_data: None,
                                });
                                disconnect_msg_idx = Some(self.display_messages.len() - 1);
                                terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;
                                reconnect_attempts = 1;
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                continue 'outer;
                            }
                            Some(server_event) => {
                                if let crate::protocol::ServerEvent::ClientDebugRequest {
                                    id,
                                    command,
                                } = server_event
                                {
                                    let output =
                                        self.handle_debug_command_remote(&command, &mut remote).await;
                                    let _ = remote.send_client_debug_response(id, output).await;
                                } else {
                                    let _ = self.handle_server_event(server_event, &mut remote);
                                }

                                // Process pending interleave or queued messages
                                // If processing: send any buffered interleave immediately as soft interrupt
                                // If not processing: send interleave or queued messages directly
                                if self.is_processing {
                                    if self.interleave_message.is_some() {
                                        // Flush any leftover interleave buffer (e.g. from debug commands)
                                        if let Some(interleave_msg) = self.interleave_message.take() {
                                            if !interleave_msg.trim().is_empty() {
                                                let msg_clone = interleave_msg.clone();
                                                if let Err(e) = remote.soft_interrupt(interleave_msg, false).await {
                                                    self.push_display_message(DisplayMessage::error(format!(
                                                        "Failed to queue soft interrupt: {}", e
                                                    )));
                                                } else {
                                                    self.pending_soft_interrupts.push(msg_clone);
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    // Not processing - send directly
                                    if let Some(interleave_msg) = self.interleave_message.take() {
                                        if !interleave_msg.trim().is_empty() {
                                            self.push_display_message(DisplayMessage {
                                                role: "user".to_string(),
                                                content: interleave_msg.clone(),
                                                tool_calls: vec![],
                                                duration_secs: None,
                                                title: None,
                                                tool_data: None,
                                            });
                                            match self
                                                .begin_remote_send(
                                                    &mut remote,
                                                    interleave_msg,
                                                    vec![],
                                                    false,
                                                )
                                                .await
                                            {
                                                Ok(_) => {}
                                                Err(e) => {
                                                    self.push_display_message(DisplayMessage::error(format!(
                                                        "Failed to send message: {}", e
                                                    )));
                                                }
                                            }
                                        }
                                    } else if !self.queued_messages.is_empty() {
                                        let messages = std::mem::take(&mut self.queued_messages);
                                        let combined = messages.join("\n\n");
                                        for msg in &messages {
                                            self.push_display_message(DisplayMessage::user(msg.clone()));
                                        }
                                        let _ = self
                                            .begin_remote_send(&mut remote, combined, vec![], true)
                                            .await;
                                    }
                                }
                            }
                        }
                    }
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key))) => {
                                if key.kind == KeyEventKind::Press {
                                    self.handle_remote_key(key.code, key.modifiers, &mut remote).await?;
                                    // Process deferred model switch from picker
                                    if let Some(spec) = self.pending_model_switch.take() {
                                        let _ = remote.set_model(&spec).await;
                                    }
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                            }
                            Some(Ok(Event::Mouse(mouse))) => {
                                self.handle_mouse_event(mouse);
                            }
                            _ => {}
                        }
                    }
                    bus_event = bus_receiver_remote.recv() => {
                        match bus_event {
                            Ok(BusEvent::UsageReport(results)) => {
                                self.handle_usage_report(results);
                            }
                            Ok(BusEvent::LoginCompleted(login)) => {
                                let success = login.success && login.provider != "copilot_code";
                                self.handle_login_completed(login);
                                if success {
                                    let _ = remote.notify_auth_changed().await;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(RunResult {
            reload_session: self.reload_requested.take(),
            rebuild_session: self.rebuild_requested.take(),
            update_session: self.update_requested.take(),
            exit_code: self.requested_exit_code,
        })
    }

    /// Run the TUI in replay mode, playing back a timeline of events.
    pub async fn run_replay(
        mut self,
        mut terminal: DefaultTerminal,
        timeline: Vec<crate::replay::TimelineEvent>,
        speed: f64,
    ) -> Result<RunResult> {
        use super::backend::RemoteConnection;
        use crate::replay::ReplayEvent;

        let mut event_stream = EventStream::new();
        let mut redraw_period = super::redraw_interval(&self);
        let mut redraw_interval = interval(redraw_period);
        let mut remote = RemoteConnection::dummy();

        let replay_events = crate::replay::timeline_to_replay_events(&timeline);

        let mut event_index: usize = 0;
        let mut paused = false;
        let mut replay_speed = speed;
        let mut next_event_at: Option<tokio::time::Instant> = Some(tokio::time::Instant::now());
        let mut replay_turn_id: u64 = 0;

        loop {
            let desired_redraw = super::redraw_interval(&self);
            if desired_redraw != redraw_period {
                redraw_period = desired_redraw;
                redraw_interval = interval(redraw_period);
            }

            terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;

            if self.should_quit {
                break;
            }

            let replay_done = event_index >= replay_events.len();

            tokio::select! {
                _ = redraw_interval.tick() => {
                    if self.stream_buffer.should_flush() {
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                    }
                }
                event = event_stream.next() => {
                    if let Some(Ok(event)) = event {
                        match event {
                            Event::Key(key) if key.kind == KeyEventKind::Press => {
                                match key.code {
                                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                        self.should_quit = true;
                                    }
                                    KeyCode::Char('q') | KeyCode::Esc => {
                                        self.should_quit = true;
                                    }
                                    KeyCode::Char(' ') => {
                                        paused = !paused;
                                        if !paused && !replay_done {
                                            next_event_at = Some(tokio::time::Instant::now());
                                        }
                                    }
                                    KeyCode::Char('+') | KeyCode::Char('=') => {
                                        replay_speed = (replay_speed * 1.5).min(20.0);
                                    }
                                    KeyCode::Char('-') => {
                                        replay_speed = (replay_speed / 1.5).max(0.1);
                                    }
                                    _ => {
                                        if let Some(amount) = self
                                            .scroll_keys
                                            .scroll_amount(key.code.clone(), key.modifiers)
                                        {
                                            if amount < 0 {
                                                self.scroll_up((-amount) as usize);
                                            } else {
                                                self.scroll_down(amount as usize);
                                            }
                                        }
                                    }
                                }
                            }
                            Event::Mouse(mouse) => {
                                self.handle_mouse_event(mouse);
                            }
                            _ => {}
                        }
                    }
                }
                _ = async {
                    if let Some(target) = next_event_at {
                        tokio::time::sleep_until(target).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if !paused && !replay_done => {
                    if event_index < replay_events.len() {
                        let replay_event = replay_events[event_index].1.clone();

                        match replay_event {
                            ReplayEvent::UserMessage { text } => {
                                self.push_display_message(DisplayMessage {
                                    role: "user".to_string(),
                                    content: text,
                                    tool_calls: vec![],
                                    duration_secs: None,
                                    title: None,
                                    tool_data: None,
                                });
                            }
                            ReplayEvent::StartProcessing => {
                                replay_turn_id += 1;
                                self.current_message_id = Some(replay_turn_id);
                                self.is_processing = true;
                                self.processing_started = Some(Instant::now());
                                self.status = ProcessingStatus::Thinking(Instant::now());
                                self.streaming_tps_start = Some(Instant::now());
                                self.streaming_tps_elapsed = std::time::Duration::ZERO;
                                self.streaming_total_output_tokens = 0;
                            }
                            ReplayEvent::Server(server_event) => {
                                if let crate::protocol::ServerEvent::TextDelta { ref text } = server_event {
                                    if !text.is_empty() {
                                        self.streaming_text.push_str(text);
                                        if matches!(self.status, ProcessingStatus::Thinking(_)) {
                                            self.status = ProcessingStatus::Streaming;
                                        }
                                        self.last_stream_activity = Some(Instant::now());
                                    }
                                } else {
                                    self.handle_server_event(server_event, &mut remote);
                                }
                            }
                        }

                        event_index += 1;

                        if event_index < replay_events.len() {
                            let next_delay = replay_events[event_index].0;
                            let adjusted = (next_delay as f64 / replay_speed) as u64;
                            next_event_at = Some(tokio::time::Instant::now() + Duration::from_millis(adjusted));
                        } else {
                            next_event_at = None;
                            self.is_processing = false;
                            self.status = ProcessingStatus::Idle;
                        }
                    }
                }
            }
        }

        Ok(RunResult {
            reload_session: None,
            rebuild_session: None,
            update_session: None,
            exit_code: None,
        })
    }

    /// Run replay headlessly, rendering each frame to an in-memory buffer.
    /// Returns a list of (timestamp_secs, Buffer) pairs for video export.
    pub async fn run_headless_replay(
        mut self,
        timeline: &[crate::replay::TimelineEvent],
        speed: f64,
        width: u16,
        height: u16,
        fps: u32,
    ) -> Result<Vec<(f64, ratatui::buffer::Buffer)>> {
        use crate::replay::ReplayEvent;
        use ratatui::backend::TestBackend;

        let replay_events = crate::replay::timeline_to_replay_events(timeline);
        if replay_events.is_empty() {
            anyhow::bail!("No replay events to export");
        }

        let backend = TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend)?;
        let mut remote = super::backend::RemoteConnection::dummy();

        let frame_duration_ms: f64 = 1000.0 / fps as f64;
        let mut frames: Vec<(f64, ratatui::buffer::Buffer)> = Vec::new();
        let mut sim_time_ms: f64 = 0.0;
        let mut next_frame_at: f64 = 0.0;

        let total_duration_ms: f64 = replay_events.iter().map(|(d, _)| *d as f64 / speed).sum();

        let mut event_schedule: Vec<(f64, &ReplayEvent)> = Vec::new();
        {
            let mut abs_time: f64 = 0.0;
            for (delay_ms, evt) in &replay_events {
                abs_time += *delay_ms as f64 / speed;
                event_schedule.push((abs_time, evt));
            }
        }

        let mut event_cursor: usize = 0;
        let mut replay_turn_id: u64 = 0;

        terminal.draw(|f| crate::tui::render_frame(f, &self))?;
        frames.push((0.0, terminal.backend().buffer().clone()));

        let progress_interval = (total_duration_ms / 20.0).max(1000.0);
        let mut next_progress = progress_interval;

        while sim_time_ms <= total_duration_ms + frame_duration_ms {
            while event_cursor < event_schedule.len()
                && event_schedule[event_cursor].0 <= sim_time_ms
            {
                let (_t, event) = event_schedule[event_cursor];
                match event {
                    ReplayEvent::UserMessage { text } => {
                        self.push_display_message(DisplayMessage {
                            role: "user".to_string(),
                            content: text.clone(),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                    }
                    ReplayEvent::StartProcessing => {
                        replay_turn_id += 1;
                        self.current_message_id = Some(replay_turn_id);
                        self.is_processing = true;
                        self.processing_started = Some(Instant::now());
                        self.status = ProcessingStatus::Thinking(Instant::now());
                        self.replay_processing_started_ms = Some(sim_time_ms);
                    }
                    ReplayEvent::Server(server_event) => {
                        if let crate::protocol::ServerEvent::TextDelta { ref text } = server_event {
                            if !text.is_empty() {
                                self.streaming_text.push_str(text);
                                if matches!(self.status, ProcessingStatus::Thinking(_)) {
                                    self.status = ProcessingStatus::Streaming;
                                }
                            }
                        } else {
                            self.handle_server_event(server_event.clone(), &mut remote);
                        }
                    }
                }
                event_cursor += 1;
            }

            if sim_time_ms >= next_frame_at {
                if let Some(start_ms) = self.replay_processing_started_ms {
                    let elapsed_ms = (sim_time_ms - start_ms).max(0.0);
                    self.replay_elapsed_override = Some(Duration::from_millis(elapsed_ms as u64));
                } else {
                    self.replay_elapsed_override = None;
                }
                terminal.draw(|f| crate::tui::render_frame(f, &self))?;
                frames.push((sim_time_ms / 1000.0, terminal.backend().buffer().clone()));
                next_frame_at = sim_time_ms + frame_duration_ms;
            }

            if sim_time_ms >= next_progress {
                let pct = (sim_time_ms / total_duration_ms * 100.0).min(100.0);
                eprint!("\r  Rendering... {:.0}%", pct);
                next_progress += progress_interval;
            }

            sim_time_ms += frame_duration_ms;
        }

        eprintln!("\r  Rendering... 100%  ({} frames captured)", frames.len());

        Ok(frames)
    }

    /// Handle a server event. Returns true if we're at a "safe point" for interleaving
    /// (after a tool completes but before the turn ends).
    fn handle_server_event(
        &mut self,
        event: crate::protocol::ServerEvent,
        remote: &mut super::backend::RemoteConnection,
    ) -> bool {
        use crate::protocol::ServerEvent;

        if self.is_processing {
            self.last_stream_activity = Some(Instant::now());
        }

        let call_output_tokens_seen = remote.call_output_tokens_seen();

        match event {
            ServerEvent::TextDelta { text } => {
                if let Some(thought_line) = Self::extract_thought_line(&text) {
                    if let Some(chunk) = self.stream_buffer.flush() {
                        self.streaming_text.push_str(&chunk);
                    }
                    self.insert_thought_line(thought_line);
                    return false;
                }
                if matches!(
                    self.status,
                    ProcessingStatus::Sending | ProcessingStatus::Connecting(_)
                ) {
                    self.status = ProcessingStatus::Streaming;
                } else if matches!(self.status, ProcessingStatus::Thinking(_)) {
                    self.status = ProcessingStatus::Streaming;
                } else if self.is_processing && matches!(self.status, ProcessingStatus::Idle) {
                    self.status = ProcessingStatus::Streaming;
                }
                if self.streaming_tps_start.is_none() {
                    self.streaming_tps_start = Some(Instant::now());
                }
                if let Some(chunk) = self.stream_buffer.push(&text) {
                    self.streaming_text.push_str(&chunk);
                }
                self.last_stream_activity = Some(Instant::now());
                false
            }
            ServerEvent::TextReplace { text } => {
                self.stream_buffer.flush();
                self.streaming_text = text;
                false
            }
            ServerEvent::ToolStart { id, name } => {
                if self.streaming_tps_start.is_none() {
                    self.streaming_tps_start = Some(Instant::now());
                }
                remote.handle_tool_start(&id, &name);
                if matches!(name.as_str(), "memory" | "remember") {
                    crate::memory::set_state(crate::tui::info_widget::MemoryState::Embedding);
                }
                self.status = ProcessingStatus::RunningTool(name.clone());
                self.streaming_tool_calls.push(ToolCall {
                    id,
                    name,
                    input: serde_json::Value::Null,
                    intent: None,
                });
                false
            }
            ServerEvent::ToolInput { delta } => {
                remote.handle_tool_input(&delta);
                false
            }
            ServerEvent::ToolExec { id, name } => {
                if let Some(start) = self.streaming_tps_start.take() {
                    self.streaming_tps_elapsed += start.elapsed();
                }
                // Update streaming_tool_calls with parsed input before clearing
                let parsed_input = remote.get_current_tool_input();
                if let Some(tc) = self.streaming_tool_calls.iter_mut().find(|tc| tc.id == id) {
                    tc.input = parsed_input.clone();
                }
                remote.handle_tool_exec(&id, &name);
                false
            }
            ServerEvent::ToolDone {
                id,
                name,
                output,
                error,
            } => {
                let _ = error; // Currently unused
                let display_output = remote.handle_tool_done(&id, &name, &output);
                // Get the tool input from streaming_tool_calls (stored in ToolExec)
                let tool_input = self
                    .streaming_tool_calls
                    .iter()
                    .find(|tc| tc.id == id)
                    .map(|tc| tc.input.clone())
                    .unwrap_or(serde_json::Value::Null);
                // Flush stream buffer
                if let Some(chunk) = self.stream_buffer.flush() {
                    self.streaming_text.push_str(&chunk);
                }
                // Commit streaming text as assistant message
                if !self.streaming_text.is_empty() {
                    let content = self.take_streaming_text();
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content,
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
                crate::tui::mermaid::clear_streaming_preview_diagram();
                // Add tool result message
                self.push_display_message(DisplayMessage {
                    role: "tool".to_string(),
                    content: display_output,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: Some(ToolCall {
                        id,
                        name,
                        input: tool_input,
                        intent: None,
                    }),
                });
                self.streaming_tool_calls.clear();
                self.status = ProcessingStatus::Streaming;
                // This is a safe point to interleave messages
                true
            }
            ServerEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                cache_creation_input,
            } => {
                self.accumulate_streaming_output_tokens(output, call_output_tokens_seen);
                self.streaming_input_tokens = input;
                self.streaming_output_tokens = output;
                if cache_read_input.is_some() {
                    self.streaming_cache_read_tokens = cache_read_input;
                }
                if cache_creation_input.is_some() {
                    self.streaming_cache_creation_tokens = cache_creation_input;
                }
                false
            }
            ServerEvent::ConnectionType { connection } => {
                self.connection_type = Some(connection);
                false
            }
            ServerEvent::ConnectionPhase { phase } => {
                let cp = match phase.as_str() {
                    "authenticating" => crate::message::ConnectionPhase::Authenticating,
                    "connecting" => crate::message::ConnectionPhase::Connecting,
                    "waiting for response" => crate::message::ConnectionPhase::WaitingForResponse,
                    "streaming" => crate::message::ConnectionPhase::Streaming,
                    _ => crate::message::ConnectionPhase::Connecting,
                };
                self.status = ProcessingStatus::Connecting(cp);
                false
            }
            ServerEvent::UpstreamProvider { provider } => {
                self.upstream_provider = Some(provider);
                false
            }
            ServerEvent::Interrupted => {
                // Treat interruption as a system/status event, not assistant output.
                // But preserve partial streamed content as a display message.
                self.rate_limit_pending_message = None;
                self.interleave_message = None;
                self.pending_soft_interrupts.clear();
                if let Some(chunk) = self.stream_buffer.flush() {
                    self.streaming_text.push_str(&chunk);
                }
                if !self.streaming_text.is_empty() {
                    let content = self.take_streaming_text();
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content,
                        tool_calls: Vec::new(),
                        duration_secs: self.processing_started.map(|t| t.elapsed().as_secs_f32()),
                        title: None,
                        tool_data: None,
                    });
                }
                self.clear_streaming_render_state();
                self.stream_buffer.clear();
                self.streaming_tool_calls.clear();
                self.thought_line_inserted = false;
                self.thinking_prefix_emitted = false;
                self.thinking_buffer.clear();
                self.push_display_message(DisplayMessage::system("Interrupted"));
                self.is_processing = false;
                self.status = ProcessingStatus::Idle;
                self.processing_started = None;
                self.current_message_id = None;
                remote.clear_pending();
                remote.reset_call_output_tokens_seen();
                false
            }
            ServerEvent::Done { id } => {
                crate::logging::info(&format!(
                    "Client received Done id={}, current_message_id={:?}",
                    id, self.current_message_id
                ));
                if self.current_message_id == Some(id) {
                    self.rate_limit_pending_message = None;
                    if let Some(chunk) = self.stream_buffer.flush() {
                        self.streaming_text.push_str(&chunk);
                    }
                    if let Some(start) = self.streaming_tps_start.take() {
                        self.streaming_tps_elapsed += start.elapsed();
                    }
                    if !self.streaming_text.is_empty() {
                        let duration = self.processing_started.map(|s| s.elapsed().as_secs_f32());
                        let content = self.take_streaming_text();
                        self.push_display_message(DisplayMessage {
                            role: "assistant".to_string(),
                            content,
                            tool_calls: vec![],
                            duration_secs: duration,
                            title: None,
                            tool_data: None,
                        });
                        self.push_turn_footer(duration);
                    }
                    crate::tui::mermaid::clear_streaming_preview_diagram();
                    self.rate_limit_pending_message = None;
                    self.is_processing = false;
                    self.status = ProcessingStatus::Idle;
                    self.processing_started = None;
                    self.replay_processing_started_ms = None;
                    self.replay_elapsed_override = None;
                    self.streaming_tool_calls.clear();
                    self.current_message_id = None;
                    self.thought_line_inserted = false;
                    self.thinking_prefix_emitted = false;
                    self.thinking_buffer.clear();
                    remote.clear_pending();
                    remote.reset_call_output_tokens_seen();
                } else if self.is_processing {
                    let is_stale = self.current_message_id.map_or(false, |mid| id < mid);
                    if is_stale {
                        crate::logging::info(&format!(
                            "Ignoring stale Done id={} (current_message_id={:?}), likely from Subscribe/ResumeSession",
                            id, self.current_message_id
                        ));
                    } else {
                        crate::logging::warn(&format!(
                            "Done id={} doesn't match current_message_id={:?} but is_processing=true, forcing idle",
                            id, self.current_message_id
                        ));
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        if !self.streaming_text.is_empty() {
                            let duration =
                                self.processing_started.map(|s| s.elapsed().as_secs_f32());
                            let content = self.take_streaming_text();
                            self.push_display_message(DisplayMessage {
                                role: "assistant".to_string(),
                                content,
                                tool_calls: vec![],
                                duration_secs: duration,
                                title: None,
                                tool_data: None,
                            });
                            self.push_turn_footer(duration);
                        }
                        crate::tui::mermaid::clear_streaming_preview_diagram();
                        self.is_processing = false;
                        self.status = ProcessingStatus::Idle;
                        self.processing_started = None;
                        self.replay_processing_started_ms = None;
                        self.replay_elapsed_override = None;
                        self.streaming_tool_calls.clear();
                        self.current_message_id = None;
                        self.thought_line_inserted = false;
                        self.thinking_prefix_emitted = false;
                        self.thinking_buffer.clear();
                        remote.clear_pending();
                        remote.reset_call_output_tokens_seen();
                    }
                }
                false
            }
            ServerEvent::Error {
                message,
                retry_after_secs,
                ..
            } => {
                let reset_duration = retry_after_secs
                    .map(Duration::from_secs)
                    .or_else(|| parse_rate_limit_error(&message));
                if let Some(reset_duration) = reset_duration {
                    self.rate_limit_reset = Some(Instant::now() + reset_duration);
                    if let Some(is_system) = self
                        .rate_limit_pending_message
                        .as_ref()
                        .map(|pending| pending.is_system)
                    {
                        self.push_display_message(DisplayMessage::system(format!(
                            "⏳ Rate limit hit. Will auto-retry in {} seconds...",
                            reset_duration.as_secs()
                        )));
                        if is_system {
                            self.set_status_notice("Rate limited; queued system retry");
                        } else {
                            self.set_status_notice("Rate limited; queued retry");
                        }
                        self.is_processing = false;
                        self.status = ProcessingStatus::Idle;
                        self.processing_started = None;
                        self.current_message_id = None;
                        remote.clear_pending();
                        remote.reset_call_output_tokens_seen();
                        return false;
                    }
                }
                self.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: message,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                self.rate_limit_pending_message = None;
                self.is_processing = false;
                self.status = ProcessingStatus::Idle;
                self.interleave_message = None;
                self.pending_soft_interrupts.clear();
                crate::tui::mermaid::clear_streaming_preview_diagram();
                self.thought_line_inserted = false;
                self.thinking_prefix_emitted = false;
                self.thinking_buffer.clear();
                remote.clear_pending();
                remote.reset_call_output_tokens_seen();
                false
            }
            ServerEvent::SessionId { session_id } => {
                remote.set_session_id(session_id.clone());
                self.remote_session_id = Some(session_id);
                self.update_terminal_title();
                false
            }
            ServerEvent::Reloading { .. } => {
                self.append_reload_message("🔄 Server reload initiated...");
                false
            }
            ServerEvent::ReloadProgress {
                step,
                message,
                success,
                output,
            } => {
                // Coalesce progress lines into one reload message to avoid extra spacing.
                let mut content = if let Some(ok) = success {
                    let status_icon = if ok { "✓" } else { "✗" };
                    format!("[{}] {} {}", step, status_icon, message)
                } else {
                    format!("[{}] {}", step, message)
                };

                if let Some(out) = output {
                    if !out.is_empty() {
                        content.push_str("\n```\n");
                        content.push_str(&out);
                        content.push_str("\n```");
                    }
                }

                self.append_reload_message(&content);

                // Store key reload info for agent notification after reconnect
                if step == "verify" || step == "git" {
                    self.reload_info.push(message.clone());
                }

                // Update status notice
                self.status_notice =
                    Some((format!("Reload: {}", message), std::time::Instant::now()));
                false
            }
            ServerEvent::History {
                messages,
                session_id,
                provider_name,
                provider_model,
                available_models,
                mcp_servers,
                skills,
                all_sessions,
                client_count,
                is_canary,
                server_version,
                server_name,
                server_icon,
                server_has_update,
                was_interrupted,
                ..
            } => {
                let prev_session_id = self.remote_session_id.clone();
                remote.set_session_id(session_id.clone());
                self.remote_session_id = Some(session_id.clone());
                let session_changed = prev_session_id.as_deref() != Some(session_id.as_str());

                if session_changed {
                    self.rate_limit_pending_message = None;
                    self.rate_limit_reset = None;
                    self.clear_display_messages();
                    self.clear_streaming_render_state();
                    self.streaming_tool_calls.clear();
                    self.thought_line_inserted = false;
                    self.thinking_prefix_emitted = false;
                    self.thinking_buffer.clear();
                    self.streaming_input_tokens = 0;
                    self.streaming_output_tokens = 0;
                    self.streaming_cache_read_tokens = None;
                    self.streaming_cache_creation_tokens = None;
                    self.processing_started = None;
                    self.replay_processing_started_ms = None;
                    self.replay_elapsed_override = None;
                    self.streaming_tps_start = None;
                    self.streaming_tps_elapsed = Duration::ZERO;
                    self.streaming_total_output_tokens = 0;
                    self.last_stream_activity = None;
                    self.is_processing = false;
                    self.status = ProcessingStatus::Idle;
                    self.follow_chat_bottom();
                    // Only clear queued messages when switching FROM a known session.
                    // When prev_session_id is None (initial connect / resume after reload),
                    // preserve queued messages — they may contain reload continuation messages
                    // that were queued before History arrived.
                    if prev_session_id.is_some() {
                        self.queued_messages.clear();
                    }
                    self.interleave_message = None;
                    self.pending_soft_interrupts.clear();
                    self.remote_total_tokens = None;
                    self.remote_swarm_members.clear();
                    self.swarm_plan_items.clear();
                    self.swarm_plan_version = None;
                    self.swarm_plan_swarm_id = None;
                    self.connection_type = None;
                    remote.reset_call_output_tokens_seen();
                }
                // Store provider info for UI display
                if let Some(name) = provider_name {
                    self.remote_provider_name = Some(name);
                }
                if let Some(model) = provider_model {
                    self.update_context_limit_for_model(&model);
                    self.remote_provider_model = Some(model);
                }
                self.remote_available_models = available_models;
                // Store session list and client count
                self.remote_sessions = all_sessions;
                self.remote_client_count = client_count;
                self.remote_is_canary = is_canary;
                self.remote_server_version = server_version;
                self.remote_server_has_update = server_has_update;
                self.remote_server_short_name = server_name;
                if let Some(icon) = server_icon {
                    self.remote_server_icon = Some(icon);
                }

                // Update terminal title (always, since server name may have arrived)
                self.update_terminal_title();

                // Parse MCP servers from "name:count" format
                if !mcp_servers.is_empty() {
                    self.mcp_server_names = mcp_servers
                        .iter()
                        .filter_map(|s| {
                            let (name, count_str) = s.split_once(':')?;
                            let count = count_str.parse::<usize>().unwrap_or(0);
                            Some((name.to_string(), count))
                        })
                        .collect();
                }

                if session_changed || !remote.has_loaded_history() {
                    remote.mark_history_loaded();
                    for msg in messages {
                        self.push_display_message(DisplayMessage {
                            role: msg.role,
                            content: msg.content,
                            tool_calls: msg.tool_calls.unwrap_or_default(),
                            duration_secs: None,
                            title: None,
                            tool_data: msg.tool_data,
                        });
                    }
                }

                if was_interrupted == Some(true) && !self.display_messages.is_empty() {
                    crate::logging::info(
                        "Session was interrupted mid-generation, queuing continuation",
                    );
                    self.push_display_message(DisplayMessage::system(
                        "⚡ Session was interrupted mid-generation. Continuing...".to_string(),
                    ));
                    self.queued_messages.push(
                        "[SYSTEM: Your previous session was interrupted while you were generating a response. \
                         The session has been restored. Please continue exactly where you left off. \
                         Look at the conversation history to understand what you were doing and resume immediately.]"
                            .to_string(),
                    );
                }

                false
            }
            ServerEvent::SwarmStatus { members } => {
                if self.swarm_enabled {
                    self.remote_swarm_members = members;
                } else {
                    self.remote_swarm_members.clear();
                }
                false
            }
            ServerEvent::SwarmPlan {
                swarm_id,
                version,
                items,
                ..
            } => {
                self.swarm_plan_swarm_id = Some(swarm_id);
                self.swarm_plan_version = Some(version);
                self.swarm_plan_items = items;
                self.set_status_notice(format!(
                    "Swarm plan synced (v{}, {} items)",
                    version,
                    self.swarm_plan_items.len()
                ));
                false
            }
            ServerEvent::SwarmPlanProposal {
                swarm_id,
                proposer_session,
                proposer_name,
                summary,
                ..
            } => {
                let proposer = proposer_name
                    .unwrap_or_else(|| proposer_session.chars().take(8).collect::<String>());
                self.push_display_message(DisplayMessage::system(format!(
                    "Plan proposal received in swarm {}\nFrom: {}\nSummary: {}",
                    swarm_id, proposer, summary
                )));
                self.set_status_notice("Plan proposal received");
                false
            }
            ServerEvent::McpStatus { servers } => {
                // Parse MCP servers from "name:count" format
                self.mcp_server_names = servers
                    .iter()
                    .filter_map(|s| {
                        let (name, count_str) = s.split_once(':')?;
                        let count = count_str.parse::<usize>().unwrap_or(0);
                        Some((name.to_string(), count))
                    })
                    .collect();
                false
            }
            ServerEvent::ModelChanged {
                model,
                provider_name,
                error,
                ..
            } => {
                if let Some(err) = error {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Failed to switch model: {}",
                        err
                    )));
                    self.set_status_notice("Model switch failed");
                } else {
                    self.update_context_limit_for_model(&model);
                    self.remote_provider_model = Some(model.clone());
                    if let Some(ref pname) = provider_name {
                        self.remote_provider_name = Some(pname.clone());
                    }
                    self.connection_type = None;
                    self.push_display_message(DisplayMessage::system(format!(
                        "✓ Switched to model: {}",
                        model
                    )));
                    self.set_status_notice(format!("Model → {}", model));
                }
                false
            }
            ServerEvent::AvailableModelsUpdated { available_models } => {
                self.remote_available_models = available_models;
                false
            }
            ServerEvent::SoftInterruptInjected {
                content,
                point: _,
                tools_skipped,
            } => {
                // Flush any in-progress streaming text as an assistant message first,
                // so the interleave message appears AFTER the preceding response.
                if let Some(chunk) = self.stream_buffer.flush() {
                    self.streaming_text.push_str(&chunk);
                }
                if !self.streaming_text.is_empty() {
                    let duration = self.processing_started.map(|s| s.elapsed().as_secs_f32());
                    let flushed = self.take_streaming_text();
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: flushed,
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
                // When injected, NOW add the message to display_messages
                // (it was previously only in the queue preview area)
                self.pending_soft_interrupts.clear();
                self.push_display_message(DisplayMessage {
                    role: "user".to_string(),
                    content: content.clone(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                // Only show status notice if tools were skipped (urgent interrupt)
                if let Some(n) = tools_skipped {
                    self.set_status_notice(format!("⚡ {} tool(s) skipped", n));
                }
                false
            }
            ServerEvent::MemoryInjected {
                count,
                prompt,
                prompt_chars,
                computed_age_ms,
            } => {
                if self.memory_enabled {
                    let plural = if count == 1 { "memory" } else { "memories" };
                    let display_prompt = if prompt.trim().is_empty() {
                        "# Memory\n\n## Notes\n1. (content unavailable from server event)"
                            .to_string()
                    } else {
                        prompt.clone()
                    };
                    let display_chars = if prompt_chars == 0 {
                        display_prompt.chars().count()
                    } else {
                        prompt_chars
                    };
                    crate::memory::record_injected_prompt(&display_prompt, count, computed_age_ms);
                    self.push_display_message(DisplayMessage::system(format!(
                        "🧠 Injected {} {} into context ({} chars, computed {}ms ago)\n\n---\n\n{}",
                        count, plural, display_chars, computed_age_ms, display_prompt
                    )));
                    self.set_status_notice(format!("🧠 {} relevant {} injected", count, plural));
                }
                false
            }
            ServerEvent::SplitResponse {
                new_session_id,
                new_session_name,
                ..
            } => {
                let exe = std::env::current_exe().unwrap_or_default();
                let cwd = std::env::current_dir().unwrap_or_default();
                let socket = std::env::var("JCODE_SOCKET").ok();
                match spawn_in_new_terminal(&exe, &new_session_id, &cwd, socket.as_deref()) {
                    Ok(true) => {
                        self.push_display_message(DisplayMessage::system(format!(
                            "✂ Split → **{}** (opened in new window)",
                            new_session_name,
                        )));
                        self.set_status_notice(format!("Split → {}", new_session_name));
                    }
                    Ok(false) => {
                        self.push_display_message(DisplayMessage::system(format!(
                            "✂ Split → **{}**\n\nNo terminal found. Resume manually:\n```\njcode --resume {}\n```",
                            new_session_name, new_session_id,
                        )));
                    }
                    Err(e) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Split created **{}** but failed to open window: {}\n\nResume manually: `jcode --resume {}`",
                            new_session_name, e, new_session_id,
                        )));
                    }
                }
                false
            }
            ServerEvent::CompactResult {
                message, success, ..
            } => {
                if success {
                    self.push_display_message(DisplayMessage::system(message));
                    self.set_status_notice("📦 Compaction started");
                } else {
                    self.push_display_message(DisplayMessage::system(message));
                    self.set_status_notice("Compaction failed");
                }
                false
            }
            ServerEvent::StdinRequest { .. } => {
                self.set_status_notice("⌨ Interactive terminal detected (command will timeout)");
                false
            }
            _ => false,
        }
    }

    fn handle_remote_char_input(&mut self, c: char) {
        if self.input.is_empty() && !self.is_processing && self.display_messages.is_empty() {
            if let Some(digit) = c.to_digit(10) {
                let suggestions = self.suggestion_prompts();
                let idx = digit as usize;
                if idx >= 1 && idx <= suggestions.len() {
                    let (_label, prompt) = &suggestions[idx - 1];
                    if !prompt.starts_with('/') {
                        self.input = prompt.clone();
                        self.cursor_pos = self.input.len();
                        self.follow_chat_bottom();
                        return;
                    }
                }
            }
        }
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
        self.follow_chat_bottom();
        self.reset_tab_completion();
        self.sync_model_picker_preview_from_input();
    }

    /// Handle keyboard input in remote mode
    async fn handle_remote_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: &mut super::backend::RemoteConnection,
    ) -> Result<()> {
        let mut code = code;
        let mut modifiers = modifiers;
        ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

        if self.changelog_scroll.is_some() {
            return self.handle_changelog_key(code);
        }

        // If picker is active and not in preview mode, handle picker keys first
        if let Some(ref picker) = self.picker_state {
            if !picker.preview {
                return self.handle_picker_key(code, modifiers);
            }
        }
        // Preview-mode picker: arrow keys navigate without leaving preview
        if self.handle_picker_preview_key(&code, modifiers)? {
            return Ok(());
        }

        if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('m')) {
            self.toggle_diagram_pane();
            return Ok(());
        }
        if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('t')) {
            self.toggle_diagram_pane_position();
            return Ok(());
        }
        if let Some(direction) = self
            .model_switch_keys
            .direction_for(code.clone(), modifiers)
        {
            remote.cycle_model(direction).await?;
            return Ok(());
        }
        if let Some(direction) = self
            .effort_switch_keys
            .direction_for(code.clone(), modifiers)
        {
            self.cycle_effort(direction);
            return Ok(());
        }
        if self
            .centered_toggle_keys
            .toggle
            .matches(code.clone(), modifiers)
        {
            self.toggle_centered_mode();
            return Ok(());
        }
        self.normalize_diagram_state();
        let diagram_available = self.diagram_available();
        if self.handle_diagram_focus_key(code.clone(), modifiers, diagram_available) {
            return Ok(());
        }
        if self.handle_diff_pane_focus_key(code.clone(), modifiers) {
            return Ok(());
        }
        // Most key handling is the same as local mode
        // Handle Alt combos
        if modifiers.contains(KeyModifiers::ALT) {
            match code {
                KeyCode::Char('b') => {
                    if matches!(self.status, ProcessingStatus::RunningTool(_)) {
                        remote.background_tool().await?;
                        self.set_status_notice("Moving tool to background...");
                        return Ok(());
                    }
                    self.cursor_pos = self.find_word_boundary_back();
                    return Ok(());
                }
                KeyCode::Char('f') => {
                    self.cursor_pos = self.find_word_boundary_forward();
                    return Ok(());
                }
                KeyCode::Char('d') => {
                    let end = self.find_word_boundary_forward();
                    self.input.drain(self.cursor_pos..end);
                    return Ok(());
                }
                KeyCode::Backspace => {
                    let start = self.find_word_boundary_back();
                    self.input.drain(start..self.cursor_pos);
                    self.cursor_pos = start;
                    return Ok(());
                }
                KeyCode::Char('v') => {
                    // Alt+V: paste image from clipboard
                    self.paste_image_from_clipboard();
                    return Ok(());
                }
                _ => {}
            }
        }

        // Handle configurable scroll keys (default: Ctrl+K/J, Alt+U/D for page)
        if let Some(amount) = self.scroll_keys.scroll_amount(code.clone(), modifiers) {
            if amount < 0 {
                self.scroll_up((-amount) as usize);
            } else {
                self.scroll_down(amount as usize);
            }
            return Ok(());
        }

        // Handle prompt jump keys (default: Ctrl+[/], plus Ctrl+1..9 recency)
        if let Some(dir) = self.scroll_keys.prompt_jump(code.clone(), modifiers) {
            if dir < 0 {
                self.scroll_to_prev_prompt();
            } else {
                self.scroll_to_next_prompt();
            }
            return Ok(());
        }

        if let Some(rank) = Self::ctrl_prompt_rank(&code, modifiers) {
            self.scroll_to_recent_prompt_rank(rank);
            return Ok(());
        }

        // Handle centered mode toggle (default: Alt+C)
        if self
            .centered_toggle_keys
            .toggle
            .matches(code.clone(), modifiers)
        {
            self.toggle_centered_mode();
            return Ok(());
        }

        // Handle scroll bookmark toggle (default: Ctrl+G)
        if self.scroll_keys.is_bookmark(code.clone(), modifiers) {
            self.toggle_scroll_bookmark();
            return Ok(());
        }

        // Shift+Tab: cycle diff mode (Off → Inline → Pinned)
        if code == KeyCode::BackTab {
            self.diff_mode = self.diff_mode.cycle();
            if !self.diff_mode.is_pinned() {
                self.diff_pane_focus = false;
            }
            let status = format!("Diffs: {}", self.diff_mode.label());
            self.set_status_notice(&status);
            return Ok(());
        }

        // Ctrl combos
        if modifiers.contains(KeyModifiers::CONTROL) {
            if self.handle_diagram_ctrl_key(code.clone(), diagram_available) {
                return Ok(());
            }
            match code {
                KeyCode::Char('b') => {
                    if matches!(self.status, ProcessingStatus::RunningTool(_)) {
                        remote.background_tool().await?;
                        self.set_status_notice("Moving tool to background...");
                        return Ok(());
                    }
                }
                KeyCode::Char('c') | KeyCode::Char('d') => {
                    self.handle_quit_request();
                    return Ok(());
                }
                KeyCode::Char('r') => {
                    self.recover_session_without_tools();
                    return Ok(());
                }
                KeyCode::Char('l')
                    if !self.is_processing && !diagram_available && !self.diff_pane_visible() =>
                {
                    self.clear_display_messages();
                    self.queued_messages.clear();
                    return Ok(());
                }
                KeyCode::Char('u') => {
                    self.input.drain(..self.cursor_pos);
                    self.cursor_pos = 0;
                    return Ok(());
                }
                KeyCode::Char('k') => {
                    self.input.truncate(self.cursor_pos);
                    return Ok(());
                }
                KeyCode::Char('a') => {
                    self.cursor_pos = 0;
                    return Ok(());
                }
                KeyCode::Char('e') => {
                    self.cursor_pos = self.input.len();
                    return Ok(());
                }
                KeyCode::Char('w') => {
                    let start = self.find_word_boundary_back();
                    self.input.drain(start..self.cursor_pos);
                    self.cursor_pos = start;
                    return Ok(());
                }
                KeyCode::Char('v') => {
                    // Ctrl+V: paste from clipboard (try image first, then text)
                    self.paste_image_from_clipboard();
                    return Ok(());
                }
                KeyCode::Tab | KeyCode::Char('t') => {
                    // Ctrl+Tab / Ctrl+T: toggle queue mode (immediate send vs wait until done)
                    self.queue_mode = !self.queue_mode;
                    let mode_str = if self.queue_mode {
                        "Queue mode: messages wait until response completes"
                    } else {
                        "Immediate mode: messages send next (no interrupt)"
                    };
                    self.set_status_notice(mode_str);
                    return Ok(());
                }
                KeyCode::Up => {
                    // Ctrl+Up: retrieve all pending unsent messages for editing
                    let had_pending = self.retrieve_pending_message_for_edit();
                    if had_pending {
                        let _ = remote.cancel_soft_interrupts().await;
                    }
                    return Ok(());
                }
                _ => {}
            }
        }

        // Shift+Enter: does opposite of queue_mode during processing
        if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
            if !self.input.is_empty() {
                let raw_input = std::mem::take(&mut self.input);
                let expanded = self.expand_paste_placeholders(&raw_input);
                self.pasted_contents.clear();
                let images = std::mem::take(&mut self.pending_images);
                self.cursor_pos = 0;

                match self.send_action(true) {
                    SendAction::Submit => {
                        // Add user message to display
                        self.push_display_message(DisplayMessage {
                            role: "user".to_string(),
                            content: raw_input,
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                        // Send expanded content to server
                        let _ = self
                            .begin_remote_send(remote, expanded, images, false)
                            .await;
                    }
                    SendAction::Queue => {
                        self.queued_messages.push(expanded);
                    }
                    SendAction::Interleave => {
                        self.send_interleave_now(expanded, remote).await;
                    }
                }
            }
            return Ok(());
        }

        // When the model picker preview is visible, arrow keys navigate the picker list
        if self
            .picker_state
            .as_ref()
            .map(|p| p.preview)
            .unwrap_or(false)
        {
            match code {
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                    return self.handle_picker_key(code, modifiers);
                }
                _ => {}
            }
        }

        // Regular keys
        match code {
            KeyCode::Char(c) => {
                self.handle_remote_char_input(c);
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    let prev = super::core::prev_char_boundary(&self.input, self.cursor_pos);
                    self.input.drain(prev..self.cursor_pos);
                    self.cursor_pos = prev;
                    self.reset_tab_completion();
                    self.sync_model_picker_preview_from_input();
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    let next = super::core::next_char_boundary(&self.input, self.cursor_pos);
                    self.input.drain(self.cursor_pos..next);
                    self.reset_tab_completion();
                    self.sync_model_picker_preview_from_input();
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos = super::core::prev_char_boundary(&self.input, self.cursor_pos);
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos = super::core::next_char_boundary(&self.input, self.cursor_pos);
                }
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
            }
            KeyCode::End => {
                self.cursor_pos = self.input.len();
            }
            KeyCode::Tab => {
                // Autocomplete command suggestions
                self.autocomplete();
            }
            KeyCode::Enter => {
                if self.activate_model_picker_from_preview() {
                    return Ok(());
                }
                if !self.input.is_empty() {
                    let raw_input = std::mem::take(&mut self.input);
                    let expanded = self.expand_paste_placeholders(&raw_input);
                    self.pasted_contents.clear();
                    let images = std::mem::take(&mut self.pending_images);
                    self.cursor_pos = 0;
                    let trimmed = expanded.trim();

                    // Handle /help - local command, no server needed
                    if let Some(topic) = trimmed
                        .strip_prefix("/help ")
                        .or_else(|| trimmed.strip_prefix("/? "))
                    {
                        if let Some(help) = self.command_help(topic) {
                            self.push_display_message(DisplayMessage::system(help));
                        } else {
                            self.push_display_message(DisplayMessage::error(format!(
                                "Unknown command '{}'. Use `/help` to list commands.",
                                topic.trim()
                            )));
                        }
                        return Ok(());
                    }

                    if trimmed == "/help" || trimmed == "/?" || trimmed == "/commands" {
                        self.follow_chat_bottom();
                        self.input = trimmed.to_string();
                        self.cursor_pos = self.input.len();
                        self.submit_input();
                        return Ok(());
                    }

                    // Handle /reload - smart reload: client and/or server if newer binary exists
                    if trimmed == "/reload" {
                        let client_needs_reload = self.has_newer_binary();
                        let server_needs_reload =
                            self.remote_server_has_update.unwrap_or(client_needs_reload);

                        if !client_needs_reload && !server_needs_reload {
                            self.push_display_message(DisplayMessage::system(
                                "No newer binary found. Nothing to reload.".to_string(),
                            ));
                            return Ok(());
                        }

                        // Reload server first (if needed), then client
                        if server_needs_reload {
                            self.append_reload_message("Reloading server with newer binary...");
                            remote.reload().await?;
                        }

                        if client_needs_reload {
                            self.push_display_message(DisplayMessage::system(
                                "Reloading client with newer binary...".to_string(),
                            ));
                            let session_id = self
                                .remote_session_id
                                .clone()
                                .unwrap_or_else(|| crate::id::new_id("ses"));
                            self.reload_requested = Some(session_id);
                            self.should_quit = true;
                        }
                        return Ok(());
                    }

                    // Handle /client-reload - force reload CLIENT binary
                    if trimmed == "/client-reload" {
                        self.push_display_message(DisplayMessage::system(
                            "Reloading client...".to_string(),
                        ));
                        let session_id = self
                            .remote_session_id
                            .clone()
                            .unwrap_or_else(|| crate::id::new_id("ses"));
                        self.reload_requested = Some(session_id);
                        self.should_quit = true;
                        return Ok(());
                    }

                    // Handle /server-reload - force reload SERVER (keeps client running)
                    if trimmed == "/server-reload" {
                        self.append_reload_message("Reloading server...");
                        remote.reload().await?;
                        return Ok(());
                    }

                    // Handle /rebuild - rebuild and reload CLIENT binary
                    if trimmed == "/rebuild" {
                        self.push_display_message(DisplayMessage::system(
                            "Rebuilding (git pull + cargo build + tests)...".to_string(),
                        ));
                        let session_id = self
                            .remote_session_id
                            .clone()
                            .unwrap_or_else(|| crate::id::new_id("ses"));
                        self.rebuild_requested = Some(session_id);
                        self.should_quit = true;
                        return Ok(());
                    }

                    // Handle /update - download latest release and reload
                    if trimmed == "/update" {
                        self.push_display_message(DisplayMessage::system(
                            "Checking for updates...".to_string(),
                        ));
                        let session_id = self
                            .remote_session_id
                            .clone()
                            .unwrap_or_else(|| crate::id::new_id("ses"));
                        self.update_requested = Some(session_id);
                        self.should_quit = true;
                        return Ok(());
                    }

                    // Handle /quit
                    if trimmed == "/quit" {
                        self.session.mark_closed();
                        let _ = self.session.save();
                        self.should_quit = true;
                        return Ok(());
                    }

                    // Handle /model commands (remote mode) - open interactive picker
                    if trimmed == "/model" || trimmed == "/models" {
                        self.open_model_picker();
                        return Ok(());
                    }

                    if let Some(model_name) = trimmed.strip_prefix("/model ") {
                        let model_name = model_name.trim();
                        if model_name.is_empty() {
                            self.push_display_message(DisplayMessage::error(
                                "Usage: /model <name>",
                            ));
                            return Ok(());
                        }
                        self.upstream_provider = None;
                        self.connection_type = None;
                        remote.set_model(model_name).await?;
                        return Ok(());
                    }

                    if trimmed == "/memory status" {
                        let default_enabled = crate::config::config().features.memory;
                        self.push_display_message(DisplayMessage::system(format!(
                            "Memory feature: **{}** (config default: {})",
                            if self.memory_enabled {
                                "enabled"
                            } else {
                                "disabled"
                            },
                            if default_enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        )));
                        return Ok(());
                    }

                    if trimmed == "/memory" {
                        let new_state = !self.memory_enabled;
                        remote
                            .set_feature(crate::protocol::FeatureToggle::Memory, new_state)
                            .await?;
                        self.set_memory_feature_enabled(new_state);
                        let label = if new_state { "ON" } else { "OFF" };
                        self.set_status_notice(&format!("Memory: {}", label));
                        self.push_display_message(DisplayMessage::system(format!(
                            "Memory feature {} for this session.",
                            if new_state { "enabled" } else { "disabled" }
                        )));
                        return Ok(());
                    }

                    if trimmed == "/memory on" {
                        remote
                            .set_feature(crate::protocol::FeatureToggle::Memory, true)
                            .await?;
                        self.set_memory_feature_enabled(true);
                        self.set_status_notice("Memory: ON");
                        self.push_display_message(DisplayMessage::system(
                            "Memory feature enabled for this session.".to_string(),
                        ));
                        return Ok(());
                    }

                    if trimmed == "/memory off" {
                        remote
                            .set_feature(crate::protocol::FeatureToggle::Memory, false)
                            .await?;
                        self.set_memory_feature_enabled(false);
                        self.set_status_notice("Memory: OFF");
                        self.push_display_message(DisplayMessage::system(
                            "Memory feature disabled for this session.".to_string(),
                        ));
                        return Ok(());
                    }

                    if trimmed.starts_with("/memory ") {
                        self.push_display_message(DisplayMessage::error(
                            "Usage: /memory [on|off|status]".to_string(),
                        ));
                        return Ok(());
                    }

                    if trimmed == "/swarm" || trimmed == "/swarm status" {
                        let default_enabled = crate::config::config().features.swarm;
                        self.push_display_message(DisplayMessage::system(format!(
                            "Swarm feature: **{}** (config default: {})",
                            if self.swarm_enabled {
                                "enabled"
                            } else {
                                "disabled"
                            },
                            if default_enabled {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        )));
                        return Ok(());
                    }

                    if trimmed == "/swarm on" {
                        remote
                            .set_feature(crate::protocol::FeatureToggle::Swarm, true)
                            .await?;
                        self.set_swarm_feature_enabled(true);
                        self.set_status_notice("Swarm: ON");
                        self.push_display_message(DisplayMessage::system(
                            "Swarm feature enabled for this session.".to_string(),
                        ));
                        return Ok(());
                    }

                    if trimmed == "/swarm off" {
                        remote
                            .set_feature(crate::protocol::FeatureToggle::Swarm, false)
                            .await?;
                        self.set_swarm_feature_enabled(false);
                        self.set_status_notice("Swarm: OFF");
                        self.push_display_message(DisplayMessage::system(
                            "Swarm feature disabled for this session.".to_string(),
                        ));
                        return Ok(());
                    }

                    if trimmed.starts_with("/swarm ") {
                        self.push_display_message(DisplayMessage::error(
                            "Usage: /swarm [on|off|status]".to_string(),
                        ));
                        return Ok(());
                    }

                    if trimmed == "/split" {
                        if self.is_processing {
                            self.push_display_message(DisplayMessage::error(
                                "Cannot split while processing. Wait for the current turn to finish.".to_string(),
                            ));
                            return Ok(());
                        }
                        self.push_display_message(DisplayMessage::system(
                            "Splitting session...".to_string(),
                        ));
                        remote.split().await?;
                        return Ok(());
                    }

                    if trimmed == "/compact" {
                        self.push_display_message(DisplayMessage::system(
                            "Requesting compaction...".to_string(),
                        ));
                        remote.compact().await?;
                        return Ok(());
                    }

                    // Check for pending login (OAuth code paste, API key paste)
                    // Must be checked before slash command routing since OAuth codes
                    // don't start with /
                    if self.pending_login.is_some() {
                        self.input = trimmed.to_string();
                        self.cursor_pos = self.input.len();
                        self.submit_input();
                        return Ok(());
                    }

                    // Any other slash command: handle locally via submit_input()
                    // (covers /auth, /login, /clear, /config, /effort, /fix,
                    //  /info, /version, /rewind, /remember, /record, etc.)
                    if trimmed.starts_with('/') {
                        self.input = trimmed.to_string();
                        self.cursor_pos = self.input.len();
                        self.submit_input();
                        return Ok(());
                    }

                    // Queue message if processing, otherwise send
                    match self.send_action(false) {
                        SendAction::Submit => {
                            // Add user message to display (show placeholder)
                            self.push_display_message(DisplayMessage {
                                role: "user".to_string(),
                                content: raw_input,
                                tool_calls: vec![],
                                duration_secs: None,
                                title: None,
                                tool_data: None,
                            });
                            // Send expanded content (with actual pasted text) to server
                            let _ = self
                                .begin_remote_send(remote, expanded, images, false)
                                .await;
                        }
                        SendAction::Queue => {
                            self.queued_messages.push(expanded);
                        }
                        SendAction::Interleave => {
                            self.send_interleave_now(expanded, remote).await;
                        }
                    }
                }
            }
            KeyCode::Up | KeyCode::PageUp => {
                let inc = if code == KeyCode::PageUp { 10 } else { 1 };
                self.scroll_up(inc);
            }
            KeyCode::Down | KeyCode::PageDown => {
                let dec = if code == KeyCode::PageDown { 10 } else { 1 };
                self.scroll_down(dec);
            }
            KeyCode::Esc => {
                if self
                    .picker_state
                    .as_ref()
                    .map(|p| p.preview)
                    .unwrap_or(false)
                {
                    self.picker_state = None;
                    self.input.clear();
                    self.cursor_pos = 0;
                } else if self.is_processing {
                    remote.cancel().await?;
                    self.set_status_notice("Interrupting...");
                } else {
                    self.follow_chat_bottom();
                    self.input.clear();
                    self.cursor_pos = 0;
                    self.sync_model_picker_preview_from_input();
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Process turn while still accepting input for queueing
    async fn process_turn_with_input(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) {
        // We need to run the turn logic step by step, checking for input between steps
        // For now, run the turn but poll for input during streaming

        match self.run_turn_interactive(terminal, event_stream).await {
            Ok(()) => {
                self.last_stream_error = None;
            }
            Err(e) => {
                let err_str = e.to_string();
                if is_context_limit_error(&err_str) {
                    if self
                        .try_auto_compact_and_retry(terminal, event_stream)
                        .await
                    {
                        // Successfully recovered
                    } else {
                        self.handle_turn_error(err_str);
                    }
                } else {
                    self.handle_turn_error(err_str);
                }
            }
        }

        // Process any queued messages
        self.process_queued_messages(terminal, event_stream).await;

        // Accumulate turn tokens into session totals
        self.total_input_tokens += self.streaming_input_tokens;
        self.total_output_tokens += self.streaming_output_tokens;

        // Calculate cost if using API-key provider (OpenRouter, direct API key)
        self.update_cost_impl();

        self.is_processing = false;
        self.status = ProcessingStatus::Idle;
        self.processing_started = None;
        self.interleave_message = None;
        self.pending_soft_interrupts.clear();
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
    }

    /// Handle a key event (wrapper for debug injection)
    fn handle_key_event(&mut self, event: crossterm::event::KeyEvent) {
        // Record the event if recording is active
        use super::test_harness::{record_event, TestEvent};
        let modifiers: Vec<String> = {
            let mut mods = vec![];
            if event.modifiers.contains(KeyModifiers::CONTROL) {
                mods.push("ctrl".to_string());
            }
            if event.modifiers.contains(KeyModifiers::ALT) {
                mods.push("alt".to_string());
            }
            if event.modifiers.contains(KeyModifiers::SHIFT) {
                mods.push("shift".to_string());
            }
            mods
        };
        let code_str = format!("{:?}", event.code);
        record_event(TestEvent::Key {
            code: code_str,
            modifiers,
        });

        let _ = self.handle_key(event.code, event.modifiers);
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        let mut code = code;
        let mut modifiers = modifiers;
        ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

        if self.changelog_scroll.is_some() {
            return self.handle_changelog_key(code);
        }

        // If picker is active and not in preview mode, handle picker keys first
        if let Some(ref picker) = self.picker_state {
            if !picker.preview {
                return self.handle_picker_key(code, modifiers);
            }
        }
        // Preview-mode picker: arrow keys navigate without leaving preview
        if self.handle_picker_preview_key(&code, modifiers)? {
            return Ok(());
        }

        if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('m')) {
            self.toggle_diagram_pane();
            return Ok(());
        }
        if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('t')) {
            self.toggle_diagram_pane_position();
            return Ok(());
        }
        if let Some(direction) = self
            .model_switch_keys
            .direction_for(code.clone(), modifiers)
        {
            self.cycle_model(direction);
            return Ok(());
        }
        if let Some(direction) = self
            .effort_switch_keys
            .direction_for(code.clone(), modifiers)
        {
            self.cycle_effort(direction);
            return Ok(());
        }
        if self
            .centered_toggle_keys
            .toggle
            .matches(code.clone(), modifiers)
        {
            self.toggle_centered_mode();
            return Ok(());
        }
        self.normalize_diagram_state();
        let diagram_available = self.diagram_available();
        if self.handle_diagram_focus_key(code.clone(), modifiers, diagram_available) {
            return Ok(());
        }
        if self.handle_diff_pane_focus_key(code.clone(), modifiers) {
            return Ok(());
        }
        // Handle Alt combos (readline word movement)
        if modifiers.contains(KeyModifiers::ALT) {
            match code {
                c if self.centered_toggle_keys.toggle.matches(c, modifiers) => {
                    self.toggle_centered_mode();
                    return Ok(());
                }
                KeyCode::Char('b') => {
                    // Alt+B: back one word
                    self.cursor_pos = self.find_word_boundary_back();
                    return Ok(());
                }
                KeyCode::Char('f') => {
                    // Alt+F: forward one word
                    self.cursor_pos = self.find_word_boundary_forward();
                    return Ok(());
                }
                KeyCode::Char('d') => {
                    // Alt+D: delete word forward
                    let end = self.find_word_boundary_forward();
                    self.input.drain(self.cursor_pos..end);
                    self.sync_model_picker_preview_from_input();
                    return Ok(());
                }
                KeyCode::Backspace => {
                    // Alt+Backspace: delete word backward
                    let start = self.find_word_boundary_back();
                    self.input.drain(start..self.cursor_pos);
                    self.cursor_pos = start;
                    self.sync_model_picker_preview_from_input();
                    return Ok(());
                }
                KeyCode::Char('i') => {
                    // Alt+I: toggle info widget
                    super::info_widget::toggle_enabled();
                    let status = if super::info_widget::is_enabled() {
                        "Info widget: ON"
                    } else {
                        "Info widget: OFF"
                    };
                    self.set_status_notice(status);
                    return Ok(());
                }
                KeyCode::Char('v') => {
                    // Alt+V: paste image from clipboard
                    self.paste_image_from_clipboard();
                    return Ok(());
                }
                _ => {}
            }
        }

        // Handle configurable scroll keys (default: Ctrl+K/J, Alt+U/D for page)
        if let Some(amount) = self.scroll_keys.scroll_amount(code.clone(), modifiers) {
            if amount < 0 {
                self.scroll_up((-amount) as usize);
            } else {
                self.scroll_down(amount as usize);
            }
            return Ok(());
        }

        // Handle prompt jump keys (default: Ctrl+[/], plus Ctrl+1..9 recency)
        if let Some(dir) = self.scroll_keys.prompt_jump(code.clone(), modifiers) {
            if dir < 0 {
                self.scroll_to_prev_prompt();
            } else {
                self.scroll_to_next_prompt();
            }
            return Ok(());
        }

        if let Some(rank) = Self::ctrl_prompt_rank(&code, modifiers) {
            self.scroll_to_recent_prompt_rank(rank);
            return Ok(());
        }

        // Handle scroll bookmark toggle (default: Ctrl+G)
        if self.scroll_keys.is_bookmark(code.clone(), modifiers) {
            self.toggle_scroll_bookmark();
            return Ok(());
        }

        // Shift+Tab: cycle diff mode (Off → Inline → Pinned)
        if code == KeyCode::BackTab {
            self.diff_mode = self.diff_mode.cycle();
            if !self.diff_mode.is_pinned() {
                self.diff_pane_focus = false;
            }
            let status = format!("Diffs: {}", self.diff_mode.label());
            self.set_status_notice(&status);
            return Ok(());
        }

        // Handle ctrl combos regardless of processing state
        if modifiers.contains(KeyModifiers::CONTROL) {
            if self.handle_diagram_ctrl_key(code.clone(), diagram_available) {
                return Ok(());
            }
            match code {
                KeyCode::Char('c') | KeyCode::Char('d') => {
                    self.handle_quit_request();
                    return Ok(());
                }
                KeyCode::Char('r') => {
                    self.recover_session_without_tools();
                    return Ok(());
                }
                KeyCode::Char('l')
                    if !self.is_processing && !diagram_available && !self.diff_pane_visible() =>
                {
                    self.clear_provider_messages();
                    self.clear_display_messages();
                    self.queued_messages.clear();
                    self.pasted_contents.clear();
                    self.pending_images.clear();
                    self.active_skill = None;
                    let mut session = Session::create(None, None);
                    session.model = Some(self.provider.model());
                    self.session = session;
                    self.provider_session_id = None;
                    return Ok(());
                }
                KeyCode::Char('u') => {
                    // Ctrl+U: kill to beginning of line
                    self.input.drain(..self.cursor_pos);
                    self.cursor_pos = 0;
                    self.sync_model_picker_preview_from_input();
                    return Ok(());
                }
                KeyCode::Char('a') => {
                    // Ctrl+A: beginning of line
                    self.cursor_pos = 0;
                    return Ok(());
                }
                KeyCode::Char('e') => {
                    // Ctrl+E: end of line
                    self.cursor_pos = self.input.len();
                    return Ok(());
                }
                KeyCode::Char('b') => {
                    // Ctrl+B: back one char
                    if self.cursor_pos > 0 {
                        self.cursor_pos =
                            super::core::prev_char_boundary(&self.input, self.cursor_pos);
                    }
                    return Ok(());
                }
                KeyCode::Char('f') => {
                    // Ctrl+F: forward one char
                    if self.cursor_pos < self.input.len() {
                        self.cursor_pos =
                            super::core::next_char_boundary(&self.input, self.cursor_pos);
                    }
                    return Ok(());
                }
                KeyCode::Char('w') => {
                    // Ctrl+W: delete word backward
                    let start = self.find_word_boundary_back();
                    self.input.drain(start..self.cursor_pos);
                    self.cursor_pos = start;
                    self.sync_model_picker_preview_from_input();
                    return Ok(());
                }
                KeyCode::Char('v') => {
                    // Ctrl+V: paste from clipboard (try image first, then text)
                    self.paste_image_from_clipboard();
                    return Ok(());
                }
                KeyCode::Tab | KeyCode::Char('t') => {
                    // Ctrl+Tab / Ctrl+T: toggle queue mode (immediate send vs wait until done)
                    self.queue_mode = !self.queue_mode;
                    let mode_str = if self.queue_mode {
                        "Queue mode: messages wait until response completes"
                    } else {
                        "Immediate mode: messages send next (no interrupt)"
                    };
                    self.set_status_notice(mode_str);
                    return Ok(());
                }
                KeyCode::Up => {
                    // Ctrl+Up: retrieve all pending unsent messages for editing
                    self.retrieve_pending_message_for_edit();
                    return Ok(());
                }
                _ => {}
            }
        }

        // Shift+Enter: does opposite of queue_mode during processing
        if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
            if !self.input.is_empty() {
                match self.send_action(true) {
                    SendAction::Submit => self.submit_input(),
                    SendAction::Queue => self.queue_message(),
                    SendAction::Interleave => {
                        let raw_input = std::mem::take(&mut self.input);
                        let expanded = self.expand_paste_placeholders(&raw_input);
                        self.pasted_contents.clear();
                        self.pending_images.clear();
                        self.cursor_pos = 0;
                        // Set interleave_message so streaming code can pick it up
                        self.interleave_message = Some(expanded);
                        self.set_status_notice("⏭ Sending now (interleave)");
                    }
                }
            }
            return Ok(());
        }

        // When the model picker preview is visible, arrow keys navigate the picker list
        if self
            .picker_state
            .as_ref()
            .map(|p| p.preview)
            .unwrap_or(false)
        {
            match code {
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                    return self.handle_picker_key(code, modifiers);
                }
                _ => {}
            }
        }

        match code {
            KeyCode::Enter => {
                if self.activate_model_picker_from_preview() {
                    return Ok(());
                }
                if !self.input.is_empty() {
                    match self.send_action(false) {
                        SendAction::Submit => self.submit_input(),
                        SendAction::Queue => self.queue_message(),
                        SendAction::Interleave => {
                            let raw_input = std::mem::take(&mut self.input);
                            let expanded = self.expand_paste_placeholders(&raw_input);
                            self.pasted_contents.clear();
                            self.pending_images.clear();
                            self.cursor_pos = 0;
                            // Set interleave_message so streaming code can pick it up
                            self.interleave_message = Some(expanded);
                            self.set_status_notice("⏭ Sending now (interleave)");
                        }
                    }
                }
            }
            KeyCode::Char(c) => {
                if self.input.is_empty()
                    && !self.is_processing
                    && self.display_messages.is_empty()
                {
                    if let Some(digit) = c.to_digit(10) {
                        let suggestions = self.suggestion_prompts();
                        let idx = digit as usize;
                        if idx >= 1 && idx <= suggestions.len() {
                            let (_label, prompt) = &suggestions[idx - 1];
                            if !prompt.starts_with('/') {
                                self.input = prompt.clone();
                                self.cursor_pos = self.input.len();
                                self.follow_chat_bottom();
                                return Ok(());
                            }
                        }
                    }
                }
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += c.len_utf8();
                self.reset_tab_completion();
                self.sync_model_picker_preview_from_input();
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    let prev = super::core::prev_char_boundary(&self.input, self.cursor_pos);
                    self.input.drain(prev..self.cursor_pos);
                    self.cursor_pos = prev;
                    self.reset_tab_completion();
                    self.sync_model_picker_preview_from_input();
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    let next = super::core::next_char_boundary(&self.input, self.cursor_pos);
                    self.input.drain(self.cursor_pos..next);
                    self.reset_tab_completion();
                    self.sync_model_picker_preview_from_input();
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos = super::core::prev_char_boundary(&self.input, self.cursor_pos);
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos = super::core::next_char_boundary(&self.input, self.cursor_pos);
                }
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.input.len(),
            KeyCode::Tab => {
                // Autocomplete command suggestions
                self.autocomplete();
            }
            KeyCode::Up | KeyCode::PageUp => {
                let inc = if code == KeyCode::PageUp { 10 } else { 1 };
                self.scroll_up(inc);
            }
            KeyCode::Down | KeyCode::PageDown => {
                let dec = if code == KeyCode::PageDown { 10 } else { 1 };
                self.scroll_down(dec);
            }
            KeyCode::Esc => {
                if self
                    .picker_state
                    .as_ref()
                    .map(|p| p.preview)
                    .unwrap_or(false)
                {
                    self.picker_state = None;
                    self.input.clear();
                    self.cursor_pos = 0;
                } else if self.is_processing {
                    // Interrupt generation
                    self.cancel_requested = true;
                    self.interleave_message = None;
                    self.pending_soft_interrupts.clear();
                } else {
                    // Reset scroll to bottom and clear input
                    self.follow_chat_bottom();
                    self.input.clear();
                    self.cursor_pos = 0;
                    self.sync_model_picker_preview_from_input();
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Try to paste an image from the clipboard. Checks native image data first,
    /// then falls back to HTML clipboard for <img> URLs, then arboard text.
    /// Used by both Ctrl+V and Alt+V handlers in both local and remote mode.
    fn paste_image_from_clipboard(&mut self) {
        if let Some((media_type, base64_data)) = clipboard_image() {
            let size_kb = base64_data.len() / 1024;
            self.pending_images.push((media_type.clone(), base64_data));
            let n = self.pending_images.len();
            let placeholder = format!("[image {}]", n);
            self.input.insert_str(self.cursor_pos, &placeholder);
            self.cursor_pos += placeholder.len();
            self.sync_model_picker_preview_from_input();
            self.set_status_notice(&format!("Pasted {} ({} KB)", media_type, size_kb));
        } else if let Ok(mut cb) = arboard::Clipboard::new() {
            if let Ok(text) = cb.get_text() {
                if let Some(url) = extract_image_url(&text) {
                    self.set_status_notice("Downloading image...");
                    if let Some((media_type, base64_data)) = download_image_url(&url) {
                        let size_kb = base64_data.len() / 1024;
                        self.pending_images.push((media_type.clone(), base64_data));
                        let n = self.pending_images.len();
                        let placeholder = format!("[image {}]", n);
                        self.input.insert_str(self.cursor_pos, &placeholder);
                        self.cursor_pos += placeholder.len();
                        self.sync_model_picker_preview_from_input();
                        self.set_status_notice(&format!("Pasted {} ({} KB)", media_type, size_kb));
                    } else {
                        self.set_status_notice("Failed to download image");
                    }
                } else {
                    self.handle_paste(text);
                }
            } else {
                self.set_status_notice("No image in clipboard");
            }
        } else {
            self.set_status_notice("No image in clipboard");
        }
    }

    /// Queue a message to be sent later
    /// Handle bracketed paste: store text content (image URLs are still detected inline)
    fn handle_paste(&mut self, text: String) {
        // Note: clipboard_image() is NOT checked here. Bracketed paste events from the
        // terminal always deliver text. Checking clipboard_image() here caused a bug where
        // text pastes were misidentified as images when the clipboard also had image data
        // (common on Wayland where apps advertise multiple MIME types). Image pasting is
        // handled by paste_image_from_clipboard() (Ctrl+V / Alt+V) instead.

        // Check if pasted text contains an image URL (e.g., Discord <img src="...">)
        if let Some(url) = extract_image_url(&text) {
            crate::logging::info(&format!("Downloading image from pasted URL: {}", url));
            self.set_status_notice("Downloading image...");
            if let Some((media_type, base64_data)) = download_image_url(&url) {
                let size_kb = base64_data.len() / 1024;
                self.pending_images.push((media_type.clone(), base64_data));
                let n = self.pending_images.len();
                let placeholder = format!("[image {}]", n);
                self.input.insert_str(self.cursor_pos, &placeholder);
                self.cursor_pos += placeholder.len();
                self.sync_model_picker_preview_from_input();
                self.set_status_notice(&format!("Pasted {} ({} KB)", media_type, size_kb));
                return;
            } else {
                self.set_status_notice("Failed to download image");
            }
        }

        crate::logging::info(&format!(
            "Text paste: {} chars, {} lines",
            text.len(),
            text.lines().count()
        ));

        let line_count = text.lines().count().max(1);
        if line_count < 5 {
            self.input.insert_str(self.cursor_pos, &text);
            self.cursor_pos += text.len();
        } else {
            self.pasted_contents.push(text);
            let placeholder = format!(
                "[pasted {} line{}]",
                line_count,
                if line_count == 1 { "" } else { "s" }
            );
            self.input.insert_str(self.cursor_pos, &placeholder);
            self.cursor_pos += placeholder.len();
        }
        self.sync_model_picker_preview_from_input();
    }

    /// Expand paste placeholders in input with actual content
    fn expand_paste_placeholders(&mut self, input: &str) -> String {
        let mut result = input.to_string();
        // Replace placeholders in reverse order to preserve indices
        for content in self.pasted_contents.iter().rev() {
            let line_count = content.lines().count().max(1);
            let placeholder = format!(
                "[pasted {} line{}]",
                line_count,
                if line_count == 1 { "" } else { "s" }
            );
            // Use rfind to match last occurrence (since we iterate in reverse)
            if let Some(pos) = result.rfind(&placeholder) {
                result.replace_range(pos..pos + placeholder.len(), content);
            }
        }
        result
    }

    fn queue_message(&mut self) {
        let content = std::mem::take(&mut self.input);
        let expanded = self.expand_paste_placeholders(&content);
        self.pasted_contents.clear();
        self.pending_images.clear();
        self.cursor_pos = 0;
        self.queued_messages.push(expanded);
    }

    /// Send an interleave message immediately to the server as a soft interrupt.
    /// Skips the intermediate buffer stage - goes directly to pending_soft_interrupts.
    async fn send_interleave_now(
        &mut self,
        content: String,
        remote: &mut super::backend::RemoteConnection,
    ) {
        if content.trim().is_empty() {
            return;
        }
        let msg_clone = content.clone();
        if let Err(e) = remote.soft_interrupt(content, false).await {
            self.push_display_message(DisplayMessage::error(format!(
                "Failed to send interleave: {}",
                e
            )));
        } else {
            self.pending_soft_interrupts.push(msg_clone);
            self.set_status_notice("⏭ Interleave sent");
        }
    }

    /// Retrieve all pending unsent messages into the input for editing.
    /// Priority: pending soft interrupts first, then interleave, then queued.
    /// Returns true if pending soft interrupts were retrieved (caller should cancel on server).
    fn retrieve_pending_message_for_edit(&mut self) -> bool {
        if !self.input.is_empty() {
            return false;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut had_pending = false;

        if !self.pending_soft_interrupts.is_empty() {
            parts.extend(std::mem::take(&mut self.pending_soft_interrupts));
            had_pending = true;
        }
        if let Some(msg) = self.interleave_message.take() {
            if !msg.is_empty() {
                parts.push(msg);
            }
        }
        parts.extend(std::mem::take(&mut self.queued_messages));

        if !parts.is_empty() {
            self.input = parts.join("\n\n");
            self.cursor_pos = self.input.len();
            let count = parts.len();
            self.set_status_notice(&format!(
                "Retrieved {} pending message{} for editing",
                count,
                if count == 1 { "" } else { "s" }
            ));
        }
        had_pending
    }

    fn send_action(&self, shift: bool) -> SendAction {
        if !self.is_processing {
            return SendAction::Submit;
        }
        // Slash commands should always be processed immediately, not queued/interleaved
        if self.input.trim().starts_with('/') {
            return SendAction::Submit;
        }
        if shift {
            if self.queue_mode {
                SendAction::Interleave
            } else {
                SendAction::Queue
            }
        } else if self.queue_mode {
            SendAction::Queue
        } else {
            SendAction::Interleave
        }
    }

    fn insert_thought_line(&mut self, line: String) {
        if self.thought_line_inserted || line.is_empty() {
            return;
        }
        self.thought_line_inserted = true;
        let mut prefix = line;
        if !prefix.ends_with('\n') {
            prefix.push('\n');
        }
        prefix.push('\n');
        if self.streaming_text.is_empty() {
            self.streaming_text = prefix;
        } else {
            self.streaming_text = format!("{}{}", prefix, self.streaming_text);
        }
    }

    fn clear_streaming_render_state(&mut self) {
        self.streaming_text.clear();
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
    }

    fn take_streaming_text(&mut self) -> String {
        let content = std::mem::take(&mut self.streaming_text);
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
        content
    }

    fn accumulate_streaming_output_tokens(
        &mut self,
        output_tokens: u64,
        call_output_tokens_seen: &mut u64,
    ) {
        let delta = if output_tokens >= *call_output_tokens_seen {
            output_tokens - *call_output_tokens_seen
        } else {
            // Usage snapshots should be monotonic within one API call. If they are not,
            // treat this as a reset and count the full value once.
            output_tokens
        };
        self.streaming_total_output_tokens += delta;
        *call_output_tokens_seen = output_tokens;
    }

    fn command_help(&self, topic: &str) -> Option<String> {
        let topic = topic.trim().trim_start_matches('/').to_lowercase();
        let help = match topic.as_str() {
            "help" | "commands" => {
                "`/help`\nShow general command list and keyboard shortcuts.\n\n`/help <command>`\nShow detailed help for one command."
            }
            "compact" => {
                "`/compact`\nForce context compaction now.\nStarts background summarization and applies it automatically when ready."
            }
            "fix" => {
                "`/fix`\nRun recovery actions when the model cannot continue.\nRepairs missing tool outputs, resets provider session state, and starts compaction when possible."
            }
            "rewind" => {
                "`/rewind`\nShow numbered conversation history.\n\n`/rewind N`\nRewind to message N (drops everything after it and resets provider session)."
            }
            "clear" => {
                "`/clear`\nClear current conversation, queue, and display; starts a fresh session."
            }
            "model" => {
                "`/model`\nOpen model picker.\n\n`/model <name>`\nSwitch model.\n\n`/model <name>@<provider>`\nPin OpenRouter routing (`@auto` clears pin)."
            }
            "effort" => {
                "`/effort`\nShow current reasoning effort.\n\n`/effort <level>`\nSet reasoning effort (none|low|medium|high|xhigh).\n\nAlso: Alt+←/→ to cycle."
            }
            "memory" => "`/memory [on|off|status]`\nToggle memory features for this session.",
            "remember" => {
                "`/remember`\nExtract memories from current conversation and store them."
            }
            "swarm" => "`/swarm [on|off|status]`\nToggle swarm features for this session.",
            "reload" => "`/reload`\nReload to a newer binary if one is available.",
            "rebuild" => "`/rebuild`\nRun full update flow (git pull + cargo build + tests).",
            "split" => "`/split`\nSplit the current session into a new window. Clones the full conversation history so both sessions continue from the same point.",
            "info" => "`/info`\nShow session metadata and token usage.",
            "usage" => "`/usage`\nFetch and display subscription usage limits for all connected OAuth providers (Anthropic, OpenAI/ChatGPT).\nShows 5-hour and 7-day windows, reset times, and extra usage status.",
            "version" => "`/version`\nShow jcode version/build details.",
            "changelog" => "`/changelog`\nShow recent changes embedded in this build.",
            "quit" => "`/quit`\nExit jcode.",
            "config" => {
                "`/config`\nShow active configuration.\n\n`/config init`\nCreate default config file.\n\n`/config edit`\nOpen config in `$EDITOR`."
            }
            "auth" | "login" => {
                "`/auth`\nShow authentication status for all providers.\n\n`/login`\nInteractive provider selection - pick a provider to log into.\n\n`/login <provider>`\nStart login flow directly (anthropic/claude, openai, openrouter, cursor, copilot, antigravity)."
            }
            "account" | "accounts" => {
                "`/account`\nList all Anthropic OAuth accounts.\n\n`/account add <label>`\nAdd a new account via OAuth login.\n\n`/account switch <label>`\nSwitch the active account.\n\n`/account remove <label>`\nRemove an account."
            }
            "client-reload" if self.is_remote => {
                "`/client-reload`\nForce client binary reload in remote mode."
            }
            "server-reload" if self.is_remote => {
                "`/server-reload`\nForce server binary reload in remote mode."
            }
            _ => return None,
        };
        Some(help.to_string())
    }

    /// Submit input - just sets up message and flags, processing happens in next loop iteration
    fn submit_input(&mut self) {
        if self.activate_model_picker_from_preview() {
            return;
        }

        let raw_input = std::mem::take(&mut self.input);
        let input = self.expand_paste_placeholders(&raw_input);
        self.pasted_contents.clear();
        self.cursor_pos = 0;
        self.follow_chat_bottom(); // Reset to bottom and resume auto-scroll on new input

        if let Some(pending) = self.pending_login.take() {
            self.handle_login_input(pending, input);
            return;
        }

        let trimmed = input.trim();
        if let Some(topic) = trimmed
            .strip_prefix("/help ")
            .or_else(|| trimmed.strip_prefix("/? "))
        {
            if let Some(help) = self.command_help(topic) {
                self.push_display_message(DisplayMessage::system(help));
            } else {
                self.push_display_message(DisplayMessage::error(format!(
                    "Unknown command '{}'. Use `/help` to list commands.",
                    topic.trim()
                )));
            }
            return;
        }

        if trimmed == "/help" || trimmed == "/?" || trimmed == "/commands" {
            let model_next = format!(
                "• `{}` - Next model (set JCODE_MODEL_SWITCH_KEY)",
                self.model_switch_keys.next_label
            );
            let model_prev = self
                .model_switch_keys
                .prev_label
                .as_ref()
                .map(|label| {
                    format!(
                        "• `{}` - Previous model (set JCODE_MODEL_SWITCH_PREV_KEY)",
                        label
                    )
                })
                .unwrap_or_default();
            let remote_reload_help = if self.is_remote {
                "\n                     • `/client-reload` - Force reload client binary\n\
                     • `/server-reload` - Force reload server binary"
            } else {
                ""
            };
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: format!(
                    "**Commands:**\n\
                     • `/help` - Show this help\n\
                     • `/help <command>` - Show details for one command\n\
                     • `/commands` - Alias for `/help`\n\
                     • `/config` - Show current configuration\n\
                     • `/config init` - Create default config file (~/.jcode/config.toml)\n\
                     • `/config edit` - Open config file in $EDITOR\n\
                     • `/model` - List available models\n\
                     • `/model <name>` - Switch to a different model\n\
                     • `/model <name>@<provider>` - Pin OpenRouter provider (`@auto` clears)\n\
                     • `/effort` - Show current reasoning effort level\n\
                     • `/effort <level>` - Set effort (none|low|medium|high|xhigh)\n\
                     • `/usage` - Show subscription usage limits for all connected providers\n\
                     • `/memory [on|off|status]` - Toggle memory features for this session\n\
                     • `/swarm [on|off|status]` - Toggle swarm features for this session\n\
                     • `/reload` - Smart reload (client/server if newer binary exists)\n\
                     • `/rebuild` - Full rebuild (git pull + cargo build + tests){}\n\
                     • `/changelog` - Show recent changes in this build\n\
                     • `/split` - Split session into a new window (clones conversation)\n\
                     • `/clear` - Clear conversation\n\
                     • `/rewind` - Show history with numbers, `/rewind N` to rewind\n\
                     • `/compact` - Manually compact context (summarize old messages)\n\
                     • `/fix` - Attempt session recovery (context/tool/session issues)\n\
                     • `/auth` - Show auth status, `/login` for interactive login, `/login <provider>` for direct login\n\
                     • `/account` - Manage Anthropic accounts (list/add/switch/remove)\n\
                     • `/debug-visual` - Enable visual debugging for TUI issues\n\
                     • `/<skill>` - Activate a skill\n\n\
                     **Available skills:** {}\n\n\
                     **Keyboard shortcuts:**\n\
                     • `Ctrl+C` / `Ctrl+D` - Quit (press twice to confirm)\n\
                     • `Ctrl+H` / `Ctrl+L` - Focus chat/diagram/diffs (pinned mode)\n\
                     • `Ctrl+Left/Right` - Cycle diagrams in side pane\n\
                     • `h/j/k/l` or arrow keys - Pan diagram (when focused)\n\
                     • `[` / `]` - Zoom diagram (when focused)\n\
                     • `+` / `-` - Resize diagram pane (when focused)\n\
                     • `Alt+M` - Toggle diagram pane\n\
                     • `Alt+T` - Toggle diagram pane position (side/top)\n\
                     • `{}` - Toggle centered/left-aligned layout\n\
                     • `Shift+Tab` - Cycle diff mode (Off → Inline → Pinned)\n\
                     • `Alt+V` - Paste image from clipboard\n\
                     • `Ctrl+R` - Recover from missing tool outputs\n\
                     • `PageUp/Down` or `Up/Down` - Scroll history\n\
                     • `{}`/`{}` - Scroll up/down (see `/config`)\n\
                     • `{}`/`{}` - Page up/down (see `/config`)\n\
                     • `Ctrl+[` / `Ctrl+]` - Jump between user prompts\n\
                     • `Ctrl+1..9` - Jump by recency (1 = most recent)\n\
                     • `Ctrl+Tab` / `Ctrl+T` - Toggle queue mode (wait vs immediate send)\n\
                     • `Ctrl+Up` - Retrieve pending message for editing\n\
                     • `Ctrl+U` - Clear input line\n\
                     {}\n\
                     {}",
                    remote_reload_help,
                    self.skills
                        .list()
                        .iter()
                        .map(|s| format!("/{}", s.name))
                        .collect::<Vec<_>>()
                        .join(", "),
                    self.scroll_keys.up_label,
                    self.scroll_keys.down_label,
                    self.scroll_keys.page_up_label,
                    self.scroll_keys.page_down_label,
                    self.centered_toggle_keys.toggle_label,
                    model_next,
                    model_prev
                ),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if trimmed == "/clear" {
            self.clear_provider_messages();
            self.clear_display_messages();
            self.queued_messages.clear();
            self.pasted_contents.clear();
            self.pending_images.clear();
            self.active_skill = None;
            let mut session = Session::create(None, None);
            session.model = Some(self.provider.model());
            self.session = session;
            self.provider_session_id = None;
            return;
        }

        // Handle /compact command - manual context compaction
        if trimmed == "/compact" {
            if !self.provider.supports_compaction() {
                self.push_display_message(DisplayMessage::system(
                    "Manual compaction is not available for this provider.".to_string(),
                ));
                return;
            }
            let compaction = self.registry.compaction();
            match compaction.try_write() {
                Ok(mut manager) => {
                    // Show current status
                    let stats = manager.stats_with(&self.messages);
                    let status_msg = format!(
                        "**Context Status:**\n\
                        • Messages: {} (active), {} (total history)\n\
                        • Token usage: ~{}k (estimate ~{}k) / {}k ({:.1}%)\n\
                        • Has summary: {}\n\
                        • Compacting: {}",
                        stats.active_messages,
                        stats.total_turns,
                        stats.effective_tokens / 1000,
                        stats.token_estimate / 1000,
                        manager.token_budget() / 1000,
                        stats.context_usage * 100.0,
                        if stats.has_summary { "yes" } else { "no" },
                        if stats.is_compacting {
                            "in progress..."
                        } else {
                            "no"
                        }
                    );

                    match manager.force_compact_with(&self.messages, self.provider.clone()) {
                        Ok(()) => {
                            self.push_display_message(DisplayMessage {
                                role: "system".to_string(),
                                content: format!(
                                    "{}\n\n✓ **Compaction started** - summarizing older messages in background.\n\
                                    The summary will be applied automatically when ready.\n\
                                    Use `/help compact` for details.",
                                    status_msg
                                ),
                                tool_calls: vec![],
                                duration_secs: None,
                                title: None,
                                tool_data: None,
                            });
                        }
                        Err(reason) => {
                            self.push_display_message(DisplayMessage {
                                role: "system".to_string(),
                                content: format!(
                                    "{}\n\n⚠ **Cannot compact:** {}\n\
                                    Try `/fix` for emergency recovery.",
                                    status_msg, reason
                                ),
                                tool_calls: vec![],
                                duration_secs: None,
                                title: None,
                                tool_data: None,
                            });
                        }
                    }
                }
                Err(_) => {
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: "⚠ Cannot access compaction manager (lock held)".to_string(),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
            }
            return;
        }

        if trimmed == "/fix" {
            self.run_fix_command();
            return;
        }

        if trimmed == "/usage" {
            self.push_display_message(DisplayMessage::system(
                "Fetching usage limits from all providers...".to_string(),
            ));
            tokio::spawn(async move {
                let results = crate::usage::fetch_all_provider_usage().await;
                Bus::global().publish(BusEvent::UsageReport(results));
            });
            return;
        }

        // Handle /remember command - extract memories from current conversation
        if trimmed == "/remember" {
            if !self.memory_enabled {
                self.push_display_message(DisplayMessage::system(
                    "Memory feature is disabled. Use `/memory on` to enable it.".to_string(),
                ));
                return;
            }

            use crate::tui::info_widget::{MemoryEventKind, MemoryState};

            // Format context for extraction
            let context = crate::memory::format_context_for_relevance(&self.messages);
            if context.len() < 100 {
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: "Not enough conversation to extract memories from.".to_string(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                return;
            }

            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "🧠 Extracting memories from conversation...".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });

            // Update memory state for UI
            crate::memory::set_state(MemoryState::Extracting {
                reason: "manual".to_string(),
            });
            crate::memory::add_event(MemoryEventKind::ExtractionStarted {
                reason: "/remember command".to_string(),
            });

            // Spawn extraction in background
            let context_owned = context.clone();
            tokio::spawn(async move {
                let sidecar = crate::sidecar::Sidecar::new();
                match sidecar.extract_memories(&context_owned).await {
                    Ok(extracted) if !extracted.is_empty() => {
                        let manager = crate::memory::MemoryManager::new();
                        let mut stored_count = 0;

                        for mem in extracted {
                            let category = match mem.category.as_str() {
                                "fact" => crate::memory::MemoryCategory::Fact,
                                "preference" => crate::memory::MemoryCategory::Preference,
                                "correction" => crate::memory::MemoryCategory::Correction,
                                _ => crate::memory::MemoryCategory::Fact,
                            };

                            let trust = match mem.trust.as_str() {
                                "high" => crate::memory::TrustLevel::High,
                                "low" => crate::memory::TrustLevel::Low,
                                _ => crate::memory::TrustLevel::Medium,
                            };

                            let entry = crate::memory::MemoryEntry::new(category, &mem.content)
                                .with_source("manual")
                                .with_trust(trust);

                            if manager.remember_project(entry).is_ok() {
                                stored_count += 1;
                            }
                        }

                        crate::logging::info(&format!(
                            "/remember: extracted {} memories",
                            stored_count
                        ));
                        crate::memory::add_event(MemoryEventKind::ExtractionComplete {
                            count: stored_count,
                        });
                        crate::memory::set_state(MemoryState::Idle);
                    }
                    Ok(_) => {
                        crate::logging::info("/remember: no memories extracted");
                        crate::memory::set_state(MemoryState::Idle);
                    }
                    Err(e) => {
                        crate::logging::error(&format!("/remember failed: {}", e));
                        crate::memory::add_event(MemoryEventKind::Error {
                            message: e.to_string(),
                        });
                        crate::memory::set_state(MemoryState::Idle);
                    }
                }
            });

            return;
        }

        if trimmed == "/memory status" {
            let default_enabled = crate::config::config().features.memory;
            self.push_display_message(DisplayMessage::system(format!(
                "Memory feature: **{}** (config default: {})",
                if self.memory_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                if default_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            )));
            return;
        }

        if trimmed == "/memory" {
            let new_state = !self.memory_enabled;
            self.set_memory_feature_enabled(new_state);
            let label = if new_state { "ON" } else { "OFF" };
            self.set_status_notice(&format!("Memory: {}", label));
            self.push_display_message(DisplayMessage::system(format!(
                "Memory feature {} for this session.",
                if new_state { "enabled" } else { "disabled" }
            )));
            return;
        }

        if trimmed == "/memory on" {
            self.set_memory_feature_enabled(true);
            self.set_status_notice("Memory: ON");
            self.push_display_message(DisplayMessage::system(
                "Memory feature enabled for this session.".to_string(),
            ));
            return;
        }

        if trimmed == "/memory off" {
            self.set_memory_feature_enabled(false);
            self.set_status_notice("Memory: OFF");
            self.push_display_message(DisplayMessage::system(
                "Memory feature disabled for this session.".to_string(),
            ));
            return;
        }

        if trimmed.starts_with("/memory ") {
            self.push_display_message(DisplayMessage::error(
                "Usage: `/memory [on|off|status]`".to_string(),
            ));
            return;
        }

        if trimmed == "/swarm" || trimmed == "/swarm status" {
            let default_enabled = crate::config::config().features.swarm;
            self.push_display_message(DisplayMessage::system(format!(
                "Swarm feature: **{}** (config default: {})",
                if self.swarm_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                if default_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            )));
            return;
        }

        if trimmed == "/swarm on" {
            self.set_swarm_feature_enabled(true);
            self.set_status_notice("Swarm: ON");
            self.push_display_message(DisplayMessage::system(
                "Swarm feature enabled for this session.".to_string(),
            ));
            return;
        }

        if trimmed == "/swarm off" {
            self.set_swarm_feature_enabled(false);
            self.set_status_notice("Swarm: OFF");
            self.push_display_message(DisplayMessage::system(
                "Swarm feature disabled for this session.".to_string(),
            ));
            return;
        }

        if trimmed.starts_with("/swarm ") {
            self.push_display_message(DisplayMessage::error(
                "Usage: `/swarm [on|off|status]`".to_string(),
            ));
            return;
        }

        // Handle /rewind command - rewind conversation to a previous point
        if trimmed == "/rewind" {
            // Show numbered history
            if self.session.messages.is_empty() {
                self.push_display_message(DisplayMessage::system(
                    "No messages in conversation.".to_string(),
                ));
                return;
            }

            let mut history = String::from("**Conversation history:**\n\n");
            for (i, msg) in self.session.messages.iter().enumerate() {
                let role_str = match msg.role {
                    Role::User => "👤 User",
                    Role::Assistant => "🤖 Assistant",
                };
                let content = msg.content_preview();
                let preview = crate::util::truncate_str(&content, 80);
                history.push_str(&format!("  `{}` {} - {}\n", i + 1, role_str, preview));
            }
            history.push_str(&format!(
                "\nUse `/rewind N` to rewind to message N (removes all messages after)."
            ));

            self.push_display_message(DisplayMessage::system(history));
            return;
        }

        if let Some(num_str) = trimmed.strip_prefix("/rewind ") {
            let num_str = num_str.trim();
            match num_str.parse::<usize>() {
                Ok(n) if n > 0 && n <= self.session.messages.len() => {
                    let removed = self.session.messages.len() - n;
                    self.session.messages.truncate(n);
                    self.replace_provider_messages(self.session.messages_for_provider());
                    self.session.updated_at = chrono::Utc::now();

                    // Rebuild display messages from session
                    self.clear_display_messages();
                    for rendered in crate::session::render_messages(&self.session) {
                        self.push_display_message(DisplayMessage {
                            role: rendered.role,
                            content: rendered.content,
                            tool_calls: rendered.tool_calls,
                            duration_secs: None,
                            title: None,
                            tool_data: rendered.tool_data,
                        });
                    }

                    // Reset provider session since conversation changed
                    self.provider_session_id = None;
                    self.session.provider_session_id = None;
                    let _ = self.session.save();

                    self.push_display_message(DisplayMessage::system(format!(
                        "✓ Rewound to message {}. Removed {} message{}.",
                        n,
                        removed,
                        if removed == 1 { "" } else { "s" }
                    )));
                }
                Ok(n) => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Invalid message number: {}. Valid range: 1-{}",
                        n,
                        self.session.messages.len()
                    )));
                }
                Err(_) => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Usage: `/rewind N` where N is a message number (1-{})",
                        self.session.messages.len()
                    )));
                }
            }
            return;
        }

        // Handle /config command
        if trimmed == "/config" {
            use crate::config::config;
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: config().display_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if trimmed == "/config init" || trimmed == "/config create" {
            use crate::config::Config;
            match Config::create_default_config_file() {
                Ok(path) => {
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!(
                            "Created default config file at:\n`{}`\n\nEdit this file to customize your keybindings and settings.",
                            path.display()
                        ),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
                Err(e) => {
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!("Failed to create config file: {}", e),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
            }
            return;
        }

        if trimmed == "/config edit" {
            use crate::config::Config;
            if let Some(path) = Config::path() {
                if !path.exists() {
                    // Create default config first
                    if let Err(e) = Config::create_default_config_file() {
                        self.push_display_message(DisplayMessage {
                            role: "system".to_string(),
                            content: format!("Failed to create config file: {}", e),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                        return;
                    }
                }

                // Open in editor
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!(
                        "Opening config in editor...\n`{} {}`\n\n*Restart jcode after editing for changes to take effect.*",
                        editor,
                        path.display()
                    ),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });

                // Spawn editor in background (user will see it after jcode exits or in another terminal)
                let _ = std::process::Command::new(&editor).arg(&path).spawn();
            }
            return;
        }

        // Handle /debug-visual command - toggle visual debugging and dump state
        if trimmed == "/debug-visual" || trimmed == "/debug-visual on" {
            use super::visual_debug;
            visual_debug::enable();
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "Visual debugging enabled. Frames are being captured.\n\
                         Use `/debug-visual dump` to write captured frames to file.\n\
                         Use `/debug-visual off` to disable."
                    .to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            self.set_status_notice("Visual debug: ON");
            return;
        }

        if trimmed == "/debug-visual off" {
            use super::visual_debug;
            visual_debug::disable();
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "Visual debugging disabled.".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            self.set_status_notice("Visual debug: OFF");
            return;
        }

        if trimmed == "/debug-visual dump" {
            use super::visual_debug;
            match visual_debug::dump_to_file() {
                Ok(path) => {
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!(
                            "Visual debug dump written to:\n`{}`\n\n\
                             This file contains frame captures with:\n\
                             - Layout computations\n\
                             - State snapshots\n\
                             - Rendered text content\n\
                             - Any detected anomalies",
                            path.display()
                        ),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
                Err(e) => {
                    self.push_display_message(DisplayMessage {
                        role: "error".to_string(),
                        content: format!("Failed to write visual debug dump: {}", e),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
            }
            return;
        }

        // Handle /screenshot-mode command - toggle screenshot automation
        if trimmed == "/screenshot-mode" || trimmed == "/screenshot-mode on" {
            use super::screenshot;
            screenshot::enable();
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "Screenshot mode enabled.\n\n\
                         Run the watcher in another terminal:\n\
                         ```bash\n\
                         ./scripts/screenshot_watcher.sh\n\
                         ```\n\n\
                         Use `/screenshot <state>` to trigger a capture.\n\
                         Use `/screenshot-mode off` to disable."
                    .to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if trimmed == "/screenshot-mode off" {
            use super::screenshot;
            screenshot::disable();
            screenshot::clear_all_signals();
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "Screenshot mode disabled.".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if trimmed.starts_with("/screenshot ") {
            use super::screenshot;
            let state_name = trimmed.strip_prefix("/screenshot ").unwrap_or("").trim();
            if !state_name.is_empty() {
                screenshot::signal_ready(
                    state_name,
                    serde_json::json!({
                        "manual_trigger": true,
                    }),
                );
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Screenshot signal sent: {}", state_name),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            return;
        }

        // Handle /record command - record user actions for replay
        if trimmed == "/record" || trimmed == "/record start" {
            use super::test_harness;
            test_harness::start_recording();
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "🎬 Recording started.\n\n\
                         All your keystrokes are now being recorded.\n\
                         Use `/record stop` to stop and save.\n\
                         Use `/record cancel` to discard."
                    .to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if trimmed == "/record stop" {
            use super::test_harness;
            test_harness::stop_recording();
            let json = test_harness::get_recorded_events_json();
            let event_count = json.matches("\"type\"").count();

            // Save to file
            let recording_dir = dirs::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("jcode")
                .join("recordings");
            let _ = std::fs::create_dir_all(&recording_dir);

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let filename = format!("recording_{}.json", timestamp);
            let filepath = recording_dir.join(&filename);

            if let Ok(mut file) = std::fs::File::create(&filepath) {
                use std::io::Write;
                let _ = file.write_all(json.as_bytes());
            }

            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: format!(
                    "🎬 Recording stopped.\n\n\
                     **Events recorded:** {}\n\
                     **Saved to:** `{}`\n\n\
                     To replay as video, run:\n\
                     ```bash\n\
                     ./scripts/replay_recording.sh {}\n\
                     ```",
                    event_count,
                    filepath.display(),
                    filepath.display()
                ),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if trimmed == "/record cancel" {
            use super::test_harness;
            test_harness::stop_recording();
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "🎬 Recording cancelled.".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        // Handle /model command - open interactive picker
        if trimmed == "/model" || trimmed == "/models" {
            self.open_model_picker();
            return;
        }

        if let Some(model_name) = trimmed.strip_prefix("/model ") {
            let model_name = model_name.trim();
            match self.provider.set_model(model_name) {
                Ok(()) => {
                    self.provider_session_id = None;
                    self.session.provider_session_id = None;
                    self.upstream_provider = None;
                    self.connection_type = None;
                    let active_model = self.provider.model();
                    self.update_context_limit_for_model(&active_model);
                    self.session.model = Some(active_model.clone());
                    let _ = self.session.save();
                    self.push_display_message(DisplayMessage {
                        role: "system".to_string(),
                        content: format!("✓ Switched to model: {}", active_model),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    self.set_status_notice(format!("Model → {}", model_name));
                }
                Err(e) => {
                    self.push_display_message(DisplayMessage {
                        role: "error".to_string(),
                        content: format!("Failed to switch model: {}", e),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    self.set_status_notice("Model switch failed");
                }
            }
            return;
        }

        if trimmed == "/effort" {
            let current = self.provider.reasoning_effort();
            let efforts = self.provider.available_efforts();
            if efforts.is_empty() {
                self.push_display_message(DisplayMessage::system(
                    "Reasoning effort not available for this provider.".to_string(),
                ));
            } else {
                let current_label = current
                    .as_deref()
                    .map(effort_display_label)
                    .unwrap_or("default");
                let list: Vec<String> = efforts
                    .iter()
                    .map(|e| {
                        if Some(e.to_string()) == current {
                            format!("**{}** ← current", effort_display_label(e))
                        } else {
                            effort_display_label(e).to_string()
                        }
                    })
                    .collect();
                self.push_display_message(DisplayMessage::system(format!(
                    "Reasoning effort: {}\nAvailable: {}\nUse `/effort <level>` or Alt+←/→ to change.",
                    current_label,
                    list.join(" · ")
                )));
            }
            return;
        }

        if let Some(level) = trimmed.strip_prefix("/effort ") {
            let level = level.trim();
            match self.provider.set_reasoning_effort(level) {
                Ok(()) => {
                    let new_effort = self.provider.reasoning_effort();
                    let label = new_effort
                        .as_deref()
                        .map(effort_display_label)
                        .unwrap_or("default");
                    self.push_display_message(DisplayMessage::system(format!(
                        "✓ Reasoning effort → {}",
                        label
                    )));
                    let efforts = self.provider.available_efforts();
                    let idx = new_effort
                        .as_ref()
                        .and_then(|e| efforts.iter().position(|x| *x == e.as_str()))
                        .unwrap_or(0);
                    let bar = effort_bar(idx, efforts.len());
                    self.set_status_notice(format!("Effort: {} {}", label, bar));
                }
                Err(e) => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Failed to set effort: {}",
                        e
                    )));
                }
            }
            return;
        }

        if trimmed == "/version" {
            let version = env!("JCODE_VERSION");
            let is_canary = if self.session.is_canary {
                " (canary/self-dev)"
            } else {
                ""
            };
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: format!("jcode {}{}", version, is_canary),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if trimmed == "/changelog" {
            self.changelog_scroll = Some(0);
            return;
        }

        if trimmed == "/auth" {
            self.show_auth_status();
            return;
        }

        if trimmed == "/login" {
            self.show_interactive_login();
            return;
        }

        if let Some(provider) = trimmed
            .strip_prefix("/login ")
            .or_else(|| trimmed.strip_prefix("/auth "))
        {
            match provider.trim().to_lowercase().as_str() {
                "claude" | "anthropic" => self.start_claude_login(),
                "openai" => self.start_openai_login(),
                "openrouter" => self.start_openrouter_login(),
                "cursor" => self.start_cursor_login(),
                "copilot" => self.start_copilot_login(),
                "antigravity" => self.start_antigravity_login(),
                other => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Unknown provider '{}'. Use: anthropic/claude, openai, openrouter, cursor, copilot, or antigravity",
                        other
                    )));
                }
            }
            return;
        }

        if trimmed == "/account" || trimmed == "/accounts" {
            self.show_accounts();
            return;
        }

        if let Some(sub) = trimmed.strip_prefix("/account ") {
            let parts: Vec<&str> = sub.trim().splitn(2, ' ').collect();
            match parts[0] {
                "list" | "ls" => self.show_accounts(),
                "switch" | "use" => {
                    if let Some(label) = parts.get(1) {
                        self.switch_account(label.trim());
                    } else {
                        self.push_display_message(DisplayMessage::error(
                            "Usage: `/account switch <label>`".to_string(),
                        ));
                    }
                }
                "add" | "login" => {
                    let label = parts.get(1).map(|s| s.trim()).unwrap_or("default");
                    self.start_claude_login_for_account(label);
                }
                "remove" | "rm" | "delete" => {
                    if let Some(label) = parts.get(1) {
                        self.remove_account(label.trim());
                    } else {
                        self.push_display_message(DisplayMessage::error(
                            "Usage: `/account remove <label>`".to_string(),
                        ));
                    }
                }
                other => {
                    let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
                    if accounts.iter().any(|a| a.label == other) {
                        self.switch_account(other);
                    } else {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Unknown subcommand '{}'. Use: list, switch, add, remove",
                            other
                        )));
                    }
                }
            }
            return;
        }

        if trimmed == "/info" {
            let version = env!("JCODE_VERSION");
            let terminal_size = crossterm::terminal::size()
                .map(|(w, h)| format!("{}x{}", w, h))
                .unwrap_or_else(|_| "unknown".to_string());
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            // Count turns (user messages)
            let turn_count = self
                .display_messages
                .iter()
                .filter(|m| m.role == "user")
                .count();

            // Session duration
            let session_duration =
                chrono::Utc::now().signed_duration_since(self.session.created_at);
            let duration_str = if session_duration.num_hours() > 0 {
                format!(
                    "{}h {}m",
                    session_duration.num_hours(),
                    session_duration.num_minutes() % 60
                )
            } else if session_duration.num_minutes() > 0 {
                format!("{}m", session_duration.num_minutes())
            } else {
                format!("{}s", session_duration.num_seconds())
            };

            // Build info string
            let mut info = String::new();
            info.push_str(&format!("**Version:** {}\n", version));
            info.push_str(&format!(
                "**Session:** {} ({})\n",
                self.session.short_name.as_deref().unwrap_or("unnamed"),
                &self.session.id[..8]
            ));
            info.push_str(&format!(
                "**Duration:** {} ({} turns)\n",
                duration_str, turn_count
            ));
            info.push_str(&format!(
                "**Tokens:** ↑{} ↓{}\n",
                self.total_input_tokens, self.total_output_tokens
            ));
            info.push_str(&format!("**Terminal:** {}\n", terminal_size));
            info.push_str(&format!("**CWD:** {}\n", cwd));
            info.push_str(&format!(
                "**Features:** memory={}, swarm={}\n",
                if self.memory_enabled { "on" } else { "off" },
                if self.swarm_enabled { "on" } else { "off" }
            ));

            // Provider info
            if let Some(ref model) = self.remote_provider_model {
                info.push_str(&format!("**Model:** {}\n", model));
            }
            if let Some(ref provider_id) = self.provider_session_id {
                info.push_str(&format!(
                    "**Provider Session:** {}...\n",
                    &provider_id[..provider_id.len().min(16)]
                ));
            }

            // Self-dev specific
            if self.session.is_canary {
                info.push_str("\n**Self-Dev Mode:** enabled\n");
                if let Some(ref build) = self.session.testing_build {
                    info.push_str(&format!("**Testing Build:** {}\n", build));
                }
            }

            // Remote mode info
            if self.is_remote {
                info.push_str(&format!("\n**Remote Mode:** connected\n"));
                if let Some(count) = self.remote_client_count {
                    info.push_str(&format!("**Connected Clients:** {}\n", count));
                }
            }

            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: info,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if trimmed == "/reload" {
            // Smart reload: check if there's a newer binary
            if !self.has_newer_binary() {
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: "No newer binary found. Nothing to reload.\nUse /rebuild to build a new version.".to_string(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                return;
            }
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "Reloading with newer binary...".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            // Save provider session ID for resume after reload
            self.session.provider_session_id = self.provider_session_id.clone();
            // Mark as reloaded and save session
            self.session
                .set_status(crate::session::SessionStatus::Reloaded);
            let _ = self.session.save();
            self.reload_requested = Some(self.session.id.clone());
            self.should_quit = true;
            return;
        }

        if trimmed == "/rebuild" {
            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: "Rebuilding jcode (git pull + cargo build + tests)...".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            // Save provider session ID for resume after rebuild
            self.session.provider_session_id = self.provider_session_id.clone();
            // Mark as reloaded and save session
            self.session
                .set_status(crate::session::SessionStatus::Reloaded);
            let _ = self.session.save();
            self.rebuild_requested = Some(self.session.id.clone());
            self.should_quit = true;
            return;
        }

        if trimmed == "/update" {
            self.push_display_message(DisplayMessage::system(
                "Checking for updates...".to_string(),
            ));
            self.session.provider_session_id = self.provider_session_id.clone();
            self.session
                .set_status(crate::session::SessionStatus::Reloaded);
            let _ = self.session.save();
            self.update_requested = Some(self.session.id.clone());
            self.should_quit = true;
            return;
        }

        // Check for skill invocation
        if let Some(skill_name) = SkillRegistry::parse_invocation(&input) {
            if let Some(skill) = self.skills.get(skill_name) {
                self.active_skill = Some(skill_name.to_string());
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Activated skill: {} - {}", skill.name, skill.description),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            } else {
                self.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Unknown skill: /{}", skill_name),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            return;
        }

        // Add user message to display (show placeholder to user, not full paste)
        self.push_display_message(DisplayMessage {
            role: "user".to_string(),
            content: raw_input, // Show placeholder to user (condensed view)
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        // Send expanded content (with actual pasted text) to model
        let images = std::mem::take(&mut self.pending_images);
        if !images.is_empty() {
            crate::logging::info(&format!(
                "Submitting with {} image(s): {}",
                images.len(),
                images
                    .iter()
                    .map(|(t, d)| format!("{} ({}KB)", t, d.len() / 1024))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if images.is_empty() {
            self.add_provider_message(Message::user(&input));
            self.session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: input.clone(),
                    cache_control: None,
                }],
            );
        } else {
            self.add_provider_message(Message::user_with_images(&input, images.clone()));
            let mut blocks: Vec<ContentBlock> = images
                .into_iter()
                .map(|(media_type, data)| ContentBlock::Image { media_type, data })
                .collect();
            blocks.push(ContentBlock::Text {
                text: input.clone(),
                cache_control: None,
            });
            self.session.add_message(Role::User, blocks);
        }
        let _ = self.session.save();

        // Set up processing state - actual processing happens after UI redraws
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.clear_streaming_render_state();
        self.stream_buffer.clear();
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.streaming_cache_read_tokens = None;
        self.streaming_cache_creation_tokens = None;
        self.upstream_provider = None;
        self.streaming_tps_start = None;
        self.streaming_tps_elapsed = Duration::ZERO;
        self.streaming_total_output_tokens = 0;
        self.processing_started = Some(Instant::now());
        self.pending_turn = true;
    }

    /// Process all queued messages (combined into a single request)
    /// Loops until queue is empty (in case more messages are queued during processing)
    async fn process_queued_messages(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) {
        while !self.queued_messages.is_empty() {
            // Combine all currently queued messages into one
            let messages = std::mem::take(&mut self.queued_messages);
            let combined = messages.join("\n\n");

            // Display each queued message as its own user prompt
            for msg in &messages {
                self.push_display_message(DisplayMessage::user(msg.clone()));
            }

            self.add_provider_message(Message::user(&combined));
            self.session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: combined,
                    cache_control: None,
                }],
            );
            let _ = self.session.save();
            self.clear_streaming_render_state();
            self.stream_buffer.clear();
            self.thought_line_inserted = false;
            self.thinking_prefix_emitted = false;
            self.thinking_buffer.clear();
            self.streaming_tool_calls.clear();
            self.streaming_input_tokens = 0;
            self.streaming_output_tokens = 0;
            self.streaming_cache_read_tokens = None;
            self.streaming_cache_creation_tokens = None;
            self.upstream_provider = None;
            self.streaming_tps_start = None;
            self.streaming_tps_elapsed = Duration::ZERO;
            self.streaming_total_output_tokens = 0;
            self.processing_started = Some(Instant::now());
            self.is_processing = true;
            self.status = ProcessingStatus::Sending;

            match self.run_turn_interactive(terminal, event_stream).await {
                Ok(()) => {
                    self.last_stream_error = None;
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if is_context_limit_error(&err_str) {
                        if self
                            .try_auto_compact_and_retry(terminal, event_stream)
                            .await
                        {
                            // Successfully recovered
                        } else {
                            self.handle_turn_error(err_str);
                        }
                    } else {
                        self.handle_turn_error(err_str);
                    }
                }
            }
            // Loop will check if more messages were queued during this turn
        }
    }

    fn cycle_model(&mut self, direction: i8) {
        let models = self.provider.available_models();
        if models.is_empty() {
            self.push_display_message(DisplayMessage::error(
                "Model switching is not available for this provider.",
            ));
            self.set_status_notice("Model switching not available");
            return;
        }

        let current = self.provider.model();
        let current_index = models.iter().position(|m| *m == current).unwrap_or(0);

        let len = models.len();
        let next_index = if direction >= 0 {
            (current_index + 1) % len
        } else {
            (current_index + len - 1) % len
        };
        let next_model = models[next_index];

        match self.provider.set_model(next_model) {
            Ok(()) => {
                self.provider_session_id = None;
                self.session.provider_session_id = None;
                self.upstream_provider = None;
                self.connection_type = None;
                self.update_context_limit_for_model(next_model);
                self.session.model = Some(self.provider.model());
                let _ = self.session.save();
                self.push_display_message(DisplayMessage::system(format!(
                    "✓ Switched to model: {}",
                    next_model
                )));
                self.set_status_notice(format!("Model → {}", next_model));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch model: {}",
                    e
                )));
                self.set_status_notice("Model switch failed");
            }
        }
    }

    fn cycle_effort(&mut self, direction: i8) {
        let efforts = self.provider.available_efforts();
        if efforts.is_empty() {
            self.set_status_notice("Reasoning effort not available for this provider");
            return;
        }

        let current = self.provider.reasoning_effort();
        let current_index = current
            .as_ref()
            .and_then(|c| efforts.iter().position(|e| *e == c.as_str()))
            .unwrap_or(efforts.len() - 1); // default to last (xhigh)

        let len = efforts.len();
        let next_index = if direction > 0 {
            if current_index + 1 >= len {
                current_index // already at max
            } else {
                current_index + 1
            }
        } else {
            if current_index == 0 {
                0 // already at min
            } else {
                current_index - 1
            }
        };

        let next_effort = efforts[next_index];
        if Some(next_effort.to_string()) == current {
            let label = effort_display_label(next_effort);
            self.set_status_notice(format!(
                "Effort: {} (already at {})",
                label,
                if direction > 0 { "max" } else { "min" }
            ));
            return;
        }

        match self.provider.set_reasoning_effort(next_effort) {
            Ok(()) => {
                let label = effort_display_label(next_effort);
                let bar = effort_bar(next_index, len);
                self.set_status_notice(format!("Effort: {} {}", label, bar));
            }
            Err(e) => {
                self.set_status_notice(format!("Effort switch failed: {}", e));
            }
        }
    }

    fn update_context_limit_for_model(&mut self, model: &str) {
        let limit = if self.is_remote {
            crate::provider::context_limit_for_model(model)
                .unwrap_or(self.provider.context_window())
        } else {
            self.provider.context_window()
        };
        self.context_limit = limit as u64;
        self.context_warning_shown = false;

        // Also update compaction manager's budget
        {
            let compaction = self.registry.compaction();
            if let Ok(mut manager) = compaction.try_write() {
                manager.set_budget(limit);
            };
        }
    }

    fn effective_context_tokens_from_usage(
        &self,
        input_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> u64 {
        if input_tokens == 0 {
            return 0;
        }
        let cache_read = cache_read_input_tokens.unwrap_or(0);
        let cache_creation = cache_creation_input_tokens.unwrap_or(0);
        let provider_name = if self.is_remote {
            self.remote_provider_name.clone().unwrap_or_default()
        } else {
            self.provider.name().to_string()
        }
        .to_lowercase();

        // Some providers report cache tokens as separate counters, others report them as subsets.
        // When in doubt, avoid over-counting unless we have strong evidence of split accounting.
        let split_cache_accounting = provider_name.contains("anthropic")
            || provider_name.contains("claude")
            || cache_creation > 0
            || cache_read > input_tokens;

        if split_cache_accounting {
            input_tokens
                .saturating_add(cache_read)
                .saturating_add(cache_creation)
        } else {
            input_tokens
        }
    }

    fn current_stream_context_tokens(&self) -> Option<u64> {
        if self.streaming_input_tokens == 0 {
            return None;
        }
        Some(self.effective_context_tokens_from_usage(
            self.streaming_input_tokens,
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
        ))
    }

    fn update_compaction_usage_from_stream(&mut self) {
        if self.is_remote || !self.provider.supports_compaction() {
            return;
        }
        let Some(tokens) = self.current_stream_context_tokens() else {
            return;
        };
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.update_observed_input_tokens(tokens);
        };
    }

    fn handle_turn_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        self.last_stream_error = Some(error.clone());

        if is_context_limit_error(&error) {
            let recovery = self.auto_recover_context_limit();
            let hint = match recovery {
                Some(msg) => format!(" {}", msg),
                None => " Context limit exceeded but auto-recovery failed. Run `/fix` to try manual recovery.".to_string(),
            };
            self.push_display_message(DisplayMessage::error(format!("Error: {}{}", error, hint)));
        } else {
            self.push_display_message(DisplayMessage::error(format!(
                "Error: {} Run `/fix` to attempt recovery.",
                error
            )));
        }
    }

    fn auto_recover_context_limit(&mut self) -> Option<String> {
        if self.is_remote || !self.provider.supports_compaction() {
            return None;
        }
        let compaction = self.registry.compaction();
        let mut manager = compaction.try_write().ok()?;

        let usage = manager.context_usage_with(&self.messages);
        if usage > 1.5 {
            match manager.hard_compact_with(&self.messages) {
                Ok(dropped) => {
                    let post_usage = manager.context_usage_with(&self.messages);
                    if post_usage <= 1.0 {
                        return Some(format!(
                            "⚡ Emergency compaction: dropped {} old messages (context was at {:.0}%). You can continue.",
                            dropped,
                            usage * 100.0
                        ));
                    }
                    let truncated = manager.emergency_truncate_with(&mut self.messages);
                    return Some(format!(
                        "⚡ Emergency compaction: dropped {} old messages and truncated {} tool result(s) (context was at {:.0}%). You can continue.",
                        dropped, truncated,
                        usage * 100.0
                    ));
                }
                Err(reason) => {
                    crate::logging::error(&format!(
                        "[auto_recover] hard_compact failed: {}",
                        reason
                    ));
                    let truncated = manager.emergency_truncate_with(&mut self.messages);
                    if truncated > 0 {
                        return Some(format!(
                            "⚡ Emergency truncation: shortened {} large tool result(s) to fit context. You can continue.",
                            truncated
                        ));
                    }
                }
            }
        }

        let observed_tokens = self
            .current_stream_context_tokens()
            .unwrap_or(self.context_limit as u64);
        manager.update_observed_input_tokens(observed_tokens);

        match manager.force_compact_with(&self.messages, self.provider.clone()) {
            Ok(()) => Some(
                "⚡ Auto-compaction started — summarizing old messages in background. Retry in a moment."
                    .to_string(),
            ),
            Err(reason) => {
                crate::logging::error(&format!(
                    "[auto_recover] force_compact failed: {}",
                    reason
                ));
                match manager.hard_compact_with(&self.messages) {
                    Ok(dropped) => Some(format!(
                        "⚡ Emergency compaction: dropped {} old messages. You can continue.",
                        dropped
                    )),
                    Err(_) => {
                        let truncated = manager.emergency_truncate_with(&mut self.messages);
                        if truncated > 0 {
                            Some(format!(
                                "⚡ Emergency truncation: shortened {} large tool result(s) to fit context. You can continue.",
                                truncated
                            ))
                        } else {
                            None
                        }
                    }
                }
            }
        }
    }

    /// Attempt automatic compaction and retry when context limit is exceeded.
    /// Returns true if the retry succeeded.
    async fn try_auto_compact_and_retry(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) -> bool {
        if self.is_remote || !self.provider.supports_compaction() {
            return false;
        }

        self.push_display_message(DisplayMessage::system(
            "⚠️ Context limit exceeded — auto-compacting and retrying...".to_string(),
        ));

        // Force the compaction manager to think we're at the limit
        let compaction = self.registry.compaction();
        let compact_started = match compaction.try_write() {
            Ok(mut manager) => {
                manager.update_observed_input_tokens(self.context_limit);
                let usage = manager.context_usage_with(&self.messages);
                if usage > 1.5 {
                    match manager.hard_compact_with(&self.messages) {
                        Ok(dropped) => {
                            self.push_display_message(DisplayMessage::system(
                                format!(
                                    "⚡ Emergency compaction: dropped {} old messages (context was at {:.0}%).",
                                    dropped,
                                    usage * 100.0
                                ),
                            ));
                            drop(manager);
                            self.provider_session_id = None;
                            self.session.provider_session_id = None;
                            self.context_warning_shown = false;
                            self.clear_streaming_render_state();
                            self.stream_buffer.clear();
                            self.streaming_tool_calls.clear();
                            self.streaming_input_tokens = 0;
                            self.streaming_output_tokens = 0;
                            self.streaming_cache_read_tokens = None;
                            self.streaming_cache_creation_tokens = None;
                            self.thought_line_inserted = false;
                            self.thinking_prefix_emitted = false;
                            self.thinking_buffer.clear();
                            self.status = ProcessingStatus::Sending;

                            self.push_display_message(DisplayMessage::system(
                                "✓ Context compacted. Retrying...".to_string(),
                            ));
                            return match self.run_turn_interactive(terminal, event_stream).await {
                                Ok(()) => {
                                    self.last_stream_error = None;
                                    true
                                }
                                Err(e) => {
                                    self.handle_turn_error(e.to_string());
                                    false
                                }
                            };
                        }
                        Err(_) => {
                            let truncated = manager.emergency_truncate_with(&mut self.messages);
                            if truncated > 0 {
                                drop(manager);
                                self.provider_session_id = None;
                                self.session.provider_session_id = None;
                                self.context_warning_shown = false;
                                self.clear_streaming_render_state();
                                self.stream_buffer.clear();
                                self.streaming_tool_calls.clear();
                                self.streaming_input_tokens = 0;
                                self.streaming_output_tokens = 0;
                                self.streaming_cache_read_tokens = None;
                                self.streaming_cache_creation_tokens = None;
                                self.thought_line_inserted = false;
                                self.thinking_prefix_emitted = false;
                                self.thinking_buffer.clear();
                                self.status = ProcessingStatus::Sending;

                                self.push_display_message(DisplayMessage::system(
                                    format!("⚡ Emergency truncation: shortened {} large tool result(s). Retrying...", truncated),
                                ));
                                return match self.run_turn_interactive(terminal, event_stream).await
                                {
                                    Ok(()) => {
                                        self.last_stream_error = None;
                                        true
                                    }
                                    Err(e) => {
                                        self.handle_turn_error(e.to_string());
                                        false
                                    }
                                };
                            }
                            false
                        }
                    }
                } else {
                    match manager.force_compact_with(&self.messages, self.provider.clone()) {
                        Ok(()) => true,
                        Err(_) => match manager.hard_compact_with(&self.messages) {
                            Ok(_) => {
                                drop(manager);
                                self.provider_session_id = None;
                                self.session.provider_session_id = None;
                                self.context_warning_shown = false;
                                self.clear_streaming_render_state();
                                self.stream_buffer.clear();
                                self.streaming_tool_calls.clear();
                                self.streaming_input_tokens = 0;
                                self.streaming_output_tokens = 0;
                                self.streaming_cache_read_tokens = None;
                                self.streaming_cache_creation_tokens = None;
                                self.thought_line_inserted = false;
                                self.thinking_prefix_emitted = false;
                                self.thinking_buffer.clear();
                                self.status = ProcessingStatus::Sending;

                                self.push_display_message(DisplayMessage::system(
                                    "✓ Context compacted (emergency). Retrying...".to_string(),
                                ));
                                return match self.run_turn_interactive(terminal, event_stream).await
                                {
                                    Ok(()) => {
                                        self.last_stream_error = None;
                                        true
                                    }
                                    Err(e) => {
                                        self.handle_turn_error(e.to_string());
                                        false
                                    }
                                };
                            }
                            Err(_) => false,
                        },
                    }
                }
            }
            Err(_) => false,
        };

        if !compact_started {
            return false;
        }

        // Wait for compaction to finish (up to 60s)
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        self.status = ProcessingStatus::RunningTool("compacting context...".to_string());

        loop {
            if std::time::Instant::now() >= deadline {
                self.push_display_message(DisplayMessage::error(
                    "Auto-compaction timed out.".to_string(),
                ));
                return false;
            }

            // Redraw UI while we wait
            let _ = terminal.draw(|frame| crate::tui::ui::draw(frame, self));

            let compaction = self.registry.compaction();
            let done = if let Ok(mut manager) = compaction.try_write() {
                if let Some(event) = manager.poll_compaction_event() {
                    self.handle_compaction_event(event);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if done {
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        self.push_display_message(DisplayMessage::system(
            "✓ Context compacted. Retrying...".to_string(),
        ));

        // Reset provider session since context changed
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.context_warning_shown = false;

        // Clear streaming state for the retry
        self.clear_streaming_render_state();
        self.stream_buffer.clear();
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.streaming_cache_read_tokens = None;
        self.streaming_cache_creation_tokens = None;
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.status = ProcessingStatus::Sending;

        // Retry the turn
        match self.run_turn_interactive(terminal, event_stream).await {
            Ok(()) => {
                self.last_stream_error = None;
                true
            }
            Err(e) => {
                self.handle_turn_error(e.to_string());
                false
            }
        }
    }

    fn handle_usage_report(&mut self, results: Vec<crate::usage::ProviderUsage>) {
        if results.is_empty() {
            self.push_display_message(DisplayMessage::system(
                "No providers with OAuth credentials found.\n\
                 Use `/login anthropic` or `/login openai` to authenticate."
                    .to_string(),
            ));
            return;
        }

        let mut output = String::from("## Subscription Usage\n\n");

        for (i, provider) in results.iter().enumerate() {
            if i > 0 {
                output.push_str("---\n\n");
            }
            output.push_str(&format!("### {}\n\n", provider.provider_name));

            if let Some(ref err) = provider.error {
                output.push_str(&format!("⚠ {}\n\n", err));
                continue;
            }

            if provider.limits.is_empty() && provider.extra_info.is_empty() {
                output.push_str("No usage data available\n\n");
                continue;
            }

            for limit in &provider.limits {
                let bar = crate::usage::format_usage_bar(limit.usage_percent, 15);
                let reset_info = if let Some(ref ts) = limit.resets_at {
                    let relative = crate::usage::format_reset_time(ts);
                    format!(" (resets in {})", relative)
                } else {
                    String::new()
                };
                output.push_str(&format!("- **{}**: {}{}\n", limit.name, bar, reset_info));
            }

            if !provider.limits.is_empty() {
                output.push('\n');
            }

            for (key, value) in &provider.extra_info {
                output.push_str(&format!("- {}: {}\n", key, value));
            }
            output.push('\n');
        }

        if self.total_input_tokens > 0 || self.total_output_tokens > 0 {
            output.push_str("---\n\n### Session Usage\n\n");
            output.push_str(&format!(
                "- **Input tokens:** {}\n- **Output tokens:** {}\n",
                self.total_input_tokens, self.total_output_tokens
            ));
            if self.total_cost > 0.0 {
                output.push_str(&format!("- **Cost:** ${:.4}\n", self.total_cost));
            }
            output.push('\n');
        }

        self.push_display_message(DisplayMessage::system(output));
    }

    fn run_fix_command(&mut self) {
        let mut actions: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let last_error = self.last_stream_error.clone();
        let context_error = last_error
            .as_deref()
            .map(is_context_limit_error)
            .unwrap_or(false);

        let repaired = self.repair_missing_tool_outputs();
        if repaired > 0 {
            actions.push(format!("Recovered {} missing tool output(s).", repaired));
        }

        if self.summarize_tool_results_missing().is_some() {
            self.recover_session_without_tools();
            actions.push("Created a recovery session with text-only history.".to_string());
        }

        if self.provider_session_id.is_some() || self.session.provider_session_id.is_some() {
            self.provider_session_id = None;
            self.session.provider_session_id = None;
            actions.push("Reset provider session resume state.".to_string());
        }

        if !self.is_remote && self.provider.supports_compaction() {
            let observed_tokens = self
                .current_stream_context_tokens()
                .or_else(|| context_error.then_some(self.context_limit));
            let compaction = self.registry.compaction();
            match compaction.try_write() {
                Ok(mut manager) => {
                    if let Some(tokens) = observed_tokens {
                        manager.update_observed_input_tokens(tokens);
                    }
                    let usage = manager.context_usage_with(&self.messages);
                    if usage > 1.5 {
                        match manager.hard_compact_with(&self.messages) {
                            Ok(dropped) => {
                                actions.push(format!(
                                    "Emergency compaction: dropped {} old messages (context was at {:.0}%).",
                                    dropped,
                                    usage * 100.0
                                ));
                            }
                            Err(reason) => {
                                notes.push(format!("Hard compaction failed: {}", reason));
                            }
                        }
                        let post_usage = manager.context_usage_with(&self.messages);
                        if post_usage > 1.0 {
                            let truncated = manager.emergency_truncate_with(&mut self.messages);
                            if truncated > 0 {
                                actions.push(format!(
                                    "Emergency truncation: shortened {} large tool result(s) to fit context.",
                                    truncated
                                ));
                            }
                        }
                    } else {
                        match manager.force_compact_with(&self.messages, self.provider.clone()) {
                            Ok(()) => {
                                actions.push("Started background context compaction.".to_string())
                            }
                            Err(reason) => match manager.hard_compact_with(&self.messages) {
                                Ok(dropped) => {
                                    actions.push(format!(
                                            "Emergency compaction: dropped {} old messages (normal compaction failed: {}).",
                                            dropped, reason
                                        ));
                                }
                                Err(hard_reason) => {
                                    notes.push(format!(
                                        "Compaction not started: {}. Emergency fallback: {}",
                                        reason, hard_reason
                                    ));
                                }
                            },
                        }
                    }
                }
                Err(_) => notes.push("Could not access compaction manager (busy).".to_string()),
            };
        } else {
            notes.push("Compaction is unavailable for this provider.".to_string());
        }

        self.context_warning_shown = false;
        self.last_stream_error = None;
        self.set_status_notice("Fix applied");

        let mut content = String::from("**Fix Results:**\n");
        if actions.is_empty() {
            content.push_str("• No structural issues detected.\n");
        } else {
            for action in &actions {
                content.push_str(&format!("• {}\n", action));
            }
        }
        for note in &notes {
            content.push_str(&format!("• {}\n", note));
        }
        if let Some(last_error) = &last_error {
            content.push_str(&format!(
                "\nLast error: `{}`",
                crate::util::truncate_str(last_error, 200)
            ));
        }
        self.push_display_message(DisplayMessage::system(content));
    }

    fn add_provider_message(&mut self, message: Message) {
        self.messages.push(message);
        if self.is_remote || !self.provider.supports_compaction() {
            return;
        }
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.notify_message_added();
        };
    }

    fn replace_provider_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.last_injected_memory_signature = None;
        self.rebuild_tool_result_index();
        self.reseed_compaction_from_provider_messages();
    }

    fn clear_provider_messages(&mut self) {
        self.messages.clear();
        self.last_injected_memory_signature = None;
        self.tool_result_ids.clear();
        self.reseed_compaction_from_provider_messages();
    }

    fn rebuild_tool_result_index(&mut self) {
        self.tool_result_ids.clear();
        for msg in &self.messages {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        self.tool_result_ids.insert(tool_use_id.clone());
                    }
                }
            }
        }
    }

    fn reseed_compaction_from_provider_messages(&mut self) {
        if self.is_remote || !self.provider.supports_compaction() {
            return;
        }
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.reset();
            manager.set_budget(self.context_limit as usize);
            for _ in &self.messages {
                manager.notify_message_added();
            }
        };
    }

    fn messages_for_provider(&mut self) -> (Vec<Message>, Option<CompactionEvent>) {
        if self.is_remote || !self.provider.supports_compaction() {
            return (self.messages.clone(), None);
        }
        let compaction = self.registry.compaction();
        let result = match compaction.try_write() {
            Ok(mut manager) => {
                let action = manager.ensure_context_fits(&self.messages, self.provider.clone());
                match action {
                    crate::compaction::CompactionAction::BackgroundStarted => {
                        self.push_display_message(DisplayMessage::system(
                            "📦 Context above 80% — summarizing older messages in background..."
                                .to_string(),
                        ));
                    }
                    crate::compaction::CompactionAction::HardCompacted(dropped) => {
                        self.push_display_message(DisplayMessage::system(format!(
                            "📦 Context critically full — dropped {} old messages to fit.",
                            dropped,
                        )));
                    }
                    crate::compaction::CompactionAction::None => {}
                }
                let messages = manager.messages_for_api_with(&self.messages);
                let event = manager.take_compaction_event();
                (messages, event)
            }
            Err(_) => (self.messages.clone(), None),
        };
        result
    }

    fn poll_compaction_completion(&mut self) {
        if self.is_remote || !self.provider.supports_compaction() {
            return;
        }
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            if let Some(event) = manager.poll_compaction_event() {
                self.handle_compaction_event(event);
            }
        };
    }

    fn handle_compaction_event(&mut self, event: CompactionEvent) {
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.context_warning_shown = false;
        let tokens_str = event
            .pre_tokens
            .map(|t| format!(" ({} tokens)", t))
            .unwrap_or_default();
        self.push_display_message(DisplayMessage::system(format!(
            "📦 Context compacted ({}){}",
            event.trigger, tokens_str
        )));
    }

    fn set_status_notice(&mut self, text: impl Into<String>) {
        self.status_notice = Some((text.into(), Instant::now()));
    }

    fn set_memory_feature_enabled(&mut self, enabled: bool) {
        self.memory_enabled = enabled;
        if !enabled {
            crate::memory::clear_pending_memory();
            crate::memory::clear_activity();
            crate::memory_agent::reset();
            self.last_injected_memory_signature = None;
        }
    }

    fn memory_prompt_signature(prompt: &str) -> String {
        prompt
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<String>>()
            .join("\n")
    }

    fn should_inject_memory_context(&mut self, prompt: &str) -> bool {
        let signature = Self::memory_prompt_signature(prompt);
        let now = Instant::now();
        if let Some((last_signature, last_injected_at)) =
            self.last_injected_memory_signature.as_ref()
        {
            if *last_signature == signature
                && now.duration_since(*last_injected_at).as_secs()
                    < MEMORY_INJECTION_SUPPRESSION_SECS
            {
                return false;
            }
        }
        self.last_injected_memory_signature = Some((signature, now));
        true
    }

    fn set_swarm_feature_enabled(&mut self, enabled: bool) {
        self.swarm_enabled = enabled;
        if !enabled {
            self.remote_swarm_members.clear();
        }
    }

    fn show_auth_status(&mut self) {
        let status = crate::auth::AuthStatus::check();
        let icon = |state: crate::auth::AuthState| match state {
            crate::auth::AuthState::Available => "✓",
            crate::auth::AuthState::Expired => "⚠ expired",
            crate::auth::AuthState::NotConfigured => "✗",
        };

        let claude_detail = if status.anthropic.has_oauth && status.anthropic.has_api_key {
            "OAuth + API key"
        } else if status.anthropic.has_oauth {
            "OAuth"
        } else if status.anthropic.has_api_key {
            "API key"
        } else {
            "not configured"
        };

        let openai_detail = if status.openai_has_oauth && status.openai_has_api_key {
            "OAuth + API key"
        } else if status.openai_has_oauth {
            "OAuth"
        } else if status.openai_has_api_key {
            "API key"
        } else {
            "not configured"
        };

        let account_info = {
            let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
            if accounts.len() > 1 {
                let active =
                    crate::auth::claude::active_account_label().unwrap_or_else(|| "?".to_string());
                format!(" ({} accounts, active: `{}`)", accounts.len(), active)
            } else if accounts.len() == 1 {
                format!(" (account: `{}`)", accounts[0].label)
            } else {
                String::new()
            }
        };

        self.push_display_message(DisplayMessage::system(format!(
            "**Authentication Status:**\n\n\
             | Provider | Status | Method |\n\
             |----------|--------|--------|\n\
             | Anthropic | {} | {}{} |\n\
             | OpenAI | {} | {} |\n\
             | OpenRouter | {} | API key |\n\
             | Cursor | {} | CLI login |\n\
             | Copilot | {} | CLI login |\n\
             | Antigravity | {} | CLI login |\n\n\
             Use `/login <provider>` to authenticate, `/account` to manage Anthropic accounts.",
            icon(status.anthropic.state),
            claude_detail,
            account_info,
            icon(status.openai),
            openai_detail,
            icon(status.openrouter),
            icon(status.cursor),
            icon(status.copilot),
            icon(status.antigravity),
        )));
    }

    fn show_interactive_login(&mut self) {
        let status = crate::auth::AuthStatus::check();
        let icon = |state: crate::auth::AuthState| match state {
            crate::auth::AuthState::Available => "✓",
            crate::auth::AuthState::Expired => "⚠",
            crate::auth::AuthState::NotConfigured => "✗",
        };

        self.push_display_message(DisplayMessage::system(format!(
            "**Login** - select a provider:\n\n\
             | # | Provider | Status |\n\
             |---|----------|--------|\n\
             | 1 | Anthropic/Claude | {} |\n\
             | 2 | OpenAI | {} |\n\
             | 3 | OpenRouter | {} |\n\
             | 4 | Cursor | {} |\n\
             | 5 | Copilot | {} |\n\
             | 6 | Antigravity | {} |\n\n\
             Type a number (1-6) or provider name, or `/cancel` to cancel.",
            icon(status.anthropic.state),
            icon(status.openai),
            icon(status.openrouter),
            icon(status.cursor),
            icon(status.copilot),
            icon(status.antigravity),
        )));
        self.pending_login = Some(PendingLogin::ProviderSelection);
    }

    fn start_claude_login(&mut self) {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use sha2::{Digest, Sha256};

        let verifier: String = {
            use rand::Rng;
            const CHARSET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            let mut rng = rand::rng();
            (0..64)
                .map(|_| {
                    let idx = rng.random_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect()
        };

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(_) => {
                self.start_claude_login_manual();
                return;
            }
        };
        let port = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(_) => {
                self.start_claude_login_manual();
                return;
            }
        };
        drop(listener);

        let redirect_uri = format!("http://localhost:{}/callback", port);

        let auth_url = format!(
            "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            crate::auth::oauth::claude::AUTHORIZE_URL,
            crate::auth::oauth::claude::CLIENT_ID,
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(crate::auth::oauth::claude::SCOPES),
            challenge,
            verifier,
        );

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             Waiting for callback on port {}...\n\
             If the browser didn't open, visit:\n{}\n\n\
             Or paste the authorization code here to complete manually.",
            port, auth_url
        )));
        self.set_status_notice("Login: waiting for browser...");
        self.pending_login = Some(PendingLogin::Claude {
            verifier: verifier.clone(),
            redirect_uri: Some(redirect_uri.clone()),
        });

        let verifier_clone = verifier;
        let redirect_clone = redirect_uri;
        tokio::spawn(async move {
            match crate::auth::oauth::wait_for_callback_async(port, &verifier_clone).await {
                Ok(code) => {
                    match Self::claude_token_exchange(
                        verifier_clone,
                        code,
                        "default",
                        Some(redirect_clone),
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "claude".to_string(),
                                success: false,
                                message: format!("Claude login failed: {}", e),
                            }));
                        }
                    }
                }
                Err(e) => {
                    crate::logging::info(&format!(
                        "Callback server error (user may paste manually): {}",
                        e
                    ));
                }
            }
        });
    }

    fn start_claude_login_manual(&mut self) {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use sha2::{Digest, Sha256};

        let verifier: String = {
            use rand::Rng;
            const CHARSET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            let mut rng = rand::rng();
            (0..64)
                .map(|_| {
                    let idx = rng.random_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect()
        };

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        let auth_url = format!(
            "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            crate::auth::oauth::claude::AUTHORIZE_URL,
            crate::auth::oauth::claude::CLIENT_ID,
            urlencoding::encode(crate::auth::oauth::claude::REDIRECT_URI),
            urlencoding::encode(crate::auth::oauth::claude::SCOPES),
            challenge,
            verifier,
        );

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             After logging in, copy the authorization code and **paste it here**.",
            auth_url
        )));
        self.set_status_notice("Login: paste code...");
        self.pending_login = Some(PendingLogin::Claude {
            verifier,
            redirect_uri: None,
        });
    }

    fn start_claude_login_for_account(&mut self, label: &str) {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use sha2::{Digest, Sha256};

        let verifier: String = {
            use rand::Rng;
            const CHARSET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            let mut rng = rand::rng();
            (0..64)
                .map(|_| {
                    let idx = rng.random_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect()
        };

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        let auth_url = format!(
            "{}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            crate::auth::oauth::claude::AUTHORIZE_URL,
            crate::auth::oauth::claude::CLIENT_ID,
            urlencoding::encode(crate::auth::oauth::claude::REDIRECT_URI),
            urlencoding::encode(crate::auth::oauth::claude::SCOPES),
            challenge,
            verifier,
        );

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login** (account: `{}`)\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             After logging in, copy the authorization code and **paste it here**.",
            label, auth_url
        )));
        self.set_status_notice(&format!("Login [{}]: paste code...", label));
        self.pending_login = Some(PendingLogin::ClaudeAccount {
            verifier,
            label: label.to_string(),
            redirect_uri: None,
        });
    }

    fn show_accounts(&mut self) {
        let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
        let active_label = crate::auth::claude::active_account_label();
        let now_ms = chrono::Utc::now().timestamp_millis();

        if accounts.is_empty() {
            self.push_display_message(DisplayMessage::system(
                "**Anthropic Accounts:** none configured\n\n\
                 Use `/account add <label>` to add an account, or `/login claude` for a default account."
                    .to_string(),
            ));
            return;
        }

        let mut lines = vec!["**Anthropic Accounts:**\n".to_string()];
        lines.push("| Account | Status | Subscription | Active |".to_string());
        lines.push("|---------|--------|-------------|--------|".to_string());

        for account in &accounts {
            let is_active = active_label.as_deref() == Some(&account.label);
            let status = if account.expires > now_ms {
                "✓ valid"
            } else {
                "⚠ expired"
            };
            let sub = account.subscription_type.as_deref().unwrap_or("unknown");
            let active_mark = if is_active { "◉" } else { "" };
            lines.push(format!(
                "| {} | {} | {} | {} |",
                account.label, status, sub, active_mark
            ));
        }

        lines.push(String::new());
        lines.push("Commands: `/account switch <label>`, `/account add <label>`, `/account remove <label>`".to_string());

        self.push_display_message(DisplayMessage::system(lines.join("\n")));
    }

    fn switch_account(&mut self, label: &str) {
        match crate::auth::claude::set_active_account(label) {
            Ok(()) => {
                {
                    let provider = self.provider.clone();
                    let label_owned = label.to_string();
                    tokio::spawn(async move {
                        provider.invalidate_credentials().await;
                        crate::logging::info(&format!(
                            "Switched to Anthropic account '{}'",
                            label_owned
                        ));
                    });
                }
                self.push_display_message(DisplayMessage::system(format!(
                    "Switched to Anthropic account `{}`.",
                    label
                )));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to switch account: {}",
                    e
                )));
            }
        }
    }

    fn remove_account(&mut self, label: &str) {
        match crate::auth::claude::remove_account(label) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "Removed Anthropic account `{}`.",
                    label
                )));
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to remove account: {}",
                    e
                )));
            }
        }
    }

    fn start_openai_login(&mut self) {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use sha2::{Digest, Sha256};

        let verifier: String = {
            use rand::Rng;
            const CHARSET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            let mut rng = rand::rng();
            (0..64)
                .map(|_| {
                    let idx = rng.random_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect()
        };

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        let state: String = {
            let bytes: [u8; 16] = rand::random();
            hex::encode(bytes)
        };

        let port = crate::auth::oauth::openai::DEFAULT_PORT;
        let redirect_uri = crate::auth::oauth::openai::redirect_uri(port);

        let auth_url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=codex_cli_rs",
            crate::auth::oauth::openai::AUTHORIZE_URL,
            crate::auth::oauth::openai::CLIENT_ID,
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(crate::auth::oauth::openai::SCOPES),
            challenge,
            state,
        );

        let _ = open::that(&auth_url);

        let verifier_clone = verifier.clone();
        let state_clone = state.clone();
        tokio::spawn(async move {
            match Self::openai_login_callback(verifier_clone, state_clone).await {
                Ok(msg) => {
                    crate::logging::info(&format!("OpenAI login: {}", msg));
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "openai".to_string(),
                        success: true,
                        message: msg,
                    }));
                }
                Err(e) => {
                    crate::logging::error(&format!("OpenAI login failed: {}", e));
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "openai".to_string(),
                        success: false,
                        message: format!("OpenAI login failed: {}", e),
                    }));
                }
            }
        });

        self.push_display_message(DisplayMessage::system(format!(
            "**OpenAI OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             Waiting for callback on `localhost:{}`... (this will complete automatically)",
            auth_url, port
        )));
        self.set_status_notice("Login: waiting…");
    }

    async fn openai_login_callback(
        verifier: String,
        expected_state: String,
    ) -> Result<String, String> {
        let port = crate::auth::oauth::openai::DEFAULT_PORT;
        let redirect_uri = crate::auth::oauth::openai::redirect_uri(port);
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            crate::auth::oauth::wait_for_callback_async(port, &expected_state),
        )
        .await
        .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())?
        .map_err(|e| format!("Callback failed: {}", e))?;

        let client = reqwest::Client::new();
        let resp = client
            .post(crate::auth::oauth::openai::TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
                crate::auth::oauth::openai::CLIENT_ID,
                code,
                verifier,
                urlencoding::encode(&redirect_uri)
            ))
            .send()
            .await
            .map_err(|e| format!("Token exchange request failed: {}", e))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Token exchange failed: {}", text));
        }

        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
            expires_in: i64,
            id_token: Option<String>,
        }

        let tokens: TokenResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse token response: {}", e))?;
        let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

        let oauth_tokens = crate::auth::oauth::OAuthTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at,
            id_token: tokens.id_token,
        };

        crate::auth::oauth::save_openai_tokens(&oauth_tokens)
            .map_err(|e| format!("Failed to save tokens: {}", e))?;

        Ok("Successfully logged in to OpenAI!".to_string())
    }

    fn start_openrouter_login(&mut self) {
        self.push_display_message(DisplayMessage::system(
            "**OpenRouter API Key**\n\n\
             Get your API key from: https://openrouter.ai/keys\n\n\
             **Paste your API key below** (it will be saved securely)."
                .to_string(),
        ));
        self.set_status_notice("Login: paste key…");
        self.pending_login = Some(PendingLogin::OpenRouter);
    }

    fn start_cursor_login(&mut self) {
        let binary =
            std::env::var("JCODE_CURSOR_CLI_PATH").unwrap_or_else(|_| "cursor-agent".to_string());

        self.push_display_message(DisplayMessage::system(format!(
            "Starting Cursor login...\n\nRunning `{} login`",
            binary
        )));
        self.set_status_notice("Login: cursor...");

        match Self::run_external_login_command(&binary, &["login"]) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(
                    "✓ **Cursor login command completed.**".to_string(),
                ));
                self.set_status_notice("Login: ✓ cursor");
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Cursor login failed: {}\n\nInstall Cursor Agent:\n\
                     - macOS/Linux/WSL: `curl https://cursor.com/install -fsS | bash`\n\
                     - Windows (PowerShell): `irm 'https://cursor.com/install?win32=true' | iex`\n\n\
                     Then log in with one of:\n\
                     - `{} login`\n\
                     - `agent login`",
                    e, binary
                )));
                self.set_status_notice("Login: cursor failed");
            }
        }
    }

    fn start_copilot_login(&mut self) {
        self.set_status_notice("Login: copilot device flow...");
        self.pending_login = Some(PendingLogin::Copilot);

        tokio::spawn(async move {
            let client = reqwest::Client::new();

            let device_resp = match crate::auth::copilot::initiate_device_flow(&client).await {
                Ok(resp) => resp,
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Copilot device flow failed: {}", e),
                    }));
                    return;
                }
            };

            let user_code = device_resp.user_code.clone();
            let verification_uri = device_resp.verification_uri.clone();

            let clipboard_ok = copy_to_clipboard(&user_code);
            let clipboard_msg = if clipboard_ok {
                " (copied to clipboard - just paste it!)"
            } else {
                ""
            };

            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                provider: "copilot_code".to_string(),
                success: true,
                message: format!(
                    "**GitHub Copilot Login**\n\n\
                     Your code: **{}**{}\n\n\
                     Opening browser to {} ...\n\
                     Paste the code there and authorize.\n\n\
                     Waiting for authorization... (type `/cancel` to abort)",
                    user_code, clipboard_msg, verification_uri
                ),
            }));

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = open::that_detached(&verification_uri);

            let token = match crate::auth::copilot::poll_for_access_token(
                &client,
                &device_resp.device_code,
                device_resp.interval,
            )
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Copilot login failed: {}", e),
                    }));
                    return;
                }
            };

            let username = crate::auth::copilot::fetch_github_username(&client, &token)
                .await
                .unwrap_or_else(|_| "unknown".to_string());

            match crate::auth::copilot::save_github_token(&token, &username) {
                Ok(()) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: true,
                        message: format!(
                            "✓ Authenticated as **{}** via GitHub Copilot.\n\n\
                             Copilot models are now available in `/model`.",
                            username
                        ),
                    }));
                }
                Err(e) => {
                    Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                        provider: "copilot".to_string(),
                        success: false,
                        message: format!("Failed to save Copilot token: {}", e),
                    }));
                }
            }
        });

        self.push_display_message(DisplayMessage::system(
            "**GitHub Copilot Login**\n\n\
             Starting device flow... please wait."
                .to_string(),
        ));
    }

    fn start_antigravity_login(&mut self) {
        let binary = std::env::var("JCODE_ANTIGRAVITY_CLI_PATH")
            .unwrap_or_else(|_| "antigravity".to_string());

        self.push_display_message(DisplayMessage::system(format!(
            "Starting Antigravity login...\n\nRunning `{} login`",
            binary
        )));
        self.set_status_notice("Login: antigravity...");

        match Self::run_external_login_command(&binary, &["login"]) {
            Ok(()) => {
                self.push_display_message(DisplayMessage::system(
                    "✓ **Antigravity login command completed.**".to_string(),
                ));
                self.set_status_notice("Login: ✓ antigravity");
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Antigravity login failed: {}\n\nCheck `{}` is installed and run `{} login`.",
                    e, binary, binary
                )));
                self.set_status_notice("Login: antigravity failed");
            }
        }
    }

    fn run_external_login_command(program: &str, args: &[&str]) -> anyhow::Result<()> {
        let raw_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }

        let status_result = std::process::Command::new(program).args(args).status();

        if raw_was_enabled {
            let _ = crossterm::terminal::enable_raw_mode();
        }

        let status = status_result.map_err(|e| {
            anyhow::anyhow!(
                "Failed to start command: {} {} ({})",
                program,
                args.join(" "),
                e
            )
        })?;
        if !status.success() {
            anyhow::bail!(
                "Command exited with non-zero status: {} {} ({})",
                program,
                args.join(" "),
                status
            );
        }
        Ok(())
    }

    fn run_external_login_command_owned(program: &str, args: &[String]) -> anyhow::Result<()> {
        let raw_was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if raw_was_enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }

        let status_result = std::process::Command::new(program).args(args).status();

        if raw_was_enabled {
            let _ = crossterm::terminal::enable_raw_mode();
        }

        let status = status_result.map_err(|e| {
            anyhow::anyhow!(
                "Failed to start command: {} {} ({})",
                program,
                args.join(" "),
                e
            )
        })?;
        if !status.success() {
            anyhow::bail!(
                "Command exited with non-zero status: {} {} ({})",
                program,
                args.join(" "),
                status
            );
        }
        Ok(())
    }

    fn handle_login_input(&mut self, pending: PendingLogin, input: String) {
        if input.is_empty() || input == "/cancel" {
            self.push_display_message(DisplayMessage::system("Login cancelled.".to_string()));
            return;
        }

        match pending {
            PendingLogin::Claude {
                verifier,
                redirect_uri,
            } => {
                self.set_status_notice("Login: exchanging...");
                let input_owned = input.clone();
                tokio::spawn(async move {
                    match Self::claude_token_exchange(
                        verifier,
                        input_owned,
                        "default",
                        redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            crate::logging::info(&format!("Claude login: {}", msg));
                        }
                        Err(e) => {
                            crate::logging::error(&format!("Claude login failed: {}", e));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(
                    "Exchanging authorization code for tokens...".to_string(),
                ));
            }
            PendingLogin::ClaudeAccount {
                verifier,
                label,
                redirect_uri,
            } => {
                self.set_status_notice(&format!("Login [{}]: exchanging...", label));
                let input_owned = input.clone();
                let label_clone = label.clone();
                tokio::spawn(async move {
                    match Self::claude_token_exchange(
                        verifier,
                        input_owned,
                        &label_clone,
                        redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            crate::logging::info(&format!(
                                "Claude login [{}]: {}",
                                label_clone, msg
                            ));
                        }
                        Err(e) => {
                            crate::logging::error(&format!(
                                "Claude login [{}] failed: {}",
                                label_clone, e
                            ));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(format!(
                    "Exchanging authorization code for account `{}`...",
                    label
                )));
            }
            PendingLogin::OpenRouter => {
                let key = input.trim().to_string();
                if !key.starts_with("sk-or-") {
                    self.push_display_message(DisplayMessage::system(
                        "⚠ OpenRouter keys typically start with `sk-or-`. Saving anyway..."
                            .to_string(),
                    ));
                }

                match Self::save_openrouter_key(&key) {
                    Ok(()) => {
                        self.push_display_message(DisplayMessage::system(
                            "✓ **OpenRouter API key saved!**\n\n\
                             Stored at `~/.config/jcode/openrouter.env`.\n\
                             You can now use `/model` to switch to OpenRouter models."
                                .to_string(),
                        ));
                        self.set_status_notice("Login: ✓ saved");
                    }
                    Err(e) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save OpenRouter key: {}",
                            e
                        )));
                    }
                }
            }
            PendingLogin::Copilot => {
                self.push_display_message(DisplayMessage::system(
                    "Copilot login is waiting for browser authorization.\n\
                     Complete the login in your browser, or type `/cancel` to abort."
                        .to_string(),
                ));
                self.pending_login = Some(PendingLogin::Copilot);
            }
            PendingLogin::ProviderSelection => {
                let choice = input.trim().to_lowercase();
                match choice.as_str() {
                    "1" | "claude" | "anthropic" => self.start_claude_login(),
                    "2" | "openai" => self.start_openai_login(),
                    "3" | "openrouter" => self.start_openrouter_login(),
                    "4" | "cursor" => self.start_cursor_login(),
                    "5" | "copilot" => self.start_copilot_login(),
                    "6" | "antigravity" => self.start_antigravity_login(),
                    _ => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Unknown selection '{}'. Type 1-6 or a provider name.",
                            input.trim()
                        )));
                        self.pending_login = Some(PendingLogin::ProviderSelection);
                    }
                }
            }
        }
    }

    fn handle_login_completed(&mut self, login: LoginCompleted) {
        if login.provider == "copilot_code" {
            self.push_display_message(DisplayMessage::system(login.message.clone()));
            if let Some(code) = login
                .message
                .split("Enter code: **")
                .nth(1)
                .and_then(|s| s.split("**").next())
            {
                self.set_status_notice(&format!("Login: enter {} at GitHub", code));
            }
            return;
        }
        crate::auth::AuthStatus::invalidate_cache();
        if login.success {
            self.push_display_message(DisplayMessage::system(login.message));
            self.set_status_notice(&format!("Login: ✓ {}", login.provider));
            self.provider.on_auth_changed();
        } else {
            self.push_display_message(DisplayMessage::error(login.message));
            self.set_status_notice(&format!("Login: {} failed", login.provider));
        }
        if self.pending_login.is_some() {
            self.pending_login = None;
        }
    }

    async fn claude_token_exchange(
        verifier: String,
        input: String,
        label: &str,
        redirect_uri: Option<String>,
    ) -> Result<String, String> {
        let raw_code = if input.contains("code=") {
            let url = url::Url::parse(&input)
                .or_else(|_| url::Url::parse(&format!("https://example.com?{}", input)))
                .map_err(|e| format!("Failed to parse URL: {}", e))?;
            url.query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string())
                .ok_or_else(|| "No code found in URL".to_string())?
        } else {
            input.trim().to_string()
        };

        let (code, state_from_callback) = if raw_code.contains('#') {
            let parts: Vec<&str> = raw_code.splitn(2, '#').collect();
            (parts[0].to_string(), Some(parts[1].to_string()))
        } else {
            (raw_code, None)
        };

        let client = reqwest::Client::new();
        let mut body = serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": crate::auth::oauth::claude::CLIENT_ID,
            "code": code,
            "code_verifier": verifier,
            "redirect_uri": redirect_uri.as_deref().unwrap_or(crate::auth::oauth::claude::REDIRECT_URI),
        });

        if let Some(state) = state_from_callback {
            body["state"] = serde_json::Value::String(state);
        }

        let resp = client
            .post(crate::auth::oauth::claude::TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Token exchange request failed: {}", e))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Token exchange failed: {}", text));
        }

        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
            expires_in: i64,
            id_token: Option<String>,
        }

        let tokens: TokenResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse token response: {}", e))?;
        let expires_at = chrono::Utc::now().timestamp_millis() + (tokens.expires_in * 1000);

        let oauth_tokens = crate::auth::oauth::OAuthTokens {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at,
            id_token: tokens.id_token,
        };

        crate::auth::oauth::save_claude_tokens_for_account(&oauth_tokens, label)
            .map_err(|e| format!("Failed to save tokens: {}", e))?;

        Ok(format!(
            "Successfully logged in to Claude! (account: {})",
            label
        ))
    }

    fn save_openrouter_key(key: &str) -> anyhow::Result<()> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
            .join("jcode");
        std::fs::create_dir_all(&config_dir)?;

        let file_path = config_dir.join("openrouter.env");
        let content = format!("OPENROUTER_API_KEY={}\n", key);
        std::fs::write(&file_path, &content)?;

        crate::platform::set_permissions_owner_only(&file_path)?;

        std::env::set_var("OPENROUTER_API_KEY", key);
        Ok(())
    }

    fn model_picker_preview_filter(input: &str) -> Option<String> {
        let trimmed = input.trim_start();
        for cmd in ["/model", "/models"] {
            if let Some(rest) = trimmed.strip_prefix(cmd) {
                if rest.is_empty() {
                    return Some(String::new());
                }
                if rest
                    .chars()
                    .next()
                    .map(|c| c.is_whitespace())
                    .unwrap_or(false)
                {
                    return Some(rest.trim_start().to_string());
                }
            }
        }
        None
    }

    fn sync_model_picker_preview_from_input(&mut self) {
        let Some(filter) = Self::model_picker_preview_filter(&self.input) else {
            if self
                .picker_state
                .as_ref()
                .map(|picker| picker.preview)
                .unwrap_or(false)
            {
                self.picker_state = None;
            }
            return;
        };

        if self.picker_state.is_none() {
            let saved_input = self.input.clone();
            let saved_cursor = self.cursor_pos;
            self.open_model_picker();
            if let Some(ref mut picker) = self.picker_state {
                picker.preview = true;
            }
            // Preview must not steal the user's command input.
            self.input = saved_input;
            self.cursor_pos = saved_cursor;
        }

        if let Some(ref mut picker) = self.picker_state {
            if picker.preview {
                picker.filter = filter;
                Self::apply_picker_filter(picker);
            }
        }
    }

    fn activate_model_picker_from_preview(&mut self) -> bool {
        if !self
            .picker_state
            .as_ref()
            .map(|picker| picker.preview)
            .unwrap_or(false)
        {
            return false;
        }

        // Clear preview flag so handle_picker_key treats it as a full picker
        if let Some(ref mut picker) = self.picker_state {
            picker.preview = false;
        }
        self.input.clear();
        self.cursor_pos = 0;
        // Delegate to picker Enter handler which confirms the selection
        let _ = self.handle_picker_key(KeyCode::Enter, KeyModifiers::NONE);
        true
    }

    /// Open the model picker with available models
    fn open_model_picker(&mut self) {
        use std::collections::BTreeMap;

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };

        let cfg = crate::config::Config::load();
        let config_default_model = cfg.provider.default_model.clone();

        let is_config_default = |name: &str| -> bool {
            match &config_default_model {
                None => false,
                Some(default) => {
                    let bare = default.strip_prefix("copilot:").unwrap_or(default);
                    let bare = bare.split('@').next().unwrap_or(bare);
                    name == default || name == bare
                }
            }
        };

        // Gather routes from provider (local) or build from available info (remote)
        let routes: Vec<crate::provider::ModelRoute> = if self.is_remote {
            // Remote mode: build routes from available models + auth status
            let auth = crate::auth::AuthStatus::check();
            let mut routes = Vec::new();
            for model in &self.remote_available_models {
                if model.contains('/') {
                    // OpenRouter model
                    let cached =
                        crate::provider::openrouter::load_endpoints_disk_cache_public(model);
                    let auto_detail = cached
                        .as_ref()
                        .and_then(|(eps, _)| {
                            eps.first().map(|ep| format!("→ {}", ep.provider_name))
                        })
                        .unwrap_or_default();
                    routes.push(crate::provider::ModelRoute {
                        model: model.clone(),
                        provider: "auto".to_string(),
                        api_method: "openrouter".to_string(),
                        available: auth.openrouter != crate::auth::AuthState::NotConfigured,
                        detail: auto_detail,
                    });
                    if let Some((endpoints, age)) = cached {
                        let age_str = if age < 3600 {
                            format!("{}m ago", age / 60)
                        } else if age < 86400 {
                            format!("{}h ago", age / 3600)
                        } else {
                            format!("{}d ago", age / 86400)
                        };
                        for ep in &endpoints {
                            routes.push(crate::provider::ModelRoute {
                                model: model.clone(),
                                provider: ep.provider_name.clone(),
                                api_method: "openrouter".to_string(),
                                available: auth.openrouter != crate::auth::AuthState::NotConfigured,
                                detail: format!("{} ({})", ep.detail_string(), age_str),
                            });
                        }
                    }
                } else if crate::provider::ALL_CLAUDE_MODELS.contains(&model.as_str()) {
                    if auth.anthropic.has_oauth {
                        let is_1m = model.ends_with("[1m]");
                        let is_opus = model.contains("opus");
                        let is_max = crate::auth::claude::is_max_subscription();
                        let (available, detail) = if is_1m && !crate::usage::has_extra_usage() {
                            (false, "requires extra usage".to_string())
                        } else if is_opus && !is_max {
                            (false, "requires Max subscription".to_string())
                        } else {
                            (true, String::new())
                        };
                        routes.push(crate::provider::ModelRoute {
                            model: model.clone(),
                            provider: "Anthropic".to_string(),
                            api_method: "claude-oauth".to_string(),
                            available,
                            detail,
                        });
                    }
                } else if crate::provider::ALL_OPENAI_MODELS.contains(&model.as_str()) {
                    let availability = crate::provider::model_availability_for_account(model);
                    let (available, detail) =
                        if auth.openai == crate::auth::AuthState::NotConfigured {
                            (false, "no credentials".to_string())
                        } else {
                            match availability.state {
                                crate::provider::AccountModelAvailabilityState::Available => {
                                    (true, String::new())
                                }
                                crate::provider::AccountModelAvailabilityState::Unavailable => (
                                    false,
                                    crate::provider::format_account_model_availability_detail(
                                        &availability,
                                    )
                                    .unwrap_or_else(|| "not available".to_string()),
                                ),
                                crate::provider::AccountModelAvailabilityState::Unknown => {
                                    let detail =
                                        crate::provider::format_account_model_availability_detail(
                                            &availability,
                                        )
                                        .unwrap_or_else(|| "availability unknown".to_string());
                                    (true, detail)
                                }
                            }
                        };
                    routes.push(crate::provider::ModelRoute {
                        model: model.clone(),
                        provider: "OpenAI".to_string(),
                        api_method: "openai-oauth".to_string(),
                        available,
                        detail,
                    });
                } else {
                    // Copilot or other provider models
                    let (available, provider_name) =
                        if auth.copilot == crate::auth::AuthState::Available {
                            (true, "Copilot".to_string())
                        } else {
                            (false, "unknown".to_string())
                        };
                    routes.push(crate::provider::ModelRoute {
                        model: model.clone(),
                        provider: provider_name,
                        api_method: "copilot".to_string(),
                        available,
                        detail: String::new(),
                    });
                }
            }
            routes
        } else {
            self.provider.model_routes()
        };

        if routes.is_empty() {
            self.set_status_notice("No models available");
            return;
        }

        // Group routes by model, preserving order of first appearance
        let mut model_order: Vec<String> = Vec::new();
        let mut model_routes: BTreeMap<String, Vec<super::RouteOption>> = BTreeMap::new();
        for r in &routes {
            if !model_routes.contains_key(&r.model) {
                model_order.push(r.model.clone());
            }
            model_routes
                .entry(r.model.clone())
                .or_default()
                .push(super::RouteOption {
                    provider: r.provider.clone(),
                    api_method: r.api_method.clone(),
                    available: r.available,
                    detail: r.detail.clone(),
                });
        }

        // Sort routes within each model: available first, then oauth > api-key > openrouter
        fn route_sort_key(r: &super::RouteOption) -> (u8, u8, String) {
            let avail = if r.available { 0 } else { 1 };
            let method = match r.api_method.as_str() {
                "claude-oauth" | "openai-oauth" => 0,
                "copilot" => 1,
                "api-key" => 2,
                "openrouter" => 3,
                _ => 4,
            };
            (avail, method, r.provider.clone())
        }

        const RECOMMENDED_MODELS: &[&str] = &[
            "gpt-5.3-codex-spark",
            "gpt-5.3-codex",
            "claude-opus-4-6",
            "claude-opus-4-6[1m]",
            "claude-opus-4.6",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6[1m]",
            "moonshotai/kimi-k2.5",
        ];

        const CLAUDE_OAUTH_ONLY_MODELS: &[&str] = &[
            "claude-opus-4-6",
            "claude-opus-4-6[1m]",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6[1m]",
        ];

        const OPENAI_OAUTH_ONLY_MODELS: &[&str] = &["gpt-5.3-codex-spark", "gpt-5.3-codex"];

        const COPILOT_OAUTH_MODELS: &[&str] = &["claude-opus-4.6", "gpt-5.3-codex"];

        // Find the latest recommended model's created timestamp from OpenRouter cache,
        // then mark anything more than 1 month older as "old".
        let latest_recommended_ts: Option<u64> = RECOMMENDED_MODELS
            .iter()
            .filter_map(|m| crate::provider::openrouter::model_created_timestamp(m))
            .max();
        let old_threshold_secs = latest_recommended_ts
            .map(|ts| ts.saturating_sub(30 * 86400)) // 1 month before latest recommended
            .unwrap_or(0);

        // Format a unix timestamp as "Mon YYYY"
        fn format_created(ts: u64) -> String {
            use chrono::{TimeZone, Utc};
            if let Some(dt) = Utc.timestamp_opt(ts as i64, 0).single() {
                dt.format("%b %Y").to_string()
            } else {
                String::new()
            }
        }

        let current_effort = self.provider.reasoning_effort();
        let available_efforts = self.provider.available_efforts();
        let is_openai = !available_efforts.is_empty();

        let mut models: Vec<super::ModelEntry> = Vec::new();
        for name in &model_order {
            let mut entry_routes = model_routes.remove(name).unwrap_or_default();
            entry_routes.sort_by_key(|r| route_sort_key(r));

            let is_openai_model = crate::provider::ALL_OPENAI_MODELS.contains(&name.as_str());

            if is_openai_model && is_openai && !available_efforts.is_empty() {
                for effort in &available_efforts {
                    let effort_label = match *effort {
                        "xhigh" => "max",
                        "high" => "high",
                        "medium" => "med",
                        "low" => "low",
                        "none" => "none",
                        other => other,
                    };
                    let display_name = format!("{} ({})", name, effort_label);
                    let is_this_current =
                        *name == current_model && current_effort.as_deref() == Some(*effort);
                    let or_created = crate::provider::openrouter::model_created_timestamp(name);
                    models.push(super::ModelEntry {
                        name: display_name,
                        routes: entry_routes.clone(),
                        selected_route: 0,
                        is_current: is_this_current,
                        recommended: RECOMMENDED_MODELS.contains(&name.as_str())
                            && (*effort == "xhigh" || *effort == "high")
                            && (!(CLAUDE_OAUTH_ONLY_MODELS.contains(&name.as_str())
                                || OPENAI_OAUTH_ONLY_MODELS.contains(&name.as_str())
                                || COPILOT_OAUTH_MODELS.contains(&name.as_str()))
                                || entry_routes.iter().any(|r| {
                                    (r.api_method == "claude-oauth"
                                        || r.api_method == "openai-oauth"
                                        || r.api_method == "copilot")
                                        && r.available
                                })),
                        old: old_threshold_secs > 0
                            && or_created.map(|t| t < old_threshold_secs).unwrap_or(false),
                        created_date: or_created.map(|t| format_created(t)),
                        effort: Some(effort.to_string()),
                        is_default: is_config_default(name),
                    });
                }
            } else {
                let or_created = crate::provider::openrouter::model_created_timestamp(name);
                let is_old = old_threshold_secs > 0
                    && or_created.map(|t| t < old_threshold_secs).unwrap_or(false);
                let is_recommended = RECOMMENDED_MODELS.contains(&name.as_str())
                    && (!(CLAUDE_OAUTH_ONLY_MODELS.contains(&name.as_str())
                        || OPENAI_OAUTH_ONLY_MODELS.contains(&name.as_str())
                        || COPILOT_OAUTH_MODELS.contains(&name.as_str()))
                        || entry_routes.iter().any(|r| {
                            (r.api_method == "claude-oauth"
                                || r.api_method == "openai-oauth"
                                || r.api_method == "copilot")
                                && r.available
                        }));
                models.push(super::ModelEntry {
                    name: name.clone(),
                    routes: entry_routes,
                    selected_route: 0,
                    is_current: *name == current_model,
                    recommended: is_recommended,
                    old: is_old,
                    created_date: or_created.map(|t| format_created(t)),
                    effort: None,
                    is_default: is_config_default(name),
                });
            }
        }

        // Sort models: current first, then recommended, then available, then alphabetical
        models.sort_by(|a, b| {
            let a_current = if a.is_current { 0u8 } else { 1 };
            let b_current = if b.is_current { 0u8 } else { 1 };
            let a_rec = if a.recommended { 0u8 } else { 1 };
            let b_rec = if b.recommended { 0u8 } else { 1 };
            let a_avail = if a.routes.first().map(|r| r.available).unwrap_or(false) {
                0u8
            } else {
                1
            };
            let b_avail = if b.routes.first().map(|r| r.available).unwrap_or(false) {
                0u8
            } else {
                1
            };
            let a_old = if a.old { 1u8 } else { 0 };
            let b_old = if b.old { 1u8 } else { 0 };
            a_current
                .cmp(&b_current)
                .then(a_rec.cmp(&b_rec))
                .then(a_avail.cmp(&b_avail))
                .then(a_old.cmp(&b_old))
                .then(a.name.cmp(&b.name))
        });

        let filtered: Vec<usize> = (0..models.len()).collect();
        let selected = 0; // Current model is sorted first

        self.picker_state = Some(super::PickerState {
            models,
            filtered,
            selected,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Handle arrow/navigation keys when picker is in preview mode.
    /// Returns Ok(true) if the key was handled, Ok(false) to fall through.
    fn handle_picker_preview_key(
        &mut self,
        code: &KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        let is_preview = self.picker_state.as_ref().map_or(false, |p| p.preview);
        if !is_preview {
            return Ok(false);
        }
        match code {
            KeyCode::Down => {
                let picker = self.picker_state.as_mut().unwrap();
                let max = picker.filtered.len().saturating_sub(1);
                picker.selected = (picker.selected + 1).min(max);
                Ok(true)
            }
            KeyCode::Up => {
                let picker = self.picker_state.as_mut().unwrap();
                picker.selected = picker.selected.saturating_sub(1);
                Ok(true)
            }
            KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
                let picker = self.picker_state.as_mut().unwrap();
                let max = picker.filtered.len().saturating_sub(1);
                picker.selected = (picker.selected + 1).min(max);
                Ok(true)
            }
            KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
                let picker = self.picker_state.as_mut().unwrap();
                picker.selected = picker.selected.saturating_sub(1);
                Ok(true)
            }
            KeyCode::PageDown => {
                let picker = self.picker_state.as_mut().unwrap();
                let max = picker.filtered.len().saturating_sub(1);
                picker.selected = (picker.selected + 5).min(max);
                Ok(true)
            }
            KeyCode::PageUp => {
                let picker = self.picker_state.as_mut().unwrap();
                picker.selected = picker.selected.saturating_sub(5);
                Ok(true)
            }
            KeyCode::Enter => {
                // Select the highlighted model directly from preview.
                // Promote to full picker at column=2 so the existing Enter
                // handler performs the confirmation logic.
                if let Some(ref mut picker) = self.picker_state {
                    if picker.filtered.is_empty() {
                        self.picker_state = None;
                        self.input.clear();
                        self.cursor_pos = 0;
                        return Ok(true);
                    }
                    picker.preview = false;
                    picker.column = 2;
                }
                self.input.clear();
                self.cursor_pos = 0;
                self.handle_picker_key(KeyCode::Enter, modifiers)?;
                Ok(true)
            }
            KeyCode::Esc => {
                self.picker_state = None;
                self.input.clear();
                self.cursor_pos = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Handle keyboard input when picker is active
    fn handle_picker_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) -> Result<()> {
        match code {
            KeyCode::Esc => {
                if let Some(ref picker) = self.picker_state {
                    if !picker.filter.is_empty() {
                        // First Esc clears filter
                        let picker = self.picker_state.as_mut().unwrap();
                        picker.filter.clear();
                        Self::apply_picker_filter(picker);
                        return Ok(());
                    }
                }
                self.picker_state = None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if matches!(code, KeyCode::Char('k')) && !_modifiers.contains(KeyModifiers::CONTROL)
                {
                    // Plain 'k' is a filter character, not navigation
                    if let Some(ref mut picker) = self.picker_state {
                        picker.filter.push('k');
                        Self::apply_picker_filter(picker);
                    }
                    return Ok(());
                }
                if let Some(ref mut picker) = self.picker_state {
                    if picker.column == 0 {
                        picker.selected = picker.selected.saturating_sub(1);
                    } else {
                        // Cycle routes for current model
                        if let Some(&idx) = picker.filtered.get(picker.selected) {
                            let entry = &mut picker.models[idx];
                            entry.selected_route = entry.selected_route.saturating_sub(1);
                        }
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if matches!(code, KeyCode::Char('j')) && !_modifiers.contains(KeyModifiers::CONTROL)
                {
                    // Plain 'j' is a filter character, not navigation
                    if let Some(ref mut picker) = self.picker_state {
                        picker.filter.push('j');
                        Self::apply_picker_filter(picker);
                    }
                    return Ok(());
                }
                if let Some(ref mut picker) = self.picker_state {
                    if picker.column == 0 {
                        let max = picker.filtered.len().saturating_sub(1);
                        picker.selected = (picker.selected + 1).min(max);
                    } else {
                        if let Some(&idx) = picker.filtered.get(picker.selected) {
                            let entry = &mut picker.models[idx];
                            let max = entry.routes.len().saturating_sub(1);
                            entry.selected_route = (entry.selected_route + 1).min(max);
                        }
                    }
                }
            }
            KeyCode::Right => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.column < 2 {
                        // Only allow moving to provider/via columns if model has multiple routes
                        if let Some(&idx) = picker.filtered.get(picker.selected) {
                            if picker.models[idx].routes.len() > 1 || picker.column > 0 {
                                picker.column += 1;
                            }
                        }
                    }
                }
            }
            KeyCode::Left | KeyCode::BackTab => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.column > 0 {
                        picker.column -= 1;
                    }
                }
            }
            KeyCode::Tab => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.column == 0 && !picker.filter.is_empty() {
                        // Tab-complete: fill to longest common prefix of matches
                        Self::tab_complete_filter(picker);
                    } else if picker.column < 2 {
                        // Move to next column if model has routes
                        if let Some(&idx) = picker.filtered.get(picker.selected) {
                            if picker.models[idx].routes.len() > 1 || picker.column > 0 {
                                picker.column += 1;
                            }
                        }
                    }
                }
            }
            KeyCode::Char('d') if _modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(ref picker) = self.picker_state {
                    if picker.filtered.is_empty() {
                        return Ok(());
                    }
                    let idx = picker.filtered[picker.selected];
                    let entry = &picker.models[idx];
                    let route = entry.routes.get(entry.selected_route);

                    let bare_name = if entry.effort.is_some() {
                        entry
                            .name
                            .rsplit_once(" (")
                            .map(|(base, _)| base.to_string())
                            .unwrap_or_else(|| entry.name.clone())
                    } else {
                        entry.name.clone()
                    };

                    let (model_spec, provider_key) = if let Some(r) = route {
                        let spec = if r.api_method == "copilot" {
                            format!("copilot:{}", bare_name)
                        } else if r.api_method == "openrouter" && r.provider != "auto" {
                            if bare_name.contains('/') {
                                format!("{}@{}", bare_name, r.provider)
                            } else {
                                format!("anthropic/{}@{}", bare_name, r.provider)
                            }
                        } else if r.api_method == "openrouter" {
                            bare_name.clone()
                        } else {
                            bare_name.clone()
                        };
                        let pkey = match r.api_method.as_str() {
                            "claude-oauth" | "api-key"
                                if crate::provider::ALL_CLAUDE_MODELS
                                    .contains(&bare_name.as_str()) =>
                            {
                                Some("claude")
                            }
                            "openai-oauth" => Some("openai"),
                            "copilot" => Some("copilot"),
                            "openrouter" => Some("openrouter"),
                            _ => None,
                        };
                        (spec, pkey)
                    } else {
                        (bare_name.clone(), None)
                    };

                    let notice = format!(
                        "Default → {} via {}",
                        model_spec,
                        provider_key.unwrap_or("auto")
                    );

                    match crate::config::Config::set_default_model(Some(&model_spec), provider_key)
                    {
                        Ok(()) => {
                            self.set_status_notice(notice);
                        }
                        Err(e) => {
                            self.set_status_notice(format!("Failed to save default: {}", e));
                        }
                    }
                    self.picker_state = None;
                }
            }
            KeyCode::Enter => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.filtered.is_empty() {
                        return Ok(());
                    }
                    let idx = picker.filtered[picker.selected];
                    let entry = &picker.models[idx];

                    if picker.column == 0 && entry.routes.len() > 1 {
                        // Advance to provider column (don't confirm yet)
                        picker.column = 1;
                        return Ok(());
                    }
                    if picker.column == 1 {
                        // Advance to via column
                        picker.column = 2;
                        return Ok(());
                    }

                    // Column 2 or single-route model: confirm selection
                    let route = &entry.routes[entry.selected_route];

                    if !route.available {
                        let name = entry.name.clone();
                        let detail = if route.detail.is_empty() {
                            "not available".to_string()
                        } else {
                            route.detail.clone()
                        };
                        self.picker_state = None;
                        self.set_status_notice(format!("{} — {}", name, detail));
                        return Ok(());
                    }

                    let bare_name = if entry.effort.is_some() {
                        entry
                            .name
                            .rsplit_once(" (")
                            .map(|(base, _)| base.to_string())
                            .unwrap_or_else(|| entry.name.clone())
                    } else {
                        entry.name.clone()
                    };

                    let spec = if route.api_method == "openrouter" && route.provider != "auto" {
                        if entry.name.contains('/') {
                            format!("{}@{}", entry.name, route.provider)
                        } else {
                            format!("anthropic/{}@{}", entry.name, route.provider)
                        }
                    } else if route.api_method == "openrouter" {
                        entry.name.clone()
                    } else if route.provider == "Copilot" {
                        format!("copilot:{}", bare_name)
                    } else {
                        bare_name.clone()
                    };

                    let effort = entry.effort.clone();
                    let notice = format!(
                        "Model → {} via {} ({})",
                        entry.name, route.provider, route.api_method
                    );

                    self.picker_state = None;
                    self.upstream_provider = None;
                    self.connection_type = None;
                    if self.is_remote {
                        self.pending_model_switch = Some(spec);
                    } else {
                        let _ = self.provider.set_model(&spec);
                    }
                    if let Some(effort) = effort {
                        let _ = self.provider.set_reasoning_effort(&effort);
                    }
                    self.set_status_notice(notice);
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut picker) = self.picker_state {
                    if picker.filter.pop().is_some() {
                        Self::apply_picker_filter(picker);
                    }
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut picker) = self.picker_state {
                    if !c.is_whitespace() {
                        picker.filter.push(c);
                        Self::apply_picker_filter(picker);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Fuzzy match score for picker: returns Some(score) if pattern is a subsequence of text.
    /// Returns the character indices in `text` that matched the fuzzy pattern.
    /// Uses the same greedy algorithm as `picker_fuzzy_score`.
    fn picker_fuzzy_match_positions(pattern: &str, text: &str) -> Option<Vec<usize>> {
        let pat: Vec<char> = pattern
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let txt: Vec<char> = text.to_lowercase().chars().collect();
        if pat.is_empty() {
            return Some(Vec::new());
        }

        let mut pi = 0;
        let mut positions = Vec::new();

        for (ti, &tc) in txt.iter().enumerate() {
            if pi < pat.len() && tc == pat[pi] {
                positions.push(ti);
                pi += 1;
            }
        }

        if pi == pat.len() {
            Some(positions)
        } else {
            None
        }
    }

    /// Higher score = better match. Bonuses for consecutive chars, word boundaries.
    fn picker_fuzzy_score(pattern: &str, text: &str) -> Option<i32> {
        let pat: Vec<char> = pattern
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let txt: Vec<char> = text.to_lowercase().chars().collect();
        if pat.is_empty() {
            return Some(0);
        }

        let mut pi = 0;
        let mut score = 0i32;
        let mut last_match: Option<usize> = None;

        for (ti, &tc) in txt.iter().enumerate() {
            if pi < pat.len() && tc == pat[pi] {
                score += 1;
                // Consecutive match bonus
                if let Some(last) = last_match {
                    if last + 1 == ti {
                        score += 3;
                    }
                }
                // Word boundary bonus (start, after / - _ space)
                if ti == 0
                    || matches!(
                        txt.get(ti.wrapping_sub(1)),
                        Some('/' | '-' | '_' | ' ' | '.')
                    )
                {
                    score += 5;
                }
                // Exact prefix bonus
                if pi == 0 && ti == 0 {
                    score += 10;
                }
                last_match = Some(ti);
                pi += 1;
            }
        }

        if pi == pat.len() {
            // Penalize long strings (prefer shorter, tighter matches)
            score -= (txt.len() as i32) / 10;
            Some(score)
        } else {
            None
        }
    }

    /// Re-filter picker models using fuzzy matching, sorted by score
    fn apply_picker_filter(picker: &mut super::PickerState) {
        if picker.filter.is_empty() {
            picker.filtered = (0..picker.models.len()).collect();
        } else {
            let mut scored: Vec<(usize, i32)> = picker
                .models
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    Self::picker_fuzzy_score(&picker.filter, &m.name).map(|s| {
                        let bonus = if m.recommended { 5 } else { 0 };
                        (i, s + bonus)
                    })
                })
                .collect();
            // Sort by score descending (best matches first)
            scored.sort_by(|a, b| b.1.cmp(&a.1));
            picker.filtered = scored.into_iter().map(|(i, _)| i).collect();
        }
        // Clamp selection
        if picker.filtered.is_empty() {
            picker.selected = 0;
        } else {
            picker.selected = picker.selected.min(picker.filtered.len() - 1);
        }
    }

    /// Tab-complete: fill filter to longest common prefix of matched model names
    fn tab_complete_filter(picker: &mut super::PickerState) {
        if picker.filtered.is_empty() {
            return;
        }
        // If only one match, fill the whole name
        if picker.filtered.len() == 1 {
            let name = picker.models[picker.filtered[0]].name.clone();
            picker.filter = name;
            Self::apply_picker_filter(picker);
            return;
        }
        // Find longest common prefix (case-insensitive) of all matches
        let names: Vec<&str> = picker
            .filtered
            .iter()
            .map(|&i| picker.models[i].name.as_str())
            .collect();
        let first = names[0].to_lowercase();
        let first_chars: Vec<char> = first.chars().collect();
        let mut prefix_len = first_chars.len();
        for name in &names[1..] {
            let lower = name.to_lowercase();
            let chars: Vec<char> = lower.chars().collect();
            let mut common = 0;
            for (a, b) in first_chars.iter().zip(chars.iter()) {
                if a == b {
                    common += 1;
                } else {
                    break;
                }
            }
            prefix_len = prefix_len.min(common);
        }
        // Only extend the filter (don't shorten it)
        if prefix_len > picker.filter.len() {
            // Use the casing from the first match
            let first_original = &picker.models[picker.filtered[0]].name;
            picker.filter = first_original[..prefix_len].to_string();
            Self::apply_picker_filter(picker);
        }
    }

    fn extract_thought_line(text: &str) -> Option<String> {
        let trimmed = text.trim();
        if trimmed.starts_with("Thought for ") && trimmed.ends_with('s') {
            Some(trimmed.to_string())
        } else {
            None
        }
    }

    /// Handle quit request (Ctrl+C/Ctrl+D). Returns true if should actually quit.
    fn handle_quit_request(&mut self) -> bool {
        const QUIT_TIMEOUT: Duration = Duration::from_secs(2);

        if let Some(pending_time) = self.quit_pending {
            if pending_time.elapsed() < QUIT_TIMEOUT {
                // Second press within timeout - actually quit
                // Mark session as closed and save
                self.session.provider_session_id = self.provider_session_id.clone();
                self.session.mark_closed();
                let _ = self.session.save();
                self.should_quit = true;
                return true;
            }
        }

        // First press or timeout expired - show warning
        self.quit_pending = Some(Instant::now());
        self.set_status_notice("Press Ctrl+C again to quit");
        false
    }

    fn missing_tool_result_ids(&self) -> Vec<String> {
        let mut tool_calls = HashSet::new();
        let mut tool_results = HashSet::new();

        for msg in &self.messages {
            match msg.role {
                Role::Assistant => {
                    for block in &msg.content {
                        if let ContentBlock::ToolUse { id, .. } = block {
                            tool_calls.insert(id.clone());
                        }
                    }
                }
                Role::User => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                            tool_results.insert(tool_use_id.clone());
                        }
                    }
                }
            }
        }

        tool_calls
            .difference(&tool_results)
            .cloned()
            .collect::<Vec<_>>()
    }

    fn summarize_tool_results_missing(&self) -> Option<String> {
        let missing = self.missing_tool_result_ids();
        if missing.is_empty() {
            return None;
        }
        let sample = missing
            .iter()
            .take(3)
            .map(|id| format!("`{}`", id))
            .collect::<Vec<_>>()
            .join(", ");
        let count = missing.len();
        let suffix = if count > 3 { "..." } else { "" };
        Some(format!(
            "Missing tool outputs for {} call(s): {}{}",
            count, sample, suffix
        ))
    }

    fn repair_missing_tool_outputs(&mut self) -> usize {
        let mut known_results = HashSet::new();
        for msg in &self.messages {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        known_results.insert(tool_use_id.clone());
                    }
                }
            }
        }

        let mut repaired = 0usize;
        let mut index = 0usize;
        while index < self.messages.len() {
            let mut missing_for_message: Vec<String> = Vec::new();
            if let Role::Assistant = self.messages[index].role {
                for block in &self.messages[index].content {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        if !known_results.contains(id) {
                            known_results.insert(id.clone());
                            missing_for_message.push(id.clone());
                        }
                    }
                }
            }

            if !missing_for_message.is_empty() {
                for (offset, id) in missing_for_message.iter().enumerate() {
                    let tool_block = ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: TOOL_OUTPUT_MISSING_TEXT.to_string(),
                        is_error: Some(true),
                    };
                    let inserted_message = Message {
                        role: Role::User,
                        content: vec![tool_block.clone()],
                        timestamp: None,
                    };
                    let stored_message = crate::session::StoredMessage {
                        id: id::new_id("message"),
                        role: Role::User,
                        content: vec![tool_block],
                        timestamp: Some(chrono::Utc::now()),
                        tool_duration_ms: None,
                        token_usage: None,
                    };
                    self.messages.insert(index + 1 + offset, inserted_message);
                    self.session
                        .messages
                        .insert(index + 1 + offset, stored_message);
                    self.tool_result_ids.insert(id.clone());
                    repaired += 1;
                }
                index += missing_for_message.len();
            }

            index += 1;
        }

        if repaired > 0 {
            self.reseed_compaction_from_provider_messages();
            let _ = self.session.save();
        }

        repaired
    }

    /// Rebuild current session into a new one without tool calls
    fn recover_session_without_tools(&mut self) {
        let old_session = self.session.clone();
        let old_messages = old_session.messages.clone();

        let new_session_id = format!("session_recovery_{}", id::new_id("rec"));
        let mut new_session =
            Session::create_with_id(new_session_id, Some(old_session.id.clone()), None);
        new_session.title = old_session.title.clone();
        new_session.provider_session_id = old_session.provider_session_id.clone();
        new_session.model = old_session.model.clone();
        new_session.is_canary = old_session.is_canary;
        new_session.testing_build = old_session.testing_build.clone();
        new_session.is_debug = old_session.is_debug;
        new_session.working_dir = old_session.working_dir.clone();

        self.clear_provider_messages();
        self.clear_display_messages();
        self.queued_messages.clear();
        self.pasted_contents.clear();
        self.pending_images.clear();
        self.active_skill = None;
        self.provider_session_id = None;
        self.session = new_session;

        for msg in old_messages {
            let role = msg.role.clone();
            let kept_blocks: Vec<ContentBlock> = msg
                .content
                .into_iter()
                .filter(|block| matches!(block, ContentBlock::Text { .. }))
                .collect();
            if kept_blocks.is_empty() {
                continue;
            }
            self.add_provider_message(Message {
                role: role.clone(),
                content: kept_blocks.clone(),
                timestamp: None,
            });
            self.push_display_message(DisplayMessage {
                role: match role {
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                },
                content: kept_blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            let _ = self.session.add_message(role, kept_blocks);
        }
        let _ = self.session.save();

        self.push_display_message(DisplayMessage::system(format!(
            "Recovery complete. New session: {}. Tool calls stripped; context preserved.",
            self.session.id
        )));
        self.set_status_notice("Recovered session");
    }

    async fn run_turn(&mut self) -> Result<()> {
        loop {
            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                let message = format!(
                    "Recovered {} missing tool output(s) from an interrupted turn.",
                    repaired
                );
                self.push_display_message(DisplayMessage::system(message));
                self.set_status_notice("Recovered missing tool outputs");
            }
            if let Some(summary) = self.summarize_tool_results_missing() {
                let message = format!(
                    "Tool outputs are missing for this turn. {}\n\nPress Ctrl+R to recover into a new session with context copied.",
                    summary
                );
                self.push_display_message(DisplayMessage::error(message));
                self.set_status_notice("Recovery needed");
                return Ok(());
            }

            let (provider_messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                self.handle_compaction_event(event);
            }

            let tools = self.registry.definitions(None).await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&provider_messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt =
                self.build_system_prompt_split(memory_pending.as_ref().map(|p| p.prompt.as_str()));
            if let Some(pending) = &memory_pending {
                let age_ms = pending.computed_at.elapsed().as_millis() as u64;
                self.show_injected_memory_context(&pending.prompt, pending.count, age_ms);
            }

            self.status = ProcessingStatus::Sending;
            let stamped;
            let send_messages = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&provider_messages);
                &stamped
            } else {
                &provider_messages
            };
            let mut stream = self
                .provider
                .complete_split(
                    send_messages,
                    &tools,
                    &split_prompt.static_part,
                    &split_prompt.dynamic_part,
                    self.provider_session_id.as_deref(),
                )
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut first_event = true;
            let mut saw_message_end = false;
            let mut call_output_tokens_seen: u64 = 0;
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();

            while let Some(event) = stream.next().await {
                // Track activity for status display
                self.last_stream_activity = Some(Instant::now());

                if first_event {
                    self.status = ProcessingStatus::Streaming;
                    first_event = false;
                }
                match event? {
                    StreamEvent::TextDelta(text) => {
                        text_content.push_str(&text);
                        if self.streaming_tps_start.is_none() {
                            self.streaming_tps_start = Some(Instant::now());
                        }
                        if let Some(chunk) = self.stream_buffer.push(&text) {
                            self.streaming_text.push_str(&chunk);
                        }
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        if self.streaming_tps_start.is_none() {
                            self.streaming_tps_start = Some(Instant::now());
                        }
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                            intent: None,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
                        if let Some(start) = self.streaming_tps_start.take() {
                            self.streaming_tps_elapsed += start.elapsed();
                        }
                        if let Some(mut tool) = current_tool.take() {
                            tool.input = serde_json::from_str(&current_tool_input)
                                .unwrap_or(serde_json::Value::Null);

                            // Flush stream buffer before committing
                            if let Some(chunk) = self.stream_buffer.flush() {
                                self.streaming_text.push_str(&chunk);
                            }

                            // Commit any pending text as a partial assistant message
                            if !self.streaming_text.is_empty() {
                                self.push_display_message(DisplayMessage {
                                    role: "assistant".to_string(),
                                    content: self.streaming_text.clone(),
                                    tool_calls: vec![],
                                    duration_secs: None,
                                    title: None,
                                    tool_data: None,
                                });
                                self.clear_streaming_render_state();
                                self.stream_buffer.clear();
                            }

                            // Add tool call as its own display message
                            self.push_display_message(DisplayMessage {
                                role: "tool".to_string(),
                                content: tool.name.clone(),
                                tool_calls: vec![],
                                duration_secs: None,
                                title: None,
                                tool_data: Some(tool.clone()),
                            });

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        let mut usage_changed = false;
                        if let Some(input) = input_tokens {
                            self.streaming_input_tokens = input;
                            usage_changed = true;
                        }
                        if let Some(output) = output_tokens {
                            self.streaming_output_tokens = output;
                            self.accumulate_streaming_output_tokens(
                                output,
                                &mut call_output_tokens_seen,
                            );
                        }
                        if cache_read_input_tokens.is_some() {
                            self.streaming_cache_read_tokens = cache_read_input_tokens;
                            usage_changed = true;
                        }
                        if cache_creation_input_tokens.is_some() {
                            self.streaming_cache_creation_tokens = cache_creation_input_tokens;
                            usage_changed = true;
                        }
                        if usage_changed {
                            self.update_compaction_usage_from_stream();
                            if let Some(context_tokens) = self.current_stream_context_tokens() {
                                self.check_context_warning(context_tokens);
                            }
                        }
                    }
                    StreamEvent::ConnectionType { connection } => {
                        self.connection_type = Some(connection);
                    }
                    StreamEvent::ConnectionPhase { phase } => {
                        self.status = ProcessingStatus::Connecting(phase);
                    }
                    StreamEvent::MessageEnd { .. } => {
                        if let Some(start) = self.streaming_tps_start.take() {
                            self.streaming_tps_elapsed += start.elapsed();
                        }
                        saw_message_end = true;
                    }
                    StreamEvent::SessionId(sid) => {
                        self.provider_session_id = Some(sid);
                        if saw_message_end {
                            break;
                        }
                    }
                    StreamEvent::Error {
                        message,
                        retry_after_secs,
                    } => {
                        // Check if this is a rate limit error
                        // First try the explicit retry_after_secs, then fall back to parsing message
                        let reset_duration = retry_after_secs
                            .map(Duration::from_secs)
                            .or_else(|| parse_rate_limit_error(&message));

                        if let Some(reset_duration) = reset_duration {
                            let reset_time = Instant::now() + reset_duration;
                            self.rate_limit_reset = Some(reset_time);
                            // Don't return error - the queued message will retry
                            let queued_info = if !self.queued_messages.is_empty() {
                                format!(" ({} messages queued)", self.queued_messages.len())
                            } else {
                                String::new()
                            };
                            self.push_display_message(DisplayMessage::system(format!(
                                "⏳ Rate limit hit. Will auto-retry in {} seconds...{}",
                                reset_duration.as_secs(),
                                queued_info
                            )));
                            self.status = ProcessingStatus::Idle;
                            self.clear_streaming_render_state();
                            return Ok(());
                        }
                        return Err(anyhow::anyhow!("Stream error: {}", message));
                    }
                    StreamEvent::ThinkingStart => {
                        // Track start and update status for real-time indicator
                        let start = Instant::now();
                        self.thinking_start = Some(start);
                        self.thinking_buffer.clear();
                        self.thinking_prefix_emitted = false;
                        // Always show Thinking in status bar (even when thinking content is visible)
                        self.status = ProcessingStatus::Thinking(start);
                    }
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Buffer thinking content and emit with prefix only once
                        self.thinking_buffer.push_str(&thinking_text);
                        // Flush any pending text first
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        // Only show thinking content if enabled in config
                        if config().display.show_thinking {
                            // Only emit the prefix once at the start of thinking
                            if !self.thinking_prefix_emitted
                                && !self.thinking_buffer.trim().is_empty()
                            {
                                self.insert_thought_line(format!(
                                    "💭 {}",
                                    self.thinking_buffer.trim_start()
                                ));
                                self.thinking_prefix_emitted = true;
                                self.thinking_buffer.clear();
                            } else if self.thinking_prefix_emitted {
                                // After prefix is emitted, append subsequent chunks directly
                                self.streaming_text.push_str(&thinking_text);
                            }
                        }
                        if store_reasoning_content {
                            reasoning_content.push_str(&thinking_text);
                        }
                    }
                    StreamEvent::ThinkingEnd => {
                        // Don't display here - ThinkingDone has accurate timing
                        self.thinking_start = None;
                        self.thinking_buffer.clear();
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        // Flush any pending buffered text first
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        // Bridge provides accurate wall-clock timing
                        let thinking_msg = format!("*Thought for {:.1}s*", duration_secs);
                        self.insert_thought_line(thinking_msg);
                        self.thinking_prefix_emitted = false;
                        self.thinking_buffer.clear();
                    }
                    StreamEvent::Compaction {
                        trigger,
                        pre_tokens,
                    } => {
                        // Flush any pending buffered text first
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        let tokens_str = pre_tokens
                            .map(|t| format!(" ({} tokens)", t))
                            .unwrap_or_default();
                        let compact_msg =
                            format!("📦 Context compacted ({}){}\n\n", trigger, tokens_str);
                        self.streaming_text.push_str(&compact_msg);
                        // Reset warning so it can appear again
                        self.context_warning_shown = false;
                    }
                    StreamEvent::UpstreamProvider { provider } => {
                        // Store the upstream provider (e.g., Fireworks, Together)
                        self.upstream_provider = Some(provider);
                    }
                    StreamEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // SDK already executed this tool, store result for later
                        self.tool_result_ids.insert(tool_use_id.clone());
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                    StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input,
                    } => {
                        // Execute native tool and send result back to SDK bridge
                        let ctx = crate::tool::ToolContext {
                            session_id: self.session_id().to_string(),
                            message_id: self.session_id().to_string(),
                            tool_call_id: request_id.clone(),
                            working_dir: self.session.working_dir.as_deref().map(PathBuf::from),
                            stdin_request_tx: None,
                        };
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                        let native_result = match tool_result {
                            Ok(output) => crate::provider::NativeToolResult::success(
                                request_id,
                                output.output,
                            ),
                            Err(e) => {
                                crate::provider::NativeToolResult::error(request_id, e.to_string())
                            }
                        };
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                }
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
                    cache_control: None,
                });
            }
            if store_reasoning_content && !reasoning_content.is_empty() {
                content_blocks.push(ContentBlock::Reasoning {
                    text: reasoning_content.clone(),
                });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                let content_clone = content_blocks.clone();
                self.add_provider_message(Message {
                    role: Role::Assistant,
                    content: content_blocks,
                    timestamp: Some(chrono::Utc::now()),
                });
                let message_id = self.session.add_message(Role::Assistant, content_clone);
                let _ = self.session.save();
                for tc in &tool_calls {
                    self.tool_result_ids.insert(tc.id.clone());
                }
                Some(message_id)
            } else {
                None
            };

            // Add remaining text to display
            let duration = self.processing_started.map(|t| t.elapsed().as_secs_f32());

            // Flush any remaining buffered text
            if let Some(chunk) = self.stream_buffer.flush() {
                self.streaming_text.push_str(&chunk);
            }

            if tool_calls.is_empty() {
                // No tool calls - display full text_content
                if !text_content.is_empty() {
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: text_content.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
            } else {
                // Had tool calls - only display text that came AFTER the last tool
                // (text before each tool was already committed in ToolUseEnd handler)
                if !self.streaming_text.is_empty() {
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: self.streaming_text.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
            }
            self.clear_streaming_render_state();
            self.stream_buffer.clear();
            self.streaming_tool_calls.clear();

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tools - SDK may have executed some, but custom tools need local execution
            // Note: handles_tools_internally() means SDK handled KNOWN tools, but custom tools like
            // selfdev are not known to the SDK and need to be executed locally.
            for tc in tool_calls {
                self.status = ProcessingStatus::RunningTool(tc.name.clone());
                if matches!(tc.name.as_str(), "memory" | "remember") {
                    crate::memory::set_state(crate::tui::info_widget::MemoryState::Embedding);
                }
                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                // Check if SDK already executed this tool
                let (output, is_error, tool_title) =
                    if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                        // Use SDK result
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: if sdk_is_error {
                                ToolStatus::Error
                            } else {
                                ToolStatus::Completed
                            },
                            title: None,
                        }));
                        (sdk_content, sdk_is_error, None)
                    } else {
                        // Execute locally
                        let ctx = ToolContext {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            working_dir: self.session.working_dir.as_deref().map(PathBuf::from),
                            stdin_request_tx: None,
                        };

                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Running,
                            title: None,
                        }));

                        let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
                        match result {
                            Ok(o) => {
                                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                                    session_id: self.session.id.clone(),
                                    message_id: message_id.clone(),
                                    tool_call_id: tc.id.clone(),
                                    tool_name: tc.name.clone(),
                                    status: ToolStatus::Completed,
                                    title: o.title.clone(),
                                }));
                                (o.output, false, o.title)
                            }
                            Err(e) => {
                                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                                    session_id: self.session.id.clone(),
                                    message_id: message_id.clone(),
                                    tool_call_id: tc.id.clone(),
                                    tool_name: tc.name.clone(),
                                    status: ToolStatus::Error,
                                    title: None,
                                }));
                                (format!("Error: {}", e), true, None)
                            }
                        }
                    };

                // Update the tool's DisplayMessage with the output
                if let Some(dm) = self
                    .display_messages
                    .iter_mut()
                    .rev()
                    .find(|dm| dm.tool_data.as_ref().map(|td| &td.id) == Some(&tc.id))
                {
                    dm.content = output.clone();
                    dm.title = tool_title;
                }

                self.add_provider_message(Message::tool_result(&tc.id, &output, is_error));
                self.session.add_message(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: tc.id.clone(),
                        content: output.clone(),
                        is_error: if is_error { Some(true) } else { None },
                    }],
                );
                let _ = self.session.save();
            }
        }

        Ok(())
    }

    /// Run turn with interactive input handling (redraws UI, accepts input during streaming)
    async fn run_turn_interactive(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) -> Result<()> {
        let mut redraw_period = super::redraw_interval(self);
        let mut redraw_interval = interval(redraw_period);

        loop {
            let desired_redraw = super::redraw_interval(self);
            if desired_redraw != redraw_period {
                redraw_period = desired_redraw;
                redraw_interval = interval(redraw_period);
            }

            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                let message = format!(
                    "Recovered {} missing tool output(s) from an interrupted turn.",
                    repaired
                );
                self.push_display_message(DisplayMessage::system(message));
                self.set_status_notice("Recovered missing tool outputs");
            }
            if let Some(summary) = self.summarize_tool_results_missing() {
                let message = format!(
                    "Tool outputs are missing for this turn. {}\n\nPress Ctrl+R to recover into a new session with context copied.",
                    summary
                );
                self.push_display_message(DisplayMessage::error(message));
                self.set_status_notice("Recovery needed");
                return Ok(());
            }

            let (provider_messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                self.handle_compaction_event(event);
            }

            let tools = self.registry.definitions(None).await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&provider_messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt =
                self.build_system_prompt_split(memory_pending.as_ref().map(|p| p.prompt.as_str()));
            if let Some(pending) = &memory_pending {
                let age_ms = pending.computed_at.elapsed().as_millis() as u64;
                self.show_injected_memory_context(&pending.prompt, pending.count, age_ms);
            }

            self.status = ProcessingStatus::Sending;
            terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;

            crate::logging::info(&format!(
                "TUI: API call starting ({} messages)",
                provider_messages.len()
            ));
            let api_start = std::time::Instant::now();

            // Clone data needed for the API call to avoid borrow issues
            // The future would hold references across the select! which conflicts with handle_key
            let provider = self.provider.clone();
            let messages_clone = if crate::config::config().features.message_timestamps {
                Message::with_timestamps(&provider_messages)
            } else {
                provider_messages.clone()
            };
            let session_id_clone = self.provider_session_id.clone();
            let static_part = split_prompt.static_part.clone();
            let dynamic_part = split_prompt.dynamic_part.clone();

            // Make API call non-blocking - poll it in select! so we can handle input while waiting
            let mut api_future = std::pin::pin!(provider.complete_split(
                &messages_clone,
                &tools,
                &static_part,
                &dynamic_part,
                session_id_clone.as_deref()
            ));

            let mut stream = loop {
                tokio::select! {
                    biased;
                    // Handle keyboard input while waiting for API
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key))) => {
                                if key.kind == KeyEventKind::Press {
                                    let _ = self.handle_key(key.code, key.modifiers);
                                    if self.cancel_requested {
                                        self.cancel_requested = false;
                                        self.interleave_message = None;
                                        self.pending_soft_interrupts.clear();
                                        self.clear_streaming_render_state();
                                        self.stream_buffer.clear();
                                        self.streaming_tool_calls.clear();
                                        self.push_display_message(DisplayMessage::system("Interrupted"));
                                        return Ok(());
                                    }
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                            }
                            _ => {}
                        }
                    }
                    // Redraw periodically
                    _ = redraw_interval.tick() => {
                        terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
                    }
                    // Poll API call
                    result = &mut api_future => {
                        break result?;
                    }
                }
            };

            crate::logging::info(&format!(
                "TUI: API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut first_event = true;
            let mut saw_message_end = false;
            let mut call_output_tokens_seen: u64 = 0;
            let mut interleaved = false; // Track if we interleaved a message mid-stream
                                         // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();

            // Stream with input handling
            loop {
                tokio::select! {
                    // Redraw periodically
                    _ = redraw_interval.tick() => {
                        // Flush stream buffer on timeout
                        if self.stream_buffer.should_flush() {
                            if let Some(chunk) = self.stream_buffer.flush() {
                                self.streaming_text.push_str(&chunk);
                            }
                        }
                        terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
                    }
                    // Handle keyboard input
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key))) => {
                                if key.kind == KeyEventKind::Press {
                                    let _ = self.handle_key(key.code, key.modifiers);
                                    // Check for cancel request
                                    if self.cancel_requested {
                                        self.cancel_requested = false;
                                        self.interleave_message = None;
                                        self.pending_soft_interrupts.clear();
                                        // Save partial assistant response before clearing
                                        if let Some(tool) = current_tool.take() {
                                            tool_calls.push(tool);
                                        }
                                        if !text_content.is_empty() || !tool_calls.is_empty() {
                                            let mut content_blocks = Vec::new();
                                            if !text_content.is_empty() {
                                                content_blocks.push(ContentBlock::Text {
                                                    text: format!("{}\n\n[generation interrupted by user]", text_content),
                                                    cache_control: None,
                                                });
                                            }
                                            if store_reasoning_content && !reasoning_content.is_empty() {
                                                content_blocks.push(ContentBlock::Reasoning {
                                                    text: reasoning_content.clone(),
                                                });
                                            }
                                            for tc in &tool_calls {
                                                content_blocks.push(ContentBlock::ToolUse {
                                                    id: tc.id.clone(),
                                                    name: tc.name.clone(),
                                                    input: tc.input.clone(),
                                                });
                                            }
                                            if !content_blocks.is_empty() {
                                                let content_clone = content_blocks.clone();
                                                self.add_provider_message(Message {
                                                    role: Role::Assistant,
                                                    content: content_blocks,
                                                    timestamp: Some(chrono::Utc::now()),
                                                });
                                                self.session.add_message(Role::Assistant, content_clone);
                                                let _ = self.session.save();
                                            }
                                            // Flush buffer and show partial response
                                            if let Some(chunk) = self.stream_buffer.flush() {
                                                self.streaming_text.push_str(&chunk);
                                            }
                                            if !self.streaming_text.is_empty() {
                                                let content = self.take_streaming_text();
                                                self.push_display_message(DisplayMessage {
                                                    role: "assistant".to_string(),
                                                    content,
                                                    tool_calls: tool_calls.iter().map(|t| t.name.clone()).collect(),
                                                    duration_secs: self.processing_started.map(|t| t.elapsed().as_secs_f32()),
                                                    title: None,
                                                    tool_data: None,
                                                });
                                            }
                                        }
                                        self.clear_streaming_render_state();
                                        self.stream_buffer.clear();
                                        self.streaming_tool_calls.clear();
                                        self.push_display_message(DisplayMessage::system("Interrupted"));
                                        return Ok(());
                                    }
                                    // Check for interleave request (Shift+Enter)
                                    if let Some(interleave_msg) = self.interleave_message.take() {
                                        // Save partial assistant response if any
                                        if !text_content.is_empty() || !tool_calls.is_empty() {
                                            // Complete any pending tool
                                            if let Some(tool) = current_tool.take() {
                                                tool_calls.push(tool);
                                            }
                                            // Build content blocks for partial response
                                            let mut content_blocks = Vec::new();
                                            if !text_content.is_empty() {
                                                content_blocks.push(ContentBlock::Text {
                                                    text: text_content.clone(),
                                                    cache_control: None,
                                                });
                                            }
                                            if store_reasoning_content && !reasoning_content.is_empty() {
                                                content_blocks.push(ContentBlock::Reasoning {
                                                    text: reasoning_content.clone(),
                                                });
                                            }
                                            for tc in &tool_calls {
                                                content_blocks.push(ContentBlock::ToolUse {
                                                    id: tc.id.clone(),
                                                    name: tc.name.clone(),
                                                    input: tc.input.clone(),
                                                });
                                            }
                                            // Add partial assistant response to messages
                                            if !content_blocks.is_empty() {
                                                self.add_provider_message(Message {
                                                    role: Role::Assistant,
                                                    content: content_blocks,
                                                    timestamp: Some(chrono::Utc::now()),
                                                });
                                            }
                                            // Add display message for partial response
                                            if !self.streaming_text.is_empty() {
                                                let content = self.take_streaming_text();
                                                self.push_display_message(DisplayMessage {
                                                    role: "assistant".to_string(),
                                                    content,
                                                    tool_calls: tool_calls.iter().map(|t| t.name.clone()).collect(),
                                                    duration_secs: None,
                                                    title: None,
                                                    tool_data: None,
                                                });
                                            }
                                        }
                                        // Add user's interleaved message
                                        self.add_provider_message(Message::user(&interleave_msg));
                                        self.push_display_message(DisplayMessage {
                                            role: "user".to_string(),
                                            content: interleave_msg,
                                            tool_calls: vec![],
                                            duration_secs: None,
                                            title: None,
                                            tool_data: None,
                                        });
                                        // Clear streaming state and continue with new turn
                                        self.clear_streaming_render_state();
                                        self.streaming_tool_calls.clear();
                                        self.stream_buffer = StreamBuffer::new();
                                        reasoning_content.clear();
                                        interleaved = true;
                                        // Continue to next iteration of outer loop (new API call)
                                        break;
                                    }
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                            }
                            _ => {}
                        }
                    }
                    // Handle stream events
                    stream_event = stream.next() => {
                        match stream_event {
                            Some(Ok(event)) => {
                                // Track activity for status display
                                self.last_stream_activity = Some(Instant::now());

                                if first_event {
                                    self.status = ProcessingStatus::Streaming;
                                    first_event = false;
                                }
                                match event {
                                    StreamEvent::TextDelta(text) => {
                                        text_content.push_str(&text);
                                        if self.streaming_tps_start.is_none() {
                                            self.streaming_tps_start = Some(Instant::now());
                                        }
                                        if let Some(chunk) = self.stream_buffer.push(&text) {
                                            self.streaming_text.push_str(&chunk);
                                            self.broadcast_debug(super::backend::DebugEvent::TextDelta {
                                                text: chunk.clone()
                                            });
                                        }
                                    }
                                    StreamEvent::ToolUseStart { id, name } => {
                                        if self.streaming_tps_start.is_none() {
                                            self.streaming_tps_start = Some(Instant::now());
                                        }
                                        self.broadcast_debug(super::backend::DebugEvent::ToolStart {
                                            id: id.clone(),
                                            name: name.clone(),
                                        });
                                        // Update status to show tool in progress
                                        self.status = ProcessingStatus::RunningTool(name.clone());
                                        if matches!(name.as_str(), "memory" | "remember") {
                                            crate::memory::set_state(
                                                crate::tui::info_widget::MemoryState::Embedding,
                                            );
                                        }
                                        self.streaming_tool_calls.push(ToolCall {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: serde_json::Value::Null,
                                            intent: None,
                                        });
                                        current_tool = Some(ToolCall {
                                            id,
                                            name,
                                            input: serde_json::Value::Null,
                                            intent: None,
                                        });
                                        current_tool_input.clear();
                                    }
                                    StreamEvent::ToolInputDelta(delta) => {
                                        self.broadcast_debug(super::backend::DebugEvent::ToolInput {
                                            delta: delta.clone()
                                        });
                                        current_tool_input.push_str(&delta);
                                    }
                                    StreamEvent::ToolUseEnd => {
                                        if let Some(start) = self.streaming_tps_start.take() {
                                            self.streaming_tps_elapsed += start.elapsed();
                                        }
                                        if let Some(mut tool) = current_tool.take() {
                                            tool.input = serde_json::from_str(&current_tool_input)
                                                .unwrap_or(serde_json::Value::Null);
                                            self.broadcast_debug(super::backend::DebugEvent::ToolExec {
                                                id: tool.id.clone(),
                                                name: tool.name.clone(),
                                            });

                                            // Flush stream buffer before committing
                                            if let Some(chunk) = self.stream_buffer.flush() {
                                                self.streaming_text.push_str(&chunk);
                                            }

                                            // Commit any pending text as a partial assistant message
                                            if !self.streaming_text.is_empty() {
                                                self.push_display_message(DisplayMessage {
                                                    role: "assistant".to_string(),
                                                    content: self.streaming_text.clone(),
                                                    tool_calls: vec![],
                                                    duration_secs: None,
                                                    title: None,
                                                    tool_data: None,
                                                });
                                                self.clear_streaming_render_state();
                                                self.stream_buffer.clear();
                                            }

                                            // Add tool call as its own display message
                                            self.push_display_message(DisplayMessage {
                                                role: "tool".to_string(),
                                                content: tool.name.clone(),
                                                tool_calls: vec![],
                                                duration_secs: None,
                                                title: None,
                                                tool_data: Some(tool.clone()),
                                            });

                                            tool_calls.push(tool);
                                            current_tool_input.clear();
                                        }
                                    }
                                    StreamEvent::TokenUsage {
                                        input_tokens,
                                        output_tokens,
                                        cache_read_input_tokens,
                                        cache_creation_input_tokens,
                                    } => {
                                        let mut usage_changed = false;
                                        if let Some(input) = input_tokens {
                                            self.streaming_input_tokens = input;
                                            usage_changed = true;
                                        }
                                        if let Some(output) = output_tokens {
                                            self.streaming_output_tokens = output;
                                            self.accumulate_streaming_output_tokens(
                                                output,
                                                &mut call_output_tokens_seen,
                                            );
                                        }
                                        if cache_read_input_tokens.is_some() {
                                            self.streaming_cache_read_tokens = cache_read_input_tokens;
                                            usage_changed = true;
                                        }
                                        if cache_creation_input_tokens.is_some() {
                                            self.streaming_cache_creation_tokens =
                                                cache_creation_input_tokens;
                                            usage_changed = true;
                                        }
                                        if usage_changed {
                                            self.update_compaction_usage_from_stream();
                                            if let Some(context_tokens) = self.current_stream_context_tokens() {
                                                self.check_context_warning(context_tokens);
                                            }
                                        }
                                        self.broadcast_debug(super::backend::DebugEvent::TokenUsage {
                                            input_tokens: self.streaming_input_tokens,
                                            output_tokens: self.streaming_output_tokens,
                                            cache_read_input_tokens: self.streaming_cache_read_tokens,
                                            cache_creation_input_tokens: self
                                                .streaming_cache_creation_tokens,
                                        });
                                    }
                                    StreamEvent::ConnectionType { connection } => {
                                        self.connection_type = Some(connection);
                                    }
                                    StreamEvent::ConnectionPhase { phase } => {
                                        self.status = ProcessingStatus::Connecting(phase);
                                    }
                                    StreamEvent::MessageEnd { .. } => {
                                        if let Some(start) = self.streaming_tps_start.take() {
                                            self.streaming_tps_elapsed += start.elapsed();
                                        }
                                        saw_message_end = true;
                                    }
                                    StreamEvent::SessionId(sid) => {
                                        self.provider_session_id = Some(sid);
                                        if saw_message_end {
                                            break;
                                        }
                                    }
                                    StreamEvent::Error { message, .. } => {
                                        return Err(anyhow::anyhow!("Stream error: {}", message));
                                    }
                                    StreamEvent::ThinkingStart => {
                                        let start = Instant::now();
                                        self.thinking_start = Some(start);
                                        self.thinking_buffer.clear();
                                        self.thinking_prefix_emitted = false;
                                        // Always show Thinking in status bar
                                        self.status = ProcessingStatus::Thinking(start);
                                        self.broadcast_debug(super::backend::DebugEvent::ThinkingStart);
                                    }
                                    StreamEvent::ThinkingDelta(thinking_text) => {
                                        // Buffer thinking content and emit with prefix only once
                                        self.thinking_buffer.push_str(&thinking_text);
                                        // Display reasoning/thinking content from OpenAI
                                        if let Some(chunk) = self.stream_buffer.flush() {
                                            self.streaming_text.push_str(&chunk);
                                        }
                                        // Only show thinking content if enabled in config
                                        if config().display.show_thinking {
                                            // Only emit the prefix once at the start of thinking
                                            if !self.thinking_prefix_emitted && !self.thinking_buffer.trim().is_empty() {
                                                self.insert_thought_line(format!("💭 {}", self.thinking_buffer.trim_start()));
                                                self.thinking_prefix_emitted = true;
                                                self.thinking_buffer.clear();
                                            } else if self.thinking_prefix_emitted {
                                                // After prefix is emitted, append subsequent chunks directly
                                                self.streaming_text.push_str(&thinking_text);
                                            }
                                        }
                                        if store_reasoning_content {
                                            reasoning_content.push_str(&thinking_text);
                                        }
                                    }
                                    StreamEvent::ThinkingEnd => {
                                        self.thinking_start = None;
                                        self.thinking_buffer.clear();
                                        self.broadcast_debug(super::backend::DebugEvent::ThinkingEnd);
                                    }
                                    StreamEvent::ThinkingDone { duration_secs } => {
                                        // Flush any pending buffered text first
                                        if let Some(chunk) = self.stream_buffer.flush() {
                                            self.streaming_text.push_str(&chunk);
                                        }
                                        let thinking_msg = format!("*Thought for {:.1}s*", duration_secs);
                                        self.insert_thought_line(thinking_msg);
                                        self.thinking_prefix_emitted = false;
                                        self.thinking_buffer.clear();
                                    }
                                    StreamEvent::Compaction { trigger, pre_tokens } => {
                                        // Flush any pending buffered text first
                                        if let Some(chunk) = self.stream_buffer.flush() {
                                            self.streaming_text.push_str(&chunk);
                                        }
                                        let tokens_str = pre_tokens
                                            .map(|t| format!(" ({} tokens)", t))
                                            .unwrap_or_default();
                                        let compact_msg = format!(
                                            "📦 Context compacted ({}){}\n\n",
                                            trigger, tokens_str
                                        );
                                        self.streaming_text.push_str(&compact_msg);
                                        self.context_warning_shown = false;
                                    }
                                    StreamEvent::UpstreamProvider { provider } => {
                                        // Store the upstream provider (e.g., Fireworks, Together)
                                        self.upstream_provider = Some(provider);
                                    }
                                    StreamEvent::ToolResult { tool_use_id, content, is_error } => {
                                        // SDK already executed this tool
                                        self.tool_result_ids.insert(tool_use_id.clone());
                                        // Find the tool name from our tracking
                                        let tool_name = self.streaming_tool_calls
                                            .iter()
                                            .find(|tc| tc.id == tool_use_id)
                                            .map(|tc| tc.name.clone())
                                            .unwrap_or_default();

                                        self.broadcast_debug(super::backend::DebugEvent::ToolDone {
                                            id: tool_use_id.clone(),
                                            name: tool_name.clone(),
                                            output: content.clone(),
                                            is_error,
                                        });

                                        // Update the tool's DisplayMessage with the output (if it exists)
                                        if let Some(dm) = self.display_messages.iter_mut().rev().find(|dm| {
                                            dm.tool_data.as_ref().map(|td| &td.id) == Some(&tool_use_id)
                                        }) {
                                            dm.content = content.clone();
                                            self.bump_display_messages_version();
                                        }

                                        // Clear this tool from streaming_tool_calls
                                        self.streaming_tool_calls.retain(|tc| tc.id != tool_use_id);

                                        // Reset status back to Streaming
                                        self.status = ProcessingStatus::Streaming;

                                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                                    }
                                    StreamEvent::NativeToolCall {
                                        request_id,
                                        tool_name,
                                        input,
                                    } => {
                                        // Execute native tool and send result back to SDK bridge
                                        let ctx = crate::tool::ToolContext {
                                            session_id: self.session_id().to_string(),
                                            message_id: self.session_id().to_string(),
                                            tool_call_id: request_id.clone(),
                                            working_dir: self.session.working_dir.as_deref().map(PathBuf::from),
                            stdin_request_tx: None,
                                        };
                                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                                        let native_result = match tool_result {
                                            Ok(output) => crate::provider::NativeToolResult::success(request_id, output.output),
                                            Err(e) => crate::provider::NativeToolResult::error(request_id, e.to_string()),
                                        };
                                        if let Some(sender) = self.provider.native_result_sender() {
                                            let _ = sender.send(native_result).await;
                                        }
                                    }
                                }
                            }
                            Some(Err(e)) => return Err(e),
                            None => break, // Stream ended
                        }
                    }
                }
            }

            // If we interleaved a message, skip post-processing and go straight to new API call
            if interleaved {
                continue;
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
                    cache_control: None,
                });
            }
            if store_reasoning_content && !reasoning_content.is_empty() {
                content_blocks.push(ContentBlock::Reasoning {
                    text: reasoning_content.clone(),
                });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                let content_clone = content_blocks.clone();
                self.add_provider_message(Message {
                    role: Role::Assistant,
                    content: content_blocks,
                    timestamp: Some(chrono::Utc::now()),
                });
                let message_id = self.session.add_message(Role::Assistant, content_clone);
                let _ = self.session.save();
                for tc in &tool_calls {
                    self.tool_result_ids.insert(tc.id.clone());
                }
                Some(message_id)
            } else {
                None
            };

            // Add remaining text to display
            let duration = self.processing_started.map(|t| t.elapsed().as_secs_f32());

            // Flush any remaining buffered text
            if let Some(chunk) = self.stream_buffer.flush() {
                self.streaming_text.push_str(&chunk);
            }

            if tool_calls.is_empty() {
                // No tool calls - display full text_content
                if !text_content.is_empty() {
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: text_content.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
            } else {
                // Had tool calls - only display text that came AFTER the last tool
                // (text before each tool was already committed in ToolUseEnd handler)
                if !self.streaming_text.is_empty() {
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content: self.streaming_text.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                    self.push_turn_footer(duration);
                }
            }
            self.clear_streaming_render_state();
            self.stream_buffer.clear();
            self.streaming_tool_calls.clear();

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tools with input handling (non-blocking)
            // SDK may have executed some tools, but custom tools need local execution
            for tc in tool_calls {
                self.status = ProcessingStatus::RunningTool(tc.name.clone());
                if matches!(tc.name.as_str(), "memory" | "remember") {
                    crate::memory::set_state(crate::tui::info_widget::MemoryState::Embedding);
                }
                terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;

                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                // Check if SDK already executed this tool
                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // Use SDK result
                    Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                        session_id: self.session.id.clone(),
                        message_id: message_id.clone(),
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        status: if sdk_is_error {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Completed
                        },
                        title: None,
                    }));

                    // Update the tool's DisplayMessage with the output
                    if let Some(dm) = self
                        .display_messages
                        .iter_mut()
                        .rev()
                        .find(|dm| dm.tool_data.as_ref().map(|td| &td.id) == Some(&tc.id))
                    {
                        dm.content = sdk_content.clone();
                        dm.title = None;
                    }

                    self.add_provider_message(Message {
                        role: Role::User,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: sdk_content,
                            is_error: if sdk_is_error { Some(true) } else { None },
                        }],
                        timestamp: Some(chrono::Utc::now()),
                    });
                    self.session.add_message(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id,
                            content: String::new(), // Already added to messages above
                            is_error: if sdk_is_error { Some(true) } else { None },
                        }],
                    );
                    self.session.save()?;
                    continue;
                }

                // Execute locally
                let ctx = ToolContext {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    working_dir: self.session.working_dir.as_deref().map(PathBuf::from),
                    stdin_request_tx: None,
                };

                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    status: ToolStatus::Running,
                    title: None,
                }));

                // Make tool execution non-blocking - poll in select! so we can handle input
                // Clone registry to avoid borrow issues
                let registry = self.registry.clone();
                let tool_name = tc.name.clone();
                let tool_input = tc.input.clone();
                let mut tool_future = std::pin::pin!(registry.execute(&tool_name, tool_input, ctx));

                // Subscribe to bus for subagent status updates
                let mut bus_receiver = Bus::global().subscribe();
                self.subagent_status = None; // Clear previous status

                let result = loop {
                    tokio::select! {
                        biased;
                        // Handle keyboard input while tool executes
                        event = event_stream.next() => {
                            match event {
                                Some(Ok(Event::Key(key))) => {
                                    if key.kind == KeyEventKind::Press {
                                        let _ = self.handle_key(key.code, key.modifiers);
                                        if self.cancel_requested {
                                            self.cancel_requested = false;
                                            self.interleave_message = None;
                                            self.pending_soft_interrupts.clear();
                                            // Partial text+tool_calls were already saved
                                            // to the session before tool execution started.
                                            // Just preserve the visual streaming content.
                                            if let Some(chunk) = self.stream_buffer.flush() {
                                                self.streaming_text.push_str(&chunk);
                                            }
                                            if !self.streaming_text.is_empty() {
                                                let content = self.take_streaming_text();
                                                self.push_display_message(DisplayMessage {
                                                    role: "assistant".to_string(),
                                                    content,
                                                    tool_calls: Vec::new(),
                                                    duration_secs: self.processing_started.map(|t| t.elapsed().as_secs_f32()),
                                                    title: None,
                                                    tool_data: None,
                                                });
                                            }
                                            self.clear_streaming_render_state();
                                            self.stream_buffer.clear();
                                            self.streaming_tool_calls.clear();
                                            self.push_display_message(DisplayMessage::system("Interrupted"));
                                            return Ok(());
                                        }
                                    }
                                }
                                Some(Ok(Event::Paste(text))) => {
                                    self.handle_paste(text);
                                }
                                _ => {}
                            }
                        }
                        // Listen for subagent status updates
                        bus_event = bus_receiver.recv() => {
                            if let Ok(BusEvent::SubagentStatus(status)) = bus_event {
                                if status.session_id != self.session.id {
                                    continue;
                                }
                                let display = if let Some(model) = &status.model {
                                    format!("{} · {}", status.status, model)
                                } else {
                                    status.status
                                };
                                self.subagent_status = Some(display);
                            }
                        }
                        // Redraw periodically
                        _ = redraw_interval.tick() => {
                            terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
                        }
                        // Poll tool execution
                        result = &mut tool_future => {
                            break result;
                        }
                    }
                };

                self.subagent_status = None; // Clear status after tool completes
                let (output, is_error, tool_title) = match result {
                    Ok(o) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Completed,
                            title: o.title.clone(),
                        }));
                        (o.output, false, o.title)
                    }
                    Err(e) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Error,
                            title: None,
                        }));
                        (format!("Error: {}", e), true, None)
                    }
                };

                // Update the tool's DisplayMessage with the output
                if let Some(dm) = self
                    .display_messages
                    .iter_mut()
                    .rev()
                    .find(|dm| dm.tool_data.as_ref().map(|td| &td.id) == Some(&tc.id))
                {
                    dm.content = output.clone();
                    dm.title = tool_title;
                }

                self.add_provider_message(Message::tool_result(&tc.id, &output, is_error));
                self.session.add_message(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: tc.id.clone(),
                        content: output.clone(),
                        is_error: if is_error { Some(true) } else { None },
                    }],
                );
                let _ = self.session.save();
            }
        }

        Ok(())
    }

    fn build_system_prompt(&mut self, memory_prompt: Option<&str>) -> String {
        let split = self.build_system_prompt_split(memory_prompt);
        if split.dynamic_part.is_empty() {
            split.static_part
        } else if split.static_part.is_empty() {
            split.dynamic_part
        } else {
            format!("{}\n\n{}", split.static_part, split.dynamic_part)
        }
    }

    /// Build split system prompt for better caching
    fn build_system_prompt_split(
        &mut self,
        memory_prompt: Option<&str>,
    ) -> crate::prompt::SplitSystemPrompt {
        // Ambient mode: use the full override prompt directly
        if let Some(ref prompt) = self.ambient_system_prompt {
            return crate::prompt::SplitSystemPrompt {
                static_part: prompt.clone(),
                dynamic_part: String::new(),
            };
        }

        let skill_prompt = self
            .active_skill
            .as_ref()
            .and_then(|name| self.skills.get(name).map(|s| s.get_prompt().to_string()));
        let available_skills: Vec<crate::prompt::SkillInfo> = self
            .skills
            .list()
            .iter()
            .map(|s| crate::prompt::SkillInfo {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect();
        let (split, context_info) = crate::prompt::build_system_prompt_split(
            skill_prompt.as_deref(),
            &available_skills,
            self.session.is_canary,
            memory_prompt,
            None,
        );
        self.context_info = context_info;
        split
    }

    fn show_injected_memory_context(&mut self, prompt: &str, count: usize, age_ms: u64) {
        let count = count.max(1);
        let plural = if count == 1 { "memory" } else { "memories" };
        let display_prompt = if prompt.trim().is_empty() {
            "# Memory\n\n## Notes\n1. (empty injection payload)".to_string()
        } else {
            prompt.to_string()
        };
        if !self.should_inject_memory_context(&display_prompt) {
            return;
        }
        let prompt_chars = display_prompt.chars().count();
        crate::memory::record_injected_prompt(&display_prompt, count, age_ms);
        self.push_display_message(DisplayMessage::system(format!(
            "🧠 Injected {} {} into context ({} chars, computed {}ms ago)\n\n---\n\n{}",
            count, plural, prompt_chars, age_ms, display_prompt
        )));
        self.set_status_notice(format!("🧠 {} {} injected", count, plural));
    }

    /// Get memory prompt using async non-blocking approach
    /// Takes any pending memory from background check and sends context to memory agent for next turn
    fn build_memory_prompt_nonblocking(
        &self,
        messages: &[Message],
    ) -> Option<crate::memory::PendingMemory> {
        if self.is_remote || !self.memory_enabled {
            return None;
        }

        // Take pending memory if available (computed in background during last turn)
        let pending = crate::memory::take_pending_memory();

        // Send context to memory agent for the NEXT turn (doesn't block current send)
        crate::memory_agent::update_context_sync(messages.to_vec());

        // Return pending memory from previous turn
        pending
    }

    /// Legacy blocking memory prompt - kept for fallback but not used in normal flow
    #[allow(dead_code)]
    async fn build_memory_prompt(&self, messages: &[Message]) -> Option<String> {
        if self.is_remote {
            return None;
        }

        let manager = crate::memory::MemoryManager::new();
        match manager.relevant_prompt_for_messages(messages).await {
            Ok(prompt) => prompt,
            Err(e) => {
                crate::logging::info(&format!("Memory relevance skipped: {}", e));
                None
            }
        }
    }

    /// Extract and store memories from the session transcript at end of session
    async fn extract_session_memories(&self) {
        // Skip if remote mode or not enough messages
        if self.is_remote || !self.memory_enabled || self.messages.len() < 4 {
            return;
        }

        crate::logging::info(&format!(
            "Extracting memories from {} messages",
            self.messages.len()
        ));

        // Build transcript from messages
        let mut transcript = String::new();
        for msg in &self.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            };
            transcript.push_str(&format!("**{}:**\n", role));
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        transcript.push_str(text);
                        transcript.push('\n');
                    }
                    ContentBlock::ToolUse { name, .. } => {
                        transcript.push_str(&format!("[Used tool: {}]\n", name));
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        // Truncate long results
                        let preview = if content.len() > 200 {
                            format!("{}...", crate::util::truncate_str(content, 200))
                        } else {
                            content.clone()
                        };
                        transcript.push_str(&format!("[Result: {}]\n", preview));
                    }
                    ContentBlock::Reasoning { .. } => {}
                    ContentBlock::Image { .. } => {
                        transcript.push_str("[Image]\n");
                    }
                }
            }
            transcript.push('\n');
        }

        // Extract memories using sidecar
        let sidecar = crate::sidecar::Sidecar::new();
        match sidecar.extract_memories(&transcript).await {
            Ok(extracted) if !extracted.is_empty() => {
                let manager = crate::memory::MemoryManager::new();
                let mut stored_count = 0;

                for memory in extracted {
                    // Map category string to enum
                    let category = match memory.category.as_str() {
                        "fact" => crate::memory::MemoryCategory::Fact,
                        "preference" => crate::memory::MemoryCategory::Preference,
                        "correction" => crate::memory::MemoryCategory::Correction,
                        _ => crate::memory::MemoryCategory::Fact, // Default to fact
                    };

                    // Map trust string to enum
                    let trust = match memory.trust.as_str() {
                        "high" => crate::memory::TrustLevel::High,
                        "low" => crate::memory::TrustLevel::Low,
                        _ => crate::memory::TrustLevel::Medium,
                    };

                    // Create memory entry
                    let entry = crate::memory::MemoryEntry {
                        id: format!("auto_{}", chrono::Utc::now().timestamp_millis()),
                        category,
                        content: memory.content,
                        tags: Vec::new(),
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                        access_count: 0,
                        trust,
                        active: true,
                        superseded_by: None,
                        strength: 1,
                        source: Some(self.session.id.clone()),
                        reinforcements: Vec::new(),
                        embedding: None, // Will be generated when stored
                        confidence: 1.0,
                    };

                    // Store memory
                    if manager.remember_project(entry).is_ok() {
                        stored_count += 1;
                    }
                }

                if stored_count > 0 {
                    crate::logging::info(&format!(
                        "Extracted {} memories from session",
                        stored_count
                    ));
                }
            }
            Ok(_) => {
                // No memories extracted, that's fine
            }
            Err(e) => {
                crate::logging::info(&format!("Memory extraction skipped: {}", e));
            }
        }
    }

    // Getters for UI
    pub fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
    }

    fn bump_display_messages_version(&mut self) {
        self.display_messages_version = self.display_messages_version.wrapping_add(1);
    }

    fn push_display_message(&mut self, message: DisplayMessage) {
        let is_tool = message.role == "tool";
        self.display_messages.push(message);
        self.bump_display_messages_version();
        if is_tool && self.diff_mode.is_pinned() && self.diff_pane_auto_scroll {
            self.diff_pane_scroll = usize::MAX;
        }
    }

    fn append_reload_message(&mut self, line: &str) {
        if let Some(idx) = self
            .display_messages
            .iter()
            .rposition(Self::is_reload_message)
        {
            let msg = &mut self.display_messages[idx];
            if !msg.content.is_empty() {
                msg.content.push('\n');
            }
            msg.content.push_str(line);
            msg.title = Some("Reload".to_string());
            self.bump_display_messages_version();
        } else {
            self.push_display_message(
                DisplayMessage::system(line.to_string()).with_title("Reload"),
            );
        }
    }

    fn is_reload_message(message: &DisplayMessage) -> bool {
        message.role == "system"
            && message
                .title
                .as_deref()
                .is_some_and(|title| title == "Reload" || title.starts_with("Reload: "))
    }

    fn clear_display_messages(&mut self) {
        if !self.display_messages.is_empty() {
            self.display_messages.clear();
            self.bump_display_messages_version();
        }
    }

    /// Find word boundary going backward (for Ctrl+W, Alt+B)
    fn find_word_boundary_back(&self) -> usize {
        if self.cursor_pos == 0 {
            return 0;
        }
        let mut pos = self.cursor_pos;

        // Move back one char
        pos = super::core::prev_char_boundary(&self.input, pos);

        // Skip trailing whitespace
        while pos > 0 {
            let ch = self.input[pos..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos = super::core::prev_char_boundary(&self.input, pos);
        }

        // Skip word characters
        while pos > 0 {
            let prev = super::core::prev_char_boundary(&self.input, pos);
            let ch = self.input[prev..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            pos = prev;
        }

        pos
    }

    /// Find word boundary going forward (for Alt+F, Alt+D)
    fn find_word_boundary_forward(&self) -> usize {
        let len = self.input.len();
        if self.cursor_pos >= len {
            return len;
        }
        let mut pos = self.cursor_pos;

        // Skip current word
        while pos < len {
            let ch = self.input[pos..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            pos = super::core::next_char_boundary(&self.input, pos);
        }

        // Skip whitespace
        while pos < len {
            let ch = self.input[pos..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos = super::core::next_char_boundary(&self.input, pos);
        }

        pos
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    fn fuzzy_score(needle: &str, haystack: &str) -> Option<usize> {
        if needle.is_empty() {
            return Some(0);
        }
        // Both needle and haystack should start with '/', match from char 1 onward
        let n = needle.strip_prefix('/').unwrap_or(needle);
        let h = haystack.strip_prefix('/').unwrap_or(haystack);
        if n.is_empty() {
            return Some(0);
        }
        // First char of the command (after /) must match
        if !h.starts_with(&n[..n.chars().next().unwrap().len_utf8()]) {
            return None;
        }
        let mut score = 0usize;
        let mut pos = 0usize;
        for ch in n.chars() {
            let Some(idx) = h[pos..].find(ch) else {
                return None;
            };
            score += idx;
            pos += idx + ch.len_utf8();
        }
        // Penalize large gaps - reject if average gap is too big
        if n.len() > 1 && score > n.len() * 3 {
            return None;
        }
        Some(score)
    }

    fn rank_suggestions(
        &self,
        needle: &str,
        candidates: Vec<(String, &'static str)>,
    ) -> Vec<(String, &'static str)> {
        let needle = needle.to_lowercase();
        let mut scored: Vec<(bool, usize, String, &'static str)> = Vec::new();
        for (cmd, help) in candidates {
            let lower = cmd.to_lowercase();
            if lower.starts_with(&needle) {
                scored.push((true, 0, cmd, help));
            } else if let Some(score) = Self::fuzzy_score(&needle, &lower) {
                scored.push((false, score, cmd, help));
            }
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.len().cmp(&b.2.len()))
                .then_with(|| a.2.cmp(&b.2))
        });
        scored
            .into_iter()
            .map(|(_, _, cmd, help)| (cmd, help))
            .collect()
    }

    /// Get command suggestions based on current input (or base input for cycling)
    fn get_suggestions_for(&self, input: &str) -> Vec<(String, &'static str)> {
        let input = input.trim();

        // Only show suggestions when input starts with /
        if !input.starts_with('/') {
            return vec![];
        }

        let prefix = input.to_lowercase();

        // /model opens the interactive picker — don't list individual models in autocomplete
        if prefix == "/model" || prefix.starts_with("/model ") || prefix.starts_with("/models") {
            return vec![("/model".into(), "Open model picker")];
        }

        if prefix.starts_with("/effort ") {
            let efforts = ["none", "low", "medium", "high", "xhigh"];
            return efforts
                .iter()
                .map(|e| (format!("/effort {}", e), effort_display_label(e)))
                .collect();
        }

        if prefix.starts_with("/login ") || prefix.starts_with("/auth ") {
            return vec![
                ("/login anthropic".into(), "Login to Anthropic (OAuth)"),
                ("/login claude".into(), "Alias for Anthropic login"),
                ("/login openai".into(), "Login to OpenAI (OAuth)"),
                ("/login openrouter".into(), "Login to OpenRouter (API key)"),
                ("/login cursor".into(), "Login to Cursor CLI"),
                ("/login copilot".into(), "Login to GitHub Copilot CLI"),
                ("/login antigravity".into(), "Login to Antigravity CLI"),
            ];
        }

        if prefix.starts_with("/account ") || prefix.starts_with("/accounts ") {
            let mut suggestions = vec![
                ("/account list".into(), "List all Anthropic accounts"),
                (
                    "/account add".into(),
                    "Add a new account (start OAuth login)",
                ),
                ("/account switch".into(), "Switch active account"),
                ("/account remove".into(), "Remove an account"),
            ];
            if let Ok(accounts) = crate::auth::claude::list_accounts() {
                for account in accounts {
                    suggestions.push((
                        format!("/account switch {}", account.label),
                        "Switch to this account",
                    ));
                }
            }
            return suggestions;
        }

        // Built-in commands
        let mut commands: Vec<(String, &'static str)> = vec![
            ("/help".into(), "Show help and keyboard shortcuts"),
            ("/commands".into(), "Alias for /help"),
            ("/model".into(), "List or switch models"),
            ("/effort".into(), "Show/change reasoning effort (Alt+←/→)"),
            ("/clear".into(), "Clear conversation history"),
            ("/rewind".into(), "Rewind conversation to previous message"),
            (
                "/compact".into(),
                "Compact context (summarize old messages)",
            ),
            ("/fix".into(), "Recover when the model cannot continue"),
            (
                "/remember".into(),
                "Extract and save memories from conversation",
            ),
            ("/memory".into(), "Toggle memory feature (on/off/status)"),
            ("/swarm".into(), "Toggle swarm feature (on/off/status)"),
            ("/version".into(), "Show current version"),
            ("/changelog".into(), "Show recent changes in this build"),
            ("/info".into(), "Show session info and tokens"),
            ("/usage".into(), "Show subscription usage limits"),
            ("/reload".into(), "Smart reload (if newer binary exists)"),
            ("/rebuild".into(), "Full rebuild (git pull + build + tests)"),
            ("/update".into(), "Check for and install latest release"),
            ("/split".into(), "Split session into a new window"),
            ("/quit".into(), "Exit jcode"),
            ("/auth".into(), "Show authentication status"),
            (
                "/login".into(),
                "Login to a provider (anthropic/openai/openrouter/cursor/copilot/antigravity)",
            ),
            (
                "/account".into(),
                "Manage Anthropic accounts (list/add/switch/remove)",
            ),
        ];

        // Add client-reload and server-reload commands in remote mode
        if self.is_remote {
            commands.push(("/client-reload".into(), "Force reload client binary"));
            commands.push(("/server-reload".into(), "Force reload server binary"));
        }

        // Add skills as commands
        let skills = self.skills.list();
        for skill in skills {
            commands.push((format!("/{}", skill.name), "Activate skill"));
        }

        // Filter by prefix match
        self.rank_suggestions(&prefix, commands)
    }

    /// Get command suggestions based on current input
    pub fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        self.get_suggestions_for(&self.input)
    }

    /// Get suggestion prompts for new users on the initial empty screen.
    /// Returns (label, prompt_text) pairs. Empty once user is experienced or not authenticated.
    pub fn suggestion_prompts(&self) -> Vec<(String, String)> {
        let is_canary = if self.is_remote {
            self.remote_is_canary.unwrap_or(false)
        } else {
            self.session.is_canary
        };
        if is_canary {
            return Vec::new();
        }

        let auth = crate::auth::AuthStatus::check();
        if !auth.has_any_available() {
            return vec![
                (
                    "Log in to get started".to_string(),
                    "/login".to_string(),
                ),
            ];
        }

        if !self.display_messages.is_empty() || self.is_processing {
            return Vec::new();
        }

        let is_new_user = crate::storage::jcode_dir()
            .ok()
            .and_then(|dir| {
                let path = dir.join("setup_hints.json");
                std::fs::read_to_string(&path).ok()
            })
            .and_then(|content| {
                serde_json::from_str::<serde_json::Value>(&content).ok()
            })
            .and_then(|v| v.get("launch_count")?.as_u64())
            .map(|count| count <= 5)
            .unwrap_or(true);

        if !is_new_user {
            return Vec::new();
        }

        vec![
            (
                "Customize my terminal theme".to_string(),
                "Find what terminal I'm using, then change its background color to pitch black and make it slightly transparent. Apply the changes for me.".to_string(),
            ),
            (
                "Review something I've been working on".to_string(),
                "Find a recent file or project I've been working on, read through it, and give me concrete suggestions on how I could improve it.".to_string(),
            ),
            (
                "Find my social media and roast me".to_string(),
                "Find a social media platform I use, look around at my profile and posts, then give me a brutally honest roast based on what you see.".to_string(),
            ),
        ]
    }

    /// Autocomplete current input - cycles through suggestions on repeated Tab
    pub fn autocomplete(&mut self) -> bool {
        // Get suggestions for current input
        let current_suggestions = self.get_suggestions_for(&self.input);

        // Check if we're continuing a tab cycle from a previous base
        if let Some((ref base, idx)) = self.tab_completion_state.clone() {
            let base_suggestions = self.get_suggestions_for(&base);

            // If current input is in base suggestions AND there are multiple options, continue cycling
            if base_suggestions.len() > 1
                && base_suggestions.iter().any(|(cmd, _)| cmd == &self.input)
            {
                let next_index = (idx + 1) % base_suggestions.len();
                let (cmd, _) = &base_suggestions[next_index];
                self.input = cmd.clone();
                self.cursor_pos = self.input.len();
                self.tab_completion_state = Some((base.clone(), next_index));
                return true;
            }
            // Otherwise, fall through to start a new cycle with current input
        }

        // Start fresh cycle with current input
        if current_suggestions.is_empty() {
            self.tab_completion_state = None;
            return false;
        }

        // If only one suggestion and it matches exactly, add trailing space for commands
        // that accept arguments, then we're done
        if current_suggestions.len() == 1 && current_suggestions[0].0 == self.input {
            if !self.input.ends_with(' ') && Self::command_accepts_args(&self.input) {
                self.input.push(' ');
                self.cursor_pos = self.input.len();
                return true;
            }
            self.tab_completion_state = None;
            return false;
        }

        // Apply first suggestion and start tracking the cycle
        let (cmd, _) = &current_suggestions[0];
        let base = self.input.clone();
        self.input = cmd.clone();
        // If unique match, add trailing space for arg-accepting commands
        if current_suggestions.len() == 1 && Self::command_accepts_args(&self.input) {
            self.input.push(' ');
        }
        self.cursor_pos = self.input.len();
        self.tab_completion_state = Some((base, 0));
        true
    }

    /// Reset tab completion state (call when user types/modifies input)
    pub fn reset_tab_completion(&mut self) {
        self.tab_completion_state = None;
    }

    fn command_accepts_args(cmd: &str) -> bool {
        matches!(
            cmd.trim(),
            "/help"
                | "/model"
                | "/effort"
                | "/login"
                | "/auth"
                | "/account"
                | "/memory"
                | "/swarm"
                | "/rewind"
                | "/config"
        )
    }

    pub fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn is_processing(&self) -> bool {
        self.is_processing
    }

    pub fn streaming_text(&self) -> &str {
        &self.streaming_text
    }

    pub fn active_skill(&self) -> Option<&str> {
        self.active_skill.as_deref()
    }

    pub fn available_skills(&self) -> Vec<&str> {
        self.skills.list().iter().map(|s| s.name.as_str()).collect()
    }

    pub fn queued_count(&self) -> usize {
        self.queued_messages.len()
    }

    pub fn queued_messages(&self) -> &[String] {
        &self.queued_messages
    }

    pub fn streaming_tokens(&self) -> (u64, u64) {
        (self.streaming_input_tokens, self.streaming_output_tokens)
    }

    fn build_turn_footer(&self, duration: Option<f32>) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(secs) = duration {
            parts.push(format!("{:.1}s", secs));
        }
        if let Some(tps) = self.compute_streaming_tps() {
            parts.push(format!("{:.1} tps", tps));
        }
        if self.streaming_input_tokens > 0 || self.streaming_output_tokens > 0 {
            parts.push(format!(
                "↑{} ↓{}",
                format_tokens(self.streaming_input_tokens),
                format_tokens(self.streaming_output_tokens)
            ));
        }
        if let Some(cache) = format_cache_footer(
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
        ) {
            parts.push(cache);
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    fn push_turn_footer(&mut self, duration: Option<f32>) {
        // Log unexpected cache misses for debugging
        self.log_cache_miss_if_unexpected();

        if let Some(footer) = self.build_turn_footer(duration) {
            self.push_display_message(DisplayMessage {
                role: "meta".to_string(),
                content: footer,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
        }
    }

    /// Log detailed info when an unexpected cache miss occurs (cache write on turn 3+)
    fn log_cache_miss_if_unexpected(&self) {
        let user_turn_count = self
            .display_messages
            .iter()
            .filter(|m| m.role == "user")
            .count();

        // Unexpected cache miss: on turn 3+, we should no longer be in cache warm-up
        let is_unexpected = super::is_unexpected_cache_miss(
            user_turn_count,
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
        );

        if is_unexpected {
            // Collect context for debugging
            let session_id = self.session_id().to_string();
            let provider = self.provider.name().to_string();
            let model = self.provider.model();
            let input_tokens = self.streaming_input_tokens;
            let output_tokens = self.streaming_output_tokens;

            // Format as Option to distinguish None vs Some(0)
            let cache_creation_dbg = format!("{:?}", self.streaming_cache_creation_tokens);
            let cache_read_dbg = format!("{:?}", self.streaming_cache_read_tokens);

            // Count message types in conversation
            let mut user_msgs = 0;
            let mut assistant_msgs = 0;
            let mut tool_msgs = 0;
            let mut other_msgs = 0;
            for msg in &self.display_messages {
                match msg.role.as_str() {
                    "user" => user_msgs += 1,
                    "assistant" => assistant_msgs += 1,
                    "tool_result" | "tool_use" => tool_msgs += 1,
                    _ => other_msgs += 1,
                }
            }

            crate::logging::warn(&format!(
                "CACHE_MISS: unexpected cache miss on turn {} | \
                 cache_creation={} cache_read={} | \
                 input={} output={} | \
                 session={} provider={} model={} | \
                 msgs: user={} assistant={} tool={} other={}",
                user_turn_count,
                cache_creation_dbg,
                cache_read_dbg,
                input_tokens,
                output_tokens,
                session_id,
                provider,
                model,
                user_msgs,
                assistant_msgs,
                tool_msgs,
                other_msgs
            ));
        }
    }

    /// Check if approaching context limit and show warning
    fn check_context_warning(&mut self, input_tokens: u64) {
        let usage_percent = (input_tokens as f64 / self.context_limit as f64) * 100.0;

        // Warn at 70%, 80%, 90%
        if !self.context_warning_shown && usage_percent >= 70.0 {
            let warning = format!(
                "\n⚠️  Context usage: {:.0}% ({}/{}k tokens) - compaction approaching\n\n",
                usage_percent,
                input_tokens / 1000,
                self.context_limit / 1000
            );
            self.streaming_text.push_str(&warning);
            self.context_warning_shown = true;
        } else if self.context_warning_shown && usage_percent >= 80.0 {
            // Reset to show 80% warning
            if usage_percent < 85.0 {
                let warning = format!(
                    "\n⚠️  Context usage: {:.0}% - compaction imminent\n\n",
                    usage_percent
                );
                self.streaming_text.push_str(&warning);
            }
        }
    }

    /// Get context usage as percentage
    pub fn context_usage_percent(&self) -> f64 {
        self.current_stream_context_tokens()
            .map(|tokens| (tokens as f64 / self.context_limit as f64) * 100.0)
            .unwrap_or(0.0)
    }

    /// Time since last streaming event (for detecting stale connections)
    pub fn time_since_activity(&self) -> Option<Duration> {
        self.last_stream_activity.map(|t| t.elapsed())
    }

    pub fn streaming_tool_calls(&self) -> &[ToolCall] {
        &self.streaming_tool_calls
    }

    pub fn status(&self) -> &ProcessingStatus {
        &self.status
    }

    pub fn subagent_status(&self) -> Option<&str> {
        self.subagent_status.as_deref()
    }

    pub fn elapsed(&self) -> Option<Duration> {
        if let Some(d) = self.replay_elapsed_override {
            return Some(d);
        }
        self.processing_started.map(|t| t.elapsed())
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn provider_model(&self) -> String {
        self.provider.model()
    }

    /// Get the upstream provider (e.g., which provider OpenRouter routed to)
    pub fn upstream_provider(&self) -> Option<&str> {
        self.upstream_provider.as_deref()
    }

    pub fn mcp_servers(&self) -> Vec<(String, usize)> {
        self.mcp_server_names.clone()
    }

    /// Calculate approximate line heights for each message (from bottom to top)
    /// Returns vec of (is_user, cumulative_lines_from_bottom)
    fn message_line_positions(&self, width: usize) -> Vec<(bool, usize)> {
        let width = width.max(40); // Minimum width estimate
        let mut positions = Vec::new();
        let mut cumulative = 0usize;

        // Process messages from bottom to top (reverse order)
        for msg in self.display_messages.iter().rev() {
            let is_user = msg.role == "user";

            // Estimate height of this message
            let height = match msg.role.as_str() {
                "user" => {
                    // User messages: "N› content" format
                    let msg_len = msg.content.len() + 4;
                    (msg_len / width).max(1) + 1 // +1 for spacing
                }
                "assistant" => {
                    // Assistant: count lines + wrap estimate
                    let content_lines = msg.content.lines().count().max(1);
                    let avg_line_len = msg.content.len() / content_lines.max(1);
                    let wrap_factor = if avg_line_len > width {
                        (avg_line_len / width) + 1
                    } else {
                        1
                    };
                    let mut h = content_lines * wrap_factor;
                    if !msg.tool_calls.is_empty() {
                        h += 1;
                    }
                    if msg.duration_secs.is_some() {
                        h += 1;
                    }
                    h + 1 // +1 for spacing
                }
                "tool" => 2, // Tool result line + spacing
                _ => 1,
            };

            cumulative += height;
            positions.push((is_user, cumulative));
        }

        positions
    }

    /// Scroll to the previous user prompt (scroll up)
    pub fn scroll_to_prev_prompt(&mut self) {
        let positions = self.message_line_positions(100); // Approximate width

        // Find user messages above current scroll position
        let current = self.scroll_offset;

        // Find the next user message position above current scroll
        for (is_user, pos) in &positions {
            if *is_user && *pos > current + 3 {
                self.scroll_offset = *pos;
                self.auto_scroll_paused = true;
                return;
            }
        }

        // If no more user messages above, scroll to top
        if let Some((_, max_pos)) = positions.last() {
            self.scroll_offset = *max_pos;
            self.auto_scroll_paused = true;
        }
    }

    /// Scroll to the next user prompt (scroll down)
    pub fn scroll_to_next_prompt(&mut self) {
        let positions = self.message_line_positions(100);

        if self.scroll_offset == 0 {
            return; // Already at bottom
        }

        let current = self.scroll_offset;

        // Find user messages, going from bottom up (positions is already reversed)
        // We want the first user message position that's LESS than current
        let mut prev_user_pos = 0usize;
        for (is_user, pos) in &positions {
            if *is_user {
                if *pos >= current {
                    // This user message is at or above current - use the previous one
                    self.scroll_offset = prev_user_pos;
                    if prev_user_pos == 0 {
                        self.follow_chat_bottom();
                    }
                    return;
                }
                prev_user_pos = *pos;
            }
        }

        // No user message found below, go to bottom
        self.follow_chat_bottom();
    }

    /// Scroll to Nth most-recent user prompt (1 = most recent prior prompt).
    fn scroll_to_recent_prompt_rank(&mut self, rank: usize) {
        let rank = rank.max(1);
        let positions = self.message_line_positions(100);
        let user_positions: Vec<usize> = positions
            .iter()
            .filter_map(|(is_user, pos)| if *is_user { Some(*pos) } else { None })
            .collect();

        if user_positions.is_empty() {
            return;
        }

        // If we're at bottom/following, rank 1 is the newest user prompt.
        // If we're already scrolled, rank 1 means the nearest prompt at/above current view.
        let current = if self.auto_scroll_paused {
            self.scroll_offset
        } else {
            0
        };

        let start_idx = if current == 0 {
            0
        } else {
            user_positions
                .iter()
                .position(|&pos| pos >= current)
                .unwrap_or(user_positions.len().saturating_sub(1))
        };

        let target_idx = (start_idx + rank - 1).min(user_positions.len().saturating_sub(1));
        self.scroll_offset = user_positions[target_idx];
        self.auto_scroll_paused = true;
    }

    /// Toggle scroll bookmark: stash current position and jump to bottom,
    /// or restore stashed position if already at bottom.
    fn toggle_scroll_bookmark(&mut self) {
        if let Some(saved) = self.scroll_bookmark.take() {
            // We have a bookmark — teleport back to it
            self.scroll_offset = saved;
            self.auto_scroll_paused = saved > 0;
            self.set_status_notice("📌 Returned to bookmark");
        } else if self.auto_scroll_paused && self.scroll_offset > 0 {
            // We're scrolled up — save position and jump to bottom
            self.scroll_bookmark = Some(self.scroll_offset);
            self.follow_chat_bottom();
            self.set_status_notice("📌 Bookmark set — press again to return");
        }
        // If already at bottom with no bookmark, do nothing
    }

    fn toggle_centered_mode(&mut self) {
        self.centered = !self.centered;
        let mode = if self.centered {
            "Centered"
        } else {
            "Left-aligned"
        };
        self.set_status_notice(format!("Layout: {}", mode));
    }

    pub fn set_centered(&mut self, centered: bool) {
        self.centered = centered;
    }

    // ==================== Debug Socket Methods ====================

    /// Enable debug socket and return the broadcast receiver
    /// Call this before run() to enable debug event broadcasting
    pub fn enable_debug_socket(
        &mut self,
    ) -> tokio::sync::broadcast::Receiver<super::backend::DebugEvent> {
        let (tx, rx) = tokio::sync::broadcast::channel(256);
        self.debug_tx = Some(tx);
        rx
    }

    /// Broadcast a debug event to connected clients (if debug socket enabled)
    fn broadcast_debug(&self, event: super::backend::DebugEvent) {
        if let Some(ref tx) = self.debug_tx {
            let _ = tx.send(event); // Ignore errors (no receivers)
        }
    }

    /// Create a full state snapshot for debug socket
    pub fn create_debug_snapshot(&self) -> super::backend::DebugEvent {
        use super::backend::{DebugEvent, DebugMessage};

        DebugEvent::StateSnapshot {
            display_messages: self
                .display_messages
                .iter()
                .map(|m| DebugMessage {
                    role: m.role.clone(),
                    content: m.content.clone(),
                    tool_calls: m.tool_calls.clone(),
                    duration_secs: m.duration_secs,
                    title: m.title.clone(),
                    tool_data: m.tool_data.clone(),
                })
                .collect(),
            streaming_text: self.streaming_text.clone(),
            streaming_tool_calls: self.streaming_tool_calls.clone(),
            input: self.input.clone(),
            cursor_pos: self.cursor_pos,
            is_processing: self.is_processing,
            scroll_offset: self.scroll_offset,
            status: format!("{:?}", self.status),
            provider_name: self.provider.name().to_string(),
            provider_model: self.provider.model().to_string(),
            mcp_servers: self
                .mcp_server_names
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            skills: self.skills.list().iter().map(|s| s.name.clone()).collect(),
            session_id: self.provider_session_id.clone(),
            input_tokens: self.streaming_input_tokens,
            output_tokens: self.streaming_output_tokens,
            cache_read_input_tokens: self.streaming_cache_read_tokens,
            cache_creation_input_tokens: self.streaming_cache_creation_tokens,
            queued_messages: self.queued_messages.clone(),
        }
    }

    /// Start debug socket listener task
    /// Returns a JoinHandle for the listener task
    pub fn start_debug_socket_listener(
        &self,
        mut rx: tokio::sync::broadcast::Receiver<super::backend::DebugEvent>,
    ) -> tokio::task::JoinHandle<()> {
        use crate::transport::Listener;
        use tokio::io::AsyncWriteExt;

        let socket_path = Self::debug_socket_path();
        let initial_snapshot = self.create_debug_snapshot();

        tokio::spawn(async move {
            // Clean up old socket
            let _ = std::fs::remove_file(&socket_path);

            let mut listener = match Listener::bind(&socket_path) {
                Ok(l) => l,
                Err(e) => {
                    crate::logging::error(&format!("Failed to bind debug socket: {}", e));
                    return;
                }
            };

            // Accept connections and forward events
            let clients: std::sync::Arc<tokio::sync::Mutex<Vec<crate::transport::WriteHalf>>> =
                std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));

            let clients_clone = clients.clone();

            // Spawn event broadcaster
            let broadcast_handle = tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let json = match serde_json::to_string(&event) {
                        Ok(j) => j + "\n",
                        Err(_) => continue,
                    };
                    let bytes = json.as_bytes();

                    let mut clients = clients_clone.lock().await;
                    let mut to_remove = Vec::new();

                    for (i, writer) in clients.iter_mut().enumerate() {
                        if writer.write_all(bytes).await.is_err() {
                            to_remove.push(i);
                        }
                    }

                    // Remove disconnected clients (reverse order to preserve indices)
                    for i in to_remove.into_iter().rev() {
                        clients.swap_remove(i);
                    }
                }
            });

            // Accept new connections
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let (_, writer) = stream.into_split();
                        let mut writer = writer;

                        // Send initial snapshot
                        let snapshot_json =
                            serde_json::to_string(&initial_snapshot).unwrap_or_default() + "\n";
                        if writer.write_all(snapshot_json.as_bytes()).await.is_ok() {
                            clients.lock().await.push(writer);
                        }
                    }
                    Err(_) => break,
                }
            }

            broadcast_handle.abort();
            let _ = std::fs::remove_file(&socket_path);
        })
    }

    /// Get the debug socket path
    pub fn debug_socket_path() -> std::path::PathBuf {
        crate::storage::runtime_dir().join("jcode-debug.sock")
    }
}

/// Update cost calculation based on token usage (for API-key providers)
impl App {
    fn update_cost_impl(&mut self) {
        let provider_name = self.provider.name().to_lowercase();

        // Only calculate cost for API-key providers
        if !provider_name.contains("openrouter")
            && !provider_name.contains("anthropic")
            && !provider_name.contains("openai")
        {
            return;
        }

        // For OAuth providers, cost is already tracked in subscription
        let is_oauth = (provider_name.contains("anthropic") || provider_name.contains("claude"))
            && std::env::var("ANTHROPIC_API_KEY").is_err();
        if is_oauth {
            return;
        }

        // Default pricing (will be cached after first turn)
        let prompt_price = *self.cached_prompt_price.get_or_insert(15.0); // $15/1M tokens default
        let completion_price = *self.cached_completion_price.get_or_insert(60.0); // $60/1M tokens default

        // Calculate cost for this turn
        let prompt_cost = (self.streaming_input_tokens as f32 * prompt_price) / 1_000_000.0;
        let completion_cost =
            (self.streaming_output_tokens as f32 * completion_price) / 1_000_000.0;
        self.total_cost += prompt_cost + completion_cost;
    }

    fn compute_streaming_tps(&self) -> Option<f32> {
        let mut elapsed = self.streaming_tps_elapsed;
        let total_tokens = self.streaming_total_output_tokens;
        if let Some(start) = self.streaming_tps_start {
            elapsed += start.elapsed();
        }
        let elapsed_secs = elapsed.as_secs_f32();
        if elapsed_secs > 0.1 && total_tokens > 0 {
            Some(total_tokens as f32 / elapsed_secs)
        } else {
            None
        }
    }
    fn handle_changelog_key(&mut self, code: KeyCode) -> Result<()> {
        let scroll = self.changelog_scroll.unwrap_or(0);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.changelog_scroll = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.changelog_scroll = Some(scroll.saturating_add(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.changelog_scroll = Some(scroll.saturating_sub(1));
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.changelog_scroll = Some(scroll.saturating_add(20));
            }
            KeyCode::PageUp => {
                self.changelog_scroll = Some(scroll.saturating_sub(20));
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.changelog_scroll = Some(0);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.changelog_scroll = Some(usize::MAX);
            }
            _ => {}
        }
        Ok(())
    }
}

impl super::TuiState for App {
    fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
    }

    fn display_messages_version(&self) -> u64 {
        self.display_messages_version
    }

    fn streaming_text(&self) -> &str {
        &self.streaming_text
    }

    fn input(&self) -> &str {
        &self.input
    }

    fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    fn is_processing(&self) -> bool {
        self.is_processing
    }

    fn queued_messages(&self) -> &[String] {
        &self.queued_messages
    }

    fn interleave_message(&self) -> Option<&str> {
        self.interleave_message.as_deref()
    }

    fn pending_soft_interrupts(&self) -> &[String] {
        &self.pending_soft_interrupts
    }

    fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn auto_scroll_paused(&self) -> bool {
        self.auto_scroll_paused
    }

    fn provider_name(&self) -> String {
        self.remote_provider_name
            .clone()
            .unwrap_or_else(|| self.provider.name().to_string())
    }

    fn provider_model(&self) -> String {
        self.remote_provider_model
            .clone()
            .unwrap_or_else(|| self.provider.model().to_string())
    }

    fn upstream_provider(&self) -> Option<String> {
        self.upstream_provider.clone()
    }

    fn mcp_servers(&self) -> Vec<(String, usize)> {
        self.mcp_server_names.clone()
    }

    fn available_skills(&self) -> Vec<String> {
        self.skills.list().iter().map(|s| s.name.clone()).collect()
    }

    fn streaming_tokens(&self) -> (u64, u64) {
        (self.streaming_input_tokens, self.streaming_output_tokens)
    }

    fn streaming_cache_tokens(&self) -> (Option<u64>, Option<u64>) {
        (
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
        )
    }

    fn output_tps(&self) -> Option<f32> {
        if !self.is_processing {
            return None;
        }
        self.compute_streaming_tps()
    }

    fn streaming_tool_calls(&self) -> Vec<ToolCall> {
        self.streaming_tool_calls.clone()
    }

    fn update_cost(&mut self) {
        self.update_cost_impl()
    }

    fn elapsed(&self) -> Option<std::time::Duration> {
        if let Some(d) = self.replay_elapsed_override {
            return Some(d);
        }
        self.processing_started.map(|t| t.elapsed())
    }

    fn status(&self) -> ProcessingStatus {
        self.status.clone()
    }

    fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        App::command_suggestions(self)
    }

    fn active_skill(&self) -> Option<String> {
        self.active_skill.clone()
    }

    fn subagent_status(&self) -> Option<String> {
        self.subagent_status.clone()
    }

    fn time_since_activity(&self) -> Option<std::time::Duration> {
        self.last_stream_activity.map(|t| t.elapsed())
    }

    fn total_session_tokens(&self) -> Option<(u64, u64)> {
        // In remote mode, use tokens from server
        // Standalone mode doesn't currently track total tokens
        self.remote_total_tokens
    }

    fn is_remote_mode(&self) -> bool {
        self.is_remote
    }

    fn is_canary(&self) -> bool {
        if self.is_remote {
            self.remote_is_canary.unwrap_or(false)
        } else {
            self.session.is_canary
        }
    }

    fn is_replay(&self) -> bool {
        self.is_replay
    }

    fn diff_mode(&self) -> crate::config::DiffDisplayMode {
        self.diff_mode
    }

    fn current_session_id(&self) -> Option<String> {
        if self.is_remote {
            self.remote_session_id.clone()
        } else {
            Some(self.session.id.clone())
        }
    }

    fn session_display_name(&self) -> Option<String> {
        if self.is_remote {
            self.remote_session_id
                .as_ref()
                .and_then(|id| crate::id::extract_session_name(id))
                .map(|s| s.to_string())
        } else {
            Some(self.session.display_name().to_string())
        }
    }

    fn server_display_name(&self) -> Option<String> {
        self.remote_server_short_name.clone()
    }

    fn server_display_icon(&self) -> Option<String> {
        self.remote_server_icon.clone()
    }

    fn server_sessions(&self) -> Vec<String> {
        self.remote_sessions.clone()
    }

    fn connected_clients(&self) -> Option<usize> {
        self.remote_client_count
    }

    fn status_notice(&self) -> Option<String> {
        self.status_notice.as_ref().and_then(|(text, at)| {
            if at.elapsed() <= Duration::from_secs(3) {
                Some(text.clone())
            } else {
                None
            }
        })
    }

    fn animation_elapsed(&self) -> f32 {
        self.app_started.elapsed().as_secs_f32()
    }

    fn rate_limit_remaining(&self) -> Option<Duration> {
        self.rate_limit_reset.and_then(|reset_time| {
            let now = Instant::now();
            if reset_time > now {
                Some(reset_time - now)
            } else {
                None
            }
        })
    }

    fn queue_mode(&self) -> bool {
        self.queue_mode
    }

    fn context_info(&self) -> crate::prompt::ContextInfo {
        use crate::message::{ContentBlock, Role};

        let mut info = self.context_info.clone();

        // Compute dynamic stats from conversation
        let mut user_chars = 0usize;
        let mut user_count = 0usize;
        let mut asst_chars = 0usize;
        let mut asst_count = 0usize;
        let mut tool_call_chars = 0usize;
        let mut tool_call_count = 0usize;
        let mut tool_result_chars = 0usize;
        let mut tool_result_count = 0usize;

        if self.is_remote {
            for msg in &self.display_messages {
                match msg.role.as_str() {
                    "user" => {
                        user_count += 1;
                        user_chars += msg.content.len();
                    }
                    "assistant" => {
                        asst_count += 1;
                        asst_chars += msg.content.len();
                    }
                    "tool" => {
                        tool_result_count += 1;
                        tool_result_chars += msg.content.len();
                        if let Some(tool) = &msg.tool_data {
                            tool_call_count += 1;
                            tool_call_chars += tool.name.len() + tool.input.to_string().len();
                        }
                    }
                    _ => {}
                }
            }
        } else {
            let skip = if self.provider.supports_compaction() {
                let compaction = self.registry.compaction();
                let result = compaction
                    .try_read()
                    .ok()
                    .map(|manager| (manager.compacted_count(), manager.summary_chars()));
                if let Some((cc, sc)) = result {
                    if cc > 0 && sc > 0 {
                        user_count += 1;
                        user_chars += sc;
                    }
                    cc
                } else {
                    0
                }
            } else {
                0
            };

            for msg in self.messages.iter().skip(skip) {
                match msg.role {
                    Role::User => user_count += 1,
                    Role::Assistant => asst_count += 1,
                }

                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => match msg.role {
                            Role::User => user_chars += text.len(),
                            Role::Assistant => asst_chars += text.len(),
                        },
                        ContentBlock::ToolUse { name, input, .. } => {
                            tool_call_count += 1;
                            tool_call_chars += name.len() + input.to_string().len();
                        }
                        ContentBlock::ToolResult { content, .. } => {
                            tool_result_count += 1;
                            tool_result_chars += content.len();
                        }
                        ContentBlock::Reasoning { text } => {
                            asst_chars += text.len();
                        }
                        ContentBlock::Image { data, .. } => {
                            user_chars += data.len();
                        }
                    }
                }
            }
        }

        // Estimate tool definitions size
        // jcode has ~25 built-in tools, each ~500 chars in definition
        // This is a rough estimate since we can't easily call async from here
        let tool_defs_count = 25;
        let tool_defs_chars = tool_defs_count * 500;

        info.user_messages_chars = user_chars;
        info.user_messages_count = user_count;
        info.assistant_messages_chars = asst_chars;
        info.assistant_messages_count = asst_count;
        info.tool_calls_chars = tool_call_chars;
        info.tool_calls_count = tool_call_count;
        info.tool_results_chars = tool_result_chars;
        info.tool_results_count = tool_result_count;
        info.tool_defs_chars = tool_defs_chars;
        info.tool_defs_count = tool_defs_count;

        // Update total
        info.total_chars = info.system_prompt_chars
            + info.env_context_chars
            + info.project_agents_md_chars
            + info.project_claude_md_chars
            + info.global_agents_md_chars
            + info.global_claude_md_chars
            + info.skills_chars
            + info.selfdev_chars
            + info.memory_chars
            + info.tool_defs_chars
            + info.user_messages_chars
            + info.assistant_messages_chars
            + info.tool_calls_chars
            + info.tool_results_chars;

        info
    }

    fn context_limit(&self) -> Option<usize> {
        Some(self.context_limit as usize)
    }

    fn client_update_available(&self) -> bool {
        self.has_newer_binary()
    }

    fn server_update_available(&self) -> Option<bool> {
        if self.is_remote {
            self.remote_server_has_update
        } else {
            None
        }
    }

    fn info_widget_data(&self) -> super::info_widget::InfoWidgetData {
        let session_id = if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        };

        let todos = if self.swarm_enabled && !self.swarm_plan_items.is_empty() {
            self.swarm_plan_items
                .iter()
                .map(|item| crate::todo::TodoItem {
                    content: item.content.clone(),
                    status: item.status.clone(),
                    priority: item.priority.clone(),
                    id: item.id.clone(),
                    blocked_by: item.blocked_by.clone(),
                    assigned_to: item.assigned_to.clone(),
                })
                .collect()
        } else {
            session_id
                .and_then(|id| crate::todo::load_todos(id).ok())
                .unwrap_or_default()
        };

        let context_info = self.context_info();
        let context_info = if context_info.total_chars > 0 {
            Some(context_info)
        } else {
            None
        };

        let (model, reasoning_effort) = if self.is_remote || self.is_replay {
            (self.remote_provider_model.clone(), None)
        } else {
            (
                Some(self.provider.model()),
                self.provider.reasoning_effort(),
            )
        };

        let (session_count, client_count) = if self.is_remote {
            (Some(self.remote_sessions.len()), None)
        } else {
            (None, None)
        };
        let session_name = self.session_display_name().map(|name| {
            if let Some(ref srv) = self.remote_server_short_name {
                format!("{} {}", srv, name)
            } else {
                name
            }
        });

        // Gather memory info
        let memory_info = if self.memory_enabled {
            use crate::memory::MemoryManager;

            let manager = MemoryManager::new();
            let project_graph = manager.load_project_graph().ok();
            let global_graph = manager.load_global_graph().ok();

            let (project_count, global_count, by_category) = {
                let mut by_category = std::collections::HashMap::new();
                let project_count = project_graph
                    .as_ref()
                    .map(|p| {
                        for entry in p.memories.values() {
                            *by_category.entry(entry.category.to_string()).or_insert(0) += 1;
                        }
                        p.memory_count()
                    })
                    .unwrap_or(0);
                let global_count = global_graph
                    .as_ref()
                    .map(|g| {
                        for entry in g.memories.values() {
                            *by_category.entry(entry.category.to_string()).or_insert(0) += 1;
                        }
                        g.memory_count()
                    })
                    .unwrap_or(0);
                (project_count, global_count, by_category)
            };

            let total_count = project_count + global_count;
            let activity = crate::memory::get_activity();

            // Build graph topology for visualization
            let (graph_nodes, graph_edges) = super::info_widget::build_graph_topology(
                project_graph.as_ref(),
                global_graph.as_ref(),
            );

            // Show memory info if we have memories OR if there's activity (agent working)
            if total_count > 0 || activity.is_some() {
                Some(super::info_widget::MemoryInfo {
                    total_count,
                    project_count,
                    global_count,
                    by_category,
                    sidecar_available: true,
                    activity,
                    graph_nodes,
                    graph_edges,
                })
            } else {
                None
            }
        } else {
            None
        };

        // Gather swarm info
        let swarm_info = if self.swarm_enabled {
            let subagent_status = self.subagent_status.clone();
            let mut members: Vec<crate::protocol::SwarmMemberStatus> = Vec::new();
            let (session_count, client_count, session_names, has_activity) = if self.is_remote {
                members = self.remote_swarm_members.clone();
                let session_names = if !members.is_empty() {
                    members
                        .iter()
                        .map(|m| {
                            m.friendly_name
                                .clone()
                                .unwrap_or_else(|| m.session_id.chars().take(8).collect())
                        })
                        .collect()
                } else {
                    self.remote_sessions.clone()
                };
                let session_count = if !members.is_empty() {
                    members.len()
                } else {
                    self.remote_sessions.len()
                };
                let has_activity = members
                    .iter()
                    .any(|m| m.status != "ready" || m.detail.is_some());
                (
                    session_count,
                    self.remote_client_count,
                    session_names,
                    has_activity,
                )
            } else {
                let (status, detail) = match &self.status {
                    ProcessingStatus::Idle => ("ready".to_string(), None),
                    ProcessingStatus::Sending => {
                        ("running".to_string(), Some("sending".to_string()))
                    }
                    ProcessingStatus::Connecting(phase) => {
                        ("running".to_string(), Some(phase.to_string()))
                    }
                    ProcessingStatus::Thinking(_) => ("thinking".to_string(), None),
                    ProcessingStatus::Streaming => {
                        ("running".to_string(), Some("streaming".to_string()))
                    }
                    ProcessingStatus::RunningTool(name) => {
                        ("running".to_string(), Some(format!("tool: {}", name)))
                    }
                };
                let detail = subagent_status.clone().or(detail);
                let has_activity = status != "ready" || detail.is_some();
                if has_activity {
                    members.push(crate::protocol::SwarmMemberStatus {
                        session_id: self.session.id.clone(),
                        friendly_name: Some(self.session.display_name().to_string()),
                        status,
                        detail,
                        role: None,
                    });
                }
                (
                    1,
                    None,
                    vec![self.session.display_name().to_string()],
                    has_activity,
                )
            };

            // Only show if there's something interesting
            if has_activity || session_count > 1 || client_count.is_some() {
                Some(super::info_widget::SwarmInfo {
                    session_count,
                    subagent_status,
                    client_count,
                    session_names,
                    members,
                })
            } else {
                None
            }
        } else {
            None
        };

        // Gather background task info
        let background_info = {
            let memory_agent_active = self.memory_enabled && crate::memory_agent::is_active();
            let memory_stats = crate::memory_agent::stats();

            // Get running background tasks count
            let bg_manager = crate::background::global();
            let (running_count, running_tasks) = bg_manager.running_snapshot();

            if memory_agent_active || running_count > 0 {
                Some(super::info_widget::BackgroundInfo {
                    running_count,
                    running_tasks,
                    memory_agent_active,
                    memory_agent_turns: memory_stats.turns_processed,
                })
            } else {
                None
            }
        };

        // Gather subscription usage info
        let usage_info = {
            // Check if current provider uses OAuth (Anthropic OAuth or OpenAI Codex)
            let provider_name = self.provider.name().to_lowercase();
            // Also check for "remote" provider with OAuth credentials (selfdev/client mode)
            let has_anthropic_oauth = crate::auth::claude::has_credentials();
            let has_openai_oauth = crate::auth::codex::load_credentials().is_ok();
            let is_anthropic_oauth = provider_name.contains("anthropic")
                || provider_name.contains("claude")
                || (provider_name == "remote" && has_anthropic_oauth && !has_openai_oauth);
            let is_openai_provider = provider_name.contains("openai")
                || ((provider_name == "remote" || provider_name == "unknown")
                    && has_openai_oauth
                    && !has_anthropic_oauth);
            let is_api_key_provider = provider_name.contains("openrouter");
            let is_copilot_provider = provider_name.contains("copilot");

            let output_tps = if self.is_processing {
                self.compute_streaming_tps()
            } else {
                None
            };

            if is_anthropic_oauth {
                let usage = crate::usage::get_sync();
                Some(super::info_widget::UsageInfo {
                    provider: super::info_widget::UsageProvider::Anthropic,
                    five_hour: usage.five_hour,
                    five_hour_resets_at: usage.five_hour_resets_at.clone(),
                    seven_day: usage.seven_day,
                    seven_day_resets_at: usage.seven_day_resets_at.clone(),
                    spark: None,
                    spark_resets_at: None,
                    total_cost: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    output_tps,
                    available: true,
                })
            } else if is_openai_provider {
                let openai_usage = crate::usage::get_openai_usage_sync();
                if openai_usage.has_limits() {
                    Some(super::info_widget::UsageInfo {
                        provider: super::info_widget::UsageProvider::OpenAI,
                        five_hour: openai_usage
                            .five_hour
                            .as_ref()
                            .map(|w| w.usage_ratio)
                            .unwrap_or(0.0),
                        five_hour_resets_at: openai_usage
                            .five_hour
                            .as_ref()
                            .and_then(|w| w.resets_at.clone()),
                        seven_day: openai_usage
                            .seven_day
                            .as_ref()
                            .map(|w| w.usage_ratio)
                            .unwrap_or(0.0),
                        seven_day_resets_at: openai_usage
                            .seven_day
                            .as_ref()
                            .and_then(|w| w.resets_at.clone()),
                        spark: openai_usage.spark.as_ref().map(|w| w.usage_ratio),
                        spark_resets_at: openai_usage
                            .spark
                            .as_ref()
                            .and_then(|w| w.resets_at.clone()),
                        total_cost: 0.0,
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: None,
                        cache_write_tokens: None,
                        output_tps,
                        available: true,
                    })
                } else {
                    Some(super::info_widget::UsageInfo {
                        provider: super::info_widget::UsageProvider::CostBased,
                        five_hour: 0.0,
                        five_hour_resets_at: None,
                        seven_day: 0.0,
                        seven_day_resets_at: None,
                        spark: None,
                        spark_resets_at: None,
                        total_cost: self.total_cost,
                        input_tokens: self.total_input_tokens,
                        output_tokens: self.total_output_tokens,
                        cache_read_tokens: self.streaming_cache_read_tokens,
                        cache_write_tokens: self.streaming_cache_creation_tokens,
                        output_tps,
                        available: true,
                    })
                }
            } else if is_api_key_provider {
                // Show costs for API-key providers (OpenRouter)
                Some(super::info_widget::UsageInfo {
                    provider: super::info_widget::UsageProvider::CostBased,
                    five_hour: 0.0,
                    five_hour_resets_at: None,
                    seven_day: 0.0,
                    seven_day_resets_at: None,
                    spark: None,
                    spark_resets_at: None,
                    total_cost: self.total_cost,
                    input_tokens: self.total_input_tokens,
                    output_tokens: self.total_output_tokens,
                    cache_read_tokens: self.streaming_cache_read_tokens,
                    cache_write_tokens: self.streaming_cache_creation_tokens,
                    output_tps,
                    available: true,
                })
            } else if is_copilot_provider {
                Some(super::info_widget::UsageInfo {
                    provider: super::info_widget::UsageProvider::Copilot,
                    five_hour: 0.0,
                    five_hour_resets_at: None,
                    seven_day: 0.0,
                    seven_day_resets_at: None,
                    spark: None,
                    spark_resets_at: None,
                    total_cost: 0.0,
                    input_tokens: self.total_input_tokens,
                    output_tokens: self.total_output_tokens,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    output_tps,
                    available: self.total_input_tokens > 0 || self.total_output_tokens > 0,
                })
            } else {
                None
            }
        };

        let tokens_per_second = self.compute_streaming_tps();

        // Determine authentication method
        let auth_method = if self.is_remote {
            super::info_widget::AuthMethod::Unknown
        } else {
            let provider_name = self.provider.name().to_lowercase();
            if provider_name.contains("anthropic") || provider_name.contains("claude") {
                // Check if using OAuth or API key
                if crate::auth::claude::has_credentials() {
                    super::info_widget::AuthMethod::AnthropicOAuth
                } else if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                    super::info_widget::AuthMethod::AnthropicApiKey
                } else {
                    super::info_widget::AuthMethod::Unknown
                }
            } else if provider_name.contains("openai") {
                // Check if using OAuth or API key
                match crate::auth::codex::load_credentials() {
                    Ok(creds) if !creds.refresh_token.is_empty() => {
                        super::info_widget::AuthMethod::OpenAIOAuth
                    }
                    _ => {
                        if std::env::var("OPENAI_API_KEY").is_ok() {
                            super::info_widget::AuthMethod::OpenAIApiKey
                        } else {
                            super::info_widget::AuthMethod::Unknown
                        }
                    }
                }
            } else if provider_name.contains("openrouter") {
                super::info_widget::AuthMethod::OpenRouterApiKey
            } else if provider_name.contains("copilot") {
                super::info_widget::AuthMethod::CopilotOAuth
            } else {
                super::info_widget::AuthMethod::Unknown
            }
        };

        // Get active mermaid diagrams - only for margin mode (pinned mode uses dedicated pane)
        let diagrams = if self.diagram_mode == crate::config::DiagramDisplayMode::Margin {
            super::mermaid::get_active_diagrams()
        } else {
            Vec::new()
        };

        super::info_widget::InfoWidgetData {
            todos,
            context_info,
            queue_mode: Some(self.queue_mode),
            context_limit: Some(self.context_limit as usize),
            model,
            reasoning_effort,
            session_count,
            session_name,
            client_count,
            memory_info,
            swarm_info,
            background_info,
            usage_info,
            tokens_per_second,
            provider_name: if self.is_remote || self.is_replay {
                self.remote_provider_name
                    .clone()
                    .or_else(|| Some(self.provider.name().to_string()))
            } else {
                Some(self.provider.name().to_string())
            },
            auth_method,
            upstream_provider: self.upstream_provider.clone(),
            connection_type: self.connection_type.clone(),
            diagrams,
            ambient_info: if crate::config::config().ambient.enabled {
                let state = crate::ambient::AmbientState::load().unwrap_or_default();
                let last_run_ago = state.last_run.map(|t| {
                    let ago = chrono::Utc::now() - t;
                    if ago.num_hours() > 0 {
                        format!("{}h ago", ago.num_hours())
                    } else {
                        format!("{}m ago", ago.num_minutes().max(0))
                    }
                });
                let next_wake = match &state.status {
                    crate::ambient::AmbientStatus::Scheduled { next_wake } => {
                        let until = *next_wake - chrono::Utc::now();
                        let mins = until.num_minutes().max(0);
                        Some(format!("in {}m", mins))
                    }
                    _ => None,
                };
                Some(super::info_widget::AmbientWidgetData {
                    status: state.status,
                    queue_count: crate::ambient::AmbientManager::new()
                        .map(|m| m.queue().len())
                        .unwrap_or(0),
                    next_queue_preview: None,
                    last_run_ago,
                    last_summary: state.last_summary,
                    next_wake,
                    budget_percent: None,
                })
            } else {
                None
            },
            observed_context_tokens: self.current_stream_context_tokens(),
            is_compacting: if !self.is_remote && self.provider.supports_compaction() {
                let compaction = self.registry.compaction();
                compaction
                    .try_read()
                    .map(|m| m.is_compacting())
                    .unwrap_or(false)
            } else {
                false
            },
            git_info: gather_git_info(),
        }
    }

    fn render_streaming_markdown(&self, width: usize) -> Vec<ratatui::text::Line<'static>> {
        let mut renderer = self.streaming_md_renderer.borrow_mut();
        renderer.set_width(Some(width));
        renderer.update(&self.streaming_text)
    }

    fn centered_mode(&self) -> bool {
        self.centered
    }

    fn auth_status(&self) -> crate::auth::AuthStatus {
        crate::auth::AuthStatus::check()
    }

    fn diagram_mode(&self) -> crate::config::DiagramDisplayMode {
        self.diagram_mode
    }

    fn diagram_focus(&self) -> bool {
        self.diagram_focus
    }

    fn diagram_index(&self) -> usize {
        self.diagram_index
    }

    fn diagram_scroll(&self) -> (i32, i32) {
        (self.diagram_scroll_x, self.diagram_scroll_y)
    }

    fn diagram_pane_ratio(&self) -> u8 {
        self.animated_diagram_pane_ratio()
    }

    fn diagram_pane_animating(&self) -> bool {
        self.diagram_pane_anim_start
            .map(|s| s.elapsed().as_secs_f32() < Self::DIAGRAM_PANE_ANIM_DURATION)
            .unwrap_or(false)
    }

    fn diagram_pane_enabled(&self) -> bool {
        self.diagram_pane_enabled
    }

    fn diagram_pane_position(&self) -> crate::config::DiagramPanePosition {
        self.diagram_pane_position
    }

    fn diagram_zoom(&self) -> u8 {
        self.diagram_zoom
    }
    fn diff_pane_scroll(&self) -> usize {
        self.diff_pane_scroll
    }
    fn diff_pane_focus(&self) -> bool {
        self.diff_pane_focus
    }
    fn pin_images(&self) -> bool {
        self.pin_images
    }
    fn diff_line_wrap(&self) -> bool {
        crate::config::config().display.diff_line_wrap
    }
    fn picker_state(&self) -> Option<&super::PickerState> {
        self.picker_state.as_ref()
    }

    fn changelog_scroll(&self) -> Option<usize> {
        self.changelog_scroll
    }

    fn working_dir(&self) -> Option<String> {
        self.session.working_dir.clone()
    }

    fn now_millis(&self) -> u64 {
        self.app_started.elapsed().as_millis() as u64
    }

    fn suggestion_prompts(&self) -> Vec<(String, String)> {
        App::suggestion_prompts(self)
    }
}

/// Copy text to clipboard, trying wl-copy first (Wayland), then arboard as fallback.
fn copy_to_clipboard(text: &str) -> bool {
    if let Ok(mut child) = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut() {
            if stdin.write_all(text.as_bytes()).is_ok() {
                drop(child.stdin.take());
                return child.wait().map(|s| s.success()).unwrap_or(false);
            }
        }
    }
    arboard::Clipboard::new()
        .and_then(|mut cb| cb.set_text(text.to_string()))
        .is_ok()
}

fn effort_display_label(effort: &str) -> &str {
    match effort {
        "xhigh" => "Max",
        "high" => "High",
        "medium" => "Medium",
        "low" => "Low",
        "none" => "None",
        other => other,
    }
}

fn effort_bar(index: usize, total: usize) -> String {
    let mut bar = String::new();
    for i in 0..total {
        if i == index {
            bar.push('●');
        } else {
            bar.push('○');
        }
    }
    bar
}

/// Spawn a new terminal window that resumes a jcode session.
/// Returns Ok(true) if a terminal was successfully launched, Ok(false) if no terminal found.
#[cfg(unix)]
fn spawn_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    socket: Option<&str>,
) -> anyhow::Result<bool> {
    use std::process::{Command, Stdio};

    let mut candidates: Vec<String> = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        if !term.trim().is_empty() {
            candidates.push(term);
        }
    }
    candidates.extend(
        [
            "kitty",
            "wezterm",
            "alacritty",
            "gnome-terminal",
            "konsole",
            "xterm",
            "foot",
        ]
        .iter()
        .map(|s| s.to_string()),
    );

    for term in candidates {
        let mut cmd = Command::new(&term);
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match term.as_str() {
            "kitty" => {
                cmd.args(["--title", "jcode split", "-e"])
                    .arg(exe)
                    .arg("--resume")
                    .arg(session_id);
                if let Some(socket) = socket {
                    cmd.arg("--socket").arg(socket);
                }
            }
            "wezterm" => {
                cmd.args([
                    "start",
                    "--always-new-process",
                    "--",
                    exe.to_string_lossy().as_ref(),
                    "--resume",
                    session_id,
                ]);
            }
            "alacritty" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "gnome-terminal" => {
                cmd.args(["--", exe.to_string_lossy().as_ref(), "--resume", session_id]);
            }
            "konsole" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "xterm" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            "foot" => {
                cmd.args(["-e"]).arg(exe).arg("--resume").arg(session_id);
            }
            _ => continue,
        }

        if cmd.spawn().is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(not(unix))]
fn spawn_in_new_terminal(
    _exe: &std::path::Path,
    _session_id: &str,
    _cwd: &std::path::Path,
    _socket: Option<&str>,
) -> anyhow::Result<bool> {
    Ok(false)
}

/// Try to get an image from the system clipboard.
///
/// Returns `Some((media_type, base64_data))` if an image is available.
/// Uses `wl-paste` on Wayland, `osascript` on macOS, falls back to `arboard::get_image()`.
fn clipboard_image() -> Option<(String, String)> {
    use base64::Engine;

    // Try wl-paste first (native Wayland - better image format support)
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        if let Ok(output) = std::process::Command::new("wl-paste")
            .arg("--list-types")
            .output()
        {
            let types = String::from_utf8_lossy(&output.stdout);
            crate::logging::info(&format!(
                "clipboard_image: wl-paste types: {:?}",
                types.trim()
            ));
            let (mime, wl_type) = if types.lines().any(|t| t.trim() == "image/png") {
                ("image/png", "image/png")
            } else if types.lines().any(|t| t.trim() == "image/jpeg") {
                ("image/jpeg", "image/jpeg")
            } else if types.lines().any(|t| t.trim() == "image/webp") {
                ("image/webp", "image/webp")
            } else if types.lines().any(|t| t.trim() == "image/gif") {
                ("image/gif", "image/gif")
            } else {
                ("", "")
            };

            if !mime.is_empty() {
                if let Ok(img_output) = std::process::Command::new("wl-paste")
                    .args(["--type", wl_type, "--no-newline"])
                    .output()
                {
                    if img_output.status.success() && !img_output.stdout.is_empty() {
                        let b64 =
                            base64::engine::general_purpose::STANDARD.encode(&img_output.stdout);
                        return Some((mime.to_string(), b64));
                    }
                }
            }

            // Fallback: check text/html for <img> tags (Discord copies HTML with image URLs)
            if types.lines().any(|t| t.trim() == "text/html") {
                if let Ok(html_output) = std::process::Command::new("wl-paste")
                    .args(["--type", "text/html"])
                    .output()
                {
                    if html_output.status.success() && !html_output.stdout.is_empty() {
                        let html = String::from_utf8_lossy(&html_output.stdout);
                        crate::logging::info(&format!(
                            "clipboard_image: checking HTML for img tags ({} bytes)",
                            html.len()
                        ));
                        if let Some(url) = extract_image_url(&html) {
                            crate::logging::info(&format!(
                                "clipboard_image: found image URL in HTML: {}",
                                &url[..url.len().min(80)]
                            ));
                            if let Some(result) = download_image_url(&url) {
                                return Some(result);
                            }
                        }
                    }
                }
            }
        }
    }

    // macOS: use osascript to check clipboard for images and save as PNG via temp file
    #[cfg(target_os = "macos")]
    {
        let temp_path = std::env::temp_dir().join("jcode_clipboard.png");
        let script = format!(
            r#"use framework "AppKit"
            set pb to current application's NSPasteboard's generalPasteboard()
            set imgClasses to current application's NSArray's arrayWithObject:(current application's NSImage)
            if (pb's canReadObjectForClasses:imgClasses options:(missing value)) then
                set imgList to pb's readObjectsForClasses:imgClasses options:(missing value)
                set img to item 1 of imgList
                set tiffData to img's TIFFRepresentation()
                set bitmapRep to current application's NSBitmapImageRep's imageRepWithData:tiffData
                set pngData to bitmapRep's representationUsingType:(current application's NSBitmapImageFileTypePNG) properties:(missing value)
                pngData's writeToFile:"{}" atomically:true
                return "ok"
            else
                return "none"
            end if"#,
            temp_path.to_string_lossy()
        );
        if let Ok(output) = std::process::Command::new("osascript")
            .args(["-l", "AppleScript", "-e", &script])
            .output()
        {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if result == "ok" {
                if let Ok(data) = std::fs::read(&temp_path) {
                    let _ = std::fs::remove_file(&temp_path);
                    if !data.is_empty() {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                        return Some(("image/png".to_string(), b64));
                    }
                }
            }
        }
    }

    // Fallback: arboard (works on X11/XWayland and macOS via NSPasteboard)
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(img) = clipboard.get_image() {
            // img.bytes is RGBA pixel data - encode as PNG
            if let Some(png_data) = encode_rgba_as_png(img.width, img.height, &img.bytes) {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png_data);
                return Some(("image/png".to_string(), b64));
            }
        }
    }

    None
}

/// Extract an image URL from text that looks like an HTML img tag or a bare image URL.
/// Returns the URL if found.
fn extract_image_url(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Check for <img src="..."> pattern (Discord web copies)
    if let Some(start) = trimmed.find("<img") {
        if let Some(src_start) = trimmed[start..].find("src=\"") {
            let url_start = start + src_start + 5;
            if let Some(url_end) = trimmed[url_start..].find('"') {
                let url = &trimmed[url_start..url_start + url_end];
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
        if let Some(src_start) = trimmed[start..].find("src='") {
            let url_start = start + src_start + 5;
            if let Some(url_end) = trimmed[url_start..].find('\'') {
                let url = &trimmed[url_start..url_start + url_end];
                if url.starts_with("http") {
                    return Some(url.to_string());
                }
            }
        }
    }

    // Check for bare image URL
    if trimmed.starts_with("http")
        && (trimmed.contains(".png")
            || trimmed.contains(".jpg")
            || trimmed.contains(".jpeg")
            || trimmed.contains(".gif")
            || trimmed.contains(".webp"))
    {
        // Strip query params for extension check but return full URL
        return Some(trimmed.to_string());
    }

    None
}

/// Download an image from a URL and return (media_type, base64_data).
/// Uses curl for simplicity (available on all platforms).
fn download_image_url(url: &str) -> Option<(String, String)> {
    use base64::Engine;

    let output = std::process::Command::new("curl")
        .args(["-sL", "--max-time", "10", "--max-filesize", "10000000", url])
        .output()
        .ok()?;

    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }

    // Detect image type from magic bytes
    let data = &output.stdout;
    let media_type = if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.starts_with(b"RIFF") && data.len() > 12 && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };

    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    Some((media_type.to_string(), b64))
}

/// Encode raw RGBA pixel data as PNG bytes
fn encode_rgba_as_png(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    use image::{ImageBuffer, RgbaImage};
    use std::io::Cursor;

    let img: RgbaImage = ImageBuffer::from_raw(width as u32, height as u32, rgba.to_vec())?;
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(buf)
}

fn gather_git_info() -> Option<super::info_widget::GitInfo> {
    use std::sync::Mutex;
    use std::time::Instant;

    static CACHE: Mutex<Option<(Instant, Option<super::info_widget::GitInfo>)>> = Mutex::new(None);

    const TTL: std::time::Duration = std::time::Duration::from_secs(5);

    if let Ok(guard) = CACHE.lock() {
        if let Some((ts, ref cached)) = *guard {
            if ts.elapsed() < TTL {
                return cached.clone();
            }
        }
    }

    let result = gather_git_info_inner();

    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some((Instant::now(), result.clone()));
    }

    result
}

fn gather_git_info_inner() -> Option<super::info_widget::GitInfo> {
    use std::process::Command;

    let in_repo = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !in_repo {
        return None;
    }

    let branch = Command::new("git")
        .args(["branch", "--show-current"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let b = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if b.is_empty() {
                    None
                } else {
                    Some(b)
                }
            } else {
                None
            }
        })
        .unwrap_or_else(|| "HEAD".to_string());

    let mut modified = 0;
    let mut staged = 0;
    let mut untracked = 0;
    let mut dirty_files = Vec::new();

    if let Ok(output) = Command::new("git").args(["status", "--porcelain"]).output() {
        if output.status.success() {
            let status = String::from_utf8_lossy(&output.stdout);
            for line in status.lines() {
                if line.len() < 3 {
                    continue;
                }
                let index_status = line.as_bytes()[0];
                let worktree_status = line.as_bytes()[1];
                let file_path = line[3..].to_string();

                if index_status == b'?' {
                    untracked += 1;
                } else {
                    if index_status != b' ' && index_status != b'?' {
                        staged += 1;
                    }
                    if worktree_status != b' ' && worktree_status != b'?' {
                        modified += 1;
                    }
                }

                if dirty_files.len() < 10 {
                    dirty_files.push(file_path);
                }
            }
        }
    }

    let (ahead, behind) = Command::new("git")
        .args(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let parts: Vec<&str> = text.split('\t').collect();
                if parts.len() == 2 {
                    let a = parts[0].parse::<usize>().unwrap_or(0);
                    let b = parts[1].parse::<usize>().unwrap_or(0);
                    Some((a, b))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or((0, 0));

    Some(super::info_widget::GitInfo {
        branch,
        modified,
        staged,
        untracked,
        ahead,
        behind,
        dirty_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock provider for testing
    struct MockProvider;

    #[async_trait::async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            unimplemented!("Mock provider")
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MockProvider)
        }
    }

    fn create_test_app() -> App {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
        let mut app = App::new(provider, registry);
        app.queue_mode = false;
        app.diff_mode = crate::config::DiffDisplayMode::Inline;
        app
    }

    #[test]
    fn test_help_topic_shows_command_details() {
        let mut app = create_test_app();
        app.input = "/help compact".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing help response");
        assert_eq!(msg.role, "system");
        assert!(msg.content.contains("`/compact`"));
        assert!(msg.content.contains("background"));
    }

    #[test]
    fn test_help_topic_shows_fix_command_details() {
        let mut app = create_test_app();
        app.input = "/help fix".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing help response");
        assert_eq!(msg.role, "system");
        assert!(msg.content.contains("`/fix`"));
    }

    #[test]
    fn test_commands_alias_shows_help() {
        let mut app = create_test_app();
        app.input = "/commands".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing help response");
        assert_eq!(msg.role, "system");
        assert!(msg.content.contains("**Commands:**"));
    }

    #[test]
    fn test_fix_resets_provider_session() {
        let mut app = create_test_app();
        app.provider_session_id = Some("provider-session".to_string());
        app.session.provider_session_id = Some("provider-session".to_string());
        app.last_stream_error = Some("Stream error: context window exceeded".to_string());

        app.input = "/fix".to_string();
        app.submit_input();

        assert!(app.provider_session_id.is_none());
        assert!(app.session.provider_session_id.is_none());

        let msg = app
            .display_messages()
            .last()
            .expect("missing /fix response");
        assert_eq!(msg.role, "system");
        assert!(msg.content.contains("Fix Results"));
        assert!(msg.content.contains("Reset provider session resume state"));
    }

    #[test]
    fn test_context_limit_error_detection() {
        assert!(is_context_limit_error(
            "OpenAI API error 400: This model's maximum context length is 200000 tokens"
        ));
        assert!(is_context_limit_error(
            "request too large: prompt is too long for context window"
        ));
        assert!(!is_context_limit_error(
            "rate limit exceeded, retry after 20s"
        ));
    }

    #[test]
    fn test_rewind_truncates_provider_messages() {
        let mut app = create_test_app();

        for idx in 1..=3 {
            let text = format!("msg-{}", idx);
            app.add_provider_message(Message::user(&text));
            app.session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text,
                    cache_control: None,
                }],
            );
        }
        app.provider_session_id = Some("provider-session".to_string());
        app.session.provider_session_id = Some("provider-session".to_string());

        app.input = "/rewind 2".to_string();
        app.submit_input();

        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.session.messages.len(), 2);
        assert!(matches!(
            &app.messages[1].content[0],
            ContentBlock::Text { text, .. } if text == "msg-2"
        ));
        assert!(app.provider_session_id.is_none());
        assert!(app.session.provider_session_id.is_none());
    }

    #[test]
    fn test_accumulate_streaming_output_tokens_uses_deltas() {
        let mut app = create_test_app();
        let mut seen = 0;

        app.accumulate_streaming_output_tokens(10, &mut seen);
        app.accumulate_streaming_output_tokens(30, &mut seen);
        app.accumulate_streaming_output_tokens(30, &mut seen);

        assert_eq!(app.streaming_total_output_tokens, 30);
        assert_eq!(seen, 30);
    }

    #[test]
    fn test_initial_state() {
        let app = create_test_app();

        assert!(!app.is_processing());
        assert!(app.input().is_empty());
        assert_eq!(app.cursor_pos(), 0);
        assert!(app.display_messages().is_empty());
        assert!(app.streaming_text().is_empty());
        assert_eq!(app.queued_count(), 0);
        assert!(matches!(app.status(), ProcessingStatus::Idle));
        assert!(app.elapsed().is_none());
    }

    #[test]
    fn test_handle_key_typing() {
        let mut app = create_test_app();

        // Type "hello"
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
            .unwrap();

        assert_eq!(app.input(), "hello");
        assert_eq!(app.cursor_pos(), 5);
    }

    #[test]
    fn test_handle_key_backspace() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Backspace, KeyModifiers::empty())
            .unwrap();

        assert_eq!(app.input(), "a");
        assert_eq!(app.cursor_pos(), 1);
    }

    #[test]
    fn test_diagram_focus_toggle_and_pan() {
        let mut app = create_test_app();
        app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        crate::tui::mermaid::clear_active_diagrams();
        crate::tui::mermaid::register_active_diagram(0x1, 100, 80, None);
        crate::tui::mermaid::register_active_diagram(0x2, 120, 90, None);

        // Ctrl+L focuses diagram when available
        app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(app.diagram_focus);

        // Pan should update scroll offsets and not type into input
        app.handle_key(KeyCode::Char('j'), KeyModifiers::empty())
            .unwrap();
        assert_eq!(app.diagram_scroll_y, 3);
        assert!(app.input.is_empty());

        // Ctrl+H returns focus to chat
        app.handle_key(KeyCode::Char('h'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(!app.diagram_focus);

        crate::tui::mermaid::clear_active_diagrams();
    }

    #[test]
    fn test_diagram_cycle_ctrl_arrows() {
        let mut app = create_test_app();
        app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        crate::tui::mermaid::clear_active_diagrams();
        crate::tui::mermaid::register_active_diagram(0x1, 100, 80, None);
        crate::tui::mermaid::register_active_diagram(0x2, 120, 90, None);
        crate::tui::mermaid::register_active_diagram(0x3, 140, 100, None);

        assert_eq!(app.diagram_index, 0);
        app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
            .unwrap();
        assert_eq!(app.diagram_index, 1);
        app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
            .unwrap();
        assert_eq!(app.diagram_index, 2);
        app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
            .unwrap();
        assert_eq!(app.diagram_index, 0);
        app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
            .unwrap();
        assert_eq!(app.diagram_index, 2);

        crate::tui::mermaid::clear_active_diagrams();
    }

    #[test]
    fn test_fuzzy_command_suggestions() {
        let app = create_test_app();
        let suggestions = app.get_suggestions_for("/mdl");
        assert!(suggestions.iter().any(|(cmd, _)| cmd == "/model"));
    }

    fn configure_test_remote_models(app: &mut App) {
        app.is_remote = true;
        app.remote_provider_model = Some("gpt-5.3-codex".to_string());
        app.remote_available_models =
            vec!["gpt-5.3-codex".to_string(), "gpt-5.2-codex".to_string()];
    }

    #[test]
    fn test_model_picker_preview_filter_parsing() {
        assert_eq!(
            App::model_picker_preview_filter("/model"),
            Some(String::new())
        );
        assert_eq!(
            App::model_picker_preview_filter("/model   gpt-5"),
            Some("gpt-5".to_string())
        );
        assert_eq!(
            App::model_picker_preview_filter("   /models codex"),
            Some("codex".to_string())
        );
        assert_eq!(App::model_picker_preview_filter("/modelx"), None);
        assert_eq!(App::model_picker_preview_filter("hello /model"), None);
    }

    #[test]
    fn test_model_picker_preview_stays_open_and_updates_filter() {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);

        for c in "/model g52c".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }

        let picker = app
            .picker_state
            .as_ref()
            .expect("model picker preview should be open");
        assert!(picker.preview);
        assert_eq!(picker.filter, "g52c");
        assert!(picker
            .filtered
            .iter()
            .any(|&i| picker.models[i].name == "gpt-5.2-codex"));
        assert_eq!(app.input(), "/model g52c");
    }

    #[test]
    fn test_model_picker_preview_enter_selects_model() {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);

        for c in "/model g52c".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        // Enter from preview mode selects the model and closes the picker
        assert!(app.picker_state.is_none());
        assert!(app.input().is_empty());
        assert_eq!(app.cursor_pos(), 0);
    }

    #[test]
    fn test_model_picker_preview_arrow_keys_navigate() {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);

        // Type /model to open preview
        for c in "/model".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }

        let picker = app
            .picker_state
            .as_ref()
            .expect("model picker preview should be open");
        assert!(picker.preview);
        let initial_selected = picker.selected;

        // Down arrow should navigate in preview mode
        app.handle_key(KeyCode::Down, KeyModifiers::empty())
            .unwrap();

        let picker = app
            .picker_state
            .as_ref()
            .expect("picker should still be open");
        assert!(picker.preview, "should remain in preview mode");
        assert_eq!(picker.selected, initial_selected + 1);

        // Up arrow should navigate back
        app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();

        let picker = app
            .picker_state
            .as_ref()
            .expect("picker should still be open");
        assert!(picker.preview, "should remain in preview mode");
        assert_eq!(picker.selected, initial_selected);

        // Input should be preserved
        assert_eq!(app.input(), "/model");
    }

    fn configure_test_remote_models_with_copilot(app: &mut App) {
        app.is_remote = true;
        app.remote_provider_model = Some("claude-sonnet-4".to_string());
        app.remote_available_models = vec![
            "claude-sonnet-4-6".to_string(),
            "gpt-5.3-codex".to_string(),
            "claude-opus-4.6".to_string(),
            "gemini-3-pro-preview".to_string(),
            "grok-code-fast-1".to_string(),
        ];
    }

    #[test]
    fn test_model_picker_includes_copilot_models_in_remote_mode() {
        let mut app = create_test_app();
        configure_test_remote_models_with_copilot(&mut app);

        app.open_model_picker();

        let picker = app
            .picker_state
            .as_ref()
            .expect("model picker should be open");

        let model_names: Vec<&str> = picker.models.iter().map(|m| m.name.as_str()).collect();

        assert!(
            model_names.contains(&"claude-opus-4.6"),
            "picker should contain copilot model claude-opus-4.6, got: {:?}",
            model_names
        );
        assert!(
            model_names.contains(&"gemini-3-pro-preview"),
            "picker should contain copilot model gemini-3-pro-preview, got: {:?}",
            model_names
        );
        assert!(
            model_names.contains(&"grok-code-fast-1"),
            "picker should contain copilot model grok-code-fast-1, got: {:?}",
            model_names
        );
    }

    #[test]
    fn test_model_picker_copilot_models_have_copilot_route() {
        let mut app = create_test_app();
        configure_test_remote_models_with_copilot(&mut app);

        app.open_model_picker();

        let picker = app
            .picker_state
            .as_ref()
            .expect("model picker should be open");

        // grok-code-fast-1 is NOT in ALL_CLAUDE_MODELS or ALL_OPENAI_MODELS,
        // so it should get a copilot route
        let grok_entry = picker
            .models
            .iter()
            .find(|m| m.name == "grok-code-fast-1")
            .expect("grok-code-fast-1 should be in picker");

        assert!(
            grok_entry.routes.iter().any(|r| r.api_method == "copilot"),
            "grok-code-fast-1 should have a copilot route, got: {:?}",
            grok_entry.routes
        );
    }

    #[test]
    fn test_model_picker_copilot_selection_prefixes_model() {
        let mut app = create_test_app();
        configure_test_remote_models_with_copilot(&mut app);

        app.open_model_picker();

        let picker = app
            .picker_state
            .as_ref()
            .expect("model picker should be open");

        // Find grok-code-fast-1 (which should only be a copilot route)
        let grok_idx = picker
            .models
            .iter()
            .position(|m| m.name == "grok-code-fast-1")
            .expect("grok-code-fast-1 should be in picker");

        // Navigate to it and select
        let filtered_pos = picker
            .filtered
            .iter()
            .position(|&i| i == grok_idx)
            .expect("grok-code-fast-1 should be in filtered list");

        // Set the selected position to grok's position
        app.picker_state.as_mut().unwrap().selected = filtered_pos;

        // Press Enter to select
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        // In remote mode, selection should produce a pending_model_switch with copilot: prefix
        if let Some(ref spec) = app.pending_model_switch {
            assert!(
                spec.starts_with("copilot:"),
                "copilot model should be prefixed with 'copilot:', got: {}",
                spec
            );
        }
        // Picker should be closed
        assert!(app.picker_state.is_none());
    }

    #[test]
    fn test_handle_key_cursor_movement() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('c'), KeyModifiers::empty())
            .unwrap();

        assert_eq!(app.cursor_pos(), 3);

        app.handle_key(KeyCode::Left, KeyModifiers::empty())
            .unwrap();
        assert_eq!(app.cursor_pos(), 2);

        app.handle_key(KeyCode::Home, KeyModifiers::empty())
            .unwrap();
        assert_eq!(app.cursor_pos(), 0);

        app.handle_key(KeyCode::End, KeyModifiers::empty()).unwrap();
        assert_eq!(app.cursor_pos(), 3);
    }

    #[test]
    fn test_handle_key_escape_clears_input() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
            .unwrap();

        assert_eq!(app.input(), "test");

        app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();

        assert!(app.input().is_empty());
        assert_eq!(app.cursor_pos(), 0);
    }

    #[test]
    fn test_handle_key_ctrl_u_clears_input() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
            .unwrap();

        app.handle_key(KeyCode::Char('u'), KeyModifiers::CONTROL)
            .unwrap();

        assert!(app.input().is_empty());
        assert_eq!(app.cursor_pos(), 0);
    }

    #[test]
    fn test_submit_input_adds_message() {
        let mut app = create_test_app();

        // Type and submit
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
            .unwrap();
        app.submit_input();

        // Check message was added to display
        assert_eq!(app.display_messages().len(), 1);
        assert_eq!(app.display_messages()[0].role, "user");
        assert_eq!(app.display_messages()[0].content, "hi");

        // Check processing state
        assert!(app.is_processing());
        assert!(app.pending_turn);
        assert!(matches!(app.status(), ProcessingStatus::Sending));
        assert!(app.elapsed().is_some());

        // Input should be cleared
        assert!(app.input().is_empty());
    }

    #[test]
    fn test_queue_message_while_processing() {
        let mut app = create_test_app();
        app.queue_mode = true;

        // Simulate processing state
        app.is_processing = true;

        // Type a message
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
            .unwrap();

        // Press Enter should queue, not submit
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        assert_eq!(app.queued_count(), 1);
        assert!(app.input().is_empty());

        // Queued messages are stored in queued_messages, not display_messages
        assert_eq!(app.queued_messages()[0], "test");
        assert!(app.display_messages().is_empty());
    }

    #[test]
    fn test_ctrl_tab_toggles_queue_mode() {
        let mut app = create_test_app();

        assert!(!app.queue_mode);

        app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(app.queue_mode);

        app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(!app.queue_mode);
    }

    #[test]
    fn test_shift_enter_opposite_send_mode() {
        let mut app = create_test_app();
        app.is_processing = true;

        // Default immediate mode: Shift+Enter should queue
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();

        assert_eq!(app.queued_count(), 1);
        assert_eq!(app.interleave_message.as_deref(), None);
        assert!(app.input().is_empty());

        // Queue mode: Shift+Enter should interleave (sets interleave_message, not queued)
        app.queue_mode = true;
        app.handle_key(KeyCode::Char('y'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();

        // Interleave now sets interleave_message instead of adding to queue
        assert_eq!(app.queued_count(), 1); // Still just "hi" in queue
        assert_eq!(app.interleave_message.as_deref(), Some("yo")); // "yo" is for interleave
    }

    #[test]
    fn test_typing_during_processing() {
        let mut app = create_test_app();
        app.is_processing = true;

        // Should still be able to type
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('c'), KeyModifiers::empty())
            .unwrap();

        assert_eq!(app.input(), "abc");
    }

    #[test]
    fn test_ctrl_up_edits_queued_message() {
        let mut app = create_test_app();
        app.queue_mode = true;
        app.is_processing = true;

        // Type and queue a message
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        assert_eq!(app.queued_count(), 1);
        assert!(app.input().is_empty());

        // Press Ctrl+Up to bring it back for editing
        app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();

        assert_eq!(app.queued_count(), 0);
        assert_eq!(app.input(), "hello");
        assert_eq!(app.cursor_pos(), 5); // Cursor at end
    }

    #[test]
    fn test_ctrl_up_prefers_pending_interleave_for_editing() {
        let mut app = create_test_app();
        app.is_processing = true;
        app.queue_mode = false; // Enter=interleave, Shift+Enter=queue

        for c in "urgent".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        for c in "later".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();

        assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
        assert_eq!(app.queued_count(), 1);

        app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();

        assert_eq!(app.input(), "urgent\n\nlater");
        assert_eq!(app.interleave_message.as_deref(), None);
        assert_eq!(app.queued_count(), 0);
    }

    #[test]
    fn test_send_action_modes() {
        let mut app = create_test_app();
        app.is_processing = true;
        app.queue_mode = false;

        assert_eq!(app.send_action(false), SendAction::Interleave);
        assert_eq!(app.send_action(true), SendAction::Queue);

        app.queue_mode = true;
        assert_eq!(app.send_action(false), SendAction::Queue);
        assert_eq!(app.send_action(true), SendAction::Interleave);

        app.is_processing = false;
        assert_eq!(app.send_action(false), SendAction::Submit);
    }

    #[test]
    fn test_streaming_tokens() {
        let mut app = create_test_app();

        assert_eq!(app.streaming_tokens(), (0, 0));

        app.streaming_input_tokens = 100;
        app.streaming_output_tokens = 50;

        assert_eq!(app.streaming_tokens(), (100, 50));
    }

    #[test]
    fn test_processing_status_display() {
        let status = ProcessingStatus::Sending;
        assert!(matches!(status, ProcessingStatus::Sending));

        let status = ProcessingStatus::Streaming;
        assert!(matches!(status, ProcessingStatus::Streaming));

        let status = ProcessingStatus::RunningTool("bash".to_string());
        if let ProcessingStatus::RunningTool(name) = status {
            assert_eq!(name, "bash");
        } else {
            panic!("Expected RunningTool");
        }
    }

    #[test]
    fn test_skill_invocation_not_queued() {
        let mut app = create_test_app();

        // Type a skill command
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
            .unwrap();

        app.submit_input();

        // Should show error for unknown skill, not start processing
        assert!(!app.pending_turn);
        assert!(!app.is_processing);
        // Should have an error message about unknown skill
        assert_eq!(app.display_messages().len(), 1);
        assert_eq!(app.display_messages()[0].role, "error");
    }

    #[test]
    fn test_multiple_queued_messages() {
        let mut app = create_test_app();
        app.is_processing = true;

        // Queue first message
        for c in "first".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();

        // Queue second message
        for c in "second".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();

        // Queue third message
        for c in "third".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();

        assert_eq!(app.queued_count(), 3);
        assert_eq!(app.queued_messages()[0], "first");
        assert_eq!(app.queued_messages()[1], "second");
        assert_eq!(app.queued_messages()[2], "third");
        assert!(app.input().is_empty());
    }

    #[test]
    fn test_queue_message_combines_on_send() {
        let mut app = create_test_app();

        // Queue two messages directly
        app.queued_messages.push("message one".to_string());
        app.queued_messages.push("message two".to_string());

        // Take and combine (simulating what process_queued_messages does)
        let combined = std::mem::take(&mut app.queued_messages).join("\n\n");

        assert_eq!(combined, "message one\n\nmessage two");
        assert!(app.queued_messages.is_empty());
    }

    #[test]
    fn test_interleave_message_separate_from_queue() {
        let mut app = create_test_app();
        app.is_processing = true;
        app.queue_mode = false; // Default mode: Enter=interleave, Shift+Enter=queue

        // Type and submit via Enter (should interleave, not queue)
        for c in "urgent".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        // Should be in interleave_message, not queued
        assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
        assert_eq!(app.queued_count(), 0);

        // Now queue one
        for c in "later".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();

        // Interleave unchanged, one message queued
        assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
        assert_eq!(app.queued_count(), 1);
        assert_eq!(app.queued_messages()[0], "later");
    }

    #[test]
    fn test_handle_paste_single_line() {
        let mut app = create_test_app();

        app.handle_paste("hello world".to_string());

        // Small paste (< 5 lines) is inlined directly
        assert_eq!(app.input(), "hello world");
        assert_eq!(app.cursor_pos(), 11);
        assert!(app.pasted_contents.is_empty()); // No placeholder storage needed
    }

    #[test]
    fn test_handle_paste_multi_line() {
        let mut app = create_test_app();

        app.handle_paste("line 1\nline 2\nline 3".to_string());

        // Small paste (< 5 lines) is inlined directly
        assert_eq!(app.input(), "line 1\nline 2\nline 3");
        assert!(app.pasted_contents.is_empty());
    }

    #[test]
    fn test_handle_paste_large() {
        let mut app = create_test_app();

        app.handle_paste("a\nb\nc\nd\ne".to_string());

        // Large paste (5+ lines) uses placeholder
        assert_eq!(app.input(), "[pasted 5 lines]");
        assert_eq!(app.pasted_contents.len(), 1);
    }

    #[test]
    fn test_paste_expansion_on_submit() {
        let mut app = create_test_app();

        // Type prefix, paste large content, type suffix
        app.handle_key(KeyCode::Char('A'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char(':'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
            .unwrap();
        // Paste 5 lines to trigger placeholder
        app.handle_paste("1\n2\n3\n4\n5".to_string());
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('B'), KeyModifiers::empty())
            .unwrap();

        // Input shows placeholder
        assert_eq!(app.input(), "A: [pasted 5 lines] B");

        // Submit expands placeholder
        app.submit_input();

        // Display shows placeholder (user sees condensed view)
        assert_eq!(app.display_messages().len(), 1);
        assert_eq!(app.display_messages()[0].content, "A: [pasted 5 lines] B");

        // Model receives expanded content (actual pasted text)
        assert_eq!(app.messages.len(), 1);
        match &app.messages[0].content[0] {
            crate::message::ContentBlock::Text { text, .. } => {
                assert_eq!(text, "A: 1\n2\n3\n4\n5 B");
            }
            _ => panic!("Expected Text content block"),
        }

        // Pasted contents should be cleared
        assert!(app.pasted_contents.is_empty());
    }

    #[test]
    fn test_multiple_pastes() {
        let mut app = create_test_app();

        // Small pastes are inlined
        app.handle_paste("first".to_string());
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
            .unwrap();
        app.handle_paste("second\nline".to_string());

        // Both small pastes inlined directly
        assert_eq!(app.input(), "first second\nline");
        assert!(app.pasted_contents.is_empty());

        app.submit_input();
        // Display and model both get the same content (no expansion needed)
        assert_eq!(app.display_messages()[0].content, "first second\nline");
        match &app.messages[0].content[0] {
            crate::message::ContentBlock::Text { text, .. } => {
                assert_eq!(text, "first second\nline");
            }
            _ => panic!("Expected Text content block"),
        }
    }

    #[test]
    fn test_restore_session_adds_reload_message() {
        use crate::session::Session;

        let mut app = create_test_app();

        // Create and save a session with a fake provider_session_id
        let mut session = Session::create(None, None);
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "test message".to_string(),
                cache_control: None,
            }],
        );
        session.provider_session_id = Some("fake-uuid".to_string());
        let session_id = session.id.clone();
        session.save().unwrap();

        // Restore the session
        app.restore_session(&session_id);

        // Should have the original message + reload success message in display
        assert_eq!(app.display_messages().len(), 2);
        assert_eq!(app.display_messages()[0].role, "user");
        assert_eq!(app.display_messages()[0].content, "test message");
        assert_eq!(app.display_messages()[1].role, "system");
        assert!(app.display_messages()[1]
            .content
            .to_lowercase()
            .contains("reloaded"));

        // Messages for API should only have the original message (no reload msg to avoid breaking alternation)
        assert_eq!(app.messages.len(), 1);

        // Provider session ID should be cleared (Claude sessions don't persist across restarts)
        assert!(app.provider_session_id.is_none());

        // Clean up
        let _ = std::fs::remove_file(crate::session::session_path(&session_id).unwrap());
    }

    #[test]
    fn test_recover_session_without_tools_preserves_debug_and_canary_flags() {
        let mut app = create_test_app();
        app.session.is_debug = true;
        app.session.is_canary = true;
        app.session.testing_build = Some("self-dev".to_string());
        app.session.working_dir = Some("/tmp/jcode-test".to_string());
        let old_session_id = app.session.id.clone();

        app.recover_session_without_tools();

        assert_ne!(app.session.id, old_session_id);
        assert_eq!(
            app.session.parent_id.as_deref(),
            Some(old_session_id.as_str())
        );
        assert!(app.session.is_debug);
        assert!(app.session.is_canary);
        assert_eq!(app.session.testing_build.as_deref(), Some("self-dev"));
        assert_eq!(app.session.working_dir.as_deref(), Some("/tmp/jcode-test"));

        let _ = std::fs::remove_file(crate::session::session_path(&app.session.id).unwrap());
    }

    #[test]
    fn test_has_newer_binary_detection() {
        use std::time::{Duration, SystemTime};

        let mut app = create_test_app();
        let exe = crate::build::launcher_binary_path().unwrap();

        let mut created = false;
        if !exe.exists() {
            if let Some(parent) = exe.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&exe, "test").unwrap();
            created = true;
        }

        app.client_binary_mtime = Some(SystemTime::UNIX_EPOCH);
        assert!(app.has_newer_binary());

        app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
        assert!(!app.has_newer_binary());

        if created {
            let _ = std::fs::remove_file(&exe);
        }
    }

    #[test]
    fn test_reload_requests_exit_when_newer_binary() {
        use std::time::{Duration, SystemTime};

        let mut app = create_test_app();
        let exe = crate::build::launcher_binary_path().unwrap();

        let mut created = false;
        if !exe.exists() {
            if let Some(parent) = exe.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&exe, "test").unwrap();
            created = true;
        }

        app.client_binary_mtime = Some(SystemTime::UNIX_EPOCH);
        app.input = "/reload".to_string();
        app.submit_input();

        assert!(app.reload_requested.is_some());
        assert!(app.should_quit);

        // Ensure the "no newer binary" path is exercised too.
        app.reload_requested = None;
        app.should_quit = false;
        app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
        app.input = "/reload".to_string();
        app.submit_input();
        assert!(app.reload_requested.is_none());
        assert!(!app.should_quit);

        if created {
            let _ = std::fs::remove_file(&exe);
        }
    }

    #[test]
    fn test_reload_progress_coalesces_into_single_message() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::Reloading { new_socket: None },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::ReloadProgress {
                step: "init".to_string(),
                message: "🔄 Starting hot-reload...".to_string(),
                success: None,
                output: None,
            },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::ReloadProgress {
                step: "verify".to_string(),
                message: "Binary verified".to_string(),
                success: Some(true),
                output: Some("size=68.4MB".to_string()),
            },
            &mut remote,
        );

        assert_eq!(app.display_messages().len(), 1);
        let reload_msg = &app.display_messages()[0];
        assert_eq!(reload_msg.role, "system");
        assert_eq!(reload_msg.title.as_deref(), Some("Reload"));
        assert_eq!(
            reload_msg.content,
            "🔄 Server reload initiated...\n[init] 🔄 Starting hot-reload...\n[verify] ✓ Binary verified\n```\nsize=68.4MB\n```"
        );
    }

    #[test]
    fn test_handle_server_event_updates_connection_type() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::ConnectionType {
                connection: "websocket".to_string(),
            },
            &mut remote,
        );

        assert_eq!(app.connection_type.as_deref(), Some("websocket"));
    }

    #[test]
    fn test_handle_server_event_interrupted_clears_stream_state_and_sets_idle() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.processing_started = Some(Instant::now());
        app.current_message_id = Some(42);
        app.streaming_text = "partial".to_string();
        app.streaming_tool_calls.push(crate::message::ToolCall {
            id: "tool_1".to_string(),
            name: "bash".to_string(),
            input: serde_json::Value::Null,
            intent: None,
        });
        app.interleave_message = Some("queued interrupt".to_string());
        app.pending_soft_interrupts
            .push("pending soft interrupt".to_string());

        remote.handle_tool_start("tool_1", "bash");
        remote.handle_tool_input("{\"command\":\"sleep 10\"}");
        remote.handle_tool_exec("tool_1", "edit");

        app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

        assert!(!app.is_processing);
        assert!(matches!(app.status, ProcessingStatus::Idle));
        assert!(app.processing_started.is_none());
        assert!(app.current_message_id.is_none());
        assert!(app.streaming_text.is_empty());
        assert!(app.streaming_tool_calls.is_empty());
        assert!(app.interleave_message.is_none());
        assert!(app.pending_soft_interrupts.is_empty());

        let last = app
            .display_messages()
            .last()
            .expect("missing interrupted message");
        assert_eq!(last.role, "system");
        assert_eq!(last.content, "Interrupted");
    }

    #[test]
    fn test_remote_error_with_retry_after_keeps_pending_for_auto_retry() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "retry me".to_string(),
            images: vec![],
            is_system: false,
        });
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(9);

        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 9,
                message: "rate limited".to_string(),
                retry_after_secs: Some(3),
            },
            &mut remote,
        );

        assert!(!app.is_processing);
        assert!(matches!(app.status, ProcessingStatus::Idle));
        assert!(app.current_message_id.is_none());
        assert!(app.rate_limit_reset.is_some());
        assert!(app.rate_limit_pending_message.is_some());

        let last = app
            .display_messages()
            .last()
            .expect("missing rate-limit status message");
        assert_eq!(last.role, "system");
        assert!(last.content.contains("Will auto-retry in 3 seconds"));
    }

    #[test]
    fn test_remote_error_without_retry_clears_pending() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "retry me".to_string(),
            images: vec![],
            is_system: false,
        });

        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 10,
                message: "provider failed hard".to_string(),
                retry_after_secs: None,
            },
            &mut remote,
        );

        assert!(app.rate_limit_pending_message.is_none());
        let last = app
            .display_messages()
            .last()
            .expect("missing error message");
        assert_eq!(last.role, "error");
        assert_eq!(last.content, "provider failed hard");
    }

    #[test]
    fn test_info_widget_data_includes_connection_type() {
        let mut app = create_test_app();
        app.connection_type = Some("https".to_string());
        let data = crate::tui::TuiState::info_widget_data(&app);
        assert_eq!(data.connection_type.as_deref(), Some("https"));
    }

    #[test]
    fn test_debug_command_message_respects_queue_mode() {
        let mut app = create_test_app();

        // Test 1: When not processing, should submit directly
        app.is_processing = false;
        let result = app.handle_debug_command("message:hello");
        assert!(
            result.starts_with("OK: submitted message"),
            "Expected submitted, got: {}",
            result
        );
        // The message should be processed (added to messages and pending_turn set)
        assert!(app.pending_turn);
        assert_eq!(app.messages.len(), 1);

        // Reset for next test
        app.pending_turn = false;
        app.messages.clear();

        // Test 2: When processing with queue_mode=true, should queue
        app.is_processing = true;
        app.queue_mode = true;
        let result = app.handle_debug_command("message:queued_msg");
        assert!(
            result.contains("queued"),
            "Expected queued, got: {}",
            result
        );
        assert_eq!(app.queued_count(), 1);
        assert_eq!(app.queued_messages()[0], "queued_msg");

        // Test 3: When processing with queue_mode=false, should interleave
        app.queued_messages.clear();
        app.queue_mode = false;
        let result = app.handle_debug_command("message:interleave_msg");
        assert!(
            result.contains("interleave"),
            "Expected interleave, got: {}",
            result
        );
        assert_eq!(app.interleave_message.as_deref(), Some("interleave_msg"));
    }

    // ====================================================================
    // Scroll testing with rendering verification
    // ====================================================================

    /// Extract plain text from a TestBackend buffer after rendering.
    fn buffer_to_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let width = buf.area.width as usize;
        let height = buf.area.height as usize;
        let mut lines = Vec::with_capacity(height);
        for y in 0..height {
            let mut line = String::with_capacity(width);
            for x in 0..width {
                let cell = &buf[(x as u16, y as u16)];
                line.push_str(cell.symbol());
            }
            lines.push(line.trim_end().to_string());
        }
        // Trim trailing empty lines
        while lines.last().map_or(false, |l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Create a test app pre-populated with scrollable content (text + mermaid diagrams).
    fn create_scroll_test_app(
        width: u16,
        height: u16,
        diagrams: usize,
        padding: usize,
    ) -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
        let mut app = create_test_app();
        let content = App::build_scroll_test_content(diagrams, padding, None);
        app.display_messages = vec![
            DisplayMessage {
                role: "user".to_string(),
                content: "Scroll test".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
            DisplayMessage {
                role: "assistant".to_string(),
                content,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
        ];
        app.bump_display_messages_version();
        app.scroll_offset = 0;
        app.auto_scroll_paused = false;
        app.is_processing = false;
        app.streaming_text.clear();
        app.status = ProcessingStatus::Idle;
        // Set deterministic session name for snapshot stability
        app.session.short_name = Some("test".to_string());

        let backend = ratatui::backend::TestBackend::new(width, height);
        let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
        (app, terminal)
    }

    /// Get the configured scroll up key binding (code, modifiers).
    fn scroll_up_key(app: &App) -> (KeyCode, KeyModifiers) {
        (
            app.scroll_keys.up.code.clone(),
            app.scroll_keys.up.modifiers,
        )
    }

    /// Get the configured scroll down key binding (code, modifiers).
    fn scroll_down_key(app: &App) -> (KeyCode, KeyModifiers) {
        (
            app.scroll_keys.down.code.clone(),
            app.scroll_keys.down.modifiers,
        )
    }

    /// Get the configured scroll up fallback key, or primary scroll up key.
    fn scroll_up_fallback_key(app: &App) -> (KeyCode, KeyModifiers) {
        app.scroll_keys
            .up_fallback
            .as_ref()
            .map(|binding| (binding.code.clone(), binding.modifiers))
            .unwrap_or_else(|| scroll_up_key(app))
    }

    /// Get the configured scroll down fallback key, or primary scroll down key.
    fn scroll_down_fallback_key(app: &App) -> (KeyCode, KeyModifiers) {
        app.scroll_keys
            .down_fallback
            .as_ref()
            .map(|binding| (binding.code.clone(), binding.modifiers))
            .unwrap_or_else(|| scroll_down_key(app))
    }

    /// Get the configured prompt-up key binding (code, modifiers).
    fn prompt_up_key(app: &App) -> (KeyCode, KeyModifiers) {
        (
            app.scroll_keys.prompt_up.code.clone(),
            app.scroll_keys.prompt_up.modifiers,
        )
    }

    /// Render app to TestBackend and return the buffer text.
    fn render_and_snap(
        app: &App,
        terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    ) -> String {
        terminal
            .draw(|f| crate::tui::ui::draw(f, app))
            .expect("draw failed");
        buffer_to_text(terminal)
    }

    #[test]
    fn test_streaming_repaint_does_not_leave_bracket_artifact() {
        let mut app = create_test_app();
        let backend = ratatui::backend::TestBackend::new(90, 20);
        let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.streaming_text = "[".to_string();
        let _ = render_and_snap(&app, &mut terminal);

        app.streaming_text = "Process A: |██████████|".to_string();
        let text = render_and_snap(&app, &mut terminal);

        assert!(
            text.contains("Process A: |██████████|"),
            "expected updated streaming content to be visible"
        );
        assert!(
            !text.lines().any(|line| line.trim() == "["),
            "stale standalone '[' artifact should not persist after repaint"
        );
    }

    #[test]
    fn test_remote_typing_resumes_bottom_follow_mode() {
        let mut app = create_test_app();
        app.scroll_offset = 7;
        app.auto_scroll_paused = true;

        app.handle_remote_char_input('x');

        assert_eq!(app.input, "x");
        assert_eq!(app.cursor_pos, 1);
        assert_eq!(app.scroll_offset, 0);
        assert!(
            !app.auto_scroll_paused,
            "typing in remote mode should follow newest content, not pin top"
        );
    }

    #[test]
    fn test_reconnect_target_prefers_remote_session_id() {
        let mut app = create_test_app();
        app.resume_session_id = Some("ses_resume_idle".to_string());
        app.remote_session_id = Some("ses_remote_active".to_string());

        assert_eq!(
            app.reconnect_target_session_id().as_deref(),
            Some("ses_remote_active")
        );
    }

    #[test]
    fn test_reconnect_target_uses_resume_when_remote_missing() {
        let mut app = create_test_app();
        app.resume_session_id = Some("ses_resume_only".to_string());
        app.remote_session_id = None;

        assert_eq!(
            app.reconnect_target_session_id().as_deref(),
            Some("ses_resume_only")
        );
    }

    #[test]
    fn test_reconnect_target_does_not_consume_resume_session_id() {
        let mut app = create_test_app();
        app.resume_session_id = Some("ses_resume_persistent".to_string());
        app.remote_session_id = None;

        let first = app.reconnect_target_session_id();
        let second = app.reconnect_target_session_id();

        assert_eq!(first.as_deref(), Some("ses_resume_persistent"));
        assert_eq!(second.as_deref(), Some("ses_resume_persistent"));
        assert_eq!(
            app.resume_session_id.as_deref(),
            Some("ses_resume_persistent")
        );
    }

    #[test]
    fn test_prompt_jump_ctrl_brackets() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

        // Seed max scroll estimates before key handling.
        render_and_snap(&app, &mut terminal);

        assert_eq!(app.scroll_offset, 0);
        assert!(!app.auto_scroll_paused);

        app.handle_key(KeyCode::Char('['), KeyModifiers::CONTROL)
            .unwrap();
        assert!(app.auto_scroll_paused);
        assert!(app.scroll_offset > 0);

        let after_up = app.scroll_offset;
        app.handle_key(KeyCode::Char(']'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(app.scroll_offset <= after_up);
    }

    #[test]
    fn test_prompt_jump_ctrl_digits_by_recency() {
        let mut app = create_test_app();
        app.display_messages = vec![
            DisplayMessage::user("first".to_string()),
            DisplayMessage::assistant("a".to_string()),
            DisplayMessage::user("second".to_string()),
            DisplayMessage::assistant("b".to_string()),
            DisplayMessage::user("third".to_string()),
            DisplayMessage::assistant("c".to_string()),
            DisplayMessage::user("fourth".to_string()),
        ];
        app.bump_display_messages_version();
        app.follow_chat_bottom();

        app.handle_key(KeyCode::Char('1'), KeyModifiers::CONTROL)
            .unwrap();
        let first_jump = app.scroll_offset;
        assert!(app.auto_scroll_paused);
        assert!(first_jump > 0);

        app.follow_chat_bottom();
        app.handle_key(KeyCode::Char('2'), KeyModifiers::CONTROL)
            .unwrap();
        let second_jump = app.scroll_offset;
        assert!(second_jump > first_jump);

        app.follow_chat_bottom();
        app.handle_key(KeyCode::Char('3'), KeyModifiers::CONTROL)
            .unwrap();
        let third_jump = app.scroll_offset;
        assert!(third_jump > second_jump);

        app.follow_chat_bottom();
        app.handle_key(KeyCode::Char('4'), KeyModifiers::CONTROL)
            .unwrap();
        let fourth_jump = app.scroll_offset;
        assert!(fourth_jump > third_jump);

        app.follow_chat_bottom();
        app.handle_key(KeyCode::Char('9'), KeyModifiers::CONTROL)
            .unwrap();
        assert_eq!(app.scroll_offset, fourth_jump);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_prompt_jump_ctrl_esc_fallback_on_macos() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

        render_and_snap(&app, &mut terminal);

        assert_eq!(app.scroll_offset, 0);
        app.handle_key(KeyCode::Esc, KeyModifiers::CONTROL).unwrap();
        assert!(app.auto_scroll_paused);
        assert!(app.scroll_offset > 0);
    }

    #[test]
    fn test_prompt_jump_ctrl_digit_is_recency_rank_in_app() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

        // Seed max scroll estimates before key handling.
        render_and_snap(&app, &mut terminal);

        let (prompt_up_code, prompt_up_mods) = prompt_up_key(&app);
        app.handle_key(prompt_up_code, prompt_up_mods).unwrap();
        assert!(app.scroll_offset > 0);

        // Ctrl+5 now means "5th most-recent prompt" (clamped to oldest).
        app.handle_key(KeyCode::Char('5'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(app.scroll_offset > 0);
    }

    #[test]
    fn test_scroll_cmd_j_k_fallback_in_app() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

        // Seed max scroll estimates before key handling.
        render_and_snap(&app, &mut terminal);

        let (up_code, up_mods) = scroll_up_fallback_key(&app);
        let (down_code, down_mods) = scroll_down_fallback_key(&app);

        app.handle_key(up_code, up_mods).unwrap();
        assert!(app.auto_scroll_paused);
        assert!(app.scroll_offset > 0);
        let after_up = app.scroll_offset;

        app.handle_key(down_code, down_mods).unwrap();
        assert!(app.scroll_offset <= after_up);
    }

    #[test]
    fn test_remote_prompt_jump_ctrl_brackets() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Seed max scroll estimates before key handling.
        render_and_snap(&app, &mut terminal);

        assert_eq!(app.scroll_offset, 0);
        assert!(!app.auto_scroll_paused);

        rt.block_on(app.handle_remote_key(KeyCode::Char('['), KeyModifiers::CONTROL, &mut remote))
            .unwrap();
        assert!(app.auto_scroll_paused);
        assert!(app.scroll_offset > 0);

        let after_up = app.scroll_offset;
        rt.block_on(app.handle_remote_key(KeyCode::Char(']'), KeyModifiers::CONTROL, &mut remote))
            .unwrap();
        assert!(app.scroll_offset <= after_up);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_remote_prompt_jump_ctrl_esc_fallback_on_macos() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Seed max scroll estimates before key handling.
        render_and_snap(&app, &mut terminal);

        assert_eq!(app.scroll_offset, 0);
        rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::CONTROL, &mut remote))
            .unwrap();
        assert!(app.auto_scroll_paused);
        assert!(app.scroll_offset > 0);
    }

    #[test]
    fn test_remote_prompt_jump_ctrl_digit_is_recency_rank() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Seed max scroll estimates before key handling.
        render_and_snap(&app, &mut terminal);

        let (prompt_up_code, prompt_up_mods) = prompt_up_key(&app);
        rt.block_on(app.handle_remote_key(prompt_up_code, prompt_up_mods, &mut remote))
            .unwrap();
        assert!(app.scroll_offset > 0);

        // Ctrl+5 now means "5th most-recent prompt" (clamped to oldest).
        rt.block_on(app.handle_remote_key(KeyCode::Char('5'), KeyModifiers::CONTROL, &mut remote))
            .unwrap();
        assert!(app.scroll_offset > 0);
    }

    #[test]
    fn test_remote_scroll_cmd_j_k_fallback() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        // Seed max scroll estimates before key handling.
        render_and_snap(&app, &mut terminal);

        let (up_code, up_mods) = scroll_up_fallback_key(&app);
        let (down_code, down_mods) = scroll_down_fallback_key(&app);

        rt.block_on(app.handle_remote_key(up_code, up_mods, &mut remote))
            .unwrap();
        assert!(app.auto_scroll_paused);
        assert!(app.scroll_offset > 0);
        let after_up = app.scroll_offset;

        rt.block_on(app.handle_remote_key(down_code, down_mods, &mut remote))
            .unwrap();
        assert!(app.scroll_offset <= after_up);
    }

    #[test]
    fn test_scroll_ctrl_k_j_offset() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

        assert_eq!(app.scroll_offset, 0);
        assert!(!app.auto_scroll_paused);

        let (up_code, up_mods) = scroll_up_key(&app);
        let (down_code, down_mods) = scroll_down_key(&app);

        // Render first so LAST_MAX_SCROLL is populated
        render_and_snap(&app, &mut terminal);

        // Scroll up (switches to absolute-from-top mode)
        app.handle_key(up_code.clone(), up_mods).unwrap();
        assert!(app.auto_scroll_paused);
        let first_offset = app.scroll_offset;

        app.handle_key(up_code.clone(), up_mods).unwrap();
        let second_offset = app.scroll_offset;
        assert!(
            second_offset < first_offset,
            "scrolling up should decrease absolute offset (move toward top)"
        );

        // Scroll down (increases absolute position = moves toward bottom)
        app.handle_key(down_code.clone(), down_mods).unwrap();
        assert_eq!(
            app.scroll_offset, first_offset,
            "one scroll down should undo one scroll up"
        );

        // Keep scrolling down until back at bottom
        for _ in 0..10 {
            app.handle_key(down_code.clone(), down_mods).unwrap();
            if !app.auto_scroll_paused {
                break;
            }
        }
        assert_eq!(app.scroll_offset, 0);
        assert!(!app.auto_scroll_paused);

        // Stays at 0 when already at bottom
        app.handle_key(down_code.clone(), down_mods).unwrap();
        assert_eq!(app.scroll_offset, 0);
    }

    #[test]
    fn test_scroll_offset_capped() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 4);

        let (up_code, up_mods) = scroll_up_key(&app);

        // Render first so LAST_MAX_SCROLL is populated
        render_and_snap(&app, &mut terminal);

        // Spam scroll-up many times
        for _ in 0..500 {
            app.handle_key(up_code.clone(), up_mods).unwrap();
        }

        // Should be at 0 (absolute top) after scrolling up enough
        assert_eq!(app.scroll_offset, 0);
        assert!(app.auto_scroll_paused);
    }

    #[test]
    fn test_scroll_render_bottom() {
        let (app, mut terminal) = create_scroll_test_app(80, 25, 1, 8);
        let text = render_and_snap(&app, &mut terminal);

        // At bottom (scroll_offset=0), content and diagram box should be visible
        assert!(
            text.contains("diagram"),
            "expected diagram content at bottom position"
        );
        assert!(
            text.contains("stretch content"),
            "expected filler content at bottom position"
        );
        // Should have ↑ indicator since content extends above viewport
        assert!(
            text.contains('↑'),
            "expected ↑ indicator when content extends above viewport"
        );
    }

    #[test]
    fn test_scroll_render_scrolled_up() {
        let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 8);
        app.scroll_offset = 10;
        app.auto_scroll_paused = true;
        let text = render_and_snap(&app, &mut terminal);

        // ↓ indicator should appear when user has scrolled up
        assert!(
            text.contains('↓'),
            "expected ↓ indicator when scrolled up from bottom"
        );
    }

    #[test]
    fn test_scroll_top_does_not_snap_to_bottom() {
        let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);

        // Top position in paused mode (absolute offset from top).
        app.scroll_offset = 0;
        app.auto_scroll_paused = true;
        let text_top = render_and_snap(&app, &mut terminal);

        // Bottom position (auto-follow mode).
        app.scroll_offset = 0;
        app.auto_scroll_paused = false;
        let text_bottom = render_and_snap(&app, &mut terminal);

        assert_ne!(
            text_top, text_bottom,
            "top viewport should differ from bottom viewport"
        );
        assert!(
            text_top.contains("Intro line 01"),
            "top viewport should include earliest content"
        );
    }

    #[test]
    fn test_scroll_content_shifts() {
        let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);

        // Render at bottom
        app.scroll_offset = 0;
        app.auto_scroll_paused = false;
        let text_bottom = render_and_snap(&app, &mut terminal);

        // Render scrolled up (absolute line 10 from top)
        app.scroll_offset = 10;
        app.auto_scroll_paused = true;
        let text_scrolled = render_and_snap(&app, &mut terminal);

        assert_ne!(
            text_bottom, text_scrolled,
            "content should change when scrolled"
        );
    }

    #[test]
    fn test_scroll_render_with_mermaid() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 2, 10);

        // Render at several positions without crashing
        for offset in [0, 5, 10, 20, 50] {
            app.scroll_offset = offset;
            app.auto_scroll_paused = offset > 0;
            terminal
                .draw(|f| crate::tui::ui::draw(f, &app))
                .unwrap_or_else(|e| panic!("draw failed at scroll_offset={}: {}", offset, e));
        }

        // Verify at bottom
        app.scroll_offset = 0;
        app.auto_scroll_paused = false;
        let text_bottom = render_and_snap(&app, &mut terminal);
        assert!(
            text_bottom.contains("diagram"),
            "mermaid: expected diagram content at bottom"
        );

        // Verify explicit top viewport in paused mode differs from bottom follow mode.
        app.scroll_offset = 0;
        app.auto_scroll_paused = true;
        let text_scrolled = render_and_snap(&app, &mut terminal);
        assert_ne!(
            text_bottom, text_scrolled,
            "mermaid: scrolled view should differ from bottom"
        );
        assert!(
            text_scrolled.contains("Intro line 01"),
            "mermaid: top viewport should include earliest content"
        );
    }

    #[test]
    fn test_scroll_visual_debug_frame() {
        let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 10);

        crate::tui::visual_debug::enable();

        // Render at bottom, verify frame capture works
        app.scroll_offset = 0;
        terminal
            .draw(|f| crate::tui::ui::draw(f, &app))
            .expect("draw at offset=0 failed");

        let frame = crate::tui::visual_debug::latest_frame();
        assert!(frame.is_some(), "visual debug frame should be captured");

        // Render at scroll_offset=10, verify no panic
        app.scroll_offset = 10;
        app.auto_scroll_paused = true;
        terminal
            .draw(|f| crate::tui::ui::draw(f, &app))
            .expect("draw at offset=10 failed");

        // Note: latest_frame() is global and may be overwritten by parallel tests,
        // so we only verify the frame capture mechanism works, not exact values.
        let frame = crate::tui::visual_debug::latest_frame();
        assert!(
            frame.is_some(),
            "frame should still be available after second draw"
        );

        crate::tui::visual_debug::disable();
    }

    #[test]
    fn test_scroll_key_then_render() {
        let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 40);

        // Render at bottom first (populates LAST_MAX_SCROLL)
        let _text_before = render_and_snap(&app, &mut terminal);

        let (up_code, up_mods) = scroll_up_key(&app);

        // Scroll up three times (9 lines total)
        for _ in 0..3 {
            app.handle_key(up_code.clone(), up_mods).unwrap();
        }
        assert!(app.auto_scroll_paused);
        assert!(app.scroll_offset > 0);

        // Render again - verifies scroll_offset produces a valid frame without panic.
        // Note: LAST_MAX_SCROLL is a process-wide global that parallel tests
        // can overwrite at any time, so we only check that rendering succeeds
        // and that scroll state is correct - not that the rendered text differs,
        // since the global can clamp scroll_offset to 0 during render.
        let _text_after = render_and_snap(&app, &mut terminal);
    }

    #[test]
    fn test_scroll_round_trip() {
        let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);

        let (up_code, up_mods) = scroll_up_key(&app);
        let (down_code, down_mods) = scroll_down_key(&app);

        // Render at bottom before scrolling (populates LAST_MAX_SCROLL)
        let text_original = render_and_snap(&app, &mut terminal);

        // Scroll up 3x
        for _ in 0..3 {
            app.handle_key(up_code.clone(), up_mods).unwrap();
        }
        assert!(app.auto_scroll_paused);

        // Verify content shifted
        let text_scrolled = render_and_snap(&app, &mut terminal);
        assert_ne!(text_original, text_scrolled, "scrolled view should differ");

        // Scroll back down until at bottom
        for _ in 0..20 {
            app.handle_key(down_code.clone(), down_mods).unwrap();
            if !app.auto_scroll_paused {
                break;
            }
        }
        assert_eq!(
            app.scroll_offset, 0,
            "scroll_offset should return to 0 after round-trip"
        );
        assert!(!app.auto_scroll_paused);

        // Verify we're back at the bottom (status bar / input prompt visible)
        let text_restored = render_and_snap(&app, &mut terminal);
        assert!(
            text_restored.contains("diagram"),
            "restored view should show diagram content at bottom"
        );
    }
}
