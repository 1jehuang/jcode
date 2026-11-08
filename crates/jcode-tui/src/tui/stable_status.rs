use super::*;
use crate::tui::info_widget::occasional_status_tip;
use crate::tui::ui::activity_indicator;

pub(super) fn draw_status(frame: &mut Frame, app: &dyn TuiState, area: Rect, pending_count: usize) {
    if app.status_notice().as_deref() == Some(crate::telemetry::OAUTH_ACCOUNT_NOTICE_TEXT) {
        crate::tui::app::oauth_telemetry::note_status_render(area);
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
        let spinner = activity_indicator(elapsed, 12.5);
        let mut snapshot = live_status_snapshot(app, elapsed);
        snapshot.phase = "building".into();
        let mut spans = composer_processing_status_line(&snapshot, stale_secs, spinner).spans;
        spans.push(Span::styled(
            format!(" · {build_progress}"),
            Style::default().fg(rgb(255, 193, 7)),
        ));
        push_queued_suffix(&mut spans, &queued_suffix);
        Line::from(spans)
    } else if let Some(remaining) = app.rate_limit_remaining() {
        let secs = remaining.as_secs();
        let spinner = activity_indicator(elapsed, 4.0);
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
        let mut snapshot = live_status_snapshot(app, elapsed);
        snapshot.phase = "retry".into();
        let mut spans = composer_processing_status_line(&snapshot, stale_secs, spinner).spans;
        spans.push(Span::styled(
            format!(" · Rate limited. Auto-retry in {time_str}..."),
            Style::default().fg(rgb(255, 193, 7)),
        ));
        push_queued_suffix(&mut spans, &queued_suffix);
        Line::from(spans)
    } else if app.is_processing() {
        let spinner = activity_indicator(elapsed, 12.5);

        match app.status() {
            ProcessingStatus::Idle => Line::from(""),
            ProcessingStatus::Sending => {
                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "sending".into();
                let mut spans =
                    composer_processing_status_line(&snapshot, stale_secs, spinner).spans;
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
                let mut spans =
                    composer_processing_status_line(&snapshot, stale_secs, spinner).spans;
                if let Some(label) = spans.get_mut(1) {
                    label.style = label.style.fg(label_color);
                }
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::Thinking(_start) => {
                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "thinking".into();
                let mut spans =
                    composer_processing_status_line(&snapshot, stale_secs, spinner).spans;
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::Streaming => {
                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "streaming".into();
                let mut spans =
                    composer_processing_status_line(&snapshot, stale_secs, spinner).spans;
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
                    spans.push(Span::styled(
                        format!(" · ⚠ {miss_str} cache miss"),
                        Style::default().fg(rgb(255, 193, 7)),
                    ));
                }
                push_queued_suffix(&mut spans, &queued_suffix);
                Line::from(spans)
            }
            ProcessingStatus::WaitingForNetwork { listener } => {
                let mut snapshot = live_status_snapshot(app, elapsed);
                snapshot.phase = "waiting_network".into();
                let mut spans =
                    composer_processing_status_line(&snapshot, stale_secs, spinner).spans;
                spans.push(Span::styled(
                    format!(" · {listener}"),
                    Style::default().fg(rgb(255, 193, 7)),
                ));
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
                let mut spans =
                    composer_processing_status_line(&snapshot, stale_secs, spinner).spans;

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

    frame.render_widget(Paragraph::new(line), area);
}

/// Background launches finish their tool call immediately, but the work and
/// its intent must remain visible while the agent moves on to another tool.
fn status_with_background_tasks(
    mut foreground: Line<'static>,
    tasks: &[crate::tui::BackgroundTaskRow],
    width: usize,
    processing: bool,
) -> Line<'static> {
    use crate::tui::BackgroundTaskRowStatus;
    use jcode_tui_render::truncate_line_with_ellipsis_to_width as fit;

    for span in &mut foreground.spans {
        // Preserve intentional prefix spacing while stripping terminal controls.
        span.content = span.content.chars().map(|c| {
            if c.is_control() || c.is_whitespace()
                || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            { ' ' } else { c }
        }).collect::<String>().into();
    }
    let running: Vec<_> = tasks
        .iter()
        .filter(|task| task.status == BackgroundTaskRowStatus::Running)
        .collect();
    if running.is_empty() {
        return fit(&foreground, width);
    }
    let labels = running
        .iter()
        .map(|task| compact_status::status_single_line(&task.label))
        .collect::<Vec<_>>()
        .join(" · ");
    let background = Line::from(Span::styled(
        format!("bg {}: {}", running.len(), labels),
        Style::default().fg(ai_color()),
    ));
    // Idle tips must not displace actual work. While processing, reserve space
    // for both activities rather than letting a long foreground intent hide bg.
    if !processing || foreground.width() == 0 || width < 24 {
        return fit(&background, width);
    }
    let background_width = background.width().min(width / 2);
    let mut line = fit(&foreground, width.saturating_sub(background_width + 3));
    line.spans
        .push(Span::styled(" · ", Style::default().fg(dim_color())));
    line.spans.extend(fit(&background, background_width).spans);
    line
}
