//! TUI modal overlay for the `askUserQuestion` tool.
//!
//! Renders a centered overlay with:
//! - The question (and optional context) at the top
//! - A list of options the user navigates with arrow keys / j-k
//! - A pre-selected recommended option (if any) for quick Enter-to-confirm
//! - A final synthetic "Other (type custom answer)" entry that switches the
//!   modal into a text-input mode where the user types a free-form reply
//! - Esc cancels and submits an `AskUserAnswerKind::Canceled` answer
//!
//! All state needed to fulfil the pending oneshot lives in the modal itself
//! plus the `request_id`. When the modal closes via Enter/Esc the host App
//! calls [`AskUserModal::take_pending_answer`] and submits it to the
//! `crate::ask_user` registry.
//!
//! ## Multi-select
//! When `allow_multiple` is true, Space toggles individual options on/off and
//! Enter submits the accumulated set; otherwise Enter directly submits the
//! single highlighted option (matching the common "pick one" pattern).

use crate::ask_user::{AskUserAnswer, AskUserAnswerKind, AskUserOption, AskUserQuestion};
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

const PANEL_BG: Color = Color::Rgb(24, 28, 40);
const PANEL_BORDER: Color = Color::Rgb(120, 140, 190);
const SECTION_BORDER: Color = Color::Rgb(70, 78, 94);
const SELECTED_BG: Color = Color::Rgb(38, 42, 56);
const SELECTED_BG_RECOMMENDED: Color = Color::Rgb(38, 56, 50);
const RECOMMENDED_FG: Color = Color::Rgb(120, 230, 170);
const MUTED: Color = Color::Rgb(140, 146, 163);
const MUTED_DARK: Color = Color::Rgb(100, 106, 122);
const OPTION_FG: Color = Color::Rgb(220, 225, 240);
const CUSTOM_HINT_FG: Color = Color::Rgb(190, 170, 240);

const OVERLAY_PERCENT_X: u16 = 70;
const OVERLAY_MAX_WIDTH: u16 = 84;
const OVERLAY_MIN_WIDTH: u16 = 44;
const OVERLAY_MIN_HEIGHT: u16 = 14;
const CONTENT_PAD_X: u16 = 2;

/// What the modal wants the host App to do after handling a key.
pub enum AskUserModalOutcome {
    /// Modal stays open; redraw.
    Continue,
    /// Modal should be removed and the contained answer submitted to the
    /// `crate::ask_user` registry.
    Done(AskUserAnswer),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Arrow-key navigation over the option list (and the synthetic Other row).
    Choosing,
    /// Free-form text input for the user's custom answer.
    Typing,
}

pub struct AskUserModal {
    request_id: String,
    title: String,
    question: String,
    context: Option<String>,
    options: Vec<AskUserOption>,
    /// Whether more than one option may be picked simultaneously.
    allow_multiple: bool,
    /// Footer hint shown beneath the option list.
    reply_instructions: Option<String>,
    /// Index of the focused row. Indices `0..options.len()` map to options;
    /// `options.len()` is the synthetic "Other" row.
    cursor: usize,
    /// Picked options when `allow_multiple` is true (set of indices).
    picked: Vec<bool>,
    mode: Mode,
    /// Free-form custom answer buffer when `mode == Typing`.
    typed: String,
}

