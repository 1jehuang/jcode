use unicode_width::UnicodeWidthStr;

use super::display_width::{clamp_display_col, display_col_slice, line_display_width};
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

pub(super) fn copy_selection_text_from_raw_lines(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some(text);
    }
    copy_selection_text_from_raw_lines_base(snapshot, start, end)
}

fn copy_selection_text_from_raw_lines_base(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<String> {
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

    let selected_lines = end
        .raw_line
        .saturating_sub(start.raw_line)
        .saturating_add(1);
    let mut out = String::new();
    for raw_line in start.raw_line..=end.raw_line {
        if raw_line > start.raw_line {
            out.push('\n');
        }
        let text = snapshot.raw_plain_line(raw_line)?;
        if raw_line != start.raw_line && raw_line != end.raw_line {
            if raw_line == start.raw_line + 1 {
                out.reserve(text.len().saturating_mul(selected_lines.min(8)));
            }
            out.push_str(text);
            continue;
        }
        let line_width = line_display_width(text);
        let start_col = if raw_line == start.raw_line {
            clamp_display_col(text, start.column)
        } else {
            0
        };
        let end_col = if raw_line == end.raw_line {
            clamp_display_col(text, end.column)
        } else {
            line_width
        };

        if end_col < start_col {
            continue;
        }

        let slice = display_col_slice(text, start_col, end_col);
        if raw_line == start.raw_line {
            out.reserve(slice.len().saturating_mul(selected_lines.min(8)));
        }
        out.push_str(slice);
    }

    Some(out)
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
            parts.push(copy_selection_text_from_raw_lines_base(
                snapshot, cursor, normal_end,
            )?);
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
        parts.push(copy_selection_text_from_raw_lines_base(
            snapshot, cursor, end,
        )?);
    }
    Some(parts.join("\n"))
}

/// Selection metrics (character count and line count) for the raw-lines path,
/// computed without allocating the full joined selection string. Mirrors the
/// slicing in [`copy_selection_text_from_raw_lines`] exactly so the displayed
/// "N chars · M lines" matches what would actually be copied.
pub(super) fn copy_selection_metrics_from_raw_lines(
    snapshot: &CopyViewportSnapshot,
    start: crate::tui::CopySelectionPoint,
    end: crate::tui::CopySelectionPoint,
) -> Option<(usize, usize)> {
    if let Some(text) = copy_selection_text_with_math_targets(snapshot, start, end) {
        return Some((text.chars().count(), text.split('\n').count().max(1)));
    }
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

    let mut chars = 0usize;
    let mut lines = 0usize;
    for raw_line in start.raw_line..=end.raw_line {
        if raw_line > start.raw_line {
            chars += 1; // the joining '\n'
        }
        lines += 1;
        let text = snapshot.raw_plain_line(raw_line)?;
        if raw_line != start.raw_line && raw_line != end.raw_line {
            chars += text.chars().count();
            continue;
        }
        let line_width = line_display_width(text);
        let start_col = if raw_line == start.raw_line {
            clamp_display_col(text, start.column)
        } else {
            0
        };
        let end_col = if raw_line == end.raw_line {
            clamp_display_col(text, end.column)
        } else {
            line_width
        };
        if end_col < start_col {
            continue;
        }
        chars += display_col_slice(text, start_col, end_col).chars().count();
    }

    Some((chars, lines.max(1)))
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
    fn selection_inside_latex_image_copies_complete_source() {
        let snapshot = math_snapshot();
        assert_eq!(
            copy_selection_text_from_raw_lines(&snapshot, point(1, 0), point(3, 0)),
            Some("$$\nx^2 + \\alpha\n$$".to_string())
        );
    }
}
