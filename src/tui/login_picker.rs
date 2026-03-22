use crate::auth::AuthState;
use crate::provider_catalog::LoginProviderDescriptor;
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
pub struct LoginPickerItem {
    pub index: usize,
    pub provider: LoginProviderDescriptor,
    pub auth_state: AuthState,
    pub method_detail: String,
}

impl LoginPickerItem {
    pub fn new(
        index: usize,
        provider: LoginProviderDescriptor,
        auth_state: AuthState,
        method_detail: impl Into<String>,
    ) -> Self {
        Self {
            index,
            provider,
            auth_state,
            method_detail: method_detail.into(),
        }
    }

    fn matches_filter(&self, filter: &str) -> bool {
        let trimmed = filter.trim();
        if trimmed.is_empty() {
            return true;
        }

        if trimmed.chars().all(|c| c.is_ascii_digit()) {
            return self.index.to_string().starts_with(trimmed);
        }

        let haystack = format!(
            "{} {} {} {} {} {} {} {} {}",
            self.index,
            self.provider.id,
            self.provider.display_name,
            self.provider.aliases.join(" "),
            self.provider.auth_kind.label(),
            self.provider.menu_detail,
            self.status_label(),
            self.method_detail,
            if self.provider.recommended {
                "recommended"
            } else {
                ""
            }
        )
        .to_lowercase();

        trimmed
            .split_whitespace()
            .all(|needle| haystack.contains(&needle.to_lowercase()))
    }