impl AskUserModal {
    pub fn from_question(question: AskUserQuestion) -> Self {
        let picked = vec![false; question.options.len()];
        // Default the cursor to the first recommended option if any, else 0.
        let recommended_idx = question
            .options
            .iter()
            .position(|opt| opt.recommended)
            .unwrap_or(0);
        Self {
            request_id: question.request_id,
            title: question.title.unwrap_or_else(|| "Question".to_string()),
            question: question.question,
            context: question.context,
            options: question.options,
            allow_multiple: question.allow_multiple,
            reply_instructions: question.reply_instructions,
            cursor: recommended_idx,
            picked,
            mode: Mode::Choosing,
            typed: String::new(),
        }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Index of the synthetic "Other" row.
    fn other_row(&self) -> usize {
        self.options.len()
    }

    /// Total number of navigable rows including "Other".
    fn rows(&self) -> usize {
        self.options.len() + 1
    }

    fn move_cursor(&mut self, delta: isize) {
        let n = self.rows() as isize;
        if n == 0 {
            return;
        }
        let mut next = self.cursor as isize + delta;
        if next < 0 {
            next += n;
        }
        next %= n;
        self.cursor = next as usize;
    }

    fn build_options_answer(&self) -> AskUserAnswerKind {
        let mut ids = Vec::new();
        let mut labels = Vec::new();
        let mut values = Vec::new();

        if self.allow_multiple {
            for (idx, picked) in self.picked.iter().enumerate() {
                if *picked && idx < self.options.len() {
                    let opt = &self.options[idx];
                    ids.push(opt.id.clone());
                    labels.push(opt.label.clone());
                    values.push(opt.value.clone());
                }
            }
            // Fallback: if user pressed Enter without toggling anything, treat
            // the current row as the single selection.
            if ids.is_empty() && self.cursor < self.options.len() {
                let opt = &self.options[self.cursor];
                ids.push(opt.id.clone());
                labels.push(opt.label.clone());
                values.push(opt.value.clone());
            }
        } else if self.cursor < self.options.len() {
            let opt = &self.options[self.cursor];
            ids.push(opt.id.clone());
            labels.push(opt.label.clone());
            values.push(opt.value.clone());
        }

        AskUserAnswerKind::Options {
            ids,
            labels,
            values,
        }
    }

    /// Process a keystroke and report the resulting modal outcome.
    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> AskUserModalOutcome {
        if matches!(self.mode, Mode::Typing) {
            return self.handle_key_typing(code, modifiers);
        }
        self.handle_key_choosing(code, modifiers)
    }

    fn handle_key_choosing(
        &mut self,
        code: KeyCode,
        _modifiers: KeyModifiers,
    ) -> AskUserModalOutcome {
        match code {
            KeyCode::Esc => AskUserModalOutcome::Done(AskUserAnswer {
                request_id: self.request_id.clone(),
                kind: AskUserAnswerKind::Canceled,
            }),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1);
                AskUserModalOutcome::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1);
                AskUserModalOutcome::Continue
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.cursor = 0;
                AskUserModalOutcome::Continue
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.cursor = self.rows().saturating_sub(1);
                AskUserModalOutcome::Continue
            }
            // Space toggles in multi-select mode (and is a no-op otherwise).
            KeyCode::Char(' ') if self.allow_multiple && self.cursor < self.options.len() => {
                let flipped = !self.picked[self.cursor];
                self.picked[self.cursor] = flipped;
                AskUserModalOutcome::Continue
            }
            // Tab also moves down for quick navigation parity with form widgets.
            KeyCode::Tab => {
                self.move_cursor(1);
                AskUserModalOutcome::Continue
            }
            KeyCode::BackTab => {
                self.move_cursor(-1);
                AskUserModalOutcome::Continue
            }
            KeyCode::Enter => {
                if self.cursor == self.other_row() {
                    // Switch to free-form text input.
                    self.mode = Mode::Typing;
                    self.typed.clear();
                    AskUserModalOutcome::Continue
                } else {
                    AskUserModalOutcome::Done(AskUserAnswer {
                        request_id: self.request_id.clone(),
                        kind: self.build_options_answer(),
                    })
                }
            }
            // Quick-select by typing the option id letter when ids are A,B,C,...
            KeyCode::Char(c) if c.is_ascii_alphanumeric() => {
                let needle = c.to_ascii_uppercase().to_string();
                if let Some(idx) = self
                    .options
                    .iter()
                    .position(|opt| opt.id.eq_ignore_ascii_case(&needle))
                {
                    self.cursor = idx;
                    if self.allow_multiple {
                        // Toggle when multi-select; otherwise the user still
                        // needs to press Enter to confirm.
                        self.picked[idx] = !self.picked[idx];
                    }
                }
                AskUserModalOutcome::Continue
            }
            _ => AskUserModalOutcome::Continue,
        }
    }

    fn handle_key_typing(&mut self, code: KeyCode, modifiers: KeyModifiers) -> AskUserModalOutcome {
        match code {
            KeyCode::Esc => {
                // Bail back to choosing without discarding typed text so the
                // user can return and finish if Esc was a slip.
                self.mode = Mode::Choosing;
                self.cursor = self.other_row();
                AskUserModalOutcome::Continue
            }
            KeyCode::Enter => {
                let text = self.typed.trim();
                if text.is_empty() {
                    // Disallow empty submissions: keep modal open.
                    return AskUserModalOutcome::Continue;
                }
                AskUserModalOutcome::Done(AskUserAnswer {
                    request_id: self.request_id.clone(),
                    kind: AskUserAnswerKind::Custom {
                        text: text.to_string(),
                    },
                })
            }
            KeyCode::Backspace => {
                self.typed.pop();
                AskUserModalOutcome::Continue
            }
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                self.typed.push(c);
                AskUserModalOutcome::Continue
            }
            _ => AskUserModalOutcome::Continue,
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area());
        self.render_into(frame, area, true);
    }

    /// Render the modal into a host-supplied rect without centering. The host
    /// is responsible for laying out the area (typically the chat input slot)
    /// so the modal can replace it inline, Claude-Code style.
    pub fn render_inline(&self, frame: &mut Frame, area: Rect) {
        self.render_into(frame, area, false);
    }

    /// Conservative estimate of the rows the modal wants. The host can use
    /// this to reserve an input chunk tall enough to fit question + context +
    /// divider + options (+ descriptions / recommendation reasons) + footer
    /// hint + typing pane.
    pub fn desired_height(&self) -> u16 {
        // We don't know the exact width here; use OVERLAY_MAX_WIDTH minus
        // padding/borders as a reasonable upper bound for wrap math. The host
        // gets a slightly generous answer when terminals are narrow, which is
        // fine since options/typing pane already have minimum heights.
        let content_width = OVERLAY_MAX_WIDTH
            .saturating_sub(2 + CONTENT_PAD_X * 2) // 2 borders + L/R padding
            .max(1) as usize;

        let question_h = wrapped_height(&self.question, content_width).clamp(1, 4);
        let context_h = self
            .context
            .as_deref()
            .map(|s| wrapped_height(s, content_width).min(4))
            .unwrap_or(0);
        // Options list: each option uses 1 row + optional description + optional
        // recommendation reason + 1 blank spacer. Plus the synthetic "Other"
        // row. Plus an optional footer hint with leading blank.
        let mut options_h: usize = 0;
        for opt in &self.options {
            options_h += 1;
            if opt.description.is_some() {
                options_h += 1;
            }
            if opt.recommended && opt.recommendation_reason.is_some() {
                options_h += 1;
            }
            options_h += 1; // visual spacer
        }
        options_h += 1; // Other row
        if self.reply_instructions.is_some() {
            options_h += 2; // blank + hint
        }
        let options_h = options_h.max(3) as u16;
        let typing_h: u16 = if matches!(self.mode, Mode::Typing) { 5 } else { 0 };

        // 2 border rows + 1 top inset pad + 1 divider + 1 blank above options.
        let mut total: u16 = 2 + 1 + question_h + 1 + 1 + options_h + typing_h;
        if context_h > 0 {
            total = total.saturating_add(context_h + 1); // blank + context
        }
        total
    }

    fn render_into(&self, frame: &mut Frame, area: Rect, clear_under: bool) {
        if clear_under {
            // Clear underlying widgets so the modal is fully opaque (only when
            // the modal is drawn as a floating overlay).
            frame.render_widget(Clear, area);
        }

        let title = Line::from(Span::styled(
            format!(" {} ", self.title),
            Style::default().fg(Color::White).bold(),
        ));
        let footer = self.footer_line();
        let outer = Block::default()
            .title(title)
            .title_bottom(footer)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(PANEL_BORDER))
            .style(Style::default().bg(PANEL_BG));
        frame.render_widget(&outer, area);
        let outer_inner = outer.inner(area);

        // Inset content from the border so text doesn't hug the edges.
        let inner = Rect {
            x: outer_inner.x + CONTENT_PAD_X,
            y: outer_inner.y + 1,
            width: outer_inner.width.saturating_sub(CONTENT_PAD_X * 2),
            height: outer_inner.height.saturating_sub(1),
        };

        let content_width = inner.width.max(1) as usize;
        let question_h = wrapped_height(&self.question, content_width).clamp(1, 4);
        let context_h = self
            .context
            .as_deref()
            .map(|s| wrapped_height(s, content_width).min(4))
            .unwrap_or(0);
        let typing_h = if matches!(self.mode, Mode::Typing) {
            5
        } else {
            0
        };

        // Vertical layout:
        //   question
        //   blank (only when context present)
        //   context (only when context present)
        //   divider
        //   blank
        //   options (fills)
        //   typing pane (only when active)
        let mut constraints: Vec<Constraint> = Vec::with_capacity(7);
        constraints.push(Constraint::Length(question_h));
        if context_h > 0 {
            constraints.push(Constraint::Length(1)); // blank
            constraints.push(Constraint::Length(context_h));
        }
        constraints.push(Constraint::Length(1)); // divider
        constraints.push(Constraint::Length(1)); // blank above options
        constraints.push(Constraint::Min(3)); // options list
        if typing_h > 0 {
            constraints.push(Constraint::Length(typing_h));
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        let mut slot = 0usize;

        // Question.
        let question_para = Paragraph::new(self.question.clone())
            .style(Style::default().fg(Color::White).bold())
            .wrap(Wrap { trim: false });
        frame.render_widget(question_para, chunks[slot]);
        slot += 1;

        if context_h > 0 {
            slot += 1; // skip blank
            let context_para =
                Paragraph::new(self.context.as_deref().unwrap_or("").to_string())
                    .style(Style::default().fg(MUTED))
                    .wrap(Wrap { trim: false });
            frame.render_widget(context_para, chunks[slot]);
            slot += 1;
        }

        // Divider that respects the content padding.
        let divider_line = "─".repeat(inner.width as usize);
        let divider = Paragraph::new(divider_line)
            .style(Style::default().fg(SECTION_BORDER));
        frame.render_widget(divider, chunks[slot]);
        slot += 1;
        slot += 1; // blank above options

        let options_area = chunks[slot];
        slot += 1;
        self.render_options(frame, options_area);

        if typing_h > 0 {
            self.render_typing(frame, chunks[slot]);
        }
    }

    fn render_options(&self, frame: &mut Frame, area: Rect) {
        let content_width = area.width as usize;
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(self.rows() * 3);

        for (idx, opt) in self.options.iter().enumerate() {
            lines.push(self.render_option_row(idx, opt, content_width));
            if let Some(desc) = opt.description.as_deref() {
                lines.push(padded_secondary_line(desc, content_width, MUTED, false));
            }
            if opt.recommended {
                if let Some(reason) = opt.recommendation_reason.as_deref() {
                    lines.push(padded_secondary_line(
                        &format!("recommended: {}", reason),
                        content_width,
                        MUTED_DARK,
                        true,
                    ));
                }
            }
            // Visual breathing room between options.
            lines.push(Line::from(""));
        }
        lines.push(self.render_other_row(content_width));

        if let Some(hint) = self.reply_instructions.as_deref() {
            lines.push(Line::from(""));
            lines.push(padded_secondary_line(
                &format!("hint: {}", hint),
                content_width,
                MUTED_DARK,
                true,
            ));
        }

        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }

    fn render_option_row(
        &self,
        idx: usize,
        opt: &AskUserOption,
        content_width: usize,
    ) -> Line<'static> {
        let selected = self.cursor == idx;
        let picked = self.allow_multiple && self.picked.get(idx).copied().unwrap_or(false);

        let arrow = if selected { "▌ " } else { "  " };
        let check = if self.allow_multiple {
            if picked { "[x] " } else { "[ ] " }
        } else {
            ""
        };
        let recommended_tag = if opt.recommended { "  ★" } else { "" };

        let row_bg = if selected {
            if opt.recommended {
                SELECTED_BG_RECOMMENDED
            } else {
                SELECTED_BG
            }
        } else {
            PANEL_BG
        };

        let row_fg = if opt.recommended {
            RECOMMENDED_FG
        } else {
            OPTION_FG
        };

        let mut spans = vec![
            Span::styled(arrow.to_string(), Style::default().fg(row_fg).bg(row_bg)),
            Span::styled(
                check.to_string(),
                Style::default().fg(row_fg).bg(row_bg),
            ),
            Span::styled(
                format!("[{}]  ", opt.id),
                Style::default().fg(row_fg).bg(row_bg).bold(),
            ),
            Span::styled(opt.label.clone(), Style::default().fg(row_fg).bg(row_bg)),
        ];
        if !recommended_tag.is_empty() {
            spans.push(Span::styled(
                recommended_tag.to_string(),
                Style::default().fg(RECOMMENDED_FG).bg(row_bg),
            ));
        }

        // Pad to full content width so the background highlight extends to the
        // right edge of the modal body.
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if used < content_width {
            spans.push(Span::styled(
                " ".repeat(content_width - used),
                Style::default().bg(row_bg),
            ));
        }

        Line::from(spans)
    }

    fn render_other_row(&self, content_width: usize) -> Line<'static> {
        let selected = self.cursor == self.other_row();
        let arrow = if selected { "▌ " } else { "  " };
        let check_pad = if self.allow_multiple { "    " } else { "" };
        let bg = if selected { SELECTED_BG } else { PANEL_BG };

        let mut spans = vec![
            Span::styled(arrow.to_string(), Style::default().fg(CUSTOM_HINT_FG).bg(bg)),
            Span::styled(
                check_pad.to_string(),
                Style::default().fg(CUSTOM_HINT_FG).bg(bg),
            ),
            Span::styled(
                "Other".to_string(),
                Style::default().fg(CUSTOM_HINT_FG).bg(bg).bold(),
            ),
            Span::styled(
                "  type a custom answer".to_string(),
                Style::default().fg(CUSTOM_HINT_FG).bg(bg).italic(),
            ),
        ];
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if used < content_width {
            spans.push(Span::styled(
                " ".repeat(content_width - used),
                Style::default().bg(bg),
            ));
        }
        Line::from(spans)
    }

    fn render_typing(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(Span::styled(
                " Custom answer ",
                Style::default().fg(CUSTOM_HINT_FG).bold(),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(CUSTOM_HINT_FG));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Display typed text plus a blinking-style caret.
        let mut text = self.typed.clone();
        text.push('▏');
        let para =
            Paragraph::new(Line::from(Span::styled(text, Style::default().fg(Color::White))))
                .wrap(Wrap { trim: false });
        frame.render_widget(para, inner);
    }

    fn footer_line(&self) -> Line<'static> {
        if matches!(self.mode, Mode::Typing) {
            Line::from(vec![
                hotkey(" Enter "),
                Span::styled(" submit  ", Style::default().fg(MUTED_DARK)),
                hotkey(" Esc "),
                Span::styled(" back to options ", Style::default().fg(MUTED_DARK)),
            ])
        } else if self.allow_multiple {
            Line::from(vec![
                hotkey(" Up/Down "),
                Span::styled(" navigate  ", Style::default().fg(MUTED_DARK)),
                hotkey(" Space "),
                Span::styled(" toggle  ", Style::default().fg(MUTED_DARK)),
                hotkey(" Enter "),
                Span::styled(" submit  ", Style::default().fg(MUTED_DARK)),
                hotkey(" Esc "),
                Span::styled(" cancel ", Style::default().fg(MUTED_DARK)),
            ])
        } else {
            Line::from(vec![
                hotkey(" Up/Down "),
                Span::styled(" navigate  ", Style::default().fg(MUTED_DARK)),
                hotkey(" Enter "),
                Span::styled(" pick  ", Style::default().fg(MUTED_DARK)),
                hotkey(" Esc "),
                Span::styled(" cancel ", Style::default().fg(MUTED_DARK)),
            ])
        }
    }
}

