//! Tool message rendering.

use super::*;

pub(crate) fn render_tool_message(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Vec<Line<'static>> {
    if let Some(lines) = render_scheduled_tool_message(msg, width) {
        return lines;
    }

    let centered = markdown::center_code_blocks();
    let mut token_badge = tool_output_token_badge(&msg.content);
    token_badge.label =
        tools_ui::tool_usage_label(&token_badge.label, msg.duration_secs, &msg.content);

    // A restored or remotely mirrored transcript can occasionally retain the
    // todo result while losing its paired ToolCall metadata. Recognize the
    // structured todo payload itself so the full card never disappears merely
    // because that display-only association was unavailable.
    let is_todo_tool = msg
        .tool_data
        .as_ref()
        .is_none_or(|tc| tools_ui::canonical_tool_name(&tc.name) == "todo");
    if is_todo_tool
        && !tools_ui::tool_output_looks_failed(&msg.content)
        && let Some(parsed) = parse_todo_tool_output(&msg.content)
    {
        if !parsed.goal_updates.is_empty() || parsed.plan_update.is_some() {
            return render_todo_assessment_updates(
                parsed.plan_update.as_ref(),
                &parsed.goal_updates,
                width,
            );
        }
        // An empty todo read (the model probing the list before planning) has
        // nothing to show. Rendering the "No tasks yet" placeholder card there
        // just adds transcript noise, so collapse it to a compact line.
        if parsed.todos.is_empty() && parsed.goals.is_empty() {
            return vec![Line::from(vec![
                Span::raw("  "),
                Span::styled("todo", Style::default().fg(todo_meta_color())),
                Span::styled("  no tasks", Style::default().fg(dim_color())),
            ])];
        }
        let payload = serde_json::json!({
            "todos": parsed.todos,
            "plan": parsed.plan,
            "goals": parsed.goals,
        })
        .to_string();
        return render_todos_message(
            &DisplayMessage::todos(payload),
            width,
            crate::config::DiffDisplayMode::Off,
        );
    }

    let mut lines: Vec<Line<'static>> = Vec::new();
    let Some(ref tc) = msg.tool_data else {
        return lines;
    };

    if tools_ui::is_memory_store_tool(tc) && !tools_ui::tool_output_looks_failed(&msg.content) {
        let content = tc
            .input
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let category = tc
            .input
            .get("category")
            .and_then(|v| v.as_str())
            .or_else(|| tc.input.get("tag").and_then(|v| v.as_str()))
            .unwrap_or("fact");
        let title = format!("🧠 saved ({}) · {}", category, token_badge.label.as_str());
        let border_style = Style::default().fg(rgb(255, 200, 100));
        let text_style = Style::default().fg(dim_color());
        let max_box = (width.saturating_sub(4) as usize).min(72);
        let inner_width = max_box.saturating_sub(4);

        let mut box_content: Vec<Line<'static>> = Vec::new();
        let text_display_width = unicode_width::UnicodeWidthStr::width(content);
        if text_display_width <= inner_width {
            box_content.push(Line::from(Span::styled(content.to_string(), text_style)));
        } else {
            for chunk in split_by_display_width(content, inner_width) {
                box_content.push(Line::from(Span::styled(chunk, text_style)));
            }
        }

        let box_lines = render_rounded_box(&title, box_content, max_box, border_style);
        for line in box_lines {
            lines.push(line);
        }
        if centered {
            left_pad_lines_for_centered_mode(&mut lines, width);
        }
        return lines;
    }

    if tools_ui::is_memory_recall_tool(tc) && !tools_ui::tool_output_looks_failed(&msg.content) {
        let border_style = Style::default().fg(rgb(150, 180, 255));
        let text_style = Style::default().fg(dim_color());

        let mut entries: Vec<(String, String)> = Vec::new();
        for line in msg.content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- [")
                && let Some(rest) = trimmed.strip_prefix("- [")
                && let Some(bracket_end) = rest.find(']')
            {
                let cat = rest[..bracket_end].to_string();
                let content = rest[bracket_end + 1..].trim();
                let content = if let Some(tag_start) = content.rfind(" [") {
                    content[..tag_start].trim()
                } else {
                    content
                };
                entries.push((cat, content.to_string()));
            }
        }

        if !entries.is_empty() {
            let count = entries.len();
            let tiles = group_into_tiles(entries);
            let header_text = format!(
                "🧠 recalled {} memor{} · {}",
                count,
                if count == 1 { "y" } else { "ies" },
                token_badge.label.as_str()
            );
            let header = Line::from(Span::styled(header_text, border_style));
            let total_width = (width.saturating_sub(4) as usize).min(120);
            let tile_lines =
                render_memory_tiles(&tiles, total_width, border_style, text_style, Some(header));
            for line in tile_lines {
                lines.push(line);
            }
            if centered {
                left_pad_lines_for_centered_mode(&mut lines, width);
            }
            return lines;
        }
    }

    let batch_counts = if tc.name == "batch" {
        tools_ui::parse_batch_completion_counts(&msg.content)
    } else {
        None
    };
    let is_error = if let Some(counts) = batch_counts {
        counts.failed > 0 && counts.succeeded == 0
    } else {
        tools_ui::tool_output_looks_failed(&msg.content)
    };
    let is_partial_batch = batch_counts
        .map(|counts| counts.failed > 0 && counts.succeeded > 0)
        .unwrap_or(false);

    let (icon, icon_color) = if is_partial_batch {
        ("⚠", rgb(214, 184, 92))
    } else if is_error {
        ("✗", rgb(220, 100, 100))
    } else {
        ("✓", rgb(100, 180, 100))
    };

    let is_edit_tool = tools_ui::is_edit_tool_name(&tc.name);
    let (additions, deletions) = if is_edit_tool {
        diff_change_counts_for_tool(tc, &msg.content)
    } else {
        (0, 0)
    };
    let has_diff_changes = additions > 0 || deletions > 0;
    let show_header_diff_counts = is_edit_tool
        && has_diff_changes
        && !(diff_mode.is_inline() && edit_tool_has_inline_diff(tc, &msg.content));

    let block_width = if centered {
        super::super::centered_content_block_width(width, 96)
    } else {
        width as usize
    };
    let row_width = block_width.saturating_sub(1);
    let display_name = tools_ui::resolve_display_tool_name(&tc.name).to_string();
    let base_prefix = format!("  {} {} ", icon, display_name);
    let token_suffix_width =
        UnicodeWidthStr::width(format!(" · {}", token_badge.label.as_str()).as_str());
    let edit_suffix_width = if show_header_diff_counts {
        UnicodeWidthStr::width(format!(" (+{} -{})", additions, deletions).as_str())
    } else {
        0
    };
    let reserved_summary_width = row_width
        .saturating_sub(UnicodeWidthStr::width(base_prefix.as_str()))
        .saturating_sub(token_suffix_width)
        .saturating_sub(edit_suffix_width);

    let intent = tc
        .intent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let intent_reserved_width = intent
        .map(|intent| UnicodeWidthStr::width(intent).saturating_add(3))
        .unwrap_or(0)
        .min(reserved_summary_width.saturating_sub(8));
    let technical_summary_width = reserved_summary_width.saturating_sub(intent_reserved_width);

    let summary = if let Some(counts) = batch_counts {
        if counts.failed > 0 {
            if counts.succeeded == 0 {
                format!("{}/{} failed", counts.failed, counts.total())
            } else {
                format!("{}/{} succeeded", counts.succeeded, counts.total())
            }
        } else if counts.total() == 1 {
            "1 call".to_string()
        } else {
            format!("{} calls", counts.total())
        }
    } else if tc.name == "subagent" {
        msg.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .map(|title| {
                super::super::line_plain_text(&super::super::truncate_line_with_ellipsis_to_width(
                    &Line::from(title.to_string()),
                    technical_summary_width,
                ))
            })
            .unwrap_or_else(|| {
                tools_ui::get_tool_summary_with_budget(tc, 50, Some(technical_summary_width))
            })
    } else {
        tools_ui::get_tool_summary_with_budget(tc, 50, Some(technical_summary_width))
    };
    // Edit rows read like every other tool row: the intent. The file path is
    // already shown on the inline diff header right below, so repeating it on
    // the row is noise. Failures keep their call description on the header.
    let edit_path_on_diff_header = is_edit_tool
        && !is_error
        && diff_mode.is_inline()
        && edit_tool_has_inline_diff(tc, &msg.content);
    let summary = if edit_path_on_diff_header {
        String::new()
    } else {
        summary
    };

    let mut tool_line = vec![
        Span::styled(format!("  {} ", icon), Style::default().fg(icon_color)),
        Span::styled(display_name, Style::default().fg(tool_color())),
    ];
    if let Some(intent) = intent {
        tool_line.push(Span::styled(" · ", Style::default().fg(dim_color())));
        tool_line.push(Span::styled(
            intent.to_string(),
            Style::default().fg(tool_color()),
        ));
        let show_detail = tools_ui::show_tool_call_details() || is_partial_batch;
        if show_detail && !summary.is_empty() && summary != intent {
            tool_line.push(Span::styled(" · ", Style::default().fg(dim_color())));
            tool_line.push(Span::styled(summary, Style::default().fg(dim_color())));
        }
    } else if !summary.is_empty() {
        tool_line.push(Span::styled(
            format!(" {}", summary),
            Style::default().fg(dim_color()),
        ));
    }
    if show_header_diff_counts {
        tool_line.push(Span::styled(" (", Style::default().fg(dim_color())));
        tool_line.push(Span::styled(
            format!("+{}", additions),
            Style::default().fg(diff_add_color()),
        ));
        tool_line.push(Span::styled(" ", Style::default().fg(dim_color())));
        tool_line.push(Span::styled(
            format!("-{}", deletions),
            Style::default().fg(diff_del_color()),
        ));
        tool_line.push(Span::styled(")", Style::default().fg(dim_color())));
    }
    let mut usage_spans = vec![Span::styled(" · ", Style::default().fg(dim_color()))];
    usage_spans.extend(tools_ui::tool_usage_spans(
        token_badge.label,
        token_badge.color,
        msg.duration_secs,
        &msg.content,
    ));
    let token_suffix = Line::from(usage_spans);

    let rendered_tool_line = super::super::truncate_line_preserving_suffix_to_width(
        &Line::from(tool_line),
        &token_suffix,
        row_width,
    );
    let rendered_tool_line_text = super::super::line_plain_text(&rendered_tool_line);
    lines.push(rendered_tool_line);
    lines.extend(tools_ui::codemode_preview(tc, row_width));
    if is_error && (tc.name != "batch" || batch_counts.is_none()) {
        lines.push(render_tool_failure_reason(&msg.content, row_width, 2));
        if centered {
            super::super::left_pad_lines_to_block_width(&mut lines, width, block_width);
        }
        return lines;
    }
    if let Some(draft_lines) = render_gmail_draft_card(tc, &msg.content, is_error, row_width) {
        lines.extend(draft_lines);
    }
    if let Some(discovery_lines) = render_discovery_card(tc, &msg.content, is_error, row_width) {
        lines.extend(discovery_lines);
    }

    // Optionally render the full agentgrep search output inline in the
    // transcript. Gated behind `display.show_agentgrep_output` (default false)
    // so most users keep the compact one-line summary.
    if tools_ui::canonical_tool_name(&tc.name) == "agentgrep"
        && crate::config::config().display.show_agentgrep_output
        && !msg.content.trim().is_empty()
    {
        for line in render_agentgrep_output_body(&msg.content, row_width) {
            lines.push(line);
        }
    }

    // Fallback command preview on a second line only when the row has no
    // intent. With an intent present, the command summary is inline-only: it
    // shows on the tool row when it fits and is dropped otherwise, never
    // spilling onto a second line.
    if tools_ui::canonical_tool_name(&tc.name) == "bash"
        && intent.is_none()
        && !rendered_tool_line_text.contains('$')
        && let Some(command) = tc.input.get("command").and_then(|v| v.as_str())
    {
        let detail_width = row_width.saturating_sub(4).max(1);
        let command_detail = tools_ui::get_tool_summary_with_budget(tc, 80, Some(detail_width));
        if !command_detail.trim().is_empty() {
            let detail_line = Line::from(vec![
                Span::raw("    "),
                Span::styled(command_detail, Style::default().fg(dim_color())),
            ]);
            lines.push(super::super::truncate_line_with_ellipsis_to_width(
                &detail_line,
                row_width,
            ));
        } else if !command.trim().is_empty() {
            let fallback = format!("$ {}", command.trim());
            let detail_line = Line::from(vec![
                Span::raw("    "),
                Span::styled(fallback, Style::default().fg(dim_color())),
            ]);
            lines.push(super::super::truncate_line_with_ellipsis_to_width(
                &detail_line,
                row_width,
            ));
        }
    }

    if tools_ui::canonical_tool_name(&tc.name) == "bash"
        && tools_ui::show_bash_output()
        && msg.content.trim() != "Command completed successfully (no output)"
    {
        const MAX_COLLAPSED_OUTPUT_LINES: usize = 3;
        let output_lines = msg.content.lines().filter(|line| !line.trim().is_empty());
        let total = output_lines.clone().count();
        for output in output_lines.skip(total.saturating_sub(MAX_COLLAPSED_OUTPUT_LINES)) {
            let output_line = Line::from(vec![
                Span::raw("      "),
                Span::styled(output.to_string(), Style::default().fg(dim_color())),
            ]);
            lines.push(super::super::truncate_line_with_ellipsis_to_width(
                &output_line,
                row_width,
            ));
        }
    }

    if tc.name == "batch"
        && let Some(calls) = tc.input.get("tool_calls").and_then(|v| v.as_array())
    {
        let sub_results = tools_ui::parse_batch_sub_outputs_by_index(&msg.content);

        for (i, call) in calls.iter().enumerate() {
            let raw_name = call
                .get("tool")
                .or_else(|| call.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let params = tools_ui::batch_subcall_params(call);
            let intent = tools_ui::batch_subcall_intent(call, &params);

            let sub_tc = ToolCall {
                id: String::new(),
                name: tools_ui::resolve_display_tool_name(raw_name).to_string(),
                input: params,
                intent,
                thought_signature: None,
            };

            let sub_result = sub_results.get(&(i + 1));
            let sub_errored = sub_result.map(|result| result.errored).unwrap_or_else(|| {
                batch_counts.is_some_and(|counts| {
                    counts.failed > 0 && counts.succeeded == 0 && counts.total() == calls.len()
                })
            });
            let (sub_icon, sub_icon_color) = if sub_errored {
                ("✗", rgb(220, 100, 100))
            } else {
                ("✓", rgb(100, 180, 100))
            };

            lines.push(tools_ui::render_batch_subcall_line(
                &sub_tc,
                sub_icon,
                sub_icon_color,
                50,
                Some(row_width),
                sub_result.map(|result| result.content.as_str()),
            ));

            if sub_errored {
                lines.push(render_tool_failure_reason(
                    sub_result
                        .map(|result| result.content.as_str())
                        .unwrap_or("Error: Tool call failed"),
                    row_width,
                    4,
                ));
                continue;
            }

            if let Some(result) = sub_result
                && let Some(mut draft_lines) = render_gmail_draft_card(
                    &sub_tc,
                    &result.content,
                    sub_errored,
                    row_width.saturating_sub(4),
                )
            {
                for line in &mut draft_lines {
                    line.spans.insert(0, Span::raw("    "));
                }
                lines.extend(draft_lines);
            }

            if let Some(result) = sub_result
                && let Some(mut discovery_lines) = render_discovery_card(
                    &sub_tc,
                    &result.content,
                    sub_errored,
                    row_width.saturating_sub(4),
                )
            {
                for line in &mut discovery_lines {
                    line.spans.insert(0, Span::raw("    "));
                }
                lines.extend(discovery_lines);
            }

            if tools_ui::canonical_tool_name(&sub_tc.name) == "todo"
                && !sub_errored
                && let Some(result) = sub_result
                && let Some(parsed) = parse_todo_tool_output(&result.content)
            {
                let nested_width = row_width.saturating_sub(4).max(1).min(u16::MAX as usize) as u16;
                let mut todo_lines =
                    if parsed.goal_updates.is_empty() && parsed.plan_update.is_none() {
                        let payload = serde_json::json!({
                            "todos": parsed.todos,
                            "plan": parsed.plan,
                            "goals": parsed.goals,
                        })
                        .to_string();
                        render_todos_message(
                            &DisplayMessage::todos(payload),
                            nested_width,
                            crate::config::DiffDisplayMode::Off,
                        )
                    } else {
                        render_todo_assessment_updates(
                            parsed.plan_update.as_ref(),
                            &parsed.goal_updates,
                            nested_width,
                        )
                    };
                for line in &mut todo_lines {
                    line.spans.insert(0, Span::raw("    "));
                }
                lines.extend(todo_lines);
            }
        }
    }

    if diff_mode.is_inline()
        && is_edit_tool
        && let Some(change_lines) = edit_tool_inline_diff_lines(tc, &msg.content)
    {
        let full_inline = diff_mode.is_full_inline();
        let file_path_for_ext = tc
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                tc.input
                    .get("patch_text")
                    .and_then(|v| v.as_str())
                    .and_then(
                        |patch_text| match tools_ui::edit_render_name(&tc.name, &tc.input) {
                            "apply_patch" => tools_ui::extract_apply_patch_primary_file(patch_text),
                            "patch" => tools_ui::extract_unified_patch_primary_file(patch_text),
                            _ => None,
                        },
                    )
            });
        const MAX_DIFF_LINES: usize = MAX_INLINE_DIFF_LINES;
        let total_changes = change_lines.len();
        let additions = change_lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Add)
            .count();
        let deletions = change_lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Del)
            .count();

        let (display_lines, truncated, half_point): (Vec<&ParsedDiffLine>, bool, usize) =
            if full_inline || total_changes <= MAX_DIFF_LINES {
                (change_lines.iter().collect(), false, usize::MAX)
            } else {
                let half = MAX_DIFF_LINES / 2;
                let mut result: Vec<&ParsedDiffLine> = change_lines.iter().take(half).collect();
                result.extend(change_lines.iter().skip(total_changes - half));
                (result, true, half)
            };

        let pad_str = "";

        let diff_file_path =
            |line: &ParsedDiffLine| line.file_path.clone().or_else(|| file_path_for_ext.clone());
        let first_file_path = display_lines.first().and_then(|line| diff_file_path(line));
        lines.push(inline_diff_header(
            "╭─",
            Some((additions, deletions)),
            first_file_path.as_deref(),
            row_width,
        ));

        let mut shown_truncation = false;
        let mut previous_file_path = first_file_path;
        let number_width = change_lines
            .iter()
            .map(|line| line.prefix.trim().trim_end_matches(['+', '-']).len())
            .max()
            .unwrap_or(0);

        for (i, line) in display_lines.iter().enumerate() {
            if truncated && !shown_truncation && i >= half_point {
                let skipped = total_changes - MAX_DIFF_LINES;
                lines.push(
                    Line::from(Span::styled(
                        format!("{}│ ... {} more changes ...", pad_str, skipped),
                        Style::default().fg(dim_color()),
                    ))
                    .alignment(ratatui::layout::Alignment::Left),
                );
                shown_truncation = true;
            }

            let current_file_path = diff_file_path(line);
            if i > 0 && current_file_path != previous_file_path {
                lines.push(inline_diff_header(
                    "├─",
                    None,
                    current_file_path.as_deref(),
                    row_width,
                ));
            }
            previous_file_path = current_file_path.clone();

            if full_inline {
                let file_ext = current_file_path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).extension())
                    .and_then(|e| e.to_str());
                lines.extend(expanded_diff_row(line, file_ext, number_width, row_width));
                continue;
            }

            let base_color = if line.kind == DiffLineKind::Add {
                diff_add_color()
            } else {
                diff_del_color()
            };

            let border_prefix = format!("{}│ ", pad_str);
            let prefix_visual_width = unicode_width::UnicodeWidthStr::width(border_prefix.as_str())
                + unicode_width::UnicodeWidthStr::width(line.prefix.as_str());
            let max_content_width = row_width.saturating_sub(prefix_visual_width);

            let mut spans: Vec<Span<'static>> = vec![
                Span::styled(border_prefix, Style::default().fg(dim_color())),
                Span::styled(line.prefix.clone(), Style::default().fg(base_color)),
            ];

            if !line.content.is_empty() {
                let content = &line.content;
                let file_ext = current_file_path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).extension())
                    .and_then(|e| e.to_str());
                let content_vis_width = unicode_width::UnicodeWidthStr::width(content.as_str());
                if !full_inline && max_content_width > 1 && content_vis_width > max_content_width {
                    let mut end = 0;
                    let mut vis_w = 0;
                    let limit = max_content_width.saturating_sub(1);
                    for (i, ch) in content.char_indices() {
                        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                        if vis_w + cw > limit {
                            break;
                        }
                        vis_w += cw;
                        end = i + ch.len_utf8();
                    }
                    let truncated = &content[..end];
                    let highlighted = markdown::highlight_line(truncated, file_ext);
                    for span in highlighted {
                        spans.push(tint_span_with_diff_color(span, base_color));
                    }
                    spans.push(Span::styled("…", Style::default().fg(dim_color())));
                } else {
                    let highlighted = markdown::highlight_line(content.as_str(), file_ext);
                    for span in highlighted {
                        spans.push(tint_span_with_diff_color(span, base_color));
                    }
                }
            }

            lines.push(Line::from(spans).alignment(ratatui::layout::Alignment::Left));
        }

        let footer = format!("{pad_str}╰─");
        lines.push(
            super::super::truncate_line_with_ellipsis_to_width(
                &Line::from(Span::styled(footer, Style::default().fg(dim_color()))),
                width as usize,
            )
            .alignment(ratatui::layout::Alignment::Left),
        );
    }

    if centered {
        super::super::left_pad_lines_to_block_width(&mut lines, width, block_width);
    }

    lines
}
