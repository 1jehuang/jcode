//! Selected-session chrome. Never consult the App behind the picker: its
//! credentials, git tree, quotas and in-flight work belong to another session.
use super::*;
use crate::tui::info_widget::{self, InfoWidgetData};

/// Only durable facts are eligible for preview widgets. In particular the
/// character-based transcript estimate is not an API context measurement.
pub(super) fn persisted_info(session: &Session) -> InfoWidgetData {
    InfoWidgetData {
        model: session.model.clone(),
        provider_name: session.provider_key.clone(),
        working_dir: session.working_dir.clone(),
        session_name: session.short_name.clone(),
        reasoning_effort: session.reasoning_effort.clone(),
        context_limit: session.context_cap,
        ..Default::default()
    }
}

fn selected_info(session: &SessionInfo, extras: Option<&PreviewExtras>) -> InfoWidgetData {
    let mut data = extras
        .and_then(|extras| extras.info_data.clone())
        .unwrap_or_default();
    // The index may have fresher identity than the saved transcript.
    data.model = session.model.clone().or(data.model);
    data.provider_name = session.provider_key.clone().or(data.provider_name);
    data.working_dir = session.working_dir.clone().or(data.working_dir);
    data.session_name = Some(session.short_name.clone());
    data
}

/// Reserve stationary chrome around the scrolling transcript, like the live
/// layout. At tiny heights prefer preserving one transcript row over widgets.
pub(super) fn draw(
    frame: &mut Frame,
    area: Rect,
    session: &SessionInfo,
    extras: Option<&PreviewExtras>,
) -> Rect {
    let data = selected_info(session, extras);
    let mut transcript = area;
    if transcript.height > 2 {
        let lines = super::super::ui::header::build_preview_header(session, area.width);
        let height = (lines.len() as u16).min(transcript.height.saturating_sub(2));
        frame.render_widget(
            Paragraph::new(lines),
            Rect {
                height,
                ..transcript
            },
        );
        transcript.y += height;
        transcript.height -= height;
    }
    if transcript.height > 1 {
        let status_area = Rect {
            y: transcript.y + transcript.height - 1,
            height: 1,
            ..transcript
        };
        frame.render_widget(
            Paragraph::new(super::super::ui::input_ui::preview_overscroll_line(
                &data, area.width,
            )),
            status_area,
        );
        transcript.height -= 1;
    }
    transcript
}

/// The same margin measurement and placement engine as the live chat, with
/// fresh private anchors so preview selection cannot move the live HUD.
pub(super) fn draw_widgets(
    frame: &mut Frame,
    area: Rect,
    lines: &[Line<'static>],
    session: &SessionInfo,
    extras: Option<&PreviewExtras>,
) {
    let data = selected_info(session, extras);
    let margins = super::super::ui::viewport::compute_visible_margins(lines, &[], area, false);
    let placements = super::super::info_widget_layout::calculate_placements_anchored(
        area,
        &margins,
        &data,
        info_widget::is_enabled(),
        &[],
    );
    info_widget::render_all(frame, &placements.visible, &data);
}

#[cfg(test)]
#[path = "preview_chrome_tests.rs"]
mod tests;
