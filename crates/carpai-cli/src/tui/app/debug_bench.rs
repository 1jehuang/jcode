use super::*;

impl App {
    pub(in crate::tui::app) fn build_scroll_test_content(
        diagrams: usize,
        padding: usize,
        override_diagram: Option<&str>,
    ) -> String {
        let mut out = String::new();
        let intro_lines = padding.max(4);
        for i in 0..intro_lines {
            out.push_str(&format!(
                "Intro line {:02} - quick brown fox jumps over the lazy dog.\n",
                i + 1
            ));
        }

        let diagram_templates = [
            r#"flowchart TD
    A[Start] --> B{Decision}
    B -->|Yes| C[Process 1]
    B -->|No| D[Process 2]
    C --> E[Merge]
    D --> E
    E --> F[End]"#,
            r#"sequenceDiagram
    participant U as User
    participant A as App
    participant S as Service
    U->>A: Scroll request
    A->>S: Render diagram
    S-->>A: PNG
    A-->>U: Draw frame"#,
            r#"stateDiagram-v2
    [*] --> Idle
    Idle --> Scrolling: input
    Scrolling --> Rendering: diagram
    Rendering --> Idle: frame drawn"#,
        ];

        for idx in 0..diagrams {
            let diagram =
                override_diagram.unwrap_or(diagram_templates[idx % diagram_templates.len()]);
            out.push_str("```mermaid\n");
            out.push_str(diagram);
            out.push_str("\n```\n");

            for j in 0..padding {
                out.push_str(&format!(
                    "After diagram {} line {:02} - stretch content for scrolling.\n",
                    idx + 1,
                    j + 1
                ));
            }
        }

        out
    }

    fn build_side_panel_latency_snapshot(
        diagrams: usize,
        padding: usize,
    ) -> crate::side_panel::SidePanelSnapshot {
        let content = Self::build_scroll_test_content(diagrams, padding, None);
        crate::side_panel::SidePanelSnapshot {
            focused_page_id: Some("latency_bench".to_string()),
            pages: vec![crate::side_panel::SidePanelPage {
                id: "latency_bench".to_string(),
                title: "Latency Bench".to_string(),
                file_path: "latency_bench.md".to_string(),
                format: crate::side_panel::SidePanelPageFormat::Markdown,
                source: crate::side_panel::SidePanelPageSource::Managed,
                content,
                updated_at_ms: 1,
            }],
        }
    }

