use super::await_members_state::{
    PersistedAwaitMembersState, ensure_pending_state, load_state, persist_final_response,
    request_key,
};
use super::{AwaitMembersRuntime, SwarmEvent, SwarmMember};
use crate::protocol::{AwaitedMemberStatus, ServerEvent};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{RwLock, broadcast, mpsc};

pub(super) async fn awaited_member_statuses(
    req_session_id: &str,
    swarm_id: &str,
    requested_ids: &[String],
    target_status: &[String],
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) -> Vec<AwaitedMemberStatus> {
    let watch_ids: Vec<String> = if requested_ids.is_empty() {
        let mut watch_ids: Vec<String> = {
            let swarms = swarms_by_id.read().await;
            swarms
                .get(swarm_id)
                .map(|sessions| {
                    sessions
                        .iter()
                        .filter(|session_id| session_id.as_str() != req_session_id)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        watch_ids.sort();
        watch_ids
    } else {
        requested_ids.to_vec()
    };

    let members = swarm_members.read().await;
    watch_ids
        .iter()
        .map(|session_id| {
            let (name, status, completion_report) = members
                .get(session_id)
                .map(|member| {
                    (
                        member.friendly_name.clone(),
                        member.status.clone(),
                        member.latest_completion_report.clone(),
                    )
                })
                .unwrap_or((None, "unknown".to_string(), None));
            let done = target_status.contains(&status)
                || (status == "unknown"
                    && (target_status.contains(&"stopped".to_string())
                        || target_status.contains(&"completed".to_string())));
            AwaitedMemberStatus {
                session_id: session_id.clone(),
                friendly_name: name,
                status,
                done,
                completion_report,
            }
        })
        .collect()
}

fn short_member_name(member: &AwaitedMemberStatus) -> String {
    member
        .friendly_name
        .clone()
        .unwrap_or_else(|| member.session_id[..8.min(member.session_id.len())].to_string())
}

pub(super) fn timeout_summary(member_statuses: &[AwaitedMemberStatus]) -> String {
    let pending: Vec<String> = member_statuses
        .iter()
        .filter(|member| !member.done)
        .map(|member| format!("{} ({})", short_member_name(member), member.status))
        .collect();
    format!("Timed out. Still waiting on: {}", pending.join(", "))
}

fn completion_summary(member_statuses: &[AwaitedMemberStatus]) -> String {
    let done_names: Vec<String> = member_statuses.iter().map(short_member_name).collect();
    format!(
        "All {} members are done: {}",
        done_names.len(),
        done_names.join(", ")
    )
}

pub(super) fn completion_mode(mode: Option<&str>) -> &str {
    match mode {
        Some("any") => "any",
        _ => "all",
    }
}

pub(super) fn mode_satisfied(member_statuses: &[AwaitedMemberStatus], mode: Option<&str>) -> bool {
    match completion_mode(mode) {
        "any" => member_statuses.iter().any(|status| status.done),
        _ => member_statuses.iter().all(|status| status.done),
    }
}

pub(super) fn mode_summary(member_statuses: &[AwaitedMemberStatus], mode: Option<&str>) -> String {
    match completion_mode(mode) {
        "any" => {
            let matching: Vec<String> = member_statuses
                .iter()
                .filter(|member| member.done)
                .map(short_member_name)
                .collect();
            format!(
                "Matched {} member{}: {}",
                matching.len(),
                if matching.len() == 1 { "" } else { "s" },
                matching.join(", ")
            )
        }
        _ => completion_summary(member_statuses),
    }
}

pub(super) fn deadline_to_instant(deadline_unix_ms: u64) -> tokio::time::Instant {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    tokio::time::Instant::now() + Duration::from_millis(deadline_unix_ms.saturating_sub(now_ms))
}

pub(super) async fn respond_to_waiters(
    runtime: &AwaitMembersRuntime,
    key: &str,
    completed: bool,
    members: Vec<AwaitedMemberStatus>,
    summary: String,
) {
    for (request_id, client_event_tx) in runtime.take_waiters(key).await {
        let _ = client_event_tx.send(ServerEvent::CommAwaitMembersResponse {
            id: request_id,
            completed,
            members: members.clone(),
            summary: summary.clone(),
        });
    }
    runtime.clear_active(key).await;
}

pub(super) async fn spawn_or_resume_await_members(
    state: PersistedAwaitMembersState,
    req_session_id: String,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    await_members_runtime: AwaitMembersRuntime,
) {
    let key = state.key.clone();
    let swarm_id = state.swarm_id.clone();
    let requested_ids = state.requested_ids.clone();
    let target_status = state.target_status.clone();
    let mode = state.mode.clone();

    tokio::spawn(async move {
        let mut event_rx = swarm_event_tx.subscribe();
        let deadline = deadline_to_instant(state.deadline_unix_ms);

        loop {
            let member_statuses = awaited_member_statuses(
                &req_session_id,
                &swarm_id,
                &requested_ids,
                &target_status,
                &swarm_members,
                &swarms_by_id,
            )
            .await;

            if member_statuses.is_empty() {
                let summary = "No other members in swarm to wait for.".to_string();
                let _ = persist_final_response(&state, true, vec![], summary.clone());
                respond_to_waiters(&await_members_runtime, &key, true, vec![], summary).await;
                return;
            }

            if mode_satisfied(&member_statuses, mode.as_deref()) {
                let summary = mode_summary(&member_statuses, mode.as_deref());
                let _ =
                    persist_final_response(&state, true, member_statuses.clone(), summary.clone());
                respond_to_waiters(&await_members_runtime, &key, true, member_statuses, summary)
                    .await;
                return;
            }

            if await_members_runtime.retain_open_waiters(&key).await == 0 {
                await_members_runtime.clear_active(&key).await;
                return;
            }

            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    let summary = timeout_summary(&member_statuses);
                    let _ = persist_final_response(&state, false, member_statuses.clone(), summary.clone());
                    respond_to_waiters(&await_members_runtime, &key, false, member_statuses, summary).await;
                    return;
                }
                event = event_rx.recv() => {
                    match event {
                        Ok(event) => {
                            if event.swarm_id.as_deref() != Some(swarm_id.as_str()) {
                                continue;
                            }
                        }
                        Err(_) => {
                            await_members_runtime.clear_active(&key).await;
                            return;
                        }
                    }
                }
            }
        }
    });
}

pub(super) struct CommAwaitMembersContext<'a> {
    pub client_event_tx: &'a mpsc::UnboundedSender<ServerEvent>,
    pub swarm_members: &'a Arc<RwLock<HashMap<String, SwarmMember>>>,
    pub swarms_by_id: &'a Arc<RwLock<HashMap<String, HashSet<String>>>>,
    pub swarm_event_tx: &'a broadcast::Sender<SwarmEvent>,
    pub await_members_runtime: &'a AwaitMembersRuntime,
}

