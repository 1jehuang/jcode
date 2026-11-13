//! Rendering for the `ask_user` form.
//!
//! The panel is bottom-anchored so the chat above stays visible. Chrome rows
//! (tabs and status) are always exactly one row and truncate with an ellipsis
//! measured in display cells; the question body scrolls as a whole.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::ViewContext;
use super::state::{EditorKind, Form, Question, State, SubmitState};

pub(super) fn render_view(frame: &mut Frame, state: &mut State, context: &ViewContext, area: Rect) {
    if !context.eligible(state) {
        return;
    }
    let buffer_area = *frame.buffer_mut().area();
    let area = area.intersection(buffer_area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let height = panel_height(area.height);
    if height == 0 {
        return;
    }
    let active = state.active;
    let Some(form) = state.forms.get_mut(active) else {
        return;
    };
    let rect = Rect::new(area.x, area.y + area.height - height, area.width, height);
    frame.render_widget(Clear, rect);
    draw_panel(frame, form, rect);
}

/// At most 60% of the terminal rows. Terminals shorter than 12 rows use every
/// row available so the form can still show focused content.
pub(super) fn panel_height(height: u16) -> u16 {
    if height == 0 {
        0
    } else if height < 12 {
        height
    } else {
        ((u32::from(height) * 3) / 5).max(3).min(u32::from(height)) as u16
    }
}

fn draw_panel(frame: &mut Frame, form: &mut Form, rect: Rect) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let show_tabs = rect.height >= 3;
    let show_status = rect.height >= 2;
    let body_height = rect
        .height
        .saturating_sub(u16::from(show_tabs))
        .saturating_sub(u16::from(show_status));
    let body_y = rect.y + u16::from(show_tabs);

    if show_tabs {
        let line = tab_line(form, rect.width as usize);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(rect.x, rect.y, rect.width, 1),
        );
    }

    if body_height > 0 {
        draw_body(
            frame,
            form,
            Rect::new(rect.x, body_y, rect.width, body_height),
        );
    }

    if show_status {
        let line = status_line(form, rect.width as usize);
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(rect.x, rect.y + rect.height - 1, rect.width, 1),
        );
    }
}

fn draw_body(frame: &mut Frame, form: &mut Form, rect: Rect) {
    let width = rect.width as usize;
    let height = rect.height as usize;
    let narrow = width < 100;
    form.last_body_height = height;

    if narrow && form.preview_focus {
        let preview = build_preview_lines(form, width);
        form.last_preview_lines = preview.len();
        form.last_body_lines = preview.len();
        let max = preview.len().saturating_sub(height);
        form.preview_scroll = form.preview_scroll.min(max);
        draw_preview_pane(
            frame,
            &preview,
            rect.x,
            rect.y,
            rect.width,
            height,
            form.preview_scroll,
        );
        return;
    }

    let (options_width, preview_width) = if narrow {
        (width, 0)
    } else {
        let preview_width = width.saturating_sub(1) / 2;
        (width.saturating_sub(preview_width + 1), preview_width)
    };

    let body = build_body(form, options_width);
    form.last_body_lines = body.lines.len();

    if form.reveal_highlight {
        if let Some(start) = body.highlight_start(form) {
            form.body_scroll = reveal_scroll(form.body_scroll, start, height);
        }
        form.reveal_highlight = false;
    }
    if let Some((row, _)) = body.cursor {
        form.body_scroll = reveal_scroll(form.body_scroll, row, height);
    }
    let max = body.lines.len().saturating_sub(height);
    form.body_scroll = form.body_scroll.min(max);
    let scroll = form.body_scroll;

    for (offset, line) in body
        .lines
        .iter()
        .skip(scroll)
        .take(height)
        .cloned()
        .enumerate()
    {
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(rect.x, rect.y + offset as u16, rect.width, 1),
        );
    }

    if let Some((row, column)) = body.cursor
        && row >= scroll
        && row < scroll + height
        && width > 0
    {
        let x = rect.x + (column.min(width.saturating_sub(1))) as u16;
        let y = rect.y + (row - scroll) as u16;
        frame.set_cursor_position(Position::new(x, y));
    }

    if preview_width > 0 {
        let preview = build_preview_lines(form, preview_width);
        form.last_preview_lines = preview.len();
        let max = preview.len().saturating_sub(height);
        form.preview_scroll = form.preview_scroll.min(max);
        draw_preview_pane(
            frame,
            &preview,
            rect.x + options_width as u16 + 1,
            rect.y,
            preview_width as u16,
            height,
            form.preview_scroll,
        );
    }
}

