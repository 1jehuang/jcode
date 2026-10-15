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

pub(super) fn persistent_ws_incremental_items(
    input: &[Value],
    start_index: usize,
) -> (Vec<Value>, usize) {
    let mut skipped_reasoning_items = 0usize;
    let incremental_items = input[start_index..]
        .iter()
        .filter_map(|item| {
            if item.get("type").and_then(|value| value.as_str()) == Some("reasoning") {
                skipped_reasoning_items += 1;
                None
            } else {
                Some(item.clone())
            }
        })
        .collect();
    (incremental_items, skipped_reasoning_items)
}
