//! Persistent WebSocket input accounting and incremental replay.

use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct WsInputStats {
    pub(super) total_items: usize,
    pub(super) message_items: usize,
    pub(super) function_call_items: usize,
    pub(super) function_call_output_items: usize,
    pub(super) other_items: usize,
}

impl WsInputStats {
    pub(super) fn tool_callback_count(self) -> usize {
        self.function_call_output_items
    }

    pub(super) fn log_fields(self) -> String {
        format!(
            "items={} messages={} function_calls={} tool_outputs={} other={}",
            self.total_items,
            self.message_items,
            self.function_call_items,
            self.function_call_output_items,
            self.other_items
        )
    }
}

pub(super) fn summarize_ws_input(items: &[Value]) -> WsInputStats {
    let mut stats = WsInputStats::default();
    for item in items {
        stats.total_items += 1;
        match item.get("type").and_then(|value| value.as_str()) {
            Some("message") => stats.message_items += 1,
            Some("function_call") => stats.function_call_items += 1,
            Some("function_call_output") => stats.function_call_output_items += 1,
            _ => stats.other_items += 1,
        }
    }
    stats
}

pub(super) fn persistent_ws_input_item_hashes(input: &[Value]) -> Vec<u64> {
    input
        .iter()
        .map(jcode_provider_core::fingerprint::stable_hash_json)
        .collect()
}

// Compare the fields retained by build_responses_input, not transport metadata
// such as item id, status, phase, annotations, or argument JSON whitespace.
fn replay_identity(item: &Value) -> Value {
    match item.get("type").and_then(Value::as_str) {
        Some("message")
            if item.get("role").and_then(Value::as_str) == Some("assistant")
                && item
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        !parts.is_empty()
                            && parts.iter().all(|part| {
                                part.get("type").and_then(Value::as_str) == Some("output_text")
                                    && part.get("text").and_then(Value::as_str).is_some()
                            })
                    }) =>
        {
            let text: String = item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect();
            serde_json::json!({"type":"message", "role":"assistant", "text":text})
        }
        Some("function_call") => {
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| item["arguments"].clone());
            serde_json::json!({"type":"function_call", "call_id":item["call_id"], "name":item["name"], "arguments":arguments})
        }
        _ => item.clone(),
    }
}

/// Keep completed output even when response.completed omits its output array.
/// When it is supplied, it is authoritative and replaces output_item.done data.
pub(super) fn observe_ws_response_output(text: &str, output: &mut Vec<Value>) {
    // Token deltas dominate the stream. Only completed output events need a
    // second parse for response-chain bookkeeping.
    if !text.contains("response.output_item.done") && !text.contains("response.completed") {
        return;
    }
    let Ok(event) = serde_json::from_str::<Value>(text) else {
        return;
    };
    match event.get("type").and_then(Value::as_str) {
        Some("response.output_item.done") => {
            if let Some(item) = event.get("item") {
                output.push(item.clone());
            }
        }
        Some("response.completed") => {
            if let Some(items) = event.pointer("/response/output").and_then(Value::as_array) {
                *output = items.clone();
            }
        }
        _ => {}
    }
}

pub(super) fn persistent_ws_incremental_items(
    input: &[Value],
    start_index: usize,
    previous_output: &[Value],
) -> (Vec<Value>, usize) {
    let mut skipped_items = 0usize;
    // Consume matches once, so an additional identical client-injected message
    // is not lost. Never remove unrelated assistant messages or tool calls.
    let mut remaining: Vec<Value> = previous_output.iter().map(replay_identity).collect();
    let incremental_items = input[start_index..]
        .iter()
        .filter_map(|item| {
            let identity = replay_identity(item);
            if let Some(index) = remaining.iter().position(|output| *output == identity) {
                remaining.remove(index);
                skipped_items += 1;
                None
            } else if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                // Reasoning store items are already owned by previous_response_id.
                skipped_items += 1;
                None
            } else {
                Some(item.clone())
            }
        })
        .collect();
    (incremental_items, skipped_items)
}
