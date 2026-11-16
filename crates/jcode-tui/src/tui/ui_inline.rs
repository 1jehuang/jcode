use super::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn inline_view_display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Height cap for the decision chooser: every option row plus the answer
/// rows and hint must stay inside this even when the draft is long. Beyond
/// the cap the draw pass scrolls a window instead, keeping the selected
/// row (or the whole selected draft) visible.
const CHOOSER_MAX_HEIGHT: u16 = 20;
/// Columns the first answer row loses to the marker, number, and
/// "Your answer: " label; the draft wraps into the space left after it.
/// Computed from the real head string so the reserved height matches the
/// rendered rows regardless of how unicode-width classifies the marker.
fn answer_head_width(option_count: usize) -> usize {
    inline_view_display_width(format!("❯ {}. Your answer: ", option_count + 1).as_str())
}

/// Wrap budget for the answer draft inside a chooser of `inner_width`
/// content columns: everything left after the first row's head.
fn answer_wrap_width(inner_width: usize, option_count: usize) -> usize {
    inner_width
        .saturating_sub(answer_head_width(option_count))
        .max(1)
}
/// Continuation rows indent under the draft text, like the composer's
/// wrapped lines.
const ANSWER_CONTINUATION_INDENT: &str = "     ";
/// Hint while an option row is selected: the answer row still accepts typed
/// free-form text, so the user needs to know it exists without moving to
/// it. Also the widest fixed chooser row, so it sets the minimum width.
const DECISION_HINT_OPTIONS_ROW: &str =
    "1-9 or ↑↓ + Enter to choose · type on Your answer · Esc dismisses";
/// Hint while the answer row is selected: number picks no longer apply,
/// and the newline key matters while typing an answer.
const DECISION_HINT_ANSWER_ROW: &str = "Enter sends · Shift/Alt+Enter newline · Esc dismisses";

/// Chooser content width for a given terminal width: the widest of title,
/// option rows, and hint, clamped to the available inner columns. Shared
/// by the height pass and the draw pass so the reserved rows match what
/// actually renders.
pub(super) fn decision_content_width(
    decision: &crate::tui::app::PendingDecision,
    width: u16,
) -> usize {
    let available = width.saturating_sub(2) as usize; // border columns
    let title_line = format!("❓ {}", decision.question);
    let mut content_width = inline_view_display_width(title_line.as_str());
    for (label, detail) in &decision.options {
        let row_width = inline_view_display_width(label)
            + detail
                .as_ref()
                .map(|d| inline_view_display_width(d) + 2)
                .unwrap_or(0)
            + 6;
        content_width = content_width.max(row_width);
    }
    content_width
        .max(inline_view_display_width(DECISION_HINT_OPTIONS_ROW))
        .min(available)
        .max(1)
}

/// Test-only wrappers: the helpers are private, but coverage tests verify
/// every truncation and height branch directly.
#[cfg(test)]
pub(super) fn decision_truncate_display_for_test(text: &str, max_width: usize) -> String {
    decision_truncate_display(text, max_width)
}

#[cfg(test)]
pub(super) fn inline_ui_height_for_test(app: &dyn TuiState, width: u16) -> u16 {
    inline_ui_height(app, width)
}

fn decision_truncate_display(text: &str, max_width: usize) -> String {
    if inline_view_display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    out
}

