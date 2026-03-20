use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};

const PANEL_BG: Color = Color::Rgb(24, 28, 40);
const PANEL_BORDER: Color = Color::Rgb(90, 95, 110);
const PANEL_BORDER_ACTIVE: Color = Color::Rgb(120, 140, 190);
const SECTION_BORDER: Color = Color::Rgb(70, 78, 94);
const SELECTED_BG: Color = Color::Rgb(38, 42, 56);
const MUTED: Color = Color::Rgb(140, 146, 163);
const MUTED_DARK: Color = Color::Rgb(100, 106, 122);
const OVERLAY_PERCENT_X: u16 = 88;
const OVERLAY_PERCENT_Y: u16 = 74;

#[derive(Debug, Clone)]
pub enum AccountProviderKind {
    Anthropic,
    OpenAi,
}

#[derive(Debug, Clone)]
pub enum AccountPickerCommand {
    SubmitInput(String),
    PromptValue {
        prompt: String,
        command_prefix: String,
        empty_value: Option<String>,
        status_notice: String,
    },
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
pub struct AccountPickerItem {
    pub provider_id: String,
    pub provider_label: String,
    pub title: String,
    pub subtitle: String,
    pub command: AccountPickerCommand,
}

impl AccountPickerItem {
    pub fn action(
        provider_id: impl Into<String>,
        provider_label: impl Into<String>,
        title: impl Into<String>,
        subtitle: impl Into<String>,
        command: AccountPickerCommand,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_label: provider_label.into(),
            title: title.into(),
            subtitle: subtitle.into(),
            command,
        }
    }

