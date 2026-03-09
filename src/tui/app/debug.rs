use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugSnapshot {
    state: serde_json::Value,
    frame: Option<crate::tui::visual_debug::FrameCapture>,
    recent_messages: Vec<DebugMessage>,
    queued_messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugMessage {
    role: String,
    content: String,
    tool_calls: Vec<String>,
    duration_secs: Option<f32>,
    title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DebugAssertion {
    field: String,
    op: String,
    value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugAssertResult {
    ok: bool,
    field: String,
    op: String,
    expected: serde_json::Value,
    actual: serde_json::Value,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugStepResult {
    step: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DebugScript {
    steps: Vec<String>,
    assertions: Vec<DebugAssertion>,
    wait_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugRunReport {
    ok: bool,
    steps: Vec<DebugStepResult>,
    assertions: Vec<DebugAssertResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ScrollTestConfig {
    width: Option<u16>,
    height: Option<u16>,
    step: Option<usize>,
    max_steps: Option<usize>,
    padding: Option<usize>,
    diagrams: Option<usize>,
    include_frames: Option<bool>,
    include_paused: Option<bool>,
    diagram: Option<String>,
    diagram_mode: Option<crate::config::DiagramDisplayMode>,
    expect_inline: Option<bool>,
    expect_pane: Option<bool>,
    expect_widget: Option<bool>,
    require_no_anomalies: Option<bool>,
}

#[derive(Debug, Clone)]
pub(super) struct ScrollTestExpectations {
    expect_inline: bool,
    expect_pane: bool,
    expect_widget: bool,
    require_no_anomalies: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ScrollSuiteConfig {
    widths: Option<Vec<u16>>,
    heights: Option<Vec<u16>>,
    diagram_modes: Option<Vec<crate::config::DiagramDisplayMode>>,
    diagrams: Option<usize>,
    step: Option<usize>,
    max_steps: Option<usize>,
    padding: Option<usize>,
    include_frames: Option<bool>,
    include_paused: Option<bool>,
    diagram: Option<String>,
    require_no_anomalies: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DebugEvent {
    at_ms: u64,
    kind: String,
    detail: String,
}

pub(super) struct DebugTrace {
    pub(super) enabled: bool,
    pub(super) started_at: Instant,
    pub(super) events: Vec<DebugEvent>,
}

impl DebugTrace {
    pub(super) fn new() -> Self {
        Self {
            enabled: false,
            started_at: Instant::now(),
            events: Vec::new(),
        }
    }

    pub(super) fn record(&mut self, kind: &str, detail: String) {
        if !self.enabled {
            return;
        }
        let at_ms = self.started_at.elapsed().as_millis() as u64;
        self.events.push(DebugEvent {
            at_ms,
            kind: kind.to_string(),
            detail,
        });
    }
}

#[derive(Clone)]
pub(super) struct ScrollTestState {
    display_messages: Vec<DisplayMessage>,
    display_messages_version: u64,
    scroll_offset: usize,
    auto_scroll_paused: bool,
    is_processing: bool,
    streaming_text: String,
    queued_messages: Vec<String>,
    interleave_message: Option<String>,
    pending_soft_interrupts: Vec<String>,
    input: String,
    cursor_pos: usize,
    status: ProcessingStatus,
    processing_started: Option<Instant>,
    status_notice: Option<(String, Instant)>,
    diagram_mode: crate::config::DiagramDisplayMode,
    diagram_focus: bool,
    diagram_index: usize,
    diagram_scroll_x: i32,
    diagram_scroll_y: i32,
    diagram_pane_ratio: u8,
    diagram_pane_ratio_from: u8,
    diagram_pane_ratio_target: u8,
    diagram_pane_anim_start: Option<Instant>,
    diagram_pane_enabled: bool,
    diagram_pane_position: crate::config::DiagramPanePosition,
    diagram_zoom: u8,
}

impl ScrollTestState {
    fn capture(app: &App) -> Self {
        Self {
            display_messages: app.display_messages.clone(),
            display_messages_version: app.display_messages_version,
            scroll_offset: app.scroll_offset,
            auto_scroll_paused: app.auto_scroll_paused,
            is_processing: app.is_processing,
            streaming_text: app.streaming_text.clone(),
            queued_messages: app.queued_messages.clone(),
            interleave_message: app.interleave_message.clone(),
            pending_soft_interrupts: app.pending_soft_interrupts.clone(),
            input: app.input.clone(),
            cursor_pos: app.cursor_pos,
            status: app.status.clone(),
            processing_started: app.processing_started,
            status_notice: app.status_notice.clone(),
            diagram_mode: app.diagram_mode,
            diagram_focus: app.diagram_focus,
            diagram_index: app.diagram_index,
            diagram_scroll_x: app.diagram_scroll_x,
            diagram_scroll_y: app.diagram_scroll_y,
            diagram_pane_ratio: app.diagram_pane_ratio,
            diagram_pane_ratio_from: app.diagram_pane_ratio_from,
            diagram_pane_ratio_target: app.diagram_pane_ratio_target,
            diagram_pane_anim_start: app.diagram_pane_anim_start,
            diagram_pane_enabled: app.diagram_pane_enabled,
            diagram_pane_position: app.diagram_pane_position,
            diagram_zoom: app.diagram_zoom,
        }
    }

    fn restore(self, app: &mut App) {
        app.display_messages = self.display_messages;
        app.display_messages_version = self.display_messages_version;
        app.scroll_offset = self.scroll_offset;
        app.auto_scroll_paused = self.auto_scroll_paused;
        app.is_processing = self.is_processing;
        app.streaming_text = self.streaming_text;
        app.queued_messages = self.queued_messages;
        app.interleave_message = self.interleave_message;
        app.pending_soft_interrupts = self.pending_soft_interrupts;
        app.input = self.input;
        app.cursor_pos = self.cursor_pos;
        app.status = self.status;
        app.processing_started = self.processing_started;
        app.status_notice = self.status_notice;
        app.diagram_mode = self.diagram_mode;
        app.diagram_focus = self.diagram_focus;
        app.diagram_index = self.diagram_index;
        app.diagram_scroll_x = self.diagram_scroll_x;
        app.diagram_scroll_y = self.diagram_scroll_y;
        app.diagram_pane_ratio = self.diagram_pane_ratio;
        app.diagram_pane_ratio_from = self.diagram_pane_ratio_from;
        app.diagram_pane_ratio_target = self.diagram_pane_ratio_target;
        app.diagram_pane_anim_start = self.diagram_pane_anim_start;
        app.diagram_pane_enabled = self.diagram_pane_enabled;
        app.diagram_pane_position = self.diagram_pane_position;
        app.diagram_zoom = self.diagram_zoom;
    }
}

impl App {
    pub(super) fn build_scroll_test_content(
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

        let diagram_area_rect = diagram_area_capture.map(crate::tui::layout_utils::rect_from_capture);
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
                if let Some(render_area) = last_area {
                    if crate::tui::layout_utils::rect_contains(area, render_area) {
                        diagram_rendered_in_pane = true;
                        break;
                    }
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

    fn run_scroll_test(&mut self, raw: Option<&str>) -> String {
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
        let max_steps = cfg.max_steps.unwrap_or(16).max(4).min(100);
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

        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

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

    fn run_scroll_suite(&mut self, raw: Option<&str>) -> String {
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
        let max_steps = cfg.max_steps.unwrap_or(12).max(4).min(100);
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

    pub(super) fn handle_debug_command(&mut self, cmd: &str) -> String {
        let cmd = cmd.trim();
        if cmd == "frame" {
            return self.handle_debug_command("screen-json");
        }
        if cmd == "frame-normalized" {
            return self.handle_debug_command("screen-json-normalized");
        }
        if cmd == "enable" || cmd == "debug-enable" {
            crate::tui::visual_debug::enable();
            return "Visual debugging enabled.".to_string();
        }
        if cmd == "disable" || cmd == "debug-disable" {
            crate::tui::visual_debug::disable();
            return "Visual debugging disabled.".to_string();
        }
        if cmd == "status" {
            let enabled = crate::tui::visual_debug::is_enabled();
            let overlay = crate::tui::visual_debug::overlay_enabled();
            return serde_json::json!({
                "visual_debug_enabled": enabled,
                "visual_debug_overlay": overlay
            })
            .to_string();
        }
        if cmd == "overlay" || cmd == "overlay:status" {
            let overlay = crate::tui::visual_debug::overlay_enabled();
            return serde_json::json!({
                "visual_debug_overlay": overlay
            })
            .to_string();
        }
        if cmd == "overlay:on" || cmd == "overlay:enable" {
            crate::tui::visual_debug::set_overlay(true);
            return "Visual debug overlay enabled.".to_string();
        }
        if cmd == "overlay:off" || cmd == "overlay:disable" {
            crate::tui::visual_debug::set_overlay(false);
            return "Visual debug overlay disabled.".to_string();
        }
        if cmd.starts_with("message:") {
            let msg = cmd.strip_prefix("message:").unwrap_or("");
            // Inject the message respecting queue mode (like keyboard Enter)
            self.input = msg.to_string();
            match self.send_action(false) {
                SendAction::Submit => {
                    self.submit_input();
                    self.debug_trace
                        .record("message", format!("submitted:{}", msg));
                    format!("OK: submitted message '{}'", msg)
                }
                SendAction::Queue => {
                    self.queue_message();
                    self.debug_trace
                        .record("message", format!("queued:{}", msg));
                    format!(
                        "OK: queued message '{}' (will send after current turn)",
                        msg
                    )
                }
                SendAction::Interleave => {
                    let prepared = input::take_prepared_input(self);
                    input::stage_local_interleave(self, prepared.expanded);
                    self.debug_trace
                        .record("message", format!("interleave:{}", msg));
                    format!("OK: interleave message '{}' (injecting now)", msg)
                }
            }
        } else if cmd == "reload" {
            // Trigger reload
            self.input = "/reload".to_string();
            self.submit_input();
            self.debug_trace.record("reload", "triggered".to_string());
            "OK: reload triggered".to_string()
        } else if cmd == "state" {
            // Return current state as JSON for easier parsing
            serde_json::json!({
                "processing": self.is_processing,
                "messages": self.messages.len(),
                "display_messages": self.display_messages.len(),
                "input": self.input,
                "cursor_pos": self.cursor_pos,
                "scroll_offset": self.scroll_offset,
                "queued_messages": self.queued_messages.len(),
                "provider_session_id": self.provider_session_id,
                "model": self.provider.name(),
                "diagram_mode": format!("{:?}", self.diagram_mode),
                "diagram_focus": self.diagram_focus,
                "diagram_index": self.diagram_index,
                "diagram_scroll": [self.diagram_scroll_x, self.diagram_scroll_y],
                "diagram_pane_ratio": self.diagram_pane_ratio_target,
                "diagram_pane_enabled": self.diagram_pane_enabled,
                "diagram_pane_position": format!("{:?}", self.diagram_pane_position),
                "diagram_zoom": self.diagram_zoom,
                "diagram_count": crate::tui::mermaid::get_active_diagrams().len(),
                "version": env!("JCODE_VERSION"),
            })
            .to_string()
        } else if cmd == "swarm" || cmd == "swarm-status" {
            if self.is_remote {
                serde_json::json!({
                    "session_count": self.remote_sessions.len(),
                    "client_count": self.remote_client_count,
                    "members": self.remote_swarm_members,
                })
                .to_string()
            } else {
                serde_json::json!({
                    "session_count": 1,
                    "client_count": null,
                    "members": vec![crate::protocol::SwarmMemberStatus {
                        session_id: self.session.id.clone(),
                        friendly_name: Some(self.session.display_name().to_string()),
                        status: match &self.status {
                            ProcessingStatus::Idle => "ready".to_string(),
                            ProcessingStatus::Sending | ProcessingStatus::Connecting(_) => "running".to_string(),
                            ProcessingStatus::Thinking(_) => "thinking".to_string(),
                            ProcessingStatus::Streaming => "running".to_string(),
                            ProcessingStatus::RunningTool(_) => "running".to_string(),
                        },
                        detail: self.subagent_status.clone(),
                        role: None,
                    }],
                })
                .to_string()
            }
        } else if cmd == "snapshot" {
            let snapshot = self.build_debug_snapshot();
            serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("wait:") {
            let raw = cmd.strip_prefix("wait:").unwrap_or("0");
            if let Ok(ms) = raw.parse::<u64>() {
                return self.apply_wait_ms(ms);
            }
            format!("ERR: invalid wait '{}'", raw)
        } else if cmd == "wait" {
            if self.is_processing {
                "wait: processing".to_string()
            } else {
                "wait: idle".to_string()
            }
        } else if cmd == "last_response" {
            // Get last assistant message
            self.display_messages
                .iter()
                .rev()
                .find(|m| m.role == "assistant" || m.role == "error")
                .map(|m| format!("last_response: [{}] {}", m.role, m.content))
                .unwrap_or_else(|| "last_response: none".to_string())
        } else if cmd == "history" {
            // Return all messages as JSON
            let msgs: Vec<serde_json::Value> = self
                .display_messages
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "role": m.role,
                        "content": m.content,
                        "tool_calls": m.tool_calls,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&msgs).unwrap_or_else(|_| "[]".to_string())
        } else if cmd == "screen" {
            // Capture current visual state
            use crate::tui::visual_debug;
            visual_debug::enable(); // Ensure enabled
                                    // Force a frame dump to file and return path
            match visual_debug::dump_to_file() {
                Ok(path) => format!("screen: {}", path.display()),
                Err(e) => format!("screen error: {}", e),
            }
        } else if cmd == "screen-json" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            visual_debug::latest_frame_json()
                .unwrap_or_else(|| "screen-json: no frames captured".to_string())
        } else if cmd == "screen-json-normalized" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            visual_debug::latest_frame_json_normalized()
                .unwrap_or_else(|| "screen-json-normalized: no frames captured".to_string())
        } else if cmd == "layout" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "terminal_size": frame.terminal_size,
                    "layout": frame.layout,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                None => "layout: no frames captured".to_string(),
            }
        } else if cmd == "margins" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "margins": frame.layout.margins,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                None => "margins: no frames captured".to_string(),
            }
        } else if cmd == "widgets" || cmd == "info-widgets" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "info_widgets": frame.info_widgets,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                None => "widgets: no frames captured".to_string(),
            }
        } else if cmd == "render-stats" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "render_timing": frame.render_timing,
                    "render_order": frame.render_order,
                }))
                .unwrap_or_else(|_| "{}".to_string()),
                None => "render-stats: no frames captured".to_string(),
            }
        } else if cmd == "render-order" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&frame.render_order)
                    .unwrap_or_else(|_| "[]".to_string()),
                None => "render-order: no frames captured".to_string(),
            }
        } else if cmd == "anomalies" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&frame.anomalies)
                    .unwrap_or_else(|_| "[]".to_string()),
                None => "anomalies: no frames captured".to_string(),
            }
        } else if cmd == "theme" {
            use crate::tui::visual_debug;
            visual_debug::enable();
            match visual_debug::latest_frame() {
                Some(frame) => serde_json::to_string_pretty(&frame.theme)
                    .unwrap_or_else(|_| "null".to_string()),
                None => "theme: no frames captured".to_string(),
            }
        } else if cmd == "mermaid:stats" {
            let stats = crate::tui::mermaid::debug_stats();
            serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:memory" {
            let profile = crate::tui::mermaid::debug_memory_profile();
            serde_json::to_string_pretty(&profile).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:memory-bench" {
            let result = crate::tui::mermaid::debug_memory_benchmark(40);
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("mermaid:memory-bench ") {
            let raw_iterations = cmd
                .strip_prefix("mermaid:memory-bench ")
                .unwrap_or("")
                .trim();
            let iterations = match raw_iterations.parse::<usize>() {
                Ok(v) => v,
                Err(_) => return "Invalid iterations (expected integer)".to_string(),
            };
            let result = crate::tui::mermaid::debug_memory_benchmark(iterations);
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:cache" {
            let entries = crate::tui::mermaid::debug_cache();
            serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
        } else if cmd == "mermaid:evict" || cmd == "mermaid:clear-cache" {
            match crate::tui::mermaid::clear_cache() {
                Ok(_) => "mermaid: cache cleared".to_string(),
                Err(e) => format!("mermaid: cache clear failed: {}", e),
            }
        } else if cmd == "markdown:stats" {
            let stats = crate::tui::markdown::debug_stats();
            serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("assert:") {
            let raw = cmd.strip_prefix("assert:").unwrap_or("");
            self.handle_assertions(raw)
        } else if cmd.starts_with("run:") {
            let raw = cmd.strip_prefix("run:").unwrap_or("");
            self.handle_script_run(raw)
        } else if cmd.starts_with("inject:") {
            let raw = cmd.strip_prefix("inject:").unwrap_or("");
            let (role, content) = if let Some((r, c)) = raw.split_once(':') {
                let role = match r {
                    "user" | "assistant" | "system" | "tool" | "error" | "meta" => r,
                    _ => "assistant",
                };
                if role == "assistant" && r != "assistant" {
                    ("assistant", raw)
                } else {
                    (role, c)
                }
            } else {
                ("assistant", raw)
            };

            self.push_display_message(DisplayMessage {
                role: role.to_string(),
                content: content.to_string(),
                tool_calls: vec![],
                duration_secs: None,
                title: None,
                tool_data: None,
            });
            format!("OK: injected {} message ({} chars)", role, content.len())
        } else if cmd == "scroll-test" || cmd.starts_with("scroll-test:") {
            let raw = cmd.strip_prefix("scroll-test:");
            self.run_scroll_test(raw)
        } else if cmd == "scroll-suite" || cmd.starts_with("scroll-suite:") {
            let raw = cmd.strip_prefix("scroll-suite:");
            self.run_scroll_suite(raw)
        } else if cmd == "quit" {
            self.should_quit = true;
            "OK: quitting".to_string()
        } else if cmd == "trace-start" {
            self.debug_trace.enabled = true;
            self.debug_trace.started_at = Instant::now();
            self.debug_trace.events.clear();
            "OK: trace started".to_string()
        } else if cmd == "trace-stop" {
            self.debug_trace.enabled = false;
            "OK: trace stopped".to_string()
        } else if cmd == "trace" {
            serde_json::to_string_pretty(&self.debug_trace.events)
                .unwrap_or_else(|_| "[]".to_string())
        } else if cmd.starts_with("scroll:") {
            let dir = cmd.strip_prefix("scroll:").unwrap_or("");
            match dir {
                "up" => {
                    self.debug_scroll_up(5);
                    format!("scroll: up to {}", self.scroll_offset)
                }
                "down" => {
                    self.debug_scroll_down(5);
                    format!("scroll: down to {}", self.scroll_offset)
                }
                "top" => {
                    self.debug_scroll_top();
                    "scroll: top".to_string()
                }
                "bottom" => {
                    self.debug_scroll_bottom();
                    "scroll: bottom".to_string()
                }
                _ => format!("scroll error: unknown direction '{}'", dir),
            }
        } else if cmd.starts_with("keys:") {
            let keys_str = cmd.strip_prefix("keys:").unwrap_or("");
            let mut results = Vec::new();
            for key_spec in keys_str.split(',') {
                match self.parse_and_inject_key(key_spec.trim()) {
                    Ok(desc) => {
                        self.debug_trace.record("key", format!("{}", desc));
                        results.push(format!("OK: {}", desc));
                    }
                    Err(e) => results.push(format!("ERR: {}", e)),
                }
            }
            results.join("\n")
        } else if cmd == "input" {
            format!("input: {:?}", self.input)
        } else if cmd.starts_with("set_input:") {
            let new_input = cmd.strip_prefix("set_input:").unwrap_or("");
            self.input = new_input.to_string();
            self.cursor_pos = self.input.len();
            self.debug_trace
                .record("input", format!("set:{}", self.input));
            format!("OK: input set to {:?}", self.input)
        } else if cmd == "submit" {
            if self.input.is_empty() {
                "submit error: input is empty".to_string()
            } else {
                self.submit_input();
                self.debug_trace.record("input", "submitted".to_string());
                "OK: submitted".to_string()
            }
        } else if cmd == "record-start" {
            use crate::tui::test_harness;
            test_harness::start_recording();
            "OK: event recording started".to_string()
        } else if cmd == "record-stop" {
            use crate::tui::test_harness;
            test_harness::stop_recording();
            "OK: event recording stopped".to_string()
        } else if cmd == "record-events" {
            use crate::tui::test_harness;
            test_harness::get_recorded_events_json()
        } else if cmd == "clock-enable" {
            use crate::tui::test_harness;
            test_harness::enable_test_clock();
            "OK: test clock enabled".to_string()
        } else if cmd == "clock-disable" {
            use crate::tui::test_harness;
            test_harness::disable_test_clock();
            "OK: test clock disabled".to_string()
        } else if cmd.starts_with("clock-advance:") {
            use crate::tui::test_harness;
            let ms_str = cmd.strip_prefix("clock-advance:").unwrap_or("0");
            match ms_str.parse::<u64>() {
                Ok(ms) => {
                    test_harness::advance_clock(std::time::Duration::from_millis(ms));
                    format!("OK: clock advanced {}ms", ms)
                }
                Err(_) => "clock-advance error: invalid ms value".to_string(),
            }
        } else if cmd == "clock-now" {
            use crate::tui::test_harness;
            format!("clock: {}ms", test_harness::now_ms())
        } else if cmd.starts_with("replay:") {
            use crate::tui::test_harness;
            let json = cmd.strip_prefix("replay:").unwrap_or("[]");
            match test_harness::EventPlayer::from_json(json) {
                Ok(mut player) => {
                    player.start();
                    let mut results = Vec::new();
                    while let Some(event) = player.next_event() {
                        results.push(format!("{:?}", event));
                    }
                    format!(
                        "replay: {} events processed, {} remaining",
                        results.len(),
                        player.remaining()
                    )
                }
                Err(e) => format!("replay error: {}", e),
            }
        } else if cmd.starts_with("bundle-start:") {
            let name = cmd.strip_prefix("bundle-start:").unwrap_or("test");
            std::env::set_var("JCODE_TEST_BUNDLE", name);
            format!("OK: test bundle '{}' started", name)
        } else if cmd == "bundle-save" {
            use crate::tui::test_harness::TestBundle;
            let name = std::env::var("JCODE_TEST_BUNDLE").unwrap_or_else(|_| "unnamed".to_string());
            let bundle = TestBundle::new(&name);
            let path = TestBundle::default_path(&name);
            match bundle.save(&path) {
                Ok(_) => format!("OK: bundle saved to {}", path.display()),
                Err(e) => format!("bundle-save error: {}", e),
            }
        } else if cmd.starts_with("script:") {
            let raw = cmd.strip_prefix("script:").unwrap_or("{}");
            match serde_json::from_str::<crate::tui::test_harness::TestScript>(raw) {
                Ok(script) => self.handle_test_script(script),
                Err(e) => format!("script error: {}", e),
            }
        } else if cmd == "version" {
            format!("version: {}", env!("JCODE_VERSION"))
        } else if cmd == "help" {
            "Debug commands:\n\
                 - message:<text> - inject and submit a message\n\
                 - inject:<role>:<text> - inject display message without sending\n\
                 - reload - trigger /reload\n\
                 - state - get basic state info\n\
                 - snapshot - get combined state + frame snapshot JSON\n\
                 - assert:<json> - run assertions (see docs)\n\
                 - run:<json> - run scripted steps + assertions\n\
                 - trace-start - start recording trace events\n\
                 - trace-stop - stop recording trace events\n\
                 - trace - dump trace events JSON\n\
                 - quit - exit the TUI\n\
                 - last_response - get last assistant message\n\
                 - history - get all messages as JSON\n\
                 - screen - dump visual debug frames\n\
                 - screen-json - dump latest visual frame JSON\n\
                 - screen-json-normalized - dump normalized frame (for diffs)\n\
                 - frame - alias for screen-json\n\
                 - frame-normalized - alias for screen-json-normalized\n\
                 - layout - dump latest layout JSON\n\
                 - margins - dump layout margins JSON\n\
                 - widgets - dump info widget summary/placements\n\
                 - render-stats - dump render timing + order JSON\n\
                 - render-order - dump render order list\n\
                 - anomalies - dump visual debug anomalies\n\
                 - theme - dump current palette snapshot\n\
                 - mermaid:stats - dump mermaid debug stats\n\
                 - mermaid:cache - list mermaid cache entries\n\
                 - mermaid:evict - clear mermaid cache\n\
                 - markdown:stats - dump markdown debug stats\n\
                 - overlay:on/off/status - toggle overlay boxes\n\
                 - enable/disable/status - control visual debug capture\n\
                 - wait - check if processing\n\
                 - wait:<ms> - block until idle or timeout\n\
                 - scroll:<up|down|top|bottom> - control scroll\n\
                 - scroll-test[:<json>] - run offscreen scroll+diagram test\n\
                 - scroll-suite[:<json>] - run scroll+diagram test suite\n\
                 - keys:<keyspec> - inject key events (e.g. keys:ctrl+r)\n\
                 - input - get current input buffer\n\
                 - set_input:<text> - set input buffer\n\
                 - submit - submit current input\n\
                 - record-start - start event recording\n\
                 - record-stop - stop event recording\n\
                 - record-events - get recorded events JSON\n\
                 - clock-enable - enable deterministic test clock\n\
                 - clock-disable - disable test clock\n\
                 - clock-advance:<ms> - advance test clock\n\
                 - clock-now - get current clock time\n\
                 - replay:<json> - replay recorded events\n\
                 - bundle-start:<name> - start test bundle\n\
                 - bundle-save - save test bundle\n\
                 - script:<json> - run test script\n\
                 - version - get version\n\
                 - help - show this help"
                .to_string()
        } else {
            format!("ERROR: unknown command '{}'. Use 'help' for list.", cmd)
        }
    }

    /// Check for new stable version and trigger migration if at safe point
    pub(super) fn check_stable_version(&mut self) {
        // Only check every 5 seconds to avoid excessive file reads
        let should_check = self
            .last_version_check
            .map(|t| t.elapsed() > Duration::from_secs(5))
            .unwrap_or(true);

        if !should_check {
            return;
        }

        self.last_version_check = Some(Instant::now());

        // Don't migrate if we're a canary session (we test changes, not receive them)
        if self.session.is_canary {
            return;
        }

        // Read current stable version
        let current_stable = match crate::build::read_stable_version() {
            Ok(Some(v)) => v,
            _ => return,
        };

        // Check if it changed
        let version_changed = self
            .known_stable_version
            .as_ref()
            .map(|v| v != &current_stable)
            .unwrap_or(true);

        if !version_changed {
            return;
        }

        // New stable version detected
        self.known_stable_version = Some(current_stable.clone());

        // Check if we're at a safe point to migrate
        let at_safe_point = !self.is_processing && self.queued_messages.is_empty();

        if at_safe_point {
            // Trigger migration
            self.pending_migration = Some(current_stable);
        }
    }

    /// Execute pending migration to new stable version
    pub(super) fn execute_migration(&mut self) -> bool {
        if let Some(ref version) = self.pending_migration.take() {
            let stable_binary = match crate::build::stable_binary_path() {
                Ok(p) if p.exists() => p,
                _ => return false,
            };

            // Save session before migration
            if let Err(e) = self.session.save() {
                let msg = format!("Failed to save session before migration: {}", e);
                crate::logging::error(&msg);
                self.push_display_message(DisplayMessage::error(msg));
                self.set_status_notice("Migration aborted");
                return false;
            }

            // Request reload to stable version
            self.save_input_for_reload(&self.session.id.clone());
            self.reload_requested = Some(self.session.id.clone());

            // The actual exec happens in main.rs when run() returns
            // We store the binary path in an env var for the reload handler
            std::env::set_var("JCODE_MIGRATE_BINARY", stable_binary);

            crate::logging::info(&format!("Migrating to stable version {}...", version));
            self.set_status_notice(format!("Migrating to stable {}...", version));
            self.should_quit = true;
            return true;
        }
        false
    }

    fn build_debug_snapshot(&self) -> DebugSnapshot {
        let frame = crate::tui::visual_debug::latest_frame();
        let recent_messages = self
            .display_messages
            .iter()
            .rev()
            .take(20)
            .map(|msg| DebugMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls: msg.tool_calls.clone(),
                duration_secs: msg.duration_secs,
                title: msg.title.clone(),
            })
            .collect::<Vec<_>>();
        DebugSnapshot {
            state: serde_json::json!({
                "processing": self.is_processing,
                "messages": self.messages.len(),
                "display_messages": self.display_messages.len(),
                "input": self.input,
                "cursor_pos": self.cursor_pos,
                "scroll_offset": self.scroll_offset,
                "queued_messages": self.queued_messages.len(),
                "provider_session_id": self.provider_session_id,
                "model": self.provider.name(),
                "diagram_mode": format!("{:?}", self.diagram_mode),
                "diagram_pane_enabled": self.diagram_pane_enabled,
                "diagram_pane_position": format!("{:?}", self.diagram_pane_position),
                "diagram_zoom": self.diagram_zoom,
                "version": env!("JCODE_VERSION"),
            }),
            frame,
            recent_messages,
            queued_messages: self.queued_messages.clone(),
        }
    }

    fn eval_assertions(&self, assertions: &[DebugAssertion]) -> Vec<DebugAssertResult> {
        let snapshot = self.build_debug_snapshot();
        let mut results = Vec::new();
        for assertion in assertions {
            let actual = self.lookup_snapshot_value(&snapshot, &assertion.field);
            let expected = assertion.value.clone();
            let op = assertion.op.as_str();
            let ok = match op {
                "eq" => actual == expected,
                "ne" => actual != expected,
                "contains" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(b)) => a.contains(b),
                    (serde_json::Value::Array(a), _) => a.contains(&expected),
                    _ => false,
                },
                "not_contains" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(b)) => !a.contains(b),
                    (serde_json::Value::Array(a), _) => !a.contains(&expected),
                    _ => true,
                },
                "exists" => actual != serde_json::Value::Null,
                "not_exists" => actual == serde_json::Value::Null,
                "gt" => match (&actual, &expected) {
                    (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                        a.as_f64().unwrap_or(0.0) > b.as_f64().unwrap_or(0.0)
                    }
                    _ => false,
                },
                "gte" => match (&actual, &expected) {
                    (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                        a.as_f64().unwrap_or(0.0) >= b.as_f64().unwrap_or(0.0)
                    }
                    _ => false,
                },
                "lt" => match (&actual, &expected) {
                    (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                        a.as_f64().unwrap_or(0.0) < b.as_f64().unwrap_or(0.0)
                    }
                    _ => false,
                },
                "lte" => match (&actual, &expected) {
                    (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
                        a.as_f64().unwrap_or(0.0) <= b.as_f64().unwrap_or(0.0)
                    }
                    _ => false,
                },
                "len" => match &actual {
                    serde_json::Value::String(s) => expected
                        .as_u64()
                        .map(|e| s.len() as u64 == e)
                        .unwrap_or(false),
                    serde_json::Value::Array(a) => expected
                        .as_u64()
                        .map(|e| a.len() as u64 == e)
                        .unwrap_or(false),
                    serde_json::Value::Object(o) => expected
                        .as_u64()
                        .map(|e| o.len() as u64 == e)
                        .unwrap_or(false),
                    _ => false,
                },
                "len_gt" => match &actual {
                    serde_json::Value::String(s) => expected
                        .as_u64()
                        .map(|e| s.len() as u64 > e)
                        .unwrap_or(false),
                    serde_json::Value::Array(a) => expected
                        .as_u64()
                        .map(|e| a.len() as u64 > e)
                        .unwrap_or(false),
                    _ => false,
                },
                "len_lt" => match &actual {
                    serde_json::Value::String(s) => expected
                        .as_u64()
                        .map(|e| (s.len() as u64) < e)
                        .unwrap_or(false),
                    serde_json::Value::Array(a) => expected
                        .as_u64()
                        .map(|e| (a.len() as u64) < e)
                        .unwrap_or(false),
                    _ => false,
                },
                "matches" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(pattern)) => {
                        regex::Regex::new(pattern)
                            .map(|re| re.is_match(a))
                            .unwrap_or(false)
                    }
                    _ => false,
                },
                "not_matches" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(pattern)) => {
                        regex::Regex::new(pattern)
                            .map(|re| !re.is_match(a))
                            .unwrap_or(true)
                    }
                    _ => true,
                },
                "starts_with" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(b)) => {
                        a.starts_with(b)
                    }
                    _ => false,
                },
                "ends_with" => match (&actual, &expected) {
                    (serde_json::Value::String(a), serde_json::Value::String(b)) => a.ends_with(b),
                    _ => false,
                },
                "is_empty" => match &actual {
                    serde_json::Value::String(s) => s.is_empty(),
                    serde_json::Value::Array(a) => a.is_empty(),
                    serde_json::Value::Object(o) => o.is_empty(),
                    serde_json::Value::Null => true,
                    _ => false,
                },
                "is_not_empty" => match &actual {
                    serde_json::Value::String(s) => !s.is_empty(),
                    serde_json::Value::Array(a) => !a.is_empty(),
                    serde_json::Value::Object(o) => !o.is_empty(),
                    serde_json::Value::Null => false,
                    _ => true,
                },
                "is_true" => actual == serde_json::Value::Bool(true),
                "is_false" => actual == serde_json::Value::Bool(false),
                _ => false,
            };
            let message = if ok {
                "ok".to_string()
            } else {
                format!(
                    "expected {} {} {:?}, got {:?}",
                    assertion.field, op, expected, actual
                )
            };
            results.push(DebugAssertResult {
                ok,
                field: assertion.field.clone(),
                op: assertion.op.clone(),
                expected,
                actual,
                message,
            });
        }
        results
    }

    fn handle_assertions(&mut self, raw: &str) -> String {
        let parsed: Result<Vec<DebugAssertion>, _> = serde_json::from_str(raw);
        let assertions = match parsed {
            Ok(a) => a,
            Err(e) => {
                return format!("assert parse error: {}", e);
            }
        };
        let results = self.eval_assertions(&assertions);
        serde_json::to_string_pretty(&results).unwrap_or_else(|_| "[]".to_string())
    }

    fn handle_script_run(&mut self, raw: &str) -> String {
        let parsed: Result<DebugScript, _> = serde_json::from_str(raw);
        let script = match parsed {
            Ok(s) => s,
            Err(e) => return format!("run parse error: {}", e),
        };

        let mut steps = Vec::new();
        let mut ok = true;
        for step in &script.steps {
            let detail = self.execute_script_step(step);
            let step_ok = !detail.starts_with("ERR");
            if !step_ok {
                ok = false;
            }
            steps.push(DebugStepResult {
                step: step.clone(),
                ok: step_ok,
                detail,
            });
        }

        if let Some(wait_ms) = script.wait_ms {
            let _ = self.apply_wait_ms(wait_ms);
        }

        let assertions = self.eval_assertions(&script.assertions);
        if assertions.iter().any(|a| !a.ok) {
            ok = false;
        }

        let report = DebugRunReport {
            ok,
            steps,
            assertions,
        };

        serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
    }

    fn handle_test_script(&mut self, script: crate::tui::test_harness::TestScript) -> String {
        use crate::tui::test_harness::TestStep;

        let mut results = Vec::new();
        for step in &script.steps {
            let step_result = match step {
                TestStep::Message { content } => {
                    self.input = content.clone();
                    self.submit_input();
                    format!("message: {}", content)
                }
                TestStep::SetInput { text } => {
                    self.input = text.clone();
                    self.cursor_pos = self.input.len();
                    format!("set_input: {}", text)
                }
                TestStep::Submit => {
                    if !self.input.is_empty() {
                        self.submit_input();
                        "submit: OK".to_string()
                    } else {
                        "submit: skipped (empty)".to_string()
                    }
                }
                TestStep::WaitIdle { timeout_ms } => {
                    let _ = self.apply_wait_ms(timeout_ms.unwrap_or(30000));
                    "wait_idle: done".to_string()
                }
                TestStep::Wait { ms } => {
                    std::thread::sleep(std::time::Duration::from_millis(*ms));
                    format!("wait: {}ms", ms)
                }
                TestStep::Checkpoint { name } => format!("checkpoint: {}", name),
                TestStep::Command { cmd } => {
                    format!("command: {} (nested commands not supported)", cmd)
                }
                TestStep::Keys { keys } => {
                    let mut key_results = Vec::new();
                    for key_spec in keys.split(',') {
                        match self.parse_and_inject_key(key_spec.trim()) {
                            Ok(desc) => key_results.push(format!("OK: {}", desc)),
                            Err(e) => key_results.push(format!("ERR: {}", e)),
                        }
                    }
                    format!("keys: {}", key_results.join(", "))
                }
                TestStep::Scroll { direction } => {
                    match direction.as_str() {
                        "up" => self.debug_scroll_up(5),
                        "down" => self.debug_scroll_down(5),
                        "top" => self.debug_scroll_top(),
                        "bottom" => self.debug_scroll_bottom(),
                        _ => {}
                    }
                    format!("scroll: {}", direction)
                }
                TestStep::Assert { assertions } => {
                    let parsed: Vec<DebugAssertion> = assertions
                        .iter()
                        .filter_map(|a| serde_json::from_value(a.clone()).ok())
                        .collect();
                    let results = self.eval_assertions(&parsed);
                    let passed = results.iter().all(|r| r.ok);
                    format!(
                        "assert: {} ({}/{})",
                        if passed { "PASS" } else { "FAIL" },
                        results.iter().filter(|r| r.ok).count(),
                        results.len()
                    )
                }
                TestStep::Snapshot { name } => format!("snapshot: {}", name),
            };
            results.push(step_result);
        }

        serde_json::json!({
            "script": script.name,
            "steps": results,
            "completed": true
        })
        .to_string()
    }

    fn apply_wait_ms(&mut self, wait_ms: u64) -> String {
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        while Instant::now() < deadline {
            if !self.is_processing {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.debug_trace.record("wait", format!("{}ms", wait_ms));
        format!("waited {}ms", wait_ms)
    }

    fn lookup_snapshot_value(&self, snapshot: &DebugSnapshot, field: &str) -> serde_json::Value {
        let parts: Vec<&str> = field.split('.').collect();
        if parts.is_empty() {
            return serde_json::Value::Null;
        }
        match parts[0] {
            "state" => Self::lookup_json_path(&snapshot.state, &parts[1..]),
            "frame" => {
                if let Some(frame) = &snapshot.frame {
                    let value = serde_json::to_value(frame).unwrap_or(serde_json::Value::Null);
                    Self::lookup_json_path(&value, &parts[1..])
                } else {
                    serde_json::Value::Null
                }
            }
            "recent_messages" => {
                let value = serde_json::to_value(&snapshot.recent_messages)
                    .unwrap_or(serde_json::Value::Null);
                Self::lookup_json_path(&value, &parts[1..])
            }
            "queued_messages" => {
                let value = serde_json::to_value(&snapshot.queued_messages)
                    .unwrap_or(serde_json::Value::Null);
                Self::lookup_json_path(&value, &parts[1..])
            }
            _ => serde_json::Value::Null,
        }
    }

    fn lookup_json_path(value: &serde_json::Value, parts: &[&str]) -> serde_json::Value {
        let mut current = value;
        for part in parts {
            if let Ok(index) = part.parse::<usize>() {
                if let Some(v) = current.get(index) {
                    current = v;
                    continue;
                }
            }
            if let Some(v) = current.get(part) {
                current = v;
                continue;
            }
            return serde_json::Value::Null;
        }
        current.clone()
    }

    fn execute_script_step(&mut self, step: &str) -> String {
        let trimmed = step.trim();
        if trimmed.is_empty() {
            return "ERR: empty step".to_string();
        }
        if trimmed.starts_with("keys:") {
            let keys_str = trimmed.strip_prefix("keys:").unwrap_or("");
            let mut results = Vec::new();
            for key_spec in keys_str.split(',') {
                match self.parse_and_inject_key(key_spec.trim()) {
                    Ok(desc) => {
                        self.debug_trace.record("key", desc.clone());
                        results.push(format!("OK: {}", desc));
                    }
                    Err(e) => results.push(format!("ERR: {}", e)),
                }
            }
            return results.join("\n");
        }
        if trimmed.starts_with("set_input:") {
            let new_input = trimmed.strip_prefix("set_input:").unwrap_or("");
            self.input = new_input.to_string();
            self.cursor_pos = self.input.len();
            self.debug_trace
                .record("input", format!("set:{}", self.input));
            return format!("OK: input set to {:?}", self.input);
        }
        if trimmed == "submit" {
            if self.input.is_empty() {
                return "ERR: input is empty".to_string();
            }
            self.submit_input();
            self.debug_trace.record("input", "submitted".to_string());
            return "OK: submitted".to_string();
        }
        if trimmed.starts_with("message:") {
            let msg = trimmed.strip_prefix("message:").unwrap_or("");
            self.input = msg.to_string();
            self.submit_input();
            self.debug_trace
                .record("message", format!("submitted:{}", msg));
            return format!("OK: queued message '{}'", msg);
        }
        if trimmed.starts_with("scroll:") {
            let dir = trimmed.strip_prefix("scroll:").unwrap_or("");
            return match dir {
                "up" => {
                    self.debug_scroll_up(5);
                    format!("scroll: up to {}", self.scroll_offset)
                }
                "down" => {
                    self.debug_scroll_down(5);
                    format!("scroll: down to {}", self.scroll_offset)
                }
                "top" => {
                    self.debug_scroll_top();
                    "scroll: top".to_string()
                }
                "bottom" => {
                    self.debug_scroll_bottom();
                    "scroll: bottom".to_string()
                }
                _ => format!("ERR: unknown scroll '{}'", dir),
            };
        }
        if trimmed == "reload" {
            self.input = "/reload".to_string();
            self.submit_input();
            self.debug_trace.record("reload", "triggered".to_string());
            return "OK: reload triggered".to_string();
        }
        if trimmed == "snapshot" {
            let snapshot = self.build_debug_snapshot();
            return serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string());
        }
        if trimmed.starts_with("wait:") {
            let raw = trimmed.strip_prefix("wait:").unwrap_or("0");
            if let Ok(ms) = raw.parse::<u64>() {
                return self.apply_wait_ms(ms);
            }
            return format!("ERR: invalid wait '{}'", raw);
        }
        if trimmed == "wait" {
            return if self.is_processing {
                "wait: processing".to_string()
            } else {
                "wait: idle".to_string()
            };
        }
        format!("ERR: unknown step '{}'", trimmed)
    }

    pub(super) fn check_debug_command(&mut self) -> Option<String> {
        let cmd_path = debug_cmd_path();
        if let Ok(cmd) = std::fs::read_to_string(&cmd_path) {
            // Remove command file immediately
            let _ = std::fs::remove_file(&cmd_path);
            let cmd = cmd.trim();

            self.debug_trace
                .record("cmd", format!("{}", cmd.to_string()));

            let response = self.handle_debug_command(cmd);

            // Write response
            let _ = std::fs::write(debug_response_path(), &response);
            return Some(response);
        }
        None
    }

    pub(super) fn parse_key_spec(&self, key_spec: &str) -> Result<(KeyCode, KeyModifiers), String> {
        let key_spec = key_spec.to_lowercase();
        let parts: Vec<&str> = key_spec.split('+').collect();

        let mut modifiers = KeyModifiers::empty();
        let mut key_part = "";

        for part in &parts {
            match *part {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                _ => key_part = part,
            }
        }

        let key_code = match key_part {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            s if s.len() == 1 => KeyCode::Char(s.chars().next().unwrap()),
            s if s.starts_with('f') && s.len() <= 3 => {
                if let Ok(n) = s[1..].parse::<u8>() {
                    KeyCode::F(n)
                } else {
                    return Err(format!("Invalid function key: {}", s));
                }
            }
            _ => return Err(format!("Unknown key: {}", key_part)),
        };

        Ok((key_code, modifiers))
    }

    /// Parse a key specification and inject it as an event
    pub(super) fn parse_and_inject_key(&mut self, key_spec: &str) -> Result<String, String> {
        let (key_code, modifiers) = self.parse_key_spec(key_spec)?;
        let key_event = crossterm::event::KeyEvent::new(key_code, modifiers);
        self.handle_key_event(key_event);
        Ok(format!("injected {:?} with {:?}", key_code, modifiers))
    }
}
