//! Privacy-safe stream diagnostics (issue #1759).
//!
//! A stalled turn is hard to attribute without a correlation id the provider
//! can look up, and without knowing whether the stream stalled before any
//! event or mid-response. This module extracts an allowlist of response
//! headers (never credentials, cookies, or account identifiers) and tracks
//! coarse per-stream progress counters for lifecycle logs.

use jcode_message_types::StreamEvent;
use serde_json::Value;
use std::time::Instant;

/// Keep explicit options intact. OAuth compatibility is not established, so
/// never add an option there. A comparison is diagnostic, not conversation state.
pub(super) fn add_cache_comparison(request: &mut Value, chatgpt: bool, baseline: &str) {
    if chatgpt || baseline.is_empty() {
        return;
    }
    let model = request["model"].as_str().unwrap_or("");
    let canonical = jcode_provider_core::model_id::canonical(model);
    if !jcode_base::provider::openai::uses_prompt_cache_options(&canonical) {
        return;
    }
    let Some(object) = request.as_object_mut() else {
        return;
    };
    let options = object
        .entry("prompt_cache_options")
        .or_insert_with(|| serde_json::json!({}));
    if let Some(options) = options.as_object_mut() {
        options
            .entry("comparison_response_id")
            .or_insert_with(|| Value::String(baseline.to_owned()));
    }
}

/// Model output (text, reasoning, tool calls), as opposed to status events.
fn is_output_event(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta(_)
            | StreamEvent::TextSuffixReplace { .. }
            | StreamEvent::ThinkingDelta(_)
            | StreamEvent::ToolUseStart { .. }
            | StreamEvent::ToolInputDelta(_)
            | StreamEvent::ToolInputDeltaFor { .. }
            | StreamEvent::NativeToolCall { .. }
    )
}

/// Response headers that are safe to log and useful for correlating a stalled
/// request with provider-side traces. Strictly an allowlist: anything that can
/// carry credentials, cookies, or account identity is excluded by construction.
const DIAGNOSTIC_HEADERS: &[(&str, &str)] = &[
    ("x-request-id", "request_id"),
    ("x-oai-request-id", "oai_request_id"),
    ("cf-ray", "cf_ray"),
    ("openai-processing-ms", "processing_ms"),
    ("x-envoy-upstream-service-time", "upstream_ms"),
];

/// Longest header value we log. Correlation ids are short; anything longer is
/// unexpected and is truncated rather than echoed.
const MAX_HEADER_VALUE_LEN: usize = 128;

/// Collect allowlisted diagnostic headers as lifecycle-log fields.
pub(super) fn response_header_fields(
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<(&'static str, String)> {
    let mut fields: Vec<_> = DIAGNOSTIC_HEADERS
        .iter()
        .filter_map(|(header, field)| {
            let value = lookup(header)?;
            let value: String = value
                .chars()
                .filter(|ch| ch.is_ascii_graphic())
                .take(MAX_HEADER_VALUE_LEN)
                .collect();
            (!value.is_empty()).then_some((*field, value))
        })
        .collect();
    // This is an opaque routing token, not a correlation ID. Observe only its
    // presence until turn-scoped replay semantics can be implemented safely.
    // Contract: openai/codex da20788df913189878ebca7f4963d8a363ee6bf2,
    // codex-rs/core/src/client.rs, ModelClientSession::turn_state.
    if lookup("x-codex-turn-state").is_some() {
        fields.push(("codex_turn_state_present", "true".to_owned()));
    }
    fields
}

pub(super) fn reqwest_header_fields(
    headers: &reqwest::header::HeaderMap,
) -> Vec<(&'static str, String)> {
    response_header_fields(|name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    })
}

pub(super) fn tungstenite_header_fields(
    response: &tokio_tungstenite::tungstenite::handshake::client::Response,
) -> Vec<(&'static str, String)> {
    response_header_fields(|name| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    })
}

/// Coarse progress counters for one streamed response. Records counts and
/// timings only, never payload content.
pub(super) struct StreamProgress {
    started_at: Instant,
    events: u64,
    output_events: u64,
    first_event_at: Option<Instant>,
    last_event_at: Option<Instant>,
}

impl StreamProgress {
    pub(super) fn new() -> Self {
        Self {
            started_at: Instant::now(),
            events: 0,
            output_events: 0,
            first_event_at: None,
            last_event_at: None,
        }
    }

