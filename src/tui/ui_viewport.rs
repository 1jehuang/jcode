use super::*;
fn lower_bound(values: &[usize], target: usize) -> usize {
    values.partition_point(|&v| v < target)
}

pub(super) fn compute_visible_margins(
    lines: &[Line],
    user_line_indices: &[usize],
    scroll: usize,
    area: Rect,
    centered: bool,
) -> info_widget::Margins {
    let visible_height = area.height as usize;
    let visible_end = scroll + visible_height;
    let visible_user_start = lower_bound(user_line_indices, scroll);
    let visible_user_end = lower_bound(user_line_indices, visible_end);
    let visible_user_indices = &user_line_indices[visible_user_start..visible_user_end];

    let mut right_widths = Vec::with_capacity(visible_height);
    let mut left_widths = Vec::with_capacity(visible_height);

    for row in 0..visible_height {
        let line_idx = scroll + row;
        if line_idx < lines.len() {
            let mut used = lines[line_idx].width().min(area.width as usize) as u16;
            if visible_user_indices.binary_search(&line_idx).is_ok() && area.width > 0 {
                used = used.saturating_add(1).min(area.width);
            }

            if centered {
                let total_margin = area.width.saturating_sub(used);
                let effective_alignment = lines[line_idx].alignment.unwrap_or(Alignment::Center);
                let (left_margin, right_margin) = match effective_alignment {
                    Alignment::Left => (0, total_margin),
                    Alignment::Center => {
                        let left = total_margin / 2;
                        let right = total_margin.saturating_sub(left);
                        (left, right)
                    }
                    Alignment::Right => (total_margin, 0),
                };
                left_widths.push(left_margin);
                right_widths.push(right_margin);
            } else {
                left_widths.push(0);
                right_widths.push(area.width.saturating_sub(used));
            }
        } else if centered {
            let half = area.width / 2;
            left_widths.push(half);
            right_widths.push(area.width.saturating_sub(half));
        } else {
            left_widths.push(0);
            right_widths.push(area.width);
        }
    }

    info_widget::Margins {
        right_widths,
        left_widths,
        centered,
    }
}

