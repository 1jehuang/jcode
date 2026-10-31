//! Lock-independent projection of the events consumed by an opened worker.
//! This does not read Agent while its turn mutex is held, or invent transport
//! information from a provider name.
use crate::protocol::{ServerEvent, SwarmLiveStatus};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

static LIVE: LazyLock<Mutex<HashMap<String, Projection>>> = LazyLock::new(Mutex::default);
static CHANGED: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);

pub(super) fn subscribe_changes() -> tokio::sync::watch::Receiver<u64> {
    CHANGED.subscribe()
}

struct Projection {
    value: SwarmLiveStatus,
    started: Instant,
    ended: Option<Instant>,
    streaming_started: Option<Instant>,
    streaming_elapsed: Duration,
    total_output: u64,
    call_output: u64,
    new_call: bool,
    tool_inputs: HashMap<String, String>,
    last_tool_id: Option<String>,
    interruption_reason: Option<String>,
}

impl Projection {
    fn new(now: Instant) -> Self {
        Self {
            value: SwarmLiveStatus {
                phase: "sending".into(),
                ..Default::default()
            },
            started: now,
            ended: None,
            streaming_started: None,
            streaming_elapsed: Duration::ZERO,
            total_output: 0,
            call_output: 0,
            new_call: true,
            tool_inputs: HashMap::new(),
            last_tool_id: None,
            interruption_reason: None,
        }
    }

    fn pause(&mut self, now: Instant) {
        if let Some(start) = self.streaming_started.take() {
            self.streaming_elapsed += now.saturating_duration_since(start);
        }
    }

    fn observe(&mut self, event: &ServerEvent, now: Instant) {
        match event {
            ServerEvent::ConnectionType { connection } => {
                self.value.connection_type = Some(connection.clone())
            }
            ServerEvent::StatusDetail { detail } => self.value.status_detail = Some(detail.clone()),
            ServerEvent::ConnectionPhase { phase } => {
                if phase == "streaming" {
                    self.streaming_started.get_or_insert(now);
                    self.value.phase = "streaming".into();
                } else {
                    self.pause(now);
                    self.value.phase = "connecting".into();
                }
                self.value.connection_phase = Some(phase.clone());
                self.value.stream_message_ended = false;
            }
            ServerEvent::ToolStart { id, .. } => {
                self.tool_inputs.insert(id.clone(), String::new());
                self.last_tool_id = Some(id.clone());
                self.value.phase = "streaming".into();
                self.streaming_started.get_or_insert(now);
            }
            ServerEvent::ToolInput { id, delta } => {
                if let Some(id) = id.as_ref().or(self.last_tool_id.as_ref()) {
                    let input = self.tool_inputs.entry(id.clone()).or_default();
                    // Only retain a small bounded input for the display intent.
                    if input.len().saturating_add(delta.len()) <= 65536 {
                        input.push_str(delta);
                    }
                }
            }
            ServerEvent::TextDelta { .. } | ServerEvent::ReasoningDelta { .. } => {
                self.value.phase = if matches!(event, ServerEvent::ReasoningDelta { .. }) {
                    "thinking"
                } else {
                    "streaming"
                }
                .into();
                self.value.stream_message_ended = false;
                self.streaming_started.get_or_insert(now);
            }
            ServerEvent::TokenUsage { input, output, .. } => {
                if self.new_call {
                    self.call_output = 0;
                    self.new_call = false;
                }
                self.total_output = self
                    .total_output
                    .saturating_add(output.saturating_sub(self.call_output));
                self.call_output = self.call_output.max(*output);
                self.value.input_tokens = Some(*input);
                self.value.output_tokens = Some(*output);
                let elapsed = self.streaming_elapsed
                    + self
                        .streaming_started
                        .map(|s| now.saturating_duration_since(s))
                        .unwrap_or(Duration::ZERO);
                self.value.output_tps_milli = (elapsed.as_secs_f64() > 0.1
                    && self.total_output > 0)
                    .then(|| (self.total_output as f64 * 1000.0 / elapsed.as_secs_f64()) as u64);
            }
            ServerEvent::ToolCall { name, input, .. } => {
                self.value.tool_name = Some(name.clone());
                self.value.tool_intent = input
                    .get("intent")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
            }
            ServerEvent::ToolExec { id, name } => {
                self.pause(now);
                self.value.phase = "running_tool".into();
                self.value.tool_name = Some(name.clone());
                self.value.tool_intent = self
                    .tool_inputs
                    .remove(id)
                    .and_then(|s| match serde_json::from_str::<serde_json::Value>(&s) {
                        Ok(value) => Some(value),
                        Err(error) => {
                            crate::logging::debug(&format!(
                                "Live tool intent unavailable for {id}: {error}"
                            ));
                            None
                        }
                    })
                    .and_then(|v| v.get("intent").and_then(|s| s.as_str()).map(str::to_owned));
            }
            ServerEvent::ToolDone { .. } => {
                self.value.tool_name = None;
                self.value.tool_intent = None;
                self.value.phase = "sending".into();
            }
            ServerEvent::MessageEnd { .. } => {
                self.pause(now);
                self.value.stream_message_ended = true;
                // Provider stop tokens describe one response, not why the
                // worker is idle/stopped. Only lifecycle/interrupt reasons
                // belong in the user-facing stop explanation.
            }
            ServerEvent::KvCacheRequest { .. } => self.new_call = true,
            ServerEvent::RetryRollback { .. } => {
                self.pause(now);
                self.value.phase = "connecting".into();
                self.value.input_tokens = None;
                self.value.output_tokens = None;
                self.new_call = true;
            }
            ServerEvent::TurnStopped { message, .. } => {
                self.interruption_reason = Some(message.clone());
                self.value.stop_reason = Some(message.clone());
            }
            ServerEvent::Interrupted => {
                self.interruption_reason = Some("Interrupted by user".into());
                self.value.stop_reason = self.interruption_reason.clone();
            }
            _ => {}
        }
    }

