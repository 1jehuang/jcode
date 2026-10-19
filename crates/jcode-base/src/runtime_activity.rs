//! Atomic admission gate for optional, idle-only runtime replacement.
//!
//! Unlike an explicit developer reload this must never checkpoint or abort work.
//! A replacement uses `try_idle` (never queues a writer), so nested activity
//! reservations cannot deadlock and an active runtime simply defers the upgrade.
use std::sync::{Arc, LazyLock};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

#[derive(Default)]
pub struct ActivityGate(Arc<RwLock<()>>);

impl ActivityGate {
    pub fn try_enter(&self) -> anyhow::Result<OwnedRwLockReadGuard<()>> {
        self.0.clone().try_read_owned().map_err(|_| {
            anyhow::anyhow!("Runtime is upgrading while idle. Reconnect and retry the request.")
        })
    }

    /// Background APIs cannot reject accepted tasks. Reserve before spawning,
    /// then move the reservation into their task, including adopted tools.
    pub async fn enter(&self) -> OwnedRwLockReadGuard<()> {
        self.0.clone().read_owned().await
    }

    pub fn try_idle(&self) -> Option<OwnedRwLockWriteGuard<()>> {
        self.0.clone().try_write_owned().ok()
    }

    pub fn spawn<F>(&self, future: F) -> Option<tokio::task::JoinHandle<F::Output>>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let reserved = self.try_enter().ok()?;
        Some(tokio::spawn(async move {
            let _activity = reserved;
            future.await
        }))
    }
}

pub fn global() -> &'static ActivityGate {
    static GATE: LazyLock<ActivityGate> = LazyLock::new(ActivityGate::default);
    &GATE
}

/// Reserve synchronously, before tokio can defer the first poll. Request
/// handlers already hold a read reservation, so accepted tasks cannot fall in
/// a gap between their Ack and their first model/tool operation.
pub fn spawn<F>(future: F) -> Option<tokio::task::JoinHandle<F::Output>>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    global().spawn(future)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepted_spawn_reserves_before_first_poll_and_rejects_during_upgrade() {
        let gate = ActivityGate::default();
        let (finish, wait) = tokio::sync::oneshot::channel();
        let task = gate.spawn(async move { wait.await.unwrap() }).unwrap();
        // Current-thread runtime has not polled the task yet. The handoff must
        // already prevent exec, not merely register activity on first poll.
        assert!(gate.try_idle().is_none());
        finish.send(()).unwrap();
        task.await.unwrap();
        let idle = gate.try_idle().expect("completed task released admission");
        assert!(
            gate.spawn(async { panic!("rejected work must never poll") })
                .is_none()
        );
        drop(idle);
    }

    #[tokio::test]
    async fn idle_upgrade_excludes_new_work_until_released() {
        let gate = ActivityGate::default();
        let idle = gate.try_idle().expect("idle runtime");
        assert!(gate.try_enter().is_err());
        assert!(gate.try_idle().is_none());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), gate.enter())
                .await
                .is_err()
        );
        drop(idle);
        assert!(gate.try_enter().is_ok());
    }

    #[test]
    fn busy_upgrade_never_waits_or_interrupts_and_nested_work_is_safe() {
        let gate = ActivityGate::default();
        let turn = gate.try_enter().unwrap();
        assert!(gate.try_idle().is_none());
        let tool = gate.try_enter().unwrap();
        drop(turn);
        assert!(gate.try_idle().is_none(), "a tool can outlive its turn");
        drop(tool);
        assert!(gate.try_idle().is_some());
    }
}