    pub(in crate::tui::app) fn run_side_panel_latency_bench(
        &mut self,
        raw: Option<&str>,
    ) -> String {
        let cfg: SidePanelLatencyConfig = if let Some(raw) = raw {
            if raw.trim().is_empty() {
                SidePanelLatencyConfig {
                    width: None,
                    height: None,
                    iterations: None,
                    warmup_iterations: None,
                    padding: None,
                    diagrams: None,
                    include_samples: None,
                }
            } else {
                match serde_json::from_str(raw) {
                    Ok(cfg) => cfg,
                    Err(e) => return format!("side-panel-latency parse error: {}", e),
                }
            }
        } else {
            SidePanelLatencyConfig {
                width: None,
                height: None,
                iterations: None,
                warmup_iterations: None,
                padding: None,
                diagrams: None,
                include_samples: None,
            }
        };

        let width = cfg.width.unwrap_or(100).max(40);
        let height = cfg.height.unwrap_or(40).max(20);
        let iterations = cfg.iterations.unwrap_or(40).clamp(4, 400);
        let warmup_iterations = cfg.warmup_iterations.unwrap_or(6).min(50);
        let padding = cfg.padding.unwrap_or(24).max(8);
        let diagrams = cfg.diagrams.unwrap_or(2).clamp(1, 3);
        let include_samples = cfg.include_samples.unwrap_or(true);

        let saved_state = ScrollTestState::capture(self);
        let saved_diagram_override = crate::tui::markdown::get_diagram_mode_override();
        let saved_active_diagrams = crate::tui::mermaid::snapshot_active_diagrams();
        let was_visual_debug = crate::tui::visual_debug::is_enabled();
        crate::tui::visual_debug::enable();

        self.display_messages = vec![
            DisplayMessage {
                role: "user".to_string(),
                content: "Headless side-panel latency benchmark".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
            DisplayMessage {
                role: "assistant".to_string(),
                content: "Benchmarking side-panel input latency.".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
        ];
        self.bump_display_messages_version();
        self.side_panel = Self::build_side_panel_latency_snapshot(diagrams, padding);
        self.diff_mode = crate::config::DiffDisplayMode::Off;
        self.diff_pane_scroll = 0;
        self.diff_pane_scroll_x = 0;
        self.diff_pane_focus = false;
        self.diff_pane_auto_scroll = false;
        self.follow_chat_bottom();
        self.is_processing = false;
        self.clear_streaming_render_state();
        self.queued_messages.clear();
        self.interleave_message = None;
        self.pending_soft_interrupts.clear();
        self.status = ProcessingStatus::Idle;
        self.processing_started = None;
        self.status_notice = None;
        crate::tui::markdown::set_diagram_mode_override(Some(self.diagram_mode));

        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let result = (|| -> Result<serde_json::Value, String> {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend)
                .map_err(|e| format!("side-panel-latency terminal error: {}", e))?;

            terminal
                .draw(|f| crate::tui::ui::draw(f, self))
                .map_err(|e| format!("side-panel-latency baseline draw error: {}", e))?;

            let diff_area = crate::tui::ui::last_layout_snapshot()
                .and_then(|layout| layout.diff_pane_area)
                .ok_or_else(|| "side-panel-latency: diff pane area missing".to_string())?;
            let total_lines = crate::tui::ui::pinned_pane_total_lines();
            let max_scroll = total_lines.saturating_sub(diff_area.height as usize);
            if max_scroll == 0 {
                return Err("side-panel-latency: side panel did not become scrollable".to_string());
            }

            self.diff_pane_scroll = max_scroll / 2;
            terminal
                .draw(|f| crate::tui::ui::draw(f, self))
                .map_err(|e| format!("side-panel-latency mid draw error: {}", e))?;

            let center_x = diff_area.x + diff_area.width / 2;
            let center_y = diff_area.y + diff_area.height / 2;
            let total_runs = warmup_iterations + iterations;
            let mut samples: Vec<SidePanelLatencySample> = Vec::with_capacity(iterations);
            let mut latency_values: Vec<f64> = Vec::with_capacity(iterations);
            let mut render_values: Vec<f64> = Vec::with_capacity(iterations);
            let mut scroll_only_count = 0usize;
            let mut unchanged_scroll_count = 0usize;

            for idx in 0..total_runs {
                let direction = if idx % 2 == 0 { "down" } else { "up" };
                let kind = if idx % 2 == 0 {
                    MouseEventKind::ScrollDown
                } else {
                    MouseEventKind::ScrollUp
                };
                let before_frame = crate::tui::visual_debug::latest_frame();
                let before_frame_id = before_frame.as_ref().map(|frame| frame.frame_id);
                let scroll_before = if self.diff_pane_scroll == usize::MAX {
                    crate::tui::ui::last_diff_pane_effective_scroll()
                } else {
                    self.diff_pane_scroll
                };
                let started = Instant::now();
                let scroll_only = self.handle_mouse_event(MouseEvent {
                    kind,
                    column: center_x,
                    row: center_y,
                    modifiers: KeyModifiers::empty(),
                });
                if scroll_only {
                    scroll_only_count += 1;
                    std::thread::sleep(crate::tui::redraw_interval(self));
                }
                terminal
                    .draw(|f| crate::tui::ui::draw(f, self))
                    .map_err(|e| format!("side-panel-latency draw error: {}", e))?;
                let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
                let after_frame = crate::tui::visual_debug::latest_frame();
                let after_frame_id = after_frame.as_ref().map(|frame| frame.frame_id);
                let scroll_after = crate::tui::ui::last_diff_pane_effective_scroll();
                let scroll_changed = scroll_after != scroll_before;
                if !scroll_changed {
                    unchanged_scroll_count += 1;
                }
                let render_ms = after_frame
                    .as_ref()
                    .and_then(|frame| frame.render_timing.as_ref().map(|timing| timing.total_ms));

                if idx >= warmup_iterations {
                    latency_values.push(latency_ms);
                    if let Some(render_ms) = render_ms {
                        render_values.push(render_ms as f64);
                    }
                    samples.push(SidePanelLatencySample {
                        iteration: idx - warmup_iterations,
                        direction,
                        scroll_only,
                        latency_ms,
                        render_ms,
                        scroll_before,
                        scroll_after,
                        frame_id_before: before_frame_id,
                        frame_id_after: after_frame_id,
                        scroll_changed,
                    });
                }
            }

            let mut sorted_latencies = latency_values.clone();
            sorted_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mut sorted_render = render_values.clone();
            sorted_render.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            Ok(serde_json::json!({
                "ok": scroll_only_count == 0 && unchanged_scroll_count == 0,
                "config": {
                    "width": width,
                    "height": height,
                    "iterations": iterations,
                    "warmup_iterations": warmup_iterations,
                    "padding": padding,
                    "diagrams": diagrams,
                },
                "summary": {
                    "samples": latency_values.len(),
                    "scroll_only_count": scroll_only_count,
                    "unchanged_scroll_count": unchanged_scroll_count,
                    "max_scroll": max_scroll,
                    "latency_ms": {
                        "p50": percentile_ms(&sorted_latencies, 0.50),
                        "p95": percentile_ms(&sorted_latencies, 0.95),
                        "p99": percentile_ms(&sorted_latencies, 0.99),
                        "max": sorted_latencies.last().copied().unwrap_or(0.0),
                        "avg": if latency_values.is_empty() { 0.0 } else { latency_values.iter().sum::<f64>() / latency_values.len() as f64 },
                    },
                    "render_ms": {
                        "p50": percentile_ms(&sorted_render, 0.50),
                        "p95": percentile_ms(&sorted_render, 0.95),
                        "p99": percentile_ms(&sorted_render, 0.99),
                        "max": sorted_render.last().copied().unwrap_or(0.0),
                        "avg": if render_values.is_empty() { 0.0 } else { render_values.iter().sum::<f64>() / render_values.len() as f64 },
                    }
                },
                "samples": if include_samples { serde_json::to_value(&samples).unwrap_or(serde_json::Value::Null) } else { serde_json::Value::Null },
                "notes": [
                    "This is a headless end-to-end app benchmark: injected side-panel mouse scroll event -> event classification -> redraw scheduling -> offscreen frame update.",
                    "It does not include terminal emulator/compositor/image protocol wall-clock paint latency outside jcode."
                ]
            }))
        })();

        saved_state.restore(self);
        crate::tui::markdown::set_diagram_mode_override(saved_diagram_override);
        crate::tui::mermaid::restore_active_diagrams(saved_active_diagrams);
        if !was_visual_debug {
            crate::tui::visual_debug::disable();
        }

        match result {
            Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string()),
            Err(e) => e,
        }
    }