    fn matches_filter(&self, filter: &str) -> bool {
        if filter.is_empty() {
            return true;
        }
        let haystack = format!(
            "{} {} {} {} {}",
            self.provider_id,
            self.provider_label,
            self.title,
            self.subtitle,
            action_kind_label(&self.command)
        )
        .to_lowercase();
        filter
            .split_whitespace()
            .all(|needle| haystack.contains(&needle.to_lowercase()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct AccountPickerSummary {
    pub ready_count: usize,
    pub attention_count: usize,
    pub setup_count: usize,
    pub provider_count: usize,
    pub named_account_count: usize,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AccountPicker {
    title: String,
    items: Vec<AccountPickerItem>,
    filtered: Vec<usize>,
    selected: usize,
    filter: String,
    summary: Option<AccountPickerSummary>,
}

pub enum OverlayAction {
    Continue,
    Close,
    Execute(AccountPickerCommand),
}

impl AccountPicker {
    pub fn new(title: impl Into<String>, items: Vec<AccountPickerItem>) -> Self {
        Self::with_summary(title, items, AccountPickerSummary::default())
    }

    pub fn with_summary(
        title: impl Into<String>,
        items: Vec<AccountPickerItem>,
        summary: AccountPickerSummary,
    ) -> Self {
        let mut picker = Self {
            title: title.into(),
            items,
            filtered: Vec::new(),
            selected: 0,
            filter: String::new(),
            summary: Some(summary),
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
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(OverlayAction::Close);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = self.filtered.len().saturating_sub(1);
                self.selected = (self.selected + 1).min(max);
            }
            KeyCode::PageUp | KeyCode::Char('K') => {
                self.selected = self.selected.saturating_sub(6);
            }
            KeyCode::PageDown | KeyCode::Char('J') => {
                let max = self.filtered.len().saturating_sub(1);
                self.selected = (self.selected + 6).min(max);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = self.filtered.len().saturating_sub(1);
            }
            KeyCode::Backspace => {
                if self.filter.pop().is_some() {
                    self.apply_filter();
                }
            }
            KeyCode::Enter => {
                if let Some(item) = self.selected_item() {
                    return Ok(OverlayAction::Execute(item.command.clone()));
                }
                return Ok(OverlayAction::Close);
            }
            KeyCode::Char(c)
                if !modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::ALT) =>
            {
                self.filter.push(c);
                self.apply_filter();
            }
            _ => {}
        }
        Ok(OverlayAction::Continue)
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = centered_rect(OVERLAY_PERCENT_X, OVERLAY_PERCENT_Y, frame.area());

        let block = Block::default()
            .title(format!(" {} ", self.title))
            .title_bottom(Line::from(vec![
                hotkey(" Enter "),
                Span::styled(" run  ", Style::default().fg(MUTED_DARK)),
                hotkey(" ↑↓ "),
                Span::styled(" navigate  ", Style::default().fg(MUTED_DARK)),
                hotkey(" type "),
                Span::styled(" filter  ", Style::default().fg(MUTED_DARK)),
                hotkey(" Esc "),
                Span::styled(" clear / close ", Style::default().fg(MUTED_DARK)),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(PANEL_BORDER));
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
                Constraint::Length(4),
                Constraint::Min(12),
                Constraint::Length(2),
            ])
            .split(inner);

        self.render_header(frame, rows[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(rows[1]);

        self.render_action_list(frame, body[0]);
        self.render_detail_pane(frame, body[1]);

        let footer = Paragraph::new(Line::from(vec![
            Span::styled("Tip ", Style::default().fg(MUTED_DARK)),
            Span::styled(
                "Use `/account <provider> settings` for a full text view, or narrow this screen by typing a provider/account name.",
                Style::default().fg(MUTED),
            ),
        ]));
        frame.render_widget(footer, rows[2]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Overview ",
                Style::default().fg(Color::White).bold(),
            ))
            .borders(Borders::ALL)
            .style(Style::default().bg(PANEL_BG))
            .border_style(Style::default().fg(SECTION_BORDER));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines = vec![
            Line::from(vec![
                Span::styled("Filter ", Style::default().fg(MUTED_DARK)),
                Span::styled(
                    if self.filter.is_empty() {
                        "type provider, account, login, switch, or setting".to_string()
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
                    format!("  ·  {} results", self.filtered.len()),
                    Style::default().fg(MUTED_DARK),
                ),
            ]),
            self.summary_line(),
            self.defaults_line(),
        ];

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn render_action_list(&self, frame: &mut Frame, area: Rect) {
        let title = if self.filtered.is_empty() {
            " Actions ".to_string()
        } else {
            format!(" Actions ({}/{}) ", self.selected + 1, self.filtered.len())
        };
        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default().fg(Color::White).bold(),
            ))
            .borders(Borders::ALL)
            .style(Style::default().bg(PANEL_BG))
            .border_style(Style::default().fg(PANEL_BORDER_ACTIVE));
        let list_inner = block.inner(area);
        frame.render_widget(block, area);

        let available_items = ((list_inner.height as usize).max(3) / 3).max(1);
        let start = self
            .selected
            .saturating_sub(available_items.saturating_sub(1).min(available_items / 2));
        let end = (start + available_items).min(self.filtered.len());

        let mut lines = Vec::new();
        if self.filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "No matching account or provider actions.",
                Style::default().fg(Color::Gray).italic(),
            )));
            lines.push(Line::from(Span::styled(
                "Try `openai`, `claude`, `login`, `switch`, `remove`, or `default`.",
                Style::default().fg(MUTED),
            )));
        } else {
            let mut current_provider: Option<&str> = None;
            for visible_idx in start..end {
                let idx = self.filtered[visible_idx];
                let item = &self.items[idx];
                let selected = visible_idx == self.selected;

                if current_provider != Some(item.provider_id.as_str()) {
                    current_provider = Some(item.provider_id.as_str());
                    lines.push(provider_header_line(
                        &item.provider_label,
                        self.filtered
                            .iter()
                            .filter(|candidate_idx| {
                                self.items[**candidate_idx].provider_id == item.provider_id
                            })
                            .count(),
                        &item.provider_id,
                    ));
                }

                let row_style = if selected {
                    Style::default().bg(SELECTED_BG)
                } else {
                    Style::default()
                };
                let (kind_label, kind_color) = action_kind_badge(&item.command);
                lines.push(Line::from(vec![
                    Span::styled(
                        if selected { "▸ " } else { "  " },
                        row_style.fg(Color::White),
                    ),
                    Span::styled(
                        format!("[{}] ", kind_label),
                        row_style.fg(kind_color).bold(),
                    ),
                    Span::styled(item.title.clone(), row_style.fg(Color::White)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  ", row_style),
                    Span::styled(
                        item.provider_label.clone(),
                        row_style.patch(provider_style(&item.provider_id)),
                    ),
                    Span::styled(" · ", row_style.fg(MUTED_DARK)),
                    Span::styled(
                        truncate_with_ellipsis(
                            &item.subtitle,
                            list_inner.width.saturating_sub(6) as usize,
                        ),
                        row_style.fg(MUTED),
                    ),
                ]));
            }
        }

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), list_inner);
    }

    fn render_detail_pane(&self, frame: &mut Frame, area: Rect) {
        let title = self
            .selected_item()
            .map(|item| format!(" {} ", item.provider_label))
            .unwrap_or_else(|| " Details ".to_string());
        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default().fg(Color::White).bold(),
            ))
            .borders(Borders::ALL)
            .style(Style::default().bg(PANEL_BG))
            .border_style(Style::default().fg(SECTION_BORDER));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(item) = self.selected_item() else {
            frame.render_widget(
                Paragraph::new("No action selected").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        };

        let (kind_label, kind_color) = action_kind_badge(&item.command);
        let related_items: Vec<&AccountPickerItem> = self
            .items
            .iter()
            .filter(|candidate| {
                candidate.provider_id == item.provider_id && candidate.title != item.title
            })
            .take(4)
            .collect();

        let mut lines = vec![
            Line::from(vec![
                Span::styled("Action ", Style::default().fg(MUTED_DARK)),
                Span::styled(item.title.clone(), Style::default().fg(Color::White).bold()),
            ]),
            Line::from(vec![
                Span::styled("Type ", Style::default().fg(MUTED_DARK)),
                Span::styled(kind_label, Style::default().fg(kind_color).bold()),
            ]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Current state",
                Style::default().fg(MUTED_DARK).bold(),
            )]),
            Line::from(vec![Span::styled(
                item.subtitle.clone(),
                Style::default().fg(MUTED),
            )]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Runs",
                Style::default().fg(MUTED_DARK).bold(),
            )]),
            Line::from(vec![Span::styled(
                command_preview(&item.command),
                Style::default().fg(Color::White),
            )]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "What happens",
                Style::default().fg(MUTED_DARK).bold(),
            )]),
            Line::from(vec![Span::styled(
                action_kind_help(&item.command),
                Style::default().fg(MUTED),
            )]),
        ];

        if !related_items.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Other actions here",
                Style::default().fg(MUTED_DARK).bold(),
            )]));
            for related in related_items {
                lines.push(Line::from(vec![
                    Span::styled("• ", Style::default().fg(MUTED_DARK)),
                    Span::styled(related.title.clone(), Style::default().fg(Color::White)),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Press Enter to run this action.",
            Style::default().fg(Color::Rgb(170, 210, 255)),
        )]));

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn summary_line(&self) -> Line<'static> {
        if let Some(summary) = &self.summary {
            let mut spans = vec![
                metric_span("ready", summary.ready_count, Color::Rgb(110, 214, 158)),
                Span::raw("  "),
                metric_span(
                    "attention",
                    summary.attention_count,
                    Color::Rgb(255, 192, 120),
                ),
                Span::raw("  "),
                metric_span("setup", summary.setup_count, Color::Rgb(160, 168, 188)),
                Span::raw("  "),
                metric_span(
                    "providers",
                    summary.provider_count,
                    Color::Rgb(140, 176, 255),
                ),
            ];
            if summary.named_account_count > 0 {
                spans.push(Span::raw("  "));
                spans.push(metric_span(
                    "named accounts",
                    summary.named_account_count,
                    Color::Rgb(196, 170, 255),
                ));
            }
            return Line::from(spans);
        }

        Line::from(vec![Span::styled(
            format!("{} actions available", self.filtered.len()),
            Style::default().fg(MUTED),
        )])
    }

    fn defaults_line(&self) -> Line<'static> {
        let Some(summary) = &self.summary else {
            return Line::from(vec![Span::styled(
                "Type to narrow actions by provider, account label, or setting.",
                Style::default().fg(MUTED),
            )]);
        };

        let provider = summary.default_provider.as_deref().unwrap_or("auto");
        let model = summary
            .default_model
            .as_deref()
            .unwrap_or("provider default");

        Line::from(vec![
            Span::styled("Defaults ", Style::default().fg(MUTED_DARK)),
            Span::styled("provider ", Style::default().fg(MUTED_DARK)),
            Span::styled(provider.to_string(), Style::default().fg(Color::White)),
            Span::styled("  ·  model ", Style::default().fg(MUTED_DARK)),
            Span::styled(model.to_string(), Style::default().fg(Color::White)),
        ])
    }
}

