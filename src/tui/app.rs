#![allow(dead_code)]

use super::keybind::{CenteredToggleKeys, ModelSwitchKeys, ScrollKeys};
use super::markdown::IncrementalMarkdownRenderer;
use super::stream_buffer::StreamBuffer;
use crate::bus::{Bus, BusEvent, LoginCompleted, ToolEvent, ToolStatus};
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
use debug::DebugTrace;
use helpers::*;
use ratatui::DefaultTerminal;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;

mod commands;
mod debug;
mod helpers;
mod input;
mod local;
mod navigation;
mod picker;
mod remote;
mod replay;

#[derive(Debug, Clone)]
struct PendingRemoteMessage {
    content: String,
    images: Vec<(String, String)>,
    is_system: bool,
    auto_retry: bool,
    retry_attempts: u8,
    retry_at: Option<Instant>,
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
    /// The session ID that was active (for resume hints on exit)
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendAction {
    Submit,
    Queue,
    Interleave,
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
    /// Waiting for user to paste an OpenAI OAuth callback URL/query.
    OpenAi {
        verifier: String,
        expected_state: String,
        redirect_uri: String,
    },
    /// Waiting for user to paste an API key for an OpenAI-compatible provider.
    ApiKeyProfile {
        provider: String,
        docs_url: String,
        env_file: String,
        key_name: String,
        default_model: Option<String>,
    },
    /// Waiting for user to paste a Cursor API key.
    CursorApiKey,
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
    // When the last API response completed (for cache TTL tracking)
    last_api_completed: Option<Instant>,
    // Input tokens from the last completed turn (for cache TTL display)
    last_turn_input_tokens: Option<u64>,
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
    remote_reasoning_effort: Option<String>,
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
    // Whether the user is dragging the diagram pane border
    diagram_pane_dragging: bool,
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
    // Stashed input: saved via Ctrl+S for later retrieval
    stashed_input: Option<(String, usize)>,
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
    /// Last mouse scroll event timestamp (for trackpad velocity detection)
    last_mouse_scroll: Option<Instant>,
    /// Scroll offset for changelog overlay (None = not visible)
    changelog_scroll: Option<usize>,
    help_scroll: Option<usize>,
    /// Session picker overlay (None = not visible)
    session_picker_overlay: Option<RefCell<super::session_picker::SessionPicker>>,
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

    async fn begin_remote_send(
        &mut self,
        remote: &mut super::backend::RemoteConnection,
        content: String,
        images: Vec<(String, String)>,
        is_system: bool,
    ) -> Result<u64> {
        remote::begin_remote_send(self, remote, content, images, is_system, false, 0).await
    }

    fn schedule_pending_remote_retry(&mut self, reason: &str) -> bool {
        let Some(pending) = self.rate_limit_pending_message.as_mut() else {
            return false;
        };
        if !pending.auto_retry {
            return false;
        }
        let outcome = {
            let current_attempts = pending.retry_attempts;
            if current_attempts >= Self::AUTO_RETRY_MAX_ATTEMPTS {
                Err(current_attempts)
            } else {
                pending.retry_attempts += 1;
                let retry_attempts = pending.retry_attempts;
                let backoff_secs = Self::AUTO_RETRY_BASE_DELAY_SECS * u64::from(retry_attempts);
                let retry_at = Instant::now() + Duration::from_secs(backoff_secs);
                pending.retry_at = Some(retry_at);
                Ok((retry_attempts, backoff_secs, retry_at))
            }
        };

        match outcome {
            Err(current_attempts) => {
                self.rate_limit_pending_message = None;
                self.rate_limit_reset = None;
                self.push_display_message(DisplayMessage::error(format!(
                    "{} Auto-retry limit reached after {} attempt{}. Use `/poke` again to retry manually.",
                    reason,
                    current_attempts,
                    if current_attempts == 1 { "" } else { "s" }
                )));
                false
            }
            Ok((retry_attempts, backoff_secs, retry_at)) => {
                self.rate_limit_reset = Some(retry_at);
                self.push_display_message(DisplayMessage::system(format!(
                    "{} Auto-retrying in {} second{} (attempt {}/{}).",
                    reason,
                    backoff_secs,
                    if backoff_secs == 1 { "" } else { "s" },
                    retry_attempts,
                    Self::AUTO_RETRY_MAX_ATTEMPTS
                )));
                true
            }
        }
    }

    fn clear_pending_remote_retry(&mut self) {
        self.rate_limit_pending_message = None;
        self.rate_limit_reset = None;
    }

    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let t0 = std::time::Instant::now();
        let skills = SkillRegistry::load().unwrap_or_default();
        let t_skills = t0.elapsed();
        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
        let mut session = Session::create(None, None);
        session.model = Some(provider.model());
        let display = config().display.clone();
        let features = config().features.clone();
        let context_limit = provider.context_window() as u64;
        let t_session = t0.elapsed();

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
        let t_prompt = t0.elapsed();
        crate::logging::info(&format!(
            "App::new timings: skills={:.1}ms session={:.1}ms prompt={:.1}ms total={:.1}ms",
            t_skills.as_secs_f64() * 1000.0,
            (t_session - t_skills).as_secs_f64() * 1000.0,
            (t_prompt - t_session).as_secs_f64() * 1000.0,
            t_prompt.as_secs_f64() * 1000.0,
        ));

        let mut app = Self {
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
            last_api_completed: None,
            last_turn_input_tokens: None,
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
            remote_reasoning_effort: None,
            remote_available_models: Vec::new(),
            remote_model_routes: Vec::new(),
            remote_mcp_servers: Vec::new(),
            remote_skills: Vec::new(),
            remote_total_tokens: None,
            remote_is_canary: None,
            remote_server_version: None,
            remote_server_has_update: None,
            pending_server_reload: false,
            remote_server_short_name: None,
            remote_server_icon: None,
            current_message_id: None,
            is_remote: false,
            server_spawning: false,
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
            diagram_pane_dragging: false,
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
            stashed_input: None,
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
            last_mouse_scroll: None,
            changelog_scroll: None,
            help_scroll: None,
            session_picker_overlay: None,
        };

        for notice in app.provider.drain_startup_notices() {
            app.status_notice = Some((notice, Instant::now()));
        }

        app
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
            if let Some((input, cursor)) = Self::restore_input_from_reload(session_id) {
                app.input = input;
                app.cursor_pos = cursor;
            }
        }

