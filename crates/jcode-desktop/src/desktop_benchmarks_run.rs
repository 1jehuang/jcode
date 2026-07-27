use super::*;

pub(crate) fn run_headless_chat_smoke(message: String) -> Result<()> {
    if message.trim().is_empty() {
        anyhow::bail!("headless chat smoke message cannot be empty");
    }

    let (event_tx, event_rx) = mpsc::channel();
    let _handle = session_launch::spawn_fresh_server_session(message, Vec::new(), event_tx)
        .context("failed to start desktop headless chat smoke")?;
    let started = Instant::now();
    let mut session_id = None;
    let mut response = String::new();
    let mut last_status = None;

    while started.elapsed() < HEADLESS_CHAT_SMOKE_TIMEOUT {
        let remaining = HEADLESS_CHAT_SMOKE_TIMEOUT.saturating_sub(started.elapsed());
        let poll = remaining.min(Duration::from_millis(250));
        let event = match event_rx.recv_timeout(poll) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!(
                    "desktop chat smoke worker disconnected before completion; last_status={}",
                    last_status.as_deref().unwrap_or("unknown")
                );
            }
        };

        match event {
            session_launch::DesktopSessionEvent::Status(status) => {
                let status = status.label();
                last_status = Some(status.clone());
                println!(
                    "{}",
                    serde_json::json!({"event": "status", "status": status})
                );
            }
            session_launch::DesktopSessionEvent::SessionStarted { session_id: id } => {
                session_id = Some(id.clone());
                println!(
                    "{}",
                    serde_json::json!({"event": "session", "session_id": id})
                );
            }
            session_launch::DesktopSessionEvent::SessionRenamed {
                title,
                display_title,
            } => {
                last_status = Some(if title.is_some() {
                    format!("renamed session to {display_title}")
                } else {
                    format!("cleared session name; title is now {display_title}")
                });
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "session_renamed",
                        "title": title,
                        "display_title": display_title,
                    })
                );
            }
            session_launch::DesktopSessionEvent::Reloaded { session_id: id } => {
                session_id = Some(id.clone());
                last_status = Some("server reconnected".to_string());
                println!(
                    "{}",
                    serde_json::json!({"event": "reloaded", "session_id": id})
                );
            }
            session_launch::DesktopSessionEvent::TextDelta(text) => {
                response.push_str(&text);
                println!(
                    "{}",
                    serde_json::json!({"event": "text_delta", "chars": text.chars().count()})
                );
            }
            session_launch::DesktopSessionEvent::ReasoningDelta(text) => {
                println!(
                    "{}",
                    serde_json::json!({"event": "reasoning_delta", "chars": text.chars().count()})
                );
            }
            session_launch::DesktopSessionEvent::ReasoningDone { duration_ms } => {
                println!(
                    "{}",
                    serde_json::json!({"event": "reasoning_done", "duration_ms": duration_ms})
                );
            }
            session_launch::DesktopSessionEvent::TextReplace(text) => {
                response = text;
                println!(
                    "{}",
                    serde_json::json!({"event": "text_replace", "chars": response.chars().count()})
                );
            }
            session_launch::DesktopSessionEvent::ToolStarted { id, name } => {
                last_status = Some(format!("preparing tool {name}"));
                println!(
                    "{}",
                    serde_json::json!({"event": "tool_started", "id": id, "name": name})
                );
            }
            session_launch::DesktopSessionEvent::ToolExecuting { id, name } => {
                last_status = Some(format!("using tool {name}"));
                println!(
                    "{}",
                    serde_json::json!({"event": "tool_executing", "id": id, "name": name})
                );
            }
            session_launch::DesktopSessionEvent::ToolInput { id, delta } => {
                println!(
                    "{}",
                    serde_json::json!({"event": "tool_input", "id": id, "chars": delta.chars().count()})
                );
            }
            session_launch::DesktopSessionEvent::ToolFinished {
                id,
                name,
                summary,
                is_error,
            } => {
                last_status = Some(if is_error {
                    format!("tool {name} failed")
                } else {
                    format!("tool {name} done")
                });
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "tool_finished",
                        "id": id,
                        "name": name,
                        "summary": summary,
                        "is_error": is_error,
                    })
                );
            }
            session_launch::DesktopSessionEvent::Reloading { new_socket } => {
                last_status = Some("server reloading, reconnecting".to_string());
                println!(
                    "{}",
                    serde_json::json!({"event": "reloading", "new_socket": new_socket})
                );
            }
            session_launch::DesktopSessionEvent::ModelChanged {
                model,
                provider_name,
                error,
            } => {
                if let Some(error) = error {
                    last_status = Some(format!("model switch failed: {error}"));
                    println!(
                        "{}",
                        serde_json::json!({
                            "event": "model_changed",
                            "model": model,
                            "provider_name": provider_name,
                            "error": error,
                        })
                    );
                    continue;
                }
                let label = provider_name
                    .as_deref()
                    .map(|provider| format!("{provider} · {model}"))
                    .unwrap_or_else(|| model.clone());
                last_status = Some(format!("model: {label}"));
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "model_changed",
                        "model": model,
                        "provider_name": provider_name,
                    })
                );
            }
            session_launch::DesktopSessionEvent::ModelCatalog {
                current_model,
                provider_name,
                models,
                ..
            } => {
                last_status = Some(format!("models loaded ({})", models.len()));
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "model_catalog",
                        "current_model": current_model,
                        "provider_name": provider_name,
                        "models": models.len(),
                    })
                );
            }
            session_launch::DesktopSessionEvent::ModelCatalogError { error } => {
                last_status = Some(format!("model picker error: {error}"));
                println!(
                    "{}",
                    serde_json::json!({"event": "model_catalog_error", "error": error})
                );
            }
            session_launch::DesktopSessionEvent::StdinRequest {
                request_id,
                prompt,
                is_password,
                tool_call_id,
            } => {
                last_status = Some("interactive input requested".to_string());
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "stdin_request",
                        "request_id": request_id,
                        "prompt": prompt,
                        "is_password": is_password,
                        "tool_call_id": tool_call_id,
                    })
                );
            }
            session_launch::DesktopSessionEvent::ReloadProgress {
                step,
                message,
                success,
                output,
            } => {
                last_status = Some(format!("reload {step}: {message}"));
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "reload_progress",
                        "step": step,
                        "message": message,
                        "success": success,
                        "output": output,
                    })
                );
            }
            session_launch::DesktopSessionEvent::RuntimeMetadata {
                connection_type,
                status_detail,
                upstream_provider,
            } => {
                if let Some(status_detail) = &status_detail {
                    last_status = Some(status_detail.clone());
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "runtime_metadata",
                        "connection_type": connection_type,
                        "status_detail": status_detail,
                        "upstream_provider": upstream_provider,
                    })
                );
            }
            session_launch::DesktopSessionEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                cache_creation_input,
            } => {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "tokens",
                        "input": input,
                        "output": output,
                        "cache_read_input": cache_read_input,
                        "cache_creation_input": cache_creation_input,
                    })
                );
            }
            session_launch::DesktopSessionEvent::SystemNotice { title, message } => {
                last_status = Some(title.clone());
                println!(
                    "{}",
                    serde_json::json!({"event": "system_notice", "title": title, "message": message})
                );
            }
            session_launch::DesktopSessionEvent::SessionCloseRequested { reason } => {
                anyhow::bail!(
                    "desktop chat smoke session close requested; session_id={}; reason={}",
                    session_id.as_deref().unwrap_or("unknown"),
                    reason
                );
            }
            session_launch::DesktopSessionEvent::Done => {
                let response = response.trim().to_string();
                if response.is_empty() {
                    anyhow::bail!(
                        "desktop chat smoke completed without assistant text; session_id={}; last_status={}",
                        session_id.as_deref().unwrap_or("unknown"),
                        last_status.as_deref().unwrap_or("unknown")
                    );
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "ok",
                        "session_id": session_id,
                        "response_chars": response.chars().count(),
                        "response_preview": response.chars().take(240).collect::<String>(),
                    })
                );
                return Ok(());
            }
            session_launch::DesktopSessionEvent::Error(error) => {
                anyhow::bail!(
                    "desktop chat smoke failed; session_id={}; error={}",
                    session_id.as_deref().unwrap_or("unknown"),
                    error
                );
            }
        }
    }

    anyhow::bail!(
        "desktop chat smoke timed out after {:?}; session_id={}; response_chars={}; last_status={}",
        HEADLESS_CHAT_SMOKE_TIMEOUT,
        session_id.as_deref().unwrap_or("unknown"),
        response.chars().count(),
        last_status.as_deref().unwrap_or("unknown")
    )
}