fn hotkey(text: &'static str) -> Span<'static> {
    Span::styled(text, Style::default().fg(Color::White).bg(Color::DarkGray))
}

fn provider_header_line(provider_label: &str, count: usize, provider_id: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(provider_label.to_string(), provider_style(provider_id)),
        Span::styled(
            format!("  ·  {} actions", count),
            Style::default().fg(MUTED_DARK),
        ),
    ])
}

fn action_kind_label(command: &AccountPickerCommand) -> &'static str {
    match command {
        AccountPickerCommand::SubmitInput(input) if input.ends_with(" settings") => "overview",
        AccountPickerCommand::SubmitInput(input) if input.contains(" remove ") => "danger",
        AccountPickerCommand::SubmitInput(input) if input.contains(" login") => "login",
        AccountPickerCommand::SubmitInput(input) if input.contains(" add") => "account",
        AccountPickerCommand::SubmitInput(input) if input.contains(" switch ") => "account",
        AccountPickerCommand::PromptValue { .. } => "setting",
        AccountPickerCommand::Switch { .. } => "account",
        AccountPickerCommand::Login { .. } => "login",
        AccountPickerCommand::Remove { .. } => "danger",
        AccountPickerCommand::PromptNew { .. } => "account",
        AccountPickerCommand::SubmitInput(_) => "action",
    }
}

