use crate::protocol::PlanGraphStatus;
use crate::protocol::ServerEvent;
use crate::server::durable_state::{
    elapsed_exceeds, hashed_request_key, load_json_state, now_unix_ms, save_json_state,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};

const SWARM_MUTATION_DIR: &str = "jcode-swarm-mutations";
const FINAL_STATE_TTL: Duration = Duration::from_secs(30);
const PENDING_STATE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "durable mutation responses prioritize straightforward serde persistence over boxing the summary payload"
)]
pub(crate) enum PersistedSwarmMutationResponse {
    Done,
    AssignTask {
        task_id: String,
        target_session: String,
    },
    TaskControl {
        action: String,
        task_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_session: Option<String>,
        status: String,
        summary: PlanGraphStatus,
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
            Self::TaskControl {
                action,
                task_id,
                target_session,
                status,
                summary,
            } => ServerEvent::CommTaskControlResponse {
                id,
                action,
                task_id,
                target_session,
                status,
                summary,
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

#[derive(Clone, Default)]
pub(crate) struct SwarmMutationRuntime {
    active_keys: Arc<RwLock<HashSet<String>>>,
    waiters: Arc<RwLock<HashMap<String, Vec<SwarmMutationWaiter>>>>,
}

impl SwarmMutationRuntime {
    pub(super) async fn add_waiter(
        &self,
        key: &str,
        request_id: u64,
        client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    ) {
        let mut waiters = self.waiters.write().await;
        waiters
            .entry(key.to_string())
            .or_default()
            .push(SwarmMutationWaiter {
                request_id,
                client_event_tx: client_event_tx.clone(),
            });
    }

    pub(super) async fn mark_active_if_new(&self, key: &str) -> bool {
        let mut active = self.active_keys.write().await;
        active.insert(key.to_string())
    }

    pub(super) async fn clear_active(&self, key: &str) {
        self.active_keys.write().await.remove(key);
    }

    pub(super) async fn take_waiters(
        &self,
        key: &str,
    ) -> Vec<(u64, mpsc::UnboundedSender<ServerEvent>)> {
        self.waiters
            .write()
            .await
            .remove(key)
            .unwrap_or_default()
            .into_iter()
            .map(|waiter| (waiter.request_id, waiter.client_event_tx))
            .collect()
    }
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
    if let Some(final_response) = load_state(key).and_then(|state| state.final_response) {
        let _ = client_event_tx.send(final_response.into_server_event(request_id, session_id));
        return None;
    }

    runtime.add_waiter(key, request_id, client_event_tx).await;
    if !runtime.mark_active_if_new(key).await {
        return None;
    }

    Some(ensure_pending_state(key, action, session_id))
}

pub(super) async fn finish_request(
    runtime: &SwarmMutationRuntime,
    state: &PersistedSwarmMutationState,
    response: PersistedSwarmMutationResponse,
) {
    let persisted = persist_final_response(state, response);
    let session_id = persisted.session_id.clone();
    let Some(final_response) = persisted.final_response else {
        runtime.clear_active(&persisted.key).await;
        return;
    };

    for (request_id, client_event_tx) in runtime.take_waiters(&persisted.key).await {
        let _ = client_event_tx.send(
            final_response
                .clone()
                .into_server_event(request_id, &session_id),
        );
    }
    runtime.clear_active(&persisted.key).await;
}

#[cfg(test)]
#[path = "swarm_mutation_state_tests.rs"]
mod swarm_mutation_state_tests;
