use super::*;

impl App {
    pub(in crate::tui::app) fn handle_debug_command(&mut self, cmd: &str) -> String {
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
                        is_headless: Some(false),
                        live_attachments: Some(1),
                        status_age_secs: Some(0),
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
        } else if cmd == "memory" {
            serde_json::to_string_pretty(&self.debug_memory_profile())
                .unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "memory-history" {
            let payload = crate::process_memory::history(128);
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "[]".to_string())
        } else if cmd == "slow-frames" {
            let payload = crate::tui::ui::debug_slow_frame_history(32);
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("slow-frames ") {
            let raw_limit = cmd.strip_prefix("slow-frames ").unwrap_or("32").trim();
            let limit = raw_limit.parse::<usize>().unwrap_or(32);
            let payload = crate::tui::ui::debug_slow_frame_history(limit);
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "flicker-frames" {
            let payload = crate::tui::ui::debug_flicker_frame_history(32);
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("flicker-frames ") {
            let raw_limit = cmd.strip_prefix("flicker-frames ").unwrap_or("32").trim();
            let limit = raw_limit.parse::<usize>().unwrap_or(32);
            let payload = crate::tui::ui::debug_flicker_frame_history(limit);
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:memory-bench" {
            let result = crate::tui::mermaid::debug_memory_benchmark(40);
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:flicker-bench" {
            let result = crate::tui::mermaid::debug_flicker_benchmark(24);
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
        } else if cmd.starts_with("mermaid:flicker-bench ") {
            let raw_steps = cmd
                .strip_prefix("mermaid:flicker-bench ")
                .unwrap_or("")
                .trim();
            let steps = match raw_steps.parse::<usize>() {
                Ok(v) => v,
                Err(_) => return "Invalid steps (expected integer)".to_string(),
            };
            let result = crate::tui::mermaid::debug_flicker_benchmark(steps);
            serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string())
        } else if cmd == "mermaid:ui-bench" || cmd.starts_with("mermaid:ui-bench:") {
            let raw = cmd.strip_prefix("mermaid:ui-bench:");
            self.run_mermaid_ui_bench(raw)
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
        } else if cmd == "side-panel:stats" || cmd == "side-panel:debug" {
            crate::tui::side_panel_debug_json()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| "null".to_string())
        } else if cmd == "diagram-pane:stats"
            || cmd == "diagram-pane:debug"
            || cmd == "pinned-diagram:stats"
        {
            crate::tui::pinned_diagram_debug_json()
                .and_then(|value| serde_json::to_string_pretty(&value).ok())
                .unwrap_or_else(|| "null".to_string())
        } else if cmd == "markdown:memory" {
            let profile = crate::tui::markdown::debug_memory_profile();
            serde_json::to_string_pretty(&profile).unwrap_or_else(|_| "{}".to_string())
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
                    "user" | "assistant" | "system" | "background_task" | "tool" | "error"
                    | "meta" => r,
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
        } else if cmd == "side-panel-latency" || cmd.starts_with("side-panel-latency:") {
            let raw = cmd.strip_prefix("side-panel-latency:");
            self.run_side_panel_latency_bench(raw)
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
                        self.debug_trace.record("key", desc.to_string());
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
            crate::env::set_var("JCODE_TEST_BUNDLE", name);
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
                 - mermaid:memory - dump mermaid memory profile\n\
                 - mermaid:flicker-bench [n] - benchmark viewport protocol churn / flicker risk\n\
                 - mermaid:ui-bench[:<json>] - benchmark live mermaid UI render path\n\
                 - mermaid:cache - list mermaid cache entries\n\
                 - mermaid:evict - clear mermaid cache\n\
                 - markdown:stats - dump markdown debug stats\n\
                 - markdown:memory - dump markdown cache memory estimate\n\
                 - memory - dump aggregate client memory profile\n\
                 - memory-history - dump recent process memory samples\n\
                 - slow-frames [n] - dump recent slow-frame records\n\
                 - flicker-frames [n] - dump recent frame-stability and flicker records\n\
                 - overlay:on/off/status - toggle overlay boxes\n\
                 - enable/disable/status - control visual debug capture\n\
                 - wait - check if processing\n\
                 - wait:<ms> - block until idle or timeout\n\
                 - scroll:<up|down|top|bottom> - control scroll\n\
                 - scroll-test[:<json>] - run offscreen scroll+diagram test\n\
                 - scroll-suite[:<json>] - run scroll+diagram test suite\n\
                 - side-panel-latency[:<json>] - benchmark headless side-panel input->frame latency\n\
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
    pub(in crate::tui::app) fn check_stable_version(&mut self) -> bool {
        // Only check every 5 seconds to avoid excessive file reads
        let should_check = self
            .last_version_check
            .map(|t| t.elapsed() > Duration::from_secs(5))
            .unwrap_or(true);

        if !should_check {
            return false;
        }

        self.last_version_check = Some(Instant::now());

        // Don't migrate if we're a canary session (we test changes, not receive them)
        if self.session.is_canary {
            return false;
        }

        // Read current stable version
        let current_stable = match crate::build::read_stable_version() {
            Ok(Some(v)) => v,
            _ => return false,
        };

        // Check if it changed
        let version_changed = self
            .known_stable_version
            .as_ref()
            .map(|v| v != &current_stable)
            .unwrap_or(true);

        if !version_changed {
            return false;
        }

        // New stable version detected
        self.known_stable_version = Some(current_stable.clone());

        // Check if we're at a safe point to migrate
        let at_safe_point = !self.is_processing && self.queued_messages.is_empty();

        if at_safe_point {
            // Trigger migration
            self.pending_migration = Some(current_stable);
            return true;
        }

        false
    }

    /// Execute pending migration to new stable version
    pub(in crate::tui::app) fn execute_migration(&mut self) -> bool {
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
            crate::env::set_var("JCODE_MIGRATE_BINARY", stable_binary);

            crate::logging::info(&format!("Migrating to stable version {}...", version));
            self.set_status_notice(format!("Migrating to stable {}...", version));
            self.should_quit = true;
            return true;
        }
        false
    }
}
