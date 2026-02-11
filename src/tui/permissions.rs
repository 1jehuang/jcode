use crate::safety::{self, PermissionRequest, Urgency};
use anyhow::Result;
use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};
use std::io::IsTerminal;
use std::time::Duration;

struct PermissionsApp {
    requests: Vec<PermissionRequest>,
    selected: usize,
    approved_count: usize,
    denied_count: usize,
    deny_input: Option<String>,
    done: bool,
}

impl PermissionsApp {
    fn new(requests: Vec<PermissionRequest>) -> Self {
        Self {
            requests,
            selected: 0,
            approved_count: 0,
            denied_count: 0,
            deny_input: None,
            done: false,
        }
    }

    fn selected_request(&self) -> Option<&PermissionRequest> {
        self.requests.get(self.selected)
    }

    fn next(&mut self) {
        if !self.requests.is_empty() {
            self.selected = (self.selected + 1).min(self.requests.len() - 1);
        }
    }

    fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn approve_selected(&mut self) {
        if let Some(req) = self.requests.get(self.selected) {
            let id = req.id.clone();
            let _ = safety::record_permission_via_file(&id, true, "permissions_tui", None);
            self.requests.remove(self.selected);
            self.approved_count += 1;
            if self.selected >= self.requests.len() && self.selected > 0 {
                self.selected -= 1;
            }
            if self.requests.is_empty() {
                self.done = true;
            }
        }
    }

    fn deny_selected(&mut self, reason: Option<String>) {
        if let Some(req) = self.requests.get(self.selected) {
            let id = req.id.clone();
            let _ = safety::record_permission_via_file(&id, false, "permissions_tui", reason);
            self.requests.remove(self.selected);
            self.denied_count += 1;
            if self.selected >= self.requests.len() && self.selected > 0 {
                self.selected -= 1;
            }
            if self.requests.is_empty() {
                self.done = true;
            }
        }
    }

    fn approve_all(&mut self) {
        while !self.requests.is_empty() {
            let id = self.requests[0].id.clone();
            let _ = safety::record_permission_via_file(&id, true, "permissions_tui", None);
            self.requests.remove(0);
            self.approved_count += 1;
        }
        self.selected = 0;
        self.done = true;
    }

    fn deny_all(&mut self) {
        while !self.requests.is_empty() {
            let id = self.requests[0].id.clone();
            let _ = safety::record_permission_via_file(&id, false, "permissions_tui", None);
            self.requests.remove(0);
            self.denied_count += 1;
        }
        self.selected = 0;
        self.done = true;
    }

    fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        if self.done {
            self.render_done(frame, area);
            return;
        }

        if self.requests.is_empty() {
            self.render_empty(frame, area);
            return;
        }