pub(crate) fn run_resize_render_benchmark(frames: usize) -> Result<()> {
    let frames = frames.max(1);
    let target_p95_ms = 16.0;
    let target_max_ms = 33.0;
    let base_size = PhysicalSize::new(1200, 760);
    let mut app = desktop_large_transcript_benchmark_app();
    let initial_body_lines = single_session_rendered_body_lines_for_tick(&app, base_size, 0);
    if let Some(metrics) = single_session_body_scroll_metrics_for_total_lines(
        &app,
        base_size,
        initial_body_lines.len(),
    ) {
        app.body_scroll_lines = metrics.max_scroll_lines as f32 / 2.0;
    }
    let sizes = (0..frames).map(benchmark_resize_size).collect::<Vec<_>>();

    let mut legacy_font_system = benchmark_font_system();
    let (legacy_samples, legacy_checksum) = benchmark_frame_samples(frames, |frame| {
        let size = sizes[frame];
        let tick = frame as u64;
        let key = single_session_text_key_for_tick_with_scroll(&app, size, tick, 0.0);
        let buffers = single_session_text_buffers_from_key(&key, size, &mut legacy_font_system);
        let areas = single_session_text_areas_for_app_with_scroll(&app, &buffers, size, tick, 0.0);
        let body_glyphs = buffers
            .get(1)
            .map(|buffer| {
                buffer
                    .layout_runs()
                    .map(|run| run.glyphs.len())
                    .sum::<usize>()
            })
            .unwrap_or_default();
        let vertices =
            build_single_session_vertices_with_scroll_and_reveal(&app, size, 0.0, tick, 0.0, 1.0);
        key.body.len() ^ buffers.len() ^ areas.len() ^ vertices.len() ^ body_glyphs
    });

    let mut optimized_font_system = benchmark_font_system();
    let mut optimized_raw_body_key = Some(app.rendered_body_cache_key((0, 0)));
    let mut optimized_raw_body_lines = app.body_styled_lines_for_tick(0);
    let mut optimized_body_key = None;
    let mut optimized_body_lines = Vec::new();
    let mut optimized_text_cache_key = None;
    let mut optimized_text_key = None;
    let mut optimized_buffers: Vec<Buffer> = Vec::new();
    let mut optimized_window_start = None;
    let mut optimized_window_end = None;
    let mut optimized_body_rebuilds = 0usize;
    let mut optimized_body_wraps = 0usize;
    let (optimized_samples, optimized_checksum) = benchmark_frame_samples(frames, |frame| {
        let size = sizes[frame];
        let tick = frame as u64;
        let body_layout_size = single_session_body_layout_cache_size(&app, size);
        let body_key = app.rendered_body_cache_key(body_layout_size);
        let rendered_body_changed = if optimized_body_key != Some(body_key) {
            let raw_body_key = app.rendered_body_cache_key((0, 0));
            if optimized_raw_body_key != Some(raw_body_key) {
                optimized_raw_body_lines = app.body_styled_lines_for_tick(tick);
                optimized_raw_body_key = Some(raw_body_key);
            }
            optimized_body_lines = single_session_rendered_body_lines_from_raw_ref(
                &app,
                size,
                &optimized_raw_body_lines,
            );
            optimized_body_key = Some(body_key);
            optimized_window_start = None;
            optimized_window_end = None;
            optimized_body_wraps += 1;
            true
        } else {
            false
        };

        let viewport =
            single_session_body_viewport_from_lines(&app, size, 0.0, &optimized_body_lines);
        let text_cache_key = single_session_text_buffer_cache_key(&app, size, tick, body_key);
        let key = single_session_text_key_for_tick_with_rendered_body(
            &app,
            size,
            tick,
            0.0,
            &optimized_body_lines,
        );
        let text_key_changed = optimized_text_key.as_ref() != Some(&key);
        if optimized_text_cache_key != Some(text_cache_key) || text_key_changed {
            let desired_body_window = single_session_body_text_window_bounds(&viewport);
            let body_window_contains = if let (Some(window_start), Some(window_end)) =
                (optimized_window_start, optimized_window_end)
            {
                single_session_body_text_window_contains(window_start, window_end, &viewport)
            } else {
                false
            };
            let previous_key = optimized_text_key.take();
            let mut old_buffers = std::mem::take(&mut optimized_buffers);
            let body_content_changed_in_buffer =
                rendered_body_changed && app.streaming_response.is_empty();
            let body_layout_compatible = previous_key.as_ref().is_some_and(|previous| {
                single_session_body_text_buffer_layout_compatible(
                    previous.size,
                    size,
                    app.text_scale(),
                )
            });
            let mut can_reuse_body_buffer = old_buffers.len() > 1
                && body_window_contains
                && !body_content_changed_in_buffer
                && body_layout_compatible;
            if old_buffers.len() > 1
                && (!body_window_contains
                    || body_content_changed_in_buffer
                    || !body_layout_compatible)
            {
                let (window_start, window_end) = desired_body_window;
                old_buffers[1] = single_session_body_text_buffer_from_lines(
                    &mut optimized_font_system,
                    &optimized_body_lines[window_start..window_end],
                    size,
                    app.text_scale(),
                );
                optimized_window_start = Some(window_start);
                optimized_window_end = Some(window_end);
                optimized_body_rebuilds += 1;
                can_reuse_body_buffer = true;
            }
            optimized_buffers = single_session_text_buffers_from_key_reusing_unchanged(
                &key,
                previous_key.as_ref(),
                old_buffers,
                can_reuse_body_buffer,
                size,
                &mut optimized_font_system,
            );
            optimized_text_key = Some(key);
            optimized_text_cache_key = Some(text_cache_key);
            if !can_reuse_body_buffer {
                optimized_window_start = None;
                optimized_window_end = None;
            }
        }

        let viewport =
            single_session_body_viewport_from_lines(&app, size, 0.0, &optimized_body_lines);
        if let (Some(window_start), Some(window_end)) =
            (optimized_window_start, optimized_window_end)
            && single_session_body_text_window_contains(window_start, window_end, &viewport)
        {
            if let Some(body_buffer) = optimized_buffers.get_mut(1) {
                body_buffer.set_scroll(
                    viewport
                        .start_line
                        .saturating_sub(window_start)
                        .min(i32::MAX as usize) as i32,
                );
            }
        } else {
            let (window_start, window_end) = single_session_body_text_window_bounds(&viewport);
            if let Some(body_buffer) = optimized_buffers.get_mut(1) {
                *body_buffer = single_session_body_text_buffer_from_lines(
                    &mut optimized_font_system,
                    &optimized_body_lines[window_start..window_end],
                    size,
                    app.text_scale(),
                );
                body_buffer.set_scroll(
                    viewport
                        .start_line
                        .saturating_sub(window_start)
                        .min(i32::MAX as usize) as i32,
                );
                optimized_body_rebuilds += 1;
            }
            optimized_window_start = Some(window_start);
            optimized_window_end = Some(window_end);
        }

        let areas = single_session_text_areas_for_app_with_cached_body_viewport(
            &app,
            &optimized_buffers,
            size,
            0.0,
            viewport,
        );
        let body_glyphs = optimized_buffers
            .get(1)
            .map(|buffer| {
                buffer
                    .layout_runs()
                    .map(|run| run.glyphs.len())
                    .sum::<usize>()
            })
            .unwrap_or_default();
        let vertices = build_single_session_vertices_with_cached_body(
            &app,
            size,
            0.0,
            tick,
            0.0,
            1.0,
            &optimized_body_lines,
        );
        optimized_body_lines.len()
            ^ optimized_buffers.len()
            ^ areas.len()
            ^ vertices.len()
            ^ body_glyphs
    });

    let mut workspace_resize_cards = benchmark_workspace_session_cards(16);
    for (index, card) in workspace_resize_cards.iter_mut().enumerate() {
        card.transcript_messages = vec![
            workspace::SessionTranscriptMessage {
                role: "user".to_string(),
                content: format!("resize prompt {index}"),
            },
            workspace::SessionTranscriptMessage {
                role: "assistant".to_string(),
                content: format!(
                    "resize response {index}: representative workspace transcript content that wraps while the window is resized"
                ),
            },
        ];
    }
    let workspace_resize_app = Workspace::from_session_cards(workspace_resize_cards);
    let mut workspace_resize_font_system = benchmark_font_system();
    let mut workspace_resize_text_pane_cache = HashMap::new();
    let (workspace_resize_samples, workspace_resize_checksum) =
        benchmark_frame_samples(frames, |frame| {
            let size = sizes[frame];
            let layout = workspace_render_layout(&workspace_resize_app, size, Some(size));
            let panes = build_workspace_single_session_text_panes(
                &mut workspace_resize_text_pane_cache,
                &workspace_resize_app,
                size,
                layout,
                None,
                &mut workspace_resize_font_system,
            );
            let pane_count = panes.len();
            let areas = workspace_single_session_text_areas(&panes);
            let area_count = areas.len();
            drop(areas);
            drop(panes);
            let mut vertices =
                Vec::with_capacity(workspace_vertex_capacity_hint(&workspace_resize_app));
            build_vertices_into(
                WorkspaceVertexBuildParams {
                    workspace: &workspace_resize_app,
                    size,
                    render_layout: layout,
                    focus_pulse: 0.0,
                    space_hold_progress: None,
                    surface_frames: None,
                    exiting_surfaces: &HashMap::new(),
                    workspace_panel_cache: Some(&workspace_resize_text_pane_cache),
                    status_color: workspace_status_bar_target_color(&workspace_resize_app),
                    status_text_frame: None,
                },
                &mut vertices,
            );
            pane_count ^ area_count ^ vertices.len() ^ workspace_resize_app.surfaces.len()
        });

    let optimized_p95 = percentile_ms(&optimized_samples, 0.95);
    let optimized_max = max_sample_ms(&optimized_samples);
    let workspace_resize_p95 = percentile_ms(&workspace_resize_samples, 0.95);
    let workspace_resize_max = max_sample_ms(&workspace_resize_samples);
    let passes_workspace_resize_budget =
        workspace_resize_p95 <= target_p95_ms && workspace_resize_max <= target_max_ms;
    let passes_resize_cpu_budget = optimized_p95 <= target_p95_ms
        && optimized_max <= target_max_ms
        && passes_workspace_resize_budget;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "frames": frames,
            "target_p95_ms": target_p95_ms,
            "target_repeated_max_ms": target_max_ms,
            "passes_resize_cpu_budget": passes_resize_cpu_budget,
            "passes_workspace_resize_budget": passes_workspace_resize_budget,
            "scenario": "large transcript and workspace continuous resize CPU layout paths",
            "size_range": {
                "min_width": sizes.iter().map(|size| size.width).min().unwrap_or_default(),
                "max_width": sizes.iter().map(|size| size.width).max().unwrap_or_default(),
                "min_height": sizes.iter().map(|size| size.height).min().unwrap_or_default(),
                "max_height": sizes.iter().map(|size| size.height).max().unwrap_or_default(),
            },
            "optimized_body_wraps": optimized_body_wraps,
            "optimized_body_buffer_rebuilds": optimized_body_rebuilds,
            "legacy": benchmark_samples_json("legacy_resize_full_text_relayout", &legacy_samples, legacy_checksum),
            "optimized": benchmark_samples_json("optimized_resize_cached_visible_body", &optimized_samples, optimized_checksum),
            "workspace_multi_pane": benchmark_samples_json("workspace_multi_pane_resize_reflow", &workspace_resize_samples, workspace_resize_checksum),
        }))?
    );
    Ok(())
}

