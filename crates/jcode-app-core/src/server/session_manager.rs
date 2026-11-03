//! The server's single publication authority for live session runtimes.
//!
//! Callers can inspect the registry, but cannot mutate its map. A reservation
//! spans lookup, construction/restoration, and publication for one session ID.
//! Every creation path uses the same gate, including headed and headless work.

use crate::agent::Agent;
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock, RwLockReadGuard};

type AgentHandle = Arc<Mutex<Agent>>;
type AgentMap = HashMap<String, AgentHandle>;

#[derive(Default)]
pub(crate) struct SessionManager {
    agents: RwLock<AgentMap>,
    reservations: StdMutex<HashMap<String, Weak<Mutex<()>>>>,
    // Removal hides a runtime, but must not grant a second writer while an old
    // client or task still owns a handle. These weak tombstones do not retain
    // idle agents and expire as soon as the last old handle is gone.
    owners: StdMutex<HashMap<String, Weak<Mutex<Agent>>>>,
    observers: StdMutex<HashMap<String, crate::agent::TurnObserver>>,
}

#[cfg(test)]
#[path = "session_manager_tests.rs"]
mod tests;

impl SessionManager {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn turn_observer(&self, session_id: &str) -> Option<crate::agent::TurnObserver> {
        self.observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
    }

    pub(super) async fn read(&self) -> RwLockReadGuard<'_, AgentMap> {
        self.agents.read().await
    }

    pub(super) fn try_read(
        &self,
    ) -> std::result::Result<RwLockReadGuard<'_, AgentMap>, tokio::sync::TryLockError> {
        self.agents.try_read()
    }

    /// Acquire before any connection, registry or agent guard. Cancellation
    /// drops the owned guard, so an abandoned restore cannot strand a slot.
    pub(super) async fn reserve(&self, session_id: &str) -> SessionReservation<'_> {
        let gate = {
            let mut gates = self
                .reservations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            gates.retain(|_, gate| gate.strong_count() > 0);
            match gates.get(session_id).and_then(Weak::upgrade) {
                Some(gate) => gate,
                None => {
                    let gate = Arc::new(Mutex::new(()));
                    gates.insert(session_id.to_owned(), Arc::downgrade(&gate));
                    gate
                }
            }
        };
        SessionReservation {
            manager: self,
            session_id: session_id.to_owned(),
            _guard: gate.lock_owned().await,
        }
    }

    pub(super) async fn register_new(&self, session_id: String, agent: AgentHandle) -> Result<()> {
        self.reserve(&session_id).await.publish(agent).await
    }

    pub(super) async fn get_or_start<F, Fut>(
        &self,
        session_id: &str,
        start: F,
    ) -> Result<AgentHandle>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<AgentHandle>>,
    {
        let slot = self.reserve(session_id).await;
        if let Some(agent) = slot.existing().await {
            return Ok(agent);
        }
        slot.ensure_vacant()?;
        let agent = start().await?;
        slot.publish(Arc::clone(&agent)).await?;
        Ok(agent)
    }

    pub(super) async fn remove(&self, session_id: &str) -> Option<AgentHandle> {
        let mut agents = self.agents.write().await;
        let removed = agents.remove(session_id);
        if removed.is_some() {
            self.observers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(session_id);
        }
        removed
    }

    pub(super) async fn remove_if_same(&self, session_id: &str, expected: &AgentHandle) -> bool {
        let mut agents = self.agents.write().await;
        if agents
            .get(session_id)
            .is_some_and(|agent| Arc::ptr_eq(agent, expected))
        {
            agents.remove(session_id);
            self.observers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(session_id);
            true
        } else {
            false
        }
    }

    pub(super) async fn retain(&self, mut keep: impl FnMut(&String, &AgentHandle) -> bool) {
        let mut agents = self.agents.write().await;
        agents.retain(|id, agent| keep(id, agent));
        self.observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|id, _| agents.contains_key(id));
    }

    #[cfg(test)]
    pub(super) fn from_agents(agents: AgentMap) -> Self {
        let observers = agents
            .iter()
            .filter_map(|(id, agent)| {
                agent
                    .try_lock()
                    .ok()
                    .map(|owner| (id.clone(), owner.server_turn_observer()))
            })
            .collect();
        let owners = agents
            .iter()
            .map(|(id, agent)| (id.clone(), Arc::downgrade(agent)))
            .collect();
        Self {
            agents: RwLock::new(agents),
            reservations: StdMutex::default(),
            owners: StdMutex::new(owners),
            observers: StdMutex::new(observers),
        }
    }

    #[cfg(test)]
    pub(super) async fn clear_for_test(&self) {
        self.agents.write().await.clear();
        self.owners.lock().unwrap().clear();
        self.observers.lock().unwrap().clear();
    }

    #[cfg(test)]
    pub(super) async fn write_for_test(&self) -> tokio::sync::RwLockWriteGuard<'_, AgentMap> {
        self.agents.write().await
    }
}