pub(super) fn draw_messages(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    prepared: &PreparedMessages,
) -> info_widget::Margins {
    let wrapped_lines = &prepared.wrapped_lines;
    let wrapped_user_indices = &prepared.wrapped_user_indices;
    let wrapped_user_prompt_starts = &prepared.wrapped_user_prompt_starts;
    let wrapped_user_prompt_ends = &prepared.wrapped_user_prompt_ends;
    let user_prompt_texts = &prepared.user_prompt_texts;

    let total_lines = wrapped_lines.len();
    let visible_height = area.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);

    LAST_MAX_SCROLL.store(max_scroll, Ordering::Relaxed);
    update_user_prompt_positions(wrapped_user_prompt_starts);

    let user_scroll = app.scroll_offset().min(max_scroll);
    let scroll = if app.auto_scroll_paused() {
        user_scroll.min(max_scroll)
    } else {
        max_scroll
    };

    let active_file_context = if app.diff_mode().is_file() {
        active_file_diff_context(prepared, scroll, visible_height)
    } else {
        None
    };

    let margins = compute_visible_margins(
        wrapped_lines,
        wrapped_user_indices,
        scroll,
        area,
        app.centered_mode(),
    );

    let prompt_preview_lines = if crate::config::config().display.prompt_preview && scroll > 0 {
        compute_prompt_preview_line_count(
            wrapped_user_prompt_starts,
            user_prompt_texts,
            scroll,
            area.width,
        )
    } else {
        0u16
    };

    let mut margins = margins;
    for row in 0..(prompt_preview_lines as usize) {
        if row < margins.right_widths.len() {
            margins.right_widths[row] = 0;
        }
        if row < margins.left_widths.len() {
            margins.left_widths[row] = 0;
        }
    }

    let visible_end = (scroll + visible_height).min(wrapped_lines.len());

    let now_ms = app.now_millis();
    let prompt_anim_enabled = crate::config::config().display.prompt_entry_animation
        && crate::perf::profile().tier.prompt_entry_animation_enabled();
    if prompt_anim_enabled {
        update_prompt_entry_animation(wrapped_user_prompt_starts, scroll, visible_end, now_ms);
    } else {
        record_prompt_viewport(scroll, visible_end);
    }

    let active_prompt_anim = if prompt_anim_enabled {
        active_prompt_entry_animation(now_ms)
    } else {
        None
    };

    let visible_user_start = lower_bound(wrapped_user_indices, scroll);
    let visible_user_end = lower_bound(wrapped_user_indices, visible_end);

    let mut visible_lines = if scroll < visible_end {
        wrapped_lines[scroll..visible_end].to_vec()
    } else {
        Vec::new()
    };
    if visible_lines.len() < visible_height {
        visible_lines
            .extend(std::iter::repeat(Line::from("")).take(visible_height - visible_lines.len()));
    }

    clear_area(frame, area);

    if let Some(anim) = active_prompt_anim {
        let t = (now_ms.saturating_sub(anim.start_ms) as f32 / PROMPT_ENTRY_ANIMATION_MS as f32)
            .clamp(0.0, 1.0);

        let prompt_idx = lower_bound(wrapped_user_prompt_starts, anim.line_idx);
        if prompt_idx < wrapped_user_prompt_starts.len()
            && wrapped_user_prompt_starts[prompt_idx] == anim.line_idx
        {
            let prompt_end = wrapped_user_prompt_ends
                .get(prompt_idx)
                .copied()
                .unwrap_or(anim.line_idx + 1);

            for abs_idx in anim.line_idx.max(scroll)..prompt_end.min(visible_end) {
                let rel_idx = abs_idx - scroll;
                if let Some(line) = visible_lines.get_mut(rel_idx) {
                    for span in &mut line.spans {
                        if !span.content.is_empty() {
                            let base = match span.style.fg {
                                Some(c) => c,
                                None => user_text(),
                            };
                            span.style = span.style.fg(prompt_entry_color(base, t));
                        }
                    }
                }
            }
        }
    }

    if let Some(active) = &active_file_context {
        let highlight_style = Style::default().fg(file_link_color()).bold();
        let accent_style = Style::default().fg(file_link_color());

        for abs_idx in active.start_line.max(scroll)..active.end_line.min(visible_end) {
            let rel_idx = abs_idx.saturating_sub(scroll);
            if let Some(line) = visible_lines.get_mut(rel_idx) {
                if abs_idx == active.start_line {
                    line.spans.insert(
                        0,
                        Span::styled(format!("→ edit#{} ", active.edit_index), highlight_style),
                    );
                } else {
                    line.spans.insert(0, Span::styled("  │ ", accent_style));
                }
            }
        }
    }

    frame.render_widget(Paragraph::new(visible_lines), area);

    let centered = app.centered_mode();
    let diagram_mode = app.diagram_mode();
    if diagram_mode != crate::config::DiagramDisplayMode::Pinned {
        let visible_image_start = prepared
            .image_regions
            .partition_point(|region| region.end_line <= scroll);
        let visible_image_end = prepared
            .image_regions
            .partition_point(|region| region.abs_line_idx < visible_end);

        for region in &prepared.image_regions[visible_image_start..visible_image_end] {
            let abs_idx = region.abs_line_idx;
            let hash = region.hash;
            let total_height = region.height;
            let image_end = region.end_line;

            if image_end > scroll && abs_idx < visible_end {
                let marker_visible = abs_idx >= scroll && abs_idx < visible_end;

                if marker_visible {
                    let screen_y = (abs_idx - scroll) as u16;
                    let available_height = (visible_height as u16).saturating_sub(screen_y);
                    let render_height = (total_height as u16).min(available_height);

                    if render_height > 0 {
                        let image_area = Rect {
                            x: area.x,
                            y: area.y + screen_y,
                            width: area.width,
                            height: render_height,
                        };
                        let rows = crate::tui::mermaid::render_image_widget(
                            hash,
                            image_area,
                            frame.buffer_mut(),
                            centered,
                            false,
                        );
                        if rows == 0 {
                            frame.render_widget(
                                Paragraph::new(Line::from(Span::styled(
                                    "↗ mermaid diagram unavailable",
                                    Style::default().fg(dim_color()),
                                ))),
                                image_area,
                            );
                        }
                    }
                } else {
                    let visible_start = scroll.max(abs_idx);
                    let visible_end_img = visible_end.min(image_end);
                    let screen_y = (visible_start - scroll) as u16;
                    let render_height = (visible_end_img - visible_start) as u16;

                    if render_height > 0 {
                        let image_area = Rect {
                            x: area.x,
                            y: area.y + screen_y,
                            width: area.width,
                            height: render_height,
                        };
                        crate::tui::mermaid::render_image_widget(
                            hash,
                            image_area,
                            frame.buffer_mut(),
                            centered,
                            true,
                        );
                    }
                }
            }
        }
    }

    let right_x = area.x + area.width.saturating_sub(1);
    for &line_idx in &wrapped_user_indices[visible_user_start..visible_user_end] {
        if line_idx >= scroll && line_idx < scroll + visible_height {
            let screen_y = area.y + (line_idx - scroll) as u16;
            let bar_area = Rect {
                x: right_x,
                y: screen_y,
                width: 1,
                height: 1,
            };
            let bar = Paragraph::new(Span::styled("│", Style::default().fg(user_color())));
            frame.render_widget(bar, bar_area);
        }
    }

    if scroll > 0 {
        let indicator = format!("↑{}", scroll);
        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(indicator.len() as u16 + 2),
            y: area.y,
            width: indicator.len() as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                indicator,
                Style::default().fg(dim_color()),
            )])),
            indicator_area,
        );
    }

    if crate::config::config().display.prompt_preview && scroll > 0 {
        let last_offscreen_prompt_idx =
            lower_bound(wrapped_user_prompt_starts, scroll).checked_sub(1);

        if let Some(prompt_order) = last_offscreen_prompt_idx {
            if let Some(prompt_text) = user_prompt_texts.get(prompt_order) {
                let prompt_text = prompt_text.trim();
                if !prompt_text.is_empty() {
                    let prompt_num = prompt_order + 1;
                    let num_str = format!("{}", prompt_num);
                    let prefix_len = num_str.len() + 2;
                    let content_width = area.width.saturating_sub(prefix_len as u16 + 2) as usize;
                    let dim_style = Style::default().dim();
                    let align = if app.centered_mode() {
                        ratatui::layout::Alignment::Center
                    } else {
                        ratatui::layout::Alignment::Left
                    };

                    let text_flat = prompt_text.replace('\n', " ");
                    let text_chars: Vec<char> = text_flat.chars().collect();
                    let is_long = text_chars.len() > content_width;

                    let preview_lines: Vec<Line<'static>> = if !is_long {
                        vec![Line::from(vec![
                            Span::styled(num_str.clone(), dim_style.fg(dim_color())),
                            Span::styled("› ", dim_style.fg(user_color())),
                            Span::styled(text_flat, dim_style.fg(user_text())),
                        ])
                        .alignment(align)]
                    } else {
                        let half = content_width.max(4);
                        let head: String =
                            text_chars[..half.min(text_chars.len())].iter().collect();
                        let tail_start = text_chars.len().saturating_sub(half);
                        let tail: String = text_chars[tail_start..].iter().collect();

                        let first = Line::from(vec![
                            Span::styled(num_str.clone(), dim_style.fg(dim_color())),
                            Span::styled("› ", dim_style.fg(user_color())),
                            Span::styled(
                                format!("{} ...", head.trim_end()),
                                dim_style.fg(user_text()),
                            ),
                        ])
                        .alignment(align);

                        let padding: String = " ".repeat(prefix_len);
                        let second = Line::from(vec![
                            Span::styled(padding, dim_style),
                            Span::styled(
                                format!("... {}", tail.trim_start()),
                                dim_style.fg(user_text()),
                            ),
                        ])
                        .alignment(align);

                        vec![first, second]
                    };

                    let line_count = preview_lines.len() as u16;
                    let preview_area = Rect {
                        x: area.x,
                        y: area.y,
                        width: area.width.saturating_sub(1),
                        height: line_count,
                    };
                    clear_area(frame, preview_area);
                    frame.render_widget(Paragraph::new(preview_lines), preview_area);
                }
            }
        }
    }

    if app.auto_scroll_paused() && scroll < max_scroll {
        let indicator = format!("↓{}", max_scroll - scroll);
        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(indicator.len() as u16 + 2),
            y: area.y + area.height.saturating_sub(1),
            width: indicator.len() as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                indicator,
                Style::default().fg(queued_color()),
            )])),
            indicator_area,
        );
    }

    margins
}

fn compute_prompt_preview_line_count(
    wrapped_user_prompt_starts: &[usize],
    user_prompt_texts: &[String],
    scroll: usize,
    area_width: u16,
) -> u16 {
    let last_offscreen = lower_bound(wrapped_user_prompt_starts, scroll).checked_sub(1);
    let Some(prompt_order) = last_offscreen else {
        return 0;
    };
    let Some(prompt_text) = user_prompt_texts.get(prompt_order) else {
        return 0;
    };
    let prompt_text = prompt_text.trim();
    if prompt_text.is_empty() {
        return 0;
    }
    let num_str = format!("{}", prompt_order + 1);
    let prefix_len = num_str.len() + 2;
    let content_width = area_width.saturating_sub(prefix_len as u16 + 2) as usize;
    let text_flat = prompt_text.replace('\n', " ");
    let char_count = text_flat.chars().count();
    if char_count > content_width {
        2
    } else {
        1
    }
}
