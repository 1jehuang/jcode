use jcode_tui_markdown::{FrameRow, classify_frame_rows};
use unicode_width::UnicodeWidthStr;

use super::display_width::{
    clamp_display_col, display_col_slice, display_col_slice_copy, line_display_width,
};
use super::url_regex_support::link_target_for_display_column;
use super::{CopyViewportData, CopyViewportSnapshot};

pub(super) fn copy_point_from_snapshot(
    snapshot: &CopyViewportSnapshot,
    column: u16,
    row: u16,
) -> Option<crate::tui::CopySelectionPoint> {
    let area = snapshot.content_area;
    if row < area.y
        || row >= area.y.saturating_add(area.height)
        || column < area.x
        || column >= area.x.saturating_add(area.width)
    {
        return None;
    }

    let rel_row = row.saturating_sub(area.y) as usize;
    let abs_line = snapshot.scroll.saturating_add(rel_row);
    if abs_line >= snapshot.visible_end || abs_line >= snapshot.wrapped_plain_line_count() {
        return None;
    }

    let left_margin = snapshot.left_margins.get(rel_row).copied().unwrap_or(0);
    let content_x = area.x.saturating_add(left_margin);
    let rel_col = column.saturating_sub(content_x) as usize;
    let text = snapshot.wrapped_plain_line(abs_line)?;
    let copy_start = snapshot.wrapped_copy_offset(abs_line).unwrap_or(0);
    Some(crate::tui::CopySelectionPoint {
        pane: snapshot.pane,
        abs_line,
        column: clamp_display_col(text, rel_col).max(copy_start),
    })
}

#[derive(Clone, Copy, Debug)]
struct RawSelectionPoint {
    raw_line: usize,
    column: usize,
}

/// Raw logical-line copy (with math substitution) and nothing else. Tests use
/// it to pin the raw path without the wrapped fallback.
#[cfg(test)]
pub(super) fn copy_selection_text_from_raw_lines(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some(text);
    }
    selection_text(|piece| visit_raw_selection(snapshot, start, end, piece))
}

/// Full copy text for a selection: semantic math substitution first, then the
/// raw logical-line path, then the wrapped display-line fallback (used when a
/// selection starts on chrome such as the session header that has no raw map).
pub(super) fn copy_selection_text_any(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some(text);
    }
    copy_segment_text(snapshot, start, end)
}

/// `(char_count, line_count)` of [`copy_selection_text_any`], computed without
/// allocating the selection string (except around math images, which are rare).
/// Shares the row walkers with the text path, so the status count always
/// matches what is copied.
pub(super) fn copy_selection_metrics_any(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<(usize, usize)> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some(text_metrics(&text));
    }
    selection_metrics(|piece| visit_raw_selection(snapshot, start, end, piece))
        .or_else(|| selection_metrics(|piece| visit_wrapped_selection(snapshot, start, end, piece)))
}

fn copy_segment_text(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    selection_text(|piece| visit_raw_selection(snapshot, start, end, piece))
        .or_else(|| selection_text(|piece| visit_wrapped_selection(snapshot, start, end, piece)))
}

/// A row walker: calls `piece` once per copied line, in order.
type PieceSink<'s> = &'s mut dyn FnMut(&str);

