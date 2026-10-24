use super::*;

pub(super) fn selected_route_notice_text(
    picker: &crate::tui::InlineInteractiveState,
    route: Option<&crate::tui::PickerOption>,
) -> Option<(String, bool)> {
    if picker.kind == crate::tui::PickerKind::Panels {
        return route.map(|route| (route.detail.clone(), false));
    }
    if picker.kind != crate::tui::PickerKind::Model {
        return None;
    }
    let route = route?;
    let unavailable = !route.available;
    let detail = route_detail_display_text(&route.detail, unavailable)?;
    if unavailable {
        return Some((format!("× {}", detail), true));
    }
    if route_detail_is_limited(&route.detail) {
        return Some((format!("⚠ {}", detail), true));
    }
    if route
        .detail
        .to_ascii_lowercase()
        .contains("inference profile")
    {
        return Some((format!("ⓘ {}", detail), false));
    }
    None
}
