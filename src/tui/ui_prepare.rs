use super::*;

fn user_prompt_number_style(color: Color) -> Style {
    Style::default().fg(color).bg(user_bg())
}

fn user_prompt_accent_style() -> Style {
    Style::default().fg(user_color()).bg(user_bg())
}

fn user_prompt_text_style() -> Style {
    Style::default().fg(user_text()).bg(user_bg())
}

fn assistant_message_copy_targets(
    content: &str,
    rendered_lines: &[Line<'static>],
) -> Vec<RawCopyTarget> {
    if content.starts_with("Error:")
        || content.starts_with("error:")
        || content.starts_with("Failed:")
    {
        return vec![RawCopyTarget {
            kind: CopyTargetKind::Error,
            content: content.trim_end().to_string(),
            start_raw_line: 0,
            end_raw_line: rendered_lines.len().max(1),
            badge_raw_line: 0,
        }];
    }

    crate::tui::markdown::extract_copy_targets_from_rendered_lines(rendered_lines)
}

pub(super) fn prepare_messages(
    app: &dyn TuiState,
    width: u16,
    height: u16,
) -> Arc<PreparedMessages> {
    let startup_active = super::super::startup_animation_active(app);

    let key = FullPrepCacheKey {
        width,
        height,
        diff_mode: app.diff_mode(),
        messages_version: app.display_messages_version(),
        diagram_mode: app.diagram_mode(),
        centered: app.centered_mode(),
        is_processing: app.is_processing(),
        streaming_text_len: app.streaming_text().len(),
        startup_active,
    };

    {
        let cache = match full_prep_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => {
                let mut c = poisoned.into_inner();
                c.key = None;
                c.prepared = None;
                c
            }
        };
        if cache.key.as_ref() == Some(&key) {
            if let Some(prepared) = cache.prepared.clone() {
                return prepared;
            }
        }
    }

    let prepared = Arc::new(prepare_messages_inner(app, width, height, startup_active));

    {
        if let Ok(mut cache) = full_prep_cache().lock() {
            cache.key = Some(key);
            cache.prepared = Some(prepared.clone());
        }
    }

    prepared
}