    fn snapshot(&self, now: Instant) -> SwarmLiveStatus {
        let mut value = self.value.clone();
        value.elapsed_ms = self
            .ended
            .unwrap_or(now)
            .saturating_duration_since(self.started)
            .as_millis()
            .min(u64::MAX as u128) as u64;
        value
    }
}

pub(super) fn begin(session_id: &str) {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.into(), Projection::new(Instant::now()));
}

pub(super) fn observe(session_id: &str, event: &ServerEvent) {
    if let Some(p) = LIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(session_id)
    {
        let previous = p.value.clone();
        let final_phase = p.ended.map(|ended| (ended, p.value.phase.clone()));
        p.observe(event, Instant::now());
        // The forwarding task can drain trailing usage after the lifecycle
        // completed. Keep those counts without reviving a stopped spinner.
        if let Some((ended, phase)) = final_phase {
            p.pause(ended);
            p.value.phase = phase;
        }
        if previous != p.value {
            CHANGED.send_modify(|generation| *generation = generation.wrapping_add(1));
        }
    }
}

pub(super) fn finish(session_id: &str, status: &str, reason: Option<&str>) {
    if let Some(p) = LIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(session_id)
    {
        let now = Instant::now();
        p.pause(now);
        p.ended.get_or_insert(now);
        p.value.phase = status.into();
        p.value.tool_name = None;
        p.value.tool_intent = None;
        if let Some(reason) = reason.filter(|reason| !reason.trim().is_empty()) {
            p.value.stop_reason = Some(reason.into());
        } else {
            p.value.stop_reason = Some(p.interruption_reason.clone().unwrap_or_else(|| {
                match status {
                    "ready" | "idle" => "Waiting for work",
                    "done" | "completed" => "Completed",
                    "stopped" => "Stopped",
                    "failed" | "crashed" => "Failed",
                    "cancelled" => "Cancelled",
                    "blocked" | "waiting" => "Waiting",
                    _ => status,
                }
                .into()
            }));
        }
    }
}

pub(super) fn snapshot(session_id: &str) -> Option<SwarmLiveStatus> {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .map(|p| p.snapshot(Instant::now()))
}

