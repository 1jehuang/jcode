//! Prepared message rendering.

use super::*;

/// Render a single transcript message into `acc`. This is the one canonical
/// per-message renderer; the full and incremental body builders both call it so
/// their output cannot drift (which previously caused subtle text-selection and
/// spacing differences on the incremental path).
pub(super) fn render_message_into(
    ctx: &BodyRenderCtx<'_>,
    acc: &mut BodyAcc,
    msg: &DisplayMessage,
    msg_global_idx: usize,
) {
    let width = ctx.width;
    let centered = ctx.centered;
    let diff_mode = ctx.diff_mode;
    let role = msg.effective_role();
    // Keep routine await notices and pinned todo tool messages in
    // display_messages for history fidelity, but omit their transcript cards.
    if super::super::messages::is_swarm_await_started(msg)
        || super::super::messages::is_routine_swarm_await_result(msg)
        || (role == "tool"
            && crate::config::config().display.pin_todos
            && msg
                .tool_data
                .as_ref()
                .is_some_and(|tool| tools_ui::canonical_tool_name(&tool.name) == "todo"))
    {
        // Hidden messages still occupy a position in the transcript cache.
        // Without this zero-length boundary, incremental updates compare later
        // messages against the wrong indices and can truncate earlier content.
        acc.segments.push((
            msg.stable_cache_hash(),
            acc.lines.len(),
            acc.raw_plain_lines.len(),
            acc.user_prompt_texts.len(),
        ));
        return;
    }
    let align = default_message_alignment(role, centered);

    if (acc.body_has_content || !acc.lines.is_empty())
        && super::super::messages::needs_message_separator(msg)
    {
        acc.push_blank();
    }

    match role {
        "user" => {
            acc.prompt_num += 1;
            let config = &crate::config::config().jarvis;
            let visible = if config.show_exact_request_payload {
                std::borrow::Cow::Borrowed(msg.content.as_str())
            } else {
                msg.user_row_text(config.show_raw_requests)
            };
            acc.user_prompt_texts.push(visible.to_string());
            let distance = ctx.total_prompts + ctx.pending_count + 1 - acc.prompt_num;
            let num_color = rainbow_prompt_color(distance);
            let displayed_prompt_num = acc.prompt_num + ctx.prompt_number_offset;
            // Only traditional dictation uses transcription tags. Never reinterpret
            // the words or JSON inside a trusted Jarvis row.
            let marked;
            let prompt_text: &str = if msg.is_jarvis_user() {
                &visible
            } else {
                let (text, transcribed) = jcode_session_types::strip_transcription(&visible);
                if transcribed {
                    marked = format!("🎙 {text}");
                    &marked
                } else {
                    &visible
                }
            };
            push_user_prompt_lines(
                &mut acc.lines,
                &mut acc.raw_plain_lines,
                &mut acc.line_raw_overrides,
                &mut acc.line_copy_offsets,
                &mut acc.user_line_indices,
                displayed_prompt_num,
                num_color,
                prompt_text,
                align,
            );
            if !crate::session::is_attached_image_label_text(&msg.content) {
                let ordinal = acc.anchor_prompt_ordinal;
                acc.anchor_prompt_ordinal += 1;
                if let (Some(app), Some(items)) =
                    (ctx.app, ctx.anchored_images.by_prompt.get(&ordinal))
                {
                    for line in super::super::inline_image_ui::anchored_image_lines(
                        items,
                        width,
                        ctx.inline_images_visible,
                        &super::super::inline_image_ui::AppExpandLevels(app),
                    ) {
                        acc.push_auto(line);
                    }
                }
            }
        }
        "assistant" => {
            let content_width = width.saturating_sub(4);
            let cached =
                get_cached_message_lines(msg, content_width, diff_mode, render_assistant_message);
            let message_copy_targets = assistant_message_copy_targets(&msg.content, &cached);
            for target in message_copy_targets {
                acc.copy_targets
                    .push(offset_copy_target(target, acc.lines.len()));
            }
            let aux = assistant_aux_data(msg, &cached, content_width, centered, diff_mode, align);
            let content_line_count = aux.content_line_count;
            let logical_plain_lines = &aux.logical_plain_lines;
            let raw_base = acc.raw_plain_lines.len();
            let content_maps = map_display_lines_to_logical_lines(
                &cached[..content_line_count],
                logical_plain_lines,
                raw_base,
            );
            if let Some(maps) = content_maps {
                // Content lines map back into the logical markdown lines for
                // text selection; seed those raws contiguously. Trailing
                // tool-summary lines (idx >= content_line_count) and any line
                // the mapping could not cover fall back to self-raws via
                // `push_auto`, keeping `raw_plain_lines` message-contiguous.
                acc.raw_plain_lines
                    .extend(logical_plain_lines.iter().cloned());
                for (idx, line) in cached.into_iter().enumerate() {
                    let line = align_if_unset(line, align);
                    if let Some(map) = maps.get(idx).copied() {
                        acc.lines.push(line);
                        acc.line_raw_overrides.push(Some(map));
                        acc.line_copy_offsets.push(0);
                    } else {
                        acc.push_auto(line);
                    }
                }
            } else {
                // Logical mapping failed; every display line uses its own plain
                // text as the raw, matching the previous wrap-time fallback but
                // recorded contiguously and in message order.
                for line in cached {
                    acc.push_auto(align_if_unset(line, align));
                }
            }
        }
        "meta" => {
            let raw_line = acc.raw_plain_lines.len();
            acc.raw_plain_lines.push(msg.content.clone());
            let raw_width = unicode_width::UnicodeWidthStr::width(msg.content.as_str());
            let prefix_width = if centered {
                0
            } else {
                unicode_width::UnicodeWidthStr::width("  ")
            };
            acc.lines.push(
                Line::from(vec![
                    Span::raw(if centered { "" } else { "  " }),
                    Span::styled(msg.content.clone(), Style::default().fg(dim_color())),
                ])
                .alignment(align),
            );
            acc.line_raw_overrides.push(Some(WrappedLineMap {
                raw_line,
                start_col: 0,
                end_col: raw_width,
            }));
            acc.line_copy_offsets.push(prefix_width);
        }
        "tool" => {
            let tool_start_line = acc.lines.len();
            let cached = get_cached_message_lines(msg, width, diff_mode, render_tool_message);
            if let Some(target) = tool_message_copy_target(msg, cached.len()) {
                acc.copy_targets
                    .push(offset_copy_target(target, tool_start_line));
            }
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
            if let Some(member) = spawned_member_for_tool(msg, &ctx.swarm_members) {
                for line in crate::tui::info_widget::swarm_gallery::render_swarm_chat_card_lines(
                    std::slice::from_ref(member),
                    width.saturating_sub(1) as usize,
                ) {
                    acc.push_auto(line.alignment(ratatui::layout::Alignment::Left));
                }
            }
            for line in todo_change_lines(ctx.messages, msg_global_idx, msg, width) {
                acc.push_auto(align_if_unset(line, align));
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
                                    match tools_ui::edit_render_name(&tc.name, &tc.input) {
                                        "apply_patch" => {
                                            tools_ui::extract_apply_patch_primary_file(patch_text)
                                        }
                                        "patch" => {
                                            tools_ui::extract_unified_patch_primary_file(patch_text)
                                        }
                                        _ => None,
                                    }
                                })
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    // Outside the inline modes the diff body is hidden entirely,
                    // so any edit with a diff can be expanded into full inline.
                    let expandable = if diff_mode.is_inline() {
                        messages::edit_tool_inline_diff_is_expandable(tc, &msg.content, width)
                    } else {
                        messages::edit_tool_has_inline_diff(tc, &msg.content)
                    };
                    acc.edit_tool_line_ranges.push((
                        msg_global_idx,
                        file_path,
                        tool_start_line,
                        acc.lines.len(),
                        expandable,
                    ));
                }
                if let (Some(app), Some(items)) = (ctx.app, ctx.anchored_images.by_tool.get(&tc.id))
                {
                    for line in super::super::inline_image_ui::anchored_image_lines(
                        items,
                        width,
                        ctx.inline_images_visible,
                        &super::super::inline_image_ui::AppExpandLevels(app),
                    ) {
                        acc.push_auto(line);
                    }
                }
            }
        }
        "system" => {
            let content_width = width.saturating_sub(4);
            let cached =
                get_cached_message_lines(msg, content_width, diff_mode, render_system_message);
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        // Terminal-style clear spacer (Ctrl+L): N blank rows that push the
        // prior transcript up out of the viewport while keeping it in
        // scrollback. The separator blank already pushed above counts toward
        // the requested height.
        "spacer" => {
            let rows: usize = msg.content.trim().parse().unwrap_or(0);
            for _ in 0..rows.saturating_sub(1) {
                acc.push_blank();
            }
        }
        "reasoning" => {
            let content_width = width.saturating_sub(4);
            let cached =
                get_cached_message_lines(msg, content_width, diff_mode, render_reasoning_message);
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "background_task" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                diff_mode,
                render_background_task_message,
            );
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "swarm" => {
            let content_width = width.saturating_sub(4);
            let cached = if super::super::messages::is_collapsed_agent_dm(msg) {
                let previous_is_dm = msg_global_idx
                    .checked_sub(1)
                    .and_then(|idx| ctx.messages.get(idx))
                    .is_some_and(super::super::messages::is_collapsed_agent_dm);
                if previous_is_dm {
                    Vec::new()
                } else {
                    let run_len = ctx.messages[msg_global_idx..]
                        .iter()
                        .take_while(|message| {
                            super::super::messages::is_collapsed_agent_dm(message)
                        })
                        .count();
                    vec![super::super::messages::render_agent_dm_run(
                        &ctx.messages[msg_global_idx..msg_global_idx + run_len],
                        content_width,
                    )]
                }
            } else {
                get_cached_message_lines(msg, content_width, diff_mode, render_swarm_message)
            };
            for line in cached {
                let line = align_if_unset(line, align);
                let plain = ui::line_plain_text(&line);
                let (semantic, prefix_width) = semantic_swarm_line_text(plain.as_str());
                let raw_line = acc.raw_plain_lines.len();
                let raw_width = unicode_width::UnicodeWidthStr::width(semantic.as_str());
                acc.raw_plain_lines.push(semantic);
                acc.lines.push(line);
                acc.line_raw_overrides.push(Some(WrappedLineMap {
                    raw_line,
                    start_col: 0,
                    end_col: raw_width,
                }));
                acc.line_copy_offsets.push(prefix_width);
            }
        }
        "memory" => {
            let border_style = Style::default().fg(rgb(130, 140, 180));
            let text_style = Style::default().fg(dim_color());
            let entries = super::super::memory_ui::parse_memory_display_entries(&msg.content);

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
            let tile_lines =
                render_memory_tiles(&tiles, total_width, border_style, text_style, Some(header));
            for line in tile_lines {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "usage" => {
            let content_width = width.saturating_sub(4);
            let cached =
                get_cached_message_lines(msg, content_width, diff_mode, render_usage_message);
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "overnight" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                diff_mode,
                super::super::messages::render_overnight_message,
            );
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "todos" => {
            let content_width = width.saturating_sub(4);
            let cached = get_cached_message_lines(
                msg,
                content_width,
                diff_mode,
                super::super::messages::render_todos_message,
            );
            for line in cached {
                acc.push_auto(align_if_unset(line, align));
            }
        }
        "error" => {
            let error_start_line = acc.lines.len();
            if let Some(target) = error_copy_target(&msg.content, 1) {
                acc.copy_targets
                    .push(offset_copy_target(target, error_start_line));
            }
            // Multi-line diagnostics (cloud failures, error chains) keep one
            // logical line per row instead of collapsing into a single row.
            let prefix = if centered { "✗ " } else { "  ✗ " };
            let continuation = if centered { "  " } else { "    " };
            let prefix_width = unicode_width::UnicodeWidthStr::width(prefix);
            for (index, text) in msg.content.lines().enumerate() {
                let raw_line = acc.raw_plain_lines.len();
                acc.raw_plain_lines.push(text.to_string());
                let raw_width = unicode_width::UnicodeWidthStr::width(text);
                acc.lines.push(
                    Line::from(vec![
                        Span::styled(
                            if index == 0 { prefix } else { continuation },
                            Style::default().fg(Color::Red),
                        ),
                        Span::styled(text.to_string(), Style::default().fg(Color::Red)),
                    ])
                    .alignment(align),
                );
                acc.line_raw_overrides.push(Some(WrappedLineMap {
                    raw_line,
                    start_col: 0,
                    end_col: raw_width,
                }));
                acc.line_copy_offsets.push(prefix_width);
            }
        }
        _ => {}
    }

    acc.segments.push((
        msg.stable_cache_hash(),
        acc.lines.len(),
        acc.raw_plain_lines.len(),
        acc.user_prompt_texts.len(),
    ));
}
