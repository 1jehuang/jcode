use crate::protocol::{AwaitedMemberStatus, ServerEvent};
use crate::server::durable_state::{
    hashed_request_key, load_json_state, now_unix_ms, save_json_state, state_dir,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};

const AWAIT_MEMBERS_DIR: &str = "jcode-await-members";
const FINAL_STATE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const PENDING_STATE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAwaitMembersResult {
    pub completed: bool,
    pub members: Vec<AwaitedMemberStatus>,
    pub summary: String,
    pub resolved_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAwaitMembersState {
    pub key: String,
    pub session_id: String,
    pub swarm_id: String,
    pub target_status: Vec<String>,
    pub requested_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub created_at_unix_ms: u64,
    pub deadline_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_response: Option<PersistedAwaitMembersResult>,
}

impl PersistedAwaitMembersState {
    pub fn is_pending(&self) -> bool {
        self.final_response.is_none()
    }

    pub fn remaining_timeout(&self) -> Duration {
        let now = now_unix_ms();
        Duration::from_millis(self.deadline_unix_ms.saturating_sub(now))
    }
}

#[derive(Clone)]
struct AwaitMembersWaiter {
    request_id: u64,
    client_event_tx: mpsc::UnboundedSender<ServerEvent>,
}

#[derive(Clone, Default)]
pub(crate) struct AwaitMembersRuntime {
    active_keys: Arc<RwLock<HashSet<String>>>,
    waiters: Arc<RwLock<HashMap<String, Vec<AwaitMembersWaiter>>>>,
}

impl AwaitMembersRuntime {
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
            .push(AwaitMembersWaiter {
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

    pub(super) async fn retain_open_waiters(&self, key: &str) -> usize {
        let mut waiters = self.waiters.write().await;
        let Some(entries) = waiters.get_mut(key) else {
            return 0;
        };
        entries.retain(|waiter| !waiter.client_event_tx.is_closed());
        let remaining = entries.len();
        if remaining == 0 {
            waiters.remove(key);
        }
        remaining
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

fn is_stale(state: &PersistedAwaitMembersState) -> bool {
    let now = now_unix_ms();
    if let Some(final_response) = &state.final_response {
        now.saturating_sub(final_response.resolved_at_unix_ms) > FINAL_STATE_TTL.as_millis() as u64
    } else {
        now.saturating_sub(state.deadline_unix_ms) > PENDING_STATE_TTL.as_millis() as u64
    }
}

pub(super) fn request_key(
    session_id: &str,
    swarm_id: &str,
    requested_ids: &[String],
    target_status: &[String],
    mode: Option<&str>,
) -> String {
    let mut requested = requested_ids.to_vec();
    requested.sort();

    let mut target = target_status.to_vec();
    target.sort();

    hashed_request_key(
        session_id,
        "await_members",
        &[
            swarm_id.to_string(),
            requested.join("\u{1f}"),
            target.join("\u{1f}"),
            mode.unwrap_or("all").to_string(),
        ],
    )
}

pub(super) fn load_state(key: &str) -> Option<PersistedAwaitMembersState> {
    load_json_state(AWAIT_MEMBERS_DIR, key, is_stale)
}

pub(super) fn save_state(state: &PersistedAwaitMembersState) {
    save_json_state(AWAIT_MEMBERS_DIR, &state.key, state, "await_members state")
}

pub(super) fn ensure_pending_state(
    key: &str,
    session_id: &str,
    swarm_id: &str,
    requested_ids: &[String],
    target_status: &[String],
    mode: Option<&str>,
    deadline_unix_ms: u64,
) -> PersistedAwaitMembersState {
    if let Some(existing) = load_state(key) {
        return existing;
    }

    let state = PersistedAwaitMembersState {
        key: key.to_string(),
        session_id: session_id.to_string(),
        swarm_id: swarm_id.to_string(),
        target_status: target_status.to_vec(),
        requested_ids: requested_ids.to_vec(),
        mode: mode.map(str::to_string),
        created_at_unix_ms: now_unix_ms(),
        deadline_unix_ms,
        final_response: None,
    };
    save_state(&state);
    state
}

pub(super) fn persist_final_response(
    state: &PersistedAwaitMembersState,
    completed: bool,
    members: Vec<AwaitedMemberStatus>,
    summary: String,
) -> PersistedAwaitMembersState {
    let mut next = state.clone();
    next.final_response = Some(PersistedAwaitMembersResult {
        completed,
        members,
        summary,
        resolved_at_unix_ms: now_unix_ms(),
    });
    save_state(&next);
    next
}

pub fn pending_await_members_for_session(session_id: &str) -> Vec<PersistedAwaitMembersState> {
    let dir = state_dir(AWAIT_MEMBERS_DIR);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut pending = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Ok(state) = crate::storage::read_json::<PersistedAwaitMembersState>(&path) else {
            continue;
        };
        if is_stale(&state) {
            let _ = std::fs::remove_file(path);
            continue;
        }
        if state.session_id == session_id
            && state.is_pending()
            && state.deadline_unix_ms > now_unix_ms()
        {
            pending.push(state);
        }
    }

    pending.sort_by_key(|state| state.deadline_unix_ms);
    pending
}