fn draw_preview_pane(
    frame: &mut Frame,
    lines: &[Line<'static>],
    x: u16,
    y: u16,
    width: u16,
    height: usize,
    scroll: usize,
) {
    if width == 0 || height == 0 {
        return;
    }
    let total = lines.len();
    let clipped = total > height;
    let content_rows = if clipped {
        height.saturating_sub(1)
    } else {
        height
    };
    let mut shown = 0usize;
    for (offset, line) in lines
        .iter()
        .skip(scroll)
        .take(content_rows)
        .cloned()
        .enumerate()
    {
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(x, y + offset as u16, width, 1),
        );
        shown += 1;
    }
    if clipped {
        let remaining = total.saturating_sub(scroll + shown);
        let line = Line::from(Span::styled(
            format!("+{remaining} lines"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(x, y + content_rows as u16, width, 1),
        );
    }
}

fn reveal_scroll(scroll: usize, target: usize, height: usize) -> usize {
    if height == 0 {
        return 0;
    }
    if target < scroll {
        target
    } else if target >= scroll + height {
        target + 1 - height
    } else {
        scroll
    }
}

struct Body {
    lines: Vec<Line<'static>>,
    starts: Vec<usize>,
    cursor: Option<(usize, usize)>,
    summary: bool,
}

impl Body {
    fn highlight_start(&self, form: &Form) -> Option<usize> {
        if self.summary {
            self.starts.get(form.summary_index).copied()
        } else {
            form.current_question()
                .and_then(|question| self.starts.get(question.highlight))
                .copied()
        }
    }
}

fn build_body(form: &Form, width: usize) -> Body {
    if width == 0 {
        return Body {
            lines: Vec::new(),
            starts: Vec::new(),
            cursor: None,
            summary: false,
        };
    }
    if form.is_submit_tab() {
        return build_summary(form, width);
    }
    build_options(form, width)
}

fn build_options(form: &Form, width: usize) -> Body {
    let Some(question) = form.current_question() else {
        return Body {
            lines: Vec::new(),
            starts: Vec::new(),
            cursor: None,
            summary: false,
        };
    };
    let mut lines = Vec::new();
    push_wrapped(
        &mut lines,
        &question.text,
        width,
        Style::default().add_modifier(Modifier::BOLD),
        "",
        "",
    );
    lines.push(Line::default());
    let mut starts = Vec::with_capacity(question.rows());
    let mut cursor = None;
    let editor = form.editor.filter(|editor| editor.question == form.tab);

    for (index, choice) in question.options.iter().enumerate() {
        starts.push(lines.len());
        let highlighted = index == question.highlight;
        let marker = if highlighted { "> " } else { "  " };
        let check = if question.multi {
            if question.selected.contains(&choice.value) {
                "[x] "
            } else {
                "[ ] "
            }
        } else {
            ""
        };
        let style = if highlighted {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let prefix = format!("{marker}{}. {check}", index + 1);
        push_wrapped(
            &mut lines,
            &choice.label,
            width,
            style,
            &prefix,
            &" ".repeat(display_width(&prefix)),
        );
        if let Some(description) = &choice.description {
            push_wrapped(
                &mut lines,
                description,
                width,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
                "    ",
                "    ",
            );
        }
    }

    if let Some(other) = &question.other {
        starts.push(lines.len());
        let row = question.options.len();
        let highlighted = question.highlight == row;
        let marker = if highlighted { "> " } else { "  " };
        let check = if question.multi {
            if question.selected.contains(&other.value) {
                "[x] "
            } else {
                "[ ] "
            }
        } else {
            ""
        };
        let style = if highlighted {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let prefix = format!("{marker}{}. {check}Other: ", row + 1);
        match editor {
            Some(editor) if editor.kind == EditorKind::Other => {
                let available = width.saturating_sub(display_width(&prefix)).max(1);
                let (editor_lines, (row, column)) =
                    layout_editor(&question.other_text, available, question.other_cursor);
                for (index, text) in editor_lines.iter().enumerate() {
                    let lead = if index == 0 {
                        prefix.clone()
                    } else {
                        " ".repeat(display_width(&prefix))
                    };
                    lines.push(Line::from(Span::styled(format!("{lead}{text}"), style)));
                }
                cursor = Some((
                    lines.len() - editor_lines.len() + row,
                    display_width(&prefix) + column,
                ));
            }
            _ => {
                let text = if question.other_text.trim().is_empty() {
                    "(type to answer)"
                } else {
                    question.other_text.as_str()
                };
                push_wrapped(
                    &mut lines,
                    text,
                    width,
                    style,
                    &prefix,
                    &" ".repeat(display_width(&prefix)),
                );
            }
        }
    }

    if !question.notes.trim().is_empty()
        || editor.is_some_and(|editor| editor.kind == EditorKind::Notes)
    {
        lines.push(Line::default());
        let prefix = "Notes: ";
        match editor {
            Some(editor) if editor.kind == EditorKind::Notes => {
                let available = width.saturating_sub(display_width(prefix)).max(1);
                let (editor_lines, (row, column)) =
                    layout_editor(&question.notes, available, question.notes_cursor);
                for (index, text) in editor_lines.iter().enumerate() {
                    let lead = if index == 0 {
                        prefix.to_string()
                    } else {
                        " ".repeat(display_width(prefix))
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{lead}{text}"),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::DIM),
                    )));
                }
                cursor = Some((
                    lines.len() - editor_lines.len() + row,
                    display_width(prefix) + column,
                ));
            }
            _ => push_wrapped(
                &mut lines,
                question.notes.trim(),
                width,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
                prefix,
                "       ",
            ),
        }
    }

    Body {
        lines,
        starts,
        cursor,
        summary: false,
    }
}

fn build_summary(form: &Form, width: usize) -> Body {
    let mut lines = Vec::new();
    let mut starts = Vec::with_capacity(form.question_count() + 1);
    for (index, question) in form.questions.iter().enumerate() {
        starts.push(lines.len());
        let selected = form.summary_index == index;
        let marker = if selected { "> " } else { "  " };
        let check = if question.answered() { "✓" } else { "·" };
        let style = if selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        let prefix = format!("{marker}{check} {}. {}: ", index + 1, question.header);
        push_wrapped(
            &mut lines,
            &question.answer_text(),
            width,
            style,
            &prefix,
            &" ".repeat(display_width(&prefix)),
        );
    }
    starts.push(lines.len());
    let selected = form.summary_index == form.submit_tab();
    let marker = if selected { "> " } else { "  " };
    let suffix = if form.all_answered() {
        ""
    } else {
        " (answer every question)"
    };
    let style = if selected {
        Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD)
    } else if form.all_answered() {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    };
    lines.push(Line::from(Span::styled(
        format!("{marker}[ Submit ]{suffix}"),
        style,
    )));

    Body {
        lines,
        starts,
        cursor: None,
        summary: true,
    }
}

fn build_preview_lines(form: &Form, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Preview",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let preview = form
        .current_question()
        .and_then(Question::preview_for_highlight);
    match preview {
        Some(preview) => push_wrapped(&mut lines, preview, width, Style::default(), "", ""),
        None => lines.push(Line::from(Span::styled(
            "No preview for this option.",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ))),
    }
    lines
}

pub(super) fn tab_line(form: &Form, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let mut segments: Vec<(String, Style)> = Vec::with_capacity(form.question_count() + 1);
    for (index, question) in form.questions.iter().enumerate() {
        let check = if question.answered() { "✓" } else { "" };
        let text = format!("{}.{}{check}", index + 1, question.header);
        segments.push((text, tab_style(form.tab == index)));
        segments.push(("  ".to_string(), Style::default()));
    }
    let submit = if form.all_answered() {
        "Submit✓"
    } else {
        "Submit"
    };
    segments.push((submit.to_string(), tab_style(form.tab == form.submit_tab())));
    // Drop the trailing separator so truncation does not waste cells.
    if segments
        .last()
        .is_some_and(|(text, _)| text.trim().is_empty())
    {
        segments.pop();
    }
    compose_chrome(segments, width)
}

fn tab_style(active: bool) -> Style {
    if active {
        Style::default()
            .add_modifier(Modifier::REVERSED)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

pub(super) fn status_line(form: &Form, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }
    let (left, style) = match &form.submit {
        SubmitState::Submitting => (
            "submitting…".to_string(),
            Style::default().fg(Color::Yellow),
        ),
        SubmitState::Rejected(message) => (
            format!("rejected: {message}"),
            Style::default().fg(Color::Red),
        ),
        SubmitState::Idle => {
            if let Some(notice) = &form.notice {
                (notice.clone(), Style::default().fg(Color::Yellow))
            } else if let Some(editor) = form.editor {
                let label = match editor.kind {
                    EditorKind::Other => "OTHER",
                    EditorKind::Notes => "NOTES",
                };
                (
                    format!("EDIT {label} · Enter save · Esc back"),
                    Style::default(),
                )
            } else if form.preview_focus {
                (
                    "PREVIEW · p options · PgUp/PgDn scroll".to_string(),
                    Style::default(),
                )
            } else {
                (
                    "OPTIONS · ↑↓ move · Space select · Enter next · n notes · p preview · Esc decline"
                        .to_string(),
                    Style::default(),
                )
            }
        }
    };
    let answered = form
        .questions
        .iter()
        .filter(|question| question.answered())
        .count();
    let right = if form.is_submit_tab() {
        format!(
            "{answered}/{} answered · Enter submit",
            form.question_count()
        )
    } else {
        format!("{answered}/{} answered", form.question_count())
    };
    let combined = format!("{left}  {right}");
    Line::from(Span::styled(
        truncate_cells(&combined, width),
        style.add_modifier(Modifier::DIM),
    ))
}

fn compose_chrome(segments: Vec<(String, Style)>, width: usize) -> Line<'static> {
    let total: usize = segments.iter().map(|(text, _)| display_width(text)).sum();
    if total <= width {
        let spans: Vec<Span<'static>> = segments
            .into_iter()
            .map(|(text, style)| Span::styled(text, style))
            .collect();
        return Line::from(spans);
    }
    let budget = width.saturating_sub(1);
    let mut used = 0usize;
    let mut spans = Vec::new();
    for (text, style) in segments {
        if used >= budget {
            break;
        }
        let text_width = display_width(&text);
        if used + text_width <= budget {
            used += text_width;
            spans.push(Span::styled(text, style));
        } else {
            // Reserve the final cell for the single ellipsis appended below.
            let truncated = truncate_raw(&text, budget - used);
            used = budget;
            spans.push(Span::styled(truncated, style));
        }
    }
    spans.push(Span::raw("…"));
    Line::from(spans)
}

fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    first_prefix: &str,
    rest_prefix: &str,
) {
    if width == 0 {
        return;
    }
    let first_width = width.saturating_sub(display_width(first_prefix)).max(1);
    let wrapped = wrap_text(text, first_width);
    for (index, part) in wrapped.iter().enumerate() {
        let prefix = if index == 0 {
            first_prefix
        } else {
            rest_prefix
        };
        lines.push(Line::from(Span::styled(format!("{prefix}{part}"), style)));
    }
}

