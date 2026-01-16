#![allow(dead_code)]

#![allow(dead_code)]

use super::keybind::ModelSwitchKeys;
use super::stream_buffer::StreamBuffer;
use crate::bus::{Bus, BusEvent, ToolEvent, ToolStatus};
use crate::mcp::McpManager;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolCall};
use crate::provider::Provider;
use crate::session::Session;
use crate::skill::SkillRegistry;
use crate::tool::{Registry, ToolContext};
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::path::PathBuf;
use tokio::sync::RwLock;
use tokio::time::interval;

/// Debug command file path
fn debug_cmd_path() -> PathBuf {
    std::env::temp_dir().join("jcode_debug_cmd")
}

/// Debug response file path
fn debug_response_path() -> PathBuf {
    std::env::temp_dir().join("jcode_debug_response")
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
    /// Session ID to reload (hot-reload)
    pub reload_session: Option<String>,
    /// Exit code to use (for canary wrapper communication)
    pub exit_code: Option<i32>,
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
    // Total session token usage (accumulated across all turns)
    total_input_tokens: u64,
    total_output_tokens: u64,
    // Context limit tracking (for compaction warning)
    context_limit: u64,
    context_warning_shown: bool,
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
    // Cached MCP server names (updated on connect/disconnect)
    mcp_server_names: Vec<String>,
    // Semantic stream buffer for chunked output
    stream_buffer: StreamBuffer,
    // Track thinking start time for extended thinking display
    thinking_start: Option<Instant>,
    // Hot-reload: if set, exec into new binary with this session ID
    reload_requested: Option<String>,
    // Pasted content storage (displayed as placeholders, expanded on submit)
    pasted_contents: Vec<String>,
    // Debug socket broadcast channel (if enabled)
    debug_tx: Option<tokio::sync::broadcast::Sender<super::backend::DebugEvent>>,
    // Remote provider info (set when running in remote mode)
    remote_provider_name: Option<String>,
    remote_provider_model: Option<String>,
    // Remote MCP servers and skills (set from server in remote mode)
    remote_mcp_servers: Vec<String>,
    remote_skills: Vec<String>,
    // Total session token usage (from server in remote mode)
    remote_total_tokens: Option<(u64, u64)>,
    // Current message request ID (for remote mode - to match Done events)
    current_message_id: Option<u64>,
    // Whether running in remote mode
    is_remote: bool,
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
    // Keybindings for model switching
    model_switch_keys: ModelSwitchKeys,
    // Short-lived notice for model switching feedback
    model_switch_notice: Option<(String, Instant)>,
}

/// A placeholder provider for remote mode (never actually called)
struct NullProvider;

#[async_trait::async_trait]
impl Provider for NullProvider {
    fn name(&self) -> &str { "remote" }
    fn model(&self) -> String { "unknown".to_string() }

    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _session_id: Option<&str>,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Err(anyhow::anyhow!("NullProvider cannot be used for completion"))
    }
}

