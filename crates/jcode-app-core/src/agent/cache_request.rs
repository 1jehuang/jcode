use super::*;
use std::hash::{Hash, Hasher};

pub(super) fn stable_hash_str(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The one place cache signatures encode JSON. A value that cannot be
/// serialized signs as the empty string, the same as before.
fn encode_json<T: serde::Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[cfg(test)]
fn stable_hash_json<T: serde::Serialize + ?Sized>(value: &T) -> u64 {
    stable_hash_str(&encode_json(value))
}

pub(super) fn stable_json_len<T: serde::Serialize + ?Sized>(value: &T) -> usize {
    encode_json(value).len()
}

fn json_hash_and_len<T: serde::Serialize + ?Sized>(value: &T) -> (u64, usize) {
    let encoded = encode_json(value);
    (stable_hash_str(&encoded), encoded.len())
}

/// Reuse each projected encoding for both the per-message signature and the
/// full JSON array signature. Hash<str> appends a 0xff domain separator, so
/// finish the incrementally written array with the same separator.
fn projected_message_hashes(messages: &[Message]) -> (u64, Vec<u64>) {
    let mut aggregate = std::collections::hash_map::DefaultHasher::new();
    aggregate.write(b"[");
    let mut hashes = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        if index != 0 {
            aggregate.write(b",");
        }
        let projected = crate::message::cache_relevant_message_value(message);
        let encoded = encode_json(&projected);
        hashes.push(stable_hash_str(&encoded));
        aggregate.write(encoded.as_bytes());
    }
    aggregate.write(b"]");
    aggregate.write_u8(0xff);
    (aggregate.finish(), hashes)
}

#[cfg(test)]
fn message_hashes(messages: &[Message]) -> Vec<u64> {
    // Hash the cache-relevant projection, not the raw Message. Raw hashing
    // keys off non-transmitted metadata (timestamp, tool_duration_ms,
    // ReasoningTrace blocks, cache_control markers), which triggers spurious
    // harness:_prefix_changed KV-cache miss reports when the same message is
    // re-serialized with backfilled metadata on the next turn.
    crate::message::cache_relevant_message_hashes(messages)
}

pub(super) fn kv_cache_request_event(
    messages: &[Message],
    tools: &[ToolDefinition],
    system_static: &str,
    ephemeral_messages: &[Message],
) -> ServerEvent {
    // Provider-native deferred definitions are outside the cached prefix.
    // Late MCP discovery must not look like a harness-induced tools change
    // when the provider reports a cache miss for some unrelated reason.
    // Borrow definitions rather than cloning potentially large MCP schemas.
    let eager_tools: Vec<_> = tools.iter().filter(|tool| !tool.defer_loading).collect();
    let (tools_hash, tools_json_chars) = json_hash_and_len(&eager_tools);
    let (messages_hash, message_hashes) = projected_message_hashes(messages);
    let (ephemeral_fingerprint, ephemeral_chars) = json_hash_and_len(ephemeral_messages);
    let ephemeral_hash = if ephemeral_messages.is_empty() {
        None
    } else {
        Some(ephemeral_fingerprint)
    };
    ServerEvent::KvCacheRequest {
        system_static_hash: stable_hash_str(system_static),
        tools_hash,
        messages_hash,
        message_hashes,
        message_count: messages.len(),
        tool_count: eager_tools.len(),
        system_static_chars: system_static.chars().count(),
        tools_json_chars,
        messages_json_chars: stable_json_len(messages),
        ephemeral_hash,
        ephemeral_chars,
        ephemeral_message_count: ephemeral_messages.len(),
    }
}

#[cfg(test)]
#[path = "cache_request/tests.rs"]
mod tests;