fn prepare_messages_inner(
    app: &dyn TuiState,
    width: u16,
    height: u16,
    startup_active: bool,
) -> PreparedMessages {
    let mut all_header_lines = header::build_persistent_header(app, width);
    all_header_lines.extend(build_header_lines(app, width));
    let header_prepared = wrap_lines(all_header_lines, &[], &[], width);
    let startup_prepared = if startup_active {
        wrap_lines(
            animations::build_startup_animation_lines(app, width),
            &[],
            &[],
            width,
        )
    } else {
        PreparedMessages {
            wrapped_lines: Vec::new(),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
        }
    };

    let body_prepared = prepare_body_cached(app, width);
    let has_streaming = app.is_processing() && !app.streaming_text().is_empty();
    let stream_prefix_blank = has_streaming && !body_prepared.wrapped_lines.is_empty();
    let streaming_prepared = if has_streaming {
        prepare_streaming_cached(app, width, stream_prefix_blank)
    } else {
        PreparedMessages {
            wrapped_lines: Vec::new(),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
        }
    };

    let mut wrapped_lines: Vec<Line<'static>>;
    let wrapped_user_indices;
    let wrapped_user_prompt_starts;
    let wrapped_user_prompt_ends;
    let user_prompt_texts;
    let mut image_regions;
    let edit_tool_ranges;
    let copy_targets;

    if startup_active {
        let elapsed = app.animation_elapsed();
        let anim_duration = super::super::STARTUP_ANIMATION_WINDOW.as_secs_f32();
        let morph_t = (elapsed / anim_duration).clamp(0.0, 1.0);

        let anim_lines = &startup_prepared.wrapped_lines;
        let header_lines = &header_prepared.wrapped_lines;

        let content_lines: Vec<Line<'static>> = if morph_t < 0.6 {
            anim_lines.clone()
        } else {
            morph_lines_to_header(anim_lines, header_lines, morph_t, width)
        };

        let content_height = content_lines.len();
        let input_reserve = 4;
        let available = (height as usize).saturating_sub(input_reserve);
        let centered_pad = available.saturating_sub(content_height) / 2;
        let header_height = header_prepared.wrapped_lines.len();
        let header_pad = available.saturating_sub(header_height) / 2;

        let slide_t = if morph_t > 0.85 {
            ((morph_t - 0.85) / 0.15).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let slide_ease = slide_t * slide_t * (3.0 - 2.0 * slide_t);
        let pad_top =
            (centered_pad as f32 + (header_pad as f32 - centered_pad as f32) * slide_ease) as usize;

        wrapped_lines = Vec::with_capacity(pad_top + content_height);
        for _ in 0..pad_top {
            wrapped_lines.push(Line::from(""));
        }
        wrapped_lines.extend(content_lines);
        wrapped_user_indices = Vec::new();
        wrapped_user_prompt_starts = Vec::new();
        wrapped_user_prompt_ends = Vec::new();
        user_prompt_texts = Vec::new();
        image_regions = Vec::new();
        edit_tool_ranges = Vec::new();
        copy_targets = Vec::new();
    } else {
        let is_initial_empty = app.display_messages().is_empty()
            && !app.is_processing()
            && app.streaming_text().is_empty();

        wrapped_lines = header_prepared.wrapped_lines;

        if is_initial_empty {
            let suggestions = app.suggestion_prompts();
            let is_centered = app.centered_mode();
            let suggestion_align = if is_centered {
                ratatui::layout::Alignment::Center
            } else {
                ratatui::layout::Alignment::Left
            };
            if !suggestions.is_empty() {
                wrapped_lines.push(Line::from(""));
                for (i, (label, prompt)) in suggestions.iter().enumerate() {
                    let is_login = prompt.starts_with('/');
                    let pad = if is_centered { "" } else { "  " };
                    let spans = if is_login {
                        vec![
                            Span::styled(
                                format!("{}{} ", pad, label),
                                Style::default()
                                    .fg(rgb(138, 180, 248))
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("(type {})", prompt),
                                Style::default().fg(dim_color()),
                            ),
                        ]
                    } else {
                        vec![
                            Span::styled(
                                format!("{}[{}] ", pad, i + 1),
                                Style::default().fg(rgb(138, 180, 248)),
                            ),
                            Span::styled(label.clone(), Style::default().fg(rgb(200, 200, 200))),
                        ]
                    };
                    wrapped_lines.push(Line::from(spans).alignment(suggestion_align));
                }
                if suggestions.len() > 1 {
                    wrapped_lines.push(Line::from(""));
                    wrapped_lines.push(
                        Line::from(Span::styled(
                            if is_centered {
                                "Press 1-3 or type anything to start"
                            } else {
                                "  Press 1-3 or type anything to start"
                            },
                            Style::default().fg(dim_color()),
                        ))
                        .alignment(suggestion_align),
                    );
                }
            }

            let content_height = wrapped_lines.len();
            let input_reserve = 4;
            let available = (height as usize).saturating_sub(input_reserve);
            let pad_top = available.saturating_sub(content_height) / 2;
            let mut centered = Vec::with_capacity(pad_top + content_height);
            for _ in 0..pad_top {
                centered.push(Line::from(""));
            }
            centered.extend(wrapped_lines);
            wrapped_lines = centered;
        }

        let header_len = wrapped_lines.len();
        let startup_len = startup_prepared.wrapped_lines.len();
        wrapped_lines.extend(startup_prepared.wrapped_lines);
        let body_offset = header_len + startup_len;
        let body_len = body_prepared.wrapped_lines.len();
        wrapped_lines.extend_from_slice(&body_prepared.wrapped_lines);
        wrapped_lines.extend(streaming_prepared.wrapped_lines);

        wrapped_user_indices = body_prepared
            .wrapped_user_indices
            .iter()
            .map(|idx| idx + body_offset)
            .collect();

        wrapped_user_prompt_starts = body_prepared
            .wrapped_user_prompt_starts
            .iter()
            .map(|idx| idx + body_offset)
            .collect();

        wrapped_user_prompt_ends = body_prepared
            .wrapped_user_prompt_ends
            .iter()
            .map(|idx| idx + body_offset)
            .collect();

        user_prompt_texts = body_prepared.user_prompt_texts.clone();

        image_regions = Vec::with_capacity(
            body_prepared.image_regions.len() + streaming_prepared.image_regions.len(),
        );
        for region in &body_prepared.image_regions {
            image_regions.push(ImageRegion {
                abs_line_idx: region.abs_line_idx + body_offset,
                end_line: region.end_line + body_offset,
                ..*region
            });
        }
        for mut region in streaming_prepared.image_regions {
            region.abs_line_idx += body_offset + body_len;
            region.end_line += body_offset + body_len;
            image_regions.push(region);
        }

        edit_tool_ranges = body_prepared
            .edit_tool_ranges
            .iter()
            .map(|r| EditToolRange {
                edit_index: r.edit_index,
                msg_index: r.msg_index,
                file_path: r.file_path.clone(),
                start_line: r.start_line + body_offset,
                end_line: r.end_line + body_offset,
            })
            .collect();

        copy_targets = body_prepared
            .copy_targets
            .iter()
            .map(|target| CopyTarget {
                kind: target.kind.clone(),
                content: target.content.clone(),
                start_line: target.start_line + body_offset,
                end_line: target.end_line + body_offset,
                badge_line: target.badge_line + body_offset,
            })
            .collect();
    }

    PreparedMessages {
        wrapped_lines,
        wrapped_user_indices,
        wrapped_user_prompt_starts,
        wrapped_user_prompt_ends,
        user_prompt_texts,
        image_regions,
        edit_tool_ranges,
        copy_targets,
    }
}

