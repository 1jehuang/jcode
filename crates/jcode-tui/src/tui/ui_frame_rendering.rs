//! Frame rendering and incremental cache helpers.

use super::*;

impl BodyCacheState {
    pub(super) fn take_best_incremental_base(
        &mut self,
        key: &BodyCacheKey,
    ) -> Option<(Arc<PreparedMessages>, usize, usize)> {
        let regular = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.msg_count > 0
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
                    && entry.key.compact_thinking == key.compact_thinking
                    && entry.key.mermaid_aspect_bucket == key.mermaid_aspect_bucket
                    // Anchored inline images render inside the body, and a
                    // late-arriving image may target an already-prepared
                    // message; only reuse bases built with the same image set.
                    && entry.key.pin_images == key.pin_images
                    && entry.key.inline_images_visible == key.inline_images_visible
                    && entry.key.images_signature == key.images_signature
                    && entry.key.expanded_images_version == key.expanded_images_version
                    && entry.key.swarm_members_signature == key.swarm_members_signature
                    && entry.key.latex_rendering == key.latex_rendering
                    && entry.key.show_link_urls == key.show_link_urls
            })
            .max_by_key(|(_, entry)| entry.msg_count)
            .map(|(idx, entry)| (false, idx, entry.msg_count));
        let oversized = self
            .oversized_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.msg_count > 0
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
                    && entry.key.compact_thinking == key.compact_thinking
                    && entry.key.mermaid_aspect_bucket == key.mermaid_aspect_bucket
                    // Anchored inline images render inside the body, and a
                    // late-arriving image may target an already-prepared
                    // message; only reuse bases built with the same image set.
                    && entry.key.pin_images == key.pin_images
                    && entry.key.inline_images_visible == key.inline_images_visible
                    && entry.key.images_signature == key.images_signature
                    && entry.key.expanded_images_version == key.expanded_images_version
                    && entry.key.swarm_members_signature == key.swarm_members_signature
                    && entry.key.latex_rendering == key.latex_rendering
                    && entry.key.show_link_urls == key.show_link_urls
            })
            .max_by_key(|(_, entry)| entry.msg_count)
            .map(|(idx, entry)| (true, idx, entry.msg_count));

        let chosen = match (regular, oversized) {
            (Some(left), Some(right)) => {
                if left.2 >= right.2 {
                    left
                } else {
                    right
                }
            }
            (Some(entry), None) | (None, Some(entry)) => entry,
            (None, None) => return None,
        };

        let (is_oversized, idx, msg_count) = chosen;
        let entry = if is_oversized {
            self.oversized_entries.remove(idx)?
        } else {
            self.entries.remove(idx)?
        };
        Some((entry.prepared, msg_count, entry.prompt_offset))
    }
}