pub(crate) fn run_stream_e2e_benchmark(raw_events: usize) -> Result<()> {
    let result = run_desktop_stream_end_to_end_benchmark(raw_events);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "target_frame_budget_ms": duration_ms(DESKTOP_120FPS_FRAME_BUDGET),
            "no_paint_budget_ms": duration_ms(DESKTOP_NO_PAINT_BUDGET),
            "event_delivery": {
                "backend_redraw_frame_interval_ms": duration_ms(BACKEND_REDRAW_FRAME_INTERVAL),
                "backend_event_forward_interval_ms": duration_ms(BACKEND_EVENT_FORWARD_INTERVAL),
                "backend_event_forward_max_raw_events": BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS,
                "backend_event_forward_max_payload_bytes": BACKEND_EVENT_FORWARD_MAX_PAYLOAD_BYTES,
            },
            "passes_120fps_interaction_cpu_budget": result.passes_interaction_budget(),
            "passes_no_paint_watchdog_budget": result.passes_no_paint_budget(),
            "end_to_end_stream_flood": result.to_json(),
        }))?
    );
    Ok(())
}

pub(crate) fn benchmark_hero_boundary_scroll_lines(
    app: &SingleSessionApp,
    size: PhysicalSize<u32>,
    body_lines: &[SingleSessionStyledLine],
) -> f32 {
    let Some(metrics) =
        single_session_body_scroll_metrics_for_total_lines(app, size, body_lines.len())
    else {
        return 0.0;
    };
    let mut probe = app.clone();
    for scroll in 0..=metrics.max_scroll_lines {
        probe.body_scroll_lines = scroll as f32;
        let key =
            single_session_text_key_for_tick_with_rendered_body(&probe, size, 0, 0.0, body_lines);
        if key.fresh_welcome_visible {
            return scroll.saturating_sub(6) as f32;
        }
    }
    metrics.max_scroll_lines.saturating_sub(12) as f32
}

