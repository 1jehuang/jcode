use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolCall};
use crate::provider::Provider;
use crate::skill::SkillRegistry;
use crate::tool::Registry;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::{
    DefaultTerminal,
    prelude::*,
};
use std::time::{Duration, Instant};

/// Queue mode for pending messages
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueMode {
    /// Insert message between tool calls (interrupt current flow)
    Interleave,
    /// Send message after current response completes
    #[default]
    AfterCompletion,
}

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

/// A queued message waiting to be sent
#[derive(Clone)]
pub struct QueuedMessage {
    pub content: String,
    pub mode: QueueMode,
}

/// A message in the conversation for display
#[derive(Clone)]
pub struct DisplayMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
}

/// TUI Application state
pub struct App {
    provider: Box<dyn Provider>,
    registry: Registry,
    skills: SkillRegistry,
    messages: Vec<Message>,
    display_messages: Vec<DisplayMessage>,
    input: String,
    cursor_pos: usize,
    scroll_offset: usize,
    active_skill: Option<String>,
    is_processing: bool,
    streaming_text: String,
    should_quit: bool,
    // Message queueing
    queued_messages: Vec<QueuedMessage>,
    queue_mode: QueueMode,
    /// Signal to interrupt current turn for interleaved message
    interrupt_for_message: bool,
    // Live token usage
    streaming_input_tokens: u64,
    streaming_output_tokens: u64,
    // Current status
    status: ProcessingStatus,
    processing_started: Option<Instant>,
    // Pending turn to process (allows UI to redraw before processing starts)
    pending_turn: bool,
}

impl App {
    pub fn new(provider: Box<dyn Provider>, registry: Registry) -> Self {
        let skills = SkillRegistry::load().unwrap_or_default();
        Self {
            provider,
            registry,
            skills,
            messages: Vec::new(),
            display_messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            active_skill: None,
            is_processing: false,
            streaming_text: String::new(),
            should_quit: false,
            queued_messages: Vec::new(),
            queue_mode: QueueMode::default(),
            interrupt_for_message: false,
            streaming_input_tokens: 0,
            streaming_output_tokens: 0,
            status: ProcessingStatus::default(),
            processing_started: None,
            pending_turn: false,
        }
    }