    pub(in crate::tui::app) fn run_mermaid_ui_bench(&mut self, raw: Option<&str>) -> String {
        let cfg: MermaidUiBenchConfig = if let Some(raw) = raw {
            if raw.trim().is_empty() {
                MermaidUiBenchConfig {
                    width: None,
                    height: None,
                    frames: None,
                    warmup_frames: None,
                    padding: None,
                    diagrams: None,
                    include_samples: None,
                    keep_mermaid_cache: None,
                    sleep_between_frames_ms: None,
                }
            } else {
                match serde_json::from_str(raw) {
                    Ok(cfg) => cfg,
                    Err(e) => return format!("mermaid:ui-bench parse error: {}", e),
                }
            }
        } else {
            MermaidUiBenchConfig {
                width: None,
                height: None,
                frames: None,
                warmup_frames: None,
                padding: None,
                diagrams: None,
                include_samples: None,
                keep_mermaid_cache: None,
                sleep_between_frames_ms: None,
            }
        };

        let width = cfg.width.unwrap_or(100).max(40);
        let height = cfg.height.unwrap_or(40).max(20);
        let frames = cfg.frames.unwrap_or(24).clamp(4, 240);
        let warmup_frames = cfg.warmup_frames.unwrap_or(0).min(frames.saturating_sub(1));
        let padding = cfg.padding.unwrap_or(24).max(8);
        let diagrams = cfg.diagrams.unwrap_or(2).clamp(1, 4);
        let include_samples = cfg.include_samples.unwrap_or(true);
        let keep_mermaid_cache = cfg.keep_mermaid_cache.unwrap_or(false);
        let sleep_between_frames_ms = cfg.sleep_between_frames_ms.unwrap_or(0).min(1_000);

        let saved_state = ScrollTestState::capture(self);
        let saved_diagram_override = crate::tui::markdown::get_diagram_mode_override();
        let saved_active_diagrams = crate::tui::mermaid::snapshot_active_diagrams();
        let was_visual_debug = crate::tui::visual_debug::is_enabled();
        crate::tui::visual_debug::enable();
        crate::tui::mermaid::init_picker();

        self.display_messages = vec![
            DisplayMessage {
                role: "user".to_string(),
                content: "Live Mermaid UI benchmark".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
            DisplayMessage {
                role: "assistant".to_string(),
                content: "Benchmarking deferred Mermaid render and image protocol reuse."
                    .to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
        ];
        self.bump_display_messages_version();
        self.side_panel = Self::build_side_panel_latency_snapshot(diagrams, padding);
        self.diff_mode = crate::config::DiffDisplayMode::Off;
        self.diff_pane_scroll = 0;
        self.diff_pane_scroll_x = 0;
        self.diff_pane_focus = false;
        self.diff_pane_auto_scroll = false;
        self.follow_chat_bottom();
        self.is_processing = false;
        self.clear_streaming_render_state();
        self.queued_messages.clear();
        self.interleave_message = None;
        self.pending_soft_interrupts.clear();
        self.status = ProcessingStatus::Idle;
        self.processing_started = None;
        self.status_notice = None;
        crate::tui::markdown::set_diagram_mode_override(Some(self.diagram_mode));
        crate::tui::clear_side_panel_render_caches();
        crate::tui::reset_side_panel_debug_stats();
        crate::tui::markdown::reset_debug_stats();
        crate::tui::mermaid::reset_debug_stats();
        crate::tui::mermaid::clear_active_diagrams();
        crate::tui::mermaid::clear_streaming_preview_diagram();
        if !keep_mermaid_cache {
            let _ = crate::tui::mermaid::clear_cache();
        }

        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let result = (|| -> Result<serde_json::Value, String> {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend)
                .map_err(|e| format!("mermaid:ui-bench terminal error: {}", e))?;

            let protocol = crate::tui::mermaid::protocol_type().map(|p| format!("{:?}", p));
            let protocol_supported = protocol.is_some();

            let mut samples = Vec::with_capacity(frames.saturating_sub(warmup_frames));
            let mut frame_times = Vec::with_capacity(frames);
            let mut render_values = Vec::with_capacity(frames.saturating_sub(warmup_frames));

            for frame_idx in 0..frames {
                let before_stats = crate::tui::mermaid::debug_stats();
                let frame_started = Instant::now();
                terminal
                    .draw(|f| crate::tui::ui::draw(f, self))
                    .map_err(|e| format!("mermaid:ui-bench draw error: {}", e))?;
                let frame_ms = frame_started.elapsed().as_secs_f64() * 1000.0;
                frame_times.push(frame_ms);

                let after_stats = crate::tui::mermaid::debug_stats();
                let latest_frame = crate::tui::visual_debug::latest_frame();
                let render_ms = latest_frame
                    .as_ref()
                    .and_then(|frame| frame.render_timing.as_ref().map(|timing| timing.total_ms));
                let image_regions = latest_frame
                    .as_ref()
                    .map(|frame| frame.image_regions.len())
                    .unwrap_or(0);

                if frame_idx >= warmup_frames {
                    if let Some(render_ms) = render_ms {
                        render_values.push(render_ms as f64);
                    }
                    samples.push(MermaidUiBenchSample {
                        frame: frame_idx - warmup_frames,
                        frame_ms,
                        render_ms,
                        image_regions,
                        deferred_pending_after: after_stats.deferred_pending,
                        deferred_enqueued: after_stats
                            .deferred_enqueued
                            .saturating_sub(before_stats.deferred_enqueued),
                        deferred_deduped: after_stats
                            .deferred_deduped
                            .saturating_sub(before_stats.deferred_deduped),
                        deferred_worker_renders: after_stats
                            .deferred_worker_renders
                            .saturating_sub(before_stats.deferred_worker_renders),
                        image_state_hits: after_stats
                            .image_state_hits
                            .saturating_sub(before_stats.image_state_hits),
                        image_state_misses: after_stats
                            .image_state_misses
                            .saturating_sub(before_stats.image_state_misses),
                        fit_state_reuse_hits: after_stats
                            .fit_state_reuse_hits
                            .saturating_sub(before_stats.fit_state_reuse_hits),
                        fit_protocol_rebuilds: after_stats
                            .fit_protocol_rebuilds
                            .saturating_sub(before_stats.fit_protocol_rebuilds),
                        viewport_state_reuse_hits: after_stats
                            .viewport_state_reuse_hits
                            .saturating_sub(before_stats.viewport_state_reuse_hits),
                        viewport_protocol_rebuilds: after_stats
                            .viewport_protocol_rebuilds
                            .saturating_sub(before_stats.viewport_protocol_rebuilds),
                    });
                }

                if sleep_between_frames_ms > 0 && frame_idx + 1 < frames {
                    std::thread::sleep(std::time::Duration::from_millis(sleep_between_frames_ms));
                }
            }

            let summary = summarize_mermaid_ui_bench(&samples, protocol_supported, protocol);
            let mut sorted_frames = frame_times.clone();
            sorted_frames.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mut sorted_render = render_values.clone();
            sorted_render.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            Ok(serde_json::json!({
                "ok": true,
                "config": {
                    "width": width,
                    "height": height,
                    "frames": frames,
                    "warmup_frames": warmup_frames,
                    "padding": padding,
                    "diagrams": diagrams,
                    "keep_mermaid_cache": keep_mermaid_cache,
                    "sleep_between_frames_ms": sleep_between_frames_ms,
                },
                "summary": summary,
                "timing": {
                    "frame_ms": {
                        "p50": percentile_ms(&sorted_frames, 0.50),
                        "p95": percentile_ms(&sorted_frames, 0.95),
                        "p99": percentile_ms(&sorted_frames, 0.99),
                        "max": sorted_frames.last().copied().unwrap_or(0.0),
                        "avg": if frame_times.is_empty() { 0.0 } else { frame_times.iter().sum::<f64>() / frame_times.len() as f64 },
                    },
                    "render_ms": {
                        "p50": percentile_ms(&sorted_render, 0.50),
                        "p95": percentile_ms(&sorted_render, 0.95),
                        "p99": percentile_ms(&sorted_render, 0.99),
                        "max": sorted_render.last().copied().unwrap_or(0.0),
                        "avg": if render_values.is_empty() { 0.0 } else { render_values.iter().sum::<f64>() / render_values.len() as f64 },
                    }
                },
                "final_mermaid_stats": crate::tui::mermaid::debug_stats(),
                "samples": if include_samples { serde_json::to_value(&samples).unwrap_or(serde_json::Value::Null) } else { serde_json::Value::Null },
                "notes": [
                    "Runs inside the live TUI client process, so Mermaid protocol capability comes from the attached terminal session.",
                    "Uses an offscreen TestBackend for repeatable frame timing while still exercising the app's real Mermaid markdown, cache, deferred render, and image protocol paths."
                ]
            }))
        })();

        saved_state.restore(self);
        crate::tui::markdown::set_diagram_mode_override(saved_diagram_override);
        crate::tui::mermaid::restore_active_diagrams(saved_active_diagrams);
        if !was_visual_debug {
            crate::tui::visual_debug::disable();
        }

        match result {
            Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string()),
            Err(e) => e,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "scroll-test capture needs terminal, labels, offsets, frame inclusion, and expectation metadata"
    )]
    fn capture_scroll_test_step(
        &mut self,
        terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
        label: &str,
        mode: &str,
        scroll_offset: usize,
        max_scroll: usize,
        include_frames: bool,
        expectations: &ScrollTestExpectations,
    ) -> Result<serde_json::Value, String> {
        self.scroll_offset = scroll_offset;
        self.auto_scroll_paused = mode == "paused";
        if let Err(e) = terminal.draw(|f| crate::tui::ui::draw(f, self)) {
            return Err(format!("draw error ({}): {}", label, e));
        }

        let frame = crate::tui::visual_debug::latest_frame();
        let (frame_id, anomalies, image_regions, normalized_frame) = match frame {
            Some(ref frame) => {
                let normalized = if include_frames {
                    Some(crate::tui::visual_debug::normalize_frame(frame))
                } else {
                    None
                };
                (
                    Some(frame.frame_id),
                    frame.anomalies.clone(),
                    frame.image_regions.clone(),
                    normalized,
                )
            }
            None => (None, Vec::new(), Vec::new(), None),
        };

        let user_scroll = scroll_offset.min(max_scroll);
        let scroll_top = if self.auto_scroll_paused && user_scroll > 0 {
            user_scroll
        } else {
            max_scroll
        };

        let mermaid_stats = crate::tui::mermaid::debug_stats_json();
        let mermaid_state = serde_json::to_value(crate::tui::mermaid::debug_image_state()).ok();
        let active_diagrams = crate::tui::mermaid::get_active_diagrams();

        let (diagram_area_capture, diagram_widget_present, diagram_mode_label) = match frame {
            Some(ref frame) => {
                let widget_present = frame
                    .info_widgets
                    .as_ref()
                    .map(|info| info.placements.iter().any(|p| p.kind == "diagrams"))
                    .unwrap_or(false);
                let mode = frame
                    .state
                    .diagram_mode
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", self.diagram_mode));
                (frame.layout.diagram_area, widget_present, mode)
            }
            None => (None, false, format!("{:?}", self.diagram_mode)),
        };

        let diagram_area_rect =
            diagram_area_capture.map(crate::tui::layout_utils::rect_from_capture);
        let diagram_area_json = diagram_area_capture.map(|rect| {
            serde_json::json!({
                "x": rect.x,
                "y": rect.y,
                "width": rect.width,
                "height": rect.height,
            })
        });

        let mut diagram_rendered_in_pane = false;
        if let (Some(area), Some(state)) = (
            diagram_area_rect,
            mermaid_state.as_ref().and_then(|v| v.as_array()),
        ) {
            for entry in state {
                let last_area = entry
                    .get("last_area")
                    .and_then(|v| v.as_str())
                    .and_then(crate::tui::layout_utils::parse_area_spec);
                if let Some(render_area) = last_area
                    && crate::tui::layout_utils::rect_contains(area, render_area)
                {
                    diagram_rendered_in_pane = true;
                    break;
                }
            }
        }

        let active_hashes: Vec<String> = active_diagrams
            .iter()
            .map(|d| format!("{:016x}", d.hash))
            .collect();
        let inline_placeholders = image_regions.len();

        let mut problems: Vec<String> = Vec::new();
        if expectations.require_no_anomalies && !anomalies.is_empty() {
            problems.push(format!("anomalies: {}", anomalies.join("; ")));
        }
        if expectations.expect_pane {
            if diagram_area_rect.is_none() {
                problems.push("missing pinned diagram area".to_string());
            }
            if active_hashes.is_empty() {
                problems.push("no active diagrams registered".to_string());
            }
            if !diagram_rendered_in_pane {
                problems.push("diagram not rendered in pinned pane".to_string());
            }
        }
        if expectations.expect_inline {
            if inline_placeholders == 0 {
                problems.push("expected inline diagram placeholders but none found".to_string());
            }
        } else if inline_placeholders > 0 {
            problems.push("unexpected inline diagram placeholders".to_string());
        }
        if expectations.expect_widget && !diagram_widget_present {
            problems.push("expected diagram widget but none present".to_string());
        }

        let checks_ok = problems.is_empty();

        Ok(serde_json::json!({
            "label": label,
            "mode": mode,
            "scroll_offset": scroll_offset,
            "scroll_top": scroll_top,
            "max_scroll": max_scroll,
            "frame_id": frame_id,
            "anomalies": anomalies,
            "image_regions": image_regions,
            "mermaid_stats": mermaid_stats,
            "mermaid_state": mermaid_state,
            "diagram": {
                "mode": diagram_mode_label,
                "area": diagram_area_json,
                "active_diagrams": active_hashes,
                "widget_present": diagram_widget_present,
                "inline_placeholders": inline_placeholders,
                "rendered_in_pane": diagram_rendered_in_pane,
            },
            "checks": {
                "ok": checks_ok,
                "problems": problems,
                "expectations": {
                    "expect_inline": expectations.expect_inline,
                    "expect_pane": expectations.expect_pane,
                    "expect_widget": expectations.expect_widget,
                    "require_no_anomalies": expectations.require_no_anomalies,
                }
            },
            "frame": normalized_frame,
        }))
    }