/// Only the manager can construct a publication capability. There is no raw
/// write-map accessor in production, and publication never replaces an owner.
pub(super) struct SessionReservation<'a> {
    manager: &'a SessionManager,
    session_id: String,
    _guard: OwnedMutexGuard<()>,
}

impl SessionReservation<'_> {
    /// Taking an Agent by value prevents restoration through a published Arc.
    pub(super) async fn restore_new(
        &self,
        mut owner: Agent,
        working_dir: Option<&str>,
        selfdev: bool,
    ) -> Result<(
        AgentHandle,
        crate::session::SessionStatus,
        bool,
        crate::agent::RuntimeMutationLease,
    )> {
        self.ensure_vacant()?;
        let lease = owner.claim_runtime_mutation()?;
        let previous = super::run_blocking_io(|| {
            owner.restore_session_deferring_save(&self.session_id, working_dir)
        })?;
        if selfdev {
            owner.set_canary("self-dev");
        }
        let is_canary = owner.is_canary();
        Ok((Arc::new(Mutex::new(owner)), previous, is_canary, lease))
    }

    pub(super) async fn refresh_stale(&self, working_dir: Option<&str>) -> Result<()> {
        if let Some(existing) = self.existing().await
            && let Ok(mut owner) = existing.try_lock()
            && owner.session_copy_is_stale()
        {
            let _lease = owner.claim_runtime_mutation()?;
            super::run_blocking_io(|| {
                owner.restore_session_deferring_save(&self.session_id, working_dir)
            })?;
            super::run_blocking_io(|| owner.persist_restored_session());
        }
        Ok(())
    }

    /// Must precede restoration, which itself may persist session state.
    pub(super) fn ensure_vacant(&self) -> Result<()> {
        let mut owners = self
            .manager
            .owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        owners.retain(|_, owner| owner.strong_count() > 0);
        if owners
            .get(&self.session_id)
            .and_then(Weak::upgrade)
            .is_some()
        {
            bail!("Session {} still has a runtime owner", self.session_id);
        }
        Ok(())
    }

    pub(super) async fn existing(&self) -> Option<AgentHandle> {
        self.manager
            .agents
            .read()
            .await
            .get(&self.session_id)
            .cloned()
    }

    pub(super) async fn publish(&self, agent: AgentHandle) -> Result<()> {
        self.publish_rekeyed(None, agent).await
    }

    async fn publish_rekeyed(&self, old_id: Option<&str>, agent: AgentHandle) -> Result<()> {
        let observer = {
            let owner = agent
                .try_lock()
                .map_err(|_| anyhow::anyhow!("Cannot publish a busy runtime"))?;
            if owner.session_id() != self.session_id {
                bail!(
                    "Runtime identity does not match reserved session {}",
                    self.session_id
                );
            }
            owner.server_turn_observer()
        };
        let mut agents = self.manager.agents.write().await;
        let mut owners = self
            .manager
            .owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        owners.retain(|_, owner| owner.strong_count() > 0);
        if let Some(owner) = owners.get(&self.session_id).and_then(Weak::upgrade)
            && !Arc::ptr_eq(&owner, &agent)
        {
            bail!("Session {} still has a runtime owner", self.session_id);
        }
        if let Some(owner) = agents.get(&self.session_id)
            && !Arc::ptr_eq(owner, &agent)
        {
            bail!("Session {} already has a runtime owner", self.session_id);
        }
        if let Some((other_id, _)) = agents.iter().find(|(id, owner)| {
            id.as_str() != self.session_id
                && Some(id.as_str()) != old_id
                && Arc::ptr_eq(owner, &agent)
        }) {
            bail!("Runtime is already published as {other_id}; an explicit rekey is required");
        }
        if let Some(old_id) = old_id.filter(|id| *id != self.session_id) {
            if !agents
                .get(old_id)
                .is_some_and(|owner| Arc::ptr_eq(owner, &agent))
            {
                bail!("Cannot rekey session {old_id}: runtime owner changed");
            }
            agents.remove(old_id);
            owners.remove(old_id);
            self.manager
                .observers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(old_id);
        }
        self.manager
            .observers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(self.session_id.clone(), observer);
        owners.insert(self.session_id.clone(), Arc::downgrade(&agent));
        agents.insert(self.session_id.clone(), agent);
        Ok(())
    }
}
