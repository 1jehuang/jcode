use super::*;

/// Repack cloud details only when asynchronously loaded Overview rows cannot
/// grow inside their original anchor. Keep the resident layout unless a fresh
/// priority pass fits the full Overview without losing primary visible widgets.
/// The supplemental directory inventory may yield when the viewport is short.
pub(in crate::tui) fn calculate_placements_with_cloud_reflow(
    area: Rect,
    margins: &Margins,
    data: &InfoWidgetData,
    enabled: bool,
    anchors: &[crate::tui::info_widget_layout::WidgetAnchor],
) -> crate::tui::info_widget_layout::PlacementOutcome {
    use crate::tui::info_widget_layout::calculate_placements_anchored;
    let resident = calculate_placements_anchored(area, margins, data, enabled, anchors);
    if data.cloud_info.is_none() || anchors.is_empty() {
        return resident;
    }
    let Some(overview) = resident
        .visible
        .iter()
        .find(|p| p.kind == WidgetKind::Overview)
        .or_else(|| {
            anchors
                .iter()
                .map(|anchor| &anchor.placement)
                .find(|placement| placement.kind == WidgetKind::Overview)
        })
    else {
        return resident;
    };
    let needed =
        calculate_widget_height(WidgetKind::Overview, data, overview.rect.width, area.height);
    if overview.rect.height >= needed {
        return resident;
    }
    let fresh = calculate_placements_anchored(area, margins, data, enabled, &[]);
    let fits = fresh.visible.iter().any(|p| {
        p.kind == WidgetKind::Overview
            && p.rect.height
                >= calculate_widget_height(WidgetKind::Overview, data, p.rect.width, area.height)
    });
    let preserves = resident.visible.iter().all(|old| {
        old.kind == WidgetKind::CloudDirectories
            || fresh.visible.iter().any(|new| new.kind == old.kind)
    });
    if fits && preserves { fresh } else { resident }
}
