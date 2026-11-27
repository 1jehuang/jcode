//! Atomic runtime admission for idle upgrades and scoped session transfers.
//!
//! Unknown ownership is deliberately global. Scoped transfer reservations never
//! queue, and hold their fences until durable ownership has been changed.
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use tokio::sync::{Notify, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

tokio::task_local! {
    static SESSION: String;
}

#[derive(Default)]
struct ScopedState {
    unhealthy: bool,
    unknown: usize,
    // Call sites of live unscoped reservations, so transfer diagnostics can
    // name the work that blocks every session instead of a bare count.
    unknown_origins: HashMap<&'static std::panic::Location<'static>, usize>,
    active: HashMap<String, usize>,
    fenced: BTreeSet<String>,
}

// A panic while updating accounting makes ownership uncertain. Retain the
// state for cleanup, but never admit work or transfers using uncertain counts.
fn lock_state(state: &Mutex<ScopedState>) -> MutexGuard<'_, ScopedState> {
    match state.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            let mut state = poisoned.into_inner();
            state.unhealthy = true;
            jcode_logging::error(
                "Runtime activity accounting poisoned; admission disabled until restart",
            );
            state
        }
    }
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
    origin: &'static std::panic::Location<'static>,
    changed: Arc<Notify>,
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
        let mut state = lock_state(&self.state);
        if self.session.is_none()
            && let Some(origin) = state.unknown_origins.get_mut(self.origin)
        {
            *origin = origin.saturating_sub(1);
            if *origin == 0 {
                state.unknown_origins.remove(self.origin);
            }
        }
        let count = match &self.session {
            Some(session) => state.active.get_mut(session),
            None => Some(&mut state.unknown),
        };
        match count.and_then(|count| {
            let remaining = count.checked_sub(1)?;
            *count = remaining;
            Some(remaining)
        }) {
            Some(0) => {
                if let Some(session) = &self.session {
                    state.active.remove(session);
                }
            }
            Some(_) => {}
            None => {
                state.unhealthy = true;
                jcode_logging::error(
                    "Runtime activity reservation missing from accounting; admission disabled until restart",
                );
            }
        }
        drop(state);
        // Wake transfer admission waiting for this work to drain.
        self.changed.notify_waiters();
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
        let mut state = lock_state(&self.state);
        for session in &self.sessions {
            state.fenced.remove(session);
        }
        self.changed.notify_waiters();
    }
}

// Keep the expected absence case explicit rather than making task-local lookup
// look like a discarded operational error to the swallowed-error guardrail.
#[allow(clippy::manual_ok_err)]
fn current_session() -> Option<String> {
    // No task-local owner is normal outside a scoped reservation. Such work
    // must be counted globally, never assigned an invented session identity.
    match SESSION.try_with(Clone::clone) {
        Ok(session) => Some(session),
        Err(_) => None,
    }
}

impl ActivityGate {
    #[track_caller]
    pub fn try_enter(&self) -> anyhow::Result<ActivityReservation> {
        self.try_enter_owned(current_session())
    }

    /// Use for work whose effects are not limited to the ambient session.
    #[track_caller]
    pub fn try_enter_global(&self) -> anyhow::Result<ActivityReservation> {
        self.try_enter_owned(None)
    }

    #[track_caller]
    pub fn try_enter_session(&self, session: &str) -> anyhow::Result<ActivityReservation> {
        self.try_enter_owned(Some(session.to_owned()))
    }

    #[track_caller]
    fn try_enter_owned(&self, session: Option<String>) -> anyhow::Result<ActivityReservation> {
        let origin = std::panic::Location::caller();
        let runtime = self.runtime.clone().try_read_owned().map_err(|_| {
            anyhow::anyhow!("Runtime is upgrading while idle. Reconnect and retry the request.")
        })?;
        self.reserve(runtime, session, origin)
    }

    fn reserve(
        &self,
        runtime: OwnedRwLockReadGuard<()>,
        session: Option<String>,
        origin: &'static std::panic::Location<'static>,
    ) -> anyhow::Result<ActivityReservation> {
        // Absent ownership must never masquerade as an unrelated session
        // and bypass a transfer fence.
        let session = session.filter(|id| !id.trim().is_empty());
        let mut state = lock_state(&self.scoped);
        anyhow::ensure!(
            !state.unhealthy,
            "Runtime activity accounting unavailable. Restart the runtime."
        );
        anyhow::ensure!(
            session
                .as_ref()
                .map_or(state.fenced.is_empty(), |id| !state.fenced.contains(id)),
            "Session transfer is in progress. Retry the request."
        );
        let count = if let Some(id) = &session {
            state.active.entry(id.clone()).or_default()
        } else {
            &mut state.unknown
        };
        match count.checked_add(1) {
            Some(next) => *count = next,
            None => {
                state.unhealthy = true;
                anyhow::bail!("Runtime activity count overflow. Restart the runtime.");
            }
        }
        if session.is_none() {
            *state.unknown_origins.entry(origin).or_default() += 1;
        }
        Ok(ActivityReservation {
            _runtime: runtime,
            state: self.scoped.clone(),
            session,
            origin,
            changed: self.changed.clone(),
        })
    }