fn action_kind_badge(command: &AccountPickerCommand) -> (&'static str, Color) {
    match action_kind_label(command) {
        "overview" => ("overview", Color::Rgb(129, 184, 255)),
        "login" => ("login", Color::Rgb(111, 214, 181)),
        "setting" => ("setting", Color::Rgb(229, 187, 111)),
        "danger" => ("remove", Color::Rgb(255, 140, 140)),
        "account" => ("account", Color::Rgb(182, 154, 255)),
        _ => ("action", Color::Rgb(180, 190, 220)),
    }
}

fn action_kind_help(command: &AccountPickerCommand) -> &'static str {
    match command {
        AccountPickerCommand::SubmitInput(input) if input.ends_with(" settings") => {
            "Opens a detailed text summary for this provider, including the exact commands you can run manually."
        }
        AccountPickerCommand::SubmitInput(input) if input.contains(" remove ") => {
            "Removes saved credentials for the selected account. Use this when an account is stale or should no longer be available in jcode."
        }
        AccountPickerCommand::SubmitInput(input) if input.contains(" login") => {
            "Starts or refreshes authentication for this provider so it becomes usable again."
        }
        AccountPickerCommand::SubmitInput(input) if input.contains(" add") => {
            "Starts the flow for adding a new named account, so you can keep multiple identities side by side."
        }
        AccountPickerCommand::SubmitInput(input) if input.contains(" switch ") => {
            "Makes this account active so future requests use it immediately."
        }
        AccountPickerCommand::PromptValue { .. } => {
            "Prompts for a new value, then saves the matching provider or global setting."
        }
        AccountPickerCommand::Switch { .. } => {
            "Switches the active named account for this provider."
        }
        AccountPickerCommand::Login { .. } => {
            "Refreshes the selected account by starting the provider login flow again."
        }
        AccountPickerCommand::Remove { .. } => {
            "Deletes the saved account credentials from local storage."
        }
        AccountPickerCommand::PromptNew { .. } => {
            "Prompts for an account label first, then continues into the login flow."
        }
        AccountPickerCommand::SubmitInput(_) => {
            "Runs the selected account-management command immediately."
        }
    }
}

