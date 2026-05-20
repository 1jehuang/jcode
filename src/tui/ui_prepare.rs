use super::*;
use crate::tui::ui::{self, WrappedLineMap};

fn content_prefers_display_as_logical_lines(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with('|') && trimmed.matches('|').count() >= 2
    })
}

fn semantic_swarm_line_text(plain: &str) -> (String, usize) {
    let trimmed = plain.trim_start_matches(' ');
    if let Some(rest) = trimmed.strip_prefix("│ ") {
        let prefix_width = unicode_width::UnicodeWidthStr::width(plain)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(rest));
        (rest.to_string(), prefix_width)
    } else {
        (plain.to_string(), 0)
    }
}

fn map_display_lines_to_logical_lines(
    display_lines: &[Line<'static>],
    logical_plain_lines: &[String],
    raw_base: usize,
) -> Option<Vec<WrappedLineMap>> {
    let mut maps = Vec::with_capacity(display_lines.len());
    let mut logical_idx = 0usize;
    let mut logical_col = 0usize;

    for line in display_lines {
        while logical_idx < logical_plain_lines.len() {
            let logical_width =
                unicode_width::UnicodeWidthStr::width(logical_plain_lines[logical_idx].as_str());
            if logical_col < logical_width || logical_width == 0 {
                break;
            }
            logical_idx += 1;
            logical_col = 0;
        }

        let logical_text = logical_plain_lines.get(logical_idx)?;
        let logical_width = unicode_width::UnicodeWidthStr::width(logical_text.as_str());
        let display_width = line.width();
        let remaining = logical_width.saturating_sub(logical_col);
        if display_width > remaining {
            return None;
        }

        maps.push(WrappedLineMap {
            raw_line: raw_base + logical_idx,
            start_col: logical_col,
            end_col: logical_col + display_width,
        });
        logical_col += display_width;
    }

    Some(maps)
}

fn user_prompt_number_style(color: Color) -> Style {
    Style::default().fg(color).bg(user_bg())
}

fn user_prompt_accent_style() -> Style {
    Style::default().fg(user_color()).bg(user_bg())
}

fn user_prompt_text_style() -> Style {
    Style::default().fg(user_text()).bg(user_bg())
}

fn default_message_alignment(role: &str, centered: bool) -> ratatui::layout::Alignment {
    if centered
        && !matches!(
            role,
            "tool" | "system" | "swarm" | "background_task" | "overnight"
        )
    {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    }
}

fn is_error_copy_content(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("Error:") || trimmed.starts_with("error:") || trimmed.starts_with("Failed:")
}

fn error_copy_target(content: &str, rendered_line_count: usize) -> Option<RawCopyTarget> {
    copy_target_for_kind(CopyTargetKind::Error, content, rendered_line_count)
}

fn tool_output_copy_target(content: &str, rendered_line_count: usize) -> Option<RawCopyTarget> {
    copy_target_for_kind(CopyTargetKind::ToolOutput, content, rendered_line_count)
}

fn copy_target_for_kind(
    kind: CopyTargetKind,
    content: &str,
    rendered_line_count: usize,
) -> Option<RawCopyTarget> {
    let content = content.trim();
    if content.is_empty() {
        return None;
    }

    Some(RawCopyTarget {
        kind,
        content: content.to_string(),
        start_raw_line: 0,
        end_raw_line: rendered_line_count.max(1),
        badge_raw_line: 0,
    })
}

fn offset_copy_target(target: RawCopyTarget, line_offset: usize) -> RawCopyTarget {
    RawCopyTarget {
        kind: target.kind,
        content: target.content,
        start_raw_line: target.start_raw_line + line_offset,
        end_raw_line: target.end_raw_line + line_offset,
        badge_raw_line: target.badge_raw_line + line_offset,
    }
}

fn assistant_message_copy_targets(
    content: &str,
    rendered_lines: &[Line<'static>],
) -> Vec<RawCopyTarget> {
    if is_error_copy_content(content) {
        return error_copy_target(content, rendered_lines.len())
            .into_iter()
            .collect();
    }

    crate::tui::markdown::extract_copy_targets_from_rendered_lines(rendered_lines)
}

fn tool_message_copy_target(
    msg: &DisplayMessage,
    rendered_line_count: usize,
) -> Option<RawCopyTarget> {
    if is_error_copy_content(&msg.content) {
        return error_copy_target(&msg.content, rendered_line_count);
    }
    if tools_ui::tool_output_looks_failed(&msg.content) {
        return tool_output_copy_target(&msg.content, rendered_line_count);
    }
    None
}

#[expect(
    clippy::too_many_arguments,
    reason = "User prompt rendering updates the prepared-line side tables together"
)]
fn push_user_prompt_lines(
    lines: &mut Vec<Line<'static>>,
    raw_plain_lines: &mut Vec<String>,
    line_raw_overrides: &mut Vec<Option<WrappedLineMap>>,
    line_copy_offsets: &mut Vec<usize>,
    user_line_indices: &mut Vec<usize>,
    prompt_num: usize,
    num_color: Color,
    content: &str,
    align: ratatui::layout::Alignment,
) {
    let prefix_width = unicode_width::UnicodeWidthStr::width(prompt_num.to_string().as_str())
        + unicode_width::UnicodeWidthStr::width("› ");
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    for (line_idx, content_line) in normalized.split('\n').enumerate() {
        let raw_line = raw_plain_lines.len();
        raw_plain_lines.push(content_line.to_string());
        let prompt_width = unicode_width::UnicodeWidthStr::width(content_line);
        let rendered_line_idx = lines.len();
        let is_first_line = line_idx == 0;
        if is_first_line {
            user_line_indices.push(rendered_line_idx);
        }

        let prefix_spans = if is_first_line {
            vec![
                Span::styled(
                    format!("{}", prompt_num),
                    user_prompt_number_style(num_color),
                ),
                Span::styled("› ", user_prompt_accent_style()),
            ]
        } else {
            vec![Span::styled(
                " ".repeat(prefix_width),
                user_prompt_accent_style(),
            )]
        };
        let mut spans = prefix_spans;
        spans.push(Span::styled(
            content_line.to_string(),
            user_prompt_text_style(),
        ));
        lines.push(Line::from(spans).alignment(align));
        line_raw_overrides.push(Some(WrappedLineMap {
            raw_line,
            start_col: 0,
            end_col: prompt_width,
        }));
        line_copy_offsets.push(prefix_width);
    }
}

