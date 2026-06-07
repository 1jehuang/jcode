use unicode_width::UnicodeWidthStr;

use super::CopyViewportSnapshot;
use super::display_width::{clamp_display_col, display_col_slice, line_display_width};
use super::url_regex_support::link_target_for_display_column;

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
        column: clamp_display_col(&text, rel_col).max(copy_start),
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
        let line_width = line_display_width(&text);
        let start_col = if raw_line == start.raw_line {
            clamp_display_col(&text, start.column)
        } else {
            0
        };
        let end_col = if raw_line == end.raw_line {
            clamp_display_col(&text, end.column)
        } else {
            line_width
        };

        if end_col < start_col {
            continue;
        }

        let slice = display_col_slice(&text, start_col, end_col);
        if raw_line == start.raw_line {
            out.reserve(slice.len().saturating_mul(selected_lines.min(8)));
        }
        out.push_str(&slice);
    }

    Some(out)
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
        let line_width = line_display_width(&text);
        let start_col = if raw_line == start.raw_line {
            clamp_display_col(&text, start.column)
        } else {
            0
        };
        let end_col = if raw_line == end.raw_line {
            clamp_display_col(&text, end.column)
        } else {
            line_width
        };
        if end_col < start_col {
            continue;
        }
        chars += display_col_slice(&text, start_col, end_col).chars().count();
    }

    Some((chars, lines.max(1)))
}

pub(super) fn link_target_from_snapshot(
    snapshot: &CopyViewportSnapshot,
    point: crate::tui::CopySelectionPoint,
) -> Option<String> {
    let raw_point = raw_selection_point(snapshot, point)?;
    let raw_text = snapshot.raw_plain_line(raw_point.raw_line)?;
    link_target_for_display_column(&raw_text, raw_point.column)
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
    let local_col = clamp_display_col(&wrapped_text, point.column).max(display_copy_start);
    let segment_width = map.end_col.saturating_sub(map.start_col);
    Some(RawSelectionPoint {
        raw_line: map.raw_line,
        column: map.start_col
            + local_col
                .saturating_sub(display_copy_start)
                .min(segment_width),
    })
}