fn command_preview(command: &AccountPickerCommand) -> String {
    match command {
        AccountPickerCommand::SubmitInput(input) => input.clone(),
        AccountPickerCommand::PromptValue {
            command_prefix,
            empty_value,
            ..
        } => match empty_value {
            Some(value) => format!("{} <value>  (special: {} )", command_prefix, value),
            None => format!("{} <value>", command_prefix),
        },
        AccountPickerCommand::Switch { provider, label } => match provider {
            AccountProviderKind::Anthropic => format!("/account switch {}", label),
            AccountProviderKind::OpenAi => format!("/account openai switch {}", label),
        },
        AccountPickerCommand::Login { provider, label } => match provider {
            AccountProviderKind::Anthropic => format!("/account claude add {}", label),
            AccountProviderKind::OpenAi => format!("/account openai add {}", label),
        },
        AccountPickerCommand::Remove { provider, label } => match provider {
            AccountProviderKind::Anthropic => format!("/account claude remove {}", label),
            AccountProviderKind::OpenAi => format!("/account openai remove {}", label),
        },
        AccountPickerCommand::PromptNew { provider } => match provider {
            AccountProviderKind::Anthropic => "/account claude add <label>".to_string(),
            AccountProviderKind::OpenAi => "/account openai add <label>".to_string(),
        },
    }
}

fn metric_span(label: &'static str, value: usize, color: Color) -> Span<'static> {
    Span::styled(
        format!("{} {}", label, value),
        Style::default().fg(color).bold(),
    )
}

fn provider_style(provider_id: &str) -> Style {
    let color = match provider_id {
        "claude" => Color::Rgb(229, 187, 111),
        "openai" => Color::Rgb(111, 214, 181),
        "gemini" | "google" => Color::Rgb(129, 184, 255),
        "copilot" => Color::Rgb(182, 154, 255),
        "cursor" => Color::Rgb(131, 215, 255),
        "openrouter"
        | "openai-compatible"
        | "opencode"
        | "opencode-go"
        | "zai"
        | "chutes"
        | "cerebras"
        | "alibaba-coding-plan"
        | "jcode"
        | "defaults" => Color::Rgb(189, 200, 255),
        _ => Color::Rgb(180, 190, 220),
    };
    Style::default().fg(color).bold()
}

fn truncate_with_ellipsis(input: &str, width: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};

    #[test]
    fn test_account_picker_preserves_underlying_background_outside_panels() {
        let picker = AccountPicker::new(
            " Accounts ",
            vec![AccountPickerItem::action(
                "openai",
                "OpenAI",
                "Add account",
                "Start login flow",
                AccountPickerCommand::SubmitInput("/account openai add default".to_string()),
            )],
        );

        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).expect("failed to create terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                let fill = vec![Line::from("X".repeat(area.width as usize)); area.height as usize];
                frame.render_widget(Paragraph::new(fill), area);
                picker.render(frame);
            })
            .expect("draw failed");

        let overlay = centered_rect(
            OVERLAY_PERCENT_X,
            OVERLAY_PERCENT_Y,
            Rect::new(0, 0, 40, 12),
        );
        let probe = &terminal.backend().buffer()[(overlay.x + overlay.width - 3, overlay.y + 2)];
        assert_eq!(probe.symbol(), "X");
        assert_ne!(probe.bg, Color::Rgb(18, 21, 30));
    }

    #[test]
    fn test_prompt_value_command_preview_shows_placeholder() {
        let preview = command_preview(&AccountPickerCommand::PromptValue {
            prompt: "Enter default model".to_string(),
            command_prefix: "/account default-model".to_string(),
            empty_value: Some("clear".to_string()),
            status_notice: "editing".to_string(),
        });

        assert!(preview.contains("/account default-model <value>"));
        assert!(preview.contains("clear"));
    }
}