/// Wrap the answer draft across rows of at most `width` display columns,
/// splitting explicit newlines into their own rows. Like the composer's
/// wrap: whitespace breaks first, long words hard-split, nothing hidden
/// behind an ellipsis. A zero width (ultra-narrow terminal) degrades to one
/// row per newline segment; the row-level truncation guard then ellipsizes
/// as before.
pub(super) fn wrap_decision_answer_draft(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(String::from).collect()
        };
    }
    let mut rows = Vec::new();
    for paragraph in text.split('\n') {
        wrap_paragraph_into_rows(paragraph, width, &mut rows);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

fn wrap_paragraph_into_rows(paragraph: &str, width: usize, rows: &mut Vec<String>) {
    if inline_view_display_width(paragraph) <= width {
        rows.push(paragraph.to_string());
        return;
    }
    let mut row = String::new();
    let mut row_width = 0usize;
    for word in paragraph.split(' ') {
        let word_width = inline_view_display_width(word);
        if row_width > 0 && row_width + 1 + word_width <= width {
            row.push(' ');
            row.push_str(word);
            row_width += 1 + word_width;
            continue;
        }
        if row_width > 0 {
            rows.push(std::mem::take(&mut row));
        }
        // Fresh row. A word wider than the budget hard-splits at the
        // exact column limit, like the composer. Splitting walks grapheme
        // clusters once and measures each with the same width function as
        // the row budget: joined emoji (ZWJ sequences) measure narrower as
        // clusters than as raw chars, so per-char bookkeeping used to stop
        // early and leave an oversized final row. One forward pass keeps
        // huge pastes linear here in both the height pass and the draw pass.
        let mut row_tail = String::new();
        let mut row_tail_width = 0usize;
        for cluster in word.graphemes(true) {
            let cluster_width = inline_view_display_width(cluster);
            if row_tail_width + cluster_width > width && !row_tail.is_empty() {
                rows.push(std::mem::take(&mut row_tail));
                row_tail_width = 0;
            }
            row_tail.push_str(cluster);
            row_tail_width += cluster_width;
        }
        row = row_tail;
        row_width = row_tail_width;
    }
    if !row.is_empty() {
        rows.push(row);
    }
}

pub(super) fn inline_ui_height(app: &dyn TuiState, width: u16) -> u16 {
    match app.inline_ui_state() {
        Some(crate::tui::InlineUiStateRef::Interactive(picker)) => {
            // Model choices replace the command suggestions, without reflowing chat.
            if picker.kind == crate::tui::PickerKind::Model {
                return 0;
            }
            let visible_rows = picker.filtered.len() as u16;
            let rows_needed = visible_rows + 1 + 2; // header + rounded border
            rows_needed.min(20)
        }
        Some(crate::tui::InlineUiStateRef::View(view)) => {
            let visible_rows = view.lines.len().max(1) as u16;
            let rows_needed = visible_rows + 1 + 2; // header + rounded border
            rows_needed.min(10)
        }
        Some(crate::tui::InlineUiStateRef::Decision(decision)) => {
            // question header + option rows (+ one row per option detail) +
            // the answer rows + hint row + rounded border. All options stay
            // visible until the cap; past it (or in a very short terminal)
            // the draw pass scrolls to keep the selected row on screen.
            let detail_rows = decision
                .options
                .iter()
                .filter(|(_, detail)| detail.is_some())
                .count();
            // A selected draft expands to one row per wrapped line, so long
            // answers stay fully visible; an unselected draft is one row, and
            // an empty selected draft is the head row plus the placeholder
            // hint. The wrap width matches the draw pass exactly, so the
            // reserved height equals the rendered height.
            let answer_rows: usize = if decision.selected_is_answer_row() {
                if decision.answer_draft.is_empty() {
                    2
                } else {
                    let wrap_width = answer_wrap_width(
                        decision_content_width(decision, width),
                        decision.options.len(),
                    );
                    wrap_decision_answer_draft(&decision.answer_draft, wrap_width).len()
                }
            } else {
                1
            };
            // usize end to end, clamped before the u16 cast: a huge pasted
            // answer (tens of thousands of wrapped rows) must saturate at
            // the cap, not wrap around a u16 and shrink the chooser to a
            // sliver of rows.
            let visible_rows = decision.options.len() + detail_rows + answer_rows + 1;
            let rows_needed = visible_rows + 1 + 2;
            rows_needed.min(CHOOSER_MAX_HEIGHT as usize) as u16
        }
        None => 0,
    }
}

pub(super) fn draw_inline_ui(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    match app.inline_ui_state() {
        Some(crate::tui::InlineUiStateRef::Interactive(picker))
            if picker.kind != crate::tui::PickerKind::Model =>
        {
            super::inline_interactive_ui::draw_inline_interactive(frame, app, area)
        }
        Some(crate::tui::InlineUiStateRef::View(view)) => draw_inline_view(frame, app, view, area),
        Some(crate::tui::InlineUiStateRef::Decision(decision)) => {
            draw_inline_decision(frame, app, decision, area)
        }
        _ => {}
    }
}

/// Render the blocking ask_user decision chooser: question header, numbered
/// options (selected row highlighted, detail on its own dim line), a free-form
/// "Your answer" entry row, and a key-hint footer. The main input box is
/// disabled while the chooser is visible; all typing goes to the answer row.
fn draw_inline_decision(
    frame: &mut Frame,
    app: &dyn TuiState,
    decision: &crate::tui::app::PendingDecision,
    area: Rect,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    if height <= 2 || width <= 2 {
        return;
    }

    let title_line = format!("❓ {}", decision.question);
    let content_width = decision_content_width(decision, area.width);
    let outer_width = content_width.saturating_add(2).min(width);
    let horizontal_offset = if app.centered_mode() {
        area.width.saturating_sub(outer_width as u16) / 2
    } else {
        0
    };
    let render_area = Rect {
        x: area.x + horizontal_offset,
        y: area.y,
        width: outer_width as u16,
        height: area.height,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(170, 140, 60)))
        .style(Style::default().bg(rgb(18, 18, 26)))
        .title(Line::from(Span::styled(
            decision_truncate_display(title_line.as_str(), content_width),
            Style::default().fg(rgb(230, 190, 90)).bold(),
        )));
    frame.render_widget(block.clone(), render_area);

    let inner = block.inner(render_area);
    let mut lines: Vec<Line> = Vec::new();
    // Line range of the selected row's block (the option row plus its
    // detail, or the whole answer draft). The visible window keeps this
    // range on screen even when the chooser outgrows the reserved height.
    let mut selected_block_start: usize = 0;
    let mut selected_block_end: usize = 0;
    for (index, (label, detail)) in decision.options.iter().enumerate() {
        let is_selected = index == decision.selected;
        if is_selected {
            selected_block_start = lines.len();
        }
        let marker = if is_selected { "❯" } else { " " };
        let number = index + 1;
        let row = format!("{} {}. {}", marker, number, label);
        let style = if is_selected {
            Style::default().fg(accent_color()).bold()
        } else {
            Style::default().fg(dim_color())
        };
        lines.push(Line::from(Span::styled(
            decision_truncate_display(row.as_str(), inner.width as usize),
            style,
        )));
        if let Some(detail) = detail {
            let detail_row = format!("     {}", detail);
            lines.push(Line::from(Span::styled(
                decision_truncate_display(detail_row.as_str(), inner.width as usize),
                Style::default().fg(dim_color()),
            )));
        }
        if is_selected {
            selected_block_end = lines.len();
        }
    }

    // Free-form "Your answer" rows: an extra option, like ask_user's own
    // free-form escape hatch, with its own inline edit buffer. Typing lands
    // here (the main input box is disabled while the chooser is visible).
    // A selected draft wraps instead of truncating, so long answers stay
    // fully visible; explicit newlines get their own rows, continuation
    // rows indent under the draft text like the composer's wrapped lines.
    let answer_selected = decision.selected_is_answer_row();
    let number = decision.options.len() + 1;
    let answer_style = if answer_selected {
        Style::default().fg(accent_color()).bold()
    } else {
        Style::default().fg(dim_color())
    };
    if answer_selected {
        selected_block_start = lines.len();
        let wrap_width = answer_wrap_width(inner.width as usize, decision.options.len());
        let wrapped = wrap_decision_answer_draft(&decision.answer_draft, wrap_width);
        let head = format!("❯ {}. Your answer: ", number);
        lines.push(Line::from(vec![
            Span::styled(head, answer_style),
            Span::styled(wrapped[0].clone(), answer_style),
        ]));
        for chunk in &wrapped[1..] {
            lines.push(Line::from(vec![
                Span::styled(ANSWER_CONTINUATION_INDENT, answer_style),
                Span::styled(chunk.clone(), answer_style),
            ]));
        }
        if decision.answer_draft.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(
                    "{}Type your own answer, then Enter",
                    ANSWER_CONTINUATION_INDENT
                ),
                Style::default().fg(dim_color()),
            )));
        }
        selected_block_end = lines.len();
    } else {
        lines.push(Line::from(Span::styled(
            format!(" {}. Your answer", number),
            answer_style,
        )));
    }

    // The hint changes with the selected row: number picks and arrow
    // navigation do nothing useful while typing a free-form answer, where
    // the newline key matters instead.
    let hint = if answer_selected {
        DECISION_HINT_ANSWER_ROW
    } else {
        DECISION_HINT_OPTIONS_ROW
    };
    let hint_line = Line::from(Span::styled(
        hint,
        Style::default().fg(rgb(120, 120, 150)).italic(),
    ));
    lines.push(hint_line);
    // Keep the selected row (and its whole draft, when the answer row is
    // selected) visible: when the chooser outgrows the reserved height, show
    // a scrolled window instead of letting the selected row fall below the
    // fold. Bottom-align on the hint when the selected block fits in the
    // same window; otherwise bottom-align on the block itself, keeping the
    // row the user is on visible at the cost of the hint.
    let capacity = inner.height as usize;
    let hint_row = lines.len() - 1;
    let visible: Vec<Line> = if lines.len() > capacity {
        let start = if selected_block_start + capacity > hint_row {
            hint_row + 1 - capacity
        } else {
            selected_block_end.saturating_sub(capacity)
        };
        lines.into_iter().skip(start).take(capacity).collect()
    } else {
        lines
    };
    frame.render_widget(Paragraph::new(visible), inner);
}

