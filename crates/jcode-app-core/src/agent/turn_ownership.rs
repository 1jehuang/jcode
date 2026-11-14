//! Runtime-local ownership protects acceptance, execution and publication.
use crate::protocol::TurnIdentity;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Default)]
struct TurnState {
    latest: u64,
    active: Option<TurnIdentity>,
}

/// Read-only runtime identity snapshot, available while the Agent is locked.
#[derive(Clone)]
pub(crate) struct TurnObserver {
    generation: Arc<str>,
    state: Arc<StdMutex<TurnState>>,
}

impl TurnObserver {
    pub(crate) fn runtime_generation(&self) -> &str {
        &self.generation
    }

    pub(crate) fn snapshot(&self) -> (u64, Option<TurnIdentity>) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.latest, state.active.clone())
    }

    #[cfg(test)]
    pub(crate) fn active(&self) -> Option<TurnIdentity> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn latest_turn_id(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
    }
}

pub(super) struct TurnOwnership {
    gate: Arc<Mutex<()>>,
    observer: TurnObserver,
}

impl Default for TurnOwnership {
    fn default() -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            observer: TurnObserver {
                generation: uuid::Uuid::new_v4().to_string().into(),
                state: Arc::new(StdMutex::new(TurnState::default())),
            },
        }
    }
}

struct Reservation {
    _guard: OwnedMutexGuard<()>,
    observer: TurnObserver,
    identity: TurnIdentity,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self
            .observer
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.active.as_ref() == Some(&self.identity) {
            state.active = None;
        }
    }
}

/// Exclusive bookkeeping capability, deliberately not an execution capability.
pub(crate) struct RuntimeMutationLease {
    _guard: OwnedMutexGuard<()>,
}

/// Non-cloneable execution capability for exactly one runtime and turn.
pub(crate) struct TurnLease {
    reservation: Arc<Reservation>,
}

/// A queue may outlive an aborted producer, but must drain before handoff.
pub(crate) struct TurnPublicationPin {
    _reservation: Arc<Reservation>,
}

/// Resolves once no turn holds the reservation. A cancelled server turn drops
/// its [`TurnLease`] only when its task is next polled, so a replacement turn
/// must await this before claiming or it races the stale lease and is rejected.
pub(crate) struct TurnReleaseSignal {
    gate: Arc<Mutex<()>>,
}

impl TurnReleaseSignal {
    /// Acquiring and immediately releasing the gate proves the prior lease was
    /// dropped. Wrap in a timeout: another legitimate turn may hold it.
    pub(crate) async fn wait(self) {
        drop(Arc::clone(&self.gate).lock_owned().await);
    }
}

impl std::fmt::Debug for TurnPublicationPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TurnPublicationPin")
            .field(&self._reservation.identity)
            .finish()
    }
}

impl TurnLease {
    pub(crate) fn identity(&self) -> &TurnIdentity {
        &self.reservation.identity
    }

    pub(crate) fn publication_pin(&self) -> TurnPublicationPin {
        TurnPublicationPin {
            _reservation: Arc::clone(&self.reservation),
        }
    }
}

impl TurnOwnership {
    pub(super) fn observer(&self) -> TurnObserver {
        self.observer.clone()
    }

    pub(super) fn claim_mutation(&self) -> anyhow::Result<RuntimeMutationLease> {
        Ok(RuntimeMutationLease {
            _guard: Arc::clone(&self.gate)
                .try_lock_owned()
                .map_err(|_| anyhow::anyhow!("Already processing a message"))?,
        })
    }

    pub(super) fn release_signal(&self) -> TurnReleaseSignal {
        TurnReleaseSignal {
            gate: Arc::clone(&self.gate),
        }
    }

