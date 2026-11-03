use super::*;
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

pub(super) fn message_hashes(messages: &[Message]) -> Vec<u64> {
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
    let ephemeral_hash = if ephemeral_messages.is_empty() {
        None
    } else {
        Some(stable_hash_json(ephemeral_messages))
    };
    ServerEvent::KvCacheRequest {
        system_static_hash: stable_hash_str(system_static),
        tools_hash: stable_hash_json(&eager_tools),
        messages_hash: stable_hash_json(&crate::message::cache_relevant_messages(messages)),
        message_hashes: message_hashes(messages),
        message_count: messages.len(),
        tool_count: eager_tools.len(),
        system_static_chars: system_static.chars().count(),
        tools_json_chars: stable_json_len(&eager_tools),
        messages_json_chars: stable_json_len(messages),
        ephemeral_hash,
        ephemeral_chars: stable_json_len(ephemeral_messages),
        ephemeral_message_count: ephemeral_messages.len(),
    }
}
