//! Process-global, deadlock-free registry of each session's current reasoning
//! effort.
//!
//! Server handlers often run while the target agent is blocked holding its own
//! agent lock, so they cannot read its effort via the agent mutex without
//! deadlocking. This tiny side-table is updated whenever an agent's effort
//! changes (cheap string writes) so handlers can learn a session's effort by id
//! without taking any agent lock.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

static SESSION_EFFORTS: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Record (or clear) a session's current reasoning effort. Pass `None` to forget
/// it (e.g. when the effort is unset).
pub fn record_session_effort(session_id: &str, effort: Option<&str>) {
    let Ok(mut map) = SESSION_EFFORTS.write() else {
        return;
    };
    match effort {
        Some(effort) => {
            map.insert(session_id.to_string(), effort.to_string());
        }
        None => {
            map.remove(session_id);
        }
    }
}

/// Look up a session's last-recorded reasoning effort, if any.
pub fn session_effort(session_id: &str) -> Option<String> {
    SESSION_EFFORTS.read().ok()?.get(session_id).cloned()
}

/// Drop a session's entry entirely (called on session teardown to bound growth).
pub fn forget_session_effort(session_id: &str) {
    if let Ok(mut map) = SESSION_EFFORTS.write() {
        map.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_back_effort() {
        let sid = "session-effort-roundtrip";
        forget_session_effort(sid);
        assert_eq!(session_effort(sid), None);

        record_session_effort(sid, Some("swarm"));
        assert_eq!(session_effort(sid).as_deref(), Some("swarm"));

        // Overwrite with a new value.
        record_session_effort(sid, Some("high"));
        assert_eq!(session_effort(sid).as_deref(), Some("high"));

        // Clearing forgets it.
        record_session_effort(sid, None);
        assert_eq!(session_effort(sid), None);
    }
}