    pub(super) fn claim(&self) -> anyhow::Result<TurnLease> {
        let guard = Arc::clone(&self.gate)
            .try_lock_owned()
            .map_err(|_| anyhow::anyhow!("Already processing a message"))?;
        let mut state = self
            .observer
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.latest = state
            .latest
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Turn identity exhausted"))?;
        let identity = TurnIdentity {
            runtime_generation: self.observer.runtime_generation().to_owned(),
            turn_id: state.latest,
        };
        state.active = Some(identity.clone());
        Ok(TurnLease {
            reservation: Arc::new(Reservation {
                _guard: guard,
                observer: self.observer.clone(),
                identity,
            }),
        })
    }

    pub(super) fn validate(&self, lease: &TurnLease) -> anyhow::Result<()> {
        anyhow::ensure!(
            Arc::ptr_eq(
                &self.gate,
                OwnedMutexGuard::mutex(&lease.reservation._guard)
            ),
            "Turn reservation belongs to another runtime"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_is_exclusive_runtime_local_and_released_on_drop() {
        let runtime = TurnOwnership::default();
        let other = TurnOwnership::default();
        let observer = runtime.observer();
        let lease = runtime.claim().unwrap();
        let first = lease.identity().clone();
        assert_eq!(observer.active(), Some(first.clone()));
        assert!(runtime.claim().is_err());
        assert!(runtime.validate(&lease).is_ok());
        assert!(other.validate(&lease).is_err());
        let independent = other.claim().unwrap();
        assert_ne!(
            first.runtime_generation,
            independent.identity().runtime_generation
        );
        drop(lease);
        assert!(observer.active().is_none());
        let next = runtime.claim().unwrap();
        assert!(next.identity().turn_id > first.turn_id);
        assert_eq!(observer.latest_turn_id(), next.identity().turn_id);
        assert!(other.claim().is_err());
        drop(independent);
    }

    #[test]
    fn mutation_excludes_execution_without_publishing_phantom_turn() {
        let runtime = TurnOwnership::default();
        let observer = runtime.observer();
        let mutation = runtime.claim_mutation().unwrap();
        assert!(runtime.claim().is_err());
        assert!(runtime.claim_mutation().is_err());
        assert_eq!(observer.snapshot(), (0, None));
        drop(mutation);
        let turn = runtime.claim().unwrap();
        assert_eq!(turn.identity().turn_id, 1);
        assert!(runtime.claim_mutation().is_err());
        drop(turn);
        let mutation = runtime.claim_mutation().unwrap();
        assert_eq!(observer.snapshot(), (1, None));
        drop(mutation);
        assert_eq!(runtime.claim().unwrap().identity().turn_id, 2);
    }

    #[tokio::test]
    async fn task_abort_releases_reservation() {
        let runtime = TurnOwnership::default();
        let lease = runtime.claim().unwrap();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _lease = lease;
            ready_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        ready_rx.await.unwrap();
        assert!(runtime.claim().is_err());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(runtime.claim().is_ok());
    }

    #[tokio::test]
    async fn release_signal_resolves_only_after_the_held_lease_drops() {
        let runtime = Arc::new(TurnOwnership::default());
        let lease = runtime.claim().unwrap();
        // A task holds the lease until told to release, mirroring a resume
        // turn that keeps its reservation while waiting for a reset.
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let held = tokio::spawn(async move {
            let _lease = lease;
            release_rx.await.unwrap();
        });

        // The signal must not resolve while the lease is still held.
        let mut wait = Box::pin(runtime.release_signal().wait());
        tokio::select! {
            biased;
            _ = &mut wait => panic!("release signal resolved while the lease was held"),
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }
        assert!(runtime.claim().is_err(), "lease still blocks a claim");

        // Once the holder drops the lease, the signal resolves and the
        // replacement can claim, exactly as the message path relies on.
        release_tx.send(()).unwrap();
        held.await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), wait)
            .await
            .expect("release signal must resolve after the lease drops");
        assert!(runtime.claim().is_ok(), "claim succeeds after release");
    }
}
