use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountProviderKind {
    Anthropic,
    OpenAi,
}

impl AccountProviderKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Self::Anthropic => "CLAUDE",
            Self::OpenAi => "OPENAI",
        }
    }
}

#[derive(Debug, Clone)]
pub enum AccountPickerCommand {
    Switch {
        provider: AccountProviderKind,
        label: String,
    },
    Login {
        provider: AccountProviderKind,
        label: String,
    },
    Remove {
        provider: AccountProviderKind,
        label: String,
    },
    PromptNew {
        provider: AccountProviderKind,
    },
}

#[derive(Debug, Clone)]
pub enum AccountPickerItemKind {
    Existing {
        label: String,
        masked_email: String,
        status: String,
        detail: String,
        is_active: bool,
    },
    NewAccount,
}

#[derive(Debug, Clone)]
pub struct AccountPickerItem {
    pub provider: AccountProviderKind,
    pub kind: AccountPickerItemKind,
}

impl AccountPickerItem {
    fn title(&self) -> String {
        match &self.kind {
            AccountPickerItemKind::Existing { label, .. } => label.clone(),
            AccountPickerItemKind::NewAccount => "Add account...".to_string(),
        }
    }

    fn subtitle(&self) -> String {
        match &self.kind {
            AccountPickerItemKind::Existing {
                masked_email,
                status,
                detail,
                is_active,
                ..
            } => {
                let active = if *is_active { "active" } else { "inactive" };
                if detail.is_empty() {
                    format!("{masked_email}  {status}  {active}")
                } else {
                    format!("{masked_email}  {status}  {detail}  {active}")
                }
            }
            AccountPickerItemKind::NewAccount => {
                "Start a browser login flow for a new labeled account".to_string()
            }
        }
    }