    /// Run the TUI application
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        loop {
            // Draw UI first - this ensures user sees their message before processing starts
            terminal.draw(|frame| crate::tui::ui::draw(frame, &self))?;

            // Process pending turn after UI redraw
            if self.pending_turn {
                self.pending_turn = false;
                self.process_turn().await;
            }

            // Handle input (non-blocking)
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key.code, key.modifiers)?;
                    }
                }
            }

            if self.should_quit {
                break;
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
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
                    return Ok(());
                }
                KeyCode::Char('u') => {
                    self.input.clear();
                    self.cursor_pos = 0;
                    return Ok(());
                }
                // Tab to toggle queue mode while processing
                KeyCode::Char('t') if self.is_processing => {
                    self.queue_mode = match self.queue_mode {
                        QueueMode::Interleave => QueueMode::AfterCompletion,
                        QueueMode::AfterCompletion => QueueMode::Interleave,
                    };
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
            KeyCode::Up => {
                if self.scroll_offset > 0 {
                    self.scroll_offset -= 1;
                }
            }
            KeyCode::Down => {
                self.scroll_offset += 1;
            }
            KeyCode::Esc => {
                self.input.clear();
                self.cursor_pos = 0;
            }
            KeyCode::Tab if self.is_processing => {
                // Tab to toggle queue mode while processing
                self.queue_mode = match self.queue_mode {
                    QueueMode::Interleave => QueueMode::AfterCompletion,
                    QueueMode::AfterCompletion => QueueMode::Interleave,
                };
            }
            _ => {}
        }

        Ok(())
    }

    /// Queue a message to be sent later
    fn queue_message(&mut self) {
        let content = std::mem::take(&mut self.input);
        self.cursor_pos = 0;

        // Show queued message in display immediately
        self.display_messages.push(DisplayMessage {
            role: "queued".to_string(),
            content: content.clone(),
            tool_calls: vec![match self.queue_mode {
                QueueMode::Interleave => "interleave".to_string(),
                QueueMode::AfterCompletion => "after".to_string(),
            }],
        });

        self.queued_messages.push(QueuedMessage {
            content,
            mode: self.queue_mode,
        });

        // If interleave mode, signal to interrupt current turn
        if self.queue_mode == QueueMode::Interleave {
            self.interrupt_for_message = true;
        }
    }

    /// Submit input - just sets up message and flags, processing happens in next loop iteration
    fn submit_input(&mut self) {
        let input = std::mem::take(&mut self.input);
        self.cursor_pos = 0;

        // Check for skill invocation
        if let Some(skill_name) = SkillRegistry::parse_invocation(&input) {
            if let Some(skill) = self.skills.get(skill_name) {
                self.active_skill = Some(skill_name.to_string());
                self.display_messages.push(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Activated skill: {} - {}", skill.name, skill.description),
                    tool_calls: vec![],
                });
            } else {
                self.display_messages.push(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Unknown skill: /{}", skill_name),
                    tool_calls: vec![],
                });
            }
            return;
        }

        // Add user message to display immediately
        self.display_messages.push(DisplayMessage {
            role: "user".to_string(),
            content: input.clone(),
            tool_calls: vec![],
        });
        self.messages.push(Message::user(&input));

        // Set up processing state - actual processing happens after UI redraws
        self.is_processing = true;
        self.streaming_text.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.processing_started = Some(Instant::now());
        self.pending_turn = true;
    }

    /// Process the pending turn (called from main loop after UI redraw)
    async fn process_turn(&mut self) {
        if let Err(e) = self.run_turn().await {
            self.display_messages.push(DisplayMessage {
                role: "error".to_string(),
                content: format!("Error: {}", e),
                tool_calls: vec![],
            });
        }

        // Process any queued "after completion" messages
        self.process_after_queue().await;

        self.is_processing = false;
        self.status = ProcessingStatus::Idle;
        self.processing_started = None;
    }

    /// Process messages queued for after completion
    async fn process_after_queue(&mut self) {
        // Keep processing until no more after-completion messages
        while let Some(idx) = self.queued_messages.iter().position(|m| m.mode == QueueMode::AfterCompletion) {
            let queued = self.queued_messages.remove(idx);

            // Update display: change "queued" to "user"
            if let Some(display_msg) = self.display_messages.iter_mut().rev()
                .find(|m| m.role == "queued" && m.content == queued.content)
            {
                display_msg.role = "user".to_string();
                display_msg.tool_calls.clear();
            }

            self.messages.push(Message::user(&queued.content));
            self.streaming_text.clear();
            self.streaming_input_tokens = 0;
            self.streaming_output_tokens = 0;

            if let Err(e) = self.run_turn().await {
                self.display_messages.push(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Error: {}", e),
                    tool_calls: vec![],
                });
            }
        }
    }

    async fn run_turn(&mut self) -> Result<()> {
        loop {
            // Check for interleaved messages before starting a new API call
            if self.process_interleaved_queue().await? {
                // An interleaved message was processed, continue the loop
                // to let the model respond to it
                continue;
            }

            let tools = self.registry.definitions().await;

            // Build system prompt with active skill
            let system_prompt = self.build_system_prompt();

            self.status = ProcessingStatus::Sending;
            let mut stream = self
                .provider
                .complete(&self.messages, &tools, &system_prompt)
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut first_event = true;

            while let Some(event) = stream.next().await {
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
                    StreamEvent::MessageEnd { .. } => break,
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

            if !content_blocks.is_empty() {
                self.messages.push(Message {
                    role: Role::Assistant,
                    content: content_blocks,
                });
            }

            // Add to display
            let tool_strs: Vec<String> = tool_calls
                .iter()
                .map(|tc| format!("[{}]", tc.name))
                .collect();

            self.display_messages.push(DisplayMessage {
                role: "assistant".to_string(),
                content: text_content,
                tool_calls: tool_strs,
            });
            self.streaming_text.clear();

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tools, checking for interleaved messages between each
            for tc in tool_calls {
                // Check for interleaved message before executing tool
                if self.interrupt_for_message {
                    self.interrupt_for_message = false;
                    // Process the interleaved message
                    if self.process_interleaved_queue().await? {
                        // Message was processed, tool results already in history
                        // Continue to let model respond to both
                    }
                }

                self.status = ProcessingStatus::RunningTool(tc.name.clone());
                let result = self.registry.execute(&tc.name, tc.input.clone()).await;
                let (output, is_error) = match result {
                    Ok(o) => (o, false),
                    Err(e) => (format!("Error: {}", e), true),
                };

                // Truncate for display
                let display_output = if output.len() > 500 {
                    format!("{}...", &output[..500])
                } else {
                    output.clone()
                };

                self.display_messages.push(DisplayMessage {
                    role: "tool".to_string(),
                    content: format!("[{}] {}", tc.name, display_output),
                    tool_calls: vec![],
                });

                self.messages.push(Message::tool_result(&tc.id, &output, is_error));
            }
        }

        Ok(())
    }

    /// Process any interleaved messages in the queue
    /// Returns true if a message was processed
    async fn process_interleaved_queue(&mut self) -> Result<bool> {
        if let Some(idx) = self.queued_messages.iter().position(|m| m.mode == QueueMode::Interleave) {
            let queued = self.queued_messages.remove(idx);

            // Update display: change "queued" to "user"
            if let Some(display_msg) = self.display_messages.iter_mut().rev()
                .find(|m| m.role == "queued" && m.content == queued.content)
            {
                display_msg.role = "user".to_string();
                display_msg.tool_calls.clear();
            }

            self.messages.push(Message::user(&queued.content));
            self.interrupt_for_message = false;
            Ok(true)
        } else {
            Ok(false)
        }
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

    pub fn queue_mode(&self) -> QueueMode {
        self.queue_mode
    }

    pub fn queued_count(&self) -> usize {
        self.queued_messages.len()
    }

    pub fn streaming_tokens(&self) -> (u64, u64) {
        (self.streaming_input_tokens, self.streaming_output_tokens)
    }

    pub fn status(&self) -> &ProcessingStatus {
        &self.status
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.processing_started.map(|t| t.elapsed())
    }
}
