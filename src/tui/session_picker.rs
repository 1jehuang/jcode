//! Interactive session picker with preview
//!
//! Shows a list of sessions on the left, with a preview of the selected session's
//! conversation on the right.

use crate::id::session_icon;
use crate::message::{ContentBlock, Role};
use crate::session::{Session, SessionStatus};
use crate::storage;
use crate::tui::markdown;
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
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_message_time: chrono::DateTime<chrono::Utc>,
    pub working_dir: Option<String>,
    pub is_canary: bool,
    pub status: SessionStatus,
    pub estimated_tokens: usize,
    pub messages_preview: Vec<PreviewMessage>,
}

#[derive(Clone)]
pub struct PreviewMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug)]
pub enum PickerResult {
    Selected(String),
    RestoreAllCrashed,
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
                if let Ok(mut session) = Session::load(stem) {
                    let updated = session.detect_crash();
                    if updated {
                        let _ = session.save();
                    }
                    let short_name = session.display_name().to_string();
                    let icon = session_icon(&short_name);

                    // Count messages and estimate tokens
                    let mut user_message_count = 0;
                    let mut assistant_message_count = 0;
                    let mut total_chars = 0;

                    for msg in &session.messages {
                        match msg.role {
                            Role::User => user_message_count += 1,
                            Role::Assistant => assistant_message_count += 1,
                        }
                        for block in &msg.content {
                            if let ContentBlock::Text { text, .. } = block {
                                total_chars += text.len();
                            }
                        }
                    }

                    // Rough token estimate: ~4 chars per token
                    let estimated_tokens = total_chars / 4;

                    // Extract preview messages (last 20)
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

                    let status = session.status.clone();

                    sessions.push(SessionInfo {
                        id: stem.to_string(),
                        short_name,
                        icon: icon.to_string(),
                        title: session.title.unwrap_or_else(|| "Untitled".to_string()),
                        message_count: session.messages.len(),
                        user_message_count,
                        assistant_message_count,
                        created_at: session.created_at,
                        last_message_time: session.updated_at,
                        working_dir: session.working_dir,
                        is_canary: session.is_canary,
                        status,
                        estimated_tokens,
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
    auto_scroll_preview: bool,
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
            auto_scroll_preview: true,
        }
    }

    pub fn selected_session(&self) -> Option<&SessionInfo> {
        self.list_state
            .selected()
            .and_then(|i| self.sessions.get(i))
    }

    pub fn next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                // Don't wrap - stay at bottom
                if i >= self.sessions.len() - 1 {
                    i
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
        self.scroll_offset = 0; // Reset preview scroll on selection change
        self.auto_scroll_preview = true;
    }

    pub fn previous(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                // Don't wrap - stay at top
                if i == 0 {
                    0
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
        self.scroll_offset = 0;
        self.auto_scroll_preview = true;
    }

    pub fn scroll_preview_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(3);
    }

    pub fn scroll_preview_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(3);
    }

    fn render_session_list(&mut self, frame: &mut Frame, area: Rect) {
        // Colors
        const DIM: Color = Color::Rgb(100, 100, 100);
        const DIMMER: Color = Color::Rgb(70, 70, 70);
        const USER_CLR: Color = Color::Rgb(138, 180, 248);
        const ACCENT: Color = Color::Rgb(186, 139, 255);

        let items: Vec<ListItem> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(idx, session)| {
                let is_selected = self.list_state.selected() == Some(idx);
                let last_msg_ago = format_time_ago(session.last_message_time);
                let created_ago = format_time_ago(session.created_at);

                // Name style
                let name_style = if is_selected {
                    Style::default()
                        .fg(Color::Rgb(140, 220, 160))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let canary_marker = if session.is_canary { " 🔬" } else { "" };

                // Status indicator with color
                let (status_icon, status_color) = match &session.status {
                    SessionStatus::Active => ("▶", Color::Rgb(100, 200, 100)),
                    SessionStatus::Closed => ("✓", DIM),
                    SessionStatus::Crashed { .. } => ("💥", Color::Rgb(220, 100, 100)),
                    SessionStatus::Reloaded => ("🔄", USER_CLR),
                    SessionStatus::Compacted => ("📦", Color::Rgb(255, 193, 7)),
                    SessionStatus::RateLimited => ("⏳", ACCENT),
                    SessionStatus::Error { .. } => ("❌", Color::Rgb(220, 100, 100)),
                };

                // Line 1: icon + name + status + last message time
                let line1 = Line::from(vec![
                    Span::styled(
                        format!("{} ", session.icon),
                        Style::default().fg(Color::Rgb(110, 210, 255)),
                    ),
                    Span::styled(&session.short_name, name_style),
                    Span::styled(canary_marker, Style::default().fg(Color::Rgb(255, 193, 7))),
                    Span::styled(
                        format!(" {}", status_icon),
                        Style::default().fg(status_color),
                    ),
                    Span::styled(
                        format!("  last: {}", last_msg_ago),
                        Style::default().fg(DIM),
                    ),
                ]);

                // Line 2: title (truncated)
                let title_display = if session.title.chars().count() > 45 {
                    format!("{}...", safe_truncate(&session.title, 42))
                } else {
                    session.title.clone()
                };
                let line2 = Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        title_display,
                        Style::default().fg(Color::Rgb(180, 180, 180)),
                    ),
                ]);

