//! Process-global registry of cancel signals for actively running turns.
//!
//! Why this exists (issue #428): a cancel (Esc) is delivered through whatever
//! `SessionControlHandle` the receiving connection happens to hold. That
//! handle's stop signal can be a *different* [`InterruptSignal`] instance from
//! the `graceful_shutdown` signal of the agent actually streaming the turn:
//!
//! - re-attach after a reload or disconnect where cleanup removed the
//!   `shutdown_signals` registration,
//! - server-initiated turns (`spawn_tracked_live_turn`, headless recovery,
//!   swarm wake delivery) running on an agent object the connection never
//!   locked,
//! - headless spawns that never registered a shutdown signal,
//! - the lock-free `cancel_only` fallback built while the agent mutex is busy.
//!
//! Firing a stale instance silently does nothing: the client shows
//! "Interrupting..." while the model keeps generating for minutes and every
//! extra Esc only stacks another `Interrupted` event ("Interrupted [x66]").
//!
//! Every turn now registers its own `graceful_shutdown` signal here for the
//! duration of the turn, and `SessionControlHandle::request_cancel` fires
//! every registered signal for the session in addition to its own handle, so
//! cancellation reaches the in-flight provider stream no matter which handle
//! instance received the request.

use jcode_agent_runtime::InterruptSignal;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
/// All interrupt signals registered for one session's in-flight turns.
type SessionTurnSignals = Vec<(u64, InterruptSignal)>;
static ACTIVE_TURNS: LazyLock<Mutex<HashMap<String, SessionTurnSignals>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// RAII registration for one running turn. Dropping the guard removes the
/// signal from the registry, so signals never outlive the turn that owns them.
///
/// Dropping also resets the signal: a cancel fired through the registry sets
/// the turn's own `graceful_shutdown` flag, and nothing else ever clears that
/// instance (the server's deferred epoch-guarded reset only touches the
/// control handle's signal). Without this, one interrupt would leave the flag
/// permanently set and instantly abort every subsequent turn on the agent.
pub struct ActiveTurnGuard {
    session_id: String,
    token: u64,
    signal: InterruptSignal,
}

/// Register `signal` as the cancel signal for a turn running in `session_id`.
/// Call at turn start; keep the guard alive for the duration of the turn.
pub fn register_active_turn(session_id: &str, signal: InterruptSignal) -> ActiveTurnGuard {
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let active = match ACTIVE_TURNS.lock() {
        Ok(mut map) => {
            let entry = map.entry(session_id.to_string()).or_default();
            entry.push((token, signal.clone()));
            entry.len()
        }
        Err(_) => 0,
    };
    crate::logging::info(&format!(
        "TURN_CANCEL_REGISTERED session={} active_turns={}",
        session_id, active
    ));
    ActiveTurnGuard {
        session_id: session_id.to_string(),
        token,
        signal,
    }
}

/// All cancel signals currently registered for turns in `session_id`.
pub fn active_turn_signals(session_id: &str) -> Vec<InterruptSignal> {
    ACTIVE_TURNS
        .lock()
        .ok()
        .and_then(|map| {
            map.get(session_id)
                .map(|entries| entries.iter().map(|(_, signal)| signal.clone()).collect())
        })
        .unwrap_or_default()
}

/// Move every in-flight turn registration from `old_session_id` to
/// `new_session_id`.
///
/// The registry is keyed by session id, but a session can be *renamed* while a
/// turn is still streaming: attaching to or resuming an existing session
/// (`rename_shutdown_signal`) swaps the connection's session id underneath the
/// running turn. Without migrating here, the running turn stays filed under the
/// old id, `request_cancel` on the new id finds no registered signal, and Esc
/// degrades to firing only the (stale) control-handle signal: the client shows
/// "Interrupting..." and the model keeps generating (issue #732, regression of
/// issue #428).
pub fn rename_active_turns(old_session_id: &str, new_session_id: &str) {
    if old_session_id == new_session_id {
        return;
    }
    let moved = match ACTIVE_TURNS.lock() {
        Ok(mut map) => match map.remove(old_session_id) {
            Some(entries) => {
                let moved = entries.len();
                map.entry(new_session_id.to_string())
                    .or_default()
                    .extend(entries);
                moved
            }
            None => 0,
        },
        Err(_) => 0,
    };
    if moved > 0 {
        crate::logging::info(&format!(
            "TURN_CANCEL_RENAMED old_session={} new_session={} moved={}",
            old_session_id, new_session_id, moved
        ));
    }
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        let remaining = match ACTIVE_TURNS.lock() {
            Ok(mut map) => {
                // Remove by token across every session bucket: the turn may
                // have been migrated to a new session id by
                // [`rename_active_turns`] after this guard was created, so the
                // guard's own `session_id` can be stale. Leaving an orphaned
                // entry behind would let a finished turn's signal be fired by
                // a later cancel.
                let mut remaining = 0usize;
                map.retain(|_, entries| {
                    entries.retain(|(token, _)| *token != self.token);
                    !entries.is_empty()
                });
                if let Some(entries) = map.get(&self.session_id) {
                    remaining = entries.len();
                }
                remaining
            }
            Err(_) => 0,
        };
        // A turn's cancel flag must never outlive the turn: if a cancel fired
        // this signal (possibly through a stale control handle that nothing
        // else ever resets), leaving it set would instantly abort the next
        // turn on this agent.
        self.signal.reset();
        crate::logging::info(&format!(
            "TURN_CANCEL_UNREGISTERED session={} active_turns={}",
            self.session_id, remaining
        ));
    }
}

