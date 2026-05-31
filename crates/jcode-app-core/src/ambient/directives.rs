use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::paths::ambient_dir;
use crate::storage;

// ---------------------------------------------------------------------------
// User Directives (from email replies)
// ---------------------------------------------------------------------------

/// A user directive received via email reply to an ambient cycle notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDirective {
    pub id: String,
    pub text: String,
    pub received_at: DateTime<Utc>,
    pub in_reply_to_cycle: String,
    pub consumed: bool,
}

fn directives_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("directives.json"))
}

pub fn load_directives() -> Vec<UserDirective> {
    directives_path()
        .ok()
        .and_then(|p| {
            if p.exists() {
                storage::read_json(&p).ok()
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn save_directives(directives: &[UserDirective]) -> Result<()> {
    storage::write_json(&directives_path()?, directives)
}

/// Store a new directive from an email reply.
pub fn add_directive(text: String, in_reply_to: String) -> Result<()> {
    let mut directives = load_directives();
    directives.push(UserDirective {
        id: format!("dir_{:08x}", rand::random::<u32>()),
        text,
        received_at: Utc::now(),
        in_reply_to_cycle: in_reply_to,
        consumed: false,
    });
    save_directives(&directives)
}

/// Record an auditable session-end marker for the ambient runner.
///
/// See `docs/SESSION_END_LEARNINGS_RULES.md` (Rule 8): when a session ends via
/// `/exit`/`/quit` the session-end learnings capture runs as an ambient task,
/// and this leaves a data-only trail in `~/.jcode/ambient/directives.json` so
/// the ambient runner has an auditable record and can pick up any follow-up.
///
/// The directive text is data only; it is never executed as an instruction.
/// Failures are intentionally swallowed by callers: this is best-effort
/// bookkeeping that must never block or fail session teardown.
pub fn record_session_end_directive(session_id: &str) -> Result<()> {
    let (text, reply_to) = session_end_directive_fields(session_id);
    add_directive(text, reply_to)
}

/// Build the (text, in_reply_to) pair for a session-end directive.
///
/// Pure helper so the format can be unit-tested without touching the
/// `~/.jcode/ambient/directives.json` store.
fn session_end_directive_fields(session_id: &str) -> (String, String) {
    (
        format!("session-end capture ran for session {session_id}"),
        format!("session_end:{session_id}"),
    )
}

/// Take all unconsumed directives, marking them as consumed.
pub fn take_pending_directives() -> Vec<UserDirective> {
    let mut all = load_directives();
    let pending: Vec<_> = all.iter().filter(|d| !d.consumed).cloned().collect();
    if pending.is_empty() {
        return pending;
    }
    for d in &mut all {
        if !d.consumed {
            d.consumed = true;
        }
    }
    let _ = save_directives(&all);
    pending
}

/// Check if there are any unconsumed directives.
pub fn has_pending_directives() -> bool {
    load_directives().iter().any(|d| !d.consumed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_end_directive_fields_format() {
        let (text, reply_to) = session_end_directive_fields("sess-123");
        assert_eq!(text, "session-end capture ran for session sess-123");
        assert_eq!(reply_to, "session_end:sess-123");
    }
}
