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

    let background_running = data.background_info.as_ref().map(|b| b.running_count);
    let background_tasks = data.background_info.as_ref().map(|b| b.running_tasks.len());

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