    fn status_label(&self) -> &'static str {
        match self.auth_state {
            AuthState::Available => "configured",
            AuthState::Expired => "needs attention",
            AuthState::NotConfigured => "not set up",
        }
    }

    fn status_icon(&self) -> &'static str {
        match self.auth_state {
            AuthState::Available => "✓",
            AuthState::Expired | AuthState::NotConfigured => "✕",
        }
    }

    fn status_color(&self) -> Color {
        match self.auth_state {
            AuthState::Available => Color::Rgb(111, 214, 181),
            AuthState::Expired => Color::Rgb(255, 196, 112),
            AuthState::NotConfigured => Color::Rgb(232, 134, 134),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoginPickerSummary {
    pub ready_count: usize,
    pub attention_count: usize,
    pub setup_count: usize,
    pub recommended_count: usize,
}

#[derive(Debug, Clone)]
pub struct LoginPicker {
    title: String,
    items: Vec<LoginPickerItem>,
    filtered: Vec<usize>,
    selected: usize,
    filter: String,
    summary: LoginPickerSummary,
}

pub enum OverlayAction {
    Continue,
    Close,
    Execute(LoginProviderDescriptor),
}

impl LoginPicker {
    pub fn with_summary(
        title: impl Into<String>,
        items: Vec<LoginPickerItem>,
        summary: LoginPickerSummary,
    ) -> Self {
        let mut picker = Self {
            title: title.into(),
            items,
            filtered: Vec::new(),
            selected: 0,
            filter: String::new(),
            summary,
        };
        picker.apply_filter();
        picker
    }

    fn selected_item(&self) -> Option<&LoginPickerItem> {
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
    ) -> anyhow::Result<OverlayAction> {
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
                    return Ok(OverlayAction::Execute(item.provider));
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
                Span::styled(" login  ", Style::default().fg(MUTED_DARK)),
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
                Constraint::Length(3),
                Constraint::Min(12),
                Constraint::Length(2),
            ])
            .split(inner);

        self.render_header(frame, rows[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(37), Constraint::Percentage(63)])
            .split(rows[1]);

        self.render_provider_list(frame, body[0]);
        self.render_detail_pane(frame, body[1]);

        let footer = Paragraph::new(Line::from(vec![
            Span::styled("Tip ", Style::default().fg(MUTED_DARK)),
            Span::styled(
                "Move through providers on the left; the focused provider expands on the right with setup and account details.",
                Style::default().fg(MUTED),
            ),
        ]));
        frame.render_widget(footer, rows[2]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Login overview ",
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
                        "type provider, status, or auth method".to_string()
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
            Line::from(vec![
                metric_span(
                    "configured",
                    self.summary.ready_count,
                    Color::Rgb(111, 214, 181),
                ),
                Span::raw("  "),
                metric_span(
                    "attention",
                    self.summary.attention_count,
                    Color::Rgb(255, 196, 112),
                ),
                Span::raw("  "),
                metric_span("setup", self.summary.setup_count, Color::Rgb(160, 168, 188)),
                Span::raw("  "),
                metric_span(
                    "recommended",
                    self.summary.recommended_count,
                    Color::Rgb(196, 170, 255),
                ),
            ]),
        ];

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn render_provider_list(&self, frame: &mut Frame, area: Rect) {
        let title = if self.filtered.is_empty() {
            " Providers ".to_string()
        } else {
            format!(
                " Providers ({}/{}) ",
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
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let available_items = inner.height.max(1) as usize;
        let start = self
            .selected
            .saturating_sub(available_items.saturating_sub(1).min(available_items / 2));
        let end = (start + available_items).min(self.filtered.len());

        let mut lines = Vec::new();
        if self.filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "No matching providers.",
                Style::default().fg(Color::Gray).italic(),
            )));
            lines.push(Line::from(Span::styled(
                "Try `openai`, `oauth`, `configured`, or `setup`.",
                Style::default().fg(MUTED),
            )));
        } else {
            for visible_idx in start..end {
                let idx = self.filtered[visible_idx];
                let item = &self.items[idx];
                let selected = visible_idx == self.selected;
                let row_style = if selected {
                    Style::default().bg(SELECTED_BG)
                } else {
                    Style::default()
                };

                let row_width = inner.width.saturating_sub(2) as usize;
                let name =
                    truncate_with_ellipsis(item.provider.display_name, row_width.saturating_sub(2));
                let visible_name_len = name.chars().count();
                let padding = row_width.saturating_sub(visible_name_len + 2);

                lines.push(Line::from(vec![
                    Span::styled(
                        if selected { "› " } else { "  " },
                        row_style.fg(Color::White),
                    ),
                    Span::styled(name, row_style.patch(provider_style(item.provider.id))),
                    Span::styled(" ".repeat(padding), row_style),
                    Span::styled(item.status_icon(), row_style.fg(item.status_color()).bold()),
                ]));
            }
        }

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn render_detail_pane(&self, frame: &mut Frame, area: Rect) {
        let title = self
            .selected_item()
            .map(|item| format!(" {} ", item.provider.display_name))
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
                Paragraph::new("No provider selected").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        };

        let aliases = if item.provider.aliases.is_empty() {
            "none".to_string()
        } else {
            item.provider.aliases.join(", ")
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    item.status_icon(),
                    Style::default().fg(item.status_color()).bold(),
                ),
                Span::styled(
                    format!(" {}", item.status_label()),
                    Style::default().fg(item.status_color()).bold(),
                ),
            ]),
            Line::from(vec![
                Span::styled("Provider ", Style::default().fg(MUTED_DARK)),
                Span::styled(
                    item.provider.display_name.to_string(),
                    provider_style(item.provider.id),
                ),
                if item.provider.recommended {
                    Span::styled(
                        "  recommended",
                        Style::default().fg(Color::Rgb(196, 170, 255)),
                    )
                } else {
                    Span::raw("")
                },
            ]),
            Line::from(vec![
                Span::styled("Login command ", Style::default().fg(MUTED_DARK)),
                Span::styled(
                    format!("/login {}", item.provider.id),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![Span::styled(
                "Authentication",
                Style::default().fg(MUTED_DARK).bold(),
            )]),
            Line::from(vec![Span::styled(
                item.provider.auth_kind.label(),
                Style::default()
                    .fg(auth_kind_color(item.provider.auth_kind.label()))
                    .bold(),
            )]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Detected setup",
                Style::default().fg(MUTED_DARK).bold(),
            )]),
            Line::from(vec![Span::styled(
                item.method_detail.clone(),
                Style::default().fg(MUTED),
            )]),
            Line::from(""),
            Line::from(vec![Span::styled(
                "What you need",
                Style::default().fg(MUTED_DARK).bold(),
            )]),
            Line::from(vec![Span::styled(
                item.provider.menu_detail.to_string(),
                Style::default().fg(Color::White),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::styled("Aliases ", Style::default().fg(MUTED_DARK)),
                Span::styled(aliases, Style::default().fg(MUTED)),
            ]),
            Line::from(vec![
                Span::styled("Numbered accounts ", Style::default().fg(MUTED_DARK)),
                Span::styled(
                    if provider_supports_named_accounts(item.provider) {
                        "supported"
                    } else {
                        "not used for this provider"
                    },
                    Style::default().fg(MUTED),
                ),
            ]),
        ];

        let account_lines = account_detail_lines(item.provider);
        if !account_lines.is_empty() {
            lines.push(Line::from(""));
            lines.extend(account_lines);
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Press Enter to begin login.",
            Style::default().fg(Color::Rgb(170, 210, 255)),
        )]));

        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

