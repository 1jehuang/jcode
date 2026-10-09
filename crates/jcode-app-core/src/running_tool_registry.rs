//! Process-global record of the tool call each session is currently waiting on.
//!
//! `background_tool` must tell the caller whether there was anything to move
//! (issue #1778). The agent only listens for the background signal while it is
//! awaiting a tool, so that window is exactly what this registry records. It is
//! keyed by session id and kept outside the Agent mutex, which the running turn
//! holds for its whole duration.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningTool {
    pub call_id: String,
    pub name: String,
}

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static RUNNING: LazyLock<Mutex<HashMap<String, (u64, RunningTool)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// RAII registration for one backgroundable tool wait. Dropping it clears the
/// entry unless a newer registration for the same session replaced it.
pub struct RunningToolGuard {
    session_id: String,
    token: u64,
}

pub fn register(session_id: &str, call_id: &str, name: &str) -> RunningToolGuard {
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut map) = RUNNING.lock() {
        map.insert(
            session_id.to_string(),
            (
                token,
                RunningTool {
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                },
            ),
        );
    }
    RunningToolGuard {
        session_id: session_id.to_string(),
        token,
    }
}

/// The tool call `session_id` is currently awaiting, if any.
pub fn current(session_id: &str) -> Option<RunningTool> {
    RUNNING
        .lock()
        .ok()
        .and_then(|map| map.get(session_id).map(|(_, tool)| tool.clone()))
}

impl Drop for RunningToolGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = RUNNING.lock()
            && map
                .get(&self.session_id)
                .is_some_and(|(token, _)| *token == self.token)
        {
            map.remove(&self.session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_scopes_the_entry_and_newer_registration_wins() {
        let session = "ses_running_tool_registry_test";
        assert_eq!(current(session), None);
        let first = register(session, "call_1", "bash");
        assert_eq!(
            current(session).map(|tool| tool.call_id),
            Some("call_1".into())
        );
        let second = register(session, "call_2", "read");
        drop(first);
        assert_eq!(
            current(session),
            Some(RunningTool {
                call_id: "call_2".into(),
                name: "read".into()
            }),
            "dropping a stale guard must not clear a newer tool"
        );
        drop(second);
        assert_eq!(current(session), None);
    }
}