fn empty_prepared_messages() -> PreparedMessages {
    PreparedMessages {
        wrapped_lines: Vec::new(),
        wrapped_plain_lines: Arc::new(Vec::new()),
        wrapped_copy_offsets: Arc::new(Vec::new()),
        raw_plain_lines: Arc::new(Vec::new()),
        wrapped_line_map: Arc::new(Vec::new()),
        wrapped_user_indices: Vec::new(),
        wrapped_user_prompt_starts: Vec::new(),
        wrapped_user_prompt_ends: Vec::new(),
        user_prompt_texts: Vec::new(),
        image_regions: Vec::new(),
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
    }
}

fn active_batch_progress(app: &dyn TuiState) -> Option<crate::bus::BatchProgress> {
    match app.status() {
        ProcessingStatus::RunningTool(name) if name == "batch" => app.batch_progress(),
        _ => None,
    }
}

pub(super) fn active_batch_progress_hash(app: &dyn TuiState) -> u64 {
    let Some(progress) = active_batch_progress(app) else {
        return 0;
    };

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if progress.completed < progress.total {
        super::activity_indicator_frame_index(app.animation_elapsed(), 12.5).hash(&mut hasher);
    }
    progress.total.hash(&mut hasher);
    progress.completed.hash(&mut hasher);
    progress.last_completed.hash(&mut hasher);
    for subcall in &progress.subcalls {
        subcall.index.hash(&mut hasher);
        subcall.tool_call.id.hash(&mut hasher);
        subcall.tool_call.name.hash(&mut hasher);
        match subcall.state {
            crate::bus::BatchSubcallState::Running => 0u8,
            crate::bus::BatchSubcallState::Succeeded => 1u8,
            crate::bus::BatchSubcallState::Failed => 2u8,
        }
        .hash(&mut hasher);
        if let Ok(input) = serde_json::to_string(&subcall.tool_call.input) {
            input.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn prepare_active_batch_progress(
    app: &dyn TuiState,
    width: u16,
    prefix_blank: bool,
) -> PreparedMessages {
    let Some(progress) = active_batch_progress(app) else {
        return empty_prepared_messages();
    };

    let centered = app.centered_mode();
    let accent = rgb(255, 193, 94);
    let spinner = super::activity_indicator(app.animation_elapsed(), 12.5);
    let block_width = if centered {
        super::centered_content_block_width(width, 96)
    } else {
        width as usize
    };
    let row_width = block_width.saturating_sub(1);
    let mut lines: Vec<Line<'static>> = Vec::new();

    if prefix_blank {
        lines.push(Line::from(""));
    }

    let mut header = vec![
        Span::styled(format!("  {} ", spinner), Style::default().fg(accent)),
        Span::styled("batch", Style::default().fg(tool_color())),
        Span::styled(
            format!(" · {}/{} done", progress.completed, progress.total),
            Style::default().fg(dim_color()),
        ),
    ];
    if let Some(last) = progress
        .last_completed
        .as_ref()
        .filter(|_| progress.completed < progress.total)
    {
        header.push(Span::styled(
            format!(" · last done: {}", last),
            Style::default().fg(dim_color()),
        ));
    }
    lines.push(super::truncate_line_with_ellipsis_to_width(
        &Line::from(header),
        width.saturating_sub(1) as usize,
    ));

    let mut hidden_completed = 0usize;
    for subcall in &progress.subcalls {
        let (icon, icon_color) = match subcall.state {
            crate::bus::BatchSubcallState::Running => (spinner, accent),
            crate::bus::BatchSubcallState::Succeeded => {
                hidden_completed += 1;
                continue;
            }
            crate::bus::BatchSubcallState::Failed => ("✗", rgb(220, 100, 100)),
        };

        lines.push(tools_ui::render_batch_subcall_line(
            &subcall.tool_call,
            icon,
            icon_color,
            50,
            Some(row_width),
            None,
        ));
    }

    if hidden_completed > 0 && progress.completed < progress.total {
        lines.push(Line::from(Span::styled(
            format!("    … {} completed", hidden_completed),
            Style::default().fg(dim_color()),
        )));
    }

    if centered {
        super::left_pad_lines_to_block_width(&mut lines, width, block_width);
    }

    wrap_lines_with_map(lines, &[], &[], &[], &[], &[], width, &[], &[])
}

pub(super) fn prepare_messages(
    app: &dyn TuiState,
    width: u16,
    height: u16,
) -> Arc<PreparedChatFrame> {
    if cfg!(test) {
        return Arc::new(prepare_messages_inner(app, width, height));
    }

    let key = FullPrepCacheKey {
        width,
        height,
        diff_mode: app.diff_mode(),
        messages_version: app.display_messages_version(),
        diagram_mode: app.diagram_mode(),
        centered: app.centered_mode(),
        is_processing: app.is_processing(),
        streaming_text_len: app.streaming_text().len(),
        streaming_text_hash: super::hash_text_for_cache(app.streaming_text()),
        batch_progress_hash: active_batch_progress_hash(app),
    };

    super::note_full_prep_request();

    {
        let cache = match full_prep_cache().lock() {
            Ok(c) => c,
            Err(poisoned) => {
                let mut c = poisoned.into_inner();
                c.entries.clear();
                c
            }
        };
        let mut cache = cache;
        if let Some((prepared, kind)) = cache.get_exact_with_kind(&key) {
            super::note_full_prep_cache_hit(kind, prepared.as_ref());
            return prepared;
        }
    }

    super::note_full_prep_cache_miss();

    let prepared = Arc::new(prepare_messages_inner(app, width, height));
    super::note_full_prep_built(prepared.as_ref());

    {
        if let Ok(mut cache) = full_prep_cache().lock() {
            cache.insert(key, prepared.clone());
        }
    }

    prepared
}

fn prepare_messages_inner(app: &dyn TuiState, width: u16, height: u16) -> PreparedChatFrame {
    let mut all_header_lines = header::build_persistent_header(app, width);
    all_header_lines.extend(header::build_header_lines(app, width));
    let header_prepared = Arc::new(wrap_lines(all_header_lines, &[], &[], &[], width));

    let body_prepared = prepare_body_cached(app, width);
    let has_batch_progress = active_batch_progress(app).is_some();
    let batch_prefix_blank = has_batch_progress && !body_prepared.wrapped_lines.is_empty();
    let batch_progress_prepared = if has_batch_progress {
        Arc::new(prepare_active_batch_progress(
            app,
            width,
            batch_prefix_blank,
        ))
    } else {
        Arc::new(empty_prepared_messages())
    };
    let has_streaming = app.is_processing() && !app.streaming_text().is_empty();
    let stream_prefix_blank = has_streaming
        && (!body_prepared.wrapped_lines.is_empty()
            || !batch_progress_prepared.wrapped_lines.is_empty());
    let streaming_prepared = if has_streaming {
        Arc::new(prepare_streaming_cached(app, width, stream_prefix_blank))
    } else {
        Arc::new(empty_prepared_messages())
    };

    let is_initial_empty = app.display_messages().is_empty()
        && !app.is_processing()
        && app.streaming_text().is_empty();

    if is_initial_empty {
        let suggestions = app.suggestion_prompts();
        let is_centered = app.centered_mode();
        let suggestion_align = if is_centered {
            ratatui::layout::Alignment::Center
        } else {
            ratatui::layout::Alignment::Left
        };
        let mut wrapped_lines = header_prepared.wrapped_lines.clone();

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
        let wrapped_lines = centered;
        let wrapped_line_count = wrapped_lines.len();
        let wrapped_plain_lines = Arc::new(wrapped_lines.iter().map(ui::line_plain_text).collect());
        let prepared = Arc::new(PreparedMessages {
            wrapped_lines,
            wrapped_plain_lines,
            wrapped_copy_offsets: Arc::new(vec![0; wrapped_line_count]),
            raw_plain_lines: Arc::new(Vec::new()),
            wrapped_line_map: Arc::new(Vec::new()),
            wrapped_user_indices: Vec::new(),
            wrapped_user_prompt_starts: Vec::new(),
            wrapped_user_prompt_ends: Vec::new(),
            user_prompt_texts: Vec::new(),
            image_regions: Vec::new(),
            edit_tool_ranges: Vec::new(),
            copy_targets: Vec::new(),
        });
        return PreparedChatFrame::from_single(prepared);
    }

    PreparedChatFrame::from_sections(vec![
        (PreparedSectionKind::Header, header_prepared),
        (PreparedSectionKind::Body, body_prepared),
        (PreparedSectionKind::BatchProgress, batch_progress_prepared),
        (PreparedSectionKind::Streaming, streaming_prepared),
    ])
}

fn prepare_body_cached(app: &dyn TuiState, width: u16) -> Arc<PreparedMessages> {
    if cfg!(test) {
        return Arc::new(prepare_body(app, width, false));
    }

    super::note_body_request();

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
    if let Some((prepared, kind)) = cache.get_exact_with_kind(&key) {
        super::note_body_cache_hit(kind, prepared.as_ref());
        return prepared;
    }

    super::note_body_cache_miss();

    let incremental_base = cache.take_best_incremental_base(&key, msg_count);

    drop(cache);

    let prepared = if let Some((prev, prev_count)) = incremental_base {
        super::note_body_incremental_reuse(prev_count);
        prepare_body_incremental(app, width, prev, prev_count)
    } else {
        Arc::new(prepare_body(app, width, false))
    };

    super::note_body_built(prepared.as_ref());

    let mut cache = match body_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };
    cache.insert(key, prepared.clone(), msg_count);
    prepared
}

pub(super) fn prepare_body_incremental(
    app: &dyn TuiState,
    width: u16,
    mut prev: Arc<PreparedMessages>,
    prev_msg_count: usize,
) -> Arc<PreparedMessages> {
    let messages = app.display_messages();
    let new_messages = &messages[prev_msg_count..];
    if new_messages.is_empty() {
        return prev;
    }

    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);
    let align = if centered {
        ratatui::layout::Alignment::Center
    } else {
        ratatui::layout::Alignment::Left
    };

    let total_prompts = app.display_user_message_count();
    let pending_count = input_ui::pending_prompt_count(app);

    let mut prompt_num = messages[..prev_msg_count]
        .iter()
        .filter(|m| m.effective_role() == "user")
        .count();

    let mut new_lines: Vec<Line> = Vec::new();
    let mut new_user_line_indices: Vec<usize> = Vec::new();
    let mut new_user_prompt_texts: Vec<String> = Vec::new();
    let mut new_edit_tool_line_ranges: Vec<(usize, String, usize, usize)> = Vec::new();
    let mut new_copy_targets: Vec<RawCopyTarget> = Vec::new();
    let mut new_raw_plain_lines: Vec<String> = Vec::new();
    let mut new_line_raw_overrides: Vec<Option<WrappedLineMap>> = Vec::new();
    let mut new_line_copy_offsets: Vec<usize> = Vec::new();

    let body_has_content = !prev.wrapped_lines.is_empty();

    for (new_msg_offset, msg) in new_messages.iter().enumerate() {
        let role = msg.effective_role();
        if (body_has_content || !new_lines.is_empty()) && role != "tool" && role != "meta" {
            new_lines.push(Line::from(""));
            new_line_raw_overrides.push(None);
            new_line_copy_offsets.push(0);
        }

        match role {
            "user" => {
                prompt_num += 1;
                new_user_line_indices.push(new_lines.len());
                new_user_prompt_texts.push(msg.content.clone());
                let distance = total_prompts + pending_count + 1 - prompt_num;
                let num_color = rainbow_prompt_color(distance);
                let raw_line = new_raw_plain_lines.len();
                new_raw_plain_lines.push(msg.content.clone());
                let prompt_width = unicode_width::UnicodeWidthStr::width(msg.content.as_str());
                let prefix_width =
                    unicode_width::UnicodeWidthStr::width(prompt_num.to_string().as_str())
                        + unicode_width::UnicodeWidthStr::width("› ");
                new_lines.push(
                    Line::from(vec![
                        Span::styled(format!("{}", prompt_num), Style::default().fg(num_color)),
                        Span::styled("› ", Style::default().fg(user_color())),
                        Span::styled(msg.content.clone(), Style::default().fg(user_text())),
                    ])
                    .alignment(align),
                );
                new_line_raw_overrides.push(Some(WrappedLineMap {
                    raw_line,
                    start_col: 0,
                    end_col: prompt_width,
                }));
                new_line_copy_offsets.push(prefix_width);
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
                    new_copy_targets.push(offset_copy_target(target, new_lines.len()));
                }
                for line in cached {
                    new_lines.push(align_if_unset(line, align));
                    new_line_raw_overrides.push(None);
                    new_line_copy_offsets.push(0);
                }
            }
            "meta" => {
                let raw_line = new_raw_plain_lines.len();
                new_raw_plain_lines.push(msg.content.clone());
                let raw_width = unicode_width::UnicodeWidthStr::width(msg.content.as_str());
                let prefix_width = if centered {
                    0
                } else {
                    unicode_width::UnicodeWidthStr::width("  ")
                };
                new_lines.push(
                    Line::from(vec![
                        Span::raw(if centered { "" } else { "  " }),
                        Span::styled(msg.content.clone(), Style::default().fg(dim_color())),
                    ])
                    .alignment(align),
                );
                new_line_raw_overrides.push(Some(WrappedLineMap {
                    raw_line,
                    start_col: 0,
                    end_col: raw_width,
                }));
                new_line_copy_offsets.push(prefix_width);
            }
            "tool" => {
                let tool_start_line = new_lines.len();
                let cached =
                    get_cached_message_lines(msg, width, app.diff_mode(), render_tool_message);
                if let Some(target) = tool_message_copy_target(msg, cached.len()) {
                    new_copy_targets.push(offset_copy_target(target, tool_start_line));
                }
                for line in cached {
                    new_lines.push(align_if_unset(line, align));
                    new_line_raw_overrides.push(None);
                    new_line_copy_offsets.push(0);
                }
                if let Some(ref tc) = msg.tool_data {
                    let is_edit_tool = tools_ui::is_edit_tool_name(&tc.name);
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
                                    .and_then(|patch_text| {
                                        match tools_ui::canonical_tool_name(&tc.name) {
                                            "apply_patch" => {
                                                tools_ui::extract_apply_patch_primary_file(
                                                    patch_text,
                                                )
                                            }
                                            "patch" => {
                                                tools_ui::extract_unified_patch_primary_file(
                                                    patch_text,
                                                )
                                            }
                                            _ => None,
                                        }
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
                    new_line_raw_overrides.push(None);
                    new_line_copy_offsets.push(0);
                }
            }
            "background_task" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_background_task_message,
                );
                for line in cached {
                    new_lines.push(align_if_unset(line, align));
                    new_line_raw_overrides.push(None);
                    new_line_copy_offsets.push(0);
                }
            }
            "swarm" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_swarm_message,
                );
                for line in cached {
                    let line = align_if_unset(line, align);
                    let plain = ui::line_plain_text(&line);
                    let (semantic, prefix_width) = semantic_swarm_line_text(plain.as_str());
                    let raw_line = new_raw_plain_lines.len();
                    let raw_width = unicode_width::UnicodeWidthStr::width(semantic.as_str());
                    new_raw_plain_lines.push(semantic);
                    new_lines.push(line);
                    new_line_raw_overrides.push(Some(WrappedLineMap {
                        raw_line,
                        start_col: 0,
                        end_col: raw_width,
                    }));
                    new_line_copy_offsets.push(prefix_width);
                }
            }
            "memory" => {
                let border_style = Style::default().fg(rgb(130, 140, 180));
                let text_style = Style::default().fg(dim_color());
                let entries = super::memory_ui::parse_memory_display_entries(&msg.content);

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
                    new_line_raw_overrides.push(None);
                    new_line_copy_offsets.push(0);
                }
            }
            "usage" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_usage_message,
                );
                for line in cached {
                    new_lines.push(align_if_unset(line, align));
                    new_line_raw_overrides.push(None);
                    new_line_copy_offsets.push(0);
                }
            }
            "overnight" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    super::messages::render_overnight_message,
                );
                for line in cached {
                    new_lines.push(align_if_unset(line, align));
                    new_line_raw_overrides.push(None);
                    new_line_copy_offsets.push(0);
                }
            }
            "error" => {
                let error_start_line = new_lines.len();
                if let Some(target) = error_copy_target(&msg.content, 1) {
                    new_copy_targets.push(offset_copy_target(target, error_start_line));
                }
                let raw_line = new_raw_plain_lines.len();
                new_raw_plain_lines.push(msg.content.clone());
                let raw_width = unicode_width::UnicodeWidthStr::width(msg.content.as_str());
                let prefix_width =
                    unicode_width::UnicodeWidthStr::width(if centered { "✗ " } else { "  ✗ " });
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
                new_line_raw_overrides.push(Some(WrappedLineMap {
                    raw_line,
                    start_col: 0,
                    end_col: raw_width,
                }));
                new_line_copy_offsets.push(prefix_width);
            }
            _ => {}
        }
    }

    let new_wrapped = wrap_lines_with_map(
        new_lines,
        &new_raw_plain_lines,
        &new_line_raw_overrides,
        &new_line_copy_offsets,
        &new_user_line_indices,
        &new_user_prompt_texts,
        width,
        &new_edit_tool_line_ranges,
        &new_copy_targets,
    );

    let prepared = Arc::make_mut(&mut prev);
    let prev_len = prepared.wrapped_lines.len();
    let prev_raw_len = prepared.raw_plain_lines.len();
    let edit_index_base = prepared.edit_tool_ranges.len();

    prepared.wrapped_lines.extend(new_wrapped.wrapped_lines);
    Arc::make_mut(&mut prepared.wrapped_plain_lines)
        .extend(new_wrapped.wrapped_plain_lines.iter().cloned());
    Arc::make_mut(&mut prepared.wrapped_copy_offsets)
        .extend(new_wrapped.wrapped_copy_offsets.iter().copied());
    Arc::make_mut(&mut prepared.raw_plain_lines)
        .extend(new_wrapped.raw_plain_lines.iter().cloned());

    {
        let wrapped_line_map = Arc::make_mut(&mut prepared.wrapped_line_map);
        for map in new_wrapped.wrapped_line_map.iter().copied() {
            wrapped_line_map.push(WrappedLineMap {
                raw_line: map.raw_line + prev_raw_len,
                ..map
            });
        }
    }

    prepared.wrapped_user_indices.extend(
        new_wrapped
            .wrapped_user_indices
            .into_iter()
            .map(|idx| idx + prev_len),
    );
    prepared.wrapped_user_prompt_starts.extend(
        new_wrapped
            .wrapped_user_prompt_starts
            .into_iter()
            .map(|idx| idx + prev_len),
    );
    prepared.wrapped_user_prompt_ends.extend(
        new_wrapped
            .wrapped_user_prompt_ends
            .into_iter()
            .map(|idx| idx + prev_len),
    );
    prepared
        .user_prompt_texts
        .extend(new_wrapped.user_prompt_texts);
    prepared
        .image_regions
        .extend(
            new_wrapped
                .image_regions
                .into_iter()
                .map(|region| ImageRegion {
                    abs_line_idx: region.abs_line_idx + prev_len,
                    end_line: region.end_line + prev_len,
                    ..region
                }),
        );
    prepared
        .edit_tool_ranges
        .extend(
            new_wrapped
                .edit_tool_ranges
                .into_iter()
                .map(|r| EditToolRange {
                    edit_index: edit_index_base + r.edit_index,
                    msg_index: r.msg_index,
                    file_path: r.file_path,
                    start_line: r.start_line + prev_len,
                    end_line: r.end_line + prev_len,
                }),
        );
    prepared.copy_targets.extend(
        new_wrapped
            .copy_targets
            .into_iter()
            .map(|target| CopyTarget {
                start_line: target.start_line + prev_len,
                end_line: target.end_line + prev_len,
                badge_line: target.badge_line + prev_len,
                ..target
            }),
    );

    prev
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
            wrapped_plain_lines: Arc::new(Vec::new()),
            wrapped_copy_offsets: Arc::new(Vec::new()),
            raw_plain_lines: Arc::new(Vec::new()),
            wrapped_line_map: Arc::new(Vec::new()),
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
    let display_width = width.saturating_sub(4) as usize;

    let content_width = if centered {
        display_width.clamp(1, 96)
    } else {
        display_width
    };
    let mut md_lines = app.render_streaming_markdown(content_width);
    if centered {
        markdown::recenter_structured_blocks_for_display(&mut md_lines, display_width);
    }
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

    wrap_lines(lines, &[], &[], &[], width)
}

