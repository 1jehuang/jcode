//! Composer, status, and notification rendering.

use super::*;

pub(in super::super) fn draw_status(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    pending_count: usize,
) {
    if app.status_notice().as_deref() == Some(crate::telemetry::OAUTH_ACCOUNT_NOTICE_TEXT) {
        if !super::super::secondary_render_active() {
            crate::tui::app::oauth_telemetry::note_status_render(area);
        }
        frame.render_widget(
            Paragraph::new(crate::telemetry::OAUTH_ACCOUNT_NOTICE_TEXT)
                .style(Style::default().fg(accent_color())),
            area,
        );
        return;
    }
    let elapsed = app.elapsed().map(|d| d.as_secs_f32()).unwrap_or(0.0);
    let stale_secs = app.time_since_activity().map(|d| d.as_secs_f32());
    let (cache_read, cache_creation) = app.streaming_cache_tokens();
    let user_turn_count = app.display_user_message_count();
    let (streaming_input_tokens, _) = app.streaming_tokens();
    let provider_name = app.provider_name();
    let upstream_provider = app.upstream_provider();
    let cache_ttl = app.cache_ttl_status();
    let kv_cache_problem = detect_kv_cache_problem(
        &provider_name,
        upstream_provider.as_deref(),
        user_turn_count,
        streaming_input_tokens,
        cache_read,
        cache_creation,
        cache_ttl.as_ref(),
    );

    let queued_suffix = if pending_count > 0 {
        format!(" · +{} queued", pending_count)
    } else {
        String::new()
    };

    let line = if let Some(build_progress) = crate::build::read_build_progress() {
        let spinner = super::super::activity_indicator(elapsed, 12.5);
        Line::from(vec![
            Span::styled(spinner, Style::default().fg(rgb(255, 193, 7))),
            Span::styled(
                format!(" {}", build_progress),
                Style::default().fg(rgb(255, 193, 7)),
            ),
        ])
    } else if let Some(remaining) = app.rate_limit_remaining() {
        let secs = remaining.as_secs();
        let spinner = super::super::activity_indicator(elapsed, 4.0);
        let time_str = if secs >= 3600 {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            format!("{}h {}m", hours, mins)
        } else if secs >= 60 {
            let mins = secs / 60;
            let s = secs % 60;
            format!("{}m {}s", mins, s)
        } else {
            format!("{}s", secs)
        };
        Line::from(vec![
            Span::styled(spinner, Style::default().fg(rgb(255, 193, 7))),
            Span::styled(
                format!(
                    " Rate limited. Auto-retry in {}...{}",
                    time_str, queued_suffix
                ),
                Style::default().fg(rgb(255, 193, 7)),
            ),
        ])
    } else if app.is_processing() {
        let spinner = super::super::activity_indicator(elapsed, 12.5);

        match app.status() {
            ProcessingStatus::Idle => Line::from(""),
            ProcessingStatus::Sending => {
                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "sending".into();
                let mut spans = live_processing_status_line(&snapshot, stale_secs, spinner).spans;
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::Connecting(ref phase) => {
                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "connecting".into();
                snapshot.connection_phase = Some(connection_phase_label(phase));
                // "Suspiciously long" is measured per connection attempt, not
                // across the whole turn, so later round-trips don't immediately
                // render yellow just because the turn has been running a while.
                let phase_elapsed = app
                    .connection_phase_elapsed()
                    .map_or(elapsed, |d| d.as_secs_f32());
                let label_color = match phase {
                    crate::message::ConnectionPhase::Retrying { .. } => rgb(255, 193, 7),
                    crate::message::ConnectionPhase::Authenticating if phase_elapsed > 10.0 => {
                        rgb(255, 193, 7)
                    }
                    crate::message::ConnectionPhase::Connecting if phase_elapsed > 10.0 => {
                        rgb(255, 193, 7)
                    }
                    crate::message::ConnectionPhase::SendingRequest if phase_elapsed > 10.0 => {
                        rgb(255, 193, 7)
                    }
                    _ => dim_color(),
                };
                let mut spans = live_processing_status_line(&snapshot, stale_secs, spinner).spans;
                if let Some(label) = spans.get_mut(1) {
                    label.style = label.style.fg(label_color);
                }
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::Thinking(_start) => {
                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "thinking".into();
                let mut spans = live_processing_status_line(&snapshot, stale_secs, spinner).spans;
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::Streaming => {
                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "streaming".into();
                let mut spans = live_processing_status_line(&snapshot, stale_secs, spinner).spans;
                if let Some(upstream) = app.upstream_provider() {
                    spans.push(Span::styled(
                        format!(" · via {upstream}"),
                        Style::default().fg(dim_color()),
                    ));
                }
                if let Some(problem) = kv_cache_problem {
                    let miss_tokens = problem.affected_tokens.unwrap_or(0);
                    let miss_str = if miss_tokens >= 1000 {
                        format!("{}k", miss_tokens / 1000)
                    } else if miss_tokens > 0 {
                        format!("{}", miss_tokens)
                    } else {
                        "kv".to_string()
                    };
                    spans.insert(
                        1,
                        Span::styled(
                            format!(" ⚠ {miss_str} cache miss ·"),
                            Style::default().fg(rgb(255, 193, 7)),
                        ),
                    );
                }
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::WaitingForNetwork { listener } => {
                let mut spans = vec![
                    Span::styled("↻ ", Style::default().fg(rgb(255, 193, 7))),
                    Span::styled(
                        format!(
                            "network disconnected, waiting to retry · {} · {}",
                            listener,
                            format_elapsed(elapsed)
                        ),
                        Style::default().fg(rgb(255, 193, 7)),
                    ),
                ];
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::RunningTool(ref name) => {
                let anim_color = ai_color();
                let batch_prog = app.batch_progress();
                let is_batch = name == "batch";
                // For batch: compute initial total from the streaming tool call input
                let batch_total_initial = if is_batch {
                    app.streaming_tool_calls()
                        .last()
                        .and_then(|tc| tc.input.get("tool_calls"))
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                } else {
                    None
                };
                let tool_detail = if is_batch {
                    None // batch always uses progress display
                } else {
                    app.streaming_tool_calls()
                        .last()
                        .map(get_tool_activity_detail)
                        .filter(|s| !s.is_empty())
                };
                let experimental_notice = app.active_experimental_feature_notice();
                let subagent = app.subagent_status();

                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "running_tool".into();
                snapshot.tool_name = Some(name.clone());
                snapshot.tool_intent = tool_detail;
                let mut spans = live_processing_status_line(&snapshot, stale_secs, spinner).spans;

                // For batch tool: show "completed/total · last_tool" progress
                if is_batch {
                    append_batch_progress_spans(
                        &mut spans,
                        anim_color,
                        batch_prog,
                        batch_total_initial,
                    );
                }

                if let Some(notice) = experimental_notice {
                    spans.push(Span::styled(
                        format!(" · ⚠ {}", notice),
                        Style::default().fg(rgb(255, 193, 7)).bold(),
                    ));
                }

                if let Some(status) = subagent {
                    spans.push(Span::styled(
                        format!(" ({})", status),
                        Style::default().fg(dim_color()),
                    ));
                }
                if let Some(label) = app
                    .upstream_provider()
                    .map(|provider| format!("via {provider}"))
                {
                    spans.push(Span::styled(
                        format!(" · {}", label),
                        Style::default().fg(dim_color()),
                    ));
                }

                if let Some(problem) = kv_cache_problem {
                    let miss_tokens = problem.affected_tokens.unwrap_or(0);
                    let miss_str = if miss_tokens >= 1000 {
                        format!("{}k", miss_tokens / 1000)
                    } else if miss_tokens > 0 {
                        format!("{}", miss_tokens)
                    } else {
                        "kv".to_string()
                    };
                    spans.push(Span::styled(
                        format!(" · ⚠ {} cache miss", miss_str),
                        Style::default().fg(rgb(255, 193, 7)),
                    ));
                }

                spans.push(Span::styled(
                    format!(" · {} bg", jcode_tui_core::keybind::alt_chord("B")),
                    Style::default().fg(rgb(100, 100, 100)),
                ));

                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
        }
    } else if let Some((total_in, total_out)) = app.total_session_tokens() {
        let total = total_in + total_out;
        if let Some(warning) = occasional_session_history_warning(
            total,
            app.session_compaction_count(),
            app.context_limit(),
            area.width as usize,
            app.animation_elapsed() as u64,
        ) {
            let severe_token_threshold = app
                .context_limit()
                .and_then(|limit| u64::try_from(limit).ok())
                .map(|limit| limit.saturating_mul(3))
                .unwrap_or(1_000_000);
            let warning_color =
                if total >= severe_token_threshold || app.session_compaction_count() >= 3 {
                    rgb(255, 100, 100)
                } else {
                    rgb(255, 193, 7)
                };
            Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(warning_color)),
                Span::styled(warning, Style::default().fg(warning_color)),
            ])
        } else if let Some(tip) =
            occasional_status_tip(area.width as usize, app.animation_elapsed() as u64)
        {
            Line::from(vec![Span::styled(tip, Style::default().fg(dim_color()))])
        } else {
            Line::from("")
        }
    } else {
        if let Some(tip) =
            occasional_status_tip(area.width as usize, app.animation_elapsed() as u64)
        {
            Line::from(vec![Span::styled(tip, Style::default().fg(dim_color()))])
        } else {
            Line::from("")
        }
    };

    let line = status_with_background_tasks(
        line,
        app.background_task_rows(),
        area.width as usize,
        app.is_processing(),
    );

    crate::memory::check_staleness();

    if app.centered_mode() {
        frame.render_widget(Paragraph::new(line.alignment(Alignment::Center)), area);
        return;
    }
    frame.render_widget(Paragraph::new(line), area);
}

pub(in super::super) fn draw_input(
    frame: &mut Frame,
    app: &dyn TuiState,
    area: Rect,
    next_prompt: usize,
    debug_capture: &mut Option<FrameCaptureBuilder>,
) -> Option<Position> {
    let input_text = app.input();
    let cursor_pos = app.cursor_pos();

    let mode = composer_mode(input_text, app.is_remote_mode());
    // Command suggestions render later as an overlay pass
    // (`draw_command_suggestions_overlay`), never inline: they must not shift
    // the input rows or anything below them.
    let has_suggestions = command_suggestions_active(app, &app.command_suggestions());

    let (prompt_char, caret_color) = input_prompt(app);
    let num_str = format!("{}", next_prompt);
    let prompt_len = input_prompt_len(app, next_prompt);
    let reserved_width = send_mode_reserved_width(app);
    let line_width = (area.width as usize).saturating_sub(prompt_len + reserved_width);

    if line_width == 0 {
        return None;
    }

    let voice_preview = app.voice_composer_preview();
    let (all_lines, cursor_line, cursor_col) = wrap_input_text_with_ghost(
        input_text,
        cursor_pos,
        line_width,
        &num_str,
        prompt_char,
        caret_color,
        prompt_len,
        &|number| app.input_image_attached(number),
        voice_ghost(voice_preview.as_ref()),
    );

    let mut lines: Vec<Line> = Vec::new();
    let mut hint_shown = false;
    let mut hint_line: Option<String> = None;
    if let Some(preview) = voice_preview.as_ref() {
        hint_shown = true;
        let line = voice_waveform_line(preview, area.width as usize);
        hint_line = Some(line.spans.iter().map(|s| s.content.as_ref()).collect());
        lines.push(line);
    } else if let Some(hint) = app.login_input_hint() {
        hint_shown = true;
        hint_line = Some(hint.to_string());
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(accent_color()),
        )));
    } else if has_suggestions {
        // Overlay pass owns the palette rows; no inline hint line competes
        // with it.
    } else if let Some(shell_hint) = shell_mode_hint(mode) {
        hint_shown = true;
        hint_line = Some(shell_hint.trim().to_string());
        lines.push(Line::from(Span::styled(
            shell_hint,
            Style::default().fg(shell_mode_color()),
        )));
    } else if app.next_prompt_new_session_armed() {
        hint_shown = true;
        let hint = "  ↗ Next prompt opens a new session";
        hint_line = Some(hint.trim().to_string());
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(rgb(120, 200, 255)),
        )));
    } else if app.is_processing() && !input_text.is_empty() {
        hint_shown = true;
        let hint = if app.queue_mode() {
            "  Ctrl/Cmd+Enter to send now"
        } else {
            "  Ctrl/Cmd+Enter to queue"
        };
        hint_line = Some(hint.trim().to_string());
        lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(dim_color()),
        )));
    }

    if let Some(capture) = debug_capture {
        capture.rendered_text.input_area = input_text.to_string();
        if let Some(hint) = &hint_line {
            capture.rendered_text.input_hint = Some(hint.clone());
        }
        visual_debug::check_shift_enter_anomaly(
            capture,
            app.is_processing(),
            input_text,
            hint_shown,
        );
    }

    let suggestions_offset = lines.len();
    let total_input_lines = all_lines.len();
    let visible_height = area.height as usize;

    let scroll_offset = if let Some(scroll) = app.input_drag_scroll_offset() {
        // Selection scrolling is independent of the editing caret.
        let available = visible_height.saturating_sub(suggestions_offset).max(1);
        scroll.min(total_input_lines.saturating_sub(available))
    } else if total_input_lines + suggestions_offset <= visible_height {
        0
    } else {
        let available_for_input = visible_height.saturating_sub(suggestions_offset);
        if cursor_line < available_for_input {
            0
        } else {
            cursor_line.saturating_sub(available_for_input.saturating_sub(1))
        }
    };

    for line in all_lines.into_iter().skip(scroll_offset) {
        lines.push(line);
        if lines.len() >= visible_height {
            break;
        }
    }
    let visible_input_rows = lines.len().saturating_sub(suggestions_offset);

    let centered = app.centered_mode();

    // Register the composer's visible rows with the shared copy-selection
    // machinery so mouse drags can select and copy the text being typed
    // (issue #430). The prompt prefix / continuation indent is excluded via
    // per-row left margins, so hit-testing and the copied text only ever
    // cover the typed text, never the prompt decoration.
    if visible_input_rows > 0 {
        let (wrapped_plain, raw_plain, line_map) =
            input_copy_snapshot_parts(input_text, line_width);
        let left_margins: Vec<u16> = (0..visible_input_rows)
            .map(|rel| {
                let abs = scroll_offset + rel;
                let text_width = wrapped_plain
                    .get(abs)
                    .map(|text| unicode_width::UnicodeWidthStr::width(text.as_str()))
                    .unwrap_or(0);
                let mut margin = prompt_len;
                if centered {
                    margin += (area.width as usize).saturating_sub(prompt_len + text_width) / 2;
                }
                margin.min(area.width as usize) as u16
            })
            .collect();
        let input_rows_area = Rect::new(
            area.x,
            area.y.saturating_add(suggestions_offset as u16),
            area.width,
            visible_input_rows as u16,
        );
        super::super::record_input_copy_snapshot(
            wrapped_plain,
            raw_plain,
            line_map,
            scroll_offset,
            scroll_offset + visible_input_rows,
            input_rows_area,
            &left_margins,
        );
    }

    // Highlight an active copy selection over the composer text, mirroring the
    // chat/side-pane selection rendering. Columns are selection-space (typed
    // text only), so shift them right by the prompt width for display.
    if let Some(range) = app.copy_selection_range().filter(|range| {
        range.start.pane == crate::tui::CopySelectionPane::Input
            && range.end.pane == crate::tui::CopySelectionPane::Input
    }) {
        let (start, end) = if (range.start.abs_line, range.start.column)
            <= (range.end.abs_line, range.end.column)
        {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };
        for rel in 0..visible_input_rows {
            let abs = scroll_offset + rel;
            if abs < start.abs_line || abs > end.abs_line {
                continue;
            }
            if let Some(line) = lines.get_mut(suggestions_offset + rel) {
                let start_col = if abs == start.abs_line {
                    prompt_len + start.column
                } else {
                    prompt_len
                };
                let end_col = if abs == end.abs_line {
                    prompt_len + end.column
                } else {
                    line.width()
                };
                *line = highlight_line_selection(line, start_col, end_col);
            }
        }
    }

    let paragraph = if centered {
        Paragraph::new(
            lines
                .iter()
                .map(|l| l.clone().alignment(Alignment::Center))
                .collect::<Vec<_>>(),
        )
    } else {
        Paragraph::new(lines.clone())
    };
    frame.render_widget(paragraph, area);

    let cursor_screen_line = cursor_line.saturating_sub(scroll_offset) + suggestions_offset;
    let cursor_y = area.y + (cursor_screen_line as u16).min(area.height.saturating_sub(1));

    let cursor_x = if centered {
        let actual_line_width = lines
            .get(cursor_screen_line)
            .map(|l| l.width())
            .unwrap_or(prompt_len);
        let center_offset = (area.width as usize).saturating_sub(actual_line_width) / 2;
        let cursor_offset = prompt_len + cursor_col;
        area.x + center_offset as u16 + cursor_offset as u16
    } else {
        area.x + prompt_len as u16 + cursor_col as u16
    };

    let cursor = Position::new(cursor_x, cursor_y);
    draw_send_mode_indicator(frame, app, area);
    if app.input_drag_scroll_offset().is_some() {
        return None;
    }
    if !super::super::secondary_render_active() {
        frame.set_cursor_position(cursor);
    }
    Some(cursor)
}

