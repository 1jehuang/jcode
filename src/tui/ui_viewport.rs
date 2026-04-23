use super::*;
use unicode_width::UnicodeWidthStr;

fn lower_bound(values: &[usize], target: usize) -> usize {
    values.partition_point(|&v| v < target)
}

fn selection_bg_for(base_bg: Option<Color>) -> Color {
    let fallback = rgb(32, 38, 48);
    blend_color(base_bg.unwrap_or(fallback), accent_color(), 0.34)
}

fn selection_fg_for(base_fg: Option<Color>) -> Option<Color> {
    base_fg.map(|fg| blend_color(fg, Color::White, 0.15))
}

fn highlight_line_selection(
    line: &Line<'static>,
    start_col: usize,
    end_col: usize,
) -> Line<'static> {
    if end_col <= start_col {
        return line.clone();
    }

    let mut rebuilt: Vec<Span<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_style: Option<Style> = None;
    let mut col = 0usize;

    let flush = |rebuilt: &mut Vec<Span<'static>>, text: &mut String, style: &mut Option<Style>| {
        if !text.is_empty() {
            let span = match style.take() {
                Some(style) => Span::styled(std::mem::take(text), style),
                None => Span::raw(std::mem::take(text)),
            };
            rebuilt.push(span);
        }
    };

    for span in &line.spans {
        for ch in span.content.chars() {
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            let selected = if width == 0 {
                col > start_col && col <= end_col
            } else {
                col < end_col && col.saturating_add(width) > start_col
            };

            let mut style = span.style;
            if selected {
                style = style.bg(selection_bg_for(style.bg));
                if let Some(fg) = selection_fg_for(style.fg) {
                    style = style.fg(fg);
                }
            }

            if current_style == Some(style) {
                current_text.push(ch);
            } else {
                flush(&mut rebuilt, &mut current_text, &mut current_style);
                current_text.push(ch);
                current_style = Some(style);
            }

            col = col.saturating_add(width);
        }
    }

    flush(&mut rebuilt, &mut current_text, &mut current_style);

    Line {
        spans: rebuilt,
        style: line.style,
        alignment: line.alignment,
    }
}