fn hotkey(label: &str) -> Span<'static> {
    Span::styled(
        label.to_string(),
        Style::default()
            .bg(Color::Rgb(60, 70, 95))
            .fg(Color::White)
            .bold(),
    )
}

/// Number of visual rows `text` will occupy when wrapped to `width` columns.
/// Handles ASCII naively (good enough for the question + context strings the
/// agent is expected to emit; we cap the result in the caller).
fn wrapped_height(text: &str, width: usize) -> u16 {
    if width == 0 {
        return 1;
    }
    let len = text.chars().count();
    if len == 0 {
        return 1;
    }
    let rows = len.div_ceil(width);
    rows.max(1) as u16
}

/// Build a left-indented secondary line with a uniform style and pad it to
/// `content_width` so the modal feels grid-aligned (no ragged right edge).
fn padded_secondary_line(
    text: &str,
    content_width: usize,
    fg: Color,
    italic: bool,
) -> Line<'static> {
    let body = format!("    {}", text);
    let style = if italic {
        Style::default().fg(fg).italic()
    } else {
        Style::default().fg(fg)
    };
    let body_len = body.chars().count();
    let pad = content_width.saturating_sub(body_len);
    Line::from(vec![
        Span::styled(body, style),
        Span::styled(" ".repeat(pad), Style::default()),
    ])
}

