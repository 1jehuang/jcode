//! Per-session environment overrides for tool-spawned processes.
//!
//! A daemon serves many sessions on one machine. An embedder (for example a
//! platform that routes each conversation through its own egress credential)
//! sets env vars per session; every shell the session's tools start, including
//! background jobs that outlive the turn, gets them. Keyed by session id, so a
//! job keeps its session's values for its whole life.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

static SESSION_ENVS: LazyLock<RwLock<HashMap<String, Vec<(String, String)>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Replace a session's env overrides. An empty list clears them.
pub fn set_session_env(session_id: &str, vars: Vec<(String, String)>) {
    let Ok(mut map) = SESSION_ENVS.write() else {
        return;
    };
    if vars.is_empty() {
        map.remove(session_id);
    } else {
        map.insert(session_id.to_string(), vars);
    }
}

/// A session's env overrides, in the order they were set.
pub fn session_env(session_id: &str) -> Vec<(String, String)> {
    SESSION_ENVS
        .read()
        .ok()
        .and_then(|map| map.get(session_id).cloned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_keep_their_own_env() {
        set_session_env("a", vec![("HTTPS_PROXY".into(), "http://a".into())]);
        set_session_env("b", vec![("HTTPS_PROXY".into(), "http://b".into())]);
        assert_eq!(session_env("a")[0].1, "http://a");
        assert_eq!(session_env("b")[0].1, "http://b");
        set_session_env("a", Vec::new());
        assert!(session_env("a").is_empty());
    }
}