fn selection_text(walk: impl FnOnce(PieceSink<'_>) -> Option<()>) -> Option<String> {
    let mut out = String::new();
    let mut first = true;
    walk(&mut |piece| {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(piece);
    })?;
    Some(out)
}

fn selection_metrics(walk: impl FnOnce(PieceSink<'_>) -> Option<()>) -> Option<(usize, usize)> {
    let mut chars = 0usize;
    let mut lines = 0usize;
    walk(&mut |piece| {
        if lines > 0 {
            chars += 1; // the joining '\n'
        }
        lines += 1;
        chars += piece.chars().count();
    })?;
    Some((chars, lines.max(1)))
}

fn text_metrics(text: &str) -> (usize, usize) {
    (text.chars().count(), text.split('\n').count().max(1))
}

/// Raw logical-line walker. Frame art is excluded per row with the whole pane
/// as context (see [`for_each_copy_row`]), so the selection's own bounds never
/// change how a row is read. Inline math is copied as its LaTeX source.
fn visit_raw_selection(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
    piece: PieceSink<'_>,
) -> Option<()> {
    if snapshot.raw_plain_line_count() == 0 || snapshot.wrapped_line_map(start.abs_line).is_none() {
        return None;
    }

    let start = raw_selection_point(snapshot, start)?;
    let end = raw_selection_point(snapshot, end)?;
    if start.raw_line >= snapshot.raw_plain_line_count()
        || end.raw_line >= snapshot.raw_plain_line_count()
    {
        return None;
    }

    for_each_copy_row(
        snapshot.pane,
        |raw_line| snapshot.raw_plain_line(raw_line),
        |raw_line| snapshot.raw_line_is_literal(raw_line),
        start.raw_line..end.raw_line + 1,
        |raw_line, text, frame| {
            let gutter = match frame {
                FrameRow::Edge => return,
                FrameRow::Content => 0,
                FrameRow::Gutter(width) => width,
            };
            let start_col = if raw_line == start.raw_line {
                clamp_display_col(text, start.column)
            } else {
                0
            }
            .max(gutter);
            let end_col = if raw_line == end.raw_line {
                clamp_display_col(text, end.column)
            } else {
                line_display_width(text)
            };
            if end_col < start_col {
                piece("");
            } else {
                piece(&semantic_slice(text, start_col, end_col));
            }
        },
    )
}

/// Wrapped display-row walker for selections without a raw-line map. Honors
/// per-line copy offsets that skip gutter chrome, and frame art like the raw
/// walker.
fn visit_wrapped_selection(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
    piece: PieceSink<'_>,
) -> Option<()> {
    for_each_copy_row(
        snapshot.pane,
        |abs_line| snapshot.wrapped_plain_line(abs_line),
        |abs_line| {
            snapshot
                .wrapped_line_map(abs_line)
                .is_some_and(|m| snapshot.raw_line_is_literal(m.raw_line))
        },
        start.abs_line..end.abs_line + 1,
        |abs_line, text, frame| {
            let gutter = match frame {
                FrameRow::Edge => return,
                FrameRow::Content => 0,
                FrameRow::Gutter(width) => width,
            };
            let copy_start = snapshot
                .wrapped_copy_offset(abs_line)
                .unwrap_or(0)
                .max(gutter);
            let start_col = if abs_line == start.abs_line {
                clamp_display_col(text, start.column).max(copy_start)
            } else {
                copy_start
            };
            let end_col = if abs_line == end.abs_line {
                clamp_display_col(text, end.column).max(copy_start)
            } else {
                line_display_width(text)
            };
            if end_col < start_col {
                piece("");
            } else {
                piece(&semantic_slice(text, start_col, end_col));
            }
        },
    )
}

/// Visit rows `range` of a pane with the frame art each row carries. Code,
/// math, and quote frames are drawn art, so transcript and side-panel rows are
/// classified. The composer and submitted prompts (`is_literal`) are the user's
/// own text: copied verbatim and blanked for the classifier.
pub(super) fn for_each_copy_row<'a>(
    pane: crate::tui::CopySelectionPane,
    row: impl Fn(usize) -> Option<&'a str>,
    is_literal: impl Fn(usize) -> bool,
    range: std::ops::Range<usize>,
    mut visit: impl FnMut(usize, &'a str, FrameRow),
) -> Option<()> {
    if pane != crate::tui::CopySelectionPane::Input {
        let masked = |idx: usize| {
            let text = row(idx)?;
            Some(if is_literal(idx) { "" } else { text })
        };
        return classify_frame_rows(masked, range, |idx, text, kind| {
            match is_literal(idx).then(|| row(idx)).flatten() {
                Some(literal) => visit(idx, literal, FrameRow::Content),
                None => visit(idx, text, kind),
            }
        });
    }
    for idx in range {
        visit(idx, row(idx)?, FrameRow::Content);
    }
    Some(())
}

/// Slice `text` between display columns, replacing any fully or partially
/// selected inline-math span with its `$source$` LaTeX form.
fn semantic_slice(text: &str, start_col: usize, end_col: usize) -> std::borrow::Cow<'_, str> {
    let Some(spans) = jcode_tui_markdown::inline_math_spans_for_plain_line(text) else {
        return std::borrow::Cow::Borrowed(display_col_slice_copy(text, start_col, end_col));
    };
    let mut out = String::new();
    let mut col = start_col;
    let mut replaced = false;
    for span in spans.iter() {
        if span.end_col <= start_col || span.start_col >= end_col {
            continue;
        }
        if span.start_col > col {
            out.push_str(display_col_slice(text, col, span.start_col));
        }
        out.push('$');
        out.push_str(&span.source);
        out.push('$');
        replaced = true;
        col = span.end_col.max(col);
    }
    if !replaced {
        return std::borrow::Cow::Borrowed(display_col_slice_copy(text, start_col, end_col));
    }
    if col < end_col {
        out.push_str(display_col_slice_copy(text, col, end_col));
    }
    std::borrow::Cow::Owned(out)
}

/// Replace selected terminal-image placeholder rows with their semantic LaTeX
/// source. Normal spans on either side still use the raw logical-line path, so
/// wrapped prose retains the same copy behavior it has without an image.
fn copy_selection_text_with_math_targets(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    let prepared = match &snapshot.data {
        CopyViewportData::ChatFrame { prepared } => prepared,
        CopyViewportData::Dense { .. } => return None,
    };
    let math_targets: Vec<_> = prepared
        .copy_targets
        .iter()
        .filter(|target| {
            matches!(
                &target.kind,
                jcode_tui_markdown::CopyTargetKind::Math { .. }
            )
                // Selection points are half-open display positions. Ending at
                // column zero of the label does not select the image; dragging
                // into any part of its label/body does.
                && (target.start_line, 0) < (end.abs_line, end.column)
                && (start.abs_line, start.column) < (target.end_line, 0)
        })
        .collect();
    if math_targets.is_empty() {
        return None;
    }

    let mut parts = Vec::with_capacity(math_targets.len().saturating_mul(2) + 1);
    let mut cursor = start;
    for target in math_targets {
        if cursor.abs_line < target.start_line {
            let last_line = target.start_line - 1;
            let last_col = snapshot
                .wrapped_plain_line(last_line)
                .map(line_display_width)
                .unwrap_or(0);
            let normal_end = crate::tui::CopySelectionPoint {
                pane: cursor.pane,
                abs_line: last_line,
                column: last_col,
            };
            parts.push(copy_segment_text(snapshot, cursor, normal_end)?);
        }
        parts.push(target.content.clone());
        cursor = crate::tui::CopySelectionPoint {
            pane: cursor.pane,
            abs_line: target.end_line,
            column: 0,
        };
    }

    if (cursor.abs_line, cursor.column) <= (end.abs_line, end.column)
        && cursor.abs_line < snapshot.wrapped_plain_line_count()
    {
        parts.push(copy_segment_text(snapshot, cursor, end)?);
    }
    Some(parts.join("\n"))
}

/// Metrics for [`copy_selection_text_from_raw_lines`], for tests.
#[cfg(test)]
pub(super) fn copy_selection_metrics_from_raw_lines(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<(usize, usize)> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some(text_metrics(&text));
    }
    selection_metrics(|piece| visit_raw_selection(snapshot, start, end, piece))
}