pub(super) fn prepare_body(
    app: &dyn TuiState,
    width: u16,
    include_streaming: bool,
) -> PreparedMessages {
    let mut lines: Vec<Line> = Vec::new();
    let mut raw_plain_lines: Vec<String> = Vec::new();
    let mut line_raw_overrides: Vec<Option<WrappedLineMap>> = Vec::new();
    let mut line_copy_offsets: Vec<usize> = Vec::new();
    let mut user_line_indices: Vec<usize> = Vec::new();
    let mut user_prompt_texts: Vec<String> = Vec::new();
    let mut edit_tool_line_ranges: Vec<(usize, String, usize, usize)> = Vec::new();
    let mut copy_targets: Vec<RawCopyTarget> = Vec::new();
    let centered = app.centered_mode();
    markdown::set_center_code_blocks(centered);
    let display_width = width.saturating_sub(4) as usize;
    let mut prompt_num = 0usize;
    let total_prompts = app.display_user_message_count();
    let pending_count = input_ui::pending_prompt_count(app);

    for (msg_idx, msg) in app.display_messages().iter().enumerate() {
        let role = msg.effective_role();
        let align = default_message_alignment(role, centered);
        if !lines.is_empty() && role != "tool" && role != "meta" && role != "swarm" {
            lines.push(Line::from(""));
            line_raw_overrides.push(None);
            line_copy_offsets.push(0);
        }

        match role {
            "user" => {
                prompt_num += 1;
                user_prompt_texts.push(msg.content.clone());
                let distance = total_prompts + pending_count + 1 - prompt_num;
                let num_color = rainbow_prompt_color(distance);
                push_user_prompt_lines(
                    &mut lines,
                    &mut raw_plain_lines,
                    &mut line_raw_overrides,
                    &mut line_copy_offsets,
                    &mut user_line_indices,
                    prompt_num,
                    num_color,
                    &msg.content,
                    align,
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
                    copy_targets.push(offset_copy_target(target, lines.len()));
                }
                let content_lines = markdown::render_markdown_with_width(
                    &msg.content,
                    Some(content_width as usize),
                );
                let content_line_count = content_lines.len().min(cached.len());
                let logical_plain_lines: Vec<String> =
                    if content_prefers_display_as_logical_lines(&msg.content) {
                        cached
                            .iter()
                            .take(content_line_count)
                            .map(ui::line_plain_text)
                            .collect()
                    } else {
                        markdown::render_markdown(&msg.content)
                            .into_iter()
                            .map(|line| ui::line_plain_text(&align_if_unset(line, align)))
                            .collect()
                    };
                let raw_base = raw_plain_lines.len();
                raw_plain_lines.extend(logical_plain_lines.iter().cloned());
                let content_maps = map_display_lines_to_logical_lines(
                    &cached[..content_line_count],
                    &logical_plain_lines,
                    raw_base,
                );

                for (idx, line) in cached.into_iter().enumerate() {
                    lines.push(align_if_unset(line, align));
                    if idx < content_line_count {
                        line_raw_overrides.push(
                            content_maps
                                .as_ref()
                                .and_then(|maps| maps.get(idx).copied()),
                        );
                    } else {
                        line_raw_overrides.push(None);
                    }
                    line_copy_offsets.push(0);
                }
            }
            "meta" => {
                let raw_line = raw_plain_lines.len();
                raw_plain_lines.push(msg.content.clone());
                let raw_width = unicode_width::UnicodeWidthStr::width(msg.content.as_str());
                let prefix_width = if centered {
                    0
                } else {
                    unicode_width::UnicodeWidthStr::width("  ")
                };
                lines.push(
                    Line::from(vec![
                        Span::raw(if centered { "" } else { "  " }),
                        Span::styled(msg.content.clone(), Style::default().fg(dim_color())),
                    ])
                    .alignment(align),
                );
                line_raw_overrides.push(Some(WrappedLineMap {
                    raw_line,
                    start_col: 0,
                    end_col: raw_width,
                }));
                line_copy_offsets.push(prefix_width);
            }
            "tool" => {
                let tool_start_line = lines.len();
                let cached =
                    get_cached_message_lines(msg, width, app.diff_mode(), render_tool_message);
                if let Some(target) = tool_message_copy_target(msg, cached.len()) {
                    copy_targets.push(offset_copy_target(target, tool_start_line));
                }
                for line in cached {
                    lines.push(align_if_unset(line, align));
                    line_raw_overrides.push(None);
                    line_copy_offsets.push(0);
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
                    line_raw_overrides.push(None);
                    line_copy_offsets.push(0);
                }
            }
            "background_task" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_background_task_message,
                );
                for line in cached {
                    lines.push(align_if_unset(line, align));
                    line_raw_overrides.push(None);
                    line_copy_offsets.push(0);
                }
            }
            "swarm" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_swarm_message,
                );
                for line in cached {
                    let line = align_if_unset(line, align);
                    let plain = ui::line_plain_text(&line);
                    let (semantic, prefix_width) = semantic_swarm_line_text(plain.as_str());
                    let raw_line = raw_plain_lines.len();
                    let raw_width = unicode_width::UnicodeWidthStr::width(semantic.as_str());
                    raw_plain_lines.push(semantic);
                    lines.push(line);
                    line_raw_overrides.push(Some(WrappedLineMap {
                        raw_line,
                        start_col: 0,
                        end_col: raw_width,
                    }));
                    line_copy_offsets.push(prefix_width);
                }
            }
            "memory" => {
                let border_style = Style::default().fg(rgb(130, 140, 180));
                let text_style = Style::default().fg(dim_color());
                let entries = super::memory_ui::parse_memory_display_entries(&msg.content);

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
                    line_raw_overrides.push(None);
                    line_copy_offsets.push(0);
                }
            }
            "usage" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    render_usage_message,
                );
                for line in cached {
                    lines.push(align_if_unset(line, align));
                    line_raw_overrides.push(None);
                    line_copy_offsets.push(0);
                }
            }
            "overnight" => {
                let content_width = width.saturating_sub(4);
                let cached = get_cached_message_lines(
                    msg,
                    content_width,
                    app.diff_mode(),
                    super::messages::render_overnight_message,
                );
                for line in cached {
                    lines.push(align_if_unset(line, align));
                    line_raw_overrides.push(None);
                    line_copy_offsets.push(0);
                }
            }
            "error" => {
                let error_start_line = lines.len();
                if let Some(target) = error_copy_target(&msg.content, 1) {
                    copy_targets.push(offset_copy_target(target, error_start_line));
                }
                let raw_line = raw_plain_lines.len();
                raw_plain_lines.push(msg.content.clone());
                let raw_width = unicode_width::UnicodeWidthStr::width(msg.content.as_str());
                let prefix_width =
                    unicode_width::UnicodeWidthStr::width(if centered { "✗ " } else { "  ✗ " });
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
                line_raw_overrides.push(Some(WrappedLineMap {
                    raw_line,
                    start_col: 0,
                    end_col: raw_width,
                }));
                line_copy_offsets.push(prefix_width);
            }
            _ => {}
        }
    }

    if include_streaming && app.is_processing() && !app.streaming_text().is_empty() {
        if !lines.is_empty() {
            lines.push(Line::from(""));
            line_raw_overrides.push(None);
            line_copy_offsets.push(0);
        }
        let content_width = if centered {
            display_width.clamp(1, 96)
        } else {
            display_width
        };
        let mut md_lines = app.render_streaming_markdown(content_width);
        if centered {
            markdown::recenter_structured_blocks_for_display(&mut md_lines, display_width);
        }
        let align = default_message_alignment("assistant", centered);
        for line in md_lines {
            lines.push(align_if_unset(line, align));
            line_raw_overrides.push(None);
            line_copy_offsets.push(0);
        }
    }

    wrap_lines_with_map(
        lines,
        &raw_plain_lines,
        &line_raw_overrides,
        &line_copy_offsets,
        &user_line_indices,
        &user_prompt_texts,
        width,
        &edit_tool_line_ranges,
        &copy_targets,
    )
}