pub(super) fn forget(session_id: &str) {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ServerEvent {
        serde_json::from_value(serde_json::json!({
            "type":"kv_cache_request", "system_static_hash":0, "tools_hash":0,
            "messages_hash":0, "message_count":0, "tool_count":0
        }))
        .unwrap()
    }

    #[test]
    fn live_status_lifecycle_reasons_do_not_leak_provider_stop_tokens() {
        let id = "lifecycle-reason-regression";
        for token in ["tool_use", "end_turn"] {
            begin(id);
            observe(
                id,
                &ServerEvent::MessageEnd {
                    stop_reason: Some(token.into()),
                },
            );
            finish(id, "ready", None);
            assert_eq!(
                snapshot(id).unwrap().stop_reason.as_deref(),
                Some("Waiting for work")
            );
            finish(id, "stopped", None);
            assert_eq!(
                snapshot(id).unwrap().stop_reason.as_deref(),
                Some("Stopped")
            );
        }
        begin(id);
        observe(id, &ServerEvent::Interrupted);
        finish(id, "stopped", None);
        assert_eq!(
            snapshot(id).unwrap().stop_reason.as_deref(),
            Some("Interrupted by user")
        );
        finish(id, "stopped", Some("Owner cancelled the deployment"));
        assert_eq!(
            snapshot(id).unwrap().stop_reason.as_deref(),
            Some("Owner cancelled the deployment")
        );
        forget(id);
    }

    #[test]
    fn live_status_usage_resets_only_on_request_or_retry() {
        let now = Instant::now();
        let mut p = Projection::new(now);
        let usage = |output| ServerEvent::TokenUsage {
            input: 100,
            output,
            cache_read_input: None,
            cache_creation_input: None,
            effective_budget: None,
        };
        p.observe(&request(), now);
        p.observe(&usage(20), now);
        p.observe(&usage(10), now); // Stale report must not lower the watermark.
        p.observe(
            &ServerEvent::ConnectionPhase {
                phase: "connecting".into(),
            },
            now,
        );
        p.observe(
            &ServerEvent::ToolDone {
                id: "native".into(),
                name: "search".into(),
                output: String::new(),
                error: None,
            
                duration_ms: None,
            },
            now,
        );
        p.observe(&usage(20), now);
        assert_eq!(p.total_output, 20);
        p.observe(&request(), now);
        p.observe(&usage(5), now);
        assert_eq!(p.total_output, 25);
        p.observe(&ServerEvent::RetryRollback { attempt: 1, max: 2 }, now);
        p.observe(&usage(5), now);
        assert_eq!(p.total_output, 30);
    }

    #[test]
    fn live_status_projects_counts_and_excludes_tool_time_from_tps() {
        let start = Instant::now();
        let mut p = Projection::new(start);
        p.observe(
            &ServerEvent::ConnectionType {
                connection: "websocket".into(),
            },
            start,
        );
        p.observe(
            &ServerEvent::ConnectionPhase {
                phase: "streaming".into(),
            },
            start,
        );
        p.observe(
            &ServerEvent::TokenUsage {
                input: 1200,
                output: 20,
                cache_read_input: Some(900),
                cache_creation_input: None,
                effective_budget: None,
            },
            start + Duration::from_secs(2),
        );
        assert_eq!(p.value.output_tps_milli, Some(10000));
        assert_eq!(p.value.input_tokens, Some(1200)); // Not input + cache + output.
        p.observe(
            &ServerEvent::TokenUsage {
                input: 1200,
                output: 20,
                cache_read_input: None,
                cache_creation_input: None,
                effective_budget: None,
            },
            start + Duration::from_secs(2),
        );
        assert_eq!(p.total_output, 20); // Duplicate final usage isn't counted twice.
        p.observe(
            &ServerEvent::MessageEnd {
                stop_reason: Some("tool_use".into()),
            },
            start + Duration::from_secs(2),
        );
        p.observe(
            &ServerEvent::TokenUsage {
                input: 1200,
                output: 20,
                cache_read_input: None,
                cache_creation_input: None,
                effective_budget: None,
            },
            start + Duration::from_secs(2),
        );
        assert_eq!(p.total_output, 20); // MessageEnd precedes the final usage event.
        p.observe(
            &ServerEvent::ToolStart {
                id: "t".into(),
                name: "bash".into(),
            },
            start + Duration::from_secs(2),
        );
        p.observe(
            &ServerEvent::ToolInput {
                id: Some("t".into()),
                delta: r#"{"intent":"Run tests"}"#.into(),
            },
            start + Duration::from_secs(2),
        );
        p.observe(
            &ServerEvent::ToolExec {
                id: "t".into(),
                name: "bash".into(),
            },
            start + Duration::from_secs(2),
        );
        assert_eq!(p.value.tool_intent.as_deref(), Some("Run tests"));
        p.observe(
            &ServerEvent::ToolDone {
                id: "t".into(),
                name: "bash".into(),
                output: "ok".into(),
                error: None,
            
                duration_ms: None,
            },
            start + Duration::from_secs(102),
        );
        p.observe(&request(), start + Duration::from_secs(102));
        p.observe(
            &ServerEvent::ConnectionPhase {
                phase: "streaming".into(),
            },
            start + Duration::from_secs(102),
        );
        p.observe(
            &ServerEvent::TokenUsage {
                input: 1300,
                output: 10,
                cache_read_input: None,
                cache_creation_input: None,
                effective_budget: None,
            },
            start + Duration::from_secs(103),
        );
        assert_eq!(p.value.output_tps_milli, Some(10000));
        assert_eq!(
            p.snapshot(start + Duration::from_secs(103)).elapsed_ms,
            103000
        );
        assert_eq!(p.value.connection_type.as_deref(), Some("websocket"));
    }

    #[test]
    fn live_status_new_turn_has_no_invented_or_stale_telemetry() {
        let p = Projection::new(Instant::now());
        assert_eq!(p.value.input_tokens, None);
        assert_eq!(p.value.output_tokens, None);
        assert_eq!(p.value.connection_type, None);
        assert_eq!(p.value.output_tps_milli, None);
        assert_eq!(p.value.tool_intent, None);
    }
}
