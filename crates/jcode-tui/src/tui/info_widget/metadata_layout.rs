use super::{InfoWidgetData, WidgetKind, compute_page_layout, render_widget_content};
use ratatui::layout::Rect;

/// Calculate the height needed for a specific widget type
pub(crate) fn calculate_widget_height(
    kind: WidgetKind,
    data: &InfoWidgetData,
    width: u16,
    max_height: u16,
) -> u16 {
    let inner_width = width.saturating_sub(2) as usize;
    let border_height = 2u16;

    let content_height = match kind {
        WidgetKind::WorkspaceMap => {
            if data.workspace_rows.is_empty() {
                return 0;
            }
            let (_preferred_w, preferred_h) =
                crate::tui::workspace_map_widget::preferred_size(&data.workspace_rows);
            preferred_h.min(max_height.saturating_sub(border_height))
        }
        WidgetKind::Overview => {
            let mut overview = data.clone();
            // Keep memory in its own widget so graph rendering stays focused.
            overview.memory_info = None;
            let inner_h = max_height.saturating_sub(border_height);
            let layout = compute_page_layout(&overview, inner_width, inner_h);
            if layout.max_page_height == 0 {
                // Taller than the pocket even as a compact page. Rather than
                // dropping the Overview (and with it usage limits and changes,
                // which it merges in place of their own widgets), take the
                // pocket and let the renderer truncate the compact page. The
                // sections are ordered by importance, so the tail is what goes.
                let full = compute_page_layout(&overview, inner_width, u16::MAX);
                if full.max_page_height == 0
                    || inner_h
                        < WidgetKind::Overview
                            .min_height()
                            .saturating_sub(border_height)
                {
                    return 0;
                }
                return max_height;
            }
            layout.max_page_height
        }
        WidgetKind::Diagrams => {
            if data.diagrams.is_empty() {
                return 0;
            }
            // Use the full available height so the image fills the panel
            max_height.saturating_sub(border_height)
        }
        // Text widgets: the height is whatever the body actually renders at
        // the full available height, so estimate and render can never drift.
        _ => {
            // Rendering is permissive (e.g. the swarm widget still draws a
            // session list), but layout only admits widgets with data.
            if !data.has_data_for(kind) {
                return 0;
            }
            let inner = Rect::new(
                0,
                0,
                width.saturating_sub(2),
                max_height.saturating_sub(border_height),
            );
            let framed = render_widget_content(kind, data, inner);
            if framed.is_empty() {
                return 0;
            }
            framed.lines.len() as u16
        }
    };

    let total = content_height + border_height;
    total.min(max_height)
}

pub(super) fn diagram_footer(data: &InfoWidgetData) -> String {
    let Some(diagram) = data.diagrams.first() else {
        return "image pending".into();
    };
    let dimensions = format!("{}×{}", diagram.width, diagram.height);
    if data.diagrams.len() > 1 {
        format!("{dimensions} · 1/{}", data.diagrams.len())
    } else {
        dimensions
    }
}

pub(super) fn workspace_footer(data: &InfoWidgetData) -> String {
    use crate::tui::workspace_map::WorkspaceSessionVisualState;
    let (count, running) = data
        .workspace_rows
        .iter()
        .flat_map(|row| &row.sessions)
        .fold((0, 0), |(count, running), session| {
            (
                count + 1,
                running + usize::from(session.state == WorkspaceSessionVisualState::Running),
            )
        });
    let noun = if count == 1 { "session" } else { "sessions" };
    if running > 0 {
        format!("{count} {noun} · {running} running")
    } else {
        format!("{count} {noun}")
    }
}

/// Token headroom complements the status line's context percentage rather
/// than relabeling the Overview or repeating one of its detail rows.
pub(super) fn overview_footer(data: &InfoWidgetData) -> String {
    if data.context_info_stale {
        return "context syncing".into();
    }
    let used = data.observed_context_tokens.or_else(|| {
        data.context_info
            .as_ref()
            .filter(|info| info.total_chars > 0)
            .map(|info| info.estimated_tokens() as u64)
    });
    match (used, data.context_limit) {
        (Some(used), Some(limit)) => {
            let left = (limit as u64).saturating_sub(used);
            let estimate = if data.observed_context_tokens.is_none() {
                "~"
            } else {
                ""
            };
            format!(
                "{estimate}{} ctx left",
                crate::kv_cache_monitor::format_token_count(left)
            )
        }
        _ => "context not reported".into(),
    }
}