pub(super) fn link_target_from_snapshot(
    snapshot: &CopyViewportSnapshot,
    point: crate::tui::CopySelectionPoint,
) -> Option<String> {
    let raw_point = raw_selection_point(snapshot, point)?;
    let raw_text = snapshot.raw_plain_line(raw_point.raw_line)?;
    link_target_for_display_column(raw_text, raw_point.column)
}

fn raw_selection_point(
    snapshot: &CopyViewportSnapshot,
    point: crate::tui::CopySelectionPoint,
) -> Option<RawSelectionPoint> {
    let wrapped_text = snapshot.wrapped_plain_line(point.abs_line)?;
    let map = snapshot.wrapped_line_map(point.abs_line)?;
    let display_copy_start = snapshot
        .wrapped_copy_offset(point.abs_line)
        .unwrap_or(0)
        .min(wrapped_text.width());
    let local_col = clamp_display_col(wrapped_text, point.column).max(display_copy_start);
    let segment_width = map.end_col.saturating_sub(map.start_col);
    Some(RawSelectionPoint {
        raw_line: map.raw_line,
        column: map.start_col
            + local_col
                .saturating_sub(display_copy_start)
                .min(segment_width),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_tui_markdown::CopyTargetKind;
    use jcode_tui_messages::{CopyTarget, PreparedChatFrame, PreparedMessages, WrappedLineMap};
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use std::sync::Arc;

    fn math_snapshot() -> CopyViewportSnapshot {
        let plain = vec![
            "before".to_string(),
            "  math".to_string(),
            "\0IIMG:000000001a7ec0de:0002:0014\0".to_string(),
            String::new(),
            "after".to_string(),
        ];
        let wrapped_lines = plain.iter().cloned().map(Line::from).collect();
        let maps = plain
            .iter()
            .enumerate()
            .map(|(raw_line, text)| WrappedLineMap {
                raw_line,
                start_col: 0,
                end_col: line_display_width(text),
            })
            .collect();
        let prepared = PreparedMessages {
            wrapped_lines,
            wrapped_plain_lines: Arc::new(plain.clone()),
            wrapped_copy_offsets: Arc::new(vec![0; plain.len()]),
            raw_plain_lines: Arc::new(plain.clone()),
            wrapped_line_map: Arc::new(maps),
            literal_raw_lines: Vec::new(),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: vec![CopyTarget {
                kind: CopyTargetKind::Math { display: true },
                content: "$$\nx^2 + \\alpha\n$$".to_string(),
                start_line: 1,
                end_line: 4,
                badge_line: 1,
            }],
            message_boundaries: Vec::new(),
            mermaid_pending_epoch: None,
        };
        CopyViewportSnapshot {
            pane: crate::tui::CopySelectionPane::Chat,
            data: CopyViewportData::ChatFrame {
                prepared: Arc::new(PreparedChatFrame::from_single(Arc::new(prepared))),
            },
            scroll: 0,
            visible_end: plain.len(),
            content_area: Rect::new(0, 0, 80, plain.len() as u16),
            left_margins: vec![0; plain.len()],
        }
    }

    fn point(line: usize, column: usize) -> crate::tui::CopySelectionPoint {
        crate::tui::CopySelectionPoint {
            pane: crate::tui::CopySelectionPane::Chat,
            abs_line: line,
            column,
        }
    }

    #[test]
    fn selection_replaces_latex_image_rows_with_source_and_preserves_neighbors() {
        let snapshot = math_snapshot();
        let copied = copy_selection_text_from_raw_lines(&snapshot, point(0, 0), point(4, 5))
            .expect("selection should resolve");
        assert_eq!(copied, "before\n$$\nx^2 + \\alpha\n$$\nafter");
        assert!(!copied.contains("IIMG"));
        assert_eq!(
            copy_selection_metrics_from_raw_lines(&snapshot, point(0, 0), point(4, 5)),
            Some((copied.chars().count(), copied.split('\n').count()))
        );
    }

    #[test]
    fn selection_over_unicode_inline_math_copies_latex_source() {
        let rendered =
            jcode_tui_markdown::render_markdown(r"Identity $e^{i\pi_{7}} + 1 = 0$ holds");
        let line = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .find(|text| text.starts_with("Identity"))
            .expect("prose line");
        let width = line_display_width(&line);
        assert_eq!(
            semantic_slice(&line, 0, width),
            r"Identity $e^{i\pi_{7}} + 1 = 0$ holds"
        );
        // A partial drag that only touches the formula still copies all of it.
        assert_eq!(semantic_slice(&line, 10, 12), r"$e^{i\pi_{7}} + 1 = 0$");
        assert_eq!(semantic_slice(&line, 0, 8), "Identity");
    }

    #[test]
    fn selection_inside_latex_image_copies_complete_source() {
        let snapshot = math_snapshot();
        assert_eq!(
            copy_selection_text_from_raw_lines(&snapshot, point(1, 0), point(3, 0)),
            Some("$$\nx^2 + \\alpha\n$$".to_string())
        );
    }
}
