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

const OVERLAY_PERCENT_X: u16 = 78;
const OVERLAY_MIN_HEIGHT: u16 = 14;

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
        let area = centered_rect(OVERLAY_PERCENT_X, frame.area());

        // Clear underlying widgets so the modal is fully opaque.
        frame.render_widget(Clear, area);

        let title = Line::from(vec![
            Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(Color::White).bold(),
            ),
        ]);
        let footer = self.footer_line();
        let outer = Block::default()
            .title(title)
            .title_bottom(footer)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(PANEL_BORDER))
            .style(Style::default().bg(PANEL_BG));
        frame.render_widget(&outer, area);
        let inner = outer.inner(area);

        let context_h = self
            .context
            .as_deref()
            .map(|s| ((s.len() as u16 / inner.width.max(1)) + 1).min(4))
            .unwrap_or(0);
        let question_h = ((self.question.len() as u16 / inner.width.max(1)) + 1).min(4);

        // Top-down layout: question, optional context divider, options list,
        // bottom typing pane (only when active).
        let typing_h = if matches!(self.mode, Mode::Typing) {
            5
        } else {
            0
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(question_h.max(1)),
                Constraint::Length(context_h),
                Constraint::Length(1), // divider
                Constraint::Min(3),
                Constraint::Length(typing_h),
            ])
            .split(inner);

        // Question text.
        let question_p = Paragraph::new(Line::from(Span::styled(
            self.question.clone(),
            Style::default().fg(Color::White).bold(),
        )))
        .wrap(Wrap { trim: false });
        frame.render_widget(question_p, chunks[0]);

        // Optional context.
        if let Some(ctx) = &self.context {
            let context_p = Paragraph::new(Line::from(Span::styled(
                ctx.clone(),
                Style::default().fg(MUTED),
            )))
            .wrap(Wrap { trim: false });
            frame.render_widget(context_p, chunks[1]);
        }

        // Divider.
        let div = Paragraph::new(Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            Style::default().fg(SECTION_BORDER),
        )));
        frame.render_widget(div, chunks[2]);

        self.render_options(frame, chunks[3]);

        if matches!(self.mode, Mode::Typing) {
            self.render_typing(frame, chunks[4]);
        }
    }

    fn render_options(&self, frame: &mut Frame, area: Rect) {
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(self.rows());
        for (idx, opt) in self.options.iter().enumerate() {
            lines.push(self.render_option_row(idx, opt));
            if let Some(desc) = opt.description.as_deref() {
                lines.push(Line::from(Span::styled(
                    format!("    {}", desc),
                    Style::default().fg(MUTED),
                )));
            }
            if opt.recommended {
                if let Some(reason) = opt.recommendation_reason.as_deref() {
                    lines.push(Line::from(Span::styled(
                        format!("    why: {}", reason),
                        Style::default().fg(MUTED_DARK).italic(),
                    )));
                }
            }
        }
        lines.push(self.render_other_row());

        // Optional reply hint just below the rows.
        if let Some(hint) = self.reply_instructions.as_deref() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("hint: {}", hint),
                Style::default().fg(MUTED_DARK).italic(),
            )));
        }

        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }

    fn render_option_row(&self, idx: usize, opt: &AskUserOption) -> Line<'static> {
        let selected = self.cursor == idx;
        let picked = self.allow_multiple && self.picked.get(idx).copied().unwrap_or(false);

        let arrow = if selected { "❯ " } else { "  " };
        let check = if self.allow_multiple {
            if picked { "[x] " } else { "[ ] " }
        } else {
            ""
        };
        let id_span = format!("{}.", opt.id);
        let recommended_tag = if opt.recommended { " (recommended)" } else { "" };

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

        Line::from(vec![
            Span::styled(
                format!("{arrow}{check}"),
                Style::default().fg(row_fg).bg(row_bg),
            ),
            Span::styled(
                format!("{id_span} "),
                Style::default().fg(row_fg).bg(row_bg).bold(),
            ),
            Span::styled(opt.label.clone(), Style::default().fg(row_fg).bg(row_bg)),
            Span::styled(
                recommended_tag,
                Style::default().fg(RECOMMENDED_FG).bg(row_bg).italic(),
            ),
        ])
    }

    fn render_other_row(&self) -> Line<'static> {
        let selected = self.cursor == self.other_row();
        let arrow = if selected { "❯ " } else { "  " };
        let check = if self.allow_multiple { "    " } else { "" };
        let bg = if selected { SELECTED_BG } else { PANEL_BG };

        Line::from(vec![
            Span::styled(
                format!("{arrow}{check}"),
                Style::default().fg(CUSTOM_HINT_FG).bg(bg),
            ),
            Span::styled(
                "Other (type custom answer)",
                Style::default().fg(CUSTOM_HINT_FG).bg(bg).italic(),
            ),
        ])
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

fn centered_rect(percent_x: u16, area: Rect) -> Rect {
    let width = (area.width as u32 * percent_x as u32 / 100) as u16;
    let width = width.clamp(40, area.width.saturating_sub(2).max(40));
    // Modal height grows with content but bounded to half the screen.
    let height = OVERLAY_MIN_HEIGHT
        .max(area.height / 2)
        .min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
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
