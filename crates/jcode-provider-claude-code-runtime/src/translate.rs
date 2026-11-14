//! stream-json stdout -> jcode `StreamEvent` translation.
//!
//! One [`Translator`] lives for one user turn. Control-channel lines
//! (`control_request`, `control_response`, `keep_alive`, ...) are handled by
//! the turn driver and never reach this module.

use jcode_message_types::StreamEvent;
use serde_json::Value;
use std::collections::HashSet;

/// jcode tool names the agent loop re-executes even for providers that run
/// tools internally. Display names must never collide with these.
pub const JCODE_REEXECUTED_TOOLS: &[&str] = &["selfdev", "desktop_selfdev", "communicate"];

/// Prefix Claude Code gives tools served by our in-process SDK MCP server.
pub const JCODE_MCP_PREFIX: &str = "mcp__jcode__";

/// One rate-limit window (utilization 0..1, reset in unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
pub struct RateLimitWindow {
    pub utilization: Option<f64>,
    pub resets_at: Option<i64>,
}

/// Latest rate-limit state reported by `rate_limit_event`.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct ClaudeCodeRateLimits {
    /// `allowed`, `allowed_warning` or `rejected`.
    pub status: String,
    /// Window that triggered this event (`five_hour`, `seven_day`, ...).
    pub rate_limit_type: Option<String>,
    pub utilization: Option<f64>,
    pub resets_at: Option<i64>,
    pub five_hour: Option<RateLimitWindow>,
    pub seven_day: Option<RateLimitWindow>,
    pub is_using_overage: bool,
    /// Unix seconds when this snapshot was observed.
    pub observed_at: i64,
}

impl ClaudeCodeRateLimits {
    pub fn is_rejected(&self) -> bool {
        self.status == "rejected" && !self.is_using_overage
    }

    pub fn from_event(info: &Value, now: i64) -> Self {
        let window = |key: &str| {
            info.get("unifiedWindows")
                .and_then(|w| w.get(key))
                .map(|w| RateLimitWindow {
                    utilization: w.get("utilization").and_then(Value::as_f64),
                    resets_at: w.get("resetsAt").and_then(Value::as_i64),
                })
        };
        Self {
            status: str_field(info, "status").unwrap_or_default(),
            rate_limit_type: str_field(info, "rateLimitType"),
            utilization: info.get("utilization").and_then(Value::as_f64),
            resets_at: info.get("resetsAt").and_then(Value::as_i64),
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
            is_using_overage: info
                .get("isUsingOverage")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            observed_at: now,
        }
    }
}

/// Side effects of a translated line that the driver must act on.
#[derive(Debug, Default)]
pub struct LineOutcome {
    pub events: Vec<StreamEvent>,
    /// The turn's `result` arrived: the turn is over.
    pub turn_finished: bool,
    /// A new rate-limit snapshot to store.
    pub rate_limits: Option<ClaudeCodeRateLimits>,
    /// The CLI parks a rate-limited turn silently; the driver must stop it.
    pub rate_limit_rejected: bool,
    /// Session id announced by the CLI (`system/init`, `result`).
    pub session_id: Option<String>,
}