pub(in super::super) fn build_notification_spans(app: &dyn TuiState) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();

    let push_sep = |spans: &mut Vec<Span<'static>>| {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", Style::default().fg(dim_color())));
        }
    };

    if let Some((recording, text)) = app.voice_input_status() {
        let color = if recording {
            rgb(255, 110, 110)
        } else {
            rgb(140, 200, 255)
        };
        spans.push(Span::styled(text, Style::default().fg(color).bold()));
    }

    if let Some(selection) = app.copy_selection_status() {
        let pane_label = selection.pane.label();
        let label = if selection.has_action {
            if selection.selected_lines > 1 {
                format!(
                    "{} selection · {} chars · {} lines · Enter/Y copy · Esc exit",
                    pane_label, selection.selected_chars, selection.selected_lines
                )
            } else {
                format!(
                    "{} selection · {} chars · Enter/Y copy · Esc exit",
                    pane_label, selection.selected_chars
                )
            }
        } else if selection.dragging {
            format!(
                "{} selection · dragging… · Enter/Y copy · Esc exit",
                pane_label
            )
        } else {
            format!("{} selection · drag to copy", pane_label)
        };
        spans.push(Span::styled(label, Style::default().fg(rgb(140, 220, 200))));
    }

    if let Some(flicker_notice) = super::super::recent_flicker_ui_notice() {
        let copy_badge_ui = app.copy_badge_ui();
        let copy_badge_now = std::time::Instant::now();
        let key = super::super::FLICKER_NOTICE_COPY_KEY;
        let alt_style = if copy_badge_ui.alt_active {
            Style::default().fg(accent_color()).bold()
        } else {
            Style::default().fg(dim_color())
        };
        let shift_style = if copy_badge_ui.shift_active {
            Style::default().fg(accent_color()).bold()
        } else {
            Style::default().fg(dim_color())
        };
        let key_style = if copy_badge_ui.key_is_active(key, copy_badge_now) {
            Style::default().fg(accent_color()).bold()
        } else {
            Style::default().fg(dim_color())
        };

        push_sep(&mut spans);
        spans.push(Span::styled(
            flicker_notice.summary,
            Style::default().fg(rgb(255, 193, 7)),
        ));
        push_sep(&mut spans);
        spans.push(Span::styled(
            flicker_notice.hint,
            Style::default().fg(rgb(140, 180, 255)),
        ));
        spans.push(Span::raw(" "));
        if let Some(success) = copy_badge_ui.feedback_for_key(key, copy_badge_now) {
            let feedback_style = if success {
                Style::default().fg(ai_color()).bold()
            } else {
                Style::default().fg(Color::Red).bold()
            };
            let feedback_text = if success {
                "✓ Copied! "
            } else {
                "✗ Copy failed "
            };
            spans.push(Span::styled(feedback_text, feedback_style));
        }
        spans.extend(super::super::viewport::shortcut_badge_spans(
            key,
            alt_style,
            shift_style,
            key_style,
        ));
    }

    if let Some(notice) = app.status_notice() {
        push_sep(&mut spans);
        spans.push(Span::styled(
            normalize_repaint_sensitive_notice_text(&notice),
            Style::default().fg(accent_color()),
        ));
    }

    // Learned-keybinding nudge: distinct bright color + bold so the user reads
    // it as "the system noticed I'm not using a shortcut", not a normal status.
    if let Some(hint) = app.learn_hint() {
        push_sep(&mut spans);
        spans.push(Span::styled(
            normalize_repaint_sensitive_notice_text(&hint),
            Style::default().fg(rgb(214, 122, 255)).bold(),
        ));
    }

    // Inline hotkey feedback: what a rarely-used chord just did, or the nearest
    // binding for an unbound chord. Cool cyan so it reads as informational.
    if let Some(feedback) = app.hotkey_feedback() {
        push_sep(&mut spans);
        spans.push(Span::styled(
            normalize_repaint_sensitive_notice_text(&feedback),
            Style::default().fg(rgb(102, 204, 221)),
        ));
    }

    if !app.is_processing() {
        let info = app.info_widget_data();
        if let Some(hint) = app.openai_reset_hint() {
            push_sep(&mut spans);
            spans.push(Span::styled(hint, Style::default().fg(rgb(255, 193, 7))));
        }
        if let Some(schedule_notice) =
            crate::tui::scheduled_notification_text(info.ambient_info.as_ref())
        {
            push_sep(&mut spans);
            spans.push(Span::styled(
                schedule_notice,
                Style::default().fg(rgb(140, 180, 255)),
            ));
        }

        if let Some(cache_info) = app.cache_ttl_status()
            && cache_info.expiry_notification_active()
        {
            if cache_info.is_cold {
                let tokens_str = cache_info.cached_tokens.map_or_else(String::new, |t| {
                    if t >= 1_000_000 {
                        format!(" ({:.1}M tok)", t as f64 / 1_000_000.0)
                    } else if t >= 1_000 {
                        format!(" ({}K tok)", t / 1000)
                    } else {
                        format!(" ({} tok)", t)
                    }
                });
                push_sep(&mut spans);
                spans.push(Span::styled(
                    format!("🧊 cache cold{}", tokens_str),
                    Style::default().fg(rgb(140, 180, 255)),
                ));
                // Small gray age since the retention window elapsed, e.g. `1h 1m`.
                spans.push(Span::styled(
                    format!(
                        " {}",
                        crate::tui::format_compact_age(cache_info.cold_for_secs)
                    ),
                    Style::default().fg(dim_color()),
                ));
            } else if cache_info.expiring_soon() {
                let tokens_str = cache_info.cached_tokens.map_or_else(String::new, |t| {
                    if t >= 1_000 {
                        format!(" {}K", t / 1000)
                    } else {
                        format!(" {}", t)
                    }
                });
                // Above a minute, a raw seconds count is hard to read at a
                // glance; show minutes granularity instead.
                let remaining = cache_info.remaining_secs;
                let time_str = if remaining > 60 {
                    format!("{}m", remaining.div_ceil(60))
                } else {
                    format!("{}s", remaining)
                };
                push_sep(&mut spans);
                spans.push(Span::styled(
                    format!("⏳ cache {}{}", time_str, tokens_str),
                    Style::default().fg(rgb(255, 193, 7)),
                ));
            }
        }
    }

    if app.has_stashed_input() {
        push_sep(&mut spans);
        spans.push(Span::styled(
            "📋 stash",
            Style::default().fg(rgb(255, 193, 7)),
        ));
    }

    spans
}
