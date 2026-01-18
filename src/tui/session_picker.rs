//! Interactive session picker with preview
//!
//! Shows a list of sessions on the left, with a preview of the selected session's
//! conversation on the right.

use crate::id::session_icon;
use crate::message::{ContentBlock, Role};
use crate::session::{Session, SessionStatus};
use crate::storage;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::time::Duration;

/// Session info for display
#[derive(Clone)]
pub struct SessionInfo {
    pub id: String,
    pub short_name: String,
    pub icon: String,
    pub title: String,
    pub message_count: usize,
    pub last_message_time: chrono::DateTime<chrono::Utc>,
    pub working_dir: Option<String>,
    pub is_canary: bool,
    pub status: SessionStatus,
    pub messages_preview: Vec<PreviewMessage>,
}

#[derive(Clone)]
pub struct PreviewMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Load all sessions with their preview data
pub fn load_sessions() -> Result<Vec<SessionInfo>> {
    let sessions_dir = storage::jcode_dir()?.join("sessions");

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions: Vec<SessionInfo> = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(session) = Session::load(stem) {
                    let short_name = session.display_name().to_string();
                    let icon = session_icon(&short_name);

                    // Extract preview messages (last 10)
                    let messages_preview: Vec<PreviewMessage> = session
                        .messages
                        .iter()
                        .rev()
                        .take(20)
                        .rev()
                        .map(|msg| {
                            let role = match msg.role {
                                Role::User => "user",
                                Role::Assistant => "assistant",
                            };
                            let content: String = msg
                                .content
                                .iter()
                                .filter_map(|c| {
                                    match c {
                                        ContentBlock::Text { text, .. } => Some(text.clone()),
                                        ContentBlock::ToolUse { name, .. } => {
                                            Some(format!("[Tool: {}]", name))
                                        }
                                        ContentBlock::ToolResult { content, .. } => {
                                            // Truncate tool results (safely for UTF-8)
                                            if content.chars().count() > 200 {
                                                Some(format!("{}...", safe_truncate(content, 200)))
                                            } else {
                                                Some(content.clone())
                                            }
                                        }
                                        _ => None,
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("");
                            PreviewMessage {
                                role: role.to_string(),
                                content,
                                timestamp: None,
                            }
                        })
                        .collect();

                    sessions.push(SessionInfo {
                        id: stem.to_string(),
                        short_name,
                        icon: icon.to_string(),
                        title: session.title.unwrap_or_else(|| "Untitled".to_string()),
                        message_count: session.messages.len(),
                        last_message_time: session.updated_at,
                        working_dir: session.working_dir,
                        is_canary: session.is_canary,
                        status: session.status.clone(),
                        messages_preview,
                    });
                }
            }
        }
    }

    // Sort by last message time (most recent first)
    sessions.sort_by(|a, b| b.last_message_time.cmp(&a.last_message_time));

    Ok(sessions)
}

/// Safely truncate a string at a character boundary
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        s
    } else {
        let mut end = 0;
        for (i, (idx, _)) in s.char_indices().enumerate() {
            if i >= max_chars {
                break;
            }
            end = idx;
        }
        // Include the last character
        if let Some((idx, c)) = s.char_indices().nth(max_chars) {
            &s[..idx]
        } else {
            s
        }
    }
}

/// Format duration since a time in a human-readable way
fn format_time_ago(time: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(time);

    let seconds = duration.num_seconds();
    if seconds < 60 {
        return format!("{}s ago", seconds);
    }

    let minutes = duration.num_minutes();
    if minutes < 60 {
        return format!("{}m ago", minutes);
    }

    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{}h ago", hours);
    }

    let days = duration.num_days();
    if days < 7 {
        return format!("{}d ago", days);
    }

    if days < 30 {
        return format!("{}w ago", days / 7);
    }

    format!("{}mo ago", days / 30)
}

/// Interactive session picker
pub struct SessionPicker {
    sessions: Vec<SessionInfo>,
    list_state: ListState,
    scroll_offset: u16,
}

