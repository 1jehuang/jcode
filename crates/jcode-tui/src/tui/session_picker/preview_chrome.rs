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

/// Draw only the image rows intersecting the scrolling transcript. Raster
/// images retain their original fit box when partially scrolled off screen.
pub(super) fn draw_images(
    frame: &mut Frame,
    area: Rect,
    regions: &[jcode_tui_messages::ImageRegion],
    scroll: usize,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let visible_end = scroll.saturating_add(area.height as usize);
    for region in regions {
        let start = region.abs_line_idx.max(scroll);
        let end = region.end_line.min(visible_end);
        if region.height == 0 || start >= end {
            continue;
        }
        let image_area = Rect {
            x: area.x,
            y: area.y + (start - scroll) as u16,
            width: area.width,
            height: (end - start).min(u16::MAX as usize) as u16,
        };
        let fit = region.render == jcode_tui_messages::ImageRegionRender::Fit;
        if fit {
            let cols = if region.width == 0 {
                area.width
            } else {
                region.width.min(area.width)
            };
            if !super::super::ui::inline_image_ui::ensure_drawable(region.hash, cols, region.height)
            {
                continue;
            }
            if !crate::tui::mermaid::render_image_widget_fit_stable(
                region.hash,
                image_area,
                frame.buffer_mut(),
                cols,
                region.height,
                (start - region.abs_line_idx).min(u16::MAX as usize) as u16,
                false,
                true,
            ) {
                crate::tui::mermaid::render_image_widget_fit(
                    region.hash,
                    image_area,
                    frame.buffer_mut(),
                    false,
                    true,
                );
            }
        } else {
            let rows = crate::tui::mermaid::render_image_widget(
                region.hash,
                image_area,
                frame.buffer_mut(),
                false,
                start > region.abs_line_idx,
            );
            if rows == 0 && start == region.abs_line_idx {
                frame.render_widget(Paragraph::new("↗ mermaid diagram unavailable"), image_area);
            }
        }
    }
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
    image_regions: &[jcode_tui_messages::ImageRegion],
    scroll: usize,
) {
    let data = selected_info(session, extras);
    let mut margins = super::super::ui::viewport::compute_visible_margins(lines, &[], area, false);
    let visible_end = scroll.saturating_add(area.height as usize);
    for region in image_regions {
        let start = region.abs_line_idx.max(scroll);
        let end = region.end_line.min(visible_end);
        let width = if region.width == 0 {
            area.width
        } else {
            region.width.min(area.width)
        };
        for row in start..end {
            let index = row - scroll;
            if let Some(right) = margins.right_widths.get_mut(index) {
                *right = (*right).min(area.width.saturating_sub(width));
            }
        }
    }
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
