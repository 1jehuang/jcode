use crate::protocol::ServerEvent;
use crate::server::durable_state::{
    elapsed_exceeds, hashed_request_key, load_json_state, now_unix_ms, save_json_state,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};

const SWARM_MUTATION_DIR: &str = "kcode-swarm-mutations";
const FINAL_STATE_TTL: Duration = Duration::from_secs(30);
const PENDING_STATE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum PersistedSwarmMutationResponse {
    Done,
    AssignTask {
        task_id: String,
        target_session: String,
    },
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_secs: Option<u64>,
    },
    Spawn {
        new_session_id: String,
    },
}

impl PersistedSwarmMutationResponse {
    fn into_server_event(self, id: u64, session_id: &str) -> ServerEvent {
        match self {
            Self::Done => ServerEvent::Done { id },
            Self::AssignTask {
                task_id,
                target_session,
            } => ServerEvent::CommAssignTaskResponse {
                id,
                task_id,
                target_session,
            },
            Self::Error {
                message,
                retry_after_secs,
            } => ServerEvent::Error {
                id,
                message,
                retry_after_secs,
            },
            Self::Spawn { new_session_id } => ServerEvent::CommSpawnResponse {
                id,
                session_id: session_id.to_string(),
                new_session_id,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedSwarmMutationState {
    pub key: String,
    pub action: String,
    pub session_id: String,
    pub created_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_response: Option<PersistedSwarmMutationResponse>,
}

#[derive(Clone)]
struct SwarmMutationWaiter {
    request_id: u64,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
}

/// In-memory coordination for in-flight swarm mutations.
///
/// A single lock guards both the active-key set and the waiter map so the
/// persisted-final check, waiter registration, and active-claim in
/// [`begin_or_replay`] form one atomic step with respect to
/// [`finish_request`] draining waiters and releasing the claim. Splitting
/// these across separate locks allowed a TOCTOU where a duplicate request
/// could observe "no final state" before the original finished, then
/// register itself after the finisher had already drained waiters and
/// cleared the active claim, re-executing the whole mutation (double
/// assign/spawn).
#[derive(Default)]
struct SwarmMutationSync {
    active_keys: HashSet<String>,
    waiters: HashMap<String, Vec<SwarmMutationWaiter>>,
}

#[derive(Clone, Default)]
pub(crate) struct SwarmMutationRuntime {
    sync: Arc<RwLock<SwarmMutationSync>>,
}

fn is_stale(state: &PersistedSwarmMutationState) -> bool {
    if state.final_response.is_some() {
        elapsed_exceeds(state.created_at_unix_ms, FINAL_STATE_TTL)
    } else {
        elapsed_exceeds(state.created_at_unix_ms, PENDING_STATE_TTL)
    }
}

pub(super) fn request_key(session_id: &str, action: &str, components: &[String]) -> String {
    hashed_request_key(session_id, action, components)
}

pub(super) fn load_state(key: &str) -> Option<PersistedSwarmMutationState> {
    load_json_state(SWARM_MUTATION_DIR, key, is_stale)
}

pub(super) fn save_state(state: &PersistedSwarmMutationState) {
    save_json_state(
        SWARM_MUTATION_DIR,
        &state.key,
        state,
        "swarm mutation state",
    )
}

pub(super) fn ensure_pending_state(
    key: &str,
    action: &str,
    session_id: &str,
) -> PersistedSwarmMutationState {
    if let Some(existing) = load_state(key) {
        return existing;
    }

    let state = PersistedSwarmMutationState {
        key: key.to_string(),
        action: action.to_string(),
        session_id: session_id.to_string(),
        created_at_unix_ms: now_unix_ms(),
        final_response: None,
    };
    save_state(&state);
    state
}

pub(super) fn persist_final_response(
    state: &PersistedSwarmMutationState,
    response: PersistedSwarmMutationResponse,
) -> PersistedSwarmMutationState {
    let mut next = state.clone();
    next.final_response = Some(response);
    save_state(&next);
    next
}

pub(super) async fn begin_or_replay(
    runtime: &SwarmMutationRuntime,
    key: &str,
    action: &str,
    session_id: &str,
    request_id: u64,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) -> Option<PersistedSwarmMutationState> {
    begin_with_mode(
        runtime,
        key,
        action,
        session_id,
        request_id,
        client_event_tx,
        true,
    )
    .await
}

/// Like [`begin_or_replay`], but never replays a persisted final response.
///
/// Explicit control-driven mutations (retry/reassign/replace/salvage) must
/// re-dispatch even when an identical mutation finished moments ago: a worker
/// that fails within `FINAL_STATE_TTL` would otherwise turn the coordinator's
/// follow-up retry into a silent no-op replay. Concurrent in-flight duplicates
/// still join the active execution as waiters.
pub(super) async fn begin_or_join_in_flight(
    runtime: &SwarmMutationRuntime,
    key: &str,
    action: &str,
    session_id: &str,
    request_id: u64,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) -> Option<PersistedSwarmMutationState> {
    begin_with_mode(
        runtime,
        key,
        action,
        session_id,
        request_id,
        client_event_tx,
        false,
    )
    .await
}

async fn begin_with_mode(
    runtime: &SwarmMutationRuntime,
    key: &str,
    action: &str,
    session_id: &str,
    request_id: u64,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    replay_final: bool,
) -> Option<PersistedSwarmMutationState> {
    {
        // Hold the sync lock across the persisted-final check, waiter
        // registration, and active-claim. If the original executor finishes
        // concurrently it either persisted the final response before we
        // loaded (we replay it here) or still holds the active claim (we
        // become a waiter that its finish drains). It can never slip fully
        // between the check and the claim, because finish drains/clears
        // under this same lock only after persisting the final state.
        let mut sync = runtime.sync.write().await;
        if replay_final
            && let Some(final_response) = load_state(key).and_then(|state| state.final_response)
        {
            let _ = client_event_tx.send(final_response.into_server_event(request_id, session_id));
            return None;
        }
        sync.waiters
            .entry(key.to_string())
            .or_default()
            .push(SwarmMutationWaiter {
                request_id,
                client_event_tx: client_event_tx.clone(),
            });
        if !sync.active_keys.insert(key.to_string()) {
            return None;
        }
    }

    if replay_final {
        Some(ensure_pending_state(key, action, session_id))
    } else {
        // A control-driven mutation is a fresh attempt: never resurrect a
        // stale persisted final response as this attempt's state.
        let state = PersistedSwarmMutationState {
            key: key.to_string(),
            action: action.to_string(),
            session_id: session_id.to_string(),
            created_at_unix_ms: now_unix_ms(),
            final_response: None,
        };
        save_state(&state);
        Some(state)
    }
}

pub(super) async fn finish_request(
    runtime: &SwarmMutationRuntime,
    state: &PersistedSwarmMutationState,
    response: PersistedSwarmMutationResponse,
) {
    // Persist the final response BEFORE draining waiters/releasing the
    // active claim: a duplicate request that misses the drain below must be
    // able to observe the persisted final state and replay it instead of
    // re-executing the mutation.
    let persisted = persist_final_response(state, response.clone());
    let waiters = {
        let mut sync = runtime.sync.write().await;
        let waiters = sync.waiters.remove(&persisted.key).unwrap_or_default();
        sync.active_keys.remove(&persisted.key);
        waiters
    };
    for waiter in waiters {
        let _ = waiter.client_event_tx.send(
            response
                .clone()
                .into_server_event(waiter.request_id, &persisted.session_id),
        );
    }
}

#[cfg(test)]
#[path = "swarm_mutation_state_tests.rs"]
mod swarm_mutation_state_tests;