fn hotkey(text: &'static str) -> Span<'static> {
    Span::styled(text, Style::default().fg(Color::White).bg(Color::DarkGray))
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
        | "antigravity"
        | "jcode" => Color::Rgb(189, 200, 255),
        _ => Color::Rgb(180, 190, 220),
    };
    Style::default().fg(color).bold()
}

fn auth_kind_color(kind: &str) -> Color {
    match kind {
        "OAuth" => Color::Rgb(129, 184, 255),
        "API key" => Color::Rgb(182, 154, 255),
        "device code" => Color::Rgb(111, 214, 181),
        "CLI" => Color::Rgb(131, 215, 255),
        "API key / CLI" => Color::Rgb(229, 187, 111),
        _ => Color::Rgb(180, 190, 220),
    }
}

fn provider_supports_named_accounts(provider: LoginProviderDescriptor) -> bool {
    matches!(
        provider.target,
        crate::provider_catalog::LoginProviderTarget::Claude
            | crate::provider_catalog::LoginProviderTarget::OpenAi
    )
}

fn account_detail_lines(provider: LoginProviderDescriptor) -> Vec<Line<'static>> {
    match provider.target {
        crate::provider_catalog::LoginProviderTarget::Claude => claude_account_lines(),
        crate::provider_catalog::LoginProviderTarget::OpenAi => openai_account_lines(),
        _ => vec![
            Line::from(vec![Span::styled(
                "Accounts",
                Style::default().fg(MUTED_DARK).bold(),
            )]),
            Line::from(vec![Span::styled(
                "This provider is usually configured as a single credential or env-based login.",
                Style::default().fg(MUTED),
            )]),
        ],
    }
}

fn claude_account_lines() -> Vec<Line<'static>> {
    let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
    let active_label = crate::auth::claude::active_account_label();
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut lines = vec![Line::from(vec![Span::styled(
        "Accounts",
        Style::default().fg(MUTED_DARK).bold(),
    )])];

    if accounts.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "No saved Claude accounts yet.",
            Style::default().fg(MUTED),
        )]));
        lines.push(Line::from(vec![
            Span::styled("Add more later with ", Style::default().fg(MUTED_DARK)),
            Span::styled("/account claude add", Style::default().fg(Color::White)),
        ]));
        return lines;
    }

    let active = active_label.unwrap_or_else(crate::auth::claude::primary_account_label);
    lines.push(Line::from(vec![Span::styled(
        format!("{} saved · active: {}", accounts.len(), active),
        Style::default().fg(MUTED),
    )]));

    for account in accounts.iter().take(6) {
        let is_active = active == account.label;
        let account_status = if account.expires > now_ms {
            "valid"
        } else {
            "expired"
        };
        let plan = account
            .subscription_type
            .as_deref()
            .unwrap_or("subscription unknown");
        let email = account
            .email
            .as_deref()
            .map(mask_email)
            .unwrap_or_else(|| "email unknown".to_string());
        lines.push(Line::from(vec![
            Span::styled(
                if is_active { "● " } else { "○ " },
                Style::default().fg(if is_active {
                    Color::Rgb(111, 214, 181)
                } else {
                    MUTED
                }),
            ),
            Span::styled(account.label.clone(), Style::default().fg(Color::White)),
            Span::styled(
                format!(" · {} · {} · {}", email, account_status, plan),
                Style::default().fg(MUTED),
            ),
        ]));
    }

    if accounts.len() > 6 {
        lines.push(Line::from(vec![Span::styled(
            format!("+{} more accounts", accounts.len() - 6),
            Style::default().fg(MUTED_DARK),
        )]));
    }

    lines.push(Line::from(vec![
        Span::styled("Manage with ", Style::default().fg(MUTED_DARK)),
        Span::styled("/account claude", Style::default().fg(Color::White)),
    ]));
    lines
}