fn prepare_body_cached(app: &dyn TuiState, width: u16) -> Arc<PreparedMessages> {
    let key = BodyCacheKey {
        width,
        diff_mode: app.diff_mode(),
        messages_version: app.display_messages_version(),
        diagram_mode: app.diagram_mode(),
        centered: app.centered_mode(),
    };
    let msg_count = app.display_messages().len();

    let cache = match body_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => {
            let mut c = poisoned.into_inner();
            c.entries.clear();
            c
        }
    };

    let mut cache = cache;
    if let Some(prepared) = cache.get_exact(&key) {
        return prepared;
    }

    let incremental_base = cache.best_incremental_base(&key, msg_count);

    drop(cache);

    let prepared = if let Some((prev, prev_count)) = incremental_base {
        prepare_body_incremental(app, width, &prev, prev_count)
    } else {
        Arc::new(prepare_body(app, width, false))
    };

    let mut cache = match body_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    cache.insert(key, prepared.clone(), msg_count);
    prepared
}

fn prepare_body_incremental(
    app: &dyn TuiState,
    width: u16,
    prev: &PreparedMessages,
    prev_msg_count: usize,
) -> Arc<PreparedMessages> {
    let messages = app.display_messages();
    let new_messages = &messages[prev_msg_count..];
    if new_messages.is_empty() {
        return Arc::new(prev.clone());
    }

    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let total_prompts = messages.iter().filter(|m| m.role == "user").count();
    let pending_count = input_ui::pending_prompt_count(app);

    let mut prompt_num = messages[..prev_msg_count]
        .iter()
        .filter(|m| m.role == "user")
        .count();

    let mut new_lines: Vec<Line> = Vec::new();
    let mut new_user_line_indices: Vec<usize> = Vec::new();
    let mut new_user_prompt_texts: Vec<String> = Vec::new();
    let mut new_edit_tool_line_ranges: Vec<(usize, String, usize, usize)> = Vec::new();
    let mut new_copy_targets: Vec<RawCopyTarget> = Vec::new();

    let body_has_content = !prev.wrapped_lines.is_empty();

    for (new_msg_offset, msg) in new_messages.iter().enumerate() {
        if (body_has_content || !new_lines.is_empty()) && msg.role != "tool" && msg.role != "meta" {
            new_lines.push(Line::from(""));
        }

        match msg.role.as_str() {
            "user" => {
                prompt_num += 1;
                new_user_line_indices.push(new_lines.len());
                new_user_prompt_texts.push(msg.content.clone());
                let distance = total_prompts + pending_count + 1 - prompt_num;
                let num_color = rainbow_prompt_color(distance);
                new_lines.push(
                    Line::from(vec![
                        Span::styled(format!("{}", prompt_num), Style::default().fg(num_color)),
                        Span::styled("› ", Style::default().fg(user_color())),
                        Span::styled(msg.content.clone(), Style::default().fg(user_text())),
                    ])
                    .alignment(align),
                );
            }
            "assistant" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_assistant_message,
                );
                let cached_copy_targets = assistant_message_copy_targets(&msg.content, &cached);
                for target in cached_copy_targets {
                    new_copy_targets.push(RawCopyTarget {
                        kind: target.kind,
                        content: target.content,
                        start_raw_line: new_lines.len() + target.start_raw_line,
                        end_raw_line: new_lines.len() + target.end_raw_line,
                        badge_raw_line: new_lines.len() + target.badge_raw_line,
                    });
                }
                for line in cached {
                    new_lines.push(align_if_unset(line, align));
                }
            }
            "meta" => {
                new_lines.push(
                    Line::from(vec![
                        Span::raw(if centered { "" } else { "  " }),
                        Span::styled(msg.content.clone(), Style::default().fg(dim_color())),
                    ])
                    .alignment(align),
                );
            }
            "tool" => {
                let tool_start_line = new_lines.len();
                let cached =
                    get_cached_message_lines(msg, width, app.diff_mode(), render_tool_message);
                for line in cached {
                    new_lines.push(align_if_unset(line, align));
                }
                if let Some(ref tc) = msg.tool_data {
                    let is_edit_tool = matches!(
                        tc.name.as_str(),
                        "edit"
                            | "Edit"
                            | "write"
                            | "multiedit"
                            | "patch"
                            | "Patch"
                            | "apply_patch"
                            | "ApplyPatch"
                    );
                    if is_edit_tool {
                        let file_path = tc
                            .input
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .or_else(|| {
                                tc.input
                                    .get("patch_text")
                                    .and_then(|v| v.as_str())
                                    .and_then(|patch_text| match tc.name.as_str() {
                                        "apply_patch" | "ApplyPatch" => {
                                            tools_ui::extract_apply_patch_primary_file(patch_text)
                                        }
                                        "patch" | "Patch" => {
                                            tools_ui::extract_unified_patch_primary_file(patch_text)
                                        }
                                        _ => None,
                                    })
                            })
                            .unwrap_or_else(|| "unknown".to_string());
                        new_edit_tool_line_ranges.push((
                            prev_msg_count + new_msg_offset,
                            file_path,
                            tool_start_line,
                            new_lines.len(),
                        ));
                    }
                }
            }
            "system" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_system_message,
                );
                for line in cached {
                    new_lines.push(align_if_unset(line, align));
                }
            }
            "swarm" => {
                let title = msg.title.clone().unwrap_or_else(|| "Swarm".to_string());
                let border_style = Style::default().fg(rgb(170, 110, 255));
                let text_style = Style::default().fg(rgb(230, 220, 255));
                let total_width = if centered {
                    (width.saturating_sub(4) as usize).min(120)
                } else {
                    width.saturating_sub(2) as usize
                };
                let content_width = total_width.saturating_sub(4).max(1);
                let mut box_content =
                    markdown::render_markdown_with_width(&msg.content, Some(content_width));
                if box_content.is_empty() {
                    box_content.push(Line::from(Span::styled(msg.content.clone(), text_style)));
                } else {
                    for line in &mut box_content {
                        for span in &mut line.spans {
                            if span.style.fg.is_none() {
                                span.style.fg = text_style.fg;
                            }
                        }
                    }
                }
                let title = format!("🟣 {}", title);
                let box_lines = render_rounded_box(&title, box_content, total_width, border_style);
                for line in box_lines {
                    new_lines.push(align_if_unset(line, align));
                }
            }
            "memory" => {
                let border_style = Style::default().fg(rgb(130, 140, 180));
                let text_style = Style::default().fg(dim_color());

                let mut entries: Vec<(String, String)> = Vec::new();
                let mut current_category = String::new();

                for text_line in msg.content.lines() {
                    if text_line.starts_with("# ") {
                        continue;
                    }
                    if text_line.starts_with("## ") {
                        current_category = text_line.trim_start_matches("## ").to_string();
                        continue;
                    }
                    if text_line.trim().is_empty() {
                        continue;
                    }
                    let content = if let Some(dot_pos) = text_line.find(". ") {
                        let prefix = &text_line[..dot_pos];
                        if prefix.trim().chars().all(|c| c.is_ascii_digit()) {
                            text_line[dot_pos + 2..].trim()
                        } else {
                            text_line.trim()
                        }
                    } else {
                        text_line.trim()
                    };
                    let cat = if current_category.is_empty() {
                        "memory".to_string()
                    } else {
                        current_category.clone()
                    };
                    entries.push((cat, content.to_string()));
                }

                let count = entries.len();
                let tiles = group_into_tiles(entries);

                let header_text = if let Some(title) = &msg.title {
                    title.clone()
                } else if count == 1 {
                    "🧠 1 memory".to_string()
                } else {
                    format!("🧠 {} memories", count)
                };
                let header = Line::from(Span::styled(header_text, border_style)).alignment(align);

                let total_width = if centered {
                    (width.saturating_sub(4) as usize).min(120)
                } else {
                    width.saturating_sub(2) as usize
                };
                let tile_lines = render_memory_tiles(
                    &tiles,
                    total_width,
                    border_style,
                    text_style,
                    Some(header),
                );
                for line in tile_lines {
                    new_lines.push(align_if_unset(line, align));
                }
            }
            "usage" => {
                new_lines.push(
                    Line::from(vec![
                        Span::styled(if centered { "" } else { "  " }, Style::default()),
                        Span::styled(msg.content.clone(), Style::default().fg(dim_color())),
                    ])
                    .alignment(align),
                );
            }
            "error" => {
                new_lines.push(
                    Line::from(vec![
                        Span::styled(
                            if centered { "✗ " } else { "  ✗ " },
                            Style::default().fg(Color::Red),
                        ),
                        Span::styled(msg.content.clone(), Style::default().fg(Color::Red)),
                    ])
                    .alignment(align),
                );
            }
            _ => {}
        }
    }

    let new_wrapped = wrap_lines_with_map(
        new_lines,
        &new_user_line_indices,
        &new_user_prompt_texts,
        width,
        &new_edit_tool_line_ranges,
        &new_copy_targets,
    );

    let prev_len = prev.wrapped_lines.len();
    let mut wrapped_lines = Vec::with_capacity(prev_len + new_wrapped.wrapped_lines.len());
    wrapped_lines.extend_from_slice(&prev.wrapped_lines);
    wrapped_lines.extend(new_wrapped.wrapped_lines);

    let mut wrapped_user_indices = prev.wrapped_user_indices.clone();
    for idx in new_wrapped.wrapped_user_indices {
        wrapped_user_indices.push(idx + prev_len);
    }

    let mut wrapped_user_prompt_starts = prev.wrapped_user_prompt_starts.clone();
    for idx in new_wrapped.wrapped_user_prompt_starts {
        wrapped_user_prompt_starts.push(idx + prev_len);
    }

    let mut wrapped_user_prompt_ends = prev.wrapped_user_prompt_ends.clone();
    for idx in new_wrapped.wrapped_user_prompt_ends {
        wrapped_user_prompt_ends.push(idx + prev_len);
    }

    let mut user_prompt_texts = prev.user_prompt_texts.clone();
    user_prompt_texts.extend(new_user_prompt_texts);

    let mut image_regions = prev.image_regions.clone();
    for region in new_wrapped.image_regions {
        image_regions.push(ImageRegion {
            abs_line_idx: region.abs_line_idx + prev_len,
            end_line: region.end_line + prev_len,
            ..region
        });
    }

    let mut edit_tool_ranges = prev.edit_tool_ranges.clone();
    for r in new_wrapped.edit_tool_ranges {
        edit_tool_ranges.push(EditToolRange {
            edit_index: prev.edit_tool_ranges.len() + r.edit_index,
            msg_index: r.msg_index,
            file_path: r.file_path,
            start_line: r.start_line + prev_len,
            end_line: r.end_line + prev_len,
        });
    }

    let mut copy_targets = prev.copy_targets.clone();
    for target in new_wrapped.copy_targets {
        copy_targets.push(CopyTarget {
            start_line: target.start_line + prev_len,
            end_line: target.end_line + prev_len,
            badge_line: target.badge_line + prev_len,
            ..target
        });
    }

    Arc::new(PreparedMessages {
        wrapped_lines,
        wrapped_user_indices,
        wrapped_user_prompt_starts,
        wrapped_user_prompt_ends,
        user_prompt_texts,
        image_regions,
        edit_tool_ranges,
        copy_targets,
    })
}