        let outer = Block::default()
            .title(format!(" Permissions ({} pending) ", self.requests.len()))
            .title_style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(80, 80, 90)));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(5),
                Constraint::Length(1),
                Constraint::Length(detail_height(inner.height)),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

        self.render_list(frame, chunks[0]);
        self.render_separator(frame, chunks[1]);
        self.render_detail(frame, chunks[2]);
        self.render_separator(frame, chunks[3]);
        self.render_help(frame, chunks[4]);
    }

    fn render_list(&self, frame: &mut Frame, area: Rect) {
        let now = Utc::now();
        let mut lines: Vec<Line> = Vec::new();

        for (i, req) in self.requests.iter().enumerate() {
            let is_selected = i == self.selected;
            let cursor = if is_selected { "❯" } else { " " };

            let (urgency_icon, urgency_color) = match req.urgency {
                Urgency::High => ("●", Color::Rgb(255, 100, 100)),
                Urgency::Normal => ("●", Color::Rgb(255, 200, 100)),
                Urgency::Low => ("○", Color::Rgb(120, 120, 130)),
            };

            let age = format_age(now - req.created_at);

            let action_style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(180, 180, 190))
            };

            let desc_style = if is_selected {
                Style::default().fg(Color::Rgb(160, 160, 170))
            } else {
                Style::default().fg(Color::Rgb(120, 120, 130))
            };

            let urgency_label = match req.urgency {
                Urgency::High => "high",
                Urgency::Normal => "normal",
                Urgency::Low => "low",
            };

            let action_text = format!(" [{}] {}", urgency_label, req.action);

            let remaining = area
                .width
                .saturating_sub(action_text.len() as u16 + age.len() as u16 + 6);
            let padding = " ".repeat(remaining as usize);

            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} ", cursor),
                    Style::default().fg(if is_selected {
                        Color::Rgb(140, 180, 255)
                    } else {
                        Color::Rgb(60, 60, 70)
                    }),
                ),
                Span::styled(
                    format!("{} ", urgency_icon),
                    Style::default().fg(urgency_color),
                ),
                Span::styled(action_text, action_style),
                Span::raw(padding),
                Span::styled(
                    format!("{} ", age),
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ),
            ]));

            let desc_text = truncate(&req.description, area.width.saturating_sub(8) as usize);
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(desc_text, desc_style),
            ]));

            if i < self.requests.len() - 1 {
                lines.push(Line::raw(""));
            }
        }

        let visible_height = area.height as usize;
        let lines_per_item = 3;
        let selected_start = self.selected * lines_per_item;
        let scroll = if selected_start + lines_per_item > visible_height {
            (selected_start + lines_per_item).saturating_sub(visible_height)
        } else {
            0
        };

        let para = Paragraph::new(lines).scroll((scroll as u16, 0));
        frame.render_widget(para, area);
    }

    fn render_separator(&self, frame: &mut Frame, area: Rect) {
        let sep = "─".repeat(area.width as usize);
        let line = Line::from(Span::styled(
            sep,
            Style::default().fg(Color::Rgb(60, 60, 70)),
        ));
        frame.render_widget(Paragraph::new(vec![line]), area);
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let Some(req) = self.selected_request() else {
            return;
        };

        let mut lines: Vec<Line> = Vec::new();

        let label_style = Style::default()
            .fg(Color::Rgb(140, 180, 255))
            .add_modifier(Modifier::BOLD);
        let value_style = Style::default().fg(Color::Rgb(180, 180, 190));

        lines.push(Line::from(vec![
            Span::styled(" Rationale: ", label_style),
            Span::styled(
                truncate(&req.rationale, area.width.saturating_sub(14) as usize),
                value_style,
            ),
        ]));

        if req.rationale.len() > (area.width.saturating_sub(14) as usize) {
            let rest = &req.rationale[(area.width.saturating_sub(14) as usize)..];
            for chunk in rest
                .as_bytes()
                .chunks(area.width.saturating_sub(2) as usize)
            {
                if let Ok(s) = std::str::from_utf8(chunk) {
                    lines.push(Line::from(vec![
                        Span::raw(" "),
                        Span::styled(s.to_string(), value_style),
                    ]));
                }
            }
        }

        lines.push(Line::raw(""));

        if let Some(ref ctx) = req.context {
            lines.push(Line::from(vec![Span::styled(" Context: ", label_style)]));
            let ctx_str = serde_json::to_string_pretty(ctx).unwrap_or_else(|_| format!("{}", ctx));
            for ctx_line in ctx_str.lines().take(area.height.saturating_sub(4) as usize) {
                lines.push(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        truncate(ctx_line, area.width.saturating_sub(5) as usize),
                        Style::default().fg(Color::Rgb(140, 140, 150)),
                    ),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled(" ID: ", label_style),
                Span::styled(
                    req.id.clone(),
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ),
            ]));

            lines.push(Line::from(vec![
                Span::styled(" Created: ", label_style),
                Span::styled(
                    req.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                    Style::default().fg(Color::Rgb(100, 100, 110)),
                ),
            ]));

            if req.wait {
                lines.push(Line::from(vec![
                    Span::styled(" ⏳ ", Style::default().fg(Color::Rgb(255, 200, 100))),
                    Span::styled(
                        "Agent is waiting for this decision",
                        Style::default().fg(Color::Rgb(255, 200, 100)),
                    ),
                ]));
            }
        }

        if let Some(ref deny_text) = self.deny_input {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled(
                    " Deny reason: ",
                    Style::default()
                        .fg(Color::Rgb(255, 100, 100))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{}▌", deny_text), Style::default().fg(Color::White)),
            ]));
        }

        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_items = if self.deny_input.is_some() {
            vec![("Enter", "confirm deny"), ("Esc", "cancel")]
        } else {
            vec![
                ("a", "approve"),
                ("d", "deny"),
                ("A", "approve all"),
                ("D", "deny all"),
                ("↑↓", "navigate"),
                ("q", "quit"),
            ]
        };

        let spans: Vec<Span> = help_items
            .iter()
            .enumerate()
            .flat_map(|(i, (key, desc))| {
                let mut s = vec![
                    Span::styled(
                        format!(" {} ", key),
                        Style::default()
                            .fg(Color::Rgb(30, 30, 35))
                            .bg(Color::Rgb(140, 180, 255)),
                    ),
                    Span::styled(
                        format!(" {} ", desc),
                        Style::default().fg(Color::Rgb(140, 140, 150)),
                    ),
                ];
                if i < help_items.len() - 1 {
                    s.push(Span::raw("  "));
                }
                s
            })
            .collect();

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_empty(&self, frame: &mut Frame, area: Rect) {
        let outer = Block::default()
            .title(" Permissions ")
            .title_style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(80, 80, 90)));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let lines = vec![
            Line::raw(""),
            Line::from(Span::styled(
                "  No pending permission requests.",
                Style::default().fg(Color::Rgb(120, 120, 130)),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                "  Press q to quit.",
                Style::default().fg(Color::Rgb(80, 80, 90)),
            )),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn render_done(&self, frame: &mut Frame, area: Rect) {
        let outer = Block::default()
            .title(" Permissions ")
            .title_style(
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(80, 80, 90)));
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let mut lines = vec![Line::raw("")];

        if self.approved_count > 0 {
            lines.push(Line::from(vec![Span::styled(
                format!("  ✓ {} approved", self.approved_count),
                Style::default().fg(Color::Rgb(100, 200, 100)),
            )]));
        }
        if self.denied_count > 0 {
            lines.push(Line::from(vec![Span::styled(
                format!("  ✗ {} denied", self.denied_count),
                Style::default().fg(Color::Rgb(255, 100, 100)),
            )]));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  Done! Press any key to exit.",
            Style::default().fg(Color::Rgb(140, 140, 150)),
        )));

        frame.render_widget(Paragraph::new(lines), inner);
    }

    pub fn run(mut self) -> Result<()> {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!("permissions viewer requires an interactive terminal");
        }

        let mut terminal = std::panic::catch_unwind(std::panic::AssertUnwindSafe(ratatui::init))
            .map_err(|payload| {
                let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                anyhow::anyhow!("failed to initialize terminal: {}", msg)
            })?;

        let result = loop {
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        if self.done {
                            break Ok(());
                        }

                        if let Some(ref mut text) = self.deny_input {
                            match key.code {
                                KeyCode::Enter => {
                                    let reason = if text.is_empty() {
                                        None
                                    } else {
                                        Some(text.clone())
                                    };
                                    self.deny_input = None;
                                    self.deny_selected(reason);
                                }
                                KeyCode::Esc => {
                                    self.deny_input = None;
                                }
                                KeyCode::Backspace => {
                                    text.pop();
                                }
                                KeyCode::Char(c) => {
                                    if key.modifiers.contains(KeyModifiers::CONTROL) && c == 'c' {
                                        break Ok(());
                                    }
                                    text.push(c);
                                }
                                _ => {}
                            }
                            continue;
                        }

                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                break Ok(())
                            }
                            KeyCode::Up | KeyCode::Char('k') => self.previous(),
                            KeyCode::Down | KeyCode::Char('j') => self.next(),
                            KeyCode::Char('a') => self.approve_selected(),
                            KeyCode::Char('d') => {
                                self.deny_input = Some(String::new());
                            }
                            KeyCode::Char('A') => self.approve_all(),
                            KeyCode::Char('D') => self.deny_all(),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        };

        ratatui::restore();
        result
    }
}

fn detail_height(total: u16) -> u16 {
    let min_list = 5;
    let help = 1;
    let separators = 2;
    let available = total.saturating_sub(min_list + help + separators);
    available.max(4).min(10)
}

fn format_age(duration: chrono::Duration) -> String {
    let secs = duration.num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        let mins = secs / 60;
        format!("{} min{} ago", mins, if mins == 1 { "" } else { "s" })
    } else if secs < 86400 {
        let hours = secs / 3600;
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else {
        let days = secs / 86400;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len > 3 {
        format!("{}…", &s[..max_len - 1])
    } else {
        s[..max_len].to_string()
    }
}

pub fn run_permissions() -> Result<()> {
    let requests = load_pending_requests()?;

    if requests.is_empty() {
        println!("No pending permission requests.");
        return Ok(());
    }

    println!(
        "{} pending permission request{}.",
        requests.len(),
        if requests.len() == 1 { "" } else { "s" }
    );

    let app = PermissionsApp::new(requests);
    app.run()
}

fn load_pending_requests() -> Result<Vec<PermissionRequest>> {
    let path = crate::storage::jcode_dir()?
        .join("safety")
        .join("queue.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let requests: Vec<PermissionRequest> = crate::storage::read_json(&path)?;
    Ok(requests)
}
