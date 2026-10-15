use super::*;

pub(crate) fn copy_pane_vertical_edge_point(
    pane: crate::tui::CopySelectionPane,
    column: u16,
    row: u16,
) -> Option<(crate::tui::CopySelectionPoint, bool)> {
    // The prompt composer cannot be wheel-scrolled, so it has no browser-style
    // edge auto-scroll. Drags past its edge clamp via `copy_pane_drag_point`.
    if pane == crate::tui::CopySelectionPane::Input {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(pane)?;
    let area = snapshot.content_area;
    if area.width == 0 || area.height == 0 {
        return None;
    }

    // Browser-style edge auto-scroll: terminals clamp the mouse to the visible
    // viewport, so a drag that "leaves" the top/bottom of the pane is reported on
    // the boundary row itself. We additionally treat a small band near each edge
    // as a hot zone, so dragging *near* (not just exactly onto) the top/bottom
    // keeps pulling in more transcript, just like dragging a selection toward the
    // edge of a browser window. The horizontal position is clamped into the pane
    // so the selection extends no matter where along the edge the cursor sits.
    let last_row = area.y.saturating_add(area.height).saturating_sub(1);
    let zone = edge_autoscroll_zone_rows(area.height);
    let top_trigger = area.y.saturating_add(zone);
    let bottom_trigger = last_row.saturating_sub(zone);
    // Only engage the hot zone when there is actually more transcript to pull in
    // that direction. Otherwise dragging into the bottom band while the view is
    // already pinned to the end (the common case) would snap the selection to the
    // last visible line and fight precise highlighting of the bottom rows. When
    // there is nothing to scroll, fall through (`None`) so the caller extends the
    // selection to the exact cell under the cursor instead.
    let can_scroll_up = snapshot.scroll > 0;
    let can_scroll_down = snapshot.visible_end < snapshot.wrapped_plain_line_count();
    let (edge_row, upward) = if row <= top_trigger && can_scroll_up {
        (area.y, true)
    } else if row >= bottom_trigger && can_scroll_down {
        (last_row, false)
    } else {
        return None;
    };

    let clamped_col = column.clamp(area.x, area.x.saturating_add(area.width).saturating_sub(1));

    copy_point_from_snapshot(&snapshot, clamped_col, edge_row).map(|point| (point, upward))
}

/// Resolve the selection point for a drag at `(column, row)`, clamping vertical
/// overshoot to the nearest in-bounds line edge.
///
/// Terminals report a drag that "leaves" the pane on the boundary row, but a
/// drag *into the empty space below the last content line* (common with short
/// transcripts that leave blank rows underneath) lands on a row that maps to no
/// line at all, so `copy_point_from_screen` returns `None`. Native terminal and
/// browser selection treat that as "select through the end of the last line".
/// This mirrors that: dragging below the last visible line snaps to the end of
/// that line, and dragging above the first visible line snaps to its start, so
/// the boundary line is fully covered even when there is nothing more to scroll.
pub(crate) fn copy_pane_drag_point(
    pane: crate::tui::CopySelectionPane,
    column: u16,
    row: u16,
) -> Option<crate::tui::CopySelectionPoint> {
    let snapshot = copy_snapshot_for_pane(pane)?;
    let area = snapshot.content_area;
    if area.width == 0 || area.height == 0 {
        return None;
    }

    // A direct hit on a real line wins: precise per-cell selection.
    if let Some(point) = copy_point_from_snapshot(&snapshot, column, row) {
        return Some(point);
    }

    let line_count = snapshot.wrapped_plain_line_count();
    if line_count == 0 {
        return None;
    }
    let last_line = line_count.saturating_sub(1);
    let last_visible_line = snapshot.visible_end.saturating_sub(1).min(last_line);
    let first_visible_line = snapshot.scroll.min(last_line);

    let last_row = area.y.saturating_add(area.height).saturating_sub(1);
    let clamped_col = column.clamp(area.x, area.x.saturating_add(area.width).saturating_sub(1));

    // Below the visible content: snap to the end of the last visible line.
    if row >= last_row {
        let text = snapshot.wrapped_plain_line(last_visible_line).unwrap_or("");
        return Some(crate::tui::CopySelectionPoint {
            pane,
            abs_line: last_visible_line,
            column: line_display_width(text),
        });
    }

    // Above the visible content: snap to the start of the first visible line.
    if row <= area.y {
        return Some(crate::tui::CopySelectionPoint {
            pane,
            abs_line: first_visible_line,
            column: snapshot
                .wrapped_copy_offset(first_visible_line)
                .unwrap_or(0),
        });
    }

    // Interior row that maps to no line (e.g. a blank gap row between/after
    // content within the visible band): fall back to the boundary-clamped point.
    copy_point_from_snapshot(&snapshot, clamped_col, row.clamp(area.y, last_row)).or(Some(
        crate::tui::CopySelectionPoint {
            pane,
            abs_line: last_visible_line,
            column: line_display_width(
                snapshot.wrapped_plain_line(last_visible_line).unwrap_or(""),
            ),
        },
    ))
}
