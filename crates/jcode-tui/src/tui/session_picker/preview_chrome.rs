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

fn next_prompt_number(session: &SessionInfo, extras: Option<&PreviewExtras>) -> usize {
    let visible_users = session
        .messages_preview
        .iter()
        .filter(|message| message.role == "user" && !message.content.trim().is_empty())
        .count();
    let preview_users = extras
        .map_or(0, |extras| extras.hidden_prompts)
        .saturating_add(visible_users);
    // The index count already includes prompts outside the preview window.
    // Use the window as a lower bound when an index snapshot is stale, not as
    // an additional offset that would count hidden prompts twice.
    session
        .user_message_count
        .max(preview_users)
        .saturating_add(1)
}

/// Draw only the image rows intersecting the scrolling transcript. Raster
/// images retain their original fit box when partially scrolled off screen.
pub(super) fn draw_images(
    frame: &mut Frame,
    area: Rect,
    regions: &[jcode_tui_messages::ImageRegion],
    source_rows: &[usize],
    reserved: Option<Rect>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    for region in regions {
        let start = source_rows.partition_point(|row| *row < region.abs_line_idx);
        let end = source_rows.partition_point(|row| *row < region.end_line);
        if region.height == 0 || start >= end {
            continue;
        }
        let source_start = source_rows[start];
        let mut image_area = Rect {
            x: area.x,
            y: area.y + start as u16,
            width: area.width,
            height: (end - start).min(u16::MAX as usize) as u16,
        };
        if let Some(panel) = reserved
            && image_area.y < panel.bottom()
            && panel.y < image_area.bottom()
        {
            image_area.width = panel
                .x
                .saturating_sub(area.x)
                .saturating_sub(1)
                .min(area.width);
        }
        if image_area.width == 0 {
            continue;
        }
        let fit = region.render == jcode_tui_messages::ImageRegionRender::Fit;
        if fit {
            let cols = if region.width == 0 {
                image_area.width
            } else {
                region.width.min(image_area.width)
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
                (source_start - region.abs_line_idx).min(u16::MAX as usize) as u16,
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
                source_start > region.abs_line_idx,
            );
            if rows == 0 && source_start == region.abs_line_idx {
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
    if transcript.height > 1 {
        let input_area = Rect {
            y: transcript.bottom() - 1,
            height: 1,
            ..transcript
        };
        super::super::ui::input_ui::draw_preview_input(
            frame,
            input_area,
            next_prompt_number(session, extras),
        );
        transcript.height -= 1;
    }
    // The live idle layout keeps a status row immediately above the composer.
    // No live processing state belongs to this selected-session snapshot.
    if transcript.height > 1 {
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

#[cfg(test)]
#[path = "preview_composer_tests.rs"]
mod composer_tests;