    /// Background APIs cannot reject accepted tasks. Reserve before spawning.
    #[track_caller]
    pub fn enter(&self) -> impl std::future::Future<Output = ActivityReservation> + '_ {
        self.enter_owned(current_session(), std::panic::Location::caller())
    }

    #[track_caller]
    pub fn enter_global(&self) -> impl std::future::Future<Output = ActivityReservation> + '_ {
        self.enter_owned(None, std::panic::Location::caller())
    }

    #[track_caller]
    pub fn enter_session(
        &self,
        session: &str,
    ) -> impl std::future::Future<Output = ActivityReservation> + '_ {
        self.enter_owned(Some(session.to_owned()), std::panic::Location::caller())
    }

    async fn enter_owned(
        &self,
        session: Option<String>,
        origin: &'static std::panic::Location<'static>,
    ) -> ActivityReservation {
        loop {
            // Register before checking the fence so release between check and
            // await cannot lose its wakeup. The runtime lock supplies its own
            // upgrade-release notification without a polling timer.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let runtime = self.runtime.clone().read_owned().await;
            match self.reserve(runtime, session.clone(), origin) {
                Ok(reservation) => return reservation,
                Err(error) => {
                    jcode_logging::debug(&format!("Runtime activity admission waiting: {error}"));
                }
            }
            changed.await;
        }
    }

    pub fn try_idle(&self) -> Option<OwnedRwLockWriteGuard<()>> {
        // Contention is the expected busy signal, not an operational failure.
        match self.runtime.clone().try_write_owned() {
            Ok(runtime) => {
                if lock_state(&self.scoped).unhealthy {
                    None
                } else {
                    Some(runtime)
                }
            }
            Err(_) => None,
        }
    }

    /// Human-readable description of what currently blocks `sessions` from
    /// a scoped transfer. Used for diagnostics only.
    pub fn busy_summary(&self, sessions: &BTreeSet<String>) -> String {
        let state = lock_state(&self.scoped);
        let mut parts = Vec::new();
        if state.unhealthy {
            parts.push("activity accounting unhealthy (restart the daemon)".to_owned());
        }
        if state.unknown != 0 {
            let mut origins: Vec<_> = state
                .unknown_origins
                .iter()
                .map(|(origin, count)| format!("{}:{} x{count}", origin.file(), origin.line()))
                .collect();
            origins.sort();
            if origins.is_empty() {
                parts.push(format!("{} unscoped task(s)", state.unknown));
            } else {
                parts.push(format!(
                    "{} unscoped task(s) from {}",
                    state.unknown,
                    origins.join(", ")
                ));
            }
        }
        let busy: Vec<_> = sessions
            .iter()
            .filter(|id| state.active.contains_key(*id))
            .map(|id| format!("{id} x{}", state.active[id]))
            .collect();
        if !busy.is_empty() {
            parts.push(format!("busy sessions: {}", busy.join(", ")));
        }
        let fenced: Vec<_> = sessions
            .iter()
            .filter(|id| state.fenced.contains(*id))
            .cloned()
            .collect();
        if !fenced.is_empty() {
            parts.push(format!("already fenced: {}", fenced.join(", ")));
        }
        if self.runtime.try_read().is_err() {
            parts.push("runtime upgrade in progress".to_owned());
        }
        if parts.is_empty() {
            "nothing (transient)".to_owned()
        } else {
            parts.join("; ")
        }
    }

    /// Resolves after any reservation or fence is released.
    pub async fn changed(&self) {
        self.changed.notified().await
    }

    pub fn try_idle_sessions(&self, sessions: &BTreeSet<String>) -> Option<SessionIdleReservation> {
        let runtime = match self.runtime.clone().try_read_owned() {
            Ok(runtime) => runtime,
            // A whole-runtime upgrade already owns the gate.
            Err(_) => return None,
        };
        let mut state = lock_state(&self.scoped);
        if state.unhealthy
            || state.unknown != 0
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

    #[track_caller]
    pub fn spawn<F>(&self, future: F) -> Option<tokio::task::JoinHandle<F::Output>>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawn_owned(current_session(), future)
    }

    #[track_caller]
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

    #[track_caller]
    fn spawn_owned<F>(
        &self,
        session: Option<String>,
        future: F,
    ) -> Option<tokio::task::JoinHandle<F::Output>>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let reserved = match self.try_enter_owned(session) {
            Ok(reserved) => reserved,
            Err(error) => {
                jcode_logging::debug(&format!("Runtime activity spawn rejected: {error}"));
                return None;
            }
        };
        Some(tokio::spawn(reserved.scope(future)))
    }
}