fn wrap_lines(
    lines: Vec<Line<'static>>,
    line_copy_offsets: &[usize],
    user_line_indices: &[usize],
    user_prompt_texts: &[String],
    width: u16,
) -> PreparedMessages {
    let full_width = width.saturating_sub(1) as usize;
    let user_width = width.saturating_sub(2) as usize;
    let mut wrapped_user_indices: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_starts: Vec<usize> = Vec::new();
    let mut wrapped_user_prompt_ends: Vec<usize> = Vec::new();
    let mut raw_plain_lines: Vec<String> = Vec::with_capacity(lines.len());
    let mut wrapped_line_map: Vec<WrappedLineMap> = Vec::new();
    let mut wrapped_copy_offsets: Vec<usize> = Vec::new();
    let mut user_line_mask = vec![false; lines.len()];
    for &idx in user_line_indices {
        if idx < user_line_mask.len() {
            user_line_mask[idx] = true;
        }
    }
    let mut wrapped_idx = 0usize;

    let mut wrapped_lines: Vec<Line> = Vec::new();
    for (orig_idx, line) in lines.into_iter().enumerate() {
        let raw_text = ui::line_plain_text(&line);
        let raw_width = unicode_width::UnicodeWidthStr::width(raw_text.as_str());
        raw_plain_lines.push(raw_text);
        let is_user_line = user_line_mask.get(orig_idx).copied().unwrap_or(false);
        let wrap_width = if is_user_line { user_width } else { full_width };
        let new_lines = markdown::wrap_line(line, wrap_width);
        let count = new_lines.len();
        let mut remaining_copy_offset = line_copy_offsets.get(orig_idx).copied().unwrap_or(0);
        let mut start_col = 0usize;

        for wrapped_line in &new_lines {
            let width = wrapped_line.width();
            let end_col = (start_col + width).min(raw_width);
            wrapped_line_map.push(WrappedLineMap {
                raw_line: orig_idx,
                start_col,
                end_col,
            });
            wrapped_copy_offsets.push(remaining_copy_offset.min(width));
            remaining_copy_offset = remaining_copy_offset.saturating_sub(width);
            start_col = end_col;
        }

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

    let wrapped_plain_lines = Arc::new(wrapped_lines.iter().map(ui::line_plain_text).collect());

    PreparedMessages {
        wrapped_lines,
        wrapped_plain_lines,
        wrapped_copy_offsets: Arc::new(wrapped_copy_offsets),
        raw_plain_lines: Arc::new(raw_plain_lines),
        wrapped_line_map: Arc::new(wrapped_line_map),
        wrapped_user_indices,
        wrapped_user_prompt_starts,
        wrapped_user_prompt_ends,
        user_prompt_texts: user_prompt_texts.to_vec(),
        image_regions,
        edit_tool_ranges: Vec::new(),
        copy_targets: Vec::new(),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Wrapped-line preparation carries explicit render state to avoid hidden coupling"
)]
fn wrap_lines_with_map(
    lines: Vec<Line<'static>>,
    seeded_raw_plain_lines: &[String],
    line_raw_overrides: &[Option<WrappedLineMap>],
    line_copy_offsets: &[usize],
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
    let mut raw_plain_lines: Vec<String> = seeded_raw_plain_lines.to_vec();
    let mut wrapped_line_map: Vec<WrappedLineMap> = Vec::new();
    let mut wrapped_copy_offsets: Vec<usize> = Vec::new();
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
        let (raw_line, start_col, end_col) =
            if let Some(Some(map)) = line_raw_overrides.get(orig_idx) {
                (map.raw_line, map.start_col, map.end_col)
            } else {
                let raw_text = ui::line_plain_text(&line);
                let raw_width = unicode_width::UnicodeWidthStr::width(raw_text.as_str());
                let raw_line = raw_plain_lines.len();
                raw_plain_lines.push(raw_text);
                (raw_line, 0usize, raw_width)
            };
        raw_to_wrapped.push(wrapped_idx);
        let is_user_line = user_line_mask.get(orig_idx).copied().unwrap_or(false);
        let wrap_width = if is_user_line { user_width } else { full_width };
        let new_lines = markdown::wrap_line(line, wrap_width);
        let count = new_lines.len();
        let mut remaining_copy_offset = line_copy_offsets.get(orig_idx).copied().unwrap_or(0);
        let mut segment_start = start_col;

        for wrapped_line in &new_lines {
            let width = wrapped_line.width();
            let segment_end = (segment_start + width).min(end_col);
            wrapped_line_map.push(WrappedLineMap {
                raw_line,
                start_col: segment_start,
                end_col: segment_end,
            });
            wrapped_copy_offsets.push(remaining_copy_offset.min(width));
            remaining_copy_offset = remaining_copy_offset.saturating_sub(width);
            segment_start = segment_end;
        }

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

    let wrapped_plain_lines = Arc::new(wrapped_lines.iter().map(ui::line_plain_text).collect());

    PreparedMessages {
        wrapped_lines,
        wrapped_plain_lines,
        wrapped_copy_offsets: Arc::new(wrapped_copy_offsets),
        raw_plain_lines: Arc::new(raw_plain_lines),
        wrapped_line_map: Arc::new(wrapped_line_map),
        wrapped_user_indices,
        wrapped_user_prompt_starts,
        wrapped_user_prompt_ends,
        user_prompt_texts: user_prompt_texts.to_vec(),
        image_regions,
        edit_tool_ranges,
        copy_targets,
    }
}

#[cfg(test)]
#[path = "ui_prepare/tests.rs"]
mod tests;
