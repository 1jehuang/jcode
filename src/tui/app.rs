#![allow(dead_code)]

use super::keybind::{ModelSwitchKeys, ScrollKeys};
use super::markdown::IncrementalMarkdownRenderer;
use super::stream_buffer::StreamBuffer;
use crate::bus::{BackgroundTaskStatus, Bus, BusEvent, ToolEvent, ToolStatus};
use crate::config::config;
use crate::id;
use crate::mcp::McpManager;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolCall};
use crate::provider::Provider;
use crate::session::Session;
use crate::skill::SkillRegistry;
use crate::tool::selfdev::ReloadContext;
use crate::tool::{Registry, ToolContext};
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;

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

/// Current processing status
#[derive(Clone, Default, Debug)]
pub enum ProcessingStatus {
    #[default]
    Idle,
    /// Sending request to API
    Sending,
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
    // Total session token usage (accumulated across all turns)
    total_input_tokens: u64,
    total_output_tokens: u64,
    // Context limit tracking (for compaction warning)
    context_limit: u64,
    context_warning_shown: bool,
    // Context info (what's loaded in system prompt)
    context_info: crate::prompt::ContextInfo,
    // Track last streaming activity for "stale" detection
    last_stream_activity: Option<Instant>,
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
    // Cached MCP server names (updated on connect/disconnect)
    mcp_server_names: Vec<String>,
    // Semantic stream buffer for chunked output
    stream_buffer: StreamBuffer,
    // Track thinking start time for extended thinking display
    thinking_start: Option<Instant>,
    // Whether we've inserted the current turn's thought line
    thought_line_inserted: bool,
    // Hot-reload: if set, exec into new binary with this session ID (no rebuild)
    reload_requested: Option<String>,
    // Hot-rebuild: if set, do full git pull + cargo build + tests then exec
    rebuild_requested: Option<String>,
    // Pasted content storage (displayed as placeholders, expanded on submit)
    pasted_contents: Vec<String>,
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
    // Current message request ID (for remote mode - to match Done events)
    current_message_id: Option<u64>,
    // Whether running in remote mode
    is_remote: bool,
    // Remember tool call ids that already have outputs
    tool_result_ids: HashSet<String>,
    // Current session ID (from server in remote mode)
    remote_session_id: Option<String>,
    // All sessions on the server (remote mode only)
    remote_sessions: Vec<String>,
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
    // Show diffs for edit/write tool outputs (toggle with Alt+D)
    show_diffs: bool,
    // Center all content (from config)
    centered: bool,
    // Keybindings for model switching
    model_switch_keys: ModelSwitchKeys,
    // Keybindings for scrolling
    scroll_keys: ScrollKeys,
    // Short-lived notice for status feedback (model switch, toggle diff, etc.)
    status_notice: Option<(String, Instant)>,
    // Message to interleave during processing (set via Shift+Enter)
    interleave_message: Option<String>,
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
    // Message that was being sent when rate limit hit (to auto-retry)
    rate_limit_pending_message: Option<String>,
    // Store reload info to pass to agent after reconnection (remote mode)
    reload_info: Vec<String>,
    // Debug trace for scripted testing
    debug_trace: DebugTrace,
    // Incremental markdown renderer for streaming text (uses RefCell for interior mutability)
    streaming_md_renderer: RefCell<IncrementalMarkdownRenderer>,
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
    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let skills = SkillRegistry::load().unwrap_or_default();
        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
        let mut session = Session::create(None, None);
        session.model = Some(provider.model());
        let display = config().display.clone();
        let context_limit = crate::provider::context_limit_for_model(&provider.model())
            .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT) as u64;

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
            active_skill: None,
            is_processing: false,
            streaming_text: String::new(),
            should_quit: false,
            queued_messages: Vec::new(),
            streaming_input_tokens: 0,
            streaming_output_tokens: 0,
            streaming_cache_read_tokens: None,
            streaming_cache_creation_tokens: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            context_limit,
            context_warning_shown: false,
            context_info,
            last_stream_activity: None,
            status: ProcessingStatus::default(),
            subagent_status: None,
            processing_started: None,
            pending_turn: false,
            streaming_tool_calls: Vec::new(),
            provider_session_id: None,
            cancel_requested: false,
            quit_pending: None,
            mcp_server_names: Vec::new(),
            stream_buffer: StreamBuffer::new(),
            thinking_start: None,
            thought_line_inserted: false,
            reload_requested: None,
            rebuild_requested: None,
            pasted_contents: Vec::new(),
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
            current_message_id: None,
            is_remote: false,
            tool_result_ids: HashSet::new(),
            remote_session_id: None,
            remote_sessions: Vec::new(),
            known_stable_version: crate::build::read_stable_version().ok().flatten(),
            last_version_check: Some(Instant::now()),
            pending_migration: None,
            remote_client_count: None,
            resume_session_id: None,
            requested_exit_code: None,
            show_diffs: display.show_diffs,
            centered: display.centered,
            model_switch_keys: super::keybind::load_model_switch_keys(),
            scroll_keys: super::keybind::load_scroll_keys(),
            status_notice: None,
            interleave_message: None,
            queue_mode: display.queue_mode,
            tab_completion_state: None,
            app_started: Instant::now(),
            client_binary_mtime: std::env::current_exe()
                .ok()
                .and_then(|p| std::fs::metadata(&p).ok())
                .and_then(|m| m.modified().ok()),
            rate_limit_reset: None,
            rate_limit_pending_message: None,
            reload_info: Vec::new(),
            debug_trace: DebugTrace::new(),
            streaming_md_renderer: RefCell::new(IncrementalMarkdownRenderer::new(None)),
        }
    }

    /// Create an App instance for remote mode (connecting to server)
    pub async fn new_for_remote(resume_session: Option<String>) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(NullProvider);
        let registry = Registry::new(Arc::clone(&provider)).await;
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

    /// Get the current session ID
    pub fn session_id(&self) -> &str {
        &self.session.id
    }

    /// Check if there's a newer binary on disk than when we started
    fn has_newer_binary(&self) -> bool {
        let Some(startup_mtime) = self.client_binary_mtime else {
            return false;
        };

        // Check the release binary in the repo
        if let Some(repo_dir) = crate::build::get_repo_dir() {
            let exe = repo_dir.join("target/release/jcode");
            if let Ok(metadata) = std::fs::metadata(&exe) {
                if let Ok(current_mtime) = metadata.modified() {
                    return current_mtime > startup_mtime;
                }
            }
        }

        // Fallback: check the binary in PATH
        if let Some(exe) = crate::build::jcode_path_in_path() {
            if let Ok(metadata) = std::fs::metadata(&exe) {
                if let Ok(current_mtime) = metadata.modified() {
                    return current_mtime > startup_mtime;
                }
            }
        }

        false
    }

    /// Initialize MCP servers (call after construction)
    pub async fn init_mcp(&mut self) {
        // Always register the MCP management tool so agent can connect servers
        let mcp_tool = crate::tool::mcp::McpManagementTool::new(Arc::clone(&self.mcp_manager));
        self.registry
            .register("mcp".to_string(), Arc::new(mcp_tool))
            .await;

        let manager = self.mcp_manager.read().await;
        if !manager.config().servers.is_empty() {
            drop(manager);
            let mut init_error = None;
            {
                let manager = self.mcp_manager.write().await;
                if let Err(e) = manager.connect_all().await {
                    init_error = Some(format!("MCP init error: {}", e));
                }
                // Cache server names
                self.mcp_server_names = manager.connected_servers().await;
            }
            if let Some(msg) = init_error {
                crate::logging::error(&msg);
                self.push_display_message(DisplayMessage::error(msg));
                self.set_status_notice("MCP init failed");
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

            // Restore full message history for provider context
            self.messages = session.messages_for_provider();

            // Don't restore provider_session_id - Claude sessions don't persist across
            // process restarts. The messages are restored, so Claude will get full context.
            self.provider_session_id = None;
            self.session = session;
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
            // Only do this if there's actually a conversation to continue
            if total_turns > 0 {
                // Try to load reload context for richer continuation message
                let reload_ctx = ReloadContext::load().ok().flatten();

                let continuation_msg = if let Some(ctx) = reload_ctx {
                    let action = if ctx.is_rollback {
                        "Rollback"
                    } else {
                        "Reload"
                    };
                    let task_info = ctx
                        .task_context
                        .map(|t| format!("\nYou were working on: {}", t))
                        .unwrap_or_default();

                    format!(
                        "[{} complete. Previous version: {}, New version: {}.{}\nSession restored with {} turns. Continue with your task.]",
                        action,
                        ctx.version_before,
                        ctx.version_after,
                        task_info,
                        total_turns
                    )
                } else {
                    // Fallback to basic message if no context
                    let cwd = std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "unknown".to_string());

                    format!(
                        "[Reload complete. Build: {}, CWD: {}, Session: {} turns. Continue where you left off.]",
                        build_hash,
                        cwd,
                        total_turns
                    )
                };

                self.queued_messages.push(continuation_msg);
            }
        } else {
            crate::logging::error(&format!("Failed to restore session: {}", session_id));

            // Check if this was a reload that failed - inject failure message if so
            if let Ok(Some(ctx)) = ReloadContext::load() {
                let action = if ctx.is_rollback {
                    "Rollback"
                } else {
                    "Reload"
                };
                let task_info = ctx
                    .task_context
                    .map(|t| format!(" You were working on: {}", t))
                    .unwrap_or_default();

                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!(
                        "⚠ {} failed. Session could not be restored. Previous version: {}, Target version: {}.{}\n\
                         Starting fresh session. You may need to re-examine your changes.",
                        action,
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
            return serde_json::json!({
                "visual_debug_enabled": enabled
            })
            .to_string();
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
                    format!("OK: queued message '{}' (will send after current turn)", msg)
                }
                SendAction::Interleave => {
                    let expanded = self.expand_paste_placeholders(&self.input.clone());
                    self.pasted_contents.clear();
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
                "version": env!("JCODE_VERSION"),
            })
            .to_string()
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
        } else if cmd.starts_with("assert:") {
            let raw = cmd.strip_prefix("assert:").unwrap_or("");
            self.handle_assertions(raw)
        } else if cmd.starts_with("run:") {
            let raw = cmd.strip_prefix("run:").unwrap_or("");
            self.handle_script_run(raw)
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
                    if self.scroll_offset > 0 {
                        self.scroll_offset = self.scroll_offset.saturating_sub(5);
                    }
                    format!("scroll: up to {}", self.scroll_offset)
                }
                "down" => {
                    self.scroll_offset += 5;
                    format!("scroll: down to {}", self.scroll_offset)
                }
                "top" => {
                    self.scroll_offset = 0;
                    "scroll: top".to_string()
                }
                "bottom" => {
                    self.scroll_offset = usize::MAX / 2;
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
                 - enable/disable/status - control visual debug capture\n\
                 - wait - check if processing\n\
                 - wait:<ms> - block until idle or timeout\n\
                 - scroll:<up|down|top|bottom> - control scroll\n\
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
                "remote": true,
                "server_version": self.remote_server_version.clone(),
                "server_has_update": self.remote_server_has_update,
                "version": env!("JCODE_VERSION"),
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
                        "up" => self.scroll_offset = self.scroll_offset.saturating_add(5),
                        "down" => self.scroll_offset = self.scroll_offset.saturating_sub(5),
                        "top" => self.scroll_offset = usize::MAX,
                        "bottom" => self.scroll_offset = 0,
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
                    if self.scroll_offset > 0 {
                        self.scroll_offset = self.scroll_offset.saturating_sub(5);
                    }
                    format!("scroll: up to {}", self.scroll_offset)
                }
                "down" => {
                    self.scroll_offset += 5;
                    format!("scroll: down to {}", self.scroll_offset)
                }
                "top" => {
                    self.scroll_offset = 0;
                    "scroll: top".to_string()
                }
                "bottom" => {
                    self.scroll_offset = usize::MAX / 2;
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

    /// Check for selfdev signal files (rebuild-signal, rollback-signal)
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

        // Check for rollback signal
        let rollback_path = jcode_dir.join("rollback-signal");
        if rollback_path.exists() {
            if let Ok(_hash) = std::fs::read_to_string(&rollback_path) {
                // Remove signal file
                let _ = std::fs::remove_file(&rollback_path);
                // Save session and trigger exit with code 43 (rollback requested)
                self.session.provider_session_id = self.provider_session_id.clone();
                let _ = self.session.save();
                self.requested_exit_code = Some(43);
                self.should_quit = true;
            }
        }
    }

    /// Run the TUI application
    /// Returns Some(session_id) if hot-reload was requested
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<RunResult> {
        let mut event_stream = EventStream::new();
        let mut redraw_interval = interval(Duration::from_millis(50));
        // Subscribe to bus for background task completion notifications
        let mut bus_receiver = Bus::global().subscribe();

        loop {
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
                        // Check for debug commands
                        self.check_debug_command();
                        // Check for selfdev signals (rebuild/rollback)
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
                                // Handle bracketed paste from terminal
                                self.handle_paste(text);
                            }
                            Some(Ok(Event::Mouse(mouse))) => {
                                // Handle mouse scroll wheel for scrolling
                                // Note: scroll_offset 0 = bottom, higher = scrolled up
                                match mouse.kind {
                                    MouseEventKind::ScrollUp => {
                                        // Scroll up in the view (increase offset)
                                        self.scroll_offset = self.scroll_offset.saturating_add(3);
                                    }
                                    MouseEventKind::ScrollDown => {
                                        // Scroll down in the view (decrease offset towards 0)
                                        self.scroll_offset = self.scroll_offset.saturating_sub(3);
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                    }
                    // Handle background task completion notifications
                    bus_event = bus_receiver.recv() => {
                        if let Ok(BusEvent::BackgroundTaskCompleted(task)) = bus_event {
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
                                    self.messages.push(Message {
                                        role: Role::User,
                                        content: vec![ContentBlock::Text {
                                            text: notification,
                                            cache_control: None,
                                        }],
                                    });
                                    self.session.add_message(Role::User, vec![ContentBlock::Text {
                                        text: format!("[Background task {} completed]", task.task_id),
                                        cache_control: None,
                                    }]);
                                    let _ = self.session.save();
                                }
                            }
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
            exit_code: self.requested_exit_code,
        })
    }

    /// Run the TUI in remote mode, connecting to a server
    pub async fn run_remote(mut self, mut terminal: DefaultTerminal) -> Result<RunResult> {
        use super::backend::RemoteConnection;

        let mut event_stream = EventStream::new();
        let mut redraw_interval = interval(Duration::from_millis(50));
        let mut reconnect_attempts = 0u32;
        const MAX_RECONNECT_ATTEMPTS: u32 = 30;

        'outer: loop {
            // Determine which session to resume
            let session_to_resume = if reconnect_attempts == 0 {
                // First connect: use --resume argument if provided
                self.resume_session_id.take()
            } else {
                // Reconnecting after server reload: restore the session we had before
                self.remote_session_id.clone()
            };

            // Connect to server (with optional session resume)
            let mut remote = match RemoteConnection::connect_with_session(
                session_to_resume.as_deref(),
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    // Put session back if connect failed (for retry)
                    if reconnect_attempts == 0 && session_to_resume.is_some() {
                        self.resume_session_id = session_to_resume;
                    }
                    if reconnect_attempts == 0 {
                        return Err(anyhow::anyhow!(
                            "Failed to connect to server. Is `jcode serve` running? Error: {}",
                            e
                        ));
                    }
                    reconnect_attempts += 1;
                    if reconnect_attempts > MAX_RECONNECT_ATTEMPTS {
                        self.push_display_message(DisplayMessage::error(
                            "Failed to reconnect after 30 seconds. Press Ctrl+C to quit.",
                        ));
                        terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;
                        loop {
                            if let Some(Ok(Event::Key(key))) = event_stream.next().await {
                                if key.kind == KeyEventKind::Press {
                                    if key.code == KeyCode::Char('c')
                                        && key.modifiers.contains(KeyModifiers::CONTROL)
                                    {
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;
                    continue;
                }
            };

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
                    self.reload_requested = Some(session_id);
                    self.should_quit = true;
                    break 'outer;
                }

                // Build success message with reload info if available
                let reload_details = if !self.reload_info.is_empty() {
                    format!("\n  {}", self.reload_info.join("\n  "))
                } else {
                    String::new()
                };

                self.push_display_message(DisplayMessage::system(format!(
                    "✓ Reconnected successfully.{}",
                    reload_details
                )));

                // Queue message to notify the agent about the reload
                if !self.reload_info.is_empty() {
                    // Try to load reload context for richer continuation message
                    let reload_ctx = ReloadContext::load().ok().flatten();

                    let continuation_msg = if let Some(ctx) = reload_ctx {
                        let action = if ctx.is_rollback {
                            "Rollback"
                        } else {
                            "Reload"
                        };
                        let task_info = ctx
                            .task_context
                            .map(|t| format!("\nYou were working on: {}", t))
                            .unwrap_or_default();

                        format!(
                            "[{} complete. Previous version: {}, New version: {}.{}\nContinue with your task.]",
                            action,
                            ctx.version_before,
                            ctx.version_after,
                            task_info
                        )
                    } else {
                        // Fallback to basic message
                        let cwd = std::env::current_dir()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| "unknown".to_string());
                        let reload_summary = self.reload_info.join(", ");
                        format!(
                            "[Reload complete. {}. CWD: {}. Session restored - continue where you left off.]",
                            reload_summary, cwd
                        )
                    };

                    self.queued_messages.push(continuation_msg);
                    self.reload_info.clear();
                }
            }

            // Reset reconnect counter after handling reconnection
            reconnect_attempts = 0;

            // Main event loop
            loop {
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
                    }
                    event = remote.next_event() => {
                        match event {
                            None => {
                                // Server disconnected
                                self.is_processing = false;
                                self.push_display_message(DisplayMessage {
                                    role: "system".to_string(),
                                    content: "Server disconnected. Reconnecting...".to_string(),
                                    tool_calls: Vec::new(),
                                    duration_secs: None,
                                    title: None,
                                    tool_data: None,
                                });
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
                                    // Fall through to process queued messages (don't continue)
                                } else {
                                    let _at_safe_point = self.handle_server_event(server_event, &mut remote);
                                }

                                // Process pending interleave or queued messages
                                // If processing: only interleave via soft interrupt
                                // If not processing: send interleave or queued messages directly
                                if self.is_processing {
                                    // Use soft interrupt - no cancel, message injected at next safe point
                                    if let Some(interleave_msg) = self.interleave_message.take() {
                                        if !interleave_msg.trim().is_empty() {
                                            // Show in UI immediately for feedback
                                            self.push_display_message(DisplayMessage {
                                                role: "user".to_string(),
                                                content: format!("⏳ {}", interleave_msg),
                                                tool_calls: vec![],
                                                duration_secs: None,
                                                title: Some("(pending injection)".to_string()),
                                                tool_data: None,
                                            });
                                            // Send soft interrupt to server
                                            if let Err(e) = remote.soft_interrupt(interleave_msg, false).await {
                                                self.push_display_message(DisplayMessage::error(format!(
                                                    "Failed to queue soft interrupt: {}", e
                                                )));
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
                                            match remote.send_message(interleave_msg).await {
                                                Ok(msg_id) => {
                                                    self.current_message_id = Some(msg_id);
                                                    self.is_processing = true;
                                                    self.status = ProcessingStatus::Sending;
                                                    self.processing_started = Some(Instant::now());
                                                }
                                                Err(e) => {
                                                    self.push_display_message(DisplayMessage::error(format!(
                                                        "Failed to send message: {}", e
                                                    )));
                                                }
                                            }
                                        }
                                    } else if !self.queued_messages.is_empty() {
                                        let combined = std::mem::take(&mut self.queued_messages).join("\n\n");
                                        self.push_display_message(DisplayMessage {
                                            role: "user".to_string(),
                                            content: combined.clone(),
                                            tool_calls: vec![],
                                            duration_secs: None,
                                            title: None,
                                            tool_data: None,
                                        });
                                        if let Ok(msg_id) = remote.send_message(combined).await {
                                            self.current_message_id = Some(msg_id);
                                            self.is_processing = true;
                                            self.status = ProcessingStatus::Sending;
                                            self.processing_started = Some(Instant::now());
                                        }
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
                                }
                            }
                            Some(Ok(Event::Paste(text))) => {
                                self.handle_paste(text);
                            }
                            Some(Ok(Event::Mouse(mouse))) => {
                                // Handle mouse scroll wheel for scrolling
                                // Note: scroll_offset 0 = bottom, higher = scrolled up
                                match mouse.kind {
                                    MouseEventKind::ScrollUp => {
                                        // Scroll up in the view (increase offset)
                                        self.scroll_offset = self.scroll_offset.saturating_add(3);
                                    }
                                    MouseEventKind::ScrollDown => {
                                        // Scroll down in the view (decrease offset towards 0)
                                        self.scroll_offset = self.scroll_offset.saturating_sub(3);
                                    }
                                    _ => {}
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
            exit_code: self.requested_exit_code,
        })
    }

    /// Handle a server event. Returns true if we're at a "safe point" for interleaving
    /// (after a tool completes but before the turn ends).
    fn handle_server_event(
        &mut self,
        event: crate::protocol::ServerEvent,
        remote: &mut super::backend::RemoteConnection,
    ) -> bool {
        use crate::protocol::ServerEvent;

        match event {
            ServerEvent::TextDelta { text } => {
                if let Some(thought_line) = Self::extract_thought_line(&text) {
                    if let Some(chunk) = self.stream_buffer.flush() {
                        self.streaming_text.push_str(&chunk);
                    }
                    self.insert_thought_line(thought_line);
                    return false;
                }
                // Update status from Sending to Streaming on first text
                if matches!(self.status, ProcessingStatus::Sending) {
                    self.status = ProcessingStatus::Streaming;
                }
                if let Some(chunk) = self.stream_buffer.push(&text) {
                    self.streaming_text.push_str(&chunk);
                }
                self.last_stream_activity = Some(Instant::now());
                false
            }
            ServerEvent::ToolStart { id, name } => {
                remote.handle_tool_start(&id, &name);
                self.status = ProcessingStatus::RunningTool(name.clone());
                self.streaming_tool_calls.push(ToolCall {
                    id,
                    name,
                    input: serde_json::Value::Null,
                });
                false
            }
            ServerEvent::ToolInput { delta } => {
                remote.handle_tool_input(&delta);
                false
            }
            ServerEvent::ToolExec { id, name } => {
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
                    let content = std::mem::take(&mut self.streaming_text);
                    self.push_display_message(DisplayMessage {
                        role: "assistant".to_string(),
                        content,
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
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
            ServerEvent::Done { id } => {
                // Only process Done for our current message request
                // (ignore Done events for Subscribe, GetHistory, etc.)
                if self.current_message_id == Some(id) {
                    // Flush stream buffer
                    if let Some(chunk) = self.stream_buffer.flush() {
                        self.streaming_text.push_str(&chunk);
                    }
                    if !self.streaming_text.is_empty() {
                        let duration = self.processing_started.map(|s| s.elapsed().as_secs_f32());
                        let content = std::mem::take(&mut self.streaming_text);
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
                    self.is_processing = false;
                    self.status = ProcessingStatus::Idle;
                    self.processing_started = None;
                    self.streaming_tool_calls.clear();
                    self.current_message_id = None;
                    self.thought_line_inserted = false;
                    remote.clear_pending();
                }
                false
            }
            ServerEvent::Error { message, .. } => {
                self.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: message,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                self.is_processing = false;
                self.status = ProcessingStatus::Idle;
                self.interleave_message = None;
                self.thought_line_inserted = false;
                remote.clear_pending();
                false
            }
            ServerEvent::SessionId { session_id } => {
                remote.set_session_id(session_id.clone());
                self.remote_session_id = Some(session_id);
                false
            }
            ServerEvent::Reloading { .. } => {
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: "🔄 Server reload initiated...".to_string(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: Some("Reload".to_string()),
                    tool_data: None,
                });
                false
            }
            ServerEvent::ReloadProgress {
                step,
                message,
                success,
                output,
            } => {
                // Format the progress message with optional output
                let status_icon = match success {
                    Some(true) => "✓",
                    Some(false) => "✗",
                    None => "→",
                };

                let mut content = format!("[{}] {}", step, message);

                if let Some(out) = output {
                    if !out.is_empty() {
                        content.push_str("\n```\n");
                        content.push_str(&out);
                        content.push_str("\n```");
                    }
                }

                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: Some(format!("Reload: {} {}", status_icon, step)),
                    tool_data: None,
                });

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
                all_sessions,
                client_count,
                is_canary,
                server_version,
                server_has_update,
                ..
            } => {
                let prev_session_id = self.remote_session_id.clone();
                remote.set_session_id(session_id.clone());
                self.remote_session_id = Some(session_id.clone());
                let session_changed = prev_session_id.as_deref() != Some(session_id.as_str());

                if session_changed {
                    self.clear_display_messages();
                    self.streaming_text.clear();
                    self.streaming_tool_calls.clear();
                    self.thought_line_inserted = false;
                    self.streaming_input_tokens = 0;
                    self.streaming_output_tokens = 0;
                    self.streaming_cache_read_tokens = None;
                    self.streaming_cache_creation_tokens = None;
                    self.processing_started = None;
                    self.last_stream_activity = None;
                    self.is_processing = false;
                    self.status = ProcessingStatus::Idle;
                    self.scroll_offset = 0;
                    self.queued_messages.clear();
                    self.interleave_message = None;
                    self.remote_total_tokens = None;
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
                false
            }
            ServerEvent::ModelChanged { model, error, .. } => {
                if let Some(err) = error {
                    self.push_display_message(DisplayMessage::error(format!(
                        "Failed to switch model: {}",
                        err
                    )));
                    self.set_status_notice("Model switch failed");
                } else {
                    self.update_context_limit_for_model(&model);
                    self.remote_provider_model = Some(model.clone());
                    self.push_display_message(DisplayMessage::system(format!(
                        "✓ Switched to model: {}",
                        model
                    )));
                    self.set_status_notice(format!("Model → {}", model));
                }
                false
            }
            ServerEvent::SoftInterruptInjected {
                content,
                point,
                tools_skipped,
            } => {
                // Update status to show injection happened
                let skip_info = tools_skipped
                    .map(|n| format!(" ({} tools skipped)", n))
                    .unwrap_or_default();
                self.set_status_notice(format!(
                    "✓ Message injected at point {}{}",
                    point, skip_info
                ));

                // Update the pending message display to show it was injected
                // Find and update the "(pending injection)" message if present
                for msg in self.display_messages.iter_mut().rev() {
                    if msg.title.as_deref() == Some("(pending injection)")
                        && msg.content.contains(&content)
                    {
                        // Update to show it was injected
                        msg.content = content.clone();
                        msg.title = Some(format!("(injected at point {}{})", point, skip_info));
                        break;
                    }
                }
                false
            }
            ServerEvent::MemoryInjected { count } => {
                // Show notice that memory was injected
                let plural = if count == 1 { "memory" } else { "memories" };
                self.set_status_notice(format!("🧠 {} relevant {} injected", count, plural));
                false
            }
            _ => false,
        }
    }

    /// Handle keyboard input in remote mode
    async fn handle_remote_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: &mut super::backend::RemoteConnection,
    ) -> Result<()> {
        if let Some(direction) = self
            .model_switch_keys
            .direction_for(code.clone(), modifiers)
        {
            remote.cycle_model(direction).await?;
            return Ok(());
        }
        // Most key handling is the same as local mode
        // Handle Alt combos
        if modifiers.contains(KeyModifiers::ALT) {
            match code {
                KeyCode::Char('b') => {
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
                _ => {}
            }
        }

        // Handle configurable scroll keys (default: Alt+K/J/U/D)
        if let Some(amount) = self.scroll_keys.scroll_amount(code.clone(), modifiers) {
            let max_estimate = self.display_messages.len() * 100 + self.streaming_text.len();
            if amount < 0 {
                // Scroll up (increase offset)
                self.scroll_offset = (self.scroll_offset + (-amount) as usize).min(max_estimate);
            } else {
                // Scroll down (decrease offset)
                self.scroll_offset = self.scroll_offset.saturating_sub(amount as usize);
            }
            return Ok(());
        }

        // Shift+Tab: toggle diff view
        if code == KeyCode::BackTab {
            self.show_diffs = !self.show_diffs;
            let status = if self.show_diffs {
                "Diffs: ON"
            } else {
                "Diffs: OFF"
            };
            self.set_status_notice(status);
            return Ok(());
        }

        // Ctrl combos
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('c') | KeyCode::Char('d') => {
                    self.handle_quit_request();
                    return Ok(());
                }
                KeyCode::Char('r') => {
                    self.recover_session_without_tools();
                    return Ok(());
                }
                KeyCode::Char('l') if !self.is_processing => {
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
                    // Ctrl+Up: retrieve last queued message for editing
                    if self.input.is_empty() && !self.queued_messages.is_empty() {
                        if let Some(msg) = self.queued_messages.pop() {
                            self.input = msg;
                            self.cursor_pos = self.input.len();
                            self.set_status_notice("Retrieved queued message for editing");
                        }
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
                        let msg_id = remote.send_message(expanded).await?;
                        self.current_message_id = Some(msg_id);
                        self.is_processing = true;
                        self.status = ProcessingStatus::Sending;
                        self.processing_started = Some(Instant::now());
                        self.thought_line_inserted = false;
                    }
                    SendAction::Queue => {
                        self.queued_messages.push(expanded);
                    }
                    SendAction::Interleave => {
                        // Show in UI immediately for feedback
                        self.push_display_message(DisplayMessage {
                            role: "user".to_string(),
                            content: format!("⏳ {}", raw_input),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: Some("(pending injection)".to_string()),
                            tool_data: None,
                        });
                        // Send soft interrupt immediately
                        if let Err(e) = remote.soft_interrupt(expanded, false).await {
                            self.push_display_message(DisplayMessage::error(format!(
                                "Failed to queue soft interrupt: {}", e
                            )));
                        } else {
                            self.set_status_notice("⏭ Queued for injection");
                        }
                    }
                }
            }
            return Ok(());
        }

        // Regular keys
        match code {
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
                self.scroll_offset = 0;
                self.reset_tab_completion();
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                    self.reset_tab_completion();
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                    self.reset_tab_completion();
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
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
                if !self.input.is_empty() {
                    let raw_input = std::mem::take(&mut self.input);
                    let expanded = self.expand_paste_placeholders(&raw_input);
                    self.pasted_contents.clear();
                    self.cursor_pos = 0;
                    let trimmed = expanded.trim();

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
                            self.push_display_message(DisplayMessage::system(
                                "Reloading server with newer binary...".to_string(),
                            ));
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
                        self.push_display_message(DisplayMessage::system(
                            "Reloading server...".to_string(),
                        ));
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

                    // Handle /quit
                    if trimmed == "/quit" {
                        self.session.mark_closed();
                        let _ = self.session.save();
                        self.should_quit = true;
                        return Ok(());
                    }

                    // Handle /model commands (remote mode)
                    if trimmed == "/model" || trimmed == "/models" {
                        let current = self
                            .remote_provider_model
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string());

                        if self.remote_available_models.is_empty() {
                            self.push_display_message(DisplayMessage::system(format!(
                                "**Available models:**\n  • {} (current)\n\nUse `/model <name>` to switch.",
                                current
                            )));
                            return Ok(());
                        }

                        let model_list = self
                            .remote_available_models
                            .iter()
                            .map(|m| {
                                if m == &current {
                                    format!("  • **{}** (current)", m)
                                } else {
                                    format!("  • {}", m)
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");

                        self.push_display_message(DisplayMessage::system(format!(
                            "**Available models:**\n{}\n\nUse `/model <name>` to switch.",
                            model_list
                        )));
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
                        remote.set_model(model_name).await?;
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
                            let msg_id = remote.send_message(expanded).await?;
                            self.current_message_id = Some(msg_id);
                            self.is_processing = true;
                            self.status = ProcessingStatus::Sending;
                            self.processing_started = Some(Instant::now());
                            self.thought_line_inserted = false;
                        }
                        SendAction::Queue => {
                            self.queued_messages.push(expanded);
                        }
                        SendAction::Interleave => {
                            // Show in UI immediately for feedback
                            self.push_display_message(DisplayMessage {
                                role: "user".to_string(),
                                content: format!("⏳ {}", raw_input),
                                tool_calls: vec![],
                                duration_secs: None,
                                title: Some("(pending injection)".to_string()),
                                tool_data: None,
                            });
                            // Send soft interrupt immediately
                            if let Err(e) = remote.soft_interrupt(expanded, false).await {
                                self.push_display_message(DisplayMessage::error(format!(
                                    "Failed to queue soft interrupt: {}", e
                                )));
                            } else {
                                self.set_status_notice("⏭ Queued for injection");
                            }
                        }
                    }
                }
            }
            KeyCode::Up | KeyCode::PageUp => {
                // Scroll up (increase offset from bottom)
                let max_estimate = self.display_messages.len() * 100 + self.streaming_text.len();
                let inc = if code == KeyCode::PageUp { 10 } else { 1 };
                self.scroll_offset = (self.scroll_offset + inc).min(max_estimate);
            }
            KeyCode::Down | KeyCode::PageDown => {
                // Scroll down (decrease offset, 0 = bottom)
                let dec = if code == KeyCode::PageDown { 10 } else { 1 };
                self.scroll_offset = self.scroll_offset.saturating_sub(dec);
            }
            KeyCode::Esc => {
                if self.is_processing {
                    remote.cancel().await?;
                    self.set_status_notice("Interrupting...");
                } else {
                    self.scroll_offset = 0;
                    self.input.clear();
                    self.cursor_pos = 0;
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

        if let Err(e) = self.run_turn_interactive(terminal, event_stream).await {
            self.push_display_message(DisplayMessage {
                role: "error".to_string(),
                content: format!("Error: {}", e),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
        }

        // Process any queued messages
        self.process_queued_messages(terminal, event_stream).await;

        // Accumulate turn tokens into session totals
        self.total_input_tokens += self.streaming_input_tokens;
        self.total_output_tokens += self.streaming_output_tokens;

        self.is_processing = false;
        self.status = ProcessingStatus::Idle;
        self.processing_started = None;
        self.interleave_message = None;
        self.thought_line_inserted = false;
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
        if let Some(direction) = self
            .model_switch_keys
            .direction_for(code.clone(), modifiers)
        {
            self.cycle_model(direction);
            return Ok(());
        }
        // Handle Alt combos (readline word movement)
        if modifiers.contains(KeyModifiers::ALT) {
            match code {
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
                    return Ok(());
                }
                KeyCode::Backspace => {
                    // Alt+Backspace: delete word backward
                    let start = self.find_word_boundary_back();
                    self.input.drain(start..self.cursor_pos);
                    self.cursor_pos = start;
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
                _ => {}
            }
        }

        // Handle configurable scroll keys (default: Alt+K/J/U/D)
        if let Some(amount) = self.scroll_keys.scroll_amount(code.clone(), modifiers) {
            let max_estimate = self.display_messages.len() * 100 + self.streaming_text.len();
            if amount < 0 {
                // Scroll up (increase offset)
                self.scroll_offset = (self.scroll_offset + (-amount) as usize).min(max_estimate);
            } else {
                // Scroll down (decrease offset)
                self.scroll_offset = self.scroll_offset.saturating_sub(amount as usize);
            }
            return Ok(());
        }

        // Shift+Tab: toggle diff view
        if code == KeyCode::BackTab {
            self.show_diffs = !self.show_diffs;
            let status = if self.show_diffs {
                "Diffs: ON"
            } else {
                "Diffs: OFF"
            };
            self.set_status_notice(status);
            return Ok(());
        }

        // Handle ctrl combos regardless of processing state
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('c') | KeyCode::Char('d') => {
                    self.handle_quit_request();
                    return Ok(());
                }
                KeyCode::Char('r') => {
                    self.recover_session_without_tools();
                    return Ok(());
                }
                KeyCode::Char('l') if !self.is_processing => {
                    self.messages.clear();
                    self.clear_display_messages();
                    self.queued_messages.clear();
                    self.pasted_contents.clear();
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
                    return Ok(());
                }
                KeyCode::Char('k') => {
                    if self.input.is_empty() {
                        // Ctrl+K with empty input: scroll up to previous prompt
                        self.scroll_to_prev_prompt();
                    } else {
                        // Ctrl+K: kill to end of line
                        self.input.truncate(self.cursor_pos);
                    }
                    return Ok(());
                }
                KeyCode::Char('j') => {
                    if self.input.is_empty() {
                        // Ctrl+J with empty input: scroll down to next prompt
                        self.scroll_to_next_prompt();
                    }
                    // Note: Ctrl+J with text is ignored (traditionally newline)
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
                        self.cursor_pos -= 1;
                    }
                    return Ok(());
                }
                KeyCode::Char('f') => {
                    // Ctrl+F: forward one char
                    if self.cursor_pos < self.input.len() {
                        self.cursor_pos += 1;
                    }
                    return Ok(());
                }
                KeyCode::Char('w') => {
                    // Ctrl+W: delete word backward
                    let start = self.find_word_boundary_back();
                    self.input.drain(start..self.cursor_pos);
                    self.cursor_pos = start;
                    return Ok(());
                }
                KeyCode::Char('v') => {
                    // Ctrl+V: paste from clipboard
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            self.handle_paste(text);
                        }
                    }
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
                    // Ctrl+Up: retrieve last queued message for editing
                    if self.input.is_empty() && !self.queued_messages.is_empty() {
                        if let Some(msg) = self.queued_messages.pop() {
                            self.input = msg;
                            self.cursor_pos = self.input.len();
                            self.set_status_notice("Retrieved queued message for editing");
                        }
                    }
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
                        self.cursor_pos = 0;
                        // Set interleave_message so streaming code can pick it up
                        self.interleave_message = Some(expanded);
                        self.set_status_notice("⏭ Sending now (interleave)");
                    }
                }
            }
            return Ok(());
        }

        match code {
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    match self.send_action(false) {
                        SendAction::Submit => self.submit_input(),
                        SendAction::Queue => self.queue_message(),
                        SendAction::Interleave => {
                            let raw_input = std::mem::take(&mut self.input);
                            let expanded = self.expand_paste_placeholders(&raw_input);
                            self.pasted_contents.clear();
                            self.cursor_pos = 0;
                            // Set interleave_message so streaming code can pick it up
                            self.interleave_message = Some(expanded);
                            self.set_status_notice("⏭ Sending now (interleave)");
                        }
                    }
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
                self.reset_tab_completion();
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                    self.reset_tab_completion();
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                    self.reset_tab_completion();
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.input.len(),
            KeyCode::Tab => {
                // Autocomplete command suggestions
                self.autocomplete();
            }
            KeyCode::Up | KeyCode::PageUp => {
                // Scroll up (increase offset from bottom)
                // Use generous estimate - UI will clamp to actual content
                let max_estimate = self.display_messages.len() * 100 + self.streaming_text.len();
                let inc = if code == KeyCode::PageUp { 10 } else { 1 };
                self.scroll_offset = (self.scroll_offset + inc).min(max_estimate);
            }
            KeyCode::Down | KeyCode::PageDown => {
                // Scroll down (decrease offset, 0 = bottom)
                let dec = if code == KeyCode::PageDown { 10 } else { 1 };
                self.scroll_offset = self.scroll_offset.saturating_sub(dec);
            }
            KeyCode::Esc => {
                if self.is_processing {
                    // Interrupt generation
                    self.cancel_requested = true;
                    self.interleave_message = None;
                } else {
                    // Reset scroll to bottom and clear input
                    self.scroll_offset = 0;
                    self.input.clear();
                    self.cursor_pos = 0;
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Queue a message to be sent later
    /// Handle paste: store content and insert placeholder
    fn handle_paste(&mut self, text: String) {
        let line_count = text.lines().count().max(1);
        self.pasted_contents.push(text);
        let placeholder = format!(
            "[pasted {} line{}]",
            line_count,
            if line_count == 1 { "" } else { "s" }
        );
        self.input.insert_str(self.cursor_pos, &placeholder);
        self.cursor_pos += placeholder.len();
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
        self.cursor_pos = 0;
        self.queued_messages.push(expanded);
    }

    fn send_action(&self, shift: bool) -> SendAction {
        if !self.is_processing {
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

    /// Submit input - just sets up message and flags, processing happens in next loop iteration
    fn submit_input(&mut self) {
        let raw_input = std::mem::take(&mut self.input);
        let input = self.expand_paste_placeholders(&raw_input);
        self.pasted_contents.clear();
        self.cursor_pos = 0;
        self.scroll_offset = 0; // Reset to bottom on new input

        // Check for built-in commands
        let trimmed = input.trim();
        if trimmed == "/help" || trimmed == "/?" {
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
                     • `/config` - Show current configuration\n\
                     • `/config init` - Create default config file (~/.jcode/config.toml)\n\
                     • `/config edit` - Open config file in $EDITOR\n\
                     • `/model` - List available models\n\
                     • `/model <name>` - Switch to a different model\n\
                     • `/reload` - Smart reload (client/server if newer binary exists)\n\
                     • `/rebuild` - Full rebuild (git pull + cargo build + tests){}\n\
                     • `/clear` - Clear conversation (Ctrl+L)\n\
                     • `/debug-visual` - Enable visual debugging for TUI issues\n\
                     • `/<skill>` - Activate a skill\n\n\
                     **Available skills:** {}\n\n\
                     **Keyboard shortcuts:**\n\
                     • `Ctrl+C` / `Ctrl+D` - Quit (press twice to confirm)\n\
                     • `Ctrl+L` - Clear conversation\n\
                     • `Ctrl+R` - Recover from missing tool outputs\n\
                     • `PageUp/Down` or `Up/Down` - Scroll history\n\
                     • `{}`/`{}` - Scroll up/down (see `/config`)\n\
                     • `{}`/`{}` - Page up/down (see `/config`)\n\
                     • `Ctrl+Tab` / `Ctrl+T` - Toggle queue mode (wait vs immediate send)\n\
                     • `Ctrl+Up` - Retrieve queued message for editing\n\
                     • `Ctrl+U` - Clear input line\n\
                     • `Ctrl+K` - Kill to end of line\n\
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
            self.messages.clear();
            self.clear_display_messages();
            self.queued_messages.clear();
            self.pasted_contents.clear();
            self.active_skill = None;
            let mut session = Session::create(None, None);
            session.model = Some(self.provider.model());
            self.session = session;
            self.provider_session_id = None;
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

        // Handle /model command
        if trimmed == "/model" || trimmed == "/models" {
            // List available models
            let models = self.provider.available_models_display();
            let current = self.provider.model();
            let model_list = if models.is_empty() {
                format!("  • {} (current)", current)
            } else {
                models
                    .iter()
                    .map(|m| {
                        if m == &current {
                            format!("  • **{}** (current)", m)
                        } else {
                            format!("  • {}", m)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };

            self.push_display_message(DisplayMessage {
                role: "system".to_string(),
                content: format!(
                    "**Available models:**\n{}\n\nUse `/model <name>` to switch.",
                    model_list
                ),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            return;
        }

        if let Some(model_name) = trimmed.strip_prefix("/model ") {
            let model_name = model_name.trim();
            match self.provider.set_model(model_name) {
                Ok(()) => {
                    self.provider_session_id = None;
                    self.session.provider_session_id = None;
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
        self.messages.push(Message::user(&input));
        self.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: input.clone(),
                cache_control: None,
            }],
        );
        let _ = self.session.save();

        // Set up processing state - actual processing happens after UI redraws
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.streaming_text.clear();
        self.streaming_md_renderer.borrow_mut().reset();
        self.stream_buffer.clear();
        self.thought_line_inserted = false;
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.streaming_cache_read_tokens = None;
        self.streaming_cache_creation_tokens = None;
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
            let combined = std::mem::take(&mut self.queued_messages).join("\n\n");

            // Add user message to display
            self.push_display_message(DisplayMessage {
                role: "user".to_string(),
                content: combined.clone(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });

            self.messages.push(Message::user(&combined));
            self.session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: combined,
                    cache_control: None,
                }],
            );
            let _ = self.session.save();
            self.streaming_text.clear();
            self.stream_buffer.clear();
            self.thought_line_inserted = false;
            self.streaming_tool_calls.clear();
            self.streaming_input_tokens = 0;
            self.streaming_output_tokens = 0;
            self.streaming_cache_read_tokens = None;
            self.streaming_cache_creation_tokens = None;
            self.processing_started = Some(Instant::now());
            self.status = ProcessingStatus::Sending;

            if let Err(e) = self.run_turn_interactive(terminal, event_stream).await {
                self.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Error: {}", e),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
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

    fn update_context_limit_for_model(&mut self, model: &str) {
        let limit = crate::provider::context_limit_for_model(model)
            .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT);
        self.context_limit = limit as u64;
        self.context_warning_shown = false;
    }

    fn set_status_notice(&mut self, text: impl Into<String>) {
        self.status_notice = Some((text.into(), Instant::now()));
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

    fn summarize_tool_results_missing(&self) -> Option<String> {
        if self.tool_result_ids.is_empty() {
            return None;
        }
        let mut known_ids = HashSet::new();
        for msg in &self.messages {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        known_ids.insert(tool_use_id.clone());
                    }
                }
            }
        }
        let missing: Vec<String> = self
            .tool_result_ids
            .difference(&known_ids)
            .cloned()
            .collect();
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

        self.messages.clear();
        self.clear_display_messages();
        self.queued_messages.clear();
        self.pasted_contents.clear();
        self.active_skill = None;
        self.provider_session_id = None;
        self.tool_result_ids.clear();
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
            self.messages.push(Message {
                role: role.clone(),
                content: kept_blocks.clone(),
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
            if let Some(summary) = self.summarize_tool_results_missing() {
                let message = format!(
                    "Tool outputs are missing for this turn. {}\n\nPress Ctrl+R to recover into a new session with context copied.",
                    summary
                );
                self.push_display_message(DisplayMessage::error(message));
                self.set_status_notice("Recovery needed");
                return Ok(());
            }

            let tools = self.registry.definitions(None).await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_prompt = self.build_memory_prompt_nonblocking(&self.messages);
            // Build system prompt with active skill
            let system_prompt = self.build_system_prompt(memory_prompt.as_deref());

            self.status = ProcessingStatus::Sending;
            let mut stream = self
                .provider
                .complete(
                    &self.messages,
                    &tools,
                    &system_prompt,
                    self.provider_session_id.as_deref(),
                )
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut first_event = true;
            let mut saw_message_end = false;
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
                        // Use semantic buffer for chunked display
                        if let Some(chunk) = self.stream_buffer.push(&text) {
                            self.streaming_text.push_str(&chunk);
                        }
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
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
                                self.streaming_text.clear();
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
                        if let Some(input) = input_tokens {
                            self.streaming_input_tokens = input;
                            // Warn when approaching context limit (80%)
                            self.check_context_warning(input);
                        }
                        if let Some(output) = output_tokens {
                            self.streaming_output_tokens = output;
                        }
                        if cache_read_input_tokens.is_some() {
                            self.streaming_cache_read_tokens = cache_read_input_tokens;
                        }
                        if cache_creation_input_tokens.is_some() {
                            self.streaming_cache_creation_tokens = cache_creation_input_tokens;
                        }
                    }
                    StreamEvent::MessageEnd { .. } => {
                        saw_message_end = true;
                        // Don't break yet - wait for SessionId
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
                            self.streaming_text.clear();
                            return Ok(());
                        }
                        return Err(anyhow::anyhow!("Stream error: {}", message));
                    }
                    StreamEvent::ThinkingStart => {
                        // Track start but don't display - wait for ThinkingDone
                        self.thinking_start = Some(Instant::now());
                    }
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Display reasoning/thinking content from OpenAI
                        // Flush any pending text first
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        // Insert thinking content as a thought line
                        self.insert_thought_line(format!("💭 {}", thinking_text));
                    }
                    StreamEvent::ThinkingEnd => {
                        // Don't display here - ThinkingDone has accurate timing
                        self.thinking_start = None;
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        // Flush any pending buffered text first
                        if let Some(chunk) = self.stream_buffer.flush() {
                            self.streaming_text.push_str(&chunk);
                        }
                        // Bridge provides accurate wall-clock timing
                        let thinking_msg = format!("*Thought for {:.1}s*", duration_secs);
                        self.insert_thought_line(thinking_msg);
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
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                let content_clone = content_blocks.clone();
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: content_blocks,
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
            self.streaming_text.clear();
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

                self.messages
                    .push(Message::tool_result(&tc.id, &output, is_error));
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
        let mut redraw_interval = interval(Duration::from_millis(50));

        loop {
            if let Some(summary) = self.summarize_tool_results_missing() {
                let message = format!(
                    "Tool outputs are missing for this turn. {}\n\nPress Ctrl+R to recover into a new session with context copied.",
                    summary
                );
                self.push_display_message(DisplayMessage::error(message));
                self.set_status_notice("Recovery needed");
                return Ok(());
            }

            let tools = self.registry.definitions(None).await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_prompt = self.build_memory_prompt_nonblocking(&self.messages);
            let system_prompt = self.build_system_prompt(memory_prompt.as_deref());

            self.status = ProcessingStatus::Sending;
            terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;

            crate::logging::info(&format!(
                "TUI: API call starting ({} messages)",
                self.messages.len()
            ));
            let api_start = std::time::Instant::now();

            // Clone data needed for the API call to avoid borrow issues
            // The future would hold references across the select! which conflicts with handle_key
            let provider = self.provider.clone();
            let messages_clone = self.messages.clone();
            let session_id_clone = self.provider_session_id.clone();

            // Make API call non-blocking - poll it in select! so we can handle input while waiting
            let mut api_future = std::pin::pin!(provider.complete(
                &messages_clone,
                &tools,
                &system_prompt,
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
                                        self.push_display_message(DisplayMessage {
                                            role: "system".to_string(),
                                            content: "Interrupted".to_string(),
                                            tool_calls: vec![],
                                            duration_secs: None,
                                            title: None,
                                            tool_data: None,
                                        });
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
            let mut interleaved = false; // Track if we interleaved a message mid-stream
                                         // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();

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
                                        self.push_display_message(DisplayMessage {
                                            role: "system".to_string(),
                                            content: "Interrupted".to_string(),
                                            tool_calls: vec![],
                                            duration_secs: None,
                                            title: None,
                                            tool_data: None,
                                        });
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
                                            for tc in &tool_calls {
                                                content_blocks.push(ContentBlock::ToolUse {
                                                    id: tc.id.clone(),
                                                    name: tc.name.clone(),
                                                    input: tc.input.clone(),
                                                });
                                            }
                                            // Add partial assistant response to messages
                                            if !content_blocks.is_empty() {
                                                self.messages.push(Message {
                                                    role: Role::Assistant,
                                                    content: content_blocks,
                                                });
                                            }
                                            // Add display message for partial response
                                            if !self.streaming_text.is_empty() {
                                                let content = std::mem::take(&mut self.streaming_text);
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
                                        self.messages.push(Message::user(&interleave_msg));
                                        self.push_display_message(DisplayMessage {
                                            role: "user".to_string(),
                                            content: interleave_msg,
                                            tool_calls: vec![],
                                            duration_secs: None,
                                            title: None,
                                            tool_data: None,
                                        });
                                        // Clear streaming state and continue with new turn
                                        self.streaming_text.clear();
                                        self.streaming_tool_calls.clear();
                                        self.stream_buffer = StreamBuffer::new();
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
                                        // Use semantic buffer for chunked display
                                        if let Some(chunk) = self.stream_buffer.push(&text) {
                                            self.streaming_text.push_str(&chunk);
                                            // Broadcast buffered text
                                            self.broadcast_debug(super::backend::DebugEvent::TextDelta {
                                                text: chunk.clone()
                                            });
                                        }
                                    }
                                    StreamEvent::ToolUseStart { id, name } => {
                                        self.broadcast_debug(super::backend::DebugEvent::ToolStart {
                                            id: id.clone(),
                                            name: name.clone(),
                                        });
                                        // Update status to show tool in progress
                                        self.status = ProcessingStatus::RunningTool(name.clone());
                                        self.streaming_tool_calls.push(ToolCall {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: serde_json::Value::Null,
                                        });
                                        current_tool = Some(ToolCall {
                                            id,
                                            name,
                                            input: serde_json::Value::Null,
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
                                                self.streaming_text.clear();
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
                                        if let Some(input) = input_tokens {
                                            self.streaming_input_tokens = input;
                                            self.check_context_warning(input);
                                        }
                                        if let Some(output) = output_tokens {
                                            self.streaming_output_tokens = output;
                                        }
                                        if cache_read_input_tokens.is_some() {
                                            self.streaming_cache_read_tokens = cache_read_input_tokens;
                                        }
                                        if cache_creation_input_tokens.is_some() {
                                            self.streaming_cache_creation_tokens =
                                                cache_creation_input_tokens;
                                        }
                                        self.broadcast_debug(super::backend::DebugEvent::TokenUsage {
                                            input_tokens: self.streaming_input_tokens,
                                            output_tokens: self.streaming_output_tokens,
                                            cache_read_input_tokens: self.streaming_cache_read_tokens,
                                            cache_creation_input_tokens: self
                                                .streaming_cache_creation_tokens,
                                        });
                                    }
                                    StreamEvent::MessageEnd { .. } => {
                                        saw_message_end = true;
                                        // Don't break yet - wait for SessionId
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
                                        self.thinking_start = Some(Instant::now());
                                        self.broadcast_debug(super::backend::DebugEvent::ThinkingStart);
                                    }
                                    StreamEvent::ThinkingDelta(thinking_text) => {
                                        // Display reasoning/thinking content from OpenAI
                                        if let Some(chunk) = self.stream_buffer.flush() {
                                            self.streaming_text.push_str(&chunk);
                                        }
                                        self.insert_thought_line(format!("💭 {}", thinking_text));
                                    }
                                    StreamEvent::ThinkingEnd => {
                                        self.thinking_start = None;
                                        self.broadcast_debug(super::backend::DebugEvent::ThinkingEnd);
                                    }
                                    StreamEvent::ThinkingDone { duration_secs } => {
                                        // Flush any pending buffered text first
                                        if let Some(chunk) = self.stream_buffer.flush() {
                                            self.streaming_text.push_str(&chunk);
                                        }
                                        let thinking_msg = format!("*Thought for {:.1}s*", duration_secs);
                                        self.insert_thought_line(thinking_msg);
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
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                let content_clone = content_blocks.clone();
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: content_blocks,
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
            self.streaming_text.clear();
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

                    self.messages.push(Message {
                        role: Role::User,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: sdk_content,
                            is_error: if sdk_is_error { Some(true) } else { None },
                        }],
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
                                            self.push_display_message(DisplayMessage {
                                                role: "system".to_string(),
                                                content: "Interrupted".to_string(),
                                                tool_calls: vec![],
                                                duration_secs: None,
                                                title: None,
                                                tool_data: None,
                                            });
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
                                self.subagent_status = Some(status.status);
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

                self.messages
                    .push(Message::tool_result(&tc.id, &output, is_error));
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
        let (prompt, context_info) = crate::prompt::build_system_prompt_with_context_and_memory(
            skill_prompt.as_deref(),
            &available_skills,
            self.session.is_canary,
            memory_prompt,
        );
        self.context_info = context_info;
        prompt
    }

    /// Get memory prompt using async non-blocking approach
    /// Takes any pending memory from background check and sends context to memory agent for next turn
    fn build_memory_prompt_nonblocking(&self, messages: &[Message]) -> Option<String> {
        if self.is_remote {
            return None;
        }

        // Take pending memory if available (computed in background during last turn)
        let pending = crate::memory::take_pending_memory();

        // Send context to memory agent for the NEXT turn (doesn't block current send)
        crate::memory_agent::update_context_sync(messages.to_vec());

        // Return pending memory from previous turn
        pending.map(|p| p.prompt)
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
        if self.is_remote || self.messages.len() < 4 {
            return;
        }

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
                            format!("{}...", &content[..200])
                        } else {
                            content.clone()
                        };
                        transcript.push_str(&format!("[Result: {}]\n", preview));
                    }
                }
            }
            transcript.push('\n');
        }

        // Extract memories using sidecar
        let sidecar = crate::sidecar::HaikuSidecar::new();
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
                        embedding: None, // Will be generated when stored
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
        self.display_messages.push(message);
        self.bump_display_messages_version();
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
        let bytes = self.input.as_bytes();
        let mut pos = self.cursor_pos - 1;

        // Skip trailing whitespace
        while pos > 0 && bytes[pos].is_ascii_whitespace() {
            pos -= 1;
        }

        // Skip word characters
        while pos > 0 && !bytes[pos - 1].is_ascii_whitespace() {
            pos -= 1;
        }

        pos
    }

    /// Find word boundary going forward (for Alt+F, Alt+D)
    fn find_word_boundary_forward(&self) -> usize {
        let len = self.input.len();
        if self.cursor_pos >= len {
            return len;
        }
        let bytes = self.input.as_bytes();
        let mut pos = self.cursor_pos;

        // Skip current word
        while pos < len && !bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        // Skip whitespace
        while pos < len && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        pos
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    /// Get command suggestions based on current input (or base input for cycling)
    fn get_suggestions_for(&self, input: &str) -> Vec<(String, &'static str)> {
        let input = input.trim();

        // Only show suggestions when input starts with /
        if !input.starts_with('/') {
            return vec![];
        }

        let prefix = input.to_lowercase();

        // Get available models
        let models: Vec<String> = if self.is_remote {
            self.remote_available_models.clone()
        } else {
            self.provider.available_models_display()
        };

        // If input is exactly "/model", show all model options for cycling
        if prefix == "/model" {
            return models
                .into_iter()
                .map(|m| (format!("/model {}", m), "Switch to this model"))
                .collect();
        }

        // Check if this is a "/model " command with a partial model name
        if let Some(model_prefix) = prefix.strip_prefix("/model ") {
            return models
                .into_iter()
                .filter(|m| model_prefix.is_empty() || m.to_lowercase().starts_with(model_prefix))
                .map(|m| (format!("/model {}", m), "Switch to this model"))
                .collect();
        }

        // Built-in commands
        let mut commands: Vec<(String, &'static str)> = vec![
            ("/help".into(), "Show help and keyboard shortcuts"),
            ("/model".into(), "List or switch models"),
            ("/clear".into(), "Clear conversation history"),
            ("/version".into(), "Show current version"),
            ("/info".into(), "Show session info and tokens"),
            ("/reload".into(), "Smart reload (if newer binary exists)"),
            ("/rebuild".into(), "Full rebuild (git pull + build + tests)"),
            ("/quit".into(), "Exit jcode"),
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
        commands
            .into_iter()
            .filter(|(cmd, _)| cmd.to_lowercase().starts_with(&prefix))
            .collect()
    }

    /// Get command suggestions based on current input
    pub fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        self.get_suggestions_for(&self.input)
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

        // If only one suggestion and it matches exactly, nothing to do
        if current_suggestions.len() == 1 && current_suggestions[0].0 == self.input {
            self.tab_completion_state = None;
            return false;
        }

        // Apply first suggestion and start tracking the cycle
        let (cmd, _) = &current_suggestions[0];
        let base = self.input.clone();
        self.input = cmd.clone();
        self.cursor_pos = self.input.len();
        self.tab_completion_state = Some((base, 0));
        true
    }

    /// Reset tab completion state (call when user types/modifies input)
    pub fn reset_tab_completion(&mut self) {
        self.tab_completion_state = None;
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
        if self.streaming_input_tokens > 0 || self.streaming_output_tokens > 0 {
            parts.push(format!(
                "↑{} ↓{}",
                self.streaming_input_tokens, self.streaming_output_tokens
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
        if self.streaming_input_tokens == 0 {
            0.0
        } else {
            (self.streaming_input_tokens as f64 / self.context_limit as f64) * 100.0
        }
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
        self.processing_started.map(|t| t.elapsed())
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn provider_model(&self) -> String {
        self.provider.model()
    }

    pub fn mcp_servers(&self) -> &[String] {
        &self.mcp_server_names
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
                // Scroll to put this message near top of view
                self.scroll_offset = *pos;
                return;
            }
        }

        // If no more user messages above, scroll to top
        if let Some((_, max_pos)) = positions.last() {
            self.scroll_offset = *max_pos;
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
                    return;
                }
                prev_user_pos = *pos;
            }
        }

        // No user message found below, go to bottom
        self.scroll_offset = 0;
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
            mcp_servers: self.mcp_server_names.clone(),
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
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixListener;

        let socket_path = Self::debug_socket_path();
        let initial_snapshot = self.create_debug_snapshot();

        tokio::spawn(async move {
            // Clean up old socket
            let _ = std::fs::remove_file(&socket_path);

            let listener = match UnixListener::bind(&socket_path) {
                Ok(l) => l,
                Err(e) => {
                    crate::logging::error(&format!("Failed to bind debug socket: {}", e));
                    return;
                }
            };

            // Accept connections and forward events
            let clients: std::sync::Arc<tokio::sync::Mutex<Vec<tokio::net::unix::OwnedWriteHalf>>> =
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
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        std::path::PathBuf::from(runtime_dir).join("jcode-debug.sock")
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

    fn scroll_offset(&self) -> usize {
        self.scroll_offset
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

    fn mcp_servers(&self) -> Vec<String> {
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

    fn streaming_tool_calls(&self) -> Vec<ToolCall> {
        self.streaming_tool_calls.clone()
    }

    fn elapsed(&self) -> Option<std::time::Duration> {
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

    fn show_diffs(&self) -> bool {
        self.show_diffs
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
            // For remote mode, extract name from session ID
            self.remote_session_id
                .as_ref()
                .and_then(|id| crate::id::extract_session_name(id))
                .map(|s| s.to_string())
        } else {
            Some(self.session.display_name().to_string())
        }
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
            for msg in &self.messages {
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

        let todos = session_id
            .and_then(|id| crate::todo::load_todos(id).ok())
            .unwrap_or_default();

        let context_info = self.context_info();
        let context_info = if context_info.total_chars > 0 {
            Some(context_info)
        } else {
            None
        };

        let (model, reasoning_effort) = if self.is_remote {
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

        // Gather memory info
        let memory_info = {
            use crate::memory::MemoryManager;

            let manager = MemoryManager::new();
            let project = manager.load_project().ok();
            let global = manager.load_global().ok();

            let (project_count, global_count, by_category) = match (project, global) {
                (Some(p), Some(g)) => {
                    let project_count = p.entries.len();
                    let global_count = g.entries.len();
                    let mut by_category = std::collections::HashMap::new();
                    for entry in p.entries.iter().chain(g.entries.iter()) {
                        *by_category.entry(entry.category.to_string()).or_insert(0) += 1;
                    }
                    (project_count, global_count, by_category)
                }
                _ => (0, 0, std::collections::HashMap::new()),
            };

            let total_count = project_count + global_count;
            let activity = crate::memory::get_activity();

            // Show memory info if we have memories OR if there's activity (agent working)
            if total_count > 0 || activity.is_some() {
                Some(super::info_widget::MemoryInfo {
                    total_count,
                    project_count,
                    global_count,
                    by_category,
                    sidecar_available: true,
                    activity,
                })
            } else {
                None
            }
        };

        // Gather swarm info
        let swarm_info = {
            let subagent_status = self.subagent_status.clone();
            let (session_count, client_count, session_names) = if self.is_remote {
                (
                    self.remote_sessions.len(),
                    self.remote_client_count,
                    self.remote_sessions.clone(),
                )
            } else {
                // In local mode, just show current session
                (1, None, vec![self.session.id.clone()])
            };

            // Only show if there's something interesting
            if subagent_status.is_some() || session_count > 1 || client_count.is_some() {
                Some(super::info_widget::SwarmInfo {
                    session_count,
                    subagent_status,
                    client_count,
                    session_names,
                })
            } else {
                None
            }
        };

        // Gather background task info
        let background_info = {
            let memory_agent_active = crate::memory_agent::is_active();

            // Get running background tasks count
            let bg_manager = crate::background::global();
            // We can't easily get running count without async, so just check if memory agent is active
            // Background tasks will show via swarm_info subagent_status

            if memory_agent_active {
                Some(super::info_widget::BackgroundInfo {
                    running_count: 0, // TODO: track this properly
                    running_tasks: Vec::new(),
                    memory_agent_active,
                    memory_agent_turns: 0, // TODO: expose this from memory_agent
                })
            } else {
                None
            }
        };

        // Gather subscription usage info (only for OAuth providers)
        let usage_info = {
            // Check if current provider uses OAuth (Anthropic OAuth or OpenAI Codex)
            let provider_name = self.provider.name().to_lowercase();
            let is_oauth_provider = provider_name.contains("anthropic") || provider_name.contains("claude");

            if is_oauth_provider {
                let usage = crate::usage::get_sync();
                if usage.fetched_at.is_some() {
                    Some(super::info_widget::UsageInfo {
                        provider: super::info_widget::UsageProvider::Anthropic,
                        five_hour: usage.five_hour,
                        seven_day: usage.seven_day,
                        available: true,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        };

        super::info_widget::InfoWidgetData {
            todos,
            context_info,
            queue_mode: Some(self.queue_mode),
            context_limit: Some(self.context_limit as usize),
            model,
            reasoning_effort,
            session_count,
            client_count,
            memory_info,
            swarm_info,
            background_info,
            usage_info,
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
        app.show_diffs = true;
        app
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
        app.handle_key(KeyCode::Enter, KeyModifiers::empty()).unwrap();

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

        assert_eq!(app.input(), "[pasted 1 line]");
        assert_eq!(app.cursor_pos(), 15);
        assert_eq!(app.pasted_contents.len(), 1);
        assert_eq!(app.pasted_contents[0], "hello world");
    }

    #[test]
    fn test_handle_paste_multi_line() {
        let mut app = create_test_app();

        app.handle_paste("line 1\nline 2\nline 3".to_string());

        assert_eq!(app.input(), "[pasted 3 lines]");
        assert_eq!(app.cursor_pos(), 16);
        assert_eq!(app.pasted_contents.len(), 1);
    }

    #[test]
    fn test_paste_expansion_on_submit() {
        let mut app = create_test_app();

        // Type prefix, paste, type suffix
        app.handle_key(KeyCode::Char('A'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char(':'), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
            .unwrap();
        app.handle_paste("pasted content".to_string());
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
            .unwrap();
        app.handle_key(KeyCode::Char('B'), KeyModifiers::empty())
            .unwrap();

        // Input shows placeholder
        assert_eq!(app.input(), "A: [pasted 1 line] B");

        // Submit expands placeholder
        app.submit_input();

        // Display shows placeholder (user sees condensed view)
        assert_eq!(app.display_messages().len(), 1);
        assert_eq!(app.display_messages()[0].content, "A: [pasted 1 line] B");

        // Model receives expanded content (actual pasted text)
        assert_eq!(app.messages.len(), 1);
        match &app.messages[0].content[0] {
            crate::message::ContentBlock::Text { text, .. } => {
                assert_eq!(text, "A: pasted content B");
            }
            _ => panic!("Expected Text content block"),
        }

        // Pasted contents should be cleared
        assert!(app.pasted_contents.is_empty());
    }

    #[test]
    fn test_multiple_pastes() {
        let mut app = create_test_app();

        app.handle_paste("first".to_string());
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
            .unwrap();
        app.handle_paste("second\nline".to_string());

        assert_eq!(app.input(), "[pasted 1 line] [pasted 2 lines]");
        assert_eq!(app.pasted_contents.len(), 2);

        app.submit_input();
        // Display shows placeholders (user sees condensed view)
        assert_eq!(
            app.display_messages()[0].content,
            "[pasted 1 line] [pasted 2 lines]"
        );
        // Model receives expanded content
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
    fn test_has_newer_binary_detection() {
        use std::time::{Duration, SystemTime};

        let mut app = create_test_app();
        let Some(repo_dir) = crate::build::get_repo_dir() else {
            return;
        };
        let exe = repo_dir.join("target/release/jcode");

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
        let Some(repo_dir) = crate::build::get_repo_dir() else {
            return;
        };
        let exe = repo_dir.join("target/release/jcode");

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
    fn test_debug_command_message_respects_queue_mode() {
        let mut app = create_test_app();

        // Test 1: When not processing, should submit directly
        app.is_processing = false;
        let result = app.handle_debug_command("message:hello");
        assert!(result.starts_with("OK: submitted message"), "Expected submitted, got: {}", result);
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
        assert!(result.contains("queued"), "Expected queued, got: {}", result);
        assert_eq!(app.queued_count(), 1);
        assert_eq!(app.queued_messages()[0], "queued_msg");

        // Test 3: When processing with queue_mode=false, should interleave
        app.queued_messages.clear();
        app.queue_mode = false;
        let result = app.handle_debug_command("message:interleave_msg");
        assert!(result.contains("interleave"), "Expected interleave, got: {}", result);
        assert_eq!(app.interleave_message.as_deref(), Some("interleave_msg"));
    }
}
