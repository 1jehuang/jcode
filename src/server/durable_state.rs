use serde::Serialize;
use serde::de::DeserializeOwned;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn hashed_request_key(session_id: &str, action: &str, components: &[String]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut hasher);
    action.hash(&mut hasher);
    for component in components {
        component.hash(&mut hasher);
    }
    format!(
        "{}-{:016x}",
        sanitize_session_id(session_id),
        hasher.finish()
    )
}

pub(super) fn state_dir(dir_name: &str) -> PathBuf {
    crate::storage::runtime_dir().join(dir_name)
}

pub(super) fn state_path(dir_name: &str, key: &str) -> PathBuf {
    state_dir(dir_name).join(format!("{key}.json"))
}

pub(super) fn load_json_state<T, F>(dir_name: &str, key: &str, is_stale: F) -> Option<T>
where
    T: DeserializeOwned,
    F: Fn(&T) -> bool,
{
    let path = state_path(dir_name, key);
    let state = crate::storage::read_json::<T>(&path).ok()?;
    if is_stale(&state) {
        let _ = std::fs::remove_file(path);
        return None;
    }
    Some(state)
}

pub(super) fn save_json_state<T>(dir_name: &str, key: &str, state: &T, label: &str)
where
    T: Serialize,
{
    let path = state_path(dir_name, key);
    if let Err(err) = crate::storage::write_json_fast(&path, state) {
        crate::logging::warn(&format!("Failed to persist {label} {key}: {err}"));
    }
}

pub(super) fn elapsed_exceeds(created_at_unix_ms: u64, ttl: Duration) -> bool {
    now_unix_ms().saturating_sub(created_at_unix_ms) > ttl.as_millis() as u64
}
