use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

#[derive(Debug, Clone)]
struct ProviderInputSnapshot {
    request_hash: u64,
    item_hashes: Vec<u64>,
    item_hashes_hash: u64,
    system_hash: Option<u64>,
    tools_hash: Option<u64>,
    captured_at: Instant,
}

static PROVIDER_INPUT_BASELINES: LazyLock<Mutex<HashMap<String, ProviderInputSnapshot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Baselines are keyed per session and hold one hash per message, but nothing
/// removes them when a session closes. A long-lived server therefore grew
/// this map without bound. Baselines only matter for cache-hit diagnostics on
/// the next request, so drop any that are older than the longest provider
/// prompt-cache TTL, and only scan once the map is large enough to matter.
const BASELINE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);
const BASELINE_PRUNE_THRESHOLD: usize = 64;

fn prune_stale_baselines(baselines: &mut HashMap<String, ProviderInputSnapshot>) {
    if baselines.len() < BASELINE_PRUNE_THRESHOLD {
        return;
    }
    baselines.retain(|_, snapshot| snapshot.captured_at.elapsed() < BASELINE_MAX_AGE);
}

pub fn stable_hash_str(value: &str) -> u64 {
    let digest = Sha256::digest(value.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

pub fn stable_hash_json<T: Serialize + ?Sized>(value: &T) -> u64 {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    stable_hash_str(&encoded)
}

fn stable_json_len<T: Serialize + ?Sized>(value: &T) -> usize {
    serde_json::to_string(value)
        .map(|encoded| encoded.len())
        .unwrap_or_default()
}

fn item_hashes(items: &[Value]) -> Vec<u64> {
    items.iter().map(stable_hash_json).collect()
}

fn prefix_matches(current: &[u64], previous: &[u64]) -> bool {
    if previous.len() > current.len() {
        return false;
    }
    current[..previous.len()] == *previous
}

fn common_prefix_len(current: &[u64], previous: &[u64]) -> usize {
    current
        .iter()
        .zip(previous.iter())
        .take_while(|(current, previous)| current == previous)
        .count()
}

/// Log a privacy-preserving fingerprint of the provider-specific prompt payload.
///
/// `payload` should be the prompt/cache-relevant request shape after provider-specific
/// normalization, not the high-level Jcode message list. Do not include volatile transport
/// IDs unless they are intentionally part of the cache key. `items` should be the ordered
/// provider-visible message/content array so prefix drift can be diagnosed by index.
#[allow(clippy::too_many_arguments)]
pub fn log_provider_canonical_input(
    provider: &str,
    model: &str,
    format: &str,
    payload: &Value,
    items: &[Value],
    system: Option<&Value>,
    tools: Option<&Value>,
    tool_count: Option<usize>,
    extra_fields: &[(&str, String)],
) {
    let request_hash = stable_hash_json(payload);
    let request_json_chars = stable_json_len(payload);
    let item_hashes = item_hashes(items);
    let item_hashes_hash = stable_hash_json(&item_hashes);
    let input_hash = stable_hash_json(items);
    let system_hash = system.map(stable_hash_json);
    let system_json_chars = system.map(stable_json_len);
    let tools_hash = tools.map(stable_hash_json);
    let tools_json_chars = tools.map(stable_json_len);
    let first_item_hash = item_hashes.first().copied();
    let last_item_hash = item_hashes.last().copied();

    let log_context = jcode_logging::current_context_snapshot();
    let session_key = log_context.session.as_deref().unwrap_or("no-session");
    let key = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        session_key, provider, model, format
    );
    let snapshot = ProviderInputSnapshot {
        request_hash,
        item_hashes: item_hashes.clone(),
        item_hashes_hash,
        system_hash,
        tools_hash,
        captured_at: Instant::now(),
    };

    let previous = PROVIDER_INPUT_BASELINES
        .lock()
        .map(|mut baselines| {
            let previous = baselines.insert(key, snapshot);
            prune_stale_baselines(&mut baselines);
            previous
        })
        .ok()
        .flatten();

    let previous_age_secs = previous
        .as_ref()
        .map(|previous| previous.captured_at.elapsed().as_secs());
    let request_changed = previous
        .as_ref()
        .map(|previous| previous.request_hash != request_hash);
    let item_hashes_changed = previous
        .as_ref()
        .map(|previous| previous.item_hashes_hash != item_hashes_hash);
    let prefix_matches = previous
        .as_ref()
        .map(|previous| prefix_matches(&item_hashes, &previous.item_hashes));
    let common_prefix_items = previous
        .as_ref()
        .map(|previous| common_prefix_len(&item_hashes, &previous.item_hashes));
    let first_changed_item_index = common_prefix_items
        .zip(previous.as_ref().map(|previous| previous.item_hashes.len()))
        .and_then(|(common, previous_len)| (common < previous_len).then_some(common));
    let previous_item_count = previous.as_ref().map(|previous| previous.item_hashes.len());
    let system_changed = previous
        .as_ref()
        .map(|previous| previous.system_hash != system_hash);
    let tools_changed = previous
        .as_ref()
        .map(|previous| previous.tools_hash != tools_hash);

    let mut extras = String::new();
    for (key, value) in extra_fields {
        if !key.is_empty() && !value.is_empty() {
            extras.push(' ');
            extras.push_str(key);
            extras.push('=');
            extras.push_str(value);
        }
    }

    jcode_logging::info(&format!(
        "PROVIDER_CANONICAL_INPUT: provider={} model={} format={} request_hash={} request_json_chars={} \
         input_hash={} item_count={} previous_item_count={:?} item_hashes_hash={} first_item_hash={:?} last_item_hash={:?} \
         previous_age_secs={:?} prefix_matches={:?} common_prefix_items={:?} first_changed_item_index={:?} \
         request_changed={:?} item_hashes_changed={:?} system_hash={:?} system_json_chars={:?} system_changed={:?} \
         tools_hash={:?} tools_json_chars={:?} tool_count={:?} tools_changed={:?}{}",
        provider,
        model,
        format,
        request_hash,
        request_json_chars,
        input_hash,
        items.len(),
        previous_item_count,
        item_hashes_hash,
        first_item_hash,
        last_item_hash,
        previous_age_secs,
        prefix_matches,
        common_prefix_items,
        first_changed_item_index,
        request_changed,
        item_hashes_changed,
        system_hash,
        system_json_chars,
        system_changed,
        tools_hash,
        tools_json_chars,
        tool_count,
        tools_changed,
        extras,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefix_matching_allows_append_only_growth() {
        assert!(prefix_matches(&[1, 2, 3], &[1, 2]));
    }

    #[test]
    fn prefix_matching_detects_changed_prefix() {
        assert!(!prefix_matches(&[1, 9, 3], &[1, 2]));
        assert_eq!(common_prefix_len(&[1, 9, 3], &[1, 2]), 1);
    }

    #[test]
    fn stale_baselines_are_pruned_once_map_is_large() {
        let snapshot = |age: std::time::Duration| ProviderInputSnapshot {
            request_hash: 0,
            item_hashes: vec![1, 2, 3],
            item_hashes_hash: 0,
            system_hash: None,
            tools_hash: None,
            captured_at: Instant::now().checked_sub(age).unwrap_or_else(Instant::now),
        };
        let old = BASELINE_MAX_AGE + std::time::Duration::from_secs(1);
        let mut small: HashMap<String, ProviderInputSnapshot> =
            HashMap::from([("old".to_string(), snapshot(old))]);
        prune_stale_baselines(&mut small);
        assert_eq!(small.len(), 1, "small maps are not scanned");

        let mut large: HashMap<String, ProviderInputSnapshot> = (0..BASELINE_PRUNE_THRESHOLD)
            .map(|i| (format!("old{i}"), snapshot(old)))
            .collect();
        large.insert("fresh".into(), snapshot(std::time::Duration::ZERO));
        prune_stale_baselines(&mut large);
        assert_eq!(large.keys().collect::<Vec<_>>(), vec!["fresh"]);
    }

    #[test]
    fn json_hashes_are_content_sensitive() {
        assert_ne!(
            stable_hash_json(&json!({"a": 1})),
            stable_hash_json(&json!({"a": 2}))
        );
    }
}
