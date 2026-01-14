//! TUI client that connects to jcode server
//!
//! This provides a full TUI experience while using the server for processing.
//! Benefits:
//! - Server maintains Claude session (caching)
//! - Can hot-reload server without losing TUI
//! - TUI can reconnect after server restart

use crate::protocol::{Request, ServerEvent};
use crate::server;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::interval;

/// Display message for client TUI
#[derive(Clone)]
pub struct DisplayMessage {
    pub role: String,
    pub content: String,
}

/// Client TUI state
pub struct ClientApp {
    display_messages: Vec<DisplayMessage>,
    input: String,
    cursor_pos: usize,
    is_processing: bool,
    streaming_text: String,
    should_quit: bool,
    session_id: Option<String>,
    next_request_id: u64,
    server_disconnected: bool,
    has_loaded_history: bool,
}

impl ClientApp {
    pub fn new() -> Self {
        Self {
            display_messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            is_processing: false,
            streaming_text: String::new(),
            should_quit: false,
            session_id: None,
            next_request_id: 1,
            server_disconnected: false,
            has_loaded_history: false,
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
        let request = Request::GetHistory { id: self.next_request_id };
        self.next_request_id += 1;
        let json = serde_json::to_string(&request)? + "\n";
        writer.write_all(json.as_bytes()).await?;

        // Read response
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let event: ServerEvent = serde_json::from_str(&line)?;

        if let ServerEvent::History { session_id, messages, .. } = event {
            self.session_id = Some(session_id);
            for msg in messages {
                self.display_messages.push(DisplayMessage {
                    role: msg.role,
                    content: msg.content,
                });
            }
        }

        Ok(())
    }

    /// Run the client TUI with auto-reconnection
    pub async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let mut event_stream = EventStream::new();
        let mut reconnect_attempts = 0;
        const MAX_RECONNECT_ATTEMPTS: u32 = 30;  // 30 seconds max

        'outer: loop {
            // Connect to server
            let stream = match self.connect().await {
                Ok(s) => {
                    reconnect_attempts = 0;
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
                            content: "Failed to reconnect after 30 seconds. Press Ctrl+C to quit.".to_string(),
                        });
                        terminal.draw(|frame| self.draw(frame))?;
                        // Wait for quit
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
                    // Wait and retry
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    terminal.draw(|frame| self.draw(frame))?;
                    continue;
                }
            };

            // Show reconnection success message if we were reconnecting
            if reconnect_attempts > 0 {
                self.display_messages.push(DisplayMessage {
                    role: "system".to_string(),
                    content: "Reconnected to server.".to_string(),
                });
            }

            let (reader, writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let writer = std::sync::Arc::new(tokio::sync::Mutex::new(writer));
            let mut redraw_interval = interval(Duration::from_millis(50));
            let mut server_line = String::new();

            // Subscribe to server events and get history
            {
                // Subscribe first
                let request = Request::Subscribe { id: self.next_request_id };
                self.next_request_id += 1;
                let json = serde_json::to_string(&request)? + "\n";
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;

                // Request history to restore display state
                let request = Request::GetHistory { id: self.next_request_id };
                self.next_request_id += 1;
                let json = serde_json::to_string(&request)? + "\n";
                w.write_all(json.as_bytes()).await?;
            }

            // Main event loop
            loop {
                // Draw UI
                terminal.draw(|frame| self.draw(frame))?;

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
                                });
                                terminal.draw(|frame| self.draw(frame))?;
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
            ServerEvent::Done { .. } => {
                if !self.streaming_text.is_empty() {
                    self.display_messages.push(DisplayMessage {
                        role: "assistant".to_string(),
                        content: std::mem::take(&mut self.streaming_text),
                    });
                }
                self.is_processing = false;
            }
            ServerEvent::Error { message, .. } => {
                self.display_messages.push(DisplayMessage {
                    role: "error".to_string(),
                    content: message,
                });
                self.is_processing = false;
            }
            ServerEvent::SessionId { session_id } => {
                self.session_id = Some(session_id);
            }
            ServerEvent::Reloading { .. } => {
                self.display_messages.push(DisplayMessage {
                    role: "system".to_string(),
                    content: "Server is reloading... Will reconnect shortly.".to_string(),
                });
            }
            ServerEvent::History { messages, session_id, .. } => {
                self.session_id = Some(session_id);
                // Only load history on first connect, not on reconnect
                // (we already have display_messages in memory on reconnect)
                if !self.has_loaded_history {
                    self.has_loaded_history = true;
                    for msg in messages {
                        self.display_messages.push(DisplayMessage {
                            role: msg.role,
                            content: msg.content,
                        });
                    }
                }
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
                        let request = Request::Reload { id: self.next_request_id };
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
                self.input.clear();
                self.cursor_pos = 0;
            }
            _ => {}
        }
        Ok(())
    }

    fn draw(&self, frame: &mut ratatui::Frame) {
        use ratatui::layout::{Constraint, Direction, Layout};
        use ratatui::style::{Color, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),  // Header
                Constraint::Min(1),     // Messages
                Constraint::Length(3),  // Input
            ])
            .split(frame.area());

        // Header
        let status = if self.server_disconnected {
            "Reconnecting..."
        } else if self.is_processing {
            "Processing..."
        } else {
            "Connected"
        };
        let header = Paragraph::new(format!("jcode client | {} | session: {}",
            status,
            self.session_id.as_deref().unwrap_or("none")
        ))
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(header, chunks[0]);

        // Messages
        let mut lines: Vec<Line> = Vec::new();
        for msg in &self.display_messages {
            let style = match msg.role.as_str() {
                "user" => Style::default().fg(Color::Cyan),
                "assistant" => Style::default().fg(Color::White),
                "system" => Style::default().fg(Color::Yellow),
                "error" => Style::default().fg(Color::Red),
                _ => Style::default(),
            };
            lines.push(Line::from(Span::styled(
                format!("[{}] {}", msg.role, msg.content),
                style,
            )));
        }
        if !self.streaming_text.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("[assistant] {}", self.streaming_text),
                Style::default().fg(Color::White),
            )));
        }
        let messages = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::NONE));
        frame.render_widget(messages, chunks[1]);

        // Input
        let input_text = if self.input.is_empty() && !self.is_processing {
            "Type a message...".to_string()
        } else {
            self.input.clone()
        };
        let input = Paragraph::new(input_text)
            .style(if self.input.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            })
            .block(Block::default().borders(Borders::ALL).title("Input"));
        frame.render_widget(input, chunks[2]);

        // Cursor
        if !self.is_processing {
            frame.set_cursor_position((
                chunks[2].x + 1 + self.cursor_pos as u16,
                chunks[2].y + 1,
            ));
        }
    }
}
