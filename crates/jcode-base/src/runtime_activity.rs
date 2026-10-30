//! Atomic runtime admission for idle upgrades and scoped session transfers.
//!
//! Unknown ownership is deliberately global. Scoped transfer reservations never
//! queue, and hold their fences until durable ownership has been changed.
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::{Notify, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

tokio::task_local! {
    static SESSION: String;
}

#[derive(Default)]
struct ScopedState {
    unknown: usize,
    active: HashMap<String, usize>,
    fenced: BTreeSet<String>,
}

#[derive(Default)]
pub struct ActivityGate {
    runtime: Arc<RwLock<()>>,
    scoped: Arc<Mutex<ScopedState>>,
    changed: Arc<Notify>,
}

pub struct ActivityReservation {
    _runtime: OwnedRwLockReadGuard<()>,
    state: Arc<Mutex<ScopedState>>,
    session: Option<String>,
}

impl ActivityReservation {
    /// Keep admission and its owner together across a task boundary. Nested
    /// detached callbacks inherit the same owner even after this future ends.
    pub async fn scope<F: std::future::Future>(self, future: F) -> F::Output {
        let session = self.session.clone();
        let _activity = self;
        match session {
            Some(session) => SESSION.scope(session, future).await,
            None => future.await,
        }
    }
}

impl Drop for ActivityReservation {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if let Some(session) = &self.session {
            let count = state.active.get_mut(session).unwrap();
            *count -= 1;
            if *count == 0 {
                state.active.remove(session);
            }
        } else {
            state.unknown -= 1;
        }
    }
}

pub struct SessionIdleReservation {
    // A scoped transfer must also prevent a whole-runtime upgrade.
    _runtime: OwnedRwLockReadGuard<()>,
    state: Arc<Mutex<ScopedState>>,
    sessions: BTreeSet<String>,
    changed: Arc<Notify>,
}

impl Drop for SessionIdleReservation {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        for session in &self.sessions {
            state.fenced.remove(session);
        }
        self.changed.notify_waiters();
    }
}

fn current_session() -> Option<String> {
    SESSION.try_with(Clone::clone).ok()
}

impl ActivityGate {
    pub fn try_enter(&self) -> anyhow::Result<ActivityReservation> {
        self.try_enter_owned(current_session())
    }

    /// Use for work whose effects are not limited to the ambient session.
    pub fn try_enter_global(&self) -> anyhow::Result<ActivityReservation> {
        self.try_enter_owned(None)
    }

    pub fn try_enter_session(&self, session: &str) -> anyhow::Result<ActivityReservation> {
        self.try_enter_owned(Some(session.to_owned()))
    }

    fn try_enter_owned(&self, session: Option<String>) -> anyhow::Result<ActivityReservation> {
        let runtime = self.runtime.clone().try_read_owned().map_err(|_| {
            anyhow::anyhow!("Runtime is upgrading while idle. Reconnect and retry the request.")
        })?;
        self.reserve(runtime, session)
    }

    fn reserve(
        &self,
        runtime: OwnedRwLockReadGuard<()>,
        session: Option<String>,
    ) -> anyhow::Result<ActivityReservation> {
        let mut state = self.scoped.lock().unwrap();
        anyhow::ensure!(
            session
                .as_ref()
                .map_or(state.fenced.is_empty(), |id| !state.fenced.contains(id)),
            "Session transfer is in progress. Retry the request."
        );
        if let Some(id) = &session {
            *state.active.entry(id.clone()).or_default() += 1;
        } else {
            state.unknown += 1;
        }
        Ok(ActivityReservation {
            _runtime: runtime,
            state: self.scoped.clone(),
            session,
        })
    }

    /// Background APIs cannot reject accepted tasks. Reserve before spawning.
    pub async fn enter(&self) -> ActivityReservation {
        self.enter_owned(current_session()).await
    }

    pub async fn enter_global(&self) -> ActivityReservation {
        self.enter_owned(None).await
    }

    pub async fn enter_session(&self, session: &str) -> ActivityReservation {
        self.enter_owned(Some(session.to_owned())).await
    }

    async fn enter_owned(&self, session: Option<String>) -> ActivityReservation {
        loop {
            // Register before checking the fence so release between check and
            // await cannot lose its wakeup. The runtime lock supplies its own
            // upgrade-release notification without a polling timer.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let runtime = self.runtime.clone().read_owned().await;
            if let Ok(reservation) = self.reserve(runtime, session.clone()) {
                return reservation;
            }
            changed.await;
        }
    }

    pub fn try_idle(&self) -> Option<OwnedRwLockWriteGuard<()>> {
        self.runtime.clone().try_write_owned().ok()
    }