pub fn global() -> &'static ActivityGate {
    static GATE: LazyLock<ActivityGate> = LazyLock::new(ActivityGate::default);
    &GATE
}

/// Reserve synchronously before the first poll, inheriting ownership into
/// detached callbacks. Tasks without a known owner remain globally blocking.
#[track_caller]
pub fn spawn<F>(future: F) -> Option<tokio::task::JoinHandle<F::Output>>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    global().spawn(future)
}

#[track_caller]
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

    fn poison(gate: &ActivityGate) {
        let state = gate.scoped.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = state.lock().unwrap();
                panic!("simulate interrupted accounting update");
            })
            .join()
            .is_err()
        );
    }

    fn assert_disabled(gate: &ActivityGate) {
        assert!(gate.try_enter_global().is_err());
        assert!(gate.try_enter_session("selected").is_err());
        assert!(gate.try_idle().is_none());
        assert!(
            gate.try_idle_sessions(&BTreeSet::from(["selected".into()]))
                .is_none()
        );
    }

    #[test]
    fn poisoned_accounting_disables_admission_and_drop_does_not_panic() {
        let gate = ActivityGate::default();
        let global = gate.try_enter_global().unwrap();
        let scoped = gate.try_enter_session("selected").unwrap();
        poison(&gate);
        assert_disabled(&gate);
        drop((global, scoped));
        assert_disabled(&gate);
    }

    #[tokio::test]
    async fn poisoned_fence_cleanup_releases_waiters_without_admitting_work() {
        let gate = Arc::new(ActivityGate::default());
        let fence = gate
            .try_idle_sessions(&BTreeSet::from(["selected".into()]))
            .unwrap();
        let waiter_gate = gate.clone();
        let waiter = tokio::spawn(async move { waiter_gate.enter_session("selected").await });
        tokio::task::yield_now().await;
        poison(&gate);
        drop(fence);
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        assert_disabled(&gate);
        assert!(gate.spawn(async {}).is_none());
        waiter.abort();
        assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
    }

    #[test]
    fn inconsistent_release_counts_fail_closed_instead_of_underflowing() {
        for owner in [None, Some("selected")] {
            let gate = ActivityGate::default();
            let reservation = gate.try_enter_owned(owner.map(str::to_owned)).unwrap();
            {
                let mut state = gate.scoped.lock().unwrap();
                state.active.clear();
                state.unknown = 0;
            }
            drop(reservation);
            assert_disabled(&gate);
        }
    }

    #[test]
    fn overflowing_admission_counts_fail_closed() {
        let gate = ActivityGate::default();
        gate.scoped.lock().unwrap().unknown = usize::MAX;
        assert!(gate.try_enter_global().is_err());
        assert_disabled(&gate);
    }

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

    #[test]
    fn empty_owner_remains_globally_blocking() {
        let gate = ActivityGate::default();
        let selected = BTreeSet::from(["selected".into()]);
        for owner in ["", "   "] {
            let unknown = gate.try_enter_session(owner).unwrap();
            assert!(gate.try_idle_sessions(&selected).is_none());
            drop(unknown);
            let fence = gate.try_idle_sessions(&selected).unwrap();
            assert!(gate.try_enter_session(owner).is_err());
            drop(fence);
        }
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

    #[test]
    fn busy_summary_names_unscoped_origin_and_clears_it() {
        let gate = ActivityGate::default();
        let selected = BTreeSet::from(["a".to_owned()]);
        let held = gate.try_enter_global().unwrap();
        let summary = gate.busy_summary(&selected);
        assert!(summary.contains("runtime_activity.rs:"), "{summary}");
        drop(held);
        assert_eq!(gate.busy_summary(&selected), "nothing (transient)");
        let scoped = gate.try_enter_session("b").unwrap();
        assert!(gate.try_idle_sessions(&selected).is_some());
        drop(scoped);
    }
}