fn draw_inline_view(
    frame: &mut Frame,
    app: &dyn TuiState,
    view: &crate::tui::InlineViewState,
    area: Rect,
) {
    let height = area.height as usize;
    let width = area.width as usize;
    if height <= 2 || width <= 2 {
        return;
    }

    let mut content_width = inline_view_display_width(view.title.as_str());
    if let Some(status) = view.status.as_ref() {
        content_width = content_width.max(inline_view_display_width(status.as_str()) + 2);
    }
    for line in &view.lines {
        content_width = content_width.max(inline_view_display_width(line.as_str()));
    }
    let content_width = content_width.min(width.saturating_sub(2)).max(1);
    let outer_width = content_width.saturating_add(2).min(width);
    let horizontal_offset = if app.centered_mode() {
        area.width.saturating_sub(outer_width as u16) / 2
    } else {
        0
    };
    let render_area = Rect {
        x: area.x + horizontal_offset,
        y: area.y,
        width: outer_width as u16,
        height: area.height,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(85, 85, 110)))
        .style(Style::default().bg(rgb(18, 18, 26)));
    frame.render_widget(block.clone(), render_area);

    let inner = block.inner(render_area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut header_spans = vec![Span::styled(
        view.title.clone(),
        Style::default().fg(Color::White).bold(),
    )];
    if let Some(status) = view.status.as_ref() {
        header_spans.push(Span::styled(
            format!("  {}", status),
            Style::default().fg(dim_color()).italic(),
        ));
    }
    lines.push(Line::from(header_spans));

    for line in &view.lines {
        lines.push(Line::from(Span::styled(
            line.clone(),
            Style::default().fg(rgb(200, 200, 220)),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