fn layout_editor(text: &str, width: usize, cursor: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let mut row = 0usize;
    let mut column = 0usize;
    let mut offset = 0usize;
    let mut cursor_position = None;
    for grapheme in text.graphemes(true) {
        if offset == cursor {
            cursor_position = Some((row, column));
        }
        if grapheme == "\n" {
            lines.push(String::new());
            row += 1;
            column = 0;
        } else {
            let grapheme_width = display_width(grapheme);
            if column + grapheme_width > width && column > 0 {
                lines.push(String::new());
                row += 1;
                column = 0;
            }
            if let Some(line) = lines.last_mut() {
                line.push_str(grapheme);
            }
            column += grapheme_width;
        }
        offset += grapheme.len();
    }
    if cursor >= text.len() {
        cursor_position = Some((row, column));
    }
    (lines, cursor_position.unwrap_or((0, 0)))
}

/// Wrap text to `width` display cells, preserving explicit newlines and
/// preferring a break at the last space on the line.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut output = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            output.push(String::new());
            continue;
        }
        if display_width(paragraph) <= width {
            output.push(paragraph.to_string());
            continue;
        }
        let mut current = String::new();
        let mut current_width = 0usize;
        let mut last_space = None;
        for grapheme in paragraph.graphemes(true) {
            let grapheme_width = display_width(grapheme);
            if current_width + grapheme_width > width && !current.is_empty() {
                if let Some(space) = last_space.filter(|space| *space > 0) {
                    let remainder = current[space + 1..].to_string();
                    output.push(current[..space].to_string());
                    current = remainder;
                } else {
                    output.push(std::mem::take(&mut current));
                }
                current_width = display_width(&current);
                last_space = current.rfind(' ');
            }
            if grapheme == " " {
                last_space = Some(current.len());
            }
            current.push_str(grapheme);
            current_width += grapheme_width;
        }
        output.push(current);
    }
    output
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Truncate a single line to `width` display cells, ending with an ellipsis.
fn truncate_cells(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut output = truncate_raw(text, width.saturating_sub(1));
    output.push('…');
    output
}

/// Truncate to at most `width` display cells without adding an ellipsis.
fn truncate_raw(text: &str, width: usize) -> String {
    let mut output = String::new();
    let mut used = 0usize;
    for grapheme in text.graphemes(true) {
        let grapheme_width = display_width(grapheme);
        if used + grapheme_width > width {
            break;
        }
        output.push_str(grapheme);
        used += grapheme_width;
    }
    output
}