    pub fn try_idle_sessions(&self, sessions: &BTreeSet<String>) -> Option<SessionIdleReservation> {
        let runtime = self.runtime.clone().try_read_owned().ok()?;
        let mut state = self.scoped.lock().unwrap();
        if state.unknown != 0
            || sessions
                .iter()
                .any(|id| state.active.contains_key(id) || state.fenced.contains(id))
        {
            return None;
        }
        state.fenced.extend(sessions.iter().cloned());
        Some(SessionIdleReservation {
            _runtime: runtime,
            state: self.scoped.clone(),
            sessions: sessions.clone(),
            changed: self.changed.clone(),
        })
    }

    pub fn spawn<F>(&self, future: F) -> Option<tokio::task::JoinHandle<F::Output>>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawn_owned(current_session(), future)
    }

    pub fn spawn_session<F>(
        &self,
        session: &str,
        future: F,
    ) -> Option<tokio::task::JoinHandle<F::Output>>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawn_owned(Some(session.to_owned()), future)
    }

    fn spawn_owned<F>(
        &self,
        session: Option<String>,
        future: F,
    ) -> Option<tokio::task::JoinHandle<F::Output>>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let reserved = self.try_enter_owned(session.clone()).ok()?;
        Some(tokio::spawn(reserved.scope(future)))
    }
}

pub fn global() -> &'static ActivityGate {
    static GATE: LazyLock<ActivityGate> = LazyLock::new(ActivityGate::default);
    &GATE
}

/// Reserve synchronously before the first poll, inheriting ownership into
/// detached callbacks. Tasks without a known owner remain globally blocking.
pub fn spawn<F>(future: F) -> Option<tokio::task::JoinHandle<F::Output>>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    global().spawn(future)
}

pub fn spawn_session<F>(session: &str, future: F) -> Option<tokio::task::JoinHandle<F::Output>>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    global().spawn_session(session, future)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepted_spawn_reserves_before_first_poll_and_rejects_during_upgrade() {
        let gate = ActivityGate::default();
        let (finish, wait) = tokio::sync::oneshot::channel();
        let task = gate.spawn(async move { wait.await.unwrap() }).unwrap();
        assert!(gate.try_idle().is_none());
        finish.send(()).unwrap();
        task.await.unwrap();
        let idle = gate.try_idle().unwrap();
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

    #[tokio::test]
    async fn scoped_waiter_wakes_after_fence_release() {
        let gate = Arc::new(ActivityGate::default());
        let ids = BTreeSet::from(["selected".into()]);
        let fence = gate.try_idle_sessions(&ids).unwrap();
        let waiting_gate = gate.clone();
        let waiter = tokio::spawn(async move { waiting_gate.enter_session("selected").await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        drop(fence);
        let reservation = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        assert!(gate.try_idle_sessions(&ids).is_none());
        drop(reservation);
        assert!(gate.try_idle_sessions(&ids).is_some());
    }

    #[test]
    fn scoped_transfer_excludes_selected_and_unknown_but_not_unrelated_activity() {
        let gate = ActivityGate::default();
        let selected = BTreeSet::from(["selected".into(), "worker".into()]);
        let unrelated = gate.try_enter_session("other").unwrap();
        let fence = gate.try_idle_sessions(&selected).unwrap();
        assert!(gate.try_enter_session("worker").is_err());
        assert!(gate.try_enter().is_err());
        assert!(gate.try_idle().is_none());
        assert!(gate.try_enter_session("other").is_ok());
        drop(fence);
        let worker = gate.try_enter_session("worker").unwrap();
        assert!(gate.try_idle_sessions(&selected).is_none());
        drop(worker);
        let unknown = gate.try_enter().unwrap();
        assert!(gate.try_idle_sessions(&selected).is_none());
        drop((unknown, unrelated));
        assert!(gate.try_idle().is_some());
    }

    #[tokio::test]
    async fn detached_callbacks_inherit_session_and_reserve_before_poll() {
        let gate = Arc::new(ActivityGate::default());
        let selected = BTreeSet::from(["selected".into()]);
        let other_gate = gate.clone();
        let child = gate
            .spawn_session("selected", async move {
                other_gate.spawn(std::future::pending::<()>()).unwrap()
            })
            .unwrap()
            .await
            .unwrap();
        assert!(gate.try_idle_sessions(&selected).is_none());
        assert!(
            gate.try_idle_sessions(&BTreeSet::from(["other".into()]))
                .is_some()
        );
        child.abort();
        let _ = child.await;
        assert!(gate.try_idle_sessions(&selected).is_some());
    }
}