                // Line 3: stats - user msgs, assistant msgs, tokens
                let tokens_display = if session.estimated_tokens >= 1000 {
                    format!("~{}k tok", session.estimated_tokens / 1000)
                } else {
                    format!("~{} tok", session.estimated_tokens)
                };
                let line3 = Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        format!("{}", session.user_message_count),
                        Style::default().fg(USER_CLR),
                    ),
                    Span::styled(" user", Style::default().fg(DIMMER)),
                    Span::styled(" · ", Style::default().fg(DIMMER)),
                    Span::styled(
                        format!("{}", session.assistant_message_count),
                        Style::default().fg(Color::Rgb(129, 199, 132)),
                    ),
                    Span::styled(" assistant", Style::default().fg(DIMMER)),
                    Span::styled(" · ", Style::default().fg(DIMMER)),
                    Span::styled(tokens_display, Style::default().fg(DIMMER)),
                ]);

                // Line 4: created time + working dir
                let dir_part = if let Some(ref dir) = session.working_dir {
                    let dir_display = if dir.chars().count() > 30 {
                        let chars: Vec<char> = dir.chars().collect();
                        let suffix: String = chars
                            .iter()
                            .rev()
                            .take(27)
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                            .collect();
                        format!("...{}", suffix)
                    } else {
                        dir.clone()
                    };
                    format!("  📁 {}", dir_display)
                } else {
                    String::new()
                };
                let line4 = Line::from(vec![
                    Span::styled("   ", Style::default()),
                    Span::styled(
                        format!("created: {}", created_ago),
                        Style::default().fg(DIMMER),
                    ),
                    Span::styled(dir_part, Style::default().fg(DIMMER)),
                ]);

                ListItem::new(vec![line1, line2, line3, line4, Line::from("")])
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Sessions (↑↓ navigate, Enter select, R restore last crash, Esc quit) ")
                    .border_style(Style::default().fg(Color::Rgb(138, 180, 248))),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(40, 44, 52))
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        // Colors matching the actual TUI
        const USER_COLOR: Color = Color::Rgb(138, 180, 248); // Soft blue
        const USER_TEXT: Color = Color::Rgb(220, 220, 220); // Bright white for user text
        const DIM_COLOR: Color = Color::Rgb(100, 100, 100); // Dim gray
        const HEADER_ICON_COLOR: Color = Color::Rgb(110, 210, 255); // Cyan
        const HEADER_SESSION_COLOR: Color = Color::Rgb(140, 220, 160); // Soft green

        let Some(session) = self.selected_session().cloned() else {
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

        // Header matching TUI style
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", session.icon),
                Style::default().fg(HEADER_ICON_COLOR),
            ),
            Span::styled(
                &session.short_name,
                Style::default()
                    .fg(HEADER_SESSION_COLOR)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", format_time_ago(session.last_message_time)),
                Style::default().fg(DIM_COLOR),
            ),
        ]));

        // Title
        lines.push(Line::from(vec![Span::styled(
            &session.title,
            Style::default().fg(Color::White),
        )]));

        // Working directory
        if let Some(ref dir) = session.working_dir {
            lines.push(Line::from(vec![Span::styled(
                format!("📁 {}", dir),
                Style::default().fg(DIM_COLOR),
            )]));
        }

        // Status line with details
        let (status_icon, status_text, status_color) = match &session.status {
            SessionStatus::Active => ("▶", "Active".to_string(), Color::Rgb(100, 200, 100)),
            SessionStatus::Closed => ("✓", "Closed normally".to_string(), Color::DarkGray),
            SessionStatus::Crashed { message } => {
                let text = match message {
                    Some(msg) => format!("Crashed: {}", safe_truncate(msg, 40)),
                    None => "Crashed".to_string(),
                };
                ("💥", text, Color::Rgb(220, 100, 100))
            }
            SessionStatus::Reloaded => ("🔄", "Reloaded".to_string(), Color::Rgb(138, 180, 248)),
            SessionStatus::Compacted => (
                "📦",
                "Compacted (context too large)".to_string(),
                Color::Rgb(255, 193, 7),
            ),
            SessionStatus::RateLimited => {
                ("⏳", "Rate limited".to_string(), Color::Rgb(186, 139, 255))
            }
            SessionStatus::Error { message } => {
                let text = format!("Error: {}", safe_truncate(message, 40));
                ("❌", text, Color::Rgb(220, 100, 100))
            }
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", status_icon),
                Style::default().fg(status_color),
            ),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "─".repeat(area.width.saturating_sub(4) as usize),
            Style::default().fg(Color::Rgb(60, 60, 60)),
        )]));
        lines.push(Line::from(""));

        // Messages preview - styled like the actual TUI
        let mut prompt_num = 0;
        for msg in &session.messages_preview {
            // Truncate long messages for preview
            let content = if msg.content.chars().count() > 800 {
                format!("{}...", safe_truncate(&msg.content, 797))
            } else {
                msg.content.clone()
            };

            // Skip empty messages and tool results
            if content.trim().is_empty() {
                continue;
            }

            // Skip tool-related content (starts with [Tool:)
            if content.starts_with("[Tool:") {
                continue;
            }

            match msg.role.as_str() {
                "user" => {
                    prompt_num += 1;
                    // User messages: number + "› " + content (like TUI)
                    let first_line = content.lines().next().unwrap_or("");
                    let max_width = (area.width as usize).saturating_sub(8);
                    let display = if first_line.chars().count() > max_width {
                        format!(
                            "{}...",
                            safe_truncate(first_line, max_width.saturating_sub(3))
                        )
                    } else {
                        first_line.to_string()
                    };

                    lines.push(Line::from(vec![
                        Span::styled(format!("{}", prompt_num), Style::default().fg(USER_COLOR)),
                        Span::styled("› ", Style::default().fg(USER_COLOR)),
                        Span::styled(display, Style::default().fg(USER_TEXT)),
                    ]));

                    // Show additional lines if any (indented)
                    for line in content.lines().skip(1).take(3) {
                        let display = if line.chars().count() > max_width {
                            format!("{}...", safe_truncate(line, max_width.saturating_sub(3)))
                        } else {
                            line.to_string()
                        };
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled(display, Style::default().fg(USER_TEXT)),
                        ]));
                    }
                    if content.lines().count() > 4 {
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled("...", Style::default().fg(DIM_COLOR)),
                        ]));
                    }
                    lines.push(Line::from("")); // Spacing after user message
                }
                "assistant" => {
                    // AI messages: use actual markdown renderer
                    let max_width = (area.width as usize).saturating_sub(4);
                    let md_lines = markdown::render_markdown_with_width(&content, Some(max_width));

                    // Take first 12 lines of rendered markdown
                    for md_line in md_lines.into_iter().take(12) {
                        lines.push(md_line);
                    }
                    if content.lines().count() > 12 {
                        lines.push(Line::from(vec![Span::styled(
                            "...",
                            Style::default().fg(DIM_COLOR),
                        )]));
                    }
                    lines.push(Line::from("")); // Spacing after assistant message
                }
                _ => {}
            }
        }

        if session.messages_preview.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "(empty session)",
                Style::default().fg(DIM_COLOR),
            )]));
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Preview (Shift+↑↓/J/K scroll) ")
            .border_style(Style::default().fg(Color::Rgb(138, 180, 248)));

        let visible_height = area.height.saturating_sub(2) as usize;
        let max_scroll = lines.len().saturating_sub(visible_height) as u16;
        if self.auto_scroll_preview {
            self.scroll_offset = max_scroll;
            self.auto_scroll_preview = false;
        } else {
            self.scroll_offset = self.scroll_offset.min(max_scroll);
        }

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
    pub fn run(mut self) -> Result<Option<PickerResult>> {
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
                            break Ok(self
                                .selected_session()
                                .map(|s| PickerResult::Selected(s.id.clone())));
                        }
                        KeyCode::Char('R') => {
                            break Ok(Some(PickerResult::RestoreAllCrashed));
                        }
                        KeyCode::Down => {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                self.scroll_preview_down();
                            } else {
                                self.next();
                            }
                        }
                        KeyCode::Up => {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                self.scroll_preview_up();
                            } else {
                                self.previous();
                            }
                        }
                        KeyCode::Char('j') | KeyCode::Char('J') => {
                            if key.modifiers.contains(KeyModifiers::SHIFT)
                                || matches!(key.code, KeyCode::Char('J'))
                            {
                                self.scroll_preview_down();
                            } else {
                                self.next();
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Char('K') => {
                            if key.modifiers.contains(KeyModifiers::SHIFT)
                                || matches!(key.code, KeyCode::Char('K'))
                            {
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
pub fn pick_session() -> Result<Option<PickerResult>> {
    // Check if we have a TTY
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        anyhow::bail!(
            "Session picker requires an interactive terminal. Use --resume <session_id> directly."
        );
    }

    let sessions = load_sessions()?;

    if sessions.is_empty() {
        eprintln!("No sessions found.");
        return Ok(None);
    }

    let picker = SessionPicker::new(sessions);
    picker.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_inference() {
        // Load sessions and ensure status display works
        let sessions = load_sessions().unwrap();
        for session in &sessions {
            let _ = session.status.display();
        }
    }
}