fn prepare_streaming_cached(
    app: &dyn TuiState,
    width: u16,
    prefix_blank: bool,
) -> PreparedMessages {
    let streaming = app.streaming_text();
    if streaming.is_empty() {
        return PreparedMessages {
            wrapped_lines: Vec::new(),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
        };
    }

    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);

    let content_width = width.saturating_sub(4) as usize;
    let md_lines = app.render_streaming_markdown(content_width);
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    if prefix_blank {
        lines.push(Line::from(""));
    }
    for line in md_lines {
        lines.push(align_if_unset(line, align));
    }

    wrap_lines(lines, &[], &[], width)
}

fn prepare_body(app: &dyn TuiState, width: u16, include_streaming: bool) -> PreparedMessages {
    let mut lines: Vec<Line> = Vec::new();
    let mut user_line_indices: Vec<usize> = Vec::new();
    let mut user_prompt_texts: Vec<String> = Vec::new();
    let mut edit_tool_line_ranges: Vec<(usize, String, usize, usize)> = Vec::new();
    let mut copy_targets: Vec<RawCopyTarget> = Vec::new();
    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let mut prompt_num = 0usize;
    let total_prompts = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "user")
        .count();
    let pending_count = input_ui::pending_prompt_count(app);

    for (msg_idx, msg) in app.display_messages().iter().enumerate() {
        if !lines.is_empty() && msg.role != "tool" && msg.role != "meta" {
            lines.push(Line::from(""));
        }

        match msg.role.as_str() {
            "user" => {
                prompt_num += 1;
                user_line_indices.push(lines.len());
                user_prompt_texts.push(msg.content.clone());
                let distance = total_prompts + pending_count + 1 - prompt_num;
                let num_color = rainbow_prompt_color(distance);
                lines.push(
                    Line::from(vec![
                        Span::styled(
                            format!("{}", prompt_num),
                            user_prompt_number_style(num_color),
                        ),
                        Span::styled("› ", user_prompt_accent_style()),
                        Span::styled(msg.content.clone(), user_prompt_text_style()),
                    ])
                    .alignment(align),
                );
            }
            "assistant" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_assistant_message,
                );
                let message_copy_targets = assistant_message_copy_targets(&msg.content, &cached);
                for target in message_copy_targets {
                    copy_targets.push(RawCopyTarget {
                        kind: target.kind,
                        content: target.content,
                        start_raw_line: lines.len() + target.start_raw_line,
                        end_raw_line: lines.len() + target.end_raw_line,
                        badge_raw_line: lines.len() + target.badge_raw_line,
                    });
                }
                for line in cached {
                    lines.push(align_if_unset(line, align));
                }
            }
            "meta" => {
                lines.push(
                    Line::from(vec![
                        Span::raw(if centered { "" } else { "  " }),
                        Span::styled(msg.content.clone(), Style::default().fg(dim_color())),
                    ])
                    .alignment(align),
                );
            }
            "tool" => {
                let tool_start_line = lines.len();
                let cached =
                    get_cached_message_lines(msg, width, app.diff_mode(), render_tool_message);
                for line in cached {
                    lines.push(align_if_unset(line, align));
                }
                if let Some(ref tc) = msg.tool_data {
                    let is_edit_tool = matches!(
                        tc.name.as_str(),
                        "edit"
                            | "Edit"
                            | "write"
                            | "multiedit"
                            | "patch"
                            | "Patch"
                            | "apply_patch"
                            | "ApplyPatch"
                    );
                    if is_edit_tool {
                        let file_path = tc
                            .input
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .or_else(|| {
                                tc.input
                                    .get("patch_text")
                                    .and_then(|v| v.as_str())
                                    .and_then(|patch_text| match tc.name.as_str() {
                                        "apply_patch" | "ApplyPatch" => {
                                            tools_ui::extract_apply_patch_primary_file(patch_text)
                                        }
                                        "patch" | "Patch" => {
                                            tools_ui::extract_unified_patch_primary_file(patch_text)
                                        }
                                        _ => None,
                                    })
                            })
                            .unwrap_or_else(|| "unknown".to_string());
                        edit_tool_line_ranges.push((
                            msg_idx,
                            file_path,
                            tool_start_line,
                            lines.len(),
                        ));
                    }
                }
            }
            "system" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_system_message,
                );
                for line in cached {
                    lines.push(align_if_unset(line, align));
                }
            }
            "memory" => {
                let border_style = Style::default().fg(rgb(130, 140, 180));
                let text_style = Style::default().fg(dim_color());

                let mut entries: Vec<(String, String)> = Vec::new();
                let mut current_category = String::new();

                for text_line in msg.content.lines() {
                    if text_line.starts_with("# ") {
                        continue;
                    }
                    if text_line.starts_with("## ") {
                        current_category = text_line.trim_start_matches("## ").to_string();
                        continue;
                    }
                    if text_line.trim().is_empty() {
                        continue;
                    }
                    let content = if let Some(dot_pos) = text_line.find(". ") {
                        let prefix = &text_line[..dot_pos];
                        if prefix.trim().chars().all(|c| c.is_ascii_digit()) {
                            text_line[dot_pos + 2..].trim()
                        } else {
                            text_line.trim()
                        }
                    } else {
                        text_line.trim()
                    };

                    let cat = if current_category.is_empty() {
                        "memory".to_string()
                    } else {
                        current_category.clone()
                    };
                    entries.push((cat, content.to_string()));
                }

                let count = entries.len();
                let tiles = group_into_tiles(entries);

                let header_text = if let Some(title) = &msg.title {
                    title.clone()
                } else if count == 1 {
                    "🧠 1 memory".to_string()
                } else {
                    format!("🧠 {} memories", count)
                };
                let header = Line::from(Span::styled(header_text, border_style)).alignment(align);

                let total_width = if centered {
                    (width.saturating_sub(4) as usize).min(120)
                } else {
                    width.saturating_sub(2) as usize
                };
                let tile_lines = render_memory_tiles(
                    &tiles,
                    total_width,
                    border_style,
                    text_style,
                    Some(header),
                );
                for line in tile_lines {
                    lines.push(align_if_unset(line, align));
                }
            }
            "usage" => {
                lines.push(
                    Line::from(vec![
                        Span::styled(if centered { "" } else { "  " }, Style::default()),
                        Span::styled(msg.content.clone(), Style::default().fg(dim_color())),
                    ])
                    .alignment(align),
                );
            }
            "error" => {
                lines.push(
                    Line::from(vec![
                        Span::styled(
                            if centered { "✗ " } else { "  ✗ " },
                            Style::default().fg(Color::Red),
                        ),
                        Span::styled(msg.content.clone(), Style::default().fg(Color::Red)),
                    ])
                    .alignment(align),
                );
            }
            _ => {}
        }
    }

    if include_streaming && app.is_processing() && !app.streaming_text().is_empty() {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        let content_width = width.saturating_sub(4) as usize;
        let md_lines = app.render_streaming_markdown(content_width);
        for line in md_lines {
            lines.push(align_if_unset(line, align));
        }
    }

    wrap_lines_with_map(
        lines,
        &user_line_indices,
        &user_prompt_texts,
        width,
        &edit_tool_line_ranges,
        &copy_targets,
    )
}