    pub(in crate::tui::app) fn run_scroll_test(&mut self, raw: Option<&str>) -> String {
        let cfg: ScrollTestConfig = if let Some(raw) = raw {
            if raw.trim().is_empty() {
                ScrollTestConfig {
                    width: None,
                    height: None,
                    step: None,
                    max_steps: None,
                    padding: None,
                    diagrams: None,
                    include_frames: None,
                    include_paused: None,
                    diagram: None,
                    diagram_mode: None,
                    expect_inline: None,
                    expect_pane: None,
                    expect_widget: None,
                    require_no_anomalies: None,
                }
            } else {
                match serde_json::from_str(raw) {
                    Ok(cfg) => cfg,
                    Err(e) => return format!("scroll-test parse error: {}", e),
                }
            }
        } else {
            ScrollTestConfig {
                width: None,
                height: None,
                step: None,
                max_steps: None,
                padding: None,
                diagrams: None,
                include_frames: None,
                include_paused: None,
                diagram: None,
                diagram_mode: None,
                expect_inline: None,
                expect_pane: None,
                expect_widget: None,
                require_no_anomalies: None,
            }
        };

        let diagram_mode = cfg.diagram_mode.unwrap_or(self.diagram_mode);
        let expectations = ScrollTestExpectations {
            expect_inline: cfg
                .expect_inline
                .unwrap_or(diagram_mode != crate::config::DiagramDisplayMode::Pinned),
            expect_pane: cfg
                .expect_pane
                .unwrap_or(diagram_mode == crate::config::DiagramDisplayMode::Pinned),
            expect_widget: cfg.expect_widget.unwrap_or(false),
            require_no_anomalies: cfg.require_no_anomalies.unwrap_or(true),
        };

        let width = cfg.width.unwrap_or(100).max(40);
        let height = cfg.height.unwrap_or(40).max(20);
        let step = cfg.step.unwrap_or(5).max(1);
        let max_steps = cfg.max_steps.unwrap_or(16).clamp(4, 100);
        let padding = cfg.padding.unwrap_or(12).max(4);
        let diagrams = cfg.diagrams.unwrap_or(2).clamp(1, 3);
        let include_frames = cfg.include_frames.unwrap_or(true);
        let include_paused = cfg.include_paused.unwrap_or(true);
        let diagram_override = cfg.diagram.as_deref();

        let saved_state = ScrollTestState::capture(self);
        let saved_diagram_override = crate::tui::markdown::get_diagram_mode_override();
        let saved_active_diagrams = crate::tui::mermaid::snapshot_active_diagrams();
        let was_visual_debug = crate::tui::visual_debug::is_enabled();
        crate::tui::visual_debug::enable();

        self.diagram_mode = diagram_mode;
        crate::tui::markdown::set_diagram_mode_override(Some(diagram_mode));

        let test_content = Self::build_scroll_test_content(diagrams, padding, diagram_override);
        self.display_messages = vec![
            DisplayMessage {
                role: "user".to_string(),
                content: "Scroll test: render mermaid + text".to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
            DisplayMessage {
                role: "assistant".to_string(),
                content: test_content,
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            },
        ];
        self.bump_display_messages_version();
        self.follow_chat_bottom();
        self.is_processing = false;
        self.clear_streaming_render_state();
        self.queued_messages.clear();
        self.interleave_message = None;
        self.pending_soft_interrupts.clear();
        self.status = ProcessingStatus::Idle;
        self.processing_started = None;
        self.status_notice = None;

        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut errors: Vec<String> = Vec::new();
        let mut steps: Vec<serde_json::Value> = Vec::new();

        let backend = TestBackend::new(width, height);
        let mut terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(e) => {
                saved_state.restore(self);
                crate::tui::markdown::set_diagram_mode_override(saved_diagram_override);
                crate::tui::mermaid::restore_active_diagrams(saved_active_diagrams);
                if !was_visual_debug {
                    crate::tui::visual_debug::disable();
                }
                return format!("scroll-test terminal error: {}", e);
            }
        };

        // Baseline render (bottom) for metrics
        self.follow_chat_bottom();
        if let Err(e) = terminal.draw(|f| crate::tui::ui::draw(f, self)) {
            errors.push(format!("baseline draw error: {}", e));
        }

        // Derive scroll positions using the latest frame
        let baseline_frame = crate::tui::visual_debug::latest_frame();
        let (visible_height, total_lines, image_regions) = if let Some(frame) = baseline_frame {
            let visible_height = frame
                .layout
                .messages_area
                .map(|r| r.height as usize)
                .unwrap_or(height as usize);
            let total_lines = frame.layout.estimated_content_height.max(1);
            (visible_height, total_lines, frame.image_regions)
        } else {
            (height as usize, 1usize, Vec::new())
        };

        let max_scroll = total_lines.saturating_sub(visible_height);

        let mut positions: Vec<(String, usize)> = Vec::new();
        positions.push(("bottom".to_string(), max_scroll));
        positions.push(("middle".to_string(), max_scroll / 2));
        positions.push(("top".to_string(), 0));

        for (idx, region) in image_regions.iter().enumerate() {
            let img_top = region.abs_line_idx;
            let img_bottom = region.abs_line_idx + region.height as usize;
            positions.push((format!("image{}_top", idx + 1), img_top));
            positions.push((
                format!("image{}_bottom", idx + 1),
                img_bottom.saturating_sub(visible_height),
            ));
            positions.push((format!("image{}_off_top", idx + 1), img_bottom));
            if img_top > 0 {
                positions.push((format!("image{}_pre", idx + 1), img_top.saturating_sub(2)));
            }
        }

        if max_scroll > 0 {
            let mut cursor = 0usize;
            while cursor <= max_scroll && positions.len() < max_steps {
                positions.push((format!("step_{}", cursor), cursor));
                cursor = cursor.saturating_add(step);
                if cursor == 0 {
                    break;
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        let mut ordered: Vec<(String, usize)> = Vec::new();
        for (label, scroll_top) in positions {
            let clamped = scroll_top.min(max_scroll);
            if seen.insert(clamped) {
                ordered.push((label, clamped));
            }
        }

        if ordered.len() > max_steps {
            ordered.truncate(max_steps);
        }

        for (label, scroll_top) in &ordered {
            let offset = max_scroll.saturating_sub(*scroll_top);
            match self.capture_scroll_test_step(
                &mut terminal,
                label,
                "normal",
                offset,
                max_scroll,
                include_frames,
                &expectations,
            ) {
                Ok(step) => steps.push(step),
                Err(e) => errors.push(e),
            }
        }

        if include_paused {
            for (label, scroll_top) in &ordered {
                let offset = (*scroll_top).min(max_scroll);
                let paused_label = format!("{}_paused", label);
                match self.capture_scroll_test_step(
                    &mut terminal,
                    &paused_label,
                    "paused",
                    offset,
                    max_scroll,
                    include_frames,
                    &expectations,
                ) {
                    Ok(step) => steps.push(step),
                    Err(e) => errors.push(e),
                }
            }
        }

        let mermaid_scroll_sim =
            serde_json::to_value(crate::tui::mermaid::debug_test_scroll(None)).ok();

        let mut step_failures: Vec<String> = Vec::new();
        for step in &steps {
            let checks = step.get("checks");
            let ok = checks
                .and_then(|c| c.get("ok"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !ok {
                let label = step.get("label").and_then(|v| v.as_str()).unwrap_or("step");
                let problems = checks
                    .and_then(|c| c.get("problems"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .unwrap_or_else(|| "unknown failure".to_string());
                step_failures.push(format!("{}: {}", label, problems));
            }
        }

        let report = serde_json::json!({
            "ok": errors.is_empty() && step_failures.is_empty(),
            "config": {
                "width": width,
                "height": height,
                "step": step,
                "max_steps": max_steps,
                "padding": padding,
                "diagrams": diagrams,
                "include_frames": include_frames,
                "include_paused": include_paused,
                "diagram_override": diagram_override,
                "diagram_mode": format!("{:?}", diagram_mode),
                "expectations": {
                    "expect_inline": expectations.expect_inline,
                    "expect_pane": expectations.expect_pane,
                    "expect_widget": expectations.expect_widget,
                    "require_no_anomalies": expectations.require_no_anomalies,
                },
            },
            "layout": {
                "total_lines": total_lines,
                "visible_height": visible_height,
                "max_scroll": max_scroll,
            },
            "steps": steps,
            "mermaid_scroll_sim": mermaid_scroll_sim,
            "errors": errors,
            "problems": step_failures,
        });

        saved_state.restore(self);
        crate::tui::markdown::set_diagram_mode_override(saved_diagram_override);
        crate::tui::mermaid::restore_active_diagrams(saved_active_diagrams);
        if !was_visual_debug {
            crate::tui::visual_debug::disable();
        }

        serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
    }

    pub(in crate::tui::app) fn run_scroll_suite(&mut self, raw: Option<&str>) -> String {
        let cfg: ScrollSuiteConfig = if let Some(raw) = raw {
            if raw.trim().is_empty() {
                ScrollSuiteConfig {
                    widths: None,
                    heights: None,
                    diagram_modes: None,
                    diagrams: None,
                    step: None,
                    max_steps: None,
                    padding: None,
                    include_frames: None,
                    include_paused: None,
                    diagram: None,
                    require_no_anomalies: None,
                }
            } else {
                match serde_json::from_str(raw) {
                    Ok(cfg) => cfg,
                    Err(e) => return format!("scroll-suite parse error: {}", e),
                }
            }
        } else {
            ScrollSuiteConfig {
                widths: None,
                heights: None,
                diagram_modes: None,
                diagrams: None,
                step: None,
                max_steps: None,
                padding: None,
                include_frames: None,
                include_paused: None,
                diagram: None,
                require_no_anomalies: None,
            }
        };

        let widths = cfg.widths.unwrap_or_else(|| vec![80, 100, 120]);
        let heights = cfg.heights.unwrap_or_else(|| vec![24, 40]);
        let diagram_modes = cfg.diagram_modes.unwrap_or_else(|| vec![self.diagram_mode]);
        let diagrams = cfg.diagrams.unwrap_or(2).clamp(1, 3);
        let step = cfg.step.unwrap_or(5).max(1);
        let max_steps = cfg.max_steps.unwrap_or(12).clamp(4, 100);
        let padding = cfg.padding.unwrap_or(12).max(4);
        let include_frames = cfg.include_frames.unwrap_or(false);
        let include_paused = cfg.include_paused.unwrap_or(true);
        let diagram_override = cfg.diagram.as_deref();
        let require_no_anomalies = cfg.require_no_anomalies.unwrap_or(true);

        let mut results: Vec<serde_json::Value> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        let mut total = 0usize;
        let max_cases = 12usize;

        for mode in &diagram_modes {
            for width in &widths {
                for height in &heights {
                    if total >= max_cases {
                        break;
                    }
                    total += 1;
                    let mode_str = match mode {
                        crate::config::DiagramDisplayMode::None => "none",
                        crate::config::DiagramDisplayMode::Margin => "margin",
                        crate::config::DiagramDisplayMode::Pinned => "pinned",
                    };
                    let case_label = format!("{}x{}_{}", width, height, mode_str);
                    let cfg_json = serde_json::json!({
                        "width": width,
                        "height": height,
                        "step": step,
                        "max_steps": max_steps,
                        "padding": padding,
                        "diagrams": diagrams,
                        "include_frames": include_frames,
                        "include_paused": include_paused,
                        "diagram": diagram_override,
                        "diagram_mode": mode_str,
                        "require_no_anomalies": require_no_anomalies,
                    });
                    let cfg_str = cfg_json.to_string();
                    let report_str = self.run_scroll_test(Some(&cfg_str));
                    let report_value: serde_json::Value = serde_json::from_str(&report_str)
                        .unwrap_or_else(
                            |_| serde_json::json!({"ok": false, "error": "invalid report json"}),
                        );
                    let ok = report_value
                        .get("ok")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if !ok {
                        failures.push(case_label.clone());
                    }
                    results.push(serde_json::json!({
                        "name": case_label,
                        "config": cfg_json,
                        "report": report_value,
                    }));
                }
                if total >= max_cases {
                    break;
                }
            }
            if total >= max_cases {
                break;
            }
        }

        let report = serde_json::json!({
            "ok": failures.is_empty(),
            "summary": {
                "total": total,
                "failed": failures.len(),
                "failures": failures,
                "max_cases": max_cases,
            },
            "cases": results,
        });

        serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
    }
}
