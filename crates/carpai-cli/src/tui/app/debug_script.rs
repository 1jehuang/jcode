use super::*;

impl App {
    pub(in crate::tui::app) fn build_debug_snapshot(&self) -> DebugSnapshot {
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
                "connection_type": self.connection_type,
                "remote_transport": self.remote_transport,
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

    pub(in crate::tui::app) fn handle_assertions(&mut self, raw: &str) -> String {
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

    pub(in crate::tui::app) fn handle_script_run(&mut self, raw: &str) -> String {
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

    pub(in crate::tui::app) fn handle_test_script(
        &mut self,
        script: crate::tui::test_harness::TestScript,
    ) -> String {
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

    pub(in crate::tui::app) fn apply_wait_ms(&mut self, wait_ms: u64) -> String {
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
            if let Ok(index) = part.parse::<usize>()
                && let Some(v) = current.get(index)
            {
                current = v;
                continue;
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

    pub(in crate::tui::app) fn check_debug_command(&mut self) -> Option<String> {
        let cmd_path = debug_cmd_path();
        if let Ok(cmd) = std::fs::read_to_string(&cmd_path) {
            // Remove command file immediately
            let _ = std::fs::remove_file(&cmd_path);
            let cmd = cmd.trim();

            self.debug_trace.record("cmd", cmd.to_string());

            let response = self.handle_debug_command(cmd);

            // Write response
            let _ = std::fs::write(debug_response_path(), &response);
            return Some(response);
        }
        None
    }

    pub(in crate::tui::app) fn parse_key_spec(
        &self,
        key_spec: &str,
    ) -> Result<(KeyCode, KeyModifiers), String> {
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
            s if s.len() == 1 => KeyCode::Char(
                s.parse::<char>()
                    .map_err(|_| "Key spec unexpectedly had no character".to_string())?,
            ),
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
    pub(in crate::tui::app) fn parse_and_inject_key(
        &mut self,
        key_spec: &str,
    ) -> Result<String, String> {
        let (key_code, modifiers) = self.parse_key_spec(key_spec)?;
        let key_event = crossterm::event::KeyEvent::new(key_code, modifiers);
        self.handle_key_event(key_event);
        Ok(format!("injected {:?} with {:?}", key_code, modifiers))
    }
}