impl App {
    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let skills = SkillRegistry::load().unwrap_or_default();
        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
        Self {
            provider,
            registry,
            skills,
            mcp_manager,
            messages: Vec::new(),
            session: Session::create(None, None),
            display_messages: Vec::new(),
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
            total_input_tokens: 0,
            total_output_tokens: 0,
            context_limit: 200_000, // Claude's context window
            context_warning_shown: false,
            last_stream_activity: None,
            status: ProcessingStatus::default(),
            subagent_status: None,
            processing_started: None,
            pending_turn: false,
            streaming_tool_calls: Vec::new(),
            provider_session_id: None,
            cancel_requested: false,
            mcp_server_names: Vec::new(),
            stream_buffer: StreamBuffer::new(),
            thinking_start: None,
            reload_requested: None,
            pasted_contents: Vec::new(),
            debug_tx: None,
            remote_provider_name: None,
            remote_provider_model: None,
            remote_mcp_servers: Vec::new(),
            remote_skills: Vec::new(),
            remote_total_tokens: None,
            current_message_id: None,
            is_remote: false,
            remote_session_id: None,
            remote_sessions: Vec::new(),
            known_stable_version: crate::build::read_stable_version().ok().flatten(),
            last_version_check: Some(Instant::now()),
            pending_migration: None,
            remote_client_count: None,
            resume_session_id: None,
            requested_exit_code: None,
            show_diffs: true, // Default to showing diffs
            model_switch_keys: super::keybind::load_model_switch_keys(),
            model_switch_notice: None,
        }
    }

    /// Create an App instance for remote mode (connecting to server)
    pub async fn new_for_remote(resume_session: Option<String>) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(NullProvider);
        let registry = Registry::new(Arc::clone(&provider)).await;
        let mut app = Self::new(provider, registry);
        app.is_remote = true;
        app.resume_session_id = resume_session;
        app
    }

    /// Get the current session ID
    pub fn session_id(&self) -> &str {
        &self.session.id
    }

    /// Initialize MCP servers (call after construction)
    pub async fn init_mcp(&mut self) {
        // Always register the MCP management tool so agent can connect servers
        let mcp_tool = crate::tool::mcp::McpManagementTool::new(Arc::clone(&self.mcp_manager));
        self.registry.register("mcp".to_string(), Arc::new(mcp_tool)).await;

        let manager = self.mcp_manager.read().await;
        if !manager.config().servers.is_empty() {
            drop(manager);
            let manager = self.mcp_manager.write().await;
            if let Err(e) = manager.connect_all().await {
                eprintln!("MCP init error: {}", e);
            }
            // Cache server names
            self.mcp_server_names = manager.connected_servers().await;
            drop(manager);

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
            // Convert session messages to display messages
            for stored_msg in &session.messages {
                let role_str = match stored_msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };

                // Extract text content from ContentBlocks
                let content: String = stored_msg
                    .content
                    .iter()
                    .filter_map(|c| {
                        if let ContentBlock::Text { text } = c {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");

                if !content.is_empty() {
                    self.display_messages.push(DisplayMessage {
                        role: role_str.to_string(),
                        content,
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    self.messages.push(stored_msg.to_message());
                }
            }

            // Don't restore provider_session_id - Claude sessions don't persist across
            // process restarts. The messages are restored, so Claude will get full context.
            self.provider_session_id = None;
            self.session = session;
            // Clear the saved provider_session_id since it's no longer valid
            self.session.provider_session_id = None;
            crate::logging::info(&format!("Restored session: {}", session_id));

            // Add success message to display
            self.display_messages.push(DisplayMessage {
                role: "system".to_string(),
                content: "✓ jcode reloaded successfully. Session restored.".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
        } else {
            crate::logging::error(&format!("Failed to restore session: {}", session_id));
        }
    }

    /// Check for and process debug commands from file
    /// Commands: "message:<text>", "reload", "state", "quit"
    /// Check for new stable version and trigger migration if at safe point
    fn check_stable_version(&mut self) {
        // Only check every 5 seconds to avoid excessive file reads
        let should_check = self.last_version_check
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
        let version_changed = self.known_stable_version
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
                eprintln!("Failed to save session before migration: {}", e);
                return false;
            }

            // Request reload to stable version
            self.reload_requested = Some(self.session.id.clone());

            // The actual exec happens in main.rs when run() returns
            // We store the binary path in an env var for the reload handler
            std::env::set_var("JCODE_MIGRATE_BINARY", stable_binary);

            eprintln!("Migrating to stable version {}...", version);
            self.should_quit = true;
            return true;
        }
        false
    }

    fn check_debug_command(&mut self) -> Option<String> {
        let cmd_path = debug_cmd_path();
        if let Ok(cmd) = std::fs::read_to_string(&cmd_path) {
            // Remove command file immediately
            let _ = std::fs::remove_file(&cmd_path);
            let cmd = cmd.trim();

            let response = if cmd.starts_with("message:") {
                let msg = cmd.strip_prefix("message:").unwrap_or("");
                // Inject the message as if user typed it
                self.input = msg.to_string();
                self.submit_input();
                format!("OK: queued message '{}'", msg)
            } else if cmd == "reload" {
                // Trigger reload
                self.input = "/reload".to_string();
                self.submit_input();
                "OK: reload triggered".to_string()
            } else if cmd == "state" {
                // Return current state
                format!(
                    "state: processing={}, messages={}, display={}, provider_session={:?}",
                    self.is_processing,
                    self.messages.len(),
                    self.display_messages.len(),
                    self.provider_session_id
                )
            } else if cmd == "quit" {
                self.should_quit = true;
                "OK: quitting".to_string()
            } else if cmd == "last_response" {
                // Get last assistant message
                self.display_messages
                    .iter()
                    .rev()
                    .find(|m| m.role == "assistant" || m.role == "error")
                    .map(|m| format!("last_response: [{}] {}", m.role, m.content))
                    .unwrap_or_else(|| "last_response: none".to_string())
            } else {
                format!("ERROR: unknown command '{}'", cmd)
            };

            // Write response
            let _ = std::fs::write(debug_response_path(), &response);
            return Some(response);
        }
        None
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
                self.process_turn_with_input(&mut terminal, &mut event_stream).await;
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
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(RunResult {
            reload_session: self.reload_requested.take(),
            exit_code: self.requested_exit_code,
        })
    }

    /// Run the TUI in remote mode, connecting to a server
    pub async fn run_remote(mut self, mut terminal: DefaultTerminal) -> Result<Option<String>> {
        use super::backend::RemoteConnection;

        let mut event_stream = EventStream::new();
        let mut redraw_interval = interval(Duration::from_millis(50));
        let mut reconnect_attempts = 0u32;
        const MAX_RECONNECT_ATTEMPTS: u32 = 30;

        'outer: loop {
            // Connect to server
            let mut remote = match RemoteConnection::connect().await {
                Ok(r) => {
                    reconnect_attempts = 0;
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
                    if reconnect_attempts > MAX_RECONNECT_ATTEMPTS {
                        self.display_messages.push(DisplayMessage::error("Failed to reconnect after 30 seconds. Press Ctrl+C to quit."));
                        terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;
                        loop {
                            if let Some(Ok(Event::Key(key))) = event_stream.next().await {
                                if key.kind == KeyEventKind::Press {
                                    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
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
                self.display_messages.push(DisplayMessage::system("Reconnected to server."));
            }

            // Resume session if requested (only on first connect, not reconnect)
            if reconnect_attempts == 0 {
                if let Some(session_id) = self.resume_session_id.take() {
                    if let Err(e) = remote.resume_session(&session_id).await {
                        self.display_messages.push(DisplayMessage::error(format!("Failed to resume session: {}", e)));
                    }
                }
            }

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
                    }
                    event = remote.next_event() => {
                        match event {
                            None => {
                                // Server disconnected
                                self.is_processing = false;
                                self.display_messages.push(DisplayMessage {
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
                                self.handle_server_event(server_event, &mut remote);

                                // Process queued messages after turn completes
                                if !self.is_processing && !self.queued_messages.is_empty() {
                                    let combined = std::mem::take(&mut self.queued_messages).join("\n\n");
                                    self.display_messages.push(DisplayMessage {
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
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(self.reload_requested.take())
    }

    /// Handle a server event in remote mode
    fn handle_server_event(&mut self, event: crate::protocol::ServerEvent, remote: &mut super::backend::RemoteConnection) {
        use crate::protocol::ServerEvent;

        match event {
            ServerEvent::TextDelta { text } => {
                // Update status from Sending to Streaming on first text
                if matches!(self.status, ProcessingStatus::Sending) {
                    self.status = ProcessingStatus::Streaming;
                }
                if let Some(chunk) = self.stream_buffer.push(&text) {
                    self.streaming_text.push_str(&chunk);
                }
                self.last_stream_activity = Some(Instant::now());
            }
            ServerEvent::ToolStart { id, name } => {
                remote.handle_tool_start(&id, &name);
                self.status = ProcessingStatus::RunningTool(name.clone());
                self.streaming_tool_calls.push(ToolCall {
                    id,
                    name,
                    input: serde_json::Value::Null,
                });
            }
            ServerEvent::ToolInput { delta } => {
                remote.handle_tool_input(&delta);
            }
            ServerEvent::ToolExec { id, name } => {
                // Update streaming_tool_calls with parsed input before clearing
                let parsed_input = remote.get_current_tool_input();
                if let Some(tc) = self.streaming_tool_calls.iter_mut().find(|tc| tc.id == id) {
                    tc.input = parsed_input.clone();
                }
                remote.handle_tool_exec(&id, &name);
            }
            ServerEvent::ToolDone { id, name, output, error } => {
                let _ = error; // Currently unused
                let display_output = remote.handle_tool_done(&id, &name, &output);
                // Get the tool input from streaming_tool_calls (stored in ToolExec)
                let tool_input = self.streaming_tool_calls
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
                    self.display_messages.push(DisplayMessage {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut self.streaming_text),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
                // Add tool result message
                self.display_messages.push(DisplayMessage {
                    role: "tool".to_string(),
                    content: display_output,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: Some(ToolCall { id, name, input: tool_input }),
                });
                self.streaming_tool_calls.clear();
                self.status = ProcessingStatus::Streaming;
            }
            ServerEvent::TokenUsage { input, output } => {
                self.streaming_input_tokens = input;
                self.streaming_output_tokens = output;
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
                        self.display_messages.push(DisplayMessage {
                            role: "assistant".to_string(),
                            content: std::mem::take(&mut self.streaming_text),
                            tool_calls: vec![],
                            duration_secs: self.processing_started.map(|s| s.elapsed().as_secs_f32()),
                            title: None,
                            tool_data: None,
                        });
                    }
                    self.is_processing = false;
                    self.status = ProcessingStatus::Idle;
                    self.processing_started = None;
                    self.streaming_tool_calls.clear();
                    self.current_message_id = None;
                    remote.clear_pending();
                }
            }
            ServerEvent::Error { message, .. } => {
                self.display_messages.push(DisplayMessage {
                    role: "error".to_string(),
                    content: message,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                self.is_processing = false;
                self.status = ProcessingStatus::Idle;
                remote.clear_pending();
            }
            ServerEvent::SessionId { session_id } => {
                remote.set_session_id(session_id.clone());
                self.remote_session_id = Some(session_id);
            }
            ServerEvent::Reloading { .. } => {
                self.display_messages.push(DisplayMessage {
                    role: "system".to_string(),
                    content: "Server is reloading... Will reconnect shortly.".to_string(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            ServerEvent::History { messages, session_id, provider_name, provider_model, all_sessions, client_count, .. } => {
                remote.set_session_id(session_id.clone());
                self.remote_session_id = Some(session_id);
                // Store provider info for UI display
                if let Some(name) = provider_name {
                    self.remote_provider_name = Some(name);
                }
                if let Some(model) = provider_model {
                    self.remote_provider_model = Some(model);
                }
                // Store session list and client count
                self.remote_sessions = all_sessions;
                self.remote_client_count = client_count;

                if !remote.has_loaded_history() {
                    remote.mark_history_loaded();
                    for msg in messages {
                        self.display_messages.push(DisplayMessage {
                            role: msg.role,
                            content: msg.content,
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                    }
                }
            }
            ServerEvent::ModelChanged { model, error, .. } => {
                if let Some(err) = error {
                    self.display_messages.push(DisplayMessage::error(format!(
                        "Failed to switch model: {}",
                        err
                    )));
                    self.set_model_switch_notice("Model switch failed");
                } else {
                    self.remote_provider_model = Some(model.clone());
                    self.display_messages.push(DisplayMessage::system(format!(
                        "✓ Switched to model: {}",
                        model
                    )));
                    self.set_model_switch_notice(format!("Model → {}", model));
                }
            }
            _ => {}
        }
    }

    /// Handle keyboard input in remote mode
    async fn handle_remote_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: &mut super::backend::RemoteConnection,
    ) -> Result<()> {
        if let Some(direction) = self.model_switch_keys.direction_for(code.clone(), modifiers) {
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

        // Handle Ctrl+Shift combos
        if modifiers.contains(KeyModifiers::CONTROL) && modifiers.contains(KeyModifiers::SHIFT) {
            let max_estimate = self.display_messages.len() * 100 + self.streaming_text.len();
            match code {
                KeyCode::Char('K') => {
                    // Ctrl+Shift+K: scroll up
                    self.scroll_offset = (self.scroll_offset + 3).min(max_estimate);
                    return Ok(());
                }
                KeyCode::Char('J') => {
                    // Ctrl+Shift+J: scroll down
                    self.scroll_offset = self.scroll_offset.saturating_sub(3);
                    return Ok(());
                }
                _ => {}
            }
        }

        // Shift+Tab: toggle diff view
        if code == KeyCode::BackTab {
            self.show_diffs = !self.show_diffs;
            return Ok(());
        }

        // Ctrl combos
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('c') | KeyCode::Char('d') => {
                    self.should_quit = true;
                    return Ok(());
                }
                KeyCode::Char('l') if !self.is_processing => {
                    self.display_messages.clear();
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
                _ => {}
            }
        }

        // Scroll with Ctrl+Shift
        if modifiers.contains(KeyModifiers::CONTROL) && modifiers.contains(KeyModifiers::SHIFT) {
            let max_estimate = self.display_messages.len() * 100 + self.streaming_text.len();
            match code {
                KeyCode::Char('K') => {
                    self.scroll_offset = (self.scroll_offset + 3).min(max_estimate);
                    return Ok(());
                }
                KeyCode::Char('J') => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(3);
                    return Ok(());
                }
                _ => {}
            }
        }

        // Regular keys
        match code {
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
                self.scroll_offset = 0;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
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
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    let raw_input = std::mem::take(&mut self.input);
                    let expanded = self.expand_paste_placeholders(&raw_input);
                    self.pasted_contents.clear();
                    self.cursor_pos = 0;

                    // Handle /reload
                    if expanded.trim() == "/reload" {
                        remote.reload().await?;
                        return Ok(());
                    }

                    // Handle /quit
                    if expanded.trim() == "/quit" {
                        self.should_quit = true;
                        return Ok(());
                    }

                    // Queue message if processing, otherwise send
                    if self.is_processing {
                        self.queued_messages.push(expanded);
                    } else {
                        // Add user message to display (show placeholder)
                        self.display_messages.push(DisplayMessage {
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
                    }
                }
            }
            KeyCode::Esc => {
                self.input.clear();
                self.cursor_pos = 0;
            }
            KeyCode::Up => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            KeyCode::Down => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
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
            self.display_messages.push(DisplayMessage {
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
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        if let Some(direction) = self.model_switch_keys.direction_for(code.clone(), modifiers) {
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
                _ => {}
            }
        }

        // Handle Ctrl+Shift combos (scrolling)
        if modifiers.contains(KeyModifiers::CONTROL) && modifiers.contains(KeyModifiers::SHIFT) {
            let max_estimate = self.display_messages.len() * 100 + self.streaming_text.len();
            match code {
                KeyCode::Char('K') => {
                    // Ctrl+Shift+K: scroll up
                    self.scroll_offset = (self.scroll_offset + 3).min(max_estimate);
                    return Ok(());
                }
                KeyCode::Char('J') => {
                    // Ctrl+Shift+J: scroll down
                    self.scroll_offset = self.scroll_offset.saturating_sub(3);
                    return Ok(());
                }
                _ => {}
            }
        }

        // Shift+Tab: toggle diff view
        if code == KeyCode::BackTab {
            self.show_diffs = !self.show_diffs;
            return Ok(());
        }

        // Handle ctrl combos regardless of processing state
        if modifiers.contains(KeyModifiers::CONTROL) {
            match code {
                KeyCode::Char('c') | KeyCode::Char('d') => {
                    self.should_quit = true;
                    return Ok(());
                }
                KeyCode::Char('l') if !self.is_processing => {
                    self.messages.clear();
                    self.display_messages.clear();
                    self.queued_messages.clear();
                    self.pasted_contents.clear();
                    self.active_skill = None;
                    self.session = Session::create(None, None);
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
                _ => {}
            }
        }

        match code {
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    if self.is_processing {
                        // Queue the message instead of blocking
                        self.queue_message();
                    } else {
                        self.submit_input();
                    }
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
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
        let placeholder = format!("[pasted {} line{}]", line_count, if line_count == 1 { "" } else { "s" });
        self.input.insert_str(self.cursor_pos, &placeholder);
        self.cursor_pos += placeholder.len();
    }

    /// Expand paste placeholders in input with actual content
    fn expand_paste_placeholders(&mut self, input: &str) -> String {
        let mut result = input.to_string();
        // Replace placeholders in reverse order to preserve indices
        for content in self.pasted_contents.iter().rev() {
            let line_count = content.lines().count().max(1);
            let placeholder = format!("[pasted {} line{}]", line_count, if line_count == 1 { "" } else { "s" });
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
            self.display_messages.push(DisplayMessage {
                role: "system".to_string(),
                content: format!(
                    "**Commands:**\n\
                     • `/help` - Show this help\n\
                     • `/model` - List available models\n\
                     • `/model <name>` - Switch to a different model\n\
                     • `/reload` - Hot-reload with new binary (keeps session)\n\
                     • `/rebuild` - Build and test new canary (self-dev mode only)\n\
                     • `/clear` - Clear conversation (Ctrl+L)\n\
                     • `/<skill>` - Activate a skill\n\n\
                     **Available skills:** {}\n\n\
                     **Keyboard shortcuts:**\n\
                     • `Ctrl+C` / `Ctrl+D` - Quit\n\
                     • `Ctrl+L` - Clear conversation\n\
                     • `PageUp/Down` - Scroll history\n\
                     • `Ctrl+U` - Clear input line\n\
                     • `Ctrl+K` - Kill to end of line\n\
                     {}\n\
                     {}",
                    self.skills.list().iter().map(|s| format!("/{}", s.name)).collect::<Vec<_>>().join(", "),
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
            self.display_messages.clear();
            self.queued_messages.clear();
            self.pasted_contents.clear();
            self.active_skill = None;
            self.session = Session::create(None, None);
            self.provider_session_id = None;
            return;
        }

        // Handle /model command
        if trimmed == "/model" || trimmed == "/models" {
            // List available models
            let models = self.provider.available_models();
            let current = self.provider.model();
            let model_list = models
                .iter()
                .map(|m| {
                    if *m == current {
                        format!("  • **{}** (current)", m)
                    } else {
                        format!("  • {}", m)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");

            self.display_messages.push(DisplayMessage {
                role: "system".to_string(),
                content: format!(
                    "**Available models for {}:**\n{}\n\nUse `/model <name>` to switch.",
                    self.provider.name(),
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
                    self.display_messages.push(DisplayMessage {
                        role: "system".to_string(),
                        content: format!("✓ Switched to model: {}", model_name),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    self.set_model_switch_notice(format!("Model → {}", model_name));
                }
                Err(e) => {
                    self.display_messages.push(DisplayMessage {
                        role: "error".to_string(),
                        content: format!("Failed to switch model: {}", e),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    self.set_model_switch_notice("Model switch failed");
                }
            }
            return;
        }

        if trimmed == "/reload" {
            self.display_messages.push(DisplayMessage {
                role: "system".to_string(),
                content: "Reloading jcode with new binary...".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            // Save provider session ID for resume after reload
            self.session.provider_session_id = self.provider_session_id.clone();
            // Save session and set reload flag
            let _ = self.session.save();
            self.reload_requested = Some(self.session.id.clone());
            self.should_quit = true;
            return;
        }

        // /rebuild - Build and test new canary, restart (only in canary/self-dev mode)
        if trimmed == "/rebuild" {
            if !self.session.is_canary {
                self.display_messages.push(DisplayMessage {
                    role: "error".to_string(),
                    content: "/rebuild is only available in self-dev mode (jcode self-dev)".to_string(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                return;
            }

            self.display_messages.push(DisplayMessage {
                role: "system".to_string(),
                content: "Building and testing new canary...".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });

            // Find jcode repo directory
            if let Some(repo_dir) = crate::build::get_repo_dir() {
                match crate::build::rebuild_canary(&repo_dir) {
                    Ok(hash) => {
                        self.display_messages.push(DisplayMessage {
                            role: "system".to_string(),
                            content: format!("Build successful ({}). Restarting with new canary...", hash),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                        // Save session
                        self.session.provider_session_id = self.provider_session_id.clone();
                        let _ = self.session.save();
                        // Exit with code 42 to signal wrapper to respawn with new canary
                        self.requested_exit_code = Some(42);
                        self.should_quit = true;
                    }
                    Err(e) => {
                        self.display_messages.push(DisplayMessage {
                            role: "error".to_string(),
                            content: format!("Build failed: {}", e),
                            tool_calls: vec![],
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                    }
                }
            } else {
                self.display_messages.push(DisplayMessage {
                    role: "error".to_string(),
                    content: "Could not find jcode repository directory".to_string(),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            return;
        }

        // Check for skill invocation
        if let Some(skill_name) = SkillRegistry::parse_invocation(&input) {
            if let Some(skill) = self.skills.get(skill_name) {
                self.active_skill = Some(skill_name.to_string());
                self.display_messages.push(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Activated skill: {} - {}", skill.name, skill.description),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            } else {
                self.display_messages.push(DisplayMessage {
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

        // Add user message to display immediately (show placeholder, not full paste)
        self.display_messages.push(DisplayMessage {
            role: "user".to_string(),
            content: raw_input.clone(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        // Send expanded content (with actual pasted text) to model
        self.messages.push(Message::user(&input));
        self.session.add_message(
            Role::User,
            vec![ContentBlock::Text { text: input.clone() }],
        );
        let _ = self.session.save();

        // Set up processing state - actual processing happens after UI redraws
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.streaming_text.clear();
                self.stream_buffer.clear();
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
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
            self.display_messages.push(DisplayMessage {
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
                vec![ContentBlock::Text { text: combined }],
            );
            let _ = self.session.save();
            self.streaming_text.clear();
                self.stream_buffer.clear();
            self.streaming_tool_calls.clear();
            self.streaming_input_tokens = 0;
            self.streaming_output_tokens = 0;
            self.processing_started = Some(Instant::now());
            self.status = ProcessingStatus::Sending;

            if let Err(e) = self.run_turn_interactive(terminal, event_stream).await {
                self.display_messages.push(DisplayMessage {
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
            self.display_messages.push(DisplayMessage::error(
                "Model switching is not available for this provider.",
            ));
            self.set_model_switch_notice("Model switching not available");
            return;
        }

        let current = self.provider.model();
        let current_index = models
            .iter()
            .position(|m| *m == current)
            .unwrap_or(0);

        let len = models.len();
        let next_index = if direction >= 0 {
            (current_index + 1) % len
        } else {
            (current_index + len - 1) % len
        };
        let next_model = models[next_index];

        match self.provider.set_model(next_model) {
            Ok(()) => {
                self.display_messages.push(DisplayMessage::system(format!(
                    "✓ Switched to model: {}",
                    next_model
                )));
                self.set_model_switch_notice(format!("Model → {}", next_model));
            }
            Err(e) => {
                self.display_messages.push(DisplayMessage::error(format!(
                    "Failed to switch model: {}",
                    e
                )));
                self.set_model_switch_notice("Model switch failed");
            }
        }
    }

    fn set_model_switch_notice(&mut self, text: impl Into<String>) {
        self.model_switch_notice = Some((text.into(), Instant::now()));
    }

    async fn run_turn(&mut self) -> Result<()> {
        loop {
            let tools = self.registry.definitions(None).await;

            // Build system prompt with active skill
            let system_prompt = self.build_system_prompt();

            self.status = ProcessingStatus::Sending;
            let mut stream = self
                .provider
                .complete(&self.messages, &tools, &system_prompt, self.provider_session_id.as_deref())
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut first_event = true;
            let mut saw_message_end = false;
            // Track tool results from SDK (already executed by Claude Agent SDK)
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
                                self.display_messages.push(DisplayMessage {
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
                            self.display_messages.push(DisplayMessage {
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
                    } => {
                        if let Some(input) = input_tokens {
                            self.streaming_input_tokens = input;
                            // Warn when approaching context limit (80%)
                            self.check_context_warning(input);
                        }
                        if let Some(output) = output_tokens {
                            self.streaming_output_tokens = output;
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
                    StreamEvent::Error(e) => {
                        return Err(anyhow::anyhow!("Stream error: {}", e));
                    }
                    StreamEvent::ThinkingStart => {
                        // Track start but don't display - wait for ThinkingDone
                        self.thinking_start = Some(Instant::now());
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
                        let thinking_msg = format!("Thought for {:.1}s\n\n", duration_secs);
                        self.streaming_text.push_str(&thinking_msg);
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
                        // Reset warning so it can appear again
                        self.context_warning_shown = false;
                    }
                    StreamEvent::ToolResult { tool_use_id, content, is_error } => {
                        // SDK already executed this tool, store result for later
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                }
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
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
                    self.display_messages.push(DisplayMessage {
                        role: "assistant".to_string(),
                        content: text_content.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                }
            } else {
                // Had tool calls - only display text that came AFTER the last tool
                // (text before each tool was already committed in ToolUseEnd handler)
                if !self.streaming_text.is_empty() {
                    self.display_messages.push(DisplayMessage {
                        role: "assistant".to_string(),
                        content: self.streaming_text.clone(),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
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
                let (output, is_error, tool_title) = if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // Use SDK result
                    Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                        session_id: self.session.id.clone(),
                        message_id: message_id.clone(),
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        status: if sdk_is_error { ToolStatus::Error } else { ToolStatus::Completed },
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
                if let Some(dm) = self.display_messages.iter_mut().rev().find(|dm| {
                    dm.tool_data.as_ref().map(|td| &td.id) == Some(&tc.id)
                }) {
                    dm.content = output.clone();
                    dm.title = tool_title;
                }

                self.messages.push(Message::tool_result(&tc.id, &output, is_error));
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
            let tools = self.registry.definitions(None).await;
            let system_prompt = self.build_system_prompt();

            self.status = ProcessingStatus::Sending;
            terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;

            crate::logging::info(&format!("TUI: API call starting ({} messages)", self.messages.len()));
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
                                        self.display_messages.push(DisplayMessage {
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

            crate::logging::info(&format!("TUI: API stream opened in {:.2}s", api_start.elapsed().as_secs_f64()));

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut first_event = true;
            let mut saw_message_end = false;
            // Track tool results from SDK (already executed by Claude Agent SDK)
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
                                        self.display_messages.push(DisplayMessage {
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
                                                self.display_messages.push(DisplayMessage {
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
                                            self.display_messages.push(DisplayMessage {
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
                                    StreamEvent::TokenUsage { input_tokens, output_tokens } => {
                                        if let Some(input) = input_tokens {
                                            self.streaming_input_tokens = input;
                                            self.check_context_warning(input);
                                        }
                                        if let Some(output) = output_tokens {
                                            self.streaming_output_tokens = output;
                                        }
                                        self.broadcast_debug(super::backend::DebugEvent::TokenUsage {
                                            input_tokens: self.streaming_input_tokens,
                                            output_tokens: self.streaming_output_tokens,
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
                                    StreamEvent::Error(e) => {
                                        return Err(anyhow::anyhow!("Stream error: {}", e));
                                    }
                                    StreamEvent::ThinkingStart => {
                                        self.thinking_start = Some(Instant::now());
                                        self.broadcast_debug(super::backend::DebugEvent::ThinkingStart);
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
                                        let thinking_msg = format!("Thought for {:.1}s\n\n", duration_secs);
                                        self.streaming_text.push_str(&thinking_msg);
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
                                        }

                                        // Clear this tool from streaming_tool_calls
                                        self.streaming_tool_calls.retain(|tc| tc.id != tool_use_id);

                                        // Reset status back to Streaming
                                        self.status = ProcessingStatus::Streaming;

                                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                                    }
                                }
                            }
                            Some(Err(e)) => return Err(e),
                            None => break, // Stream ended
                        }
                    }
                }
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
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
                    self.display_messages.push(DisplayMessage {
                        role: "assistant".to_string(),
                        content: text_content.clone(),
                        tool_calls: vec![],
                        duration_secs: duration,
                        title: None,
                        tool_data: None,
                    });
                }
            } else {
                // Had tool calls - only display text that came AFTER the last tool
                // (text before each tool was already committed in ToolUseEnd handler)
                if !self.streaming_text.is_empty() {
                    self.display_messages.push(DisplayMessage {
                        role: "assistant".to_string(),
                        content: self.streaming_text.clone(),
                        tool_calls: vec![],
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
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
                        status: if sdk_is_error { ToolStatus::Error } else { ToolStatus::Completed },
                        title: None,
                    }));

                    // Update the tool's DisplayMessage with the output
                    if let Some(dm) = self.display_messages.iter_mut().rev().find(|dm| {
                        dm.tool_data.as_ref().map(|td| &td.id) == Some(&tc.id)
                    }) {
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
                let mut tool_future = std::pin::pin!(
                    registry.execute(&tool_name, tool_input, ctx)
                );

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
                                            self.display_messages.push(DisplayMessage {
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
                if let Some(dm) = self.display_messages.iter_mut().rev().find(|dm| {
                    dm.tool_data.as_ref().map(|td| &td.id) == Some(&tc.id)
                }) {
                    dm.content = output.clone();
                    dm.title = tool_title;
                }

                self.messages.push(Message::tool_result(&tc.id, &output, is_error));
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

    fn build_system_prompt(&self) -> String {
        let skill_prompt = self.active_skill.as_ref().and_then(|name| {
            self.skills.get(name).map(|s| s.get_prompt().to_string())
        });
        crate::prompt::build_system_prompt(skill_prompt.as_deref())
    }

    // Getters for UI
    pub fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
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

    /// Get command suggestions based on current input
    pub fn command_suggestions(&self) -> Vec<(&'static str, &'static str)> {
        let input = self.input.trim();

        // Only show suggestions when input starts with /
        if !input.starts_with('/') {
            return vec![];
        }

        // Built-in commands
        let mut commands: Vec<(&'static str, &'static str)> = vec![
            ("/help", "Show help and keyboard shortcuts"),
            ("/reload", "Pull, rebuild, and restart (keeps session)"),
            ("/clear", "Clear conversation history"),
        ];
        // Add /rebuild only in canary/self-dev mode
        if self.session.is_canary {
            commands.insert(2, ("/rebuild", "Build, test, and restart with new canary"));
        }

        // Filter by prefix match
        let prefix = input.to_lowercase();
        commands
            .into_iter()
            .filter(|(cmd, _)| cmd.to_lowercase().starts_with(&prefix))
            .collect()
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
                    let wrap_factor = if avg_line_len > width { (avg_line_len / width) + 1 } else { 1 };
                    let mut h = content_lines * wrap_factor;
                    if !msg.tool_calls.is_empty() { h += 1; }
                    if msg.duration_secs.is_some() { h += 1; }
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
    pub fn enable_debug_socket(&mut self) -> tokio::sync::broadcast::Receiver<super::backend::DebugEvent> {
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
            display_messages: self.display_messages.iter().map(|m| DebugMessage {
                role: m.role.clone(),
                content: m.content.clone(),
                tool_calls: m.tool_calls.clone(),
                duration_secs: m.duration_secs,
                title: m.title.clone(),
                tool_data: m.tool_data.clone(),
            }).collect(),
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
                    eprintln!("Failed to bind debug socket: {}", e);
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
                        let snapshot_json = serde_json::to_string(&initial_snapshot).unwrap_or_default() + "\n";
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
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| "/tmp".to_string());
        std::path::PathBuf::from(runtime_dir).join("jcode-debug.sock")
    }
}

impl super::TuiState for App {
    fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
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

    fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn provider_name(&self) -> String {
        self.remote_provider_name.clone()
            .unwrap_or_else(|| self.provider.name().to_string())
    }

    fn provider_model(&self) -> String {
        self.remote_provider_model.clone()
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

    fn streaming_tool_calls(&self) -> Vec<ToolCall> {
        self.streaming_tool_calls.clone()
    }

    fn elapsed(&self) -> Option<std::time::Duration> {
        self.processing_started.map(|t| t.elapsed())
    }

    fn status(&self) -> ProcessingStatus {
        self.status.clone()
    }

    fn command_suggestions(&self) -> Vec<(&'static str, &'static str)> {
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
        self.session.is_canary
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

    fn server_sessions(&self) -> Vec<String> {
        self.remote_sessions.clone()
    }

    fn connected_clients(&self) -> Option<usize> {
        self.remote_client_count
    }

    fn model_switch_notice(&self) -> Option<String> {
        self.model_switch_notice.as_ref().and_then(|(text, at)| {
            if at.elapsed() <= Duration::from_secs(5) {
                Some(text.clone())
            } else {
                None
            }
        })
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
    }

    fn create_test_app() -> App {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
        App::new(provider, registry)
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
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('l'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('l'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('o'), KeyModifiers::empty()).unwrap();

        assert_eq!(app.input(), "hello");
        assert_eq!(app.cursor_pos(), 5);
    }

    #[test]
    fn test_handle_key_backspace() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Backspace, KeyModifiers::empty()).unwrap();

        assert_eq!(app.input(), "a");
        assert_eq!(app.cursor_pos(), 1);
    }

    #[test]
    fn test_handle_key_cursor_movement() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('c'), KeyModifiers::empty()).unwrap();

        assert_eq!(app.cursor_pos(), 3);

        app.handle_key(KeyCode::Left, KeyModifiers::empty()).unwrap();
        assert_eq!(app.cursor_pos(), 2);

        app.handle_key(KeyCode::Home, KeyModifiers::empty()).unwrap();
        assert_eq!(app.cursor_pos(), 0);

        app.handle_key(KeyCode::End, KeyModifiers::empty()).unwrap();
        assert_eq!(app.cursor_pos(), 3);
    }

    #[test]
    fn test_handle_key_escape_clears_input() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('s'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty()).unwrap();

        assert_eq!(app.input(), "test");

        app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();

        assert!(app.input().is_empty());
        assert_eq!(app.cursor_pos(), 0);
    }

    #[test]
    fn test_handle_key_ctrl_u_clears_input() {
        let mut app = create_test_app();

        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('s'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty()).unwrap();

        app.handle_key(KeyCode::Char('u'), KeyModifiers::CONTROL).unwrap();

        assert!(app.input().is_empty());
        assert_eq!(app.cursor_pos(), 0);
    }

    #[test]
    fn test_submit_input_adds_message() {
        let mut app = create_test_app();

        // Type and submit
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty()).unwrap();
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

        // Simulate processing state
        app.is_processing = true;

        // Type a message
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('s'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty()).unwrap();

        // Press Enter should queue, not submit
        app.handle_key(KeyCode::Enter, KeyModifiers::empty()).unwrap();

        assert_eq!(app.queued_count(), 1);
        assert!(app.input().is_empty());

        // Queued messages are stored in queued_messages, not display_messages
        assert_eq!(app.queued_messages()[0], "test");
        assert!(app.display_messages().is_empty());
    }

    #[test]
    fn test_typing_during_processing() {
        let mut app = create_test_app();
        app.is_processing = true;

        // Should still be able to type
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('c'), KeyModifiers::empty()).unwrap();

        assert_eq!(app.input(), "abc");
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
        app.handle_key(KeyCode::Char('/'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('e'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('s'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('t'), KeyModifiers::empty()).unwrap();

        app.submit_input();

        // Should show error for unknown skill, not start processing
        assert!(!app.pending_turn);
        assert!(!app.is_processing);
        // Should have an error message about unknown skill
        assert_eq!(app.display_messages().len(), 1);
        assert_eq!(app.display_messages()[0].role, "error");
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
        app.handle_key(KeyCode::Char('A'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char(':'), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty()).unwrap();
        app.handle_paste("pasted content".to_string());
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty()).unwrap();
        app.handle_key(KeyCode::Char('B'), KeyModifiers::empty()).unwrap();

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
            crate::message::ContentBlock::Text { text } => {
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
        app.handle_key(KeyCode::Char(' '), KeyModifiers::empty()).unwrap();
        app.handle_paste("second\nline".to_string());

        assert_eq!(app.input(), "[pasted 1 line] [pasted 2 lines]");
        assert_eq!(app.pasted_contents.len(), 2);

        app.submit_input();
        // Display shows placeholders (user sees condensed view)
        assert_eq!(app.display_messages()[0].content, "[pasted 1 line] [pasted 2 lines]");
        // Model receives expanded content
        match &app.messages[0].content[0] {
            crate::message::ContentBlock::Text { text } => {
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
        session.add_message(Role::User, vec![ContentBlock::Text { text: "test message".to_string() }]);
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
        assert!(app.display_messages()[1].content.contains("reloaded successfully"));

        // Messages for API should only have the original message (no reload msg to avoid breaking alternation)
        assert_eq!(app.messages.len(), 1);

        // Provider session ID should be cleared (Claude sessions don't persist across restarts)
        assert!(app.provider_session_id.is_none());

        // Clean up
        let _ = std::fs::remove_file(crate::session::session_path(&session_id).unwrap());
    }
}