pub(super) fn compute_visible_margins(
    lines: &[Line],
    visible_user_indices: &[usize],
    area: Rect,
    centered: bool,
) -> info_widget::Margins {
    let visible_height = area.height as usize;
    let mut visible_user_cursor = 0usize;

    let mut right_widths = Vec::with_capacity(visible_height);
    let mut left_widths = Vec::with_capacity(visible_height);

    for row in 0..visible_height {
        while visible_user_cursor < visible_user_indices.len()
            && visible_user_indices[visible_user_cursor] < row
        {
            visible_user_cursor += 1;
        }
        let is_user_line = visible_user_cursor < visible_user_indices.len()
            && visible_user_indices[visible_user_cursor] == row;

        if row < lines.len() {
            let mut used = lines[row].width().min(area.width as usize) as u16;
            if is_user_line && area.width > 0 {
                used = used.saturating_add(1).min(area.width);
            }

            if centered {
                let total_margin = area.width.saturating_sub(used);
                let effective_alignment = lines[row].alignment.unwrap_or(Alignment::Center);
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
    prepared: Arc<PreparedChatFrame>,
    show_native_scrollbar: bool,
) -> info_widget::Margins {
    let (render_area, scrollbar_area) =
        super::split_native_scrollbar_area(area, show_native_scrollbar);
    let left_inset = super::left_aligned_content_inset(render_area.width, app.centered_mode());
    let text_render_area = Rect {
        x: render_area.x.saturating_add(left_inset),
        y: render_area.y,
        width: render_area.width.saturating_sub(left_inset),
        height: render_area.height,
    };
    let wrapped_user_indices = &prepared.wrapped_user_indices;
    let wrapped_user_prompt_starts = &prepared.wrapped_user_prompt_starts;
    let wrapped_user_prompt_ends = &prepared.wrapped_user_prompt_ends;
    let user_prompt_texts = &prepared.user_prompt_texts;

    let total_lines = prepared.total_wrapped_lines();
    let viewport_height = render_area.height as usize;
    let max_scroll = compute_max_scroll_with_prompt_preview(
        total_lines,
        wrapped_user_prompt_starts,
        user_prompt_texts,
        text_render_area,
    );

    super::set_last_max_scroll(max_scroll);
    update_user_prompt_positions(wrapped_user_prompt_starts);

    let user_scroll = app.scroll_offset().min(max_scroll);
    let scroll = if app.auto_scroll_paused() {
        user_scroll.min(max_scroll)
    } else {
        max_scroll
    };

    let prompt_preview_lines = if crate::config::config().display.prompt_preview && scroll > 0 {
        compute_prompt_preview_line_count(
            wrapped_user_prompt_starts,
            user_prompt_texts,
            scroll,
            text_render_area.width,
        )
    } else {
        0u16
    };

    let content_area = Rect {
        x: text_render_area.x,
        y: render_area.y.saturating_add(prompt_preview_lines),
        width: text_render_area.width,
        height: render_area.height.saturating_sub(prompt_preview_lines),
    };
    let visible_height = content_area.height as usize;

    let active_file_context = if app.diff_mode().is_file() {
        active_file_diff_context(prepared.as_ref(), scroll, visible_height)
    } else {
        None
    };

    let visible_end = (scroll + visible_height).min(total_lines);
    let visible_user_start = lower_bound(wrapped_user_indices, scroll);
    let visible_user_end = lower_bound(wrapped_user_indices, visible_end);
    let visible_user_indices: Vec<usize> = wrapped_user_indices
        [visible_user_start..visible_user_end]
        .iter()
        .map(|idx| idx.saturating_sub(scroll))
        .collect();

    let mut visible_lines = prepared.materialize_line_slice(scroll, visible_end);
    let stability_hash = super::viewport_stability_hash(
        &visible_lines,
        &visible_user_indices,
        content_area.width,
        prompt_preview_lines,
    );
    let visible_streaming_hash =
        if prepared.visible_intersects_section(PreparedSectionKind::Streaming, scroll, visible_end)
        {
            super::hash_text_for_cache(app.streaming_text())
        } else {
            0
        };
    let visible_batch_progress_hash = if prepared.visible_intersects_section(
        PreparedSectionKind::BatchProgress,
        scroll,
        visible_end,
    ) {
        super::prepare::active_batch_progress_hash(app)
    } else {
        0
    };
    let content_margins = compute_visible_margins(
        &visible_lines,
        &visible_user_indices,
        content_area,
        app.centered_mode(),
    );
    let mut margins = info_widget::Margins {
        right_widths: vec![0; prompt_preview_lines as usize],
        left_widths: vec![0; prompt_preview_lines as usize],
        centered: content_margins.centered,
    };
    margins
        .right_widths
        .extend(content_margins.right_widths.clone());
    margins
        .left_widths
        .extend(content_margins.left_widths.clone());
    while margins.right_widths.len() < viewport_height {
        margins.right_widths.push(0);
    }
    while margins.left_widths.len() < viewport_height {
        margins.left_widths.push(0);
    }

    let copy_badge_ui = app.copy_badge_ui();
    let copy_badge_now = std::time::Instant::now();

    record_copy_viewport_frame_snapshot(
        prepared.clone(),
        scroll,
        visible_end,
        content_area,
        &content_margins.left_widths,
    );

    let mut visible_copy_targets: Vec<VisibleCopyTarget> = Vec::new();
    let mut badge_assignments: Vec<(usize, char)> = Vec::new();
    for (slot, target) in prepared
        .copy_targets
        .iter()
        .filter(|target| target.end_line > scroll && target.start_line < visible_end)
        .take(COPY_BADGE_KEYS.len())
        .enumerate()
    {
        let key = COPY_BADGE_KEYS[slot];
        visible_copy_targets.push(VisibleCopyTarget {
            key,
            kind_label: target.kind.label(),
            copied_notice: target.kind.copied_notice(),
            content: target.content.clone(),
        });
        badge_assignments.push((target.badge_line, key));
    }
    set_visible_copy_targets(visible_copy_targets);
    super::note_viewport_metrics(super::ViewportMetrics {
        scroll,
        visible_end,
        visible_lines: visible_lines.len(),
        total_wrapped_lines: total_lines,
        prompt_preview_lines,
        visible_user_prompts: visible_user_indices.len(),
        visible_copy_targets: badge_assignments.len(),
        content_width: content_area.width,
        stability_hash,
        visible_streaming_hash,
        visible_batch_progress_hash,
    });

    let now_ms = app.now_millis();
    let policy = crate::perf::tui_policy();
    let prompt_anim_enabled = crate::config::config().display.prompt_entry_animation
        && policy.enable_decorative_animations
        && policy.tier.prompt_entry_animation_enabled();
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

    if visible_lines.len() < visible_height {
        visible_lines.extend(std::iter::repeat_n(
            Line::from(""),
            visible_height - visible_lines.len(),
        ));
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
                    let line_width = line.width().max(1) as f32;
                    let mut consumed = 0usize;
                    for span in &mut line.spans {
                        if !span.content.is_empty() {
                            let base_fg = match span.style.fg {
                                Some(c) => c,
                                None => user_text(),
                            };
                            let base_bg = span.style.bg.unwrap_or(user_bg());
                            let span_width = span.content.as_ref().width();
                            let span_center = if span_width == 0 {
                                consumed as f32 / line_width
                            } else {
                                (consumed as f32 + span_width as f32 * 0.5) / line_width
                            }
                            .clamp(0.0, 1.0);

                            let pulsed_fg = prompt_entry_color(base_fg, t);
                            let shimmer_fg = prompt_entry_shimmer_color(pulsed_fg, span_center, t);
                            let spotlight_bg = prompt_entry_bg_color(base_bg, t);

                            span.style = span.style.fg(shimmer_fg).bg(spotlight_bg);
                            consumed += span_width;
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

    for (badge_line, key) in badge_assignments {
        if badge_line < scroll || badge_line >= visible_end {
            continue;
        }
        let rel_idx = badge_line - scroll;
        if let Some(line) = visible_lines.get_mut(rel_idx) {
            let alt_style = if copy_badge_ui.alt_is_active(copy_badge_now) {
                Style::default().fg(queued_color()).bold()
            } else {
                Style::default().fg(dim_color())
            };
            let shift_style = if copy_badge_ui.shift_is_active(copy_badge_now) {
                Style::default().fg(queued_color()).bold()
            } else {
                Style::default().fg(dim_color())
            };
            let key_style = if copy_badge_ui.key_is_active(key, copy_badge_now) {
                Style::default().fg(accent_color()).bold()
            } else {
                Style::default().fg(dim_color())
            };

            if let Some(success) = copy_badge_ui.feedback_for_key(key, copy_badge_now) {
                let feedback_style = if success {
                    Style::default().fg(ai_color()).bold()
                } else {
                    Style::default().fg(Color::Red).bold()
                };
                let feedback_text = if success {
                    " ✓ Copied!"
                } else {
                    " ✗ Copy failed"
                };
                line.spans.push(Span::styled(feedback_text, feedback_style));
                line.spans.push(Span::raw(" "));
            }

            line.spans.push(Span::styled("[Alt]", alt_style));
            line.spans.push(Span::raw(" "));
            line.spans.push(Span::styled("[⇧]", shift_style));
            line.spans.push(Span::raw(" "));
            line.spans.push(Span::styled(
                format!("[{}]", key.to_ascii_uppercase()),
                key_style,
            ));
        }
    }

    if let Some(range) = app.copy_selection_range().filter(|range| {
        range.start.pane == crate::tui::CopySelectionPane::Chat
            && range.end.pane == crate::tui::CopySelectionPane::Chat
    }) {
        let (start, end) = if (range.start.abs_line, range.start.column)
            <= (range.end.abs_line, range.end.column)
        {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };

        for abs_idx in start.abs_line.max(scroll)..=end.abs_line.min(visible_end.saturating_sub(1))
        {
            let rel_idx = abs_idx.saturating_sub(scroll);
            if let Some(line) = visible_lines.get_mut(rel_idx) {
                let copy_start = prepared.wrapped_copy_offset(abs_idx).unwrap_or(0);
                let start_col = if abs_idx == start.abs_line {
                    start.column.max(copy_start)
                } else {
                    copy_start
                };
                let end_col = if abs_idx == end.abs_line {
                    end.column.max(copy_start)
                } else {
                    copy_viewport_line_text(abs_idx)
                        .map(|text| UnicodeWidthStr::width(text.as_str()))
                        .unwrap_or_else(|| line.width())
                };
                *line = highlight_line_selection(line, start_col, end_col);
            }
        }
    }

    frame.render_widget(Paragraph::new(visible_lines), content_area);

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
                    let available_height = content_area.height.saturating_sub(screen_y);
                    let render_height = total_height.min(available_height);

                    if render_height > 0 {
                        let image_area = Rect {
                            x: content_area.x,
                            y: content_area.y + screen_y,
                            width: content_area.width,
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
                            x: content_area.x,
                            y: content_area.y + screen_y,
                            width: content_area.width,
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

    let right_x = render_area.x + render_area.width.saturating_sub(1);
    for &line_idx in &wrapped_user_indices[visible_user_start..visible_user_end] {
        if line_idx >= scroll && line_idx < scroll + visible_height {
            let screen_y = content_area.y + (line_idx - scroll) as u16;
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

    if !show_native_scrollbar && scroll > 0 {
        let indicator = format!("↑{}", scroll);
        let indicator_area = Rect {
            x: render_area.x + render_area.width.saturating_sub(indicator.len() as u16 + 2),
            y: render_area.y,
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

        if let Some(prompt_order) = last_offscreen_prompt_idx
            && let Some(prompt_text) = user_prompt_texts.get(prompt_order)
        {
            let prompt_text = prompt_text.trim();
            if !prompt_text.is_empty() {
                let prompt_num = prompt_order + 1;
                let num_str = format!("{}", prompt_num);
                let prefix_len = num_str.len() + 2;
                let content_width =
                    render_area.width.saturating_sub(prefix_len as u16 + 2) as usize;
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
                    vec![
                        Line::from(vec![
                            Span::styled(num_str.clone(), dim_style.fg(dim_color()).bg(user_bg())),
                            Span::styled("› ", dim_style.fg(user_color()).bg(user_bg())),
                            Span::styled(text_flat, dim_style.fg(user_text()).bg(user_bg())),
                        ])
                        .alignment(align),
                    ]
                } else {
                    let half = content_width.max(4);
                    let head: String = text_chars[..half.min(text_chars.len())].iter().collect();
                    let tail_start = text_chars.len().saturating_sub(half);
                    let tail: String = text_chars[tail_start..].iter().collect();

                    let first = Line::from(vec![
                        Span::styled(num_str.clone(), dim_style.fg(dim_color()).bg(user_bg())),
                        Span::styled("› ", dim_style.fg(user_color()).bg(user_bg())),
                        Span::styled(
                            format!("{} ...", head.trim_end()),
                            dim_style.fg(user_text()).bg(user_bg()),
                        ),
                    ])
                    .alignment(align);

                    let padding: String = " ".repeat(prefix_len);
                    let second = Line::from(vec![
                        Span::styled(padding, dim_style.bg(user_bg())),
                        Span::styled(
                            format!("... {}", tail.trim_start()),
                            dim_style.fg(user_text()).bg(user_bg()),
                        ),
                    ])
                    .alignment(align);

                    vec![first, second]
                };

                let line_count = preview_lines.len() as u16;
                let preview_area = Rect {
                    x: content_area.x,
                    y: render_area.y,
                    width: content_area.width.saturating_sub(1),
                    height: line_count,
                };
                clear_area(frame, preview_area);
                frame.render_widget(Paragraph::new(preview_lines), preview_area);
            }
        }
    }

    if !show_native_scrollbar && app.auto_scroll_paused() && scroll < max_scroll {
        let indicator = format!("↓{}", max_scroll - scroll);
        let indicator_area = Rect {
            x: render_area.x + render_area.width.saturating_sub(indicator.len() as u16 + 2),
            y: render_area.y + render_area.height.saturating_sub(1),
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

    if let Some(scrollbar_area) = scrollbar_area {
        super::render_native_scrollbar(
            frame,
            scrollbar_area,
            scroll,
            total_lines,
            visible_height,
            false,
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
    let display_width = UnicodeWidthStr::width(text_flat.as_str());
    if display_width > content_width { 2 } else { 1 }
}

fn compute_max_scroll_with_prompt_preview(
    total_lines: usize,
    wrapped_user_prompt_starts: &[usize],
    user_prompt_texts: &[String],
    area: Rect,
) -> usize {
    let mut max_scroll = total_lines.saturating_sub(area.height as usize);
    if max_scroll == 0 || !crate::config::config().display.prompt_preview {
        return max_scroll;
    }

    for _ in 0..4 {
        let prompt_preview_lines = compute_prompt_preview_line_count(
            wrapped_user_prompt_starts,
            user_prompt_texts,
            max_scroll,
            area.width,
        );
        let content_height = area.height.saturating_sub(prompt_preview_lines) as usize;
        let adjusted = total_lines.saturating_sub(content_height);
        if adjusted == max_scroll {
            break;
        }
        max_scroll = adjusted;
    }

    max_scroll
}
