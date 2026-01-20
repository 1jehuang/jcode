//! TUI client that connects to jcode server
//!
//! This provides a full TUI experience while using the server for processing.
//! Benefits:
//! - Server maintains Claude session (caching)
//! - Can hot-reload server without losing TUI
//! - TUI can reconnect after server restart

use super::{DisplayMessage, ProcessingStatus, TuiState};
use crate::message::ToolCall;
use crate::protocol::{NotificationType, Request, ServerEvent};
use crate::server;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use similar::TextDiff;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::interval;

/// Check if client-side diffs are enabled (default: true, disable with JCODE_SHOW_DIFFS=0)
fn show_diffs_enabled() -> bool {
    std::env::var("JCODE_SHOW_DIFFS")
        .map(|v| v != "0" && v != "false")
        .unwrap_or(true)
}

/// Tracks a pending file edit for diff generation
struct PendingFileDiff {
    file_path: String,
    original_content: String,
}

/// Client TUI state
pub struct ClientApp {
    // Display state (matching App for TuiState)
    display_messages: Vec<DisplayMessage>,
    input: String,
    cursor_pos: usize,
    is_processing: bool,
    streaming_text: String,
    queued_messages: Vec<String>,
    scroll_offset: usize,
    status: ProcessingStatus,
    streaming_tool_calls: Vec<ToolCall>,
    streaming_input_tokens: u64,
    streaming_output_tokens: u64,
    streaming_cache_read_tokens: Option<u64>,
    streaming_cache_creation_tokens: Option<u64>,
    processing_started: Option<Instant>,
    last_activity: Option<Instant>,

    // Client-specific state
    should_quit: bool,
    session_id: Option<String>,
    next_request_id: u64,
    server_disconnected: bool,
    has_loaded_history: bool,
    provider_name: String,
    provider_model: String,

    // For client-side diff generation
    pending_diffs: HashMap<String, PendingFileDiff>,
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    current_tool_input: String,
    // Short-lived notice for status feedback
    status_notice: Option<(String, Instant)>,
    // Time when app started (for startup animations)
    app_started: Instant,
    // Store reload info to pass to agent after reconnection
    reload_info: Vec<String>,
    // Context info (what's loaded in system prompt)
    context_info: crate::prompt::ContextInfo,
}

impl ClientApp {
    pub fn new() -> Self {
        Self {
            // Display state
            display_messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            is_processing: false,
            streaming_text: String::new(),
            queued_messages: Vec::new(),
            scroll_offset: 0,
            status: ProcessingStatus::Idle,
            streaming_tool_calls: Vec::new(),
            streaming_input_tokens: 0,
            streaming_output_tokens: 0,
            streaming_cache_read_tokens: None,
            streaming_cache_creation_tokens: None,
            processing_started: None,
            last_activity: None,

            // Client-specific state
            should_quit: false,
            session_id: None,
            next_request_id: 1,
            server_disconnected: false,
            has_loaded_history: false,
            provider_name: "unknown".to_string(),
            provider_model: "unknown".to_string(),

            // Diff tracking
            pending_diffs: HashMap::new(),
            current_tool_id: None,
            current_tool_name: None,
            current_tool_input: String::new(),
            status_notice: None,
            app_started: Instant::now(),
            reload_info: Vec::new(),
            // Compute context info at startup (selfdev mode is always canary)
            context_info: {
                let (_, info) = crate::prompt::build_system_prompt_with_context(
                    None,
                    &[],  // No skills in client mode
                    true, // selfdev = canary
                );
                info
            },
        }
    }

    /// Connect to server and sync state
    pub async fn connect(&mut self) -> Result<UnixStream> {
        let stream = UnixStream::connect(server::socket_path()).await?;

        // Will sync history after connection is established
        Ok(stream)
    }