/// Translator state for one turn.
pub struct Translator {
    /// Messages (by API message id) whose text/thinking streamed as partials.
    streamed_messages: HashSet<String>,
    /// Messages whose usage was already reported.
    usage_reported: HashSet<String>,
    /// Tool use ids already surfaced (snapshots repeat).
    tool_uses_seen: HashSet<String>,
    current_stream_message: Option<String>,
    in_thinking: bool,
    emitted_text: bool,
    message_has_text: bool,
    emitted_any_usage: bool,
    auth_failed: bool,
    rate_limited: bool,
    last_stop_reason: Option<String>,
    /// Login command shown when authentication fails.
    login_hint: String,
    /// Our turn's user message uuid; results for other turns are ignored.
    turn_uuid: Option<String>,
    now: fn() -> i64,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Display name for a Claude Code tool. `mcp__jcode__x` renders as `x`, unless
/// that would collide with a tool jcode re-executes.
pub fn display_tool_name(name: &str) -> String {
    if let Some(bare) = name.strip_prefix(JCODE_MCP_PREFIX)
        && !bare.is_empty()
        && !JCODE_REEXECUTED_TOOLS.contains(&bare)
    {
        return bare.to_string();
    }
    if JCODE_REEXECUTED_TOOLS.contains(&name) {
        return format!("claude_code_{name}");
    }
    name.to_string()
}

/// Flatten tool_result content (string or block array) to display text.
pub fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            let parts: Vec<String> = items
                .iter()
                .map(|item| match item.get("type").and_then(Value::as_str) {
                    Some("text") => str_field(item, "text").unwrap_or_default(),
                    Some("tool_reference") => format!(
                        "[tool loaded: {}]",
                        str_field(item, "tool_name").unwrap_or_default()
                    ),
                    Some("image") => "[image]".to_string(),
                    _ => item.to_string(),
                })
                .collect();
            parts.join("\n")
        }
        Some(other) => other.to_string(),
    }
}

fn usage_event(usage: &Value) -> Option<StreamEvent> {
    let get = |k: &str| usage.get(k).and_then(Value::as_u64);
    let input_tokens = get("input_tokens");
    let output_tokens = get("output_tokens");
    let cache_read_input_tokens = get("cache_read_input_tokens");
    let cache_creation_input_tokens = get("cache_creation_input_tokens");
    if input_tokens.is_none()
        && output_tokens.is_none()
        && cache_read_input_tokens.is_none()
        && cache_creation_input_tokens.is_none()
    {
        return None;
    }
    Some(StreamEvent::TokenUsage {
        input_tokens,
        output_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens,
    })
}

/// Seconds until `resets_at`, at least 1.
fn secs_until(resets_at: Option<i64>, now: i64) -> Option<u64> {
    resets_at.map(|at| (at - now).max(1) as u64)
}

/// Human window label.
fn window_label(kind: Option<&str>) -> &'static str {
    match kind {
        Some("five_hour") => "5-hour",
        Some("seven_day") => "weekly",
        Some("seven_day_opus") => "weekly Opus",
        Some("seven_day_sonnet") => "weekly Sonnet",
        Some("overage") => "overage",
        _ => "usage",
    }
}

fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    if hours >= 24 {
        format!("{}d {}h", hours / 24, hours % 24)
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{}m", minutes.max(1))
    }
}

/// Error event for a rejected rate limit.
pub fn rate_limit_error(limits: &ClaudeCodeRateLimits, now: i64) -> StreamEvent {
    let retry = secs_until(limits.resets_at, now);
    let label = window_label(limits.rate_limit_type.as_deref());
    let mut message = match retry {
        Some(secs) => format!(
            "Claude Code {label} limit reached; resets in {}.",
            format_duration(secs)
        ),
        None => format!("Claude Code {label} limit reached."),
    };
    let resets_in = limits.resets_at.map(|at| at - now);
    if jcode_provider_core::usage_limit::is_far_usage_limit_reset(resets_in) {
        message.push(' ');
        message.push_str(&jcode_provider_core::account_usage_limit_marker(
            limits.resets_at,
        ));
    }
    StreamEvent::Error {
        message,
        retry_after_secs: retry,
    }
}

