//! Ephemeral foreground blocking waits, independent of the agent mutex.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static WAITS: LazyLock<Mutex<HashMap<String, Vec<Wait>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static CHANGES: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);

/// Subscribers observe entry and cleanup without waiting for another tool event.
pub fn subscribe_changes() -> tokio::sync::watch::Receiver<u64> {
    CHANGES.subscribe()
}

pub fn session_ids() -> Vec<String> {
    WAITS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect()
}

fn notify_change() {
    CHANGES.send_modify(|generation| *generation = generation.wrapping_add(1));
}

struct Wait {
    token: u64,
    reason: String,
    started: Instant,
}

/// Cancellation, errors and normal completion all drop this registration.
#[must_use]
pub struct WaitGuard {
    session_id: String,
    token: u64,
}

pub fn register(session_id: &str, reason: &str) -> WaitGuard {
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    WAITS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(session_id.to_owned())
        .or_default()
        .push(Wait {
            token,
            reason: reason.to_owned(),
            started: Instant::now(),
        });
    notify_change();
    WaitGuard {
        session_id: session_id.to_owned(),
        token,
    }
}

/// Snapshot a reason and its monotonic age together, never from persisted state.
pub fn current(session_id: &str) -> Option<(String, u64)> {
    WAITS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .and_then(|waits| waits.last())
        .map(|wait| (wait.reason.clone(), wait.started.elapsed().as_secs()))
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        let mut waits = WAITS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(active) = waits.get_mut(&self.session_id) {
            active.retain(|wait| wait.token != self.token);
            if active.is_empty() {
                waits.remove(&self.session_id);
            }
            drop(waits);
            notify_change();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn entry_and_cleanup_notify_subscribers() {
        let mut changes = subscribe_changes();
        let guard = register("wait_registry_notifications", "waiting");
        changes.changed().await.unwrap();
        assert!(
            session_ids()
                .iter()
                .any(|sid| sid == "wait_registry_notifications")
        );
        drop(guard);
        changes.changed().await.unwrap();
        assert_eq!(current("wait_registry_notifications"), None);
    }

    #[test]
    fn stale_guard_cannot_erase_new_wait() {
        let session = "wait_registry_replacement";
        let old = register(session, "old");
        let new = register(session, "new");
        drop(old);
        assert_eq!(current(session), Some(("new".into(), 0)));
        drop(new);
        assert_eq!(current(session), None);
    }

    #[test]
    fn overlapping_wait_completion_restores_older_active_wait() {
        let sid = "wait_registry_overlapping";
        let first = register(sid, "first task");
        let second = register(sid, "second task");
        assert_eq!(current(sid).unwrap().0, "second task");
        drop(second);
        assert_eq!(current(sid).unwrap().0, "first task");
        drop(first);
        assert_eq!(current(sid), None);
    }

    #[test]
    fn reports_monotonic_age() {
        let session = "wait_registry_age";
        let guard = register(session, "waiting");
        WAITS
            .lock()
            .unwrap()
            .get_mut(session)
            .unwrap()
            .last_mut()
            .unwrap()
            .started = Instant::now() - std::time::Duration::from_secs(5);
        assert_eq!(current(session), Some(("waiting".into(), 5)));
        drop(guard);
        assert_eq!(current(session), None);
    }

    #[tokio::test]
    async fn cancellation_clears_wait() {
        let session = "wait_registry_cancel";
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = register(session, "waiting");
            tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        rx.await.unwrap();
        assert!(current(session).is_some());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(current(session), None);
    }
}