pub(crate) fn benchmark_font_system() -> FontSystem {
    create_desktop_font_system()
}

pub(crate) fn desktop_scroll_benchmark_app() -> SingleSessionApp {
    desktop_scroll_benchmark_app_with_turns(320)
}

pub(crate) fn desktop_large_transcript_benchmark_app() -> SingleSessionApp {
    desktop_scroll_benchmark_app_with_turns(2_000)
}

pub(crate) fn benchmark_workspace_session_cards(count: usize) -> Vec<workspace::SessionCard> {
    (0..count)
        .map(|index| workspace::SessionCard {
            session_id: format!("benchmark-session-{index}"),
            title: format!("agent {index} · desktop benchmark"),
            subtitle: format!("workspace surface {index}"),
            detail: "rendering session metadata, preview lines, and detail transcript".to_string(),
            preview_lines: vec![
                "recent prompt: inspect render latency and input jank".to_string(),
                "assistant: caching text and geometry keeps navigation responsive".to_string(),
                format!("status: benchmark card {index}"),
            ],
            detail_lines: (0..16)
                .map(|line| {
                    format!(
                        "detail line {line}: this synthetic transcript line exercises zoom/detail rendering for card {index}"
                    )
                })
                .collect(),
            transcript_messages: Vec::new(),
        })
        .collect()
}