fn centered_rect(area: Rect) -> Rect {
    // Width: percent of screen, clamped to [MIN, MAX], and never wider than the
    // available area.
    let width_pct = (area.width as u32 * OVERLAY_PERCENT_X as u32 / 100) as u16;
    let width = width_pct
        .clamp(OVERLAY_MIN_WIDTH, OVERLAY_MAX_WIDTH)
        .min(area.width.saturating_sub(2).max(OVERLAY_MIN_WIDTH));
    // Height: grows with the screen but never less than the minimum and never
    // more than two-thirds of the screen so the chat stays visible behind it.
    let two_thirds = (area.height as u32 * 2 / 3) as u16;
    let height = OVERLAY_MIN_HEIGHT
        .max(two_thirds)
        .min(area.height.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ask_user::AskUserOption;

    fn sample_question() -> AskUserQuestion {
        AskUserQuestion {
            request_id: "req".into(),
            session_id: "ses".into(),
            question: "Pick".into(),
            context: Some("Why".into()),
            options: vec![
                AskUserOption {
                    id: "A".into(),
                    label: "Alpha".into(),
                    description: None,
                    value: None,
                    recommended: false,
                    recommendation_reason: None,
                },
                AskUserOption {
                    id: "B".into(),
                    label: "Beta".into(),
                    description: Some("preferred".into()),
                    value: Some("b-value".into()),
                    recommended: true,
                    recommendation_reason: Some("safer".into()),
                },
            ],
            allow_multiple: false,
            reply_instructions: None,
            title: None,
        }
    }

    #[test]
    fn cursor_starts_on_recommended() {
        let m = AskUserModal::from_question(sample_question());
        assert_eq!(m.cursor, 1);
    }

    #[test]
    fn arrow_keys_wrap() {
        let mut m = AskUserModal::from_question(sample_question());
        // 2 options + 1 other row = 3 rows. Starting at cursor=1 (recommended).
        m.move_cursor(1);
        assert_eq!(m.cursor, 2); // Other
        m.move_cursor(1);
        assert_eq!(m.cursor, 0); // wraps to first option
        m.move_cursor(-1);
        assert_eq!(m.cursor, 2); // wraps backwards to Other
    }

    #[test]
    fn enter_on_option_submits_options_answer() {
        let mut m = AskUserModal::from_question(sample_question());
        m.cursor = 1;
        let out = m.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let AskUserModalOutcome::Done(answer) = out else {
            panic!("expected Done");
        };
        match answer.kind {
            AskUserAnswerKind::Options {
                ids,
                labels,
                values,
            } => {
                assert_eq!(ids, vec!["B"]);
                assert_eq!(labels, vec!["Beta"]);
                assert_eq!(values, vec![Some("b-value".into())]);
            }
            other => panic!("unexpected kind: {other:?}"),
        }
    }

    #[test]
    fn esc_cancels() {
        let mut m = AskUserModal::from_question(sample_question());
        let out = m.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        let AskUserModalOutcome::Done(answer) = out else {
            panic!("expected Done");
        };
        assert!(matches!(answer.kind, AskUserAnswerKind::Canceled));
    }

    #[test]
    fn other_row_switches_to_typing_then_submits_custom() {
        let mut m = AskUserModal::from_question(sample_question());
        // Move to Other row.
        m.cursor = m.other_row();
        let out = m.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(out, AskUserModalOutcome::Continue));
        assert!(matches!(m.mode, Mode::Typing));

        // Type "hi" then Enter.
        m.handle_key(KeyCode::Char('h'), KeyModifiers::NONE);
        m.handle_key(KeyCode::Char('i'), KeyModifiers::NONE);
        let out = m.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let AskUserModalOutcome::Done(answer) = out else {
            panic!("expected Done");
        };
        match answer.kind {
            AskUserAnswerKind::Custom { text } => assert_eq!(text, "hi"),
            other => panic!("unexpected kind: {other:?}"),
        }
    }

    #[test]
    fn empty_custom_does_not_submit() {
        let mut m = AskUserModal::from_question(sample_question());
        m.cursor = m.other_row();
        m.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let out = m.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(out, AskUserModalOutcome::Continue));
    }

    #[test]
    fn typing_esc_returns_to_choosing() {
        let mut m = AskUserModal::from_question(sample_question());
        m.cursor = m.other_row();
        m.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        m.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);
        let out = m.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(out, AskUserModalOutcome::Continue));
        assert!(matches!(m.mode, Mode::Choosing));
        assert_eq!(m.typed, "x"); // preserves text in case user comes back
    }

    #[test]
    fn quick_select_by_id_letter() {
        let mut m = AskUserModal::from_question(sample_question());
        m.cursor = 0;
        m.handle_key(KeyCode::Char('b'), KeyModifiers::NONE);
        assert_eq!(m.cursor, 1);
    }

    #[test]
    fn multi_select_space_toggles() {
        let mut q = sample_question();
        q.allow_multiple = true;
        let mut m = AskUserModal::from_question(q);
        m.cursor = 0;
        m.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(m.picked[0]);
        m.cursor = 1;
        m.handle_key(KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(m.picked[1]);
        let out = m.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let AskUserModalOutcome::Done(answer) = out else {
            panic!("expected Done");
        };
        match answer.kind {
            AskUserAnswerKind::Options { ids, .. } => {
                assert_eq!(ids, vec!["A", "B"]);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