fn wrap_lines(
    lines: Vec<Line<'static>>,
    user_line_indices: &[usize],
    user_prompt_texts: &[String],
    width: u16,
) -> PreparedMessages {
    let full_width = width.saturating_sub(1) as usize;
    let user_width = width.saturating_sub(2) as usize;
    let mut wrapped_user_indices: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_starts: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_ends: Vec<usize> = Vec::new();
    let mut user_line_mask = vec![false; lines.len()];
    for &idx in user_line_indices {
        if idx < user_line_mask.len() {
            user_line_mask[idx] = true;
        }
    }
    let mut wrapped_idx = 0usize;

    let mut wrapped_lines: Vec<Line> = Vec::new();
    for (orig_idx, line) in lines.into_iter().enumerate() {
        let is_user_line = user_line_mask.get(orig_idx).copied().unwrap_or(false);
        let wrap_width = if is_user_line { user_width } else { full_width };
        let new_lines = markdown::wrap_line(line, wrap_width);
        let count = new_lines.len();

        if is_user_line {
            wrapped_user_prompt_starts.push(wrapped_idx);
            wrapped_user_prompt_ends.push(wrapped_idx + count);
            for i in 0..count {
                wrapped_user_indices.push(wrapped_idx + i);
            }
        }

        wrapped_lines.extend(new_lines);
        wrapped_idx += count;
    }

    let mut image_regions = Vec::new();
    for (idx, line) in wrapped_lines.iter().enumerate() {
        if let Some(hash) = super::super::mermaid::parse_image_placeholder(line) {
            let mut height = 1u16;
            for subsequent in wrapped_lines.iter().skip(idx + 1) {
                if subsequent.spans.is_empty()
                    || (subsequent.spans.len() == 1 && subsequent.spans[0].content.is_empty())
                {
                    height += 1;
                } else {
                    break;
                }
            }
            image_regions.push(ImageRegion {
                abs_line_idx: idx,
                end_line: idx + height as usize,
                hash,
                height,
            });
        }
    }

    PreparedMessages {
        wrapped_lines,
        wrapped_user_indices,
        wrapped_user_prompt_starts,
        wrapped_user_prompt_ends,
        user_prompt_texts: user_prompt_texts.to_vec(),
        image_regions,
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
    }
}