pub(crate) fn desktop_scroll_benchmark_app_with_turns(turns: usize) -> SingleSessionApp {
    let mut app = SingleSessionApp::new(None);
    app.draft = "summarize the latest changes and suggest the next optimization".to_string();
    app.draft_cursor = app.draft.len();
    for turn in 0..turns {
        app.messages.push(SingleSessionMessage::user(format!(
            "Prompt {turn}: inspect this desktop render path and explain where scroll jank may come from."
        )));
        app.messages.push(SingleSessionMessage::assistant(format!(
            "Assistant response {turn}: The render path includes markdown wrapping, transcript card geometry, scrollbar geometry, text buffer preparation, and GPU primitive upload. This paragraph is intentionally long enough to wrap across several desktop body lines so the benchmark exercises visible-line virtualization and wrapping behavior.\n\n- Check cached text keys.\n- Check smooth scroll fractional offsets.\n- Check whether geometry can update without reshaping text.\n\n```rust\nfn sample_{turn}() {{ println!(\"benchmark\"); }}\n```"
        )));
    }
    app.set_status_label("benchmark fixture");
    app
}

#[derive(Debug, Clone)]
pub(crate) struct DesktopStreamEndToEndBenchmark {
    raw_events: usize,
    batches: usize,
    coalesced_events: usize,
    paints: usize,
    max_batch_raw_events: usize,
    max_batch_payload_bytes: usize,
    total_wall: Duration,
    max_forwarder_accumulated: Duration,
    max_apply: Duration,
    max_no_paint_gap: Duration,
    max_batch_to_paint: Duration,
    stream_left_queued_after_first_batch: bool,
}