    pub(super) fn record_event(&mut self, event: &StreamEvent) {
        self.record(is_output_event(event));
    }

    pub(super) fn record(&mut self, output: bool) {
        let now = Instant::now();
        self.events += 1;
        if output {
            self.output_events += 1;
        }
        self.first_event_at.get_or_insert(now);
        self.last_event_at = Some(now);
    }

    /// Lifecycle fields describing where in the response a stall happened:
    /// before any event, or after output began and how long ago.
    pub(super) fn fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = vec![
            ("events", self.events.to_string()),
            ("output_events", self.output_events.to_string()),
        ];
        if let Some(first) = self.first_event_at {
            fields.push((
                "first_event_ms",
                first
                    .duration_since(self.started_at)
                    .as_millis()
                    .to_string(),
            ));
        }
        let since_last = self.last_event_at.unwrap_or(self.started_at).elapsed();
        fields.push(("since_last_event_ms", since_last.as_millis().to_string()));
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_comparison_is_api_modern_model_only_and_preserves_options() {
        for (model, supported) in [
            ("gpt-5.6-sol", true),
            ("gpt-6-astra", true),
            ("gpt-6.1-sol", true),
            ("gpt-5.5", false),
            ("gpt-4.1", false),
            ("other", false),
        ] {
            for chatgpt in [false, true] {
                let mut request = serde_json::json!({"model":model, "prompt_cache_key":"stable"});
                add_cache_comparison(&mut request, chatgpt, "resp_before");
                assert_eq!(
                    request.get("prompt_cache_options").is_some(),
                    supported && !chatgpt,
                    "{model} chatgpt={chatgpt}"
                );
                assert_eq!(request["prompt_cache_key"], "stable");
            }
        }
        let mut request = serde_json::json!({"model":"gpt-6-astra", "prompt_cache_options":{"mode":"implicit", "comparison_response_id":"explicit"}});
        let before = request.clone();
        add_cache_comparison(&mut request, false, "resp_before");
        assert_eq!(request, before);
        request["prompt_cache_options"]
            .as_object_mut()
            .unwrap()
            .remove("comparison_response_id");
        add_cache_comparison(&mut request, false, "resp_before");
        assert_eq!(request["prompt_cache_options"]["mode"], "implicit");
        assert_eq!(
            request["prompt_cache_options"]["comparison_response_id"],
            "resp_before"
        );
        let mut first = serde_json::json!({"model":"gpt-6-astra"});
        add_cache_comparison(&mut first, false, "");
        assert!(first.get("prompt_cache_options").is_none());
    }

    #[test]
    fn header_fields_are_allowlisted_and_sanitized() {
        let fields = response_header_fields(|name| match name {
            "x-request-id" => Some("req_123\r\nInjected: yes".to_string()),
            "cf-ray" => Some("8abc-SJC".to_string()),
            "authorization" | "set-cookie" | "chatgpt-account-id" => {
                panic!("non-allowlisted header {name} must never be read")
            }
            _ => None,
        });
        assert_eq!(
            fields,
            vec![
                ("request_id", "req_123Injected:yes".to_string()),
                ("cf_ray", "8abc-SJC".to_string()),
            ]
        );
    }

    #[test]
    fn header_fields_truncate_oversized_values() {
        let fields =
            response_header_fields(|name| (name == "x-request-id").then(|| "a".repeat(10_000)));
        assert_eq!(fields[0].1.len(), MAX_HEADER_VALUE_LEN);
    }

    #[test]
    fn turn_state_header_value_is_never_logged() {
        let fields = response_header_fields(|name| {
            (name == "x-codex-turn-state").then(|| "opaque secret routing state".to_owned())
        });
        assert_eq!(fields, vec![("codex_turn_state_present", "true".into())]);
    }

    #[test]
    fn progress_distinguishes_no_events_from_mid_stream_stall() {
        let mut progress = StreamProgress::new();
        let before = progress.fields();
        assert!(before.contains(&("events", "0".to_string())));
        assert!(!before.iter().any(|(key, _)| *key == "first_event_ms"));

        progress.record(false);
        progress.record(true);
        let after = progress.fields();
        assert!(after.contains(&("events", "2".to_string())));
        assert!(after.contains(&("output_events", "1".to_string())));
        assert!(after.iter().any(|(key, _)| *key == "first_event_ms"));
        assert!(after.iter().any(|(key, _)| *key == "since_last_event_ms"));
    }
}
