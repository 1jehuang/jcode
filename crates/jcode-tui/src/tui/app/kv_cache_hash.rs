//! Stable hashing helpers for KV-cache request signature comparison.

use crate::message::{Message, cache_relevant_message_value};
use std::hash::{Hash, Hasher};

pub(super) fn stable_hash_str(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn stable_hash_json<T: serde::Serialize + ?Sized>(value: &T) -> u64 {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    stable_hash_str(&encoded)
}

pub(super) fn stable_json_len<T: serde::Serialize + ?Sized>(value: &T) -> usize {
    serde_json::to_string(value)
        .map(|encoded| encoded.len())
        .unwrap_or_default()
}

// The cache-relevant projection lives in `jcode-message-types` (re-exported
// through `crate::message`) so this local path and the server event path in
// `jcode-app-core::agent::kv_cache_request_event` hash messages identically.
// If the two projections drift, remote sessions report false
// `harness:_prefix_changed` KV-cache misses.
pub(super) fn message_hashes(messages: &[Message]) -> Vec<u64> {
    messages
        .iter()
        .map(|message| stable_hash_json(&cache_relevant_message_value(message)))
        .collect()
}

pub(super) fn ratio_pct(numerator: u64, denominator: u64) -> u8 {
    if denominator == 0 {
        0
    } else {
        ((numerator as f32 / denominator as f32) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }
}