impl DesktopStreamEndToEndBenchmark {
    pub(crate) fn passes_no_paint_budget(&self) -> bool {
        self.max_no_paint_gap <= DESKTOP_NO_PAINT_BUDGET
    }

    pub(crate) fn passes_interaction_budget(&self) -> bool {
        self.max_apply <= DESKTOP_120FPS_FRAME_BUDGET
            && self.max_forwarder_accumulated <= DESKTOP_NO_PAINT_BUDGET
            && self.max_batch_to_paint <= DESKTOP_NO_PAINT_BUDGET
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "raw_events": self.raw_events,
            "batches": self.batches,
            "coalesced_events": self.coalesced_events,
            "paints": self.paints,
            "max_batch_raw_events": self.max_batch_raw_events,
            "max_batch_payload_bytes": self.max_batch_payload_bytes,
            "total_wall_ms": duration_ms(self.total_wall),
            "max_forwarder_accumulated_ms": duration_ms(self.max_forwarder_accumulated),
            "max_apply_ms": duration_ms(self.max_apply),
            "max_no_paint_gap_ms": duration_ms(self.max_no_paint_gap),
            "max_batch_to_paint_ms": duration_ms(self.max_batch_to_paint),
            "stream_left_queued_after_first_batch": self.stream_left_queued_after_first_batch,
            "passes_no_paint_budget": self.passes_no_paint_budget(),
            "passes_interaction_budget": self.passes_interaction_budget(),
        })
    }
}