fn wrap_lines_with_map(
    lines: Vec<Line<'static>>,
    user_line_indices: &[usize],
    user_prompt_texts: &[String],
    width: u16,
    edit_ranges: &[(usize, String, usize, usize)],
    copy_ranges: &[RawCopyTarget],
) -> PreparedMessages {
    let full_width = width.saturating_sub(1) as usize;
    let user_width = width.saturating_sub(2) as usize;
    let mut wrapped_user_indices: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_starts: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_ends: Vec<usize> = Vec::new();
    let mut user_line_mask = vec![false; lines.len()];
    for &idx in user_line_indices {
        if idx < user_line_mask.len() {
            user_line_mask[idx] = true;
        }
    }
    let mut wrapped_idx = 0usize;

    let mut raw_to_wrapped: Vec<usize> = Vec::with_capacity(lines.len() + 1);

    let mut wrapped_lines: Vec<Line> = Vec::new();
    for (orig_idx, line) in lines.into_iter().enumerate() {
        raw_to_wrapped.push(wrapped_idx);
        let is_user_line = user_line_mask.get(orig_idx).copied().unwrap_or(false);
        let wrap_width = if is_user_line { user_width } else { full_width };
        let new_lines = markdown::wrap_line(line, wrap_width);
        let count = new_lines.len();

        if is_user_line {
            wrapped_user_prompt_starts.push(wrapped_idx);
            wrapped_user_prompt_ends.push(wrapped_idx + count);
            for i in 0..count {
                wrapped_user_indices.push(wrapped_idx + i);
            }
        }

        wrapped_lines.extend(new_lines);
        wrapped_idx += count;
    }
    raw_to_wrapped.push(wrapped_idx);

    let mut image_regions = Vec::new();
    for (idx, line) in wrapped_lines.iter().enumerate() {
        if let Some(hash) = super::super::mermaid::parse_image_placeholder(line) {
            let mut height = 1u16;
            for subsequent in wrapped_lines.iter().skip(idx + 1) {
                if subsequent.spans.is_empty()
                    || (subsequent.spans.len() == 1 && subsequent.spans[0].content.is_empty())
                {
                    height += 1;
                } else {
                    break;
                }
            }
            image_regions.push(ImageRegion {
                abs_line_idx: idx,
                end_line: idx + height as usize,
                hash,
                height,
            });
        }
    }

    let mut edit_tool_ranges = Vec::new();
    for (msg_idx, file_path, raw_start, raw_end) in edit_ranges {
        let start_line = raw_to_wrapped.get(*raw_start).copied().unwrap_or(0);
        let end_line = raw_to_wrapped
            .get(*raw_end)
            .copied()
            .unwrap_or(wrapped_lines.len());
        edit_tool_ranges.push(EditToolRange {
            edit_index: edit_tool_ranges.len(),
            msg_index: *msg_idx,
            file_path: file_path.clone(),
            start_line,
            end_line,
        });
    }

    let mut copy_targets = Vec::new();
    for target in copy_ranges {
        let start_line = raw_to_wrapped
            .get(target.start_raw_line)
            .copied()
            .unwrap_or(0);
        let end_line = raw_to_wrapped
            .get(target.end_raw_line)
            .copied()
            .unwrap_or(wrapped_lines.len());
        let badge_line = raw_to_wrapped
            .get(target.badge_raw_line)
            .copied()
            .unwrap_or(start_line);
        copy_targets.push(CopyTarget {
            kind: target.kind.clone(),
            content: target.content.clone(),
            start_line,
            end_line,
            badge_line,
        });
    }

    PreparedMessages {
        wrapped_lines,
        wrapped_user_indices,
        wrapped_user_prompt_starts,
        wrapped_user_prompt_ends,
        user_prompt_texts: user_prompt_texts.to_vec(),
        image_regions,
        edit_tool_ranges,
        copy_targets,
    }
}