    /// Sync history from server (for reconnection)
    #[allow(dead_code)]
    pub async fn sync_history(&mut self, stream: &mut UnixStream) -> Result<()> {
        let (reader, mut writer) = stream.split();
        let mut reader = BufReader::new(reader);

        // Send GetHistory request
        let request = Request::GetHistory {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        let json = serde_json::to_string(&request)? + "\n";
        writer.write_all(json.as_bytes()).await?;

        // Read response
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let event: ServerEvent = serde_json::from_str(&line)?;

        if let ServerEvent::History {
            session_id,
            messages,
            ..
        } = event
        {
            self.session_id = Some(session_id);
            for msg in messages {
                self.display_messages.push(DisplayMessage {
                    role: msg.role,
                    content: msg.content,
                    tool_calls: Vec::new(),
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
        }

        Ok(())
    }

    /// Run the client TUI with auto-reconnection
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let mut event_stream = EventStream::new();
        let mut reconnect_attempts = 0;
        const MAX_RECONNECT_ATTEMPTS: u32 = 30; // 30 seconds max

        'outer: loop {
            // Connect to server
            let stream = match self.connect().await {
                Ok(s) => {
                    self.server_disconnected = false;
                    s
                }
                Err(e) => {
                    if reconnect_attempts == 0 {
                        // First connection attempt failed
                        return Err(anyhow::anyhow!(
                            "Failed to connect to server. Is `jcode serve` running? Error: {}",
                            e
                        ));
                    }
                    // Reconnecting after disconnect
                    reconnect_attempts += 1;
                    if reconnect_attempts > MAX_RECONNECT_ATTEMPTS {
                        self.display_messages.push(DisplayMessage {
                            role: "error".to_string(),
                            content: "Failed to reconnect after 30 seconds. Press Ctrl+C to quit."
                                .to_string(),
                            tool_calls: Vec::new(),
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                        terminal.draw(|frame| super::ui::draw(frame, &self))?;
                        // Wait for quit
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
                    // Wait and retry
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    terminal.draw(|frame| super::ui::draw(frame, &self))?;
                    continue;
                }
            };

            // Show reconnection success message if we were reconnecting
            if reconnect_attempts > 0 {
                // Build success message with reload info if available
                let reload_details = if !self.reload_info.is_empty() {
                    format!("\n  {}", self.reload_info.join("\n  "))
                } else {
                    String::new()
                };

                self.display_messages.push(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("✓ Reconnected successfully.{}", reload_details),
                    tool_calls: Vec::new(),
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });

                // Queue message to notify the agent about the reload
                if !self.reload_info.is_empty() {
                    let cwd = std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "unknown".to_string());
                    let reload_summary = self.reload_info.join(", ");
                    self.queued_messages.push(format!(
                        "[Reload complete. {}. CWD: {}. Session restored - continue where you left off.]",
                        reload_summary, cwd
                    ));
                    self.reload_info.clear();
                }
            }

            let (reader, writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let writer = std::sync::Arc::new(tokio::sync::Mutex::new(writer));
            let mut redraw_interval = interval(Duration::from_millis(50));
            let mut server_line = String::new();

            // Subscribe to server events and get history
            {
                let mut w = writer.lock().await;

                // If reconnecting after server reload, restore the session first
                if reconnect_attempts > 0 {
                    if let Some(ref session_id) = self.session_id {
                        let exists_on_disk = crate::session::session_path(session_id)
                            .map(|p| p.exists())
                            .unwrap_or(false);
                        if exists_on_disk {
                            let request = Request::ResumeSession {
                                id: self.next_request_id,
                                session_id: session_id.clone(),
                            };
                            self.next_request_id += 1;
                            let json = serde_json::to_string(&request)? + "\n";
                            w.write_all(json.as_bytes()).await?;
                        }
                    }
                }
                reconnect_attempts = 0;

                // Subscribe to events
                let (working_dir, selfdev) = super::subscribe_metadata();
                let request = Request::Subscribe {
                    id: self.next_request_id,
                    working_dir,
                    selfdev,
                };
                self.next_request_id += 1;
                let json = serde_json::to_string(&request)? + "\n";
                w.write_all(json.as_bytes()).await?;

                // Request history to restore display state
                let request = Request::GetHistory {
                    id: self.next_request_id,
                };
                self.next_request_id += 1;
                let json = serde_json::to_string(&request)? + "\n";
                w.write_all(json.as_bytes()).await?;
            }

            // Main event loop
            loop {
                // Draw UI
                terminal.draw(|frame| super::ui::draw(frame, &self))?;

                if self.should_quit {
                    break 'outer;
                }

                tokio::select! {
                    _ = redraw_interval.tick() => {
                        // Just redraw
                    }
                    // Read from server
                    result = reader.read_line(&mut server_line) => {
                        match result {
                            Ok(0) | Err(_) => {
                                // Server disconnected - try to reconnect
                                self.server_disconnected = true;
                                self.is_processing = false;
                                self.display_messages.push(DisplayMessage {
                                    role: "system".to_string(),
                                    content: "Server disconnected. Reconnecting...".to_string(),
                                    tool_calls: Vec::new(),
                                    duration_secs: None,
                                    title: None,
                                    tool_data: None,
                                });
                                terminal.draw(|frame| super::ui::draw(frame, &self))?;
                                reconnect_attempts = 1;
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                continue 'outer;
                            }
                            Ok(_) => {
                                if let Ok(event) = serde_json::from_str::<ServerEvent>(&server_line) {
                                    self.handle_server_event(event);
                                }
                                server_line.clear();
                            }
                        }
                    }
                    // Handle keyboard input
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key))) => {
                                if key.kind == KeyEventKind::Press {
                                    self.handle_key(key.code, key.modifiers, &writer).await?;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn handle_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::TextDelta { text } => {
                self.streaming_text.push_str(&text);
            }
            ServerEvent::ToolStart { id, name } => {
                // Start tracking this tool for potential diff generation
                self.current_tool_id = Some(id);
                self.current_tool_name = Some(name);
                self.current_tool_input.clear();
            }
            ServerEvent::ToolInput { delta } => {
                // Accumulate tool input JSON
                self.current_tool_input.push_str(&delta);
            }
            ServerEvent::ToolExec { id, name } => {
                // Tool is about to execute - if it's edit/write, cache the file content
                if show_diffs_enabled() && (name == "edit" || name == "write") {
                    if let Ok(input) =
                        serde_json::from_str::<serde_json::Value>(&self.current_tool_input)
                    {
                        if let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) {
                            // Read current file content (sync is fine here, it's quick)
                            let original = std::fs::read_to_string(file_path).unwrap_or_default();
                            self.pending_diffs.insert(
                                id.clone(),
                                PendingFileDiff {
                                    file_path: file_path.to_string(),
                                    original_content: original,
                                },
                            );
                        }
                    }
                }
                // Clear tracking state
                self.current_tool_id = None;
                self.current_tool_name = None;
                self.current_tool_input.clear();
            }
            ServerEvent::ToolDone {
                id, name, output, ..
            } => {
                // Check if we have a pending diff for this tool
                if let Some(pending) = self.pending_diffs.remove(&id) {
                    // Read the file again and generate diff
                    let new_content =
                        std::fs::read_to_string(&pending.file_path).unwrap_or_default();
                    let diff = generate_unified_diff(
                        &pending.original_content,
                        &new_content,
                        &pending.file_path,
                    );
                    if !diff.is_empty() {
                        self.streaming_text
                            .push_str(&format!("\n[{}] {}\n{}\n", name, pending.file_path, diff));
                    } else {
                        // No changes or couldn't generate diff, show original output
                        self.streaming_text
                            .push_str(&format!("\n[{}] {}\n", name, output));
                    }
                } else {
                    // No pending diff, just show the output
                    self.streaming_text
                        .push_str(&format!("\n[{}] {}\n", name, output));
                }
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
            }
            ServerEvent::Done { .. } => {
                if !self.streaming_text.is_empty() {
                    self.display_messages.push(DisplayMessage {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut self.streaming_text),
                        tool_calls: Vec::new(),
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                }
                self.is_processing = false;
                // Clear any leftover diff tracking state
                self.pending_diffs.clear();
            }
            ServerEvent::Error { message, .. } => {
                self.display_messages.push(DisplayMessage {
                    role: "error".to_string(),
                    content: message,
                    tool_calls: Vec::new(),
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                self.is_processing = false;
                self.pending_diffs.clear();
            }
            ServerEvent::SessionId { session_id } => {
                self.session_id = Some(session_id);
            }
            ServerEvent::Reloading { .. } => {
                self.display_messages.push(DisplayMessage {
                    role: "system".to_string(),
                    content: "🔄 Server reload initiated...".to_string(),
                    tool_calls: Vec::new(),
                    duration_secs: None,
                    title: Some("Reload".to_string()),
                    tool_data: None,
                });
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

                self.display_messages.push(DisplayMessage {
                    role: "system".to_string(),
                    content,
                    tool_calls: Vec::new(),
                    duration_secs: None,
                    title: Some(format!("Reload: {} {}", status_icon, step)),
                    tool_data: None,
                });

                // Store key reload info for agent notification after reconnect
                // Store info from verify and git steps
                if step == "verify" || step == "git" {
                    self.reload_info.push(message.clone());
                }

                // Update status notice
                self.status_notice =
                    Some((format!("Reload: {}", message), std::time::Instant::now()));
            }
            ServerEvent::History {
                messages,
                session_id,
                ..
            } => {
                let session_changed = self.session_id.as_deref() != Some(&session_id);
                self.session_id = Some(session_id);

                if session_changed {
                    self.display_messages.clear();
                    self.streaming_text.clear();
                    self.streaming_tool_calls.clear();
                    self.streaming_input_tokens = 0;
                    self.streaming_output_tokens = 0;
                    self.streaming_cache_read_tokens = None;
                    self.streaming_cache_creation_tokens = None;
                    self.processing_started = None;
                    self.last_activity = None;
                    self.is_processing = false;
                    self.status = ProcessingStatus::Idle;
                    self.scroll_offset = 0;
                    self.queued_messages.clear();
                    self.pending_diffs.clear();
                    self.current_tool_id = None;
                    self.current_tool_name = None;
                    self.current_tool_input.clear();
                    self.has_loaded_history = false;
                }

                if session_changed || !self.has_loaded_history {
                    self.has_loaded_history = true;
                    for msg in messages {
                        self.display_messages.push(DisplayMessage {
                            role: msg.role,
                            content: msg.content,
                            tool_calls: Vec::new(),
                            duration_secs: None,
                            title: None,
                            tool_data: None,
                        });
                    }
                }
            }
            ServerEvent::ModelChanged { model, error, .. } => {
                if let Some(err) = error {
                    self.display_messages.push(DisplayMessage {
                        role: "error".to_string(),
                        content: format!("Failed to switch model: {}", err),
                        tool_calls: Vec::new(),
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    self.status_notice = Some(("Model switch failed".to_string(), Instant::now()));
                } else {
                    self.provider_model = model.clone();
                    self.display_messages.push(DisplayMessage {
                        role: "system".to_string(),
                        content: format!("✓ Switched to model: {}", model),
                        tool_calls: Vec::new(),
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });
                    self.status_notice = Some((format!("Model → {}", model), Instant::now()));
                }
            }
            ServerEvent::Notification {
                from_session,
                from_name,
                notification_type,
                message,
            } => {
                let from = from_name.unwrap_or_else(|| from_session.chars().take(8).collect());
                let prefix = match notification_type {
                    NotificationType::FileConflict { path, .. } => {
                        format!("⚠️ File conflict ({})", path)
                    }
                    NotificationType::SharedContext { key, .. } => {
                        format!("📤 Context shared: {}", key)
                    }
                    NotificationType::Message => "💬 Message".to_string(),
                };
                self.display_messages.push(DisplayMessage {
                    role: "notification".to_string(),
                    content: format!("{}\nFrom: {}\n\n{}", prefix, from, message),
                    tool_calls: Vec::new(),
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
                self.status_notice = Some(("Notification received".to_string(), Instant::now()));
            }
            _ => {}
        }
    }

    async fn handle_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        writer: &std::sync::Arc<tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>>,
    ) -> Result<()> {
        match code {
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
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
            KeyCode::Enter => {
                if !self.input.is_empty() && !self.is_processing {
                    let input = std::mem::take(&mut self.input);
                    self.cursor_pos = 0;

                    // Handle /reload specially
                    if input.trim() == "/reload" {
                        let request = Request::Reload {
                            id: self.next_request_id,
                        };
                        self.next_request_id += 1;
                        let json = serde_json::to_string(&request)? + "\n";
                        let mut w = writer.lock().await;
                        w.write_all(json.as_bytes()).await?;
                        return Ok(());
                    }

                    // Add user message to display
                    self.display_messages.push(DisplayMessage {
                        role: "user".to_string(),
                        content: input.clone(),
                        tool_calls: Vec::new(),
                        duration_secs: None,
                        title: None,
                        tool_data: None,
                    });

                    // Send to server
                    let request = Request::Message {
                        id: self.next_request_id,
                        content: input,
                    };
                    self.next_request_id += 1;
                    let json = serde_json::to_string(&request)? + "\n";
                    let mut w = writer.lock().await;
                    w.write_all(json.as_bytes()).await?;

                    self.is_processing = true;
                }
            }
            KeyCode::Esc => {
                if self.is_processing {
                    // Send cancel request to server
                    let request = Request::Cancel {
                        id: self.next_request_id,
                    };
                    self.next_request_id += 1;
                    let json = serde_json::to_string(&request)? + "\n";
                    let mut w = writer.lock().await;
                    w.write_all(json.as_bytes()).await?;
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
}

/// Generate a unified diff between two strings
fn generate_unified_diff(old: &str, new: &str, file_path: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    // Header
    output.push_str(&format!("--- a/{}\n", file_path));
    output.push_str(&format!("+++ b/{}\n", file_path));

    // Generate hunks
    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        output.push_str(&format!("{}", hunk));
    }

    output
}

impl TuiState for ClientApp {
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

    fn interleave_message(&self) -> Option<&str> {
        None
    }

    fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn provider_name(&self) -> String {
        self.provider_name.clone()
    }

    fn provider_model(&self) -> String {
        self.provider_model.clone()
    }

    fn mcp_servers(&self) -> Vec<String> {
        Vec::new() // Client doesn't track MCP servers yet
    }

    fn available_skills(&self) -> Vec<String> {
        Vec::new() // Client doesn't track skills yet
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

    fn elapsed(&self) -> Option<Duration> {
        self.processing_started.map(|t| t.elapsed())
    }

    fn status(&self) -> ProcessingStatus {
        self.status.clone()
    }

    fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        // Basic command suggestions for client
        if self.input.starts_with('/') {
            vec![
                ("/reload".into(), "Reload server code"),
                ("/quit".into(), "Quit client"),
            ]
        } else {
            Vec::new()
        }
    }

    fn active_skill(&self) -> Option<String> {
        None // Client doesn't track active skill yet
    }

    fn subagent_status(&self) -> Option<String> {
        None // Client doesn't track subagent status yet
    }

    fn time_since_activity(&self) -> Option<Duration> {
        self.last_activity.map(|t| t.elapsed())
    }

    fn total_session_tokens(&self) -> Option<(u64, u64)> {
        None // Deprecated client doesn't track total tokens
    }

    fn is_remote_mode(&self) -> bool {
        true // ClientApp is always remote mode
    }

    fn is_canary(&self) -> bool {
        false // Deprecated client doesn't support canary mode
    }

    fn show_diffs(&self) -> bool {
        true // Always show diffs in deprecated client
    }

    fn current_session_id(&self) -> Option<String> {
        self.session_id.clone()
    }

    fn session_display_name(&self) -> Option<String> {
        self.session_id
            .as_ref()
            .and_then(|id| crate::id::extract_session_name(id))
            .map(|s| s.to_string())
    }

    fn server_sessions(&self) -> Vec<String> {
        Vec::new() // Deprecated client doesn't track server sessions
    }

    fn connected_clients(&self) -> Option<usize> {
        None // Deprecated client doesn't track client count
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
        None // Rate limits handled by server in client mode
    }

    fn queue_mode(&self) -> bool {
        true // Deprecated client doesn't support immediate mode
    }

    fn context_info(&self) -> crate::prompt::ContextInfo {
        self.context_info.clone()
    }

    fn context_limit(&self) -> Option<usize> {
        None
    }

    fn client_update_available(&self) -> bool {
        false
    }

    fn server_update_available(&self) -> Option<bool> {
        None
    }

    fn info_widget_data(&self) -> super::info_widget::InfoWidgetData {
        // Deprecated client - return empty widget data
        super::info_widget::InfoWidgetData::default()
    }
}
