use super::info_widget;
use super::visual_debug::{InfoWidgetSummary, WidgetPlacementCapture};
use ratatui::prelude::Rect;

pub(super) fn capture_widget_placements(
    placements: &[info_widget::WidgetPlacement],
) -> Vec<WidgetPlacementCapture> {
    placements
        .iter()
        .map(|p| WidgetPlacementCapture {
            kind: p.kind.as_str().to_string(),
            side: p.side.as_str().to_string(),
            rect: p.rect.into(),
        })
        .collect()
}

pub(super) fn build_info_widget_summary(data: &info_widget::InfoWidgetData) -> InfoWidgetSummary {
    let todos_total = data.todos.len();
    let todos_done = data
        .todos
        .iter()
        .filter(|t| t.status == "completed")
        .count();

    let context_total_chars = data.context_info.as_ref().map(|c| c.total_chars);
    let context_limit = data.context_limit;

    let memory_total = data.memory_info.as_ref().map(|m| m.total_count);
    let memory_project = data.memory_info.as_ref().map(|m| m.project_count);
    let memory_global = data.memory_info.as_ref().map(|m| m.global_count);
    let memory_activity = data.memory_info.as_ref().map(|m| m.activity.is_some());

    let swarm_session_count = data.swarm_info.as_ref().map(|s| s.session_count);
    let swarm_member_count = data.swarm_info.as_ref().map(|s| s.members.len());
    let swarm_subagent_status = data
        .swarm_info
        .as_ref()
        .and_then(|s| s.subagent_status.clone());

    let background_running = data.background_info.as_ref().map(|b| {
        if b.rows.is_empty() {
            b.running_count
        } else {
            b.rows
                .iter()
                .filter(|row| row.status == crate::tui::BackgroundTaskRowStatus::Running)
                .count()
        }
    });
    let background_tasks = data
        .background_info
        .as_ref()
        .map(|b| b.running_tasks.len().max(b.rows.len()));

    let usage_available = data.usage_info.as_ref().map(|u| u.available);
    let usage_provider = data
        .usage_info
        .as_ref()
        .map(|u| format!("{:?}", u.provider));

    InfoWidgetSummary {
        todos_total,
        todos_done,
        context_total_chars,
        context_limit,
        queue_mode: data.queue_mode,
        model: data.model.clone(),
        reasoning_effort: data.reasoning_effort.clone(),
        session_count: data.session_count,
        client_count: data.client_count,
        memory_total,
        memory_project,
        memory_global,
        memory_activity,
        swarm_session_count,
        swarm_member_count,
        swarm_subagent_status,
        background_running,
        background_tasks,
        usage_available,
        usage_provider,
        tokens_per_second: data.tokens_per_second,
        auth_method: Some(format!("{:?}", data.auth_method)),
        upstream_provider: data.upstream_provider.clone(),
    }
}

pub(super) fn rects_overlap(a: Rect, b: Rect) -> bool {
    if a.width == 0 || a.height == 0 || b.width == 0 || b.height == 0 {
        return false;
    }
    let a_right = a.x.saturating_add(a.width);
    let a_bottom = a.y.saturating_add(a.height);
    let b_right = b.x.saturating_add(b.width);
    let b_bottom = b.y.saturating_add(b.height);
    a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
}

pub(super) fn rect_within_bounds(rect: Rect, bounds: Rect) -> bool {
    let right = rect.x.saturating_add(rect.width);
    let bottom = rect.y.saturating_add(rect.height);
    let bounds_right = bounds.x.saturating_add(bounds.width);
    let bounds_bottom = bounds.y.saturating_add(bounds.height);
    rect.x >= bounds.x && rect.y >= bounds.y && right <= bounds_right && bottom <= bounds_bottom
}

/// Detect whether an info-widget placement intrudes into used content rather
/// than sitting in the free margin the layout reported.
///
/// Info widgets are *expected* to live inside the messages rectangle, so a plain
/// `rects_overlap(widget, messages_area)` is always true and tells us nothing.
/// The meaningful invariant is per-row: for each row the widget covers, the
/// reported free width on the widget's side must be large enough to hold it. If
/// the widget extends past the free margin into the content column (e.g. over a
/// centered header line), that is the real anomaly.
pub(super) fn widget_overlaps_content(
    placement: &info_widget::WidgetPlacement,
    messages_area: Rect,
    margins: &info_widget::Margins,
) -> bool {
    use info_widget::Side;

    let rect = placement.rect;
    if rect.width == 0 || rect.height == 0 {
        return false;
    }

    let widths = match placement.side {
        Side::Right => &margins.right_widths,
        Side::Left => &margins.left_widths,
    };

    let row_start = rect.y.saturating_sub(messages_area.y) as usize;
    let row_end = row_start + rect.height as usize;
    for row in row_start..row_end {
        // A row outside the margin table means we have no slack recorded for it,
        // so any widget coverage there is intrusion.
        let free = widths.get(row).copied().unwrap_or(0);
        if rect.width > free {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::info_widget::{Margins, Side, WidgetKind, WidgetPlacement};

    fn placement(side: Side, x: u16, y: u16, w: u16, h: u16) -> WidgetPlacement {
        WidgetPlacement {
            kind: WidgetKind::Overview,
            rect: Rect::new(x, y, w, h),
            side,
        }
    }

    #[test]
    fn widget_fitting_in_reported_margin_is_not_flagged() {
        let area = Rect::new(0, 0, 80, 5);
        let margins = Margins {
            right_widths: vec![30; 5],
            left_widths: vec![0; 5],
            centered: false,
            ..Default::default()
        };
        // 30-wide widget pinned to the right edge, fully inside the 30-col gap.
        let p = placement(Side::Right, 50, 0, 30, 5);
        assert!(!widget_overlaps_content(&p, area, &margins));
    }

    #[test]
    fn widget_wider_than_free_margin_intrudes() {
        let area = Rect::new(0, 0, 80, 5);
        // A centered header line on row 2 shrinks the right gap; a widget that was
        // sized off a stale/over-reported margin would intrude here.
        let margins = Margins {
            right_widths: vec![30, 30, 12, 30, 30],
            left_widths: vec![0; 5],
            centered: false,
            ..Default::default()
        };
        let p = placement(Side::Right, 50, 0, 30, 5);
        assert!(widget_overlaps_content(&p, area, &margins));
    }

    #[test]
    fn widget_on_row_without_recorded_margin_intrudes() {
        let area = Rect::new(0, 0, 80, 5);
        let margins = Margins {
            right_widths: vec![30, 30],
            left_widths: vec![0, 0],
            centered: false,
            ..Default::default()
        };
        // Rows 2..5 have no recorded slack, so coverage there is intrusion.
        let p = placement(Side::Right, 50, 0, 30, 5);
        assert!(widget_overlaps_content(&p, area, &margins));
    }
}