fn openai_account_lines() -> Vec<Line<'static>> {
    let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
    let active_label = crate::auth::codex::active_account_label();
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut lines = vec![Line::from(vec![Span::styled(
        "Accounts",
        Style::default().fg(MUTED_DARK).bold(),
    )])];

    if accounts.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "No saved OpenAI accounts yet.",
            Style::default().fg(MUTED),
        )]));
        lines.push(Line::from(vec![
            Span::styled("Add more later with ", Style::default().fg(MUTED_DARK)),
            Span::styled("/account openai add", Style::default().fg(Color::White)),
        ]));
        return lines;
    }

    let active = active_label.unwrap_or_else(crate::auth::codex::primary_account_label);
    lines.push(Line::from(vec![Span::styled(
        format!("{} saved · active: {}", accounts.len(), active),
        Style::default().fg(MUTED),
    )]));

    for account in accounts.iter().take(6) {
        let is_active = active == account.label;
        let account_status = match account.expires_at {
            Some(expires_at) if expires_at > now_ms => "valid",
            Some(_) => "expired",
            None => "valid",
        };
        let email = account
            .email
            .as_deref()
            .map(mask_email)
            .unwrap_or_else(|| "email unknown".to_string());
        let account_id = account
            .account_id
            .as_deref()
            .unwrap_or("account id unknown");
        lines.push(Line::from(vec![
            Span::styled(
                if is_active { "● " } else { "○ " },
                Style::default().fg(if is_active {
                    Color::Rgb(111, 214, 181)
                } else {
                    MUTED
                }),
            ),
            Span::styled(account.label.clone(), Style::default().fg(Color::White)),
            Span::styled(
                format!(" · {} · {} · {}", email, account_status, account_id),
                Style::default().fg(MUTED),
            ),
        ]));
    }

    if accounts.len() > 6 {
        lines.push(Line::from(vec![Span::styled(
            format!("+{} more accounts", accounts.len() - 6),
            Style::default().fg(MUTED_DARK),
        )]));
    }

    lines.push(Line::from(vec![
        Span::styled("Manage with ", Style::default().fg(MUTED_DARK)),
        Span::styled("/account openai", Style::default().fg(Color::White)),
    ]));
    lines
}

fn mask_email(email: &str) -> String {
    let Some((local, domain)) = email.split_once('@') else {
        return email.to_string();
    };

    let masked_local = match local.chars().count() {
        0 => "?".to_string(),
        1..=2 => format!("{}*", local.chars().next().unwrap_or('?')),
        _ => {
            let first = local.chars().next().unwrap_or('?');
            let last = local.chars().last().unwrap_or('?');
            format!("{}***{}", first, last)
        }
    };

    format!("{}@{}", masked_local, domain)
}

fn truncate_with_ellipsis(input: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= width {
        return input.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut out: String = chars.into_iter().take(width - 3).collect();
    out.push_str("...");
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
    fn test_login_picker_preserves_underlying_background_outside_panels() {
        let picker = LoginPicker::with_summary(
            " Login ",
            vec![LoginPickerItem::new(
                1,
                crate::provider_catalog::OPENAI_LOGIN_PROVIDER,
                AuthState::Available,
                "OAuth credentials configured",
            )],
            LoginPickerSummary {
                ready_count: 1,
                ..LoginPickerSummary::default()
            },
        );

        let backend = TestBackend::new(50, 14);
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
            Rect::new(0, 0, 50, 14),
        );
        let probe = &terminal.backend().buffer()[(overlay.x + overlay.width - 3, overlay.y + 2)];
        assert_eq!(probe.symbol(), "X");
        assert_ne!(probe.bg, Color::Rgb(18, 21, 30));
    }
}