/// Whether any turn is currently registered as running for `session_id`.
///
/// A cancel that arrives while the session is idle has nothing to stop, but
/// the "no local task" path still fires the signal and only clears it on a
/// 500ms timer, because it cannot tell an idle session from one whose turn is
/// owned by another connection. Any message sent inside that window is aborted
/// the instant it starts, which looks to the user like a message that vanished
/// with no reply. The registry already knows whether a turn exists, so ask it
/// rather than guessing.
pub fn has_active_turn(session_id: &str) -> bool {
    ACTIVE_TURNS
        .lock()
        .ok()
        .is_some_and(|map| map.get(session_id).is_some_and(|turns| !turns.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_drop_tracks_active_signals() {
        let session_id = "turn_cancel_registry_register_drop";
        assert!(active_turn_signals(session_id).is_empty());

        let signal = InterruptSignal::new();
        let guard = register_active_turn(session_id, signal.clone());
        let registered = active_turn_signals(session_id);
        assert_eq!(registered.len(), 1);
        assert!(registered[0].same_instance(&signal));

        drop(guard);
        assert!(
            active_turn_signals(session_id).is_empty(),
            "dropping the guard must remove the registration"
        );
    }

    /// An idle cancel must be distinguishable from one racing a turn that
    /// another connection owns. Firing the signal when nothing is running
    /// leaves the flag set for the 500ms until the deferred reset, and any
    /// message sent in that window is aborted the instant it starts, with no
    /// reply and no error shown to the user.
    #[test]
    fn has_active_turn_tracks_registration_lifetime() {
        let session_id = "turn_cancel_registry_has_active";
        assert!(!has_active_turn(session_id));

        let guard = register_active_turn(session_id, InterruptSignal::new());
        assert!(has_active_turn(session_id));

        drop(guard);
        assert!(
            !has_active_turn(session_id),
            "an ended turn must not look active to a later cancel"
        );
    }

    #[test]
    fn multiple_turns_for_one_session_are_all_listed() {
        let session_id = "turn_cancel_registry_multiple";
        let first = InterruptSignal::new();
        let second = InterruptSignal::new();
        let _guard_first = register_active_turn(session_id, first.clone());
        let guard_second = register_active_turn(session_id, second.clone());

        let registered = active_turn_signals(session_id);
        assert_eq!(registered.len(), 2);
        assert!(registered.iter().any(|signal| signal.same_instance(&first)));
        assert!(
            registered
                .iter()
                .any(|signal| signal.same_instance(&second))
        );

        drop(guard_second);
        let registered = active_turn_signals(session_id);
        assert_eq!(registered.len(), 1);
        assert!(registered[0].same_instance(&first));
    }

    /// Issue #732: a session renamed mid-turn (attach/resume) must carry its
    /// in-flight turn registration to the new id, otherwise a cancel routed
    /// through the new session id reaches no running turn.
    #[test]
    fn rename_moves_active_turns_to_the_new_session_id() {
        let old_id = "turn_cancel_registry_rename_old";
        let new_id = "turn_cancel_registry_rename_new";
        let signal = InterruptSignal::new();
        let guard = register_active_turn(old_id, signal.clone());

        rename_active_turns(old_id, new_id);

        assert!(
            active_turn_signals(old_id).is_empty(),
            "the old session id must no longer hold the registration"
        );
        let registered = active_turn_signals(new_id);
        assert_eq!(registered.len(), 1, "the turn must follow the rename");
        assert!(registered[0].same_instance(&signal));

        // A cancel routed through the new id now reaches the running turn.
        for found in active_turn_signals(new_id) {
            found.fire();
        }
        assert!(signal.is_set(), "cancel must reach the renamed turn");

        drop(guard);
        assert!(
            active_turn_signals(new_id).is_empty(),
            "dropping the guard must clean up the migrated registration"
        );
    }

    /// The guard holds the pre-rename session id, so cleanup must find the
    /// entry by token rather than leaving an orphan behind that a later cancel
    /// could fire against a finished turn.
    #[test]
    fn dropping_a_renamed_guard_leaves_no_orphan_entry() {
        let old_id = "turn_cancel_registry_orphan_old";
        let new_id = "turn_cancel_registry_orphan_new";
        let survivor_signal = InterruptSignal::new();
        let _survivor = register_active_turn(new_id, survivor_signal.clone());

        let moved_signal = InterruptSignal::new();
        let moved_guard = register_active_turn(old_id, moved_signal.clone());
        rename_active_turns(old_id, new_id);
        assert_eq!(active_turn_signals(new_id).len(), 2);

        drop(moved_guard);
        let registered = active_turn_signals(new_id);
        assert_eq!(
            registered.len(),
            1,
            "only the still-running turn may remain registered"
        );
        assert!(registered[0].same_instance(&survivor_signal));
        assert!(active_turn_signals(old_id).is_empty());
    }
}