pub(super) async fn handle_comm_await_members(
    id: u64,
    req_session_id: String,
    target_status: Vec<String>,
    requested_ids: Vec<String>,
    mode: Option<String>,
    timeout_secs: Option<u64>,
    ctx: CommAwaitMembersContext<'_>,
) {
    let swarm_id = {
        let members = ctx.swarm_members.read().await;
        members
            .get(&req_session_id)
            .and_then(|member| member.swarm_id.clone())
    };

    if let Some(swarm_id) = swarm_id {
        let key = request_key(
            &req_session_id,
            &swarm_id,
            &requested_ids,
            &target_status,
            mode.as_deref(),
        );
        let persisted = load_state(&key);

        if let Some(final_response) = persisted
            .as_ref()
            .and_then(|state| state.final_response.clone())
        {
            let _ = ctx
                .client_event_tx
                .send(ServerEvent::CommAwaitMembersResponse {
                    id,
                    completed: final_response.completed,
                    members: final_response.members,
                    summary: final_response.summary,
                });
            return;
        }

        let initial_statuses = awaited_member_statuses(
            &req_session_id,
            &swarm_id,
            &requested_ids,
            &target_status,
            ctx.swarm_members,
            ctx.swarms_by_id,
        )
        .await;

        if initial_statuses.is_empty() {
            let _ = ctx
                .client_event_tx
                .send(ServerEvent::CommAwaitMembersResponse {
                    id,
                    completed: true,
                    members: vec![],
                    summary: "No other members in swarm to wait for.".to_string(),
                });
            return;
        }

        let requested_deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + Duration::from_secs(timeout_secs.unwrap_or(3600)).as_millis() as u64;
        let state = persisted.unwrap_or_else(|| {
            ensure_pending_state(
                &key,
                &req_session_id,
                &swarm_id,
                &requested_ids,
                &target_status,
                mode.as_deref(),
                requested_deadline,
            )
        });

        ctx.await_members_runtime
            .add_waiter(&key, id, ctx.client_event_tx)
            .await;

        if state.deadline_unix_ms
            <= SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        {
            let summary = timeout_summary(&initial_statuses);
            let _ =
                persist_final_response(&state, false, initial_statuses.clone(), summary.clone());
            respond_to_waiters(
                ctx.await_members_runtime,
                &key,
                false,
                initial_statuses,
                summary,
            )
            .await;
            return;
        }

        if ctx.await_members_runtime.mark_active_if_new(&key).await {
            spawn_or_resume_await_members(
                state,
                req_session_id,
                ctx.swarm_members.clone(),
                ctx.swarms_by_id.clone(),
                ctx.swarm_event_tx.clone(),
                ctx.await_members_runtime.clone(),
            )
            .await;
        }
    } else {
        let _ = ctx.client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
        });
    }
}