        app.resume_session_id = resume_session;
        app
    }

    /// Mark that a server was just spawned - run_remote will retry initial connection
    /// instead of failing fatally, allowing the TUI to show while the server starts.
    pub fn set_server_spawning(&mut self) {
        self.server_spawning = true;
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
            self.remote_is_canary.unwrap_or(self.session.is_canary)
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
            self.remote_is_canary.unwrap_or(self.session.is_canary)
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
        if let Some((input, cursor)) = Self::restore_input_from_reload(session_id) {
            self.input = input;
            self.cursor_pos = cursor;
        }
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
            let reload_ctx = ReloadContext::load_for_session(session_id).ok().flatten();
            if let Some(ctx) = reload_ctx {
                // This session initiated the reload - send the reload-specific continuation
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
            } else if self.was_interrupted_by_reload() {
                // This session was interrupted by another session's reload.
                // The conversation has incomplete tool results - auto-continue.
                crate::logging::info(
                    "Session was interrupted by reload (not initiator), queuing continuation",
                );
                self.push_display_message(DisplayMessage::system(
                    "⚡ Session was interrupted by server reload. Continuing...".to_string(),
                ));
                self.queued_messages.push(
                    "[SYSTEM: Your session was interrupted by a server reload while a tool was running. \
                     The tool was aborted and its results may be incomplete. \
                     Please continue exactly where you left off. Look at the conversation history \
                     to understand what you were doing and resume immediately. \
                     Do NOT ask the user what to do - just continue your work.]"
                        .to_string(),
                );
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

    /// Check if the current session was interrupted by a server reload.
    /// Detects two patterns:
    /// 1. Last message is a User ToolResult containing reload interruption text
    /// 2. Last assistant message ends with "[generation interrupted - server reloading]"
    fn was_interrupted_by_reload(&self) -> bool {
        use crate::message::{ContentBlock, Role};
        let messages = &self.session.messages;
        if messages.is_empty() {
            return false;
        }
        let last = &messages[messages.len() - 1];
        match last.role {
            Role::User => last.content.iter().any(|block| match block {
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    is_error.unwrap_or(false)
                        && (content.contains("interrupted by server reload")
                            || content.contains("Skipped - server reloading"))
                }
                _ => false,
            }),
            Role::Assistant => last.content.iter().any(|block| match block {
                ContentBlock::Text { text, .. } => {
                    text.ends_with("[generation interrupted - server reloading]")
                }
                _ => false,
            }),
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
                        local::handle_tick(&mut self);
                    }
                    event = event_stream.next() => {
                        local::handle_terminal_event(&mut self, &mut terminal, event)?;
                    }
                    // Handle background task completion notifications
                    bus_event = bus_receiver.recv() => {
                        local::handle_bus_event(&mut self, bus_event);
                    }
                }
            }
        }

        self.extract_session_memories().await;

        Ok(RunResult {
            reload_session: self.reload_requested.take(),
            rebuild_session: self.rebuild_requested.take(),
            update_session: self.update_requested.take(),
            exit_code: self.requested_exit_code,
            session_id: Some(self.session.id.clone()),
        })
    }

    /// Run the TUI in remote mode, connecting to a server
    pub async fn run_remote(mut self, mut terminal: DefaultTerminal) -> Result<RunResult> {
        let mut event_stream = EventStream::new();
        let mut redraw_period = super::redraw_interval(&self);
        let mut redraw_interval = interval(redraw_period);
        let mut remote_state = remote::RemoteRunState::default();

        'outer: loop {
            let session_to_resume = self.reconnect_target_session_id();

            let mut remote = match remote::connect_with_retry(
                &mut self,
                &mut terminal,
                &mut event_stream,
                &mut remote_state,
                session_to_resume.as_deref(),
            )
            .await?
            {
                remote::ConnectOutcome::Connected(remote) => remote,
                remote::ConnectOutcome::Retry => continue,
                remote::ConnectOutcome::Quit => break 'outer,
            };

            match remote::handle_post_connect(
                &mut self,
                &mut terminal,
                &mut remote,
                &mut remote_state,
                session_to_resume.as_deref(),
            )
            .await?
            {
                remote::PostConnectOutcome::Ready => {}
                remote::PostConnectOutcome::Quit => break 'outer,
            }

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
                        remote::handle_tick(&mut self, &mut remote).await;
                    }
                    event = remote.next_event() => {
                        match remote::handle_remote_event(
                            &mut self,
                            &mut terminal,
                            &mut remote,
                            &mut remote_state,
                            event,
                        )
                        .await?
                        {
                            remote::RemoteEventOutcome::Continue => {}
                            remote::RemoteEventOutcome::Reconnect => continue 'outer,
                        }
                    }
                    event = event_stream.next() => {
                        remote::handle_terminal_event(&mut self, &mut terminal, &mut remote, event).await?;
                    }
                    bus_event = bus_receiver_remote.recv() => {
                        remote::handle_bus_event(&mut self, &mut remote, bus_event).await;
                    }
                }
            }
        }

        Ok(RunResult {
            reload_session: self.reload_requested.take(),
            rebuild_session: self.rebuild_requested.take(),
            update_session: self.update_requested.take(),
            exit_code: self.requested_exit_code,
            session_id: if self.is_remote {
                self.remote_session_id.clone()
            } else {
                Some(self.session.id.clone())
            },
        })
    }

    /// Run the TUI in replay mode, playing back a timeline of events.
    pub async fn run_replay(
        self,
        terminal: DefaultTerminal,
        timeline: Vec<crate::replay::TimelineEvent>,
        speed: f64,
    ) -> Result<RunResult> {
        replay::run_replay(self, terminal, timeline, speed).await
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
                replay::apply_replay_event(
                    &mut self,
                    &mut remote,
                    event,
                    &mut replay_turn_id,
                    Some(sim_time_ms),
                );
                event_cursor += 1;
            }

            if sim_time_ms >= next_frame_at {
                replay::update_replay_elapsed_override(&mut self, sim_time_ms);
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
        remote::handle_server_event(self, event, remote)
    }

    fn handle_remote_char_input(&mut self, c: char) {
        remote::handle_remote_char_input(self, c);
    }

    /// Handle keyboard input in remote mode
    async fn handle_remote_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: &mut super::backend::RemoteConnection,
    ) -> Result<()> {
        remote::handle_remote_key(self, code, modifiers, remote).await
    }

    /// Process turn while still accepting input for queueing
    async fn process_turn_with_input(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) {
        local::process_turn_with_input(self, terminal, event_stream).await;
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

        if input::handle_modal_key(self, code.clone(), modifiers)? {
            return Ok(());
        }

        if input::handle_pre_control_shortcuts(self, code.clone(), modifiers) {
            return Ok(());
        }

        self.normalize_diagram_state();
        let diagram_available = self.diagram_available();

        // Handle ctrl combos regardless of processing state
        if modifiers.contains(KeyModifiers::CONTROL)
            && input::handle_global_control_shortcuts(self, code, diagram_available)
        {
            return Ok(());
        }

        // Shift+Enter: does opposite of queue_mode during processing
        if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
            input::handle_shift_enter(self);
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

        if code == KeyCode::Enter {
            input::handle_enter(self);
            return Ok(());
        }

        if input::handle_basic_key(self, code) {
            return Ok(());
        }

        Ok(())
    }

    fn redraw_now(&self, terminal: &mut DefaultTerminal) -> Result<()> {
        terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
        Ok(())
    }

    /// Try to paste an image from the clipboard. Checks native image data first,
    /// then falls back to HTML clipboard for <img> URLs, then arboard text.
    /// Used by both Ctrl+V and Alt+V handlers in both local and remote mode.
    fn paste_image_from_clipboard(&mut self) {
        input::paste_image_from_clipboard(self);
    }

    /// Queue a message to be sent later
    /// Handle bracketed paste: store text content (image URLs are still detected inline)
    fn handle_paste(&mut self, text: String) {
        input::handle_paste(self, text);
    }

    /// Expand paste placeholders in input with actual content
    fn expand_paste_placeholders(&mut self, input: &str) -> String {
        input::expand_paste_placeholders(self, input)
    }

    fn queue_message(&mut self) {
        input::queue_message(self);
    }

    /// Send an interleave message immediately to the server as a soft interrupt.
    /// Skips the intermediate buffer stage - goes directly to pending_soft_interrupts.
    async fn send_interleave_now(
        &mut self,
        content: String,
        remote: &mut super::backend::RemoteConnection,
    ) {
        remote::send_interleave_now(self, content, remote).await;
    }

    /// Retrieve all pending unsent messages into the input for editing.
    /// Priority: pending soft interrupts first, then interleave, then queued.
    /// Returns true if pending soft interrupts were retrieved (caller should cancel on server).
    fn retrieve_pending_message_for_edit(&mut self) -> bool {
        input::retrieve_pending_message_for_edit(self)
    }

    fn send_action(&self, shift: bool) -> SendAction {
        input::send_action(self, shift)
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
            "poke" => {
                "`/poke`\nPoke the model to resume when it has stopped with incomplete todos.\n\
                Injects a reminder listing all pending/in-progress tasks and prompts the model to either\n\
                finish the work, update the todo list to reflect what is done, or ask for user input if genuinely blocked."
            }
            "reload" => "`/reload`\nReload to a newer binary if one is available.",
            "rebuild" => "`/rebuild`\nRun full update flow (git pull + cargo build + tests).",
            "split" => "`/split`\nSplit the current session into a new window. Clones the full conversation history so both sessions continue from the same point.",
            "resume" | "sessions" => "`/resume`\nOpen the interactive session picker. Browse and search all sessions, preview conversation history, and open any session in a new terminal window.\n\nPress `Esc` to return to your current session.",
            "info" => "`/info`\nShow session metadata and token usage.",
            "usage" => "`/usage`\nFetch and display subscription usage limits for all connected OAuth providers (Anthropic, OpenAI/ChatGPT).\nShows 5-hour and 7-day windows, reset times, and extra usage status.",
            "version" => "`/version`\nShow jcode version/build details.",
            "changelog" => "`/changelog`\nShow recent changes embedded in this build.",
            "quit" => "`/quit`\nExit jcode.",
            "config" => {
                "`/config`\nShow active configuration.\n\n`/config init`\nCreate default config file.\n\n`/config edit`\nOpen config in `$EDITOR`."
            }
            "auth" | "login" => {
                "`/auth`\nShow authentication status for all providers.\n\n`/login`\nInteractive provider selection - pick a provider to log into.\n\n`/login <provider>`\nStart login flow directly for any provider shown by `/login` or the `/login ` completions."
            }
            "account" | "accounts" => {
                "`/account`\nList all Anthropic OAuth accounts.\n\n`/account add <label>`\nAdd a new account via OAuth login.\n\n`/account switch <label>`\nSwitch the active account.\n\n`/account remove <label>`\nRemove an account."
            }
            "save" => {
                "`/save`\nBookmark the current session so it appears at the top of `/resume`.\n\n`/save <label>`\nBookmark with a custom label for easy identification.\n\nSaved sessions are shown in a dedicated \"Saved\" section in the session picker."
            }
            "unsave" => {
                "`/unsave`\nRemove the bookmark from the current session."
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
        if commands::handle_help_command(self, trimmed)
            || commands::handle_session_command(self, trimmed)
            || commands::handle_config_command(self, trimmed)
            || commands::handle_debug_command(self, trimmed)
            || commands::handle_model_command(self, trimmed)
            || commands::handle_info_command(self, trimmed)
            || commands::handle_auth_command(self, trimmed)
            || commands::handle_dev_command(self, trimmed)
        {
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

        // Wait for compaction to finish (up to 60s), reacting to Bus event
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        self.status = ProcessingStatus::RunningTool("compacting context...".to_string());
        let mut bus_rx = Bus::global().subscribe();

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

            // Wait for Bus notification or timeout (instead of sleep-polling)
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::select! {
                _ = bus_rx.recv() => {}
                _ = timeout => {}
            }
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
                            "📦 **Compaction started** — context above 80%, summarizing older messages in background..."
                                .to_string(),
                        ));
                        self.set_status_notice("Compacting context...");
                    }
                    crate::compaction::CompactionAction::HardCompacted(dropped) => {
                        self.push_display_message(DisplayMessage::system(format!(
                            "📦 **Emergency compaction** — context critically full, dropped {} old messages to fit.",
                            dropped,
                        )));
                        self.set_status_notice(format!(
                            "Emergency compaction: {} msgs dropped",
                            dropped
                        ));
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
            .map(|t| format!(" (was {} tokens)", t))
            .unwrap_or_default();
        self.push_display_message(DisplayMessage::system(format!(
            "📦 **Compaction complete** — context summarized ({}){}",
            event.trigger, tokens_str
        )));
        self.set_status_notice("Context compacted");
    }

    fn set_status_notice(&mut self, text: impl Into<String>) {
        self.status_notice = Some((text.into(), Instant::now()));
    }

    fn set_memory_feature_enabled(&mut self, enabled: bool) {
        self.memory_enabled = enabled;
        if !enabled {
            crate::memory::clear_pending_memory(&self.session.id);
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
        let providers = crate::provider_catalog::auth_status_login_providers();
        let mut message = String::from(
            "**Authentication Status:**\n\n| Provider | Status | Method |\n|----------|--------|--------|\n",
        );
        for provider in providers {
            message.push_str(&format!(
                "| {} | {} | {} |\n",
                provider.display_name,
                icon(status.state_for_provider(provider)),
                status.method_detail_for_provider(provider),
            ));
        }
        message.push_str(
            "\nUse `/login <provider>` to authenticate, `/account` to manage Anthropic accounts.",
        );
        self.push_display_message(DisplayMessage::system(message));
    }

    fn show_interactive_login(&mut self) {
        use std::fmt::Write as _;

        let status = crate::auth::AuthStatus::check();
        let icon = |state: crate::auth::AuthState| match state {
            crate::auth::AuthState::Available => "✓",
            crate::auth::AuthState::Expired => "⚠",
            crate::auth::AuthState::NotConfigured => "✗",
        };
        let providers = crate::provider_catalog::tui_login_providers();
        let mut message = String::from(
            "**Login** - select a provider:\n\n| # | Provider | Auth | Status |\n|---|----------|------|--------|\n",
        );
        for (index, provider) in providers.iter().enumerate() {
            let state = status.state_for_provider(*provider);
            let _ = writeln!(
                &mut message,
                "| {} | {} | {} | {} |",
                index + 1,
                provider.display_name,
                provider.auth_kind.label(),
                icon(state)
            );
        }
        let _ = write!(
            &mut message,
            "\nType a number (1-{}) or provider name, or `/cancel` to cancel.",
            providers.len()
        );
        self.push_display_message(DisplayMessage::system(message));
        self.pending_login = Some(PendingLogin::ProviderSelection);
    }

    fn start_login_provider(&mut self, provider: crate::provider_catalog::LoginProviderDescriptor) {
        match provider.target {
            crate::provider_catalog::LoginProviderTarget::Claude => self.start_claude_login(),
            crate::provider_catalog::LoginProviderTarget::OpenAi => self.start_openai_login(),
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                self.start_openrouter_login()
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
                self.start_openai_compatible_profile_login(profile)
            }
            crate::provider_catalog::LoginProviderTarget::Cursor => self.start_cursor_login(),
            crate::provider_catalog::LoginProviderTarget::Copilot => self.start_copilot_login(),
            crate::provider_catalog::LoginProviderTarget::Antigravity => {
                self.start_antigravity_login()
            }
            crate::provider_catalog::LoginProviderTarget::Google => {
                self.push_display_message(DisplayMessage::error(
                    "Google/Gmail login is only available from the CLI right now. Run `jcode login --provider google`."
                        .to_string(),
                ));
            }
        }
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

        let auth_url = crate::auth::oauth::claude_auth_url(&redirect_uri, &challenge, &verifier);
        let manual_auth_url = crate::auth::oauth::claude_auth_url(
            crate::auth::oauth::claude::REDIRECT_URI,
            &challenge,
            &verifier,
        );
        let qr_section = crate::login_qr::markdown_section(
            &manual_auth_url,
            "No browser on this machine? Scan this on another device, finish login there, then paste the full callback URL here:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             Waiting for callback on port {}...\n\
             If the browser didn't open, visit:\n{}\n\n\
             Or paste the authorization code here to complete manually.{}",
            port, auth_url, qr_section
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

        let auth_url = crate::auth::oauth::claude_auth_url(
            crate::auth::oauth::claude::REDIRECT_URI,
            &challenge,
            &verifier,
        );
        let qr_section = crate::login_qr::markdown_section(
            &auth_url,
            "Scan this on another device if this machine has no browser:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             After logging in, copy the callback URL or authorization code and **paste it here**.{}",
            auth_url, qr_section
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

        let auth_url = crate::auth::oauth::claude_auth_url(
            crate::auth::oauth::claude::REDIRECT_URI,
            &challenge,
            &verifier,
        );
        let qr_section = crate::login_qr::markdown_section(
            &auth_url,
            "Scan this on another device if this machine has no browser:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let _ = open::that(&auth_url);

        self.push_display_message(DisplayMessage::system(format!(
            "**Claude OAuth Login** (account: `{}`)\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             After logging in, copy the callback URL or authorization code and **paste it here**.{}",
            label, auth_url, qr_section
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
        lines.push("| Account | Email | Status | Subscription | Active |".to_string());
        lines.push("|---------|-------|--------|-------------|--------|".to_string());

        for account in &accounts {
            let is_active = active_label.as_deref() == Some(&account.label);
            let status = if account.expires > now_ms {
                "✓ valid"
            } else {
                "⚠ expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let sub = account.subscription_type.as_deref().unwrap_or("unknown");
            let active_mark = if is_active { "◉" } else { "" };
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                account.label, email, status, sub, active_mark
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
                // Keep account-sensitive UI state in sync immediately.
                crate::auth::AuthStatus::invalidate_cache();
                self.context_limit = self.provider.context_window() as u64;
                self.context_warning_shown = false;
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
        let auth_url = crate::auth::oauth::openai_auth_url(&redirect_uri, &challenge, &state);
        let qr_section = crate::login_qr::markdown_section(
            &auth_url,
            "Scan this on another device if this machine has no browser, then paste the full callback URL here:",
        )
        .map(|section| format!("\n\n{section}"))
        .unwrap_or_default();

        let browser_opened = open::that(&auth_url).is_ok();
        let callback_available = crate::auth::oauth::callback_listener_available(port);

        if callback_available {
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
                        crate::logging::info(&format!(
                            "OpenAI automatic callback did not complete: {}",
                            e
                        ));
                    }
                }
            });
        }

        let callback_line = if callback_available {
            format!(
                "Waiting for callback on `localhost:{}`... (this will complete automatically)\n",
                port
            )
        } else {
            format!(
                "Local callback port `localhost:{}` is unavailable, so finish in any browser and paste the full callback URL here.\n",
                port
            )
        };
        let browser_line = if browser_opened {
            String::new()
        } else {
            "This machine could not open a browser automatically.\n".to_string()
        };

        self.push_display_message(DisplayMessage::system(format!(
            "**OpenAI OAuth Login**\n\n\
             Opening browser for authentication...\n\n\
             If the browser didn't open, visit:\n{}\n\n\
             {}{}\
             Or paste the full callback URL or query string here to finish from another device.{}",
            auth_url, browser_line, callback_line, qr_section
        )));
        self.set_status_notice("Login: waiting…");
        self.pending_login = Some(PendingLogin::OpenAi {
            verifier,
            expected_state: state,
            redirect_uri,
        });
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

        Self::openai_token_exchange(verifier, code, None, &redirect_uri).await
    }

    async fn openai_token_exchange(
        verifier: String,
        input: String,
        expected_state: Option<String>,
        redirect_uri: &str,
    ) -> Result<String, String> {
        let oauth_tokens = if let Some(expected_state) = expected_state {
            crate::auth::oauth::exchange_openai_callback_input(
                &verifier,
                input.trim(),
                &expected_state,
                redirect_uri,
            )
            .await
            .map_err(|e| e.to_string())?
        } else {
            crate::auth::oauth::exchange_openai_code(&input, &verifier, redirect_uri)
                .await
                .map_err(|e| e.to_string())?
        };

        crate::auth::oauth::save_openai_tokens(&oauth_tokens)
            .map_err(|e| format!("Failed to save tokens: {}", e))?;

        Ok("Successfully logged in to OpenAI!".to_string())
    }

    fn start_openrouter_login(&mut self) {
        self.start_api_key_login(
            "OpenRouter",
            "https://openrouter.ai/keys",
            "openrouter.env",
            "OPENROUTER_API_KEY",
            None,
        );
    }

    fn start_opencode_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::OPENCODE_PROFILE);
    }

    fn start_opencode_go_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::OPENCODE_GO_PROFILE);
    }

    fn start_zai_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::ZAI_PROFILE);
    }

    fn start_chutes_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::CHUTES_PROFILE);
    }

    fn start_cerebras_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::CEREBRAS_PROFILE);
    }

    fn start_openai_compatible_login(&mut self) {
        self.start_openai_compatible_profile_login(crate::provider_catalog::OPENAI_COMPAT_PROFILE);
    }

    fn start_openai_compatible_profile_login(
        &mut self,
        profile: crate::provider_catalog::OpenAiCompatibleProfile,
    ) {
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        self.start_api_key_login(
            &resolved.display_name,
            &resolved.setup_url,
            &resolved.env_file,
            &resolved.api_key_env,
            resolved.default_model.as_deref(),
        );
    }

    fn start_api_key_login(
        &mut self,
        provider: &str,
        docs_url: &str,
        env_file: &str,
        key_name: &str,
        default_model: Option<&str>,
    ) {
        let model_hint = default_model
            .map(|m| format!("Suggested default model: `{}`\n\n", m))
            .unwrap_or_default();
        self.push_display_message(DisplayMessage::system(format!(
            "**{} API Key**\n\n\
             Setup docs: {}\n\
             Stored variable: `{}`\n\
             {}\n\
             **Paste your API key below** (it will be saved securely).",
            provider, docs_url, key_name, model_hint
        )));
        self.set_status_notice("Login: paste key…");
        self.pending_login = Some(PendingLogin::ApiKeyProfile {
            provider: provider.to_string(),
            docs_url: docs_url.to_string(),
            env_file: env_file.to_string(),
            key_name: key_name.to_string(),
            default_model: default_model.map(|m| m.to_string()),
        });
    }

    fn start_cursor_login(&mut self) {
        let binary = crate::auth::cursor::cursor_agent_cli_path();

        if crate::auth::cursor::has_cursor_agent_cli() {
            self.push_display_message(DisplayMessage::system(format!(
                "**Cursor Login**\n\n\
                 Running `{} login` to open browser authentication.\n\n\
                 If that fails, jcode will fall back to saving a Cursor API key for `cursor-agent`.",
                binary
            )));
            self.set_status_notice("Login: cursor browser...");

            match Self::run_external_login_command(&binary, &["login"]) {
                Ok(()) => {
                    self.push_display_message(DisplayMessage::system(
                        "✓ **Cursor login completed.**".to_string(),
                    ));
                    self.set_status_notice("Login: ✓ cursor");
                    crate::auth::AuthStatus::invalidate_cache();
                    return;
                }
                Err(e) => {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Cursor CLI login failed: {}\n\nFalling back to API key mode...",
                        e
                    )));
                }
            }
        }

        self.push_display_message(DisplayMessage::system(
            "**Cursor API Key**\n\n\
             Get your API key from: https://cursor.com/settings\n\
             (Dashboard > Integrations > User API Keys)\n\n\
             jcode will save it securely and provide it to `cursor-agent` at runtime.\n\
             You still need Cursor Agent installed to use the Cursor provider.\n\n\
             **Paste your API key below**."
                .to_string(),
        ));
        self.set_status_notice("Login: paste cursor key...");
        self.pending_login = Some(PendingLogin::CursorApiKey);
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
                message: {
                    let qr_section = crate::login_qr::markdown_section(
                        &verification_uri,
                        "Scan this on another device to open the GitHub verification page:",
                    )
                    .map(|section| format!("\n\n{section}"))
                    .unwrap_or_default();
                    format!(
                        "**GitHub Copilot Login**\n\n\
                         Your code: **{}**{}\n\n\
                         Opening browser to {} ...\n\
                         Paste the code there and authorize.{}\n\n\
                         Waiting for authorization... (type `/cancel` to abort)",
                        user_code, clipboard_msg, verification_uri, qr_section
                    )
                },
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
                                message: format!("Claude login [{}] failed: {}", label_clone, e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(format!(
                    "Exchanging authorization code for account `{}`...",
                    label
                )));
            }
            PendingLogin::OpenAi {
                verifier,
                expected_state,
                redirect_uri,
            } => {
                self.set_status_notice("Login: exchanging...");
                let input_owned = input.clone();
                tokio::spawn(async move {
                    match Self::openai_token_exchange(
                        verifier,
                        input_owned,
                        Some(expected_state),
                        &redirect_uri,
                    )
                    .await
                    {
                        Ok(msg) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "openai".to_string(),
                                success: true,
                                message: msg,
                            }));
                        }
                        Err(e) => {
                            Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                                provider: "openai".to_string(),
                                success: false,
                                message: format!("OpenAI login failed: {}", e),
                            }));
                        }
                    }
                });
                self.push_display_message(DisplayMessage::system(
                    "Exchanging OpenAI callback for tokens...".to_string(),
                ));
            }
            PendingLogin::ApiKeyProfile {
                provider,
                docs_url: _,
                env_file,
                key_name,
                default_model,
            } => {
                let key = input.trim().to_string();
                if key_name == "OPENROUTER_API_KEY" && !key.starts_with("sk-or-") {
                    self.push_display_message(DisplayMessage::system(
                        "⚠ OpenRouter keys typically start with `sk-or-`. Saving anyway..."
                            .to_string(),
                    ));
                }

                match Self::save_named_api_key(&env_file, &key_name, &key) {
                    Ok(()) => {
                        let model_hint = default_model
                            .map(|m| format!("\nSuggested default model: `{}`", m))
                            .unwrap_or_default();
                        let guidance = if key_name == "OPENROUTER_API_KEY" {
                            "You can now use `/model` to switch to OpenRouter models.".to_string()
                        } else {
                            format!(
                                "Restart with `--provider {}` to use this backend in a new session.",
                                provider.to_lowercase().replace(' ', "-")
                            )
                        };
                        self.push_display_message(DisplayMessage::system(format!(
                            "✓ **{} API key saved!**\n\n\
                             Stored at `~/.config/jcode/{}`.\n\
                             {}{}",
                            provider, env_file, guidance, model_hint
                        )));
                        self.set_status_notice("Login: ✓ saved");
                    }
                    Err(e) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save {} key: {}",
                            provider, e
                        )));
                    }
                }
            }
            PendingLogin::CursorApiKey => {
                let key = input.trim().to_string();
                if key.is_empty() {
                    self.push_display_message(DisplayMessage::error(
                        "API key cannot be empty.".to_string(),
                    ));
                    self.pending_login = Some(PendingLogin::CursorApiKey);
                    return;
                }

                match crate::auth::cursor::save_api_key(&key) {
                    Ok(()) => {
                        crate::auth::AuthStatus::invalidate_cache();
                        self.push_display_message(DisplayMessage::system(
                            "✓ **Cursor API key saved!**\n\n\
                             Stored at `~/.config/jcode/cursor.env`.\n\
                             jcode will pass it to `cursor-agent` automatically.\n\
                             Install Cursor Agent if it is not already on PATH."
                                .to_string(),
                        ));
                        self.set_status_notice("Login: ✓ cursor");
                    }
                    Err(e) => {
                        self.push_display_message(DisplayMessage::error(format!(
                            "Failed to save Cursor API key: {}",
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
                let providers = crate::provider_catalog::tui_login_providers();
                if let Some(provider) =
                    crate::provider_catalog::resolve_login_selection(&input, &providers)
                {
                    self.start_login_provider(provider);
                } else {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Unknown selection '{}'. Type 1-{} or a provider name.",
                        input.trim(),
                        providers.len()
                    )));
                    self.pending_login = Some(PendingLogin::ProviderSelection);
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

    fn handle_update_status(&mut self, status: crate::bus::UpdateStatus) {
        use crate::bus::UpdateStatus;
        match status {
            UpdateStatus::Checking => {
                self.set_status_notice("Checking for updates...");
            }
            UpdateStatus::Available { current, latest } => {
                self.set_status_notice(&format!("Update available: {} → {}", current, latest));
            }
            UpdateStatus::Downloading { version } => {
                self.set_status_notice(&format!("⬇️  Downloading {}...", version));
            }
            UpdateStatus::Installed { version } => {
                self.set_status_notice(&format!("✅ Updated to {} — restarting", version));
            }
            UpdateStatus::UpToDate => {}
            UpdateStatus::Error(e) => {
                self.set_status_notice(&format!("Update failed: {}", e));
            }
        }
    }

    async fn claude_token_exchange(
        verifier: String,
        input: String,
        label: &str,
        redirect_uri: Option<String>,
    ) -> Result<String, String> {
        let fallback_redirect_uri =
            redirect_uri.unwrap_or_else(|| crate::auth::oauth::claude::REDIRECT_URI.to_string());
        let redirect_uri =
            crate::auth::oauth::claude_redirect_uri_for_input(input.trim(), &fallback_redirect_uri);
        let oauth_tokens =
            crate::auth::oauth::exchange_claude_code(&verifier, input.trim(), &redirect_uri)
                .await
                .map_err(|e| e.to_string())?;

        crate::auth::oauth::save_claude_tokens_for_account(&oauth_tokens, label)
            .map_err(|e| format!("Failed to save tokens: {}", e))?;

        let profile_suffix = match crate::auth::oauth::update_claude_account_profile(
            label,
            &oauth_tokens.access_token,
        )
        .await
        {
            Ok(Some(email)) => format!(" (email: {})", mask_email(&email)),
            Ok(None) => String::new(),
            Err(e) => {
                crate::logging::warn(&format!(
                    "Claude login [{}] profile fetch failed: {}",
                    label, e
                ));
                String::new()
            }
        };

        Ok(format!(
            "Successfully logged in to Claude! (account: {}){}",
            label, profile_suffix
        ))
    }

    fn save_named_api_key(env_file: &str, key_name: &str, key: &str) -> anyhow::Result<()> {
        if !crate::provider_catalog::is_safe_env_key_name(key_name) {
            anyhow::bail!("Invalid API key variable name: {}", key_name);
        }
        if !crate::provider_catalog::is_safe_env_file_name(env_file) {
            anyhow::bail!("Invalid env file name: {}", env_file);
        }

        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
            .join("jcode");
        std::fs::create_dir_all(&config_dir)?;
        crate::platform::set_directory_permissions_owner_only(&config_dir)?;

        let file_path = config_dir.join(env_file);
        let content = format!("{}={}\n", key_name, key);
        std::fs::write(&file_path, &content)?;

        crate::platform::set_permissions_owner_only(&file_path)?;

        std::env::set_var(key_name, key);
        Ok(())
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
        new_session.saved = old_session.saved;
        new_session.save_label = old_session.save_label.clone();
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

                // Poll for background compaction completion during streaming
                self.poll_compaction_completion();

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
                            .map(|t| format!(" (was {} tokens)", t))
                            .unwrap_or_default();
                        let compact_msg = format!(
                            "📦 **Compaction complete** — context summarized ({}){}\n\n",
                            trigger, tokens_str
                        );
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
                                    self.redraw_now(terminal)?;
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                                self.redraw_now(terminal)?;
                            }
                            Some(Ok(Event::Mouse(mouse))) => {
                                self.handle_mouse_event(mouse);
                                self.redraw_now(terminal)?;
                            }
                            Some(Ok(Event::Resize(_, _))) => {
                                let _ = terminal.clear();
                                self.redraw_now(terminal)?;
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
                        // Poll for background compaction completion during streaming
                        self.poll_compaction_completion();
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

                                    self.redraw_now(terminal)?;
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                                self.redraw_now(terminal)?;
                            }
                            Some(Ok(Event::Mouse(mouse))) => {
                                self.handle_mouse_event(mouse);
                                self.redraw_now(terminal)?;
                            }
                            Some(Ok(Event::Resize(_, _))) => {
                                let _ = terminal.clear();
                                self.redraw_now(terminal)?;
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
                                            .map(|t| format!(" (was {} tokens)", t))
                                            .unwrap_or_default();
                                        let compact_msg = format!(
                                            "📦 **Compaction complete** — context summarized ({}){}\n\n",
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

                                        self.redraw_now(terminal)?;
                                    }
                                }
                                Some(Ok(Event::Paste(text))) => {
                                    self.handle_paste(text);
                                    self.redraw_now(terminal)?;
                                }
                                Some(Ok(Event::Mouse(mouse))) => {
                                    self.handle_mouse_event(mouse);
                                    self.redraw_now(terminal)?;
                                }
                                Some(Ok(Event::Resize(_, _))) => {
                                    let _ = terminal.clear();
                                    self.redraw_now(terminal)?;
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
        crate::memory::record_injected_prompt(&display_prompt, count, age_ms);
        let summary = if count == 1 {
            "🧠 auto-recalled 1 memory".to_string()
        } else {
            format!("🧠 auto-recalled {} memories", count)
        };
        self.push_display_message(DisplayMessage::memory(summary, display_prompt));
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
        let pending = crate::memory::take_pending_memory(&self.session.id);

        // Send context to memory agent for the NEXT turn (doesn't block current send)
        crate::memory_agent::update_context_sync(&self.session.id, messages.to_vec());

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

        // Extract memories using sidecar (with existing context for dedup)
        let manager = crate::memory::MemoryManager::new();
        let existing: Vec<String> = manager
            .list_all()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.active)
            .map(|e| e.content)
            .collect();
        let sidecar = crate::sidecar::Sidecar::new();
        match sidecar
            .extract_memories_with_existing(&transcript, &existing)
            .await
        {
            Ok(extracted) if !extracted.is_empty() => {
                let manager = crate::memory::MemoryManager::new();
                let mut stored_count = 0;

                for memory in extracted {
                    let category = crate::memory::MemoryCategory::from_extracted(&memory.category);

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
        if is_tool && self.diff_mode.has_side_pane() && self.diff_pane_auto_scroll {
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
            return crate::provider_catalog::tui_login_providers()
                .iter()
                .map(|provider| (format!("/login {}", provider.id), provider.menu_detail))
                .collect();
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
            ("/poke".into(), "Poke model to resume with incomplete todos"),
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
            ("/resume".into(), "Open session picker"),
            ("/save".into(), "Bookmark session for easy access"),
            ("/unsave".into(), "Remove bookmark from session"),
            ("/split".into(), "Split session into a new window"),
            ("/quit".into(), "Exit jcode"),
            ("/auth".into(), "Show authentication status"),
            ("/cache".into(), "Toggle cache TTL between 5min and 1h"),
            (
                "/login".into(),
                "Login to a provider (use `/login <provider>` for the full list)",
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
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        };
        if is_canary {
            return Vec::new();
        }

        let auth = crate::auth::AuthStatus::check();
        if !auth.has_any_available() {
            return vec![("Log in to get started".to_string(), "/login".to_string())];
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
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
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
                | "/save"
                | "/cache"
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
        self.log_cache_miss_if_unexpected();

        self.last_api_completed = Some(Instant::now());
        self.last_turn_input_tokens = {
            let input = self.streaming_input_tokens;
            if input > 0 {
                Some(input)
            } else {
                None
            }
        };

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

    /// Scroll to the previous user prompt (scroll up - earlier in conversation)
    pub fn scroll_to_prev_prompt(&mut self) {
        let positions = super::ui::last_user_prompt_positions();
        if positions.is_empty() {
            return;
        }

        let current = self.scroll_offset;

        // positions are in document order (top to bottom).
        // Find the last position that is strictly less than current (i.e. earlier/above).
        // If we're at the bottom (!auto_scroll_paused), treat current as past-the-end.
        if !self.auto_scroll_paused {
            // Jump to the most recent (last) prompt
            if let Some(&pos) = positions.last() {
                self.scroll_offset = pos;
                self.auto_scroll_paused = true;
            }
            return;
        }

        let mut target = None;
        for &pos in positions.iter().rev() {
            if pos < current {
                target = Some(pos);
                break;
            }
        }

        if let Some(pos) = target {
            self.scroll_offset = pos;
        }
        // If no prompt above, stay where we are
    }

    /// Scroll to the next user prompt (scroll down - later in conversation)
    pub fn scroll_to_next_prompt(&mut self) {
        let positions = super::ui::last_user_prompt_positions();
        if positions.is_empty() || !self.auto_scroll_paused {
            return;
        }

        let current = self.scroll_offset;

        // Find the first position strictly greater than current (i.e. later/below).
        for &pos in &positions {
            if pos > current {
                self.scroll_offset = pos;
                return;
            }
        }

        // No more prompts below - go to bottom
        self.follow_chat_bottom();
    }

    /// Scroll to Nth most-recent user prompt (1 = most recent, 2 = second most recent, etc.).
    /// Uses actual wrapped line positions from the last render frame for accurate placement,
    /// positioning the prompt at the top of the viewport.
    fn scroll_to_recent_prompt_rank(&mut self, rank: usize) {
        let rank = rank.max(1);
        let positions = super::ui::last_user_prompt_positions();
        let max_scroll = super::ui::last_max_scroll();

        if positions.is_empty() {
            return;
        }

        // positions are in document order (top to bottom), we want most-recent first
        let target_idx = positions.len().saturating_sub(rank);
        let target_line = positions[target_idx];
        self.set_status_notice(format!(
            "Ctrl+{}: idx={}/{} line={} max={}",
            rank,
            target_idx,
            positions.len(),
            target_line,
            max_scroll
        ));
        self.scroll_offset = target_line;
        self.auto_scroll_paused = true;
    }

    fn toggle_input_stash(&mut self) {
        if let Some((stashed, stashed_cursor)) = self.stashed_input.take() {
            let current_input = std::mem::replace(&mut self.input, stashed);
            let current_cursor = std::mem::replace(&mut self.cursor_pos, stashed_cursor);
            if current_input.is_empty() {
                self.set_status_notice("📋 Input restored from stash");
            } else {
                self.stashed_input = Some((current_input, current_cursor));
                self.set_status_notice("📋 Swapped input with stash");
            }
        } else if !self.input.is_empty() {
            let input = std::mem::take(&mut self.input);
            let cursor = std::mem::replace(&mut self.cursor_pos, 0);
            self.stashed_input = Some((input, cursor));
            self.set_status_notice("📋 Input stashed");
        }
    }

    fn save_input_for_reload(&self, session_id: &str) {
        if self.input.is_empty() {
            return;
        }
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join(format!("client-input-{}", session_id));
            let data = format!("{}\n{}", self.cursor_pos, self.input);
            let _ = std::fs::write(&path, &data);
        }
    }

    fn restore_input_from_reload(session_id: &str) -> Option<(String, usize)> {
        let jcode_dir = crate::storage::jcode_dir().ok()?;
        let path = jcode_dir.join(format!("client-input-{}", session_id));
        if !path.exists() {
            return None;
        }
        let data = std::fs::read_to_string(&path).ok()?;
        let _ = std::fs::remove_file(&path);
        let (cursor_str, input) = data.split_once('\n')?;
        let cursor = cursor_str.parse::<usize>().unwrap_or(0);
        let cursor = cursor.min(input.len());
        Some((input.to_string(), cursor))
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

            // Restrict TUI debug socket to owner-only.
            let _ = crate::platform::set_permissions_owner_only(&socket_path);

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

    fn handle_help_key(&mut self, code: KeyCode) -> Result<()> {
        let scroll = self.help_scroll.unwrap_or(0);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.help_scroll = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.help_scroll = Some(scroll.saturating_add(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.help_scroll = Some(scroll.saturating_sub(1));
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.help_scroll = Some(scroll.saturating_add(20));
            }
            KeyCode::PageUp => {
                self.help_scroll = Some(scroll.saturating_sub(20));
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.help_scroll = Some(0);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.help_scroll = Some(usize::MAX);
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
            self.remote_is_canary.unwrap_or(self.session.is_canary)
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

    fn has_stashed_input(&self) -> bool {
        self.stashed_input.is_some()
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
            (
                self.remote_provider_model.clone(),
                self.remote_reasoning_effort.clone(),
            )
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
                || (provider_name == "remote" && has_anthropic_oauth);
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

            if is_copilot_provider {
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
            } else if is_anthropic_oauth {
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

    fn help_scroll(&self) -> Option<usize> {
        self.help_scroll
    }

    fn session_picker_overlay(&self) -> Option<&RefCell<super::session_picker::SessionPicker>> {
        self.session_picker_overlay.as_ref()
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

    fn cache_ttl_status(&self) -> Option<super::CacheTtlInfo> {
        let last_completed = self.last_api_completed?;
        let provider = self.provider_name();
        let ttl_secs = super::cache_ttl_for_provider(&provider)?;
        let elapsed = last_completed.elapsed().as_secs();
        let remaining = ttl_secs.saturating_sub(elapsed);
        Some(super::CacheTtlInfo {
            remaining_secs: remaining,
            ttl_secs,
            is_cold: remaining == 0,
            cached_tokens: self.last_turn_input_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::TuiState;
    use ratatui::layout::Rect;

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
    fn test_mask_email_censors_local_part() {
        assert_eq!(mask_email("jeremyh1@uw.edu"), "j***1@uw.edu");
    }

    #[test]
    fn test_show_accounts_includes_masked_email_column() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let accounts = vec![crate::auth::claude::AnthropicAccount {
            label: "work".to_string(),
            access: "acc".to_string(),
            refresh: "ref".to_string(),
            expires: now_ms + 60000,
            email: Some("user@example.com".to_string()),
            subscription_type: Some("max".to_string()),
        }];

        let mut lines = vec!["**Anthropic Accounts:**\n".to_string()];
        lines.push("| Account | Email | Status | Subscription | Active |".to_string());
        lines.push("|---------|-------|--------|-------------|--------|".to_string());

        for account in &accounts {
            let status = if account.expires > now_ms {
                "✓ valid"
            } else {
                "⚠ expired"
            };
            let email = account
                .email
                .as_deref()
                .map(mask_email)
                .unwrap_or_else(|| "unknown".to_string());
            let sub = account.subscription_type.as_deref().unwrap_or("unknown");
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                account.label, email, status, sub, "◉"
            ));
        }

        let output = lines.join("\n");
        assert!(output.contains("| Account | Email | Status | Subscription | Active |"));
        assert!(output.contains("u***r@example.com"));
    }

    #[test]
    fn test_commands_alias_shows_help() {
        let mut app = create_test_app();
        app.input = "/commands".to_string();
        app.submit_input();

        assert!(
            app.help_scroll.is_some(),
            "/commands should open help overlay"
        );
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
    fn test_pinned_side_diagram_layout_allocates_right_pane() {
        let _render_lock = scroll_render_test_lock();
        let mut app = create_test_app();
        app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        app.diagram_pane_enabled = true;
        app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
        app.diagram_pane_ratio = 40;

        crate::tui::mermaid::clear_active_diagrams();
        crate::tui::mermaid::register_active_diagram(0x111, 900, 450, Some("side".to_string()));

        crate::tui::visual_debug::enable();
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
        terminal
            .draw(|f| crate::tui::ui::draw(f, &app))
            .expect("draw failed");

        let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
        let diagram = frame.layout.diagram_area.expect("diagram area");
        let messages = frame.layout.messages_area.expect("messages area");

        assert!(
            diagram.width >= 24,
            "diagram pane too narrow: {}",
            diagram.width
        );
        assert_eq!(diagram.height, 40);
        assert_eq!(diagram.x, messages.x + messages.width);
        assert_eq!(diagram.y, 0);
        assert!(
            diagram.width < 120,
            "diagram should not consume full terminal width"
        );
        assert!(frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram"));

        crate::tui::visual_debug::disable();
        crate::tui::mermaid::clear_active_diagrams();
    }

    #[test]
    fn test_pinned_top_diagram_layout_allocates_top_pane() {
        let _render_lock = scroll_render_test_lock();
        let mut app = create_test_app();
        app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        app.diagram_pane_enabled = true;
        app.diagram_pane_position = crate::config::DiagramPanePosition::Top;
        app.diagram_pane_ratio = 35;

        crate::tui::mermaid::clear_active_diagrams();
        crate::tui::mermaid::register_active_diagram(0x222, 500, 900, Some("top".to_string()));

        crate::tui::visual_debug::enable();
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
        terminal
            .draw(|f| crate::tui::ui::draw(f, &app))
            .expect("draw failed");

        let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
        let diagram = frame.layout.diagram_area.expect("diagram area");
        let messages = frame.layout.messages_area.expect("messages area");

        assert_eq!(diagram.x, 0);
        assert_eq!(diagram.width, 120);
        assert!(
            diagram.height >= 6,
            "diagram pane too short: {}",
            diagram.height
        );
        assert_eq!(messages.y, diagram.y + diagram.height);
        assert!(frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram"));

        crate::tui::visual_debug::disable();
        crate::tui::mermaid::clear_active_diagrams();
    }

    #[test]
    fn test_pinned_diagram_not_shown_when_terminal_too_narrow() {
        let _render_lock = scroll_render_test_lock();
        let mut app = create_test_app();
        app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        app.diagram_pane_enabled = true;
        app.diagram_pane_position = crate::config::DiagramPanePosition::Side;

        crate::tui::mermaid::clear_active_diagrams();
        crate::tui::mermaid::register_active_diagram(0x333, 900, 450, None);

        crate::tui::visual_debug::enable();
        let backend = ratatui::backend::TestBackend::new(30, 20);
        let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
        terminal
            .draw(|f| crate::tui::ui::draw(f, &app))
            .expect("draw failed");

        let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
        assert!(
            frame.layout.diagram_area.is_none(),
            "diagram pane should be suppressed on narrow terminal"
        );
        assert!(!frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram"));

        crate::tui::visual_debug::disable();
        crate::tui::mermaid::clear_active_diagrams();
    }

    #[test]
    fn test_mouse_scroll_over_diff_pane_scrolls_side_panel() {
        let mut app = create_test_app();
        app.diff_mode = crate::config::DiffDisplayMode::File;
        app.diff_pane_scroll = 5;
        app.diff_pane_focus = false;
        app.diff_pane_auto_scroll = true;

        crate::tui::ui::record_layout_snapshot(
            Rect::new(0, 0, 40, 20),
            None,
            Some(Rect::new(40, 0, 20, 20)),
        );

        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 45,
            row: 5,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(app.diff_pane_scroll, 8);
        assert!(app.diff_pane_focus);
        assert!(!app.diff_pane_auto_scroll);
    }

    #[test]
    fn test_mouse_scroll_over_unfocused_diagram_resizes_immediately_without_animation() {
        let mut app = create_test_app();
        app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        app.diagram_pane_enabled = true;
        app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
        app.diagram_pane_ratio = 40;
        app.diagram_pane_ratio_from = 40;
        app.diagram_pane_ratio_target = 40;
        app.diagram_pane_anim_start = None;
        app.diagram_focus = false;

        crate::tui::mermaid::clear_active_diagrams();
        crate::tui::mermaid::register_active_diagram(0x444, 900, 450, None);
        crate::tui::ui::record_layout_snapshot(
            Rect::new(0, 0, 80, 30),
            Some(Rect::new(80, 0, 40, 30)),
            None,
        );

        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 90,
            row: 10,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(app.diagram_pane_ratio, 43);
        assert_eq!(app.diagram_pane_ratio_from, 43);
        assert_eq!(app.diagram_pane_ratio_target, 43);
        assert!(app.diagram_pane_anim_start.is_none());

        crate::tui::mermaid::clear_active_diagrams();
    }

    #[test]
    fn test_dragging_diagram_border_resizes_immediately_without_animation() {
        let mut app = create_test_app();
        app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
        app.diagram_pane_enabled = true;
        app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
        app.diagram_pane_ratio = 40;
        app.diagram_pane_ratio_from = 40;
        app.diagram_pane_ratio_target = 40;
        app.diagram_pane_anim_start = Some(Instant::now());
        app.diagram_pane_dragging = false;

        crate::tui::mermaid::clear_active_diagrams();
        crate::tui::mermaid::register_active_diagram(0x445, 900, 450, None);
        crate::tui::ui::record_layout_snapshot(
            Rect::new(0, 0, 80, 30),
            Some(Rect::new(80, 0, 40, 30)),
            None,
        );

        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 80,
            row: 10,
            modifiers: KeyModifiers::empty(),
        });
        assert!(app.diagram_pane_dragging);

        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 72,
            row: 10,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(app.diagram_pane_ratio, 40);
        assert_eq!(app.diagram_pane_ratio_from, 40);
        assert_eq!(app.diagram_pane_ratio_target, 40);
        assert!(app.diagram_pane_anim_start.is_none());

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

    fn configure_test_remote_models_with_openai_recommendations(app: &mut App) {
        app.is_remote = true;
        app.remote_provider_model = Some("gpt-5.2".to_string());
        app.remote_available_models = vec![
            "gpt-5.2".to_string(),
            "gpt-5.4".to_string(),
            "gpt-5.4-pro".to_string(),
            "gpt-5.3-codex-spark".to_string(),
            "gpt-5.3-codex".to_string(),
            "claude-opus-4-6".to_string(),
        ];
        app.remote_model_routes = app
            .remote_available_models
            .iter()
            .cloned()
            .map(|model| crate::provider::ModelRoute {
                model,
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            })
            .collect();
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
    fn test_model_picker_preserves_recommendation_priority_order() {
        let mut app = create_test_app();
        configure_test_remote_models_with_openai_recommendations(&mut app);

        app.open_model_picker();

        let picker = app
            .picker_state
            .as_ref()
            .expect("model picker should be open");

        let model_names: Vec<&str> = picker.models.iter().map(|m| m.name.as_str()).collect();

        assert_eq!(model_names.first().copied(), Some("gpt-5.2"));

        let gpt54 = picker
            .models
            .iter()
            .position(|model| model.name == "gpt-5.4")
            .expect("gpt-5.4 should be present");
        let gpt54_pro = picker
            .models
            .iter()
            .position(|model| model.name == "gpt-5.4-pro")
            .expect("gpt-5.4-pro should be present");
        let claude_opus = picker
            .models
            .iter()
            .position(|model| model.name == "claude-opus-4-6")
            .expect("claude-opus-4-6 should be present");
        let spark = picker
            .models
            .iter()
            .position(|model| model.name == "gpt-5.3-codex-spark")
            .expect("gpt-5.3-codex-spark should be present");
        let codex = picker
            .models
            .iter()
            .position(|model| model.name == "gpt-5.3-codex")
            .expect("gpt-5.3-codex should be present");

        assert!(
            gpt54 < gpt54_pro,
            "gpt-5.4 should rank ahead of gpt-5.4-pro, got {:?}",
            model_names
        );
        assert!(
            gpt54_pro < claude_opus,
            "gpt-5.4-pro should rank ahead of claude-opus-4-6, got {:?}",
            model_names
        );
        assert!(
            claude_opus < spark,
            "claude-opus-4-6 should rank ahead of non-recommended gpt-5.3-codex-spark, got {:?}",
            model_names
        );
        assert!(
            !picker.models[spark].recommended,
            "gpt-5.3-codex-spark should not be recommended"
        );
        assert!(
            !picker.models[codex].recommended,
            "gpt-5.3-codex should not be recommended"
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
    fn test_ctrl_c_requests_cancel_while_processing() {
        let mut app = create_test_app();
        app.is_processing = true;
        app.interleave_message = Some("queued interrupt".to_string());
        app.pending_soft_interrupts
            .push("pending soft interrupt".to_string());

        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
            .unwrap();

        assert!(app.cancel_requested);
        assert!(app.interleave_message.is_none());
        assert!(app.pending_soft_interrupts.is_empty());
        assert_eq!(app.status_notice(), Some("Interrupting...".to_string()));
    }

    #[test]
    fn test_ctrl_c_still_arms_quit_when_idle() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
            .unwrap();

        assert!(!app.cancel_requested);
        assert!(app.quit_pending.is_some());
        assert_eq!(
            app.status_notice(),
            Some("Press Ctrl+C again to quit".to_string())
        );
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
    fn test_handle_server_event_token_usage_uses_per_call_deltas() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::TokenUsage {
                input: 100,
                output: 10,
                cache_read_input: None,
                cache_creation_input: None,
            },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::TokenUsage {
                input: 100,
                output: 30,
                cache_read_input: None,
                cache_creation_input: None,
            },
            &mut remote,
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::TokenUsage {
                input: 100,
                output: 30,
                cache_read_input: None,
                cache_creation_input: None,
            },
            &mut remote,
        );

        assert_eq!(app.streaming_output_tokens, 30);
        assert_eq!(app.streaming_total_output_tokens, 30);
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
    fn test_handle_remote_disconnect_flushes_streaming_text_and_sets_reconnect_state() {
        let mut app = create_test_app();
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(7);
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "retry me".to_string(),
            images: vec![],
            is_system: false,
            auto_retry: false,
            retry_attempts: 0,
            retry_at: None,
        });
        app.streaming_text = "partial response being streamed".to_string();

        let mut state = remote::RemoteRunState::default();
        remote::handle_disconnect(&mut app, &mut state);

        assert!(!app.is_processing);
        assert!(matches!(app.status, ProcessingStatus::Idle));
        assert!(app.current_message_id.is_none());
        assert!(app.rate_limit_pending_message.is_none());
        assert!(app.streaming_text.is_empty());
        assert_eq!(state.disconnect_msg_idx, Some(1));
        assert_eq!(state.reconnect_attempts, 1);
        assert!(state.disconnect_start.is_some());

        let assistant = app
            .display_messages()
            .iter()
            .find(|m| m.role == "assistant")
            .expect("streaming text should have been saved as assistant message");
        assert_eq!(assistant.content, "partial response being streamed");

        let last = app
            .display_messages()
            .last()
            .expect("missing reconnect status message");
        assert_eq!(last.role, "system");
        assert_eq!(last.content, "⚡ Connection lost — reconnecting…");
    }

    #[test]
    fn test_handle_remote_disconnect_retryable_pending_schedules_retry() {
        let mut app = create_test_app();
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(7);
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "retry me".to_string(),
            images: vec![],
            is_system: true,
            auto_retry: true,
            retry_attempts: 0,
            retry_at: None,
        });

        let mut state = remote::RemoteRunState::default();
        remote::handle_disconnect(&mut app, &mut state);

        let pending = app
            .rate_limit_pending_message
            .as_ref()
            .expect("retryable continuation should remain pending");
        assert!(pending.auto_retry);
        assert_eq!(pending.retry_attempts, 1);
        assert!(pending.retry_at.is_some());
        assert!(app.rate_limit_reset.is_some());
    }

    #[test]
    fn test_handle_server_event_history_with_interruption_queues_continuation() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::History {
                id: 1,
                session_id: "ses_test_123".to_string(),
                messages: vec![crate::protocol::HistoryMessage {
                    role: "assistant".to_string(),
                    content: "I was working on something".to_string(),
                    tool_calls: None,
                    tool_data: None,
                }],
                provider_name: Some("claude".to_string()),
                provider_model: Some("claude-sonnet-4-20250514".to_string()),
                available_models: vec![],
                available_model_routes: vec![],
                mcp_servers: vec![],
                skills: vec![],
                total_tokens: None,
                all_sessions: vec![],
                client_count: None,
                is_canary: None,
                server_version: None,
                server_name: None,
                server_icon: None,
                server_has_update: None,
                was_interrupted: Some(true),
                upstream_provider: None,
                reasoning_effort: None,
            },
            &mut remote,
        );

        assert!(app.display_messages().len() >= 2);
        let system_msg = app
            .display_messages()
            .iter()
            .find(|m| m.role == "system" && m.content.contains("interrupted"))
            .expect("should have a system message about interruption");
        assert!(system_msg.content.contains("interrupted mid-generation"));

        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("interrupted by a server reload"));
    }

    #[test]
    fn test_handle_server_event_history_without_interruption_does_not_queue() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::History {
                id: 1,
                session_id: "ses_test_456".to_string(),
                messages: vec![crate::protocol::HistoryMessage {
                    role: "assistant".to_string(),
                    content: "Normal response".to_string(),
                    tool_calls: None,
                    tool_data: None,
                }],
                provider_name: Some("claude".to_string()),
                provider_model: Some("claude-sonnet-4-20250514".to_string()),
                available_models: vec![],
                available_model_routes: vec![],
                mcp_servers: vec![],
                skills: vec![],
                total_tokens: None,
                all_sessions: vec![],
                client_count: None,
                is_canary: None,
                server_version: None,
                server_name: None,
                server_icon: None,
                server_has_update: None,
                was_interrupted: None,
                upstream_provider: None,
                reasoning_effort: None,
            },
            &mut remote,
        );

        assert!(app.queued_messages().is_empty());
        assert!(!app
            .display_messages()
            .iter()
            .any(|m| m.content.contains("interrupted")));
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
            auto_retry: false,
            retry_attempts: 0,
            retry_at: None,
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
            auto_retry: false,
            retry_attempts: 0,
            retry_at: None,
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
    fn test_remote_error_with_retryable_pending_schedules_retry() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "retry me".to_string(),
            images: vec![],
            is_system: true,
            auto_retry: true,
            retry_attempts: 0,
            retry_at: None,
        });
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;

        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 11,
                message: "provider failed hard".to_string(),
                retry_after_secs: None,
            },
            &mut remote,
        );

        let pending = app
            .rate_limit_pending_message
            .as_ref()
            .expect("retryable continuation should remain pending");
        assert!(pending.auto_retry);
        assert_eq!(pending.retry_attempts, 1);
        assert!(pending.retry_at.is_some());
        assert!(app.rate_limit_reset.is_some());
        assert!(app
            .display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Auto-retrying")));
    }

    #[test]
    fn test_schedule_pending_remote_retry_respects_retry_limit() {
        let mut app = create_test_app();
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "retry me".to_string(),
            images: vec![],
            is_system: true,
            auto_retry: true,
            retry_attempts: App::AUTO_RETRY_MAX_ATTEMPTS,
            retry_at: None,
        });

        assert!(!app.schedule_pending_remote_retry("⚠ failed."));
        assert!(app.rate_limit_pending_message.is_none());
        assert!(app.rate_limit_reset.is_none());
        assert!(app
            .display_messages()
            .iter()
            .any(|m| m.role == "error" && m.content.contains("Auto-retry limit reached")));
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

    fn scroll_render_test_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};

        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        let _render_lock = scroll_render_test_lock();
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

    // NOTE: test_prompt_jump_ctrl_digits_by_recency was removed because it relied on
    // pre-render prompt positions that no longer exist. The render-based version
    // test_prompt_jump_ctrl_digit_is_recency_rank_in_app covers this functionality.

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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
    fn test_remote_ctrl_c_interrupts_while_processing() {
        let mut app = create_test_app();
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        rt.block_on(app.handle_remote_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut remote))
            .unwrap();

        assert!(app.quit_pending.is_none());
        assert!(app.is_processing);
    }

    #[test]
    fn test_remote_ctrl_c_still_arms_quit_when_idle() {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        rt.block_on(app.handle_remote_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut remote))
            .unwrap();

        assert!(app.quit_pending.is_some());
        assert_eq!(
            app.status_notice(),
            Some("Press Ctrl+C again to quit".to_string())
        );
    }

    #[test]
    fn test_remote_scroll_cmd_j_k_fallback() {
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
        let (app, mut terminal) = create_scroll_test_app(80, 15, 1, 20);
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
        // Should have scroll indicator or prompt preview since content extends above viewport.
        // The prompt preview (N›) renders on top of the ↑ indicator, so check for either.
        assert!(
            text.contains('↑') || text.contains('›'),
            "expected ↑ indicator or prompt preview when content extends above viewport"
        );
    }

    #[test]
    fn test_scroll_render_scrolled_up() {
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
        let _render_lock = scroll_render_test_lock();
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
