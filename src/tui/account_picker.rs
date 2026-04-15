use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::collections::HashMap;

#[path = "account_picker_render.rs"]
mod render_support;
use render_support::{
    ActionSection, account_count_summary, account_is_active, action_icon, action_kind_badge,
    action_kind_help, action_kind_label, action_section, centered_rect, command_preview,
    compact_item_title, hotkey, metric_span, provider_header_line, provider_style,
    truncate_with_ellipsis,
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
    OpenAccountCenter {
        provider_filter: Option<String>,
    },
    OpenAddReplaceFlow {
        provider_filter: Option<String>,
    },
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
        let provider_order = self.provider_order();
        self.filtered.sort_by(|left, right| {
            let left_item = &self.items[*left];
            let right_item = &self.items[*right];

            provider_order
                .get(&left_item.provider_id)
                .cmp(&provider_order.get(&right_item.provider_id))
                .then_with(|| action_section(left_item).cmp(&action_section(right_item)))
                .then_with(|| left_item.title.cmp(&right_item.title))
                .then_with(|| left.cmp(right))
        });
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    fn provider_order(&self) -> HashMap<String, usize> {
        let mut order = HashMap::new();
        let mut next = 0usize;
        for item in &self.items {
            if order.contains_key(&item.provider_id) {
                continue;
            }
            let rank = if item.provider_id == "defaults" {
                usize::MAX / 2
            } else {
                let current = next;
                next += 1;
                current
            };
            order.insert(item.provider_id.clone(), rank);
        }
        order
    }

    fn filtered_provider_switch_count(&self, provider_id: &str) -> usize {
        self.filtered
            .iter()
            .filter(|idx| {
                let item = &self.items[**idx];
                item.provider_id == provider_id
                    && matches!(action_section(item), ActionSection::Switch)
            })
            .count()
    }

    fn filtered_provider_secondary_count(&self, provider_id: &str) -> usize {
        self.filtered
            .iter()
            .filter(|idx| {
                let item = &self.items[**idx];
                item.provider_id == provider_id
                    && !matches!(action_section(item), ActionSection::Switch)
            })
            .count()
    }

    fn select_prev_provider_group(&mut self) {
        let Some(current_idx) = self.filtered.get(self.selected).copied() else {
            return;
        };
        let current_provider = self.items[current_idx].provider_id.as_str();
        let mut target = None;

        for pos in (0..self.selected).rev() {
            let provider_id = self.items[self.filtered[pos]].provider_id.as_str();
            if provider_id != current_provider {
                target = Some(pos);
                break;
            }
        }

        let Some(mut pos) = target else {
            return;
        };
        let provider_id = self.items[self.filtered[pos]].provider_id.clone();
        while pos > 0 && self.items[self.filtered[pos - 1]].provider_id == provider_id {
            pos -= 1;
        }
        self.selected = pos;
    }

    fn select_next_provider_group(&mut self) {
        let Some(current_idx) = self.filtered.get(self.selected).copied() else {
            return;
        };
        let current_provider = self.items[current_idx].provider_id.as_str();

        for pos in (self.selected + 1)..self.filtered.len() {
            let provider_id = self.items[self.filtered[pos]].provider_id.as_str();
            if provider_id != current_provider {
                self.selected = pos;
                break;
            }
        }
    }

    fn provider_overview_line(&self) -> Line<'static> {
        let mut seen = Vec::new();
        let mut stats: HashMap<String, (String, usize, usize)> = HashMap::new();

        for item in &self.items {
            if matches!(item.provider_id.as_str(), "defaults" | "account-flow") {
                continue;
            }
            if !stats.contains_key(&item.provider_id) {
                seen.push(item.provider_id.clone());
                stats.insert(
                    item.provider_id.clone(),
                    (item.provider_label.clone(), 0, 0),
                );
            }
            if let Some((_, accounts, actions)) = stats.get_mut(&item.provider_id) {
                if matches!(action_section(item), ActionSection::Switch) {
                    *accounts += 1;
                } else {
                    *actions += 1;
                }
            }
        }

        let mut spans = vec![Span::styled("Providers ", Style::default().fg(MUTED_DARK))];
        let mut first = true;
        for provider_id in seen {
            let Some((label, accounts, actions)) = stats.get(&provider_id) else {
                continue;
            };
            if !first {
                spans.push(Span::styled(" | ", Style::default().fg(MUTED_DARK)));
            }
            first = false;
            let summary = if *accounts > 0 {
                format!("{} {}", label, account_count_summary(*accounts))
            } else {
                format!(
                    "{} {} control{}",
                    label,
                    actions,
                    if *actions == 1 { "" } else { "s" }
                )
            };
            spans.push(Span::styled(summary, provider_style(&provider_id)));
        }
        if first {
            spans.push(Span::styled(
                "No providers available",
                Style::default().fg(MUTED),
            ));
        }
        Line::from(spans)
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
            KeyCode::Left => {
                self.select_prev_provider_group();
            }
            KeyCode::Right => {
                self.select_next_provider_group();
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
                hotkey(" Up/Down "),
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
                Constraint::Length(7),
                Constraint::Min(10),
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
            Span::styled("Focus ", Style::default().fg(MUTED_DARK)),
            Span::styled(
                "saved accounts stay surfaced here; use Left/Right to jump provider groups, type to narrow further, or use `/account <provider> settings` for the full text view.",
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
                        "type provider or account name".to_string()
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
                    format!("  -  {} results", self.filtered.len()),
                    Style::default().fg(MUTED_DARK),
                ),
            ]),
            self.provider_overview_line(),
            self.summary_line(),
            self.defaults_line(),
        ];

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn render_action_list(&self, frame: &mut Frame, area: Rect) {
        let title = if self.filtered.is_empty() {
            " Providers & Quick Actions ".to_string()
        } else {
            format!(
                " Providers & Quick Actions ({}/{}) ",
                self.selected + 1,
                self.filtered.len()
            )
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

        let available_items = (list_inner.height as usize).max(1);
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
                "Try `openai`, `claude`, an account label, `login`, or `default`.",
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
                        self.filtered_provider_switch_count(&item.provider_id),
                        self.filtered_provider_secondary_count(&item.provider_id),
                        &item.provider_id,
                    ));
                }

                let row_style = if selected {
                    Style::default().bg(SELECTED_BG)
                } else {
                    Style::default()
                };
                let (icon, icon_color) = action_icon(item);
                let title = compact_item_title(item);
                let meta_width = list_inner.width.saturating_sub(16) as usize;
                let meta = truncate_with_ellipsis(&item.subtitle, meta_width);
                lines.push(Line::from(vec![
                    Span::styled(
                        if selected { "> " } else { "  " },
                        row_style.fg(Color::White),
                    ),
                    Span::styled(format!("{} ", icon), row_style.fg(icon_color).bold()),
                    Span::styled(
                        truncate_with_ellipsis(&title, 22),
                        row_style.fg(Color::White),
                    ),
                    Span::styled(" - ", row_style.fg(MUTED_DARK)),
                    Span::styled(meta, row_style.fg(MUTED)),
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

        let provider_items: Vec<&AccountPickerItem> = self
            .items
            .iter()
            .filter(|candidate| candidate.provider_id == item.provider_id)
            .collect();
        let mut account_items: Vec<&AccountPickerItem> = provider_items
            .iter()
            .copied()
            .filter(|candidate| matches!(action_section(candidate), ActionSection::Switch))
            .collect();
        account_items.sort_by(|left, right| {
            account_is_active(right)
                .cmp(&account_is_active(left))
                .then_with(|| compact_item_title(left).cmp(&compact_item_title(right)))
        });
        let mut secondary_items: Vec<&AccountPickerItem> = provider_items
            .iter()
            .copied()
            .filter(|candidate| !matches!(action_section(candidate), ActionSection::Switch))
            .filter(|candidate| candidate.title != item.title)
            .collect();
        secondary_items.sort_by(|left, right| {
            action_section(left)
                .cmp(&action_section(right))
                .then_with(|| compact_item_title(left).cmp(&compact_item_title(right)))
        });
        secondary_items.truncate(6);
        let (kind_label, kind_color) = action_kind_badge(&item.command);

        let mut lines = vec![
            Line::from(vec![
                Span::styled("Provider ", Style::default().fg(MUTED_DARK)),
                Span::styled(
                    item.provider_label.clone(),
                    provider_style(&item.provider_id),
                ),
            ]),
            Line::from(vec![
                Span::styled("Saved accounts ", Style::default().fg(MUTED_DARK)),
                Span::styled(
                    account_count_summary(account_items.len()),
                    Style::default().fg(Color::White).bold(),
                ),
            ]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Quick switch",
                Style::default().fg(MUTED_DARK).bold(),
            )]),
        ];

        if account_items.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "No saved accounts for this provider yet.",
                Style::default().fg(MUTED),
            )]));
        } else {
            for account in &account_items {
                let is_selected = account.title == item.title;
                let bullet = if account_is_active(account) { "*" } else { "o" };
                let note = if is_selected { "  [selected]" } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} ", bullet),
                        Style::default().fg(if account_is_active(account) {
                            Color::Rgb(110, 214, 158)
                        } else {
                            MUTED_DARK
                        }),
                    ),
                    Span::styled(
                        compact_item_title(account),
                        Style::default().fg(Color::White).bold(),
                    ),
                    Span::styled(
                        note.to_string(),
                        Style::default().fg(Color::Rgb(170, 210, 255)),
                    ),
                ]));
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        "  {}",
                        truncate_with_ellipsis(
                            &account.subtitle,
                            inner.width.saturating_sub(3) as usize,
                        )
                    ),
                    Style::default().fg(MUTED),
                )]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Selected action",
            Style::default().fg(MUTED_DARK).bold(),
        )]));
        lines.push(Line::from(vec![
            Span::styled(kind_label, Style::default().fg(kind_color).bold()),
            Span::styled(" - ", Style::default().fg(MUTED_DARK)),
            Span::styled(item.title.clone(), Style::default().fg(Color::White).bold()),
        ]));
        lines.push(Line::from(vec![Span::styled(
            item.subtitle.clone(),
            Style::default().fg(MUTED),
        )]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Runs",
            Style::default().fg(MUTED_DARK).bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            command_preview(&item.command),
            Style::default().fg(Color::White),
        )]));
        lines.push(Line::from(vec![Span::styled(
            action_kind_help(&item.command),
            Style::default().fg(MUTED),
        )]));

        if !secondary_items.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Other controls",
                Style::default().fg(MUTED_DARK).bold(),
            )]));
            for related in secondary_items {
                lines.push(Line::from(vec![
                    Span::styled("- ", Style::default().fg(MUTED_DARK)),
                    Span::styled(
                        compact_item_title(related),
                        Style::default().fg(Color::White),
                    ),
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
                    "accounts",
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
            Span::styled("  -  model ", Style::default().fg(MUTED_DARK)),
            Span::styled(model.to_string(), Style::default().fg(Color::White)),
        ])
    }
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

    #[test]
    fn test_account_picker_sorts_switches_before_settings() {
        let picker = AccountPicker::new(
            " Accounts ",
            vec![
                AccountPickerItem::action(
                    "openai",
                    "OpenAI",
                    "Provider settings",
                    "configured",
                    AccountPickerCommand::SubmitInput("/account openai settings".to_string()),
                ),
                AccountPickerItem::action(
                    "openai",
                    "OpenAI",
                    "Switch account `work`",
                    "user@example.com - valid - active",
                    AccountPickerCommand::SubmitInput("/account openai switch work".to_string()),
                ),
                AccountPickerItem::action(
                    "defaults",
                    "Global",
                    "Default provider",
                    "Current: auto",
                    AccountPickerCommand::PromptValue {
                        prompt: "provider".to_string(),
                        command_prefix: "/account default-provider".to_string(),
                        empty_value: Some("auto".to_string()),
                        status_notice: "editing".to_string(),
                    },
                ),
            ],
        );

        let ordered_titles: Vec<String> = picker
            .filtered
            .iter()
            .map(|idx| picker.items[*idx].title.clone())
            .collect();

        assert_eq!(ordered_titles[0], "Switch account `work`");
        assert_eq!(ordered_titles[1], "Provider settings");
        assert_eq!(ordered_titles[2], "Default provider");
    }

    #[test]
    fn test_account_picker_left_right_jump_by_provider_group() {
        let mut picker = AccountPicker::new(
            " Accounts ",
            vec![
                AccountPickerItem::action(
                    "claude",
                    "Claude",
                    "Switch account `work`",
                    "a@example.com - valid - active",
                    AccountPickerCommand::SubmitInput("/account claude switch work".to_string()),
                ),
                AccountPickerItem::action(
                    "claude",
                    "Claude",
                    "Provider settings",
                    "configured",
                    AccountPickerCommand::SubmitInput("/account claude settings".to_string()),
                ),
                AccountPickerItem::action(
                    "openai",
                    "OpenAI",
                    "Switch account `default`",
                    "b@example.com - valid - active",
                    AccountPickerCommand::SubmitInput("/account openai switch default".to_string()),
                ),
            ],
        );

        picker.selected = 1;
        let _ = picker.handle_overlay_key(KeyCode::Right, KeyModifiers::empty());
        assert_eq!(
            picker.items[picker.filtered[picker.selected]].provider_id,
            "openai"
        );

        let _ = picker.handle_overlay_key(KeyCode::Left, KeyModifiers::empty());
        assert_eq!(
            picker.items[picker.filtered[picker.selected]].provider_id,
            "claude"
        );
        assert_eq!(picker.selected, 0);
    }
}