impl Translator {
    pub fn new(login_hint: impl Into<String>, turn_uuid: Option<String>) -> Self {
        Self {
            streamed_messages: HashSet::new(),
            usage_reported: HashSet::new(),
            tool_uses_seen: HashSet::new(),
            current_stream_message: None,
            in_thinking: false,
            emitted_text: false,
            message_has_text: false,
            emitted_any_usage: false,
            auth_failed: false,
            rate_limited: false,
            last_stop_reason: None,
            login_hint: login_hint.into(),
            turn_uuid,
            now: unix_now,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, now: fn() -> i64) -> Self {
        self.now = now;
        self
    }

    /// Whether any visible assistant output was produced so far.
    pub fn produced_output(&self) -> bool {
        self.emitted_text || !self.tool_uses_seen.is_empty()
    }

    pub fn auth_failed(&self) -> bool {
        self.auth_failed
    }

    fn push_text(&mut self, out: &mut Vec<StreamEvent>, text: String) {
        if text.is_empty() {
            return;
        }
        if self.emitted_text && !self.message_has_text {
            out.push(StreamEvent::TextDelta("\n\n".into()));
        }
        self.emitted_text = true;
        self.message_has_text = true;
        out.push(StreamEvent::TextDelta(text));
    }

    fn close_thinking(&mut self, out: &mut Vec<StreamEvent>) {
        if self.in_thinking {
            self.in_thinking = false;
            out.push(StreamEvent::ThinkingEnd);
        }
    }

    /// Translate one parsed stdout line.
    pub fn handle(&mut self, line: &Value) -> LineOutcome {
        let mut out = LineOutcome::default();
        let parent = line
            .get("parent_tool_use_id")
            .map(|v| !v.is_null())
            .unwrap_or(false);
        match line.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                if !parent && let Some(event) = line.get("event") {
                    self.handle_stream_event(event, &mut out.events);
                }
            }
            Some("assistant") => self.handle_assistant(line, parent, &mut out.events),
            Some("user") => {
                if !parent {
                    self.handle_user(line, &mut out.events);
                }
            }
            Some("system") => self.handle_system(line, &mut out),
            Some("rate_limit_event") => {
                if let Some(info) = line.get("rate_limit_info") {
                    let limits = ClaudeCodeRateLimits::from_event(info, (self.now)());
                    if limits.is_rejected() {
                        self.close_thinking(&mut out.events);
                        out.events.push(rate_limit_error(&limits, (self.now)()));
                        out.rate_limit_rejected = true;
                        self.rate_limited = true;
                    }
                    out.rate_limits = Some(limits);
                }
            }
            Some("result") => self.handle_result(line, &mut out),
            Some("error") => {
                let message = line
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| line.get("error").and_then(Value::as_str))
                    .unwrap_or("Claude Code reported an error")
                    .to_string();
                out.events.push(StreamEvent::Error {
                    message,
                    retry_after_secs: None,
                });
            }
            Some("tool_progress") => {
                if let Some(name) = line.get("tool_name").and_then(Value::as_str) {
                    let secs = line
                        .get("elapsed_time_seconds")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0);
                    out.events.push(StreamEvent::StatusDetail {
                        detail: format!("{} running ({secs:.0}s)", display_tool_name(name)),
                    });
                }
            }
            _ => {}
        }
        out
    }

    fn handle_stream_event(&mut self, event: &Value, out: &mut Vec<StreamEvent>) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                let message = event.get("message");
                let id = message.and_then(|m| str_field(m, "id"));
                self.message_has_text = false;
                self.current_stream_message = id;
            }
            Some("content_block_start") => {
                let block = event.get("content_block");
                if block.and_then(|b| b.get("type")).and_then(Value::as_str) == Some("thinking") {
                    self.mark_streamed();
                    if !self.in_thinking {
                        self.in_thinking = true;
                        out.push(StreamEvent::ThinkingStart);
                    }
                }
            }
            Some("content_block_delta") => {
                let Some(delta) = event.get("delta") else {
                    return;
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        self.mark_streamed();
                        self.close_thinking(out);
                        let text = str_field(delta, "text").unwrap_or_default();
                        self.push_text(out, text);
                    }
                    Some("thinking_delta") => {
                        self.mark_streamed();
                        let text = str_field(delta, "thinking").unwrap_or_default();
                        if !text.is_empty() {
                            if !self.in_thinking {
                                self.in_thinking = true;
                                out.push(StreamEvent::ThinkingStart);
                            }
                            out.push(StreamEvent::ThinkingDelta(text));
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => self.close_thinking(out),
            Some("message_delta") => {
                if let Some(reason) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.last_stop_reason = Some(reason.to_string());
                }
                if let Some(usage) = event.get("usage").and_then(usage_event) {
                    if let Some(id) = &self.current_stream_message {
                        self.usage_reported.insert(id.clone());
                    }
                    self.emitted_any_usage = true;
                    out.push(usage);
                }
            }
            Some("message_stop") => self.close_thinking(out),
            Some("error") => {
                let message = event
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Claude API stream error")
                    .to_string();
                out.push(StreamEvent::Error {
                    message,
                    retry_after_secs: None,
                });
            }
            _ => {}
        }
    }

    fn mark_streamed(&mut self) {
        if let Some(id) = &self.current_stream_message {
            self.streamed_messages.insert(id.clone());
        }
    }

    fn handle_assistant(&mut self, line: &Value, parent: bool, out: &mut Vec<StreamEvent>) {
        match line.get("error").and_then(Value::as_str) {
            Some("authentication_failed") => self.auth_failed = true,
            Some("rate_limit") => self.rate_limited = true,
            _ => {}
        }
        let Some(message) = line.get("message") else {
            return;
        };
        let blocks = message
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if parent {
            for block in &blocks {
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let name = str_field(block, "name").unwrap_or_default();
                    out.push(StreamEvent::StatusDetail {
                        detail: format!("Subagent: {}", display_tool_name(&name)),
                    });
                }
            }
            return;
        }
        let id = str_field(message, "id").unwrap_or_default();
        let streamed = self.streamed_messages.contains(&id);
        if self.current_stream_message.as_deref() != Some(id.as_str()) && !streamed {
            // Snapshot-only message (no partials): new message boundary.
            if self.current_stream_message.is_some() || self.emitted_text {
                self.message_has_text = false;
            }
            self.current_stream_message = Some(id.clone());
        }
        for block in &blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") if !streamed => {
                    self.close_thinking(out);
                    let text = str_field(block, "text").unwrap_or_default();
                    self.push_text(out, text);
                }
                Some("thinking") if !streamed => {
                    let text = str_field(block, "thinking").unwrap_or_default();
                    if !text.is_empty() {
                        out.push(StreamEvent::ThinkingStart);
                        out.push(StreamEvent::ThinkingDelta(text));
                        out.push(StreamEvent::ThinkingEnd);
                    }
                }
                Some("tool_use") => {
                    let tool_id = str_field(block, "id").unwrap_or_default();
                    if !self.tool_uses_seen.insert(tool_id.clone()) {
                        continue;
                    }
                    self.close_thinking(out);
                    let name = str_field(block, "name").unwrap_or_default();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    out.push(StreamEvent::ToolUseStart {
                        id: tool_id,
                        name: display_tool_name(&name),
                    });
                    out.push(StreamEvent::ToolInputDelta(input.to_string()));
                    out.push(StreamEvent::ToolUseEnd);
                }
                _ => {}
            }
        }
        if !streamed
            && !self.usage_reported.contains(&id)
            && let Some(usage) = message.get("usage").and_then(usage_event)
        {
            self.usage_reported.insert(id);
            self.emitted_any_usage = true;
            out.push(usage);
        }
    }

    fn handle_user(&mut self, line: &Value, out: &mut Vec<StreamEvent>) {
        let Some(blocks) = line
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            out.push(StreamEvent::ToolResult {
                tool_use_id: str_field(block, "tool_use_id").unwrap_or_default(),
                content: tool_result_text(block.get("content")),
                is_error: block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
    }

    fn handle_system(&mut self, line: &Value, out: &mut LineOutcome) {
        match line.get("subtype").and_then(Value::as_str) {
            Some("init") => {
                out.session_id = str_field(line, "session_id");
            }
            Some("compact_boundary") => {
                let meta = line.get("compact_metadata");
                out.events.push(StreamEvent::Compaction {
                    trigger: meta
                        .and_then(|m| str_field(m, "trigger"))
                        .unwrap_or_else(|| "auto".into()),
                    pre_tokens: meta
                        .and_then(|m| m.get("pre_tokens"))
                        .and_then(Value::as_u64),
                    openai_encrypted_content: None,
                });
            }
            Some("api_retry") => {
                let attempt = line.get("attempt").and_then(Value::as_u64).unwrap_or(0);
                let max = line.get("max_retries").and_then(Value::as_u64).unwrap_or(0);
                let status = line
                    .get("error_status")
                    .and_then(Value::as_u64)
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default();
                out.events.push(StreamEvent::StatusDetail {
                    detail: format!("Claude API retry {attempt}/{max}{status}"),
                });
            }
            Some("status") => {
                if let Some(status) = line.get("status").and_then(Value::as_str)
                    && status == "compacting"
                {
                    out.events.push(StreamEvent::StatusDetail {
                        detail: "Claude Code is compacting the conversation".into(),
                    });
                }
            }
            Some("task_started") | Some("task_progress") => {
                if let Some(desc) = line
                    .get("description")
                    .or_else(|| line.get("summary"))
                    .and_then(Value::as_str)
                {
                    out.events.push(StreamEvent::StatusDetail {
                        detail: format!("Subagent: {desc}"),
                    });
                }
            }
            Some("local_command_output") => {
                if let Some(content) = line.get("content").and_then(Value::as_str) {
                    let content = content.to_string();
                    self.push_text(&mut out.events, content);
                }
            }
            _ => {}
        }
    }

    fn handle_result(&mut self, line: &Value, out: &mut LineOutcome) {
        if let (Some(ours), Some(uuids)) = (
            self.turn_uuid.as_deref(),
            line.get("user_message_uuids").and_then(Value::as_array),
        ) && !uuids.is_empty()
            && !uuids.iter().any(|u| u.as_str() == Some(ours))
        {
            return;
        }
        self.close_thinking(&mut out.events);
        out.turn_finished = true;
        out.session_id = str_field(line, "session_id");
        if !self.emitted_any_usage
            && let Some(usage) = line.get("usage").and_then(usage_event)
        {
            out.events.push(usage);
        }
        if let Some(error) = self.result_error(line) {
            out.events.push(error);
        }
        let stop_reason = str_field(line, "stop_reason").or(self.last_stop_reason.take());
        out.events.push(StreamEvent::MessageEnd { stop_reason });
    }

    fn result_error(&self, line: &Value) -> Option<StreamEvent> {
        let subtype = line.get("subtype").and_then(Value::as_str).unwrap_or("");
        let is_error = line
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let api_status = line.get("api_error_status").and_then(Value::as_u64);
        let terminal = line
            .get("terminal_reason")
            .and_then(Value::as_str)
            .unwrap_or("");
        let errors: Vec<String> = line
            .get("errors")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|e| !e.starts_with("[ede_diagnostic]"))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let result_text = str_field(line, "result").unwrap_or_default();

        if self.auth_failed || api_status == Some(401) {
            return Some(StreamEvent::Error {
                message: format!(
                    "Claude Code could not authenticate. Run `{}` and try again.",
                    self.login_hint
                ),
                retry_after_secs: None,
            });
        }
        if api_status == Some(529) {
            return Some(StreamEvent::Error {
                message: "Claude API is overloaded (529). Try again shortly.".into(),
                retry_after_secs: None,
            });
        }
        let interrupted = matches!(terminal, "aborted_tools" | "aborted_streaming")
            || errors.iter().any(|e| {
                let e = e.to_ascii_lowercase();
                e.contains("interrupt") || e.contains("aborted")
            });
        if interrupted {
            return None;
        }
        if !is_error && !subtype.starts_with("error") {
            return None;
        }
        if self.rate_limited && api_status.is_none_or(|s| s == 429) {
            // Already surfaced via rate_limit_event, or the assistant error flag.
            return Some(StreamEvent::Error {
                message:
                    "Claude usage limit reached. Send the message again once the limit resets."
                        .into(),
                retry_after_secs: None,
            });
        }
        let detail = if !errors.is_empty() {
            errors.join("; ")
        } else if !result_text.is_empty() {
            result_text
        } else if !terminal.is_empty() {
            terminal.replace('_', " ")
        } else {
            subtype.replace('_', " ")
        };
        let status = api_status
            .map(|s| format!(" (HTTP {s})"))
            .unwrap_or_default();
        Some(StreamEvent::Error {
            message: format!("Claude Code error{status}: {detail}"),
            retry_after_secs: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SIMPLE: &str = include_str!("../tests/fixtures/simple.jsonl");
    const TOOL: &str = include_str!("../tests/fixtures/tool.jsonl");
    const PERMISSION: &str = include_str!("../tests/fixtures/permission.jsonl");
    const SDK_MCP: &str = include_str!("../tests/fixtures/sdk_mcp.jsonl");

    fn clock() -> i64 {
        1_791_100_000
    }

    fn run(fixture: &str) -> (Vec<StreamEvent>, Vec<LineOutcome>) {
        let mut t = Translator::new("claude auth login", None).with_clock(clock);
        let mut events = Vec::new();
        let mut outcomes = Vec::new();
        for line in fixture.lines().filter(|l| !l.trim().is_empty()) {
            let value: Value = serde_json::from_str(line).unwrap();
            let ty = value.get("type").and_then(Value::as_str).unwrap_or("");
            if ty.starts_with("control_") {
                continue;
            }
            let outcome = t.handle(&value);
            events.extend(outcome.events.iter().cloned());
            outcomes.push(outcome);
        }
        (events, outcomes)
    }

    fn text(events: &[StreamEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn simple_turn_streams_text_once_and_ends() {
        let (events, outcomes) = run(SIMPLE);
        assert_eq!(
            text(&events),
            "OK",
            "snapshot text must not duplicate partials"
        );
        assert!(matches!(events.first(), Some(StreamEvent::ThinkingStart)));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ThinkingEnd)));
        let usage: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::TokenUsage { .. }))
            .collect();
        assert_eq!(usage.len(), 1, "message_delta usage only: {usage:?}");
        assert!(matches!(
            usage[0],
            StreamEvent::TokenUsage {
                output_tokens: Some(55),
                cache_read_input_tokens: Some(13803),
                ..
            }
        ));
        assert!(matches!(
            events.last(),
            Some(StreamEvent::MessageEnd { stop_reason: Some(r) }) if r == "end_turn"
        ));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::Error { .. }))
        );
        let finished: Vec<_> = outcomes.iter().filter(|o| o.turn_finished).collect();
        assert_eq!(finished.len(), 1);
        assert_eq!(
            finished[0].session_id.as_deref(),
            Some("8a32e911-d927-4172-9b1c-d4399dd50dc2")
        );
        assert!(
            outcomes
                .iter()
                .any(|o| o.session_id.is_some() && !o.turn_finished)
        );
        let limits = outcomes.iter().find_map(|o| o.rate_limits.clone()).unwrap();
        assert_eq!(limits.status, "allowed_warning");
        assert_eq!(limits.seven_day.unwrap().utilization, Some(0.89));
        assert_eq!(limits.five_hour.unwrap().resets_at, Some(1791123600));
        assert!(!outcomes.iter().any(|o| o.rate_limit_rejected));
    }

    #[test]
    fn tool_turn_without_partials_uses_snapshots() {
        let (events, _) = run(TOOL);
        let pos_start = events
            .iter()
            .position(|e| matches!(e, StreamEvent::ToolUseStart { id, name } if id == "toolu_015k6qhFR4NpzYda6vra2umD" && name == "Read"))
            .expect("tool start");
        assert!(
            matches!(&events[pos_start + 1], StreamEvent::ToolInputDelta(j) if j.contains("note.txt"))
        );
        assert!(matches!(events[pos_start + 2], StreamEvent::ToolUseEnd));
        assert!(events.iter().any(|e| matches!(e,
            StreamEvent::ToolResult { tool_use_id, content, is_error: false }
                if tool_use_id == "toolu_015k6qhFR4NpzYda6vra2umD" && content == "1\thello\n2\t")));
        assert_eq!(text(&events), "hello");
        // usage reported once per API message (two messages), not result total.
        let usage = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::TokenUsage { .. }))
            .count();
        assert_eq!(usage, 2);
        assert!(matches!(
            events.last(),
            Some(StreamEvent::MessageEnd { .. })
        ));
    }

    #[test]
    fn permission_fixture_reports_write_tool_and_result() {
        let (events, _) = run(PERMISSION);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolUseStart { name, .. } if name == "Write"))
        );
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolResult { content, .. } if content.starts_with("File created successfully"))));
        assert_eq!(text(&events), "Done.");
    }

    #[test]
    fn sdk_mcp_tool_names_are_stripped_for_display() {
        let (events, _) = run(SDK_MCP);
        let names: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolUseStart { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["ToolSearch", "echo"]);
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolResult { content, .. } if content == "[tool loaded: mcp__jcode__echo]")));
        assert!(events.iter().any(
            |e| matches!(e, StreamEvent::ToolResult { content, .. } if content == "echo:MCPOK")
        ));
        assert_eq!(text(&events), "echo:MCPOK");
    }

    #[test]
    fn display_names_never_hit_reexecuted_tools() {
        assert_eq!(display_tool_name("mcp__jcode__memory"), "memory");
        assert_eq!(
            display_tool_name("mcp__jcode__selfdev"),
            "mcp__jcode__selfdev"
        );
        assert_eq!(display_tool_name("communicate"), "claude_code_communicate");
        assert_eq!(display_tool_name("Bash"), "Bash");
    }

    #[test]
    fn subagent_partials_are_ignored() {
        let mut t = Translator::new("x", None);
        let out = t.handle(&json!({"type":"stream_event","parent_tool_use_id":"toolu_1",
            "event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"inner"}}}));
        assert!(out.events.is_empty());
        let out = t.handle(&json!({"type":"assistant","parent_tool_use_id":"toolu_1",
            "message":{"id":"m","content":[{"type":"tool_use","id":"t2","name":"Grep","input":{}}]}}));
        assert!(
            matches!(&out.events[..], [StreamEvent::StatusDetail { detail }] if detail == "Subagent: Grep")
        );
        let out = t.handle(&json!({"type":"user","parent_tool_use_id":"toolu_1",
            "message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"x"}]}}));
        assert!(out.events.is_empty());
    }

    #[test]
    fn rate_limit_rejected_yields_retry_after() {
        let mut t = Translator::new("x", None).with_clock(clock);
        let out = t.handle(&json!({"type":"rate_limit_event","rate_limit_info":{
            "status":"rejected","resetsAt": clock() + 7200,"rateLimitType":"five_hour",
            "utilization":1.0,"isUsingOverage":false}}));
        assert!(out.rate_limit_rejected);
        match &out.events[..] {
            [
                StreamEvent::Error {
                    message,
                    retry_after_secs,
                },
            ] => {
                assert_eq!(*retry_after_secs, Some(7200));
                assert!(message.contains("5-hour limit reached"), "{message}");
                assert!(message.contains("2h 0m"));
                assert!(message.contains("[account-usage-limit resets_at="));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn short_rate_limit_has_no_exhaustion_marker() {
        let limits = ClaudeCodeRateLimits {
            status: "rejected".into(),
            resets_at: Some(clock() + 30),
            ..Default::default()
        };
        match rate_limit_error(&limits, clock()) {
            StreamEvent::Error {
                message,
                retry_after_secs,
            } => {
                assert_eq!(retry_after_secs, Some(30));
                assert!(!message.contains("account-usage-limit"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn authentication_failure_is_actionable() {
        let mut t = Translator::new("CLAUDE_CONFIG_DIR=/h claude auth login", None);
        t.handle(&json!({"type":"assistant","error":"authentication_failed",
            "message":{"id":"m1","content":[{"type":"text","text":"Invalid API key · Please run /login"}]}}));
        assert!(t.auth_failed());
        let out = t.handle(&json!({"type":"result","subtype":"success","is_error":true,
            "result":"Invalid API key","session_id":"s"}));
        assert!(out.turn_finished);
        assert!(
            out.events
                .iter()
                .any(|e| matches!(e, StreamEvent::Error { message, .. }
            if message.contains("CLAUDE_CONFIG_DIR=/h claude auth login")))
        );
    }

    #[test]
    fn overloaded_and_error_subtypes() {
        let mut t = Translator::new("x", None);
        let out = t.handle(&json!({"type":"result","subtype":"success","is_error":true,
            "api_error_status":529,"result":"overloaded"}));
        assert!(out.events.iter().any(|e| matches!(e, StreamEvent::Error { message, .. } if message.contains("overloaded (529)"))));

        let mut t = Translator::new("x", None);
        let out = t.handle(
            &json!({"type":"result","subtype":"error_max_turns","is_error":true,
            "errors":["[ede_diagnostic] hidden","Reached max turns"]}),
        );
        let msg = out
            .events
            .iter()
            .find_map(|e| match e {
                StreamEvent::Error { message, .. } => Some(message.clone()),
                _ => None,
            })
            .unwrap();
        assert!(msg.contains("Reached max turns") && !msg.contains("ede_diagnostic"));

        let mut t = Translator::new("x", None);
        let out = t.handle(
            &json!({"type":"result","subtype":"error_during_execution","is_error":true,
            "terminal_reason":"aborted_streaming","errors":["Request was aborted"]}),
        );
        assert!(
            !out.events
                .iter()
                .any(|e| matches!(e, StreamEvent::Error { .. }))
        );
        assert!(out.turn_finished);
    }

    #[test]
    fn result_for_other_turn_is_ignored() {
        let mut t = Translator::new("x", Some("turn-a".into()));
        let out =
            t.handle(&json!({"type":"result","subtype":"success","user_message_uuids":["turn-b"]}));
        assert!(!out.turn_finished && out.events.is_empty());
        let out =
            t.handle(&json!({"type":"result","subtype":"success","user_message_uuids":["turn-a"]}));
        assert!(out.turn_finished);
    }

    #[test]
    fn compaction_and_retry_status() {
        let mut t = Translator::new("x", None);
        let out = t.handle(&json!({"type":"system","subtype":"compact_boundary",
            "compact_metadata":{"trigger":"manual","pre_tokens":12345}}));
        assert!(
            matches!(&out.events[..], [StreamEvent::Compaction { trigger, pre_tokens: Some(12345), .. }] if trigger == "manual")
        );
        let out = t.handle(&json!({"type":"system","subtype":"api_retry","attempt":2,"max_retries":10,"error_status":500}));
        assert!(
            matches!(&out.events[..], [StreamEvent::StatusDetail { detail }] if detail == "Claude API retry 2/10 (500)")
        );
    }

    #[test]
    fn separate_messages_get_paragraph_break() {
        let mut t = Translator::new("x", None);
        let a = t.handle(&json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"one"}]}}));
        let b = t.handle(&json!({"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"two"}]}}));
        let all: Vec<_> = a.events.into_iter().chain(b.events).collect();
        assert_eq!(text(&all), "one\n\ntwo");
    }
}
