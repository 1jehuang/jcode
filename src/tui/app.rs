use crate::bus::{Bus, BusEvent, ToolEvent, ToolStatus};
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
use tokio::time::interval;

/// Current processing status
#[derive(Clone, Default)]
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
}

/// TUI Application state
pub struct App {
    provider: Arc<dyn Provider>,
    registry: Registry,
    skills: SkillRegistry,
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
    // Live token usage
    streaming_input_tokens: u64,
    streaming_output_tokens: u64,
    // Track last streaming activity for "stale" detection
    last_stream_activity: Option<Instant>,
    // Current status
    status: ProcessingStatus,
    processing_started: Option<Instant>,
    // Pending turn to process (allows UI to redraw before processing starts)
    pending_turn: bool,
    // Tool calls detected during streaming (shown in real-time)
    streaming_tool_calls: Vec<String>,
    // Provider-specific session ID for conversation resume
    provider_session_id: Option<String>,
}

impl App {
    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let skills = SkillRegistry::load().unwrap_or_default();
        Self {
            provider,
            registry,
            skills,
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
            last_stream_activity: None,
            status: ProcessingStatus::default(),
            processing_started: None,
            pending_turn: false,
            streaming_tool_calls: Vec::new(),
            provider_session_id: None,
        }
    }

    /// Run the TUI application
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
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
                        // Just redraw on next iteration
                    }
                    event = event_stream.next() => {
                        if let Some(Ok(Event::Key(key))) = event {
                            if key.kind == KeyEventKind::Press {
                                self.handle_key(key.code, key.modifiers)?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Process turn while still accepting input for queueing
    async fn process_turn_with_input(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) {
        use tokio::select;

        // We need to run the turn logic step by step, checking for input between steps
        // For now, run the turn but poll for input during streaming

        if let Err(e) = self.run_turn_interactive(terminal, event_stream).await {
            self.display_messages.push(DisplayMessage {
                role: "error".to_string(),
                content: format!("Error: {}", e),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
            });
        }

        // Process any queued messages
        self.process_queued_messages(terminal, event_stream).await;

        self.is_processing = false;
        self.status = ProcessingStatus::Idle;
        self.processing_started = None;
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
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
                    // Ctrl+K: kill to end of line
                    self.input.truncate(self.cursor_pos);
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
                self.scroll_offset += if code == KeyCode::PageUp { 10 } else { 1 };
            }
            KeyCode::Down | KeyCode::PageDown => {
                // Scroll down (decrease offset, 0 = bottom)
                let dec = if code == KeyCode::PageDown { 10 } else { 1 };
                self.scroll_offset = self.scroll_offset.saturating_sub(dec);
            }
            KeyCode::Esc => {
                // Reset scroll to bottom and clear input
                self.scroll_offset = 0;
                self.input.clear();
                self.cursor_pos = 0;
            }
            _ => {}
        }

        Ok(())
    }

    /// Queue a message to be sent later
    fn queue_message(&mut self) {
        let content = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        self.queued_messages.push(content);
    }

    /// Submit input - just sets up message and flags, processing happens in next loop iteration
    fn submit_input(&mut self) {
        let input = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        self.scroll_offset = 0; // Reset to bottom on new input

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
                });
            } else {
                self.display_messages.push(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Unknown skill: /{}", skill_name),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                });
            }
            return;
        }

        // Add user message to display immediately
        self.display_messages.push(DisplayMessage {
            role: "user".to_string(),
            content: input.clone(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
        });
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
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.processing_started = Some(Instant::now());
        self.pending_turn = true;
    }

    /// Process all queued messages (combined into a single request)
    async fn process_queued_messages(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) {
        if self.queued_messages.is_empty() {
            return;
        }

        // Combine all queued messages into one
        let combined = std::mem::take(&mut self.queued_messages).join("\n\n");

        // Add user message to display
        self.display_messages.push(DisplayMessage {
            role: "user".to_string(),
            content: combined.clone(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
        });

        self.messages.push(Message::user(&combined));
        self.session.add_message(
            Role::User,
            vec![ContentBlock::Text { text: combined }],
        );
        let _ = self.session.save();
        self.streaming_text.clear();
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
            });
        }
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

            while let Some(event) = stream.next().await {
                // Track activity for status display
                self.last_stream_activity = Some(Instant::now());

                if first_event {
                    self.status = ProcessingStatus::Streaming;
                    first_event = false;
                }
                match event? {
                    StreamEvent::TextDelta(text) => {
                        self.streaming_text.push_str(&text);
                        text_content.push_str(&text);
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        self.streaming_tool_calls.push(name.clone());
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

            // Add to display
            let tool_strs: Vec<String> = tool_calls
                .iter()
                .map(|tc| format!("[{}]", tc.name))
                .collect();

            let duration = self.processing_started.map(|t| t.elapsed().as_secs_f32());
            self.display_messages.push(DisplayMessage {
                role: "assistant".to_string(),
                content: text_content,
                tool_calls: tool_strs,
                duration_secs: if tool_calls.is_empty() { duration } else { None },
                title: None,
            });
            self.streaming_text.clear();
            self.streaming_tool_calls.clear();

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tools
            for tc in tool_calls {
                self.status = ProcessingStatus::RunningTool(tc.name.clone());
                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());
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
                let (output, is_error) = match result {
                    Ok(o) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Completed,
                            title: o.title.clone(),
                        }));
                        (o.output, false)
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
                        (format!("Error: {}", e), true)
                    }
                };

                // Truncate for display
                let display_output = if output.len() > 500 {
                    format!("{}...", &output[..500])
                } else {
                    output.clone()
                };

                self.display_messages.push(DisplayMessage {
                    role: "tool".to_string(),
                    content: display_output,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: Some(tc.name.clone()),
                });

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
            // Redraw to show "sending" status
            terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;

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

            // Stream with input handling
            loop {
                tokio::select! {
                    // Redraw periodically
                    _ = redraw_interval.tick() => {
                        terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
                    }
                    // Handle keyboard input
                    event = event_stream.next() => {
                        if let Some(Ok(Event::Key(key))) = event {
                            if key.kind == KeyEventKind::Press {
                                let _ = self.handle_key(key.code, key.modifiers);
                            }
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
                                        self.streaming_text.push_str(&text);
                                        text_content.push_str(&text);
                                    }
                                    StreamEvent::ToolUseStart { id, name } => {
                                        self.streaming_tool_calls.push(name.clone());
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
                                            tool_calls.push(tool);
                                            current_tool_input.clear();
                                        }
                                    }
                                    StreamEvent::TokenUsage { input_tokens, output_tokens } => {
                                        if let Some(input) = input_tokens {
                                            self.streaming_input_tokens = input;
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

            // Add to display
            let tool_strs: Vec<String> = tool_calls
                .iter()
                .map(|tc| format!("[{}]", tc.name))
                .collect();

            let duration = self.processing_started.map(|t| t.elapsed().as_secs_f32());
            self.display_messages.push(DisplayMessage {
                role: "assistant".to_string(),
                content: text_content,
                tool_calls: tool_strs,
                duration_secs: if tool_calls.is_empty() { duration } else { None },
                title: None,
            });
            self.streaming_text.clear();
            self.streaming_tool_calls.clear();

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tools with input handling
            for tc in tool_calls {
                self.status = ProcessingStatus::RunningTool(tc.name.clone());
                terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;

                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());
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
                let (output, is_error) = match result {
                    Ok(o) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Completed,
                            title: o.title.clone(),
                        }));
                        (o.output, false)
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
                        (format!("Error: {}", e), true)
                    }
                };

                let display_output = if output.len() > 500 {
                    format!("{}...", &output[..500])
                } else {
                    output.clone()
                };

                self.display_messages.push(DisplayMessage {
                    role: "tool".to_string(),
                    content: display_output,
                    tool_calls: vec![],
                    duration_secs: None,
                    title: Some(tc.name.clone()),
                });

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
        const BASE_PROMPT: &str = r#"You are a coding assistant with access to tools for file operations and shell commands.

## Available Tools
- bash: Execute shell commands
- read: Read file contents
- write: Create or overwrite files
- edit: Edit files by replacing text
- glob: Find files by pattern
- grep: Search file contents with regex
- ls: List directory contents

## Guidelines
1. Use tools to explore and modify the codebase
2. Read files before editing to understand current state
3. Use glob/grep to find relevant files
4. Prefer edit over write for existing files
5. Keep responses concise and action-focused
6. Execute commands to verify changes work

When you need to make changes, use the tools directly. Don't just describe what to do."#;

        if let Some(ref skill_name) = self.active_skill {
            if let Some(skill) = self.skills.get(skill_name) {
                return format!("{}\n\n{}", BASE_PROMPT, skill.get_prompt());
            }
        }
        BASE_PROMPT.to_string()
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

    /// Time since last streaming event (for detecting stale connections)
    pub fn time_since_activity(&self) -> Option<Duration> {
        self.last_stream_activity.map(|t| t.elapsed())
    }

    pub fn streaming_tool_calls(&self) -> &[String] {
        &self.streaming_tool_calls
    }

    pub fn status(&self) -> &ProcessingStatus {
        &self.status
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.processing_started.map(|t| t.elapsed())
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
}