impl SessionPicker {
    pub fn new(sessions: Vec<SessionInfo>) -> Self {
        let mut list_state = ListState::default();
        if !sessions.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            sessions,
            list_state,
            scroll_offset: 0,
        }
    }

    pub fn selected_session(&self) -> Option<&SessionInfo> {
        self.list_state.selected().and_then(|i| self.sessions.get(i))
    }

    pub fn next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.sessions.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
        self.scroll_offset = 0; // Reset preview scroll on selection change
    }

    pub fn previous(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.sessions.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
        self.scroll_offset = 0;
    }

    pub fn scroll_preview_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(3);
    }

    pub fn scroll_preview_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(3);
    }

    fn render_session_list(&mut self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(idx, session)| {
                let is_selected = self.list_state.selected() == Some(idx);
                let time_ago = format_time_ago(session.last_message_time);

                // First line: icon + name + time
                let name_style = if is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let canary_marker = if session.is_canary { " 🔬" } else { "" };

                // Status indicator with color
                let (status_icon, status_color) = match &session.status {
                    SessionStatus::Active => ("▶", Color::Green),
                    SessionStatus::Closed => ("✓", Color::DarkGray),
                    SessionStatus::Crashed { .. } => ("💥", Color::Red),
                    SessionStatus::Reloaded => ("🔄", Color::Blue),
                    SessionStatus::Compacted => ("📦", Color::Yellow),
                    SessionStatus::RateLimited => ("⏳", Color::Magenta),
                    SessionStatus::Error { .. } => ("❌", Color::Red),
                };

                let line1 = Line::from(vec![
                    Span::styled(format!("{} ", session.icon), Style::default()),
                    Span::styled(&session.short_name, name_style),
                    Span::styled(canary_marker, Style::default().fg(Color::Yellow)),
                    Span::styled(format!(" {}", status_icon), Style::default().fg(status_color)),
                    Span::styled(
                        format!("  {}", time_ago),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);

                // Second line: title (truncated) + message count
                let title_display = if session.title.chars().count() > 35 {
                    format!("{}...", safe_truncate(&session.title, 32))
                } else {
                    session.title.clone()
                };

                let line2 = Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(title_display, Style::default().fg(Color::Gray)),
                    Span::styled(
                        format!("  ({} msgs)", session.message_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);

                // Third line: working dir (if available)
                let line3 = if let Some(ref dir) = session.working_dir {
                    let dir_display = if dir.chars().count() > 40 {
                        // Safe suffix truncation
                        let chars: Vec<char> = dir.chars().collect();
                        let suffix: String = chars.iter().rev().take(37).collect::<Vec<_>>().into_iter().rev().collect();
                        format!("...{}", suffix)
                    } else {
                        dir.clone()
                    };
                    Line::from(vec![
                        Span::styled("   ", Style::default()),
                        Span::styled(dir_display, Style::default().fg(Color::DarkGray)),
                    ])
                } else {
                    Line::from("")
                };

                ListItem::new(vec![line1, line2, line3, Line::from("")])
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Sessions (↑↓ navigate, Enter select, Esc quit) ")
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_preview(&self, frame: &mut Frame, area: Rect) {
        let Some(session) = self.selected_session() else {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" Preview ")
                .border_style(Style::default().fg(Color::DarkGray));
            let paragraph = Paragraph::new("No session selected")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
            return;
        };

        // Build preview content
        let mut lines: Vec<Line> = Vec::new();

        // Header with session info
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} {} ", session.icon, session.short_name),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format_time_ago(session.last_message_time),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        lines.push(Line::from(vec![Span::styled(
            &session.title,
            Style::default().fg(Color::White),
        )]));

        if let Some(ref dir) = session.working_dir {
            lines.push(Line::from(vec![Span::styled(
                format!("📁 {}", dir),
                Style::default().fg(Color::DarkGray),
            )]));
        }

        // Status line with details
        let (status_icon, status_text, status_color) = match &session.status {
            SessionStatus::Active => ("▶", "Active".to_string(), Color::Green),
            SessionStatus::Closed => ("✓", "Closed normally".to_string(), Color::DarkGray),
            SessionStatus::Crashed { message } => {
                let text = match message {
                    Some(msg) => format!("Crashed: {}", safe_truncate(msg, 40)),
                    None => "Crashed".to_string(),
                };
                ("💥", text, Color::Red)
            }
            SessionStatus::Reloaded => ("🔄", "Reloaded".to_string(), Color::Blue),
            SessionStatus::Compacted => ("📦", "Compacted (context too large)".to_string(), Color::Yellow),
            SessionStatus::RateLimited => ("⏳", "Rate limited".to_string(), Color::Magenta),
            SessionStatus::Error { message } => {
                let text = format!("Error: {}", safe_truncate(message, 40));
                ("❌", text, Color::Red)
            }
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", status_icon), Style::default().fg(status_color)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "─".repeat(area.width.saturating_sub(4) as usize),
            Style::default().fg(Color::DarkGray),
        )]));
        lines.push(Line::from(""));

        // Messages preview
        for msg in &session.messages_preview {
            let (role_style, role_prefix) = match msg.role.as_str() {
                "user" => (Style::default().fg(Color::Green), "You: "),
                "assistant" => (Style::default().fg(Color::Blue), "AI: "),
                _ => (Style::default().fg(Color::Gray), ""),
            };

            // Truncate long messages for preview
            let content = if msg.content.chars().count() > 500 {
                format!("{}...", safe_truncate(&msg.content, 497))
            } else {
                msg.content.clone()
            };

            // Skip empty messages
            if content.trim().is_empty() {
                continue;
            }

            lines.push(Line::from(vec![Span::styled(
                role_prefix,
                role_style.add_modifier(Modifier::BOLD),
            )]));

            // Wrap content into multiple lines
            for line in content.lines().take(10) {
                let max_width = (area.width as usize).saturating_sub(6);
                let display_line = if line.chars().count() > max_width {
                    let truncate_at = max_width.saturating_sub(3);
                    format!("{}...", safe_truncate(line, truncate_at))
                } else {
                    line.to_string()
                };
                lines.push(Line::from(vec![Span::styled(
                    format!("  {}", display_line),
                    Style::default().fg(Color::Gray),
                )]));
            }
            lines.push(Line::from(""));
        }

        if session.messages_preview.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "(empty session)",
                Style::default().fg(Color::DarkGray),
            )]));
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Preview (Shift+↑↓ scroll) ")
            .border_style(Style::default().fg(Color::Cyan));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_offset, 0));

        frame.render_widget(paragraph, area);
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(frame.area());

        self.render_session_list(frame, chunks[0]);
        self.render_preview(frame, chunks[1]);
    }

    /// Run the interactive picker, returns selected session ID or None if cancelled
    pub fn run(mut self) -> Result<Option<String>> {
        let mut terminal = ratatui::init();
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;

        let result = loop {
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            break Ok(None);
                        }
                        KeyCode::Enter => {
                            break Ok(self.selected_session().map(|s| s.id.clone()));
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                self.scroll_preview_down();
                            } else {
                                self.next();
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                self.scroll_preview_up();
                            } else {
                                self.previous();
                            }
                        }
                        KeyCode::PageDown => {
                            self.scroll_preview_down();
                            self.scroll_preview_down();
                            self.scroll_preview_down();
                        }
                        KeyCode::PageUp => {
                            self.scroll_preview_up();
                            self.scroll_preview_up();
                            self.scroll_preview_up();
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break Ok(None);
                        }
                        _ => {}
                    }
                }
            }
        };

        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        ratatui::restore();

        result
    }
}

/// Run the interactive session picker
/// Returns the selected session ID, or None if the user cancelled
pub fn pick_session() -> Result<Option<String>> {
    // Check if we have a TTY
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        anyhow::bail!("Session picker requires an interactive terminal. Use --resume <session_id> directly.");
    }

    let sessions = load_sessions()?;

    if sessions.is_empty() {
        eprintln!("No sessions found.");
        return Ok(None);
    }

    let picker = SessionPicker::new(sessions);
    picker.run()
}