pub fn draw(frame: &mut Frame, app: &dyn TuiState) {
    if !secondary_render_active() {
        record_idle_animation_area(None);
    }
    if let Some(frozen) = app.hibernation_frame()
        && frozen.area == frame.area()
    {
        frame.buffer_mut().clone_from(frozen);
        if let Some(notice) = app.hibernation_notice() {
            // Keep the transcript, draft and scroll exactly as last painted.
            // The notice replaces only the status line row.
            let area = frame.area();
            let row = last_status_area()
                .filter(|row| area.contains(row.as_position()) && row.bottom() <= area.bottom())
                .unwrap_or_else(|| {
                    Rect::new(
                        area.x,
                        area.bottom().saturating_sub(1),
                        area.width,
                        area.height.min(1),
                    )
                });
            clear_area(frame, row);
            frame.render_widget(
                Paragraph::new(notice).style(Style::default().fg(dim_color())),
                row,
            );
        }
        // In particular do not send deferred Kitty deletions while the frozen
        // screen still shows those images. Normal rendering drains them on wake.
        if visual_debug::is_enabled() {
            let area = frame.area();
            let mut capture = FrameCaptureBuilder::new(area.width, area.height);
            capture.state.status = "hibernated".to_string();
            capture.state.input_len = app.input().len();
            capture.state.input_preview = app.input().chars().take(200).collect();
            capture.state.cursor_pos = app.cursor_pos();
            capture.state.scroll_offset = app.scroll_offset();
            capture.state.message_count = app.display_messages().len();
            capture.rendered_text.status_line = app.hibernation_notice().unwrap_or("").to_string();
            capture
                .render_order
                .push("frozen_hibernation_frame".to_string());
            if !secondary_render_active() {
                visual_debug::record_frame(capture.build());
            }
        }
        return;
    }
    // Suggestions are read many times while composing one frame. Bump the
    // epoch here so the memo is scoped to exactly this frame.
    app.advance_command_suggestions_epoch();
    crate::tui::mermaid::begin_terminal_image_frame();
    match jcode_tui_style::catch_recoverable_panic(|| {
        crate::tui::markdown::with_deferred_mermaid_render_context(|| draw_inner(frame, app))
    }) {
        Ok(()) => layout_support::draw_side_panel_restore(frame, app),
        Err(payload) => render_recovered_panic_frame(frame, &payload),
    }
    // Adapt the finished frame at buffer level so every widget and overlay
    // follows the same policy. User-configured colors remain exact.
    // Attribute explicit palette overrides before light-theme contrast repair
    // can make distinct muted source grays converge to the same rendered ink.
    jcode_tui_style::adapt_buffer_for_display(frame.buffer_mut());
    adapt_buffer_for_emoji_preference(frame.buffer_mut());
    // Classic Kitty placements must follow all text writes. Unlike Unicode
    // placeholders they survive scrolling, so finalize even an image-free frame.
    crate::tui::mermaid::finish_terminal_image_frame(frame.buffer_mut());
    if !secondary_render_active() {
        crate::tui::cloud_latency::rendered();
    }
}

/// Resolve styled markdown labels against their owning source before falling
/// back to the existing bare-URL hit tester. No global label-to-URL registry is
/// used, so repeated labels cannot leak destinations between messages.
pub(crate) fn link_target_from_screen_with_state(
    column: u16,
    row: u16,
    app: &dyn TuiState,
) -> Option<String> {
    let point = copy_point_from_screen(column, row)?;
    if point.pane == crate::tui::CopySelectionPane::Input {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(point.pane)?;
    let target = match &snapshot.data {
        CopyViewportData::ChatFrame { prepared } => prepared.sections.iter().find_map(|section| {
            let local = point.abs_line.checked_sub(section.line_start)?;
            if local >= section.prepared.wrapped_lines.len() {
                return None;
            }
            let (source, start, end) = match section.kind {
                PreparedSectionKind::Body => {
                    let boundaries = &section.prepared.message_boundaries;
                    let index =
                        boundaries.partition_point(|boundary| boundary.wrapped_len <= local);
                    let message = app.display_messages().get(index)?;
                    let end = boundaries.get(index)?.wrapped_len;
                    let start = index
                        .checked_sub(1)
                        .map(|i| boundaries[i].wrapped_len)
                        .unwrap_or(0);
                    (message.content.as_str(), start, end)
                }
                PreparedSectionKind::Streaming => (
                    app.streaming_text(),
                    0,
                    section.prepared.wrapped_lines.len(),
                ),
                _ => return None,
            };
            jcode_tui_markdown::markdown_link_target(
                source,
                section.prepared.wrapped_lines.get(start..end)?,
                local - start,
                point.column,
            )
        }),
        CopyViewportData::Dense {
            link_lines: Some(lines),
            ..
        } if point.pane == crate::tui::CopySelectionPane::SidePane => {
            app.side_panel().focused_page().and_then(|page| {
                jcode_tui_markdown::markdown_link_target(
                    &page.content,
                    lines,
                    point.abs_line,
                    point.column,
                )
            })
        }
        _ => None,
    };
    target.or_else(|| link_target_from_screen(column, row))
}