    fn actions(&self) -> &'static [&'static str] {
        match self.kind {
            AccountPickerItemKind::Existing { .. } => &["Switch", "Login", "Remove"],
            AccountPickerItemKind::NewAccount => &["Create"],
        }
    }

    fn matches_filter(&self, filter: &str) -> bool {
        if filter.is_empty() {
            return true;
        }
        let haystack = format!(
            "{} {} {}",
            self.provider.display_name(),
            self.title(),
            self.subtitle()
        )
        .to_lowercase();
        filter
            .split_whitespace()
            .all(|needle| haystack.contains(&needle.to_lowercase()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    List,
    Action,
}

#[derive(Debug, Clone)]
pub struct AccountPicker {
    title: String,
    items: Vec<AccountPickerItem>,
    filtered: Vec<usize>,
    selected: usize,
    selected_action: usize,
    filter: String,
    focus: FocusPane,
}

pub enum OverlayAction {
    Continue,
    Close,
    Execute(AccountPickerCommand),
}

impl AccountPicker {
    pub fn new(title: impl Into<String>, items: Vec<AccountPickerItem>) -> Self {
        let mut picker = Self {
            title: title.into(),
            items,
            filtered: Vec::new(),
            selected: 0,
            selected_action: 0,
            filter: String::new(),
            focus: FocusPane::List,
        };
        picker.apply_filter();
        picker
    }

    fn selected_item(&self) -> Option<&AccountPickerItem> {
        self.filtered
            .get(self.selected)
            .and_then(|idx| self.items.get(*idx))
    }

    fn apply_filter(&mut self) {
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| item.matches_filter(&self.filter).then_some(idx))
            .collect();
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        let max_action = self
            .selected_item()
            .map(|item| item.actions().len().saturating_sub(1))
            .unwrap_or(0);
        self.selected_action = self.selected_action.min(max_action);
    }

    fn command_for_selected(&self) -> Option<AccountPickerCommand> {
        let item = self.selected_item()?;
        match (&item.kind, self.selected_action) {
            (AccountPickerItemKind::Existing { label, .. }, 0) => {
                Some(AccountPickerCommand::Switch {
                    provider: item.provider,
                    label: label.clone(),
                })
            }
            (AccountPickerItemKind::Existing { label, .. }, 1) => {
                Some(AccountPickerCommand::Login {
                    provider: item.provider,
                    label: label.clone(),
                })
            }
            (AccountPickerItemKind::Existing { label, .. }, 2) => {
                Some(AccountPickerCommand::Remove {
                    provider: item.provider,
                    label: label.clone(),
                })
            }
            (AccountPickerItemKind::NewAccount, _) => Some(AccountPickerCommand::PromptNew {
                provider: item.provider,
            }),
            _ => None,
        }
    }

    pub fn handle_overlay_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<OverlayAction> {
        match code {
            KeyCode::Esc => {
                if !self.filter.is_empty() {
                    self.filter.clear();
                    self.apply_filter();
                    return Ok(OverlayAction::Continue);
                }
                return Ok(OverlayAction::Close);
            }
            KeyCode::Char('q') if !modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(OverlayAction::Close);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.focus == FocusPane::List {
                    self.selected = self.selected.saturating_sub(1);
                } else {
                    self.selected_action = self.selected_action.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.focus == FocusPane::List {
                    let max = self.filtered.len().saturating_sub(1);
                    self.selected = (self.selected + 1).min(max);
                } else if let Some(item) = self.selected_item() {
                    let max = item.actions().len().saturating_sub(1);
                    self.selected_action = (self.selected_action + 1).min(max);
                }
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.focus = FocusPane::List;
            }
            KeyCode::Right | KeyCode::Tab => {
                if self.selected_item().is_some() {
                    self.focus = FocusPane::Action;
                }
            }
            KeyCode::Backspace => {
                if self.filter.pop().is_some() {
                    self.apply_filter();
                }
            }
            KeyCode::Enter => {
                if self.filtered.is_empty() {
                    return Ok(OverlayAction::Close);
                }
                if self.focus == FocusPane::List
                    && self
                        .selected_item()
                        .map(|item| item.actions().len() > 1)
                        .unwrap_or(false)
                {
                    self.focus = FocusPane::Action;
                    return Ok(OverlayAction::Continue);
                }
                if let Some(command) = self.command_for_selected() {
                    return Ok(OverlayAction::Execute(command));
                }
            }
            KeyCode::Char(c)
                if !modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::ALT)
                    && !c.is_whitespace() =>
            {
                self.filter.push(c);
                self.apply_filter();
            }
            _ => {}
        }

        let max_action = self
            .selected_item()
            .map(|item| item.actions().len().saturating_sub(1))
            .unwrap_or(0);
        self.selected_action = self.selected_action.min(max_action);
        Ok(OverlayAction::Continue)
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = centered_rect(82, 65, frame.area());
        frame.render_widget(Clear, area);

        let block = Block::default()
            .title(format!(" {} ", self.title))
            .title_bottom(Line::from(vec![
                Span::styled(
                    " Enter ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::styled(" run action  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    " Tab ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::styled(" actions  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    " Esc ",
                    Style::default().fg(Color::White).bg(Color::DarkGray),
                ),
                Span::styled(" close/clear filter ", Style::default().fg(Color::DarkGray)),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(90, 95, 110)));
        frame.render_widget(block, area);

        let inner = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(8),
                Constraint::Length(5),
            ])
            .split(inner);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(54), Constraint::Percentage(46)])
            .split(rows[1]);

        let filter_line = vec![
            Span::styled("Search ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if self.filter.is_empty() {
                    "type to filter".to_string()
                } else {
                    self.filter.clone()
                },
                if self.filter.is_empty() {
                    Style::default().fg(Color::Gray).italic()
                } else {
                    Style::default().fg(Color::White)
                },
            ),
            Span::styled(
                format!("  {} results", self.filtered.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ];
        frame.render_widget(Paragraph::new(Line::from(filter_line)), rows[0]);

        let list_block = Block::default()
            .title(Span::styled(
                " Accounts ",
                if self.focus == FocusPane::List {
                    Style::default().fg(Color::White).bold()
                } else {
                    Style::default().fg(Color::Gray)
                },
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if self.focus == FocusPane::List {
                Color::Rgb(120, 140, 190)
            } else {
                Color::Rgb(70, 75, 90)
            }));

        let mut list_lines = Vec::new();
        if self.filtered.is_empty() {
            list_lines.push(Line::from(Span::styled(
                "No matching accounts",
                Style::default().fg(Color::Gray).italic(),
            )));
        } else {
            for (visible_idx, idx) in self.filtered.iter().enumerate() {
                let item = &self.items[*idx];
                let selected = visible_idx == self.selected;
                let provider_style = match item.provider {
                    AccountProviderKind::Anthropic => {
                        Style::default().fg(Color::Rgb(229, 187, 111))
                    }
                    AccountProviderKind::OpenAi => Style::default().fg(Color::Rgb(111, 214, 181)),
                };
                let row_style = if selected {
                    Style::default().bg(Color::Rgb(38, 42, 56))
                } else {
                    Style::default()
                };
                let title_style = match &item.kind {
                    AccountPickerItemKind::Existing {
                        is_active: true, ..
                    } => Style::default().fg(Color::White).bold(),
                    AccountPickerItemKind::Existing { .. } => Style::default().fg(Color::White),
                    AccountPickerItemKind::NewAccount => {
                        Style::default().fg(Color::Rgb(180, 190, 220))
                    }
                };
                list_lines.push(Line::from(vec![
                    Span::styled(
                        if selected { "▸ " } else { "  " },
                        row_style.fg(Color::White),
                    ),
                    Span::styled(
                        format!("{:<7}", item.provider.short_name()),
                        row_style.patch(provider_style),
                    ),
                    Span::styled(format!(" {}", item.title()), row_style.patch(title_style)),
                ]));
                list_lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        truncate_line(&item.subtitle(), cols[0].width.saturating_sub(2) as usize),
                        row_style.fg(Color::Gray),
                    ),
                ]));
                list_lines.push(Line::from(""));
            }
        }
        frame.render_widget(
            Paragraph::new(list_lines)
                .block(list_block)
                .wrap(Wrap { trim: false }),
            cols[0],
        );

        let detail_block = Block::default()
            .title(Span::styled(
                " Actions ",
                if self.focus == FocusPane::Action {
                    Style::default().fg(Color::White).bold()
                } else {
                    Style::default().fg(Color::Gray)
                },
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if self.focus == FocusPane::Action {
                Color::Rgb(120, 140, 190)
            } else {
                Color::Rgb(70, 75, 90)
            }));

        let detail_lines = if let Some(item) = self.selected_item() {
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    item.provider.display_name(),
                    Style::default().fg(Color::White).bold(),
                ),
                Span::raw("  "),
                Span::styled(item.title(), Style::default().fg(Color::Gray)),
            ])];
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                item.subtitle(),
                Style::default().fg(Color::Gray),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Available actions",
                Style::default().fg(Color::DarkGray).bold(),
            )));
            for (idx, action) in item.actions().iter().enumerate() {
                let selected = idx == self.selected_action;
                let style = if selected {
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(52, 70, 108))
                        .bold()
                } else {
                    Style::default().fg(Color::Gray)
                };
                lines.push(Line::from(Span::styled(format!("  {}  ", action), style)));
            }
            lines
        } else {
            vec![Line::from(Span::styled(
                "No accounts configured yet",
                Style::default().fg(Color::Gray),
            ))]
        };
        frame.render_widget(
            Paragraph::new(detail_lines)
                .block(detail_block)
                .wrap(Wrap { trim: false }),
            cols[1],
        );

        let footer = Paragraph::new(vec![
            Line::from(Span::styled(
                "Use this picker to switch accounts quickly, restart a login for an existing label, remove stale accounts, or create a new labeled login.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Tip ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Create asks for a label next, then launches the provider login flow.",
                    Style::default().fg(Color::White),
                ),
            ]),
        ]);
        frame.render_widget(footer, rows[2]);
    }
}

fn truncate_line(input: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= width {
        return input.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let mut out: String = chars.into_iter().take(width - 1).collect();
    out.push('…');
    out
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}
