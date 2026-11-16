/// Fingerprint of the last fully-applied History payload for one client
/// instance, so byte-identical bootstrap redeliveries can be dropped without
/// rebuilding the display transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AppliedHistoryFingerprint {
    pub(super) session_id: String,
    pub(super) fingerprint: u64,
}

/// Last fully-applied History payload fingerprint, keyed by
/// `App::remote_client_instance_id`.
///
/// This is deliberately NOT stored on `RemoteConnection`: a reconnect builds a
/// fresh connection (resetting `has_loaded_history`), and that reconnect
/// re-bootstrap is exactly the duplicate full-payload delivery this state must
/// survive to dedup. Keeping it module-local also keeps the dedup concern
/// entirely inside the History handler. Entries are tiny (session id + u64);
/// the map is bounded because many short-lived `App`s only exist in tests.
static LAST_APPLIED_HISTORY: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, AppliedHistoryFingerprint>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

pub(super) fn last_applied_history_fingerprint(
    instance_id: &str,
) -> Option<AppliedHistoryFingerprint> {
    LAST_APPLIED_HISTORY
        .lock()
        .ok()
        .and_then(|map| map.get(instance_id).cloned())
}

pub(super) fn record_applied_history_fingerprint(
    instance_id: &str,
    session_id: &str,
    fingerprint: u64,
) {
    if let Ok(mut map) = LAST_APPLIED_HISTORY.lock() {
        // Bound growth from short-lived test/replay Apps; one entry per live
        // client is the steady state, so clearing is harmless (worst case one
        // extra full re-apply per client).
        if !map.contains_key(instance_id) && map.len() >= 64 {
            map.clear();
        }
        map.insert(
            instance_id.to_string(),
            AppliedHistoryFingerprint {
                session_id: session_id.to_string(),
                fingerprint,
            },
        );
    }
}

/// Hash a JSON value structurally without serializing it to a string, so large
/// tool inputs contribute to the fingerprint in one allocation-free pass.
pub(super) fn hash_json_value(value: &serde_json::Value, hasher: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    match value {
        serde_json::Value::Null => 0u8.hash(hasher),
        serde_json::Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        serde_json::Value::Number(n) => {
            2u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        serde_json::Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        serde_json::Value::Array(items) => {
            4u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_json_value(item, hasher);
            }
        }
        serde_json::Value::Object(map) => {
            5u8.hash(hasher);
            map.len().hash(hasher);
            for (key, item) in map {
                key.hash(hasher);
                hash_json_value(item, hasher);
            }
        }
    }
}

/// Cheap structural fingerprint of a full History payload.
///
/// Reconnects, session-switch storms, and the history-recovery watchdog can
/// redeliver the same multi-megabyte bootstrap payload within seconds.
/// Re-applying it rebuilds the whole display transcript (~3-4x the wire size
/// in transient arenas) for zero visible change. One pass over message bytes,
/// no allocations proportional to payload size.
pub(super) fn history_payload_fingerprint(messages: &[crate::protocol::HistoryMessage]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    messages.len().hash(&mut hasher);
    for message in messages {
        message.role.hash(&mut hasher);
        message.content.hash(&mut hasher);
        match &message.tool_calls {
            Some(calls) => {
                calls.len().hash(&mut hasher);
                for call in calls {
                    call.hash(&mut hasher);
                }
            }
            None => usize::MAX.hash(&mut hasher),
        }
        match &message.tool_data {
            Some(tool) => {
                1u8.hash(&mut hasher);
                tool.id.hash(&mut hasher);
                tool.name.hash(&mut hasher);
                tool.intent.hash(&mut hasher);
                tool.thought_signature.hash(&mut hasher);
                hash_json_value(&tool.input, &mut hasher);
            }
            None => 0u8.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Pure skip decision for a full History payload: skip only when the session
/// did not change, the display still has content to preserve, and the payload
/// fingerprints identical to the one most recently applied for this session.
/// Session switches and rewinds always re-apply (a rewind's truncated payload
/// fingerprints differently, and a session switch flips `session_changed`).
pub(super) fn should_skip_identical_history_payload(
    session_changed: bool,
    display_is_empty: bool,
    last_applied: Option<&AppliedHistoryFingerprint>,
    session_id: &str,
    fingerprint: u64,
) -> bool {
    !session_changed
        && !display_is_empty
        && last_applied
            .is_some_and(|entry| entry.session_id == session_id && entry.fingerprint == fingerprint)
}

/// True when the incoming rendered-image set is (cheaply) identical to the
/// already-retained set: same count and, per image, same data length plus
/// equal cheap metadata. Image data is compared by length only so duplicate
/// multi-megabyte base64 payloads are never traversed byte-by-byte.
pub(super) fn history_images_match_retained(
    incoming: &[crate::session::RenderedImage],
    retained: &[crate::session::RenderedImage],
) -> bool {
    incoming.len() == retained.len()
        && incoming.iter().zip(retained.iter()).all(|(a, b)| {
            a.data.len() == b.data.len()
                && a.media_type == b.media_type
                && a.label == b.label
                && a.source == b.source
                && a.anchor == b.anchor
        })
}
