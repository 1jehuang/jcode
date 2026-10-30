//! Ephemeral per-session overrides for tool subprocesses.
//!
//! These values may be credentials. Never log or persist this registry. Each
//! launch takes an owned snapshot, so updates cannot change accepted jobs.
//! Overrides are lost on daemon restart and must be reapplied before resuming.

use anyhow::{Result, bail};
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

static SESSION_ENV: LazyLock<RwLock<HashMap<String, HashMap<String, String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Replace all overrides atomically. An empty map clears the entry.
/// Invalid updates leave the previous values untouched. Errors never include
/// keys or values, since either could contain confidential caller input.
pub fn set_session_env(session_id: &str, env: HashMap<String, String>) -> Result<()> {
    for (key, value) in &env {
        if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
            bail!(
                "Invalid session environment: keys must be nonempty and contain neither '=' nor NUL, and values must not contain NUL"
            );
        }
        if key.to_ascii_uppercase().starts_with("JCODE_") {
            bail!("Session environment cannot override reserved JCODE_* variables");
        }
    }
    let mut entries = SESSION_ENV.write().unwrap_or_else(|e| e.into_inner());
    if env.is_empty() {
        entries.remove(session_id);
    } else {
        entries.insert(session_id.to_owned(), env);
    }
    Ok(())
}

/// Return an owned launch-time snapshot. Do not serialize it into diagnostics.
pub fn session_env(session_id: &str) -> HashMap<String, String> {
    SESSION_ENV
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .cloned()
        .unwrap_or_default()
}

/// Give a child an independent copy of its parent's current overrides.
pub fn inherit_session_env(parent: &str, child: &str) {
    let mut entries = SESSION_ENV.write().unwrap_or_else(|e| e.into_inner());
    if let Some(env) = entries.get(parent).cloned() {
        entries.insert(child.to_owned(), env);
    } else {
        entries.remove(child);
    }
}

/// Forget a destroyed session. Already-launched jobs retain their snapshots.
pub fn remove_session_env(session_id: &str) {
    SESSION_ENV
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(value: &str) -> HashMap<String, String> {
        HashMap::from([("HTTPS_PROXY".into(), value.into())])
    }

    #[test]
    fn invalid_updates_are_atomic_and_do_not_disclose_input() {
        let sid = "env-invalid-update";
        set_session_env(sid, vars("original")).unwrap();
        for (key, value) in [
            ("", "secret"),
            ("secret=key", "secret"),
            ("secret\0key", "secret"),
            ("KEY", "secret\0value"),
            ("JCODE_SESSION_ID", "secret"),
            ("jcode_internal", "secret"),
        ] {
            let err =
                set_session_env(sid, HashMap::from([(key.into(), value.into())])).unwrap_err();
            assert!(!err.to_string().contains("secret"));
            assert_eq!(session_env(sid), vars("original"));
        }
        remove_session_env(sid);
    }

    #[test]
    fn snapshots_and_children_are_independent_and_cleanup_removes_entries() {
        let parent = "env-parent";
        let child = "env-child";
        let other = "env-other";
        set_session_env(parent, vars("parent")).unwrap();
        set_session_env(other, vars("other")).unwrap();
        let snapshot = session_env(parent);
        inherit_session_env(parent, child);
        set_session_env(parent, vars("replacement")).unwrap();
        assert_eq!(snapshot, vars("parent"));
        assert_eq!(session_env(child), vars("parent"));
        assert_eq!(session_env(other), vars("other"));
        set_session_env(parent, HashMap::new()).unwrap();
        assert!(!SESSION_ENV.read().unwrap().contains_key(parent));
        remove_session_env(child);
        assert!(!SESSION_ENV.read().unwrap().contains_key(child));
        assert_eq!(snapshot, vars("parent"));
        remove_session_env(other);
    }
}