pub(crate) fn run_desktop_stream_end_to_end_benchmark(
    raw_events: usize,
) -> DesktopStreamEndToEndBenchmark {
    let raw_events = raw_events.max(1);
    let (tx, rx) = mpsc::channel();
    for index in 0..raw_events {
        // Receiver lives until after the loop; send cannot fail, and a
        // benchmark shouldn't panic even if it somehow did.
        let _ = tx.send(session_launch::DesktopSessionEvent::TextDelta(format!(
            "{} ",
            index + 1
        )));
    }
    drop(tx);

    let started = Instant::now();
    let mut next_forward_at = started;
    let mut app = DesktopApp::SingleSession(SingleSessionApp::new(None));
    let mut batches = 0usize;
    let mut coalesced_events = 0usize;
    let mut paints = 0usize;
    let mut max_batch_raw_events = 0usize;
    let mut max_batch_payload_bytes = 0usize;
    let mut max_forwarder_accumulated = Duration::ZERO;
    let mut max_apply = Duration::ZERO;
    let mut max_no_paint_gap = Duration::ZERO;
    let mut max_batch_to_paint = Duration::ZERO;
    let mut last_paint_at = started;
    let mut pending_batch_since: Option<Instant> = None;
    let mut stream_left_queued_after_first_batch = false;

    while let Ok(first_event) = rx.try_recv() {
        let now = Instant::now();
        if now < next_forward_at {
            std::thread::sleep(next_forward_at.saturating_duration_since(now));
        }

        let batch = collect_desktop_session_event_batch(first_event, &rx);
        if batches == 0 {
            stream_left_queued_after_first_batch = batch.raw_event_count < raw_events;
        }
        let forwarded_at = Instant::now();
        next_forward_at = forwarded_at + BACKEND_EVENT_FORWARD_INTERVAL;

        batches += 1;
        coalesced_events += batch.events.len();
        max_batch_raw_events = max_batch_raw_events.max(batch.raw_event_count);
        max_batch_payload_bytes = max_batch_payload_bytes.max(batch.raw_payload_bytes);
        max_forwarder_accumulated = max_forwarder_accumulated.max(batch.accumulated_for());
        pending_batch_since.get_or_insert(batch.first_received_at);

        let apply_stats = apply_desktop_session_event_batch_with_stats(&mut app, batch.events);
        max_apply = max_apply.max(apply_stats.elapsed);
        if apply_stats.visible_changed {
            let paint_now = Instant::now();
            if paint_now.saturating_duration_since(last_paint_at) >= BACKEND_REDRAW_FRAME_INTERVAL {
                paints += 1;
                max_no_paint_gap =
                    max_no_paint_gap.max(paint_now.saturating_duration_since(last_paint_at));
                if let Some(pending_since) = pending_batch_since.take() {
                    max_batch_to_paint =
                        max_batch_to_paint.max(paint_now.saturating_duration_since(pending_since));
                }
                last_paint_at = paint_now;
            }
        }
    }

    if let Some(pending_since) = pending_batch_since.take() {
        let paint_now = Instant::now();
        paints += 1;
        max_no_paint_gap = max_no_paint_gap.max(paint_now.saturating_duration_since(last_paint_at));
        max_batch_to_paint =
            max_batch_to_paint.max(paint_now.saturating_duration_since(pending_since));
    }

    DesktopStreamEndToEndBenchmark {
        raw_events,
        batches,
        coalesced_events,
        paints,
        max_batch_raw_events,
        max_batch_payload_bytes,
        total_wall: started.elapsed(),
        max_forwarder_accumulated,
        max_apply,
        max_no_paint_gap,
        max_batch_to_paint,
        stream_left_queued_after_first_batch,
    }
}
