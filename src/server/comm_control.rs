use super::await_members_state::{
    PersistedAwaitMembersState, ensure_pending_state, load_state, persist_final_response,
    request_key,
};
use super::{
    AwaitMembersRuntime, ClientConnectionInfo, SwarmEvent, SwarmEventType, SwarmMember,
    VersionedPlan, broadcast_swarm_plan, broadcast_swarm_status, queue_soft_interrupt_for_session,
    record_swarm_event, truncate_detail, update_member_status,
};
use crate::agent::Agent;
use crate::protocol::{AwaitedMemberStatus, NotificationType, ServerEvent};
use jcode_agent_runtime::SoftInterruptSource;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

async fn awaited_member_statuses(
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
            let (name, status) = members
                .get(session_id)
                .map(|member| (member.friendly_name.clone(), member.status.clone()))
                .unwrap_or((None, "unknown".to_string()));
            let done = target_status.contains(&status)
                || (status == "unknown"
                    && (target_status.contains(&"stopped".to_string())
                        || target_status.contains(&"completed".to_string())));
            AwaitedMemberStatus {
                session_id: session_id.clone(),
                friendly_name: name,
                status,
                done,
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

fn timeout_summary(member_statuses: &[AwaitedMemberStatus]) -> String {
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

fn deadline_to_instant(deadline_unix_ms: u64) -> tokio::time::Instant {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    tokio::time::Instant::now() + Duration::from_millis(deadline_unix_ms.saturating_sub(now_ms))
}

async fn respond_to_waiters(
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

async fn spawn_or_resume_await_members(
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

            if member_statuses.iter().all(|status| status.done) {
                let summary = completion_summary(&member_statuses);
                let _ =
                    persist_final_response(&state, true, member_statuses.clone(), summary.clone());
                respond_to_waiters(&await_members_runtime, &key, true, member_statuses, summary)
                    .await;
                return;
            }

            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    let summary = timeout_summary(&member_statuses);
                    let _ = persist_final_response(&state, false, member_statuses.clone(), summary.clone());
                    respond_to_waiters(&await_members_runtime, &key, false, member_statuses, summary).await;
                    return;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    if await_members_runtime.retain_open_waiters(&key).await == 0 {
                        await_members_runtime.clear_active(&key).await;
                        return;
                    }
                }
                recv_result = event_rx.recv() => match recv_result {
                    Ok(event) => {
                        if event.swarm_id.as_deref() != Some(swarm_id.as_str()) {
                            continue;
                        }

                        match &event.event {
                            SwarmEventType::StatusChange { .. } | SwarmEventType::MemberChange { .. } => {}
                            _ => continue,
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        await_members_runtime.clear_active(&key).await;
                        return;
                    }
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_assign_role(
    id: u64,
    req_session_id: String,
    target_session: String,
    role: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let (swarm_id, is_coordinator) = {
        let members = swarm_members.read().await;
        let swarm_id = members
            .get(&req_session_id)
            .and_then(|member| member.swarm_id.clone());

        let is_coordinator = if let Some(ref sid) = swarm_id {
            let coordinators = swarm_coordinators.read().await;
            let current_coordinator = coordinators.get(sid).cloned();
            drop(coordinators);

            crate::logging::info(&format!(
                "[CommAssignRole] req={} target={} role={} swarm={} current_coord={:?}",
                req_session_id, target_session, role, sid, current_coordinator
            ));

            if current_coordinator.as_deref() == Some(req_session_id.as_str()) {
                true
            } else if role == "coordinator" && target_session == req_session_id {
                drop(members);
                if let Some(ref coord_id) = current_coordinator {
                    let (channel_closed, coord_is_headless) = {
                        let members = swarm_members.read().await;
                        members
                            .get(coord_id)
                            .map(|member| (member.event_tx.is_closed(), member.is_headless))
                            .unwrap_or((true, false))
                    };
                    let not_in_sessions = !sessions.read().await.contains_key(coord_id);
                    channel_closed || not_in_sessions || coord_is_headless
                } else {
                    true
                }
            } else {
                false
            }
        } else {
            false
        };
        (swarm_id, is_coordinator)
    };

    if !is_coordinator {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Only the coordinator can assign roles. (Tip: if the coordinator has disconnected, use assign_role with target_session set to your own session ID to self-promote.)".to_string(),
            retry_after_secs: None,
        });
        return;
    }

    let swarm_id = match swarm_id {
        Some(swarm_id) => swarm_id,
        None => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: "Not in a swarm.".to_string(),
                retry_after_secs: None,
            });
            return;
        }
    };

    {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(&target_session) {
            member.role = role.clone();
        } else {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!("Unknown session '{}'", target_session),
                retry_after_secs: None,
            });
            return;
        }
    }

    if role == "coordinator" {
        {
            let mut coordinators = swarm_coordinators.write().await;
            coordinators.insert(swarm_id.clone(), target_session.clone());
        }
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(&req_session_id) {
            if member.session_id != target_session {
                member.role = "agent".to_string();
            }
        }
    }

    broadcast_swarm_status(&swarm_id, swarm_members, swarms_by_id).await;
    record_swarm_event(
        event_history,
        event_counter,
        swarm_event_tx,
        req_session_id,
        None,
        Some(swarm_id),
        SwarmEventType::Notification {
            notification_type: "role_assignment".to_string(),
            message: format!("{} -> {}", target_session, role),
        },
    )
    .await;
    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_assign_task(
    id: u64,
    req_session_id: String,
    target_session: String,
    task_id: String,
    message: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    soft_interrupt_queues: &super::SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let swarm_id = match require_coordinator_swarm(
        id,
        &req_session_id,
        "Only the coordinator can assign tasks.",
        client_event_tx,
        swarm_members,
        swarm_coordinators,
    )
    .await
    {
        Some(swarm_id) => swarm_id,
        None => return,
    };

    let (task_content, participant_ids, plan_item_count) = {
        let mut plans = swarm_plans.write().await;
        let plan = plans
            .entry(swarm_id.clone())
            .or_insert_with(VersionedPlan::new);
        let found = plan.items.iter_mut().find(|item| item.id == task_id);
        if let Some(item) = found {
            item.assigned_to = Some(target_session.clone());
            item.status = "queued".to_string();
            plan.version += 1;
            plan.participants.insert(req_session_id.clone());
            plan.participants.insert(target_session.clone());
            (
                Some(item.content.clone()),
                plan.participants.clone(),
                plan.items.len(),
            )
        } else {
            (None, HashSet::new(), 0)
        }
    };

    let Some(content) = task_content else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!("Task '{}' not found in swarm plan", task_id),
            retry_after_secs: None,
        });
        return;
    };

    broadcast_swarm_plan(
        &swarm_id,
        Some("task_assigned".to_string()),
        swarm_plans,
        swarm_members,
        swarms_by_id,
    )
    .await;
    record_swarm_event(
        event_history,
        event_counter,
        swarm_event_tx,
        req_session_id.clone(),
        None,
        Some(swarm_id.clone()),
        SwarmEventType::PlanUpdate {
            swarm_id: swarm_id.clone(),
            item_count: plan_item_count,
        },
    )
    .await;

    let coordinator_name = {
        let members = swarm_members.read().await;
        members
            .get(&req_session_id)
            .and_then(|member| member.friendly_name.clone())
    };
    let notification = if let Some(ref extra) = message {
        format!(
            "Task assigned to you by coordinator: {} — {}",
            content, extra
        )
    } else {
        format!("Task assigned to you by coordinator: {}", content)
    };

    let target_agent = {
        let agent_sessions = sessions.read().await;
        agent_sessions.get(&target_session).cloned()
    };
    let _ = queue_soft_interrupt_for_session(
        &target_session,
        notification.clone(),
        false,
        SoftInterruptSource::System,
        soft_interrupt_queues,
        sessions,
    )
    .await;
    if let Some(member) = swarm_members.read().await.get(&target_session) {
        let _ = member.event_tx.send(ServerEvent::Notification {
            from_session: req_session_id.clone(),
            from_name: coordinator_name.clone(),
            notification_type: NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
            },
            message: notification,
        });
    }

    let target_has_client = {
        let connections = client_connections.read().await;
        connections
            .values()
            .any(|connection| connection.session_id == target_session)
    };
    if !target_has_client {
        if let Some(agent_arc) = target_agent {
            let target_session_for_run = target_session.clone();
            let swarm_members_for_run = Arc::clone(swarm_members);
            let swarms_for_run = Arc::clone(swarms_by_id);
            let swarm_plans_for_run = Arc::clone(swarm_plans);
            let swarm_id_for_run = swarm_id.clone();
            let task_id_for_run = task_id.clone();
            let event_history_for_run = Arc::clone(event_history);
            let event_counter_for_run = Arc::clone(event_counter);
            let swarm_event_tx_for_run = swarm_event_tx.clone();
            let assignment_text = if let Some(extra) = message.clone() {
                format!(
                    "{}\n\nAdditional coordinator instructions:\n{}",
                    content, extra
                )
            } else {
                content.clone()
            };
            tokio::spawn(async move {
                {
                    let mut plans = swarm_plans_for_run.write().await;
                    if let Some(plan) = plans.get_mut(&swarm_id_for_run) {
                        if let Some(item) = plan
                            .items
                            .iter_mut()
                            .find(|item| item.id == task_id_for_run)
                        {
                            item.status = "running".to_string();
                            plan.version += 1;
                        }
                    }
                }
                broadcast_swarm_plan(
                    &swarm_id_for_run,
                    Some("task_running".to_string()),
                    &swarm_plans_for_run,
                    &swarm_members_for_run,
                    &swarms_for_run,
                )
                .await;
                update_member_status(
                    &target_session_for_run,
                    "running",
                    Some(truncate_detail(&assignment_text, 120)),
                    &swarm_members_for_run,
                    &swarms_for_run,
                    Some(&event_history_for_run),
                    Some(&event_counter_for_run),
                    Some(&swarm_event_tx_for_run),
                )
                .await;

                let result = {
                    let mut agent = agent_arc.lock().await;
                    agent.run_once_capture(&assignment_text).await
                };

                match result {
                    Ok(_) => {
                        {
                            let mut plans = swarm_plans_for_run.write().await;
                            if let Some(plan) = plans.get_mut(&swarm_id_for_run) {
                                if let Some(item) = plan
                                    .items
                                    .iter_mut()
                                    .find(|item| item.id == task_id_for_run)
                                {
                                    item.status = "done".to_string();
                                    plan.version += 1;
                                }
                            }
                        }
                        broadcast_swarm_plan(
                            &swarm_id_for_run,
                            Some("task_completed".to_string()),
                            &swarm_plans_for_run,
                            &swarm_members_for_run,
                            &swarms_for_run,
                        )
                        .await;
                        update_member_status(
                            &target_session_for_run,
                            "completed",
                            None,
                            &swarm_members_for_run,
                            &swarms_for_run,
                            Some(&event_history_for_run),
                            Some(&event_counter_for_run),
                            Some(&swarm_event_tx_for_run),
                        )
                        .await;
                    }
                    Err(error) => {
                        {
                            let mut plans = swarm_plans_for_run.write().await;
                            if let Some(plan) = plans.get_mut(&swarm_id_for_run) {
                                if let Some(item) = plan
                                    .items
                                    .iter_mut()
                                    .find(|item| item.id == task_id_for_run)
                                {
                                    item.status = "failed".to_string();
                                    plan.version += 1;
                                }
                            }
                        }
                        broadcast_swarm_plan(
                            &swarm_id_for_run,
                            Some("task_failed".to_string()),
                            &swarm_plans_for_run,
                            &swarm_members_for_run,
                            &swarms_for_run,
                        )
                        .await;
                        update_member_status(
                            &target_session_for_run,
                            "failed",
                            Some(truncate_detail(&error.to_string(), 120)),
                            &swarm_members_for_run,
                            &swarms_for_run,
                            Some(&event_history_for_run),
                            Some(&event_counter_for_run),
                            Some(&swarm_event_tx_for_run),
                        )
                        .await;
                    }
                }
            });
        }
    }

    let plan_msg = format!(
        "Plan updated: task '{}' assigned to {}.",
        task_id, target_session
    );
    let members = swarm_members.read().await;
    for sid in participant_ids {
        if sid == target_session || sid == req_session_id {
            continue;
        }
        if let Some(member) = members.get(&sid) {
            let _ = member.event_tx.send(ServerEvent::Notification {
                from_session: req_session_id.clone(),
                from_name: coordinator_name.clone(),
                notification_type: NotificationType::Message {
                    scope: Some("plan".to_string()),
                    channel: None,
                },
                message: plan_msg.clone(),
            });
        }
    }

    let _ = client_event_tx.send(ServerEvent::Done { id });
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_comm_await_members(
    id: u64,
    req_session_id: String,
    target_status: Vec<String>,
    requested_ids: Vec<String>,
    timeout_secs: Option<u64>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    await_members_runtime: &AwaitMembersRuntime,
) {
    let swarm_id = {
        let members = swarm_members.read().await;
        members
            .get(&req_session_id)
            .and_then(|member| member.swarm_id.clone())
    };

    if let Some(swarm_id) = swarm_id {
        let key = request_key(&req_session_id, &swarm_id, &requested_ids, &target_status);
        let persisted = load_state(&key);

        if let Some(final_response) = persisted
            .as_ref()
            .and_then(|state| state.final_response.clone())
        {
            let _ = client_event_tx.send(ServerEvent::CommAwaitMembersResponse {
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
            swarm_members,
            swarms_by_id,
        )
        .await;

        if initial_statuses.is_empty() {
            let _ = client_event_tx.send(ServerEvent::CommAwaitMembersResponse {
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
                requested_deadline,
            )
        });

        await_members_runtime
            .add_waiter(&key, id, client_event_tx)
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
                await_members_runtime,
                &key,
                false,
                initial_statuses,
                summary,
            )
            .await;
            return;
        }

        if await_members_runtime.mark_active_if_new(&key).await {
            spawn_or_resume_await_members(
                state,
                req_session_id,
                swarm_members.clone(),
                swarms_by_id.clone(),
                swarm_event_tx.clone(),
                await_members_runtime.clone(),
            )
            .await;
        }
    } else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm. Use a git repository to enable swarm features.".to_string(),
            retry_after_secs: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::handle_comm_await_members;
    use crate::protocol::ServerEvent;
    use crate::server::{AwaitMembersRuntime, SwarmEvent, SwarmEventType, SwarmMember};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tokio::sync::{RwLock, broadcast, mpsc};

    struct RuntimeEnvGuard {
        _guard: std::sync::MutexGuard<'static, ()>,
        prev_runtime: Option<std::ffi::OsString>,
    }

    impl RuntimeEnvGuard {
        fn new() -> (Self, tempfile::TempDir) {
            let guard = crate::storage::lock_test_env();
            let temp = tempfile::TempDir::new().expect("create runtime dir");
            let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
            crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
            (
                Self {
                    _guard: guard,
                    prev_runtime,
                },
                temp,
            )
        }
    }

    impl Drop for RuntimeEnvGuard {
        fn drop(&mut self) {
            if let Some(prev_runtime) = self.prev_runtime.take() {
                crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
            } else {
                crate::env::remove_var("JCODE_RUNTIME_DIR");
            }
        }
    }

    fn member(session_id: &str, swarm_id: &str, status: &str) -> SwarmMember {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx,
            working_dir: None,
            swarm_id: Some(swarm_id.to_string()),
            swarm_enabled: true,
            status: status.to_string(),
            detail: None,
            friendly_name: Some(session_id.to_string()),
            role: "agent".to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: false,
        }
    }

    fn swarm_event(session_id: &str, swarm_id: &str, event: SwarmEventType) -> SwarmEvent {
        SwarmEvent {
            id: 1,
            session_id: session_id.to_string(),
            session_name: Some(session_id.to_string()),
            swarm_id: Some(swarm_id.to_string()),
            event,
            timestamp: Instant::now(),
            absolute_time: SystemTime::now(),
        }
    }

    #[tokio::test]
    async fn await_members_includes_late_joiners_when_watching_swarm() {
        let (_env, _runtime) = RuntimeEnvGuard::new();
        let swarm_id = "swarm-a";
        let requester = "req";
        let initial_peer = "peer-1";
        let late_peer = "peer-2";
        let await_runtime = AwaitMembersRuntime::default();

        let (client_tx, mut client_rx) = mpsc::unbounded_channel();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            (requester.to_string(), member(requester, swarm_id, "ready")),
            (
                initial_peer.to_string(),
                member(initial_peer, swarm_id, "running"),
            ),
        ])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            swarm_id.to_string(),
            HashSet::from([requester.to_string(), initial_peer.to_string()]),
        )])));
        let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

        handle_comm_await_members(
            1,
            requester.to_string(),
            vec!["completed".to_string()],
            vec![],
            Some(2),
            &client_tx,
            &swarm_members,
            &swarms_by_id,
            &swarm_event_tx,
            &await_runtime,
        )
        .await;

        {
            let mut members = swarm_members.write().await;
            members.insert(
                late_peer.to_string(),
                member(late_peer, swarm_id, "running"),
            );
        }
        {
            let mut swarms = swarms_by_id.write().await;
            swarms
                .get_mut(swarm_id)
                .expect("swarm exists")
                .insert(late_peer.to_string());
        }
        let _ = swarm_event_tx.send(swarm_event(
            late_peer,
            swarm_id,
            SwarmEventType::MemberChange {
                action: "joined".to_string(),
            },
        ));

        {
            let mut members = swarm_members.write().await;
            members
                .get_mut(initial_peer)
                .expect("initial peer exists")
                .status = "completed".to_string();
        }
        let _ = swarm_event_tx.send(swarm_event(
            initial_peer,
            swarm_id,
            SwarmEventType::StatusChange {
                old_status: "running".to_string(),
                new_status: "completed".to_string(),
            },
        ));

        {
            let mut members = swarm_members.write().await;
            members.get_mut(late_peer).expect("late peer exists").status = "completed".to_string();
        }
        let _ = swarm_event_tx.send(swarm_event(
            late_peer,
            swarm_id,
            SwarmEventType::StatusChange {
                old_status: "running".to_string(),
                new_status: "completed".to_string(),
            },
        ));

        let response = tokio::time::timeout(std::time::Duration::from_secs(1), client_rx.recv())
            .await
            .expect("response should arrive")
            .expect("channel should stay open");

        match response {
            ServerEvent::CommAwaitMembersResponse {
                completed, members, ..
            } => {
                assert!(completed, "await should complete after both peers finish");
                let watched: HashSet<String> = members.into_iter().map(|m| m.session_id).collect();
                assert!(watched.contains(initial_peer));
                assert!(watched.contains(late_peer));
            }
            other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn await_members_stops_when_requesting_client_disconnects() {
        let (_env, _runtime) = RuntimeEnvGuard::new();
        let swarm_id = "swarm-b";
        let requester = "req";
        let peer = "peer-1";
        let await_runtime = AwaitMembersRuntime::default();

        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            (requester.to_string(), member(requester, swarm_id, "ready")),
            (peer.to_string(), member(peer, swarm_id, "running")),
        ])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            swarm_id.to_string(),
            HashSet::from([requester.to_string(), peer.to_string()]),
        )])));
        let (swarm_event_tx, swarm_event_rx) = broadcast::channel(32);
        drop(swarm_event_rx);
        let baseline_receivers = swarm_event_tx.receiver_count();

        handle_comm_await_members(
            1,
            requester.to_string(),
            vec!["completed".to_string()],
            vec![],
            Some(60),
            &client_tx,
            &swarm_members,
            &swarms_by_id,
            &swarm_event_tx,
            &await_runtime,
        )
        .await;

        drop(client_rx);

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if swarm_event_tx.receiver_count() == baseline_receivers {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("await task should unsubscribe promptly after client disconnect");
    }

    #[tokio::test]
    async fn await_members_reuses_persisted_deadline_after_reload_retry() {
        let (_env, _runtime_dir) = RuntimeEnvGuard::new();
        let swarm_id = "swarm-c";
        let requester = "req";
        let peer = "peer-1";
        let key = crate::server::await_members_state::request_key(
            requester,
            swarm_id,
            &[],
            &["completed".to_string()],
        );
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        crate::server::await_members_state::save_state(
            &crate::server::await_members_state::PersistedAwaitMembersState {
                key,
                session_id: requester.to_string(),
                swarm_id: swarm_id.to_string(),
                target_status: vec!["completed".to_string()],
                requested_ids: vec![],
                created_at_unix_ms: now_ms,
                deadline_unix_ms: now_ms + 150,
                final_response: None,
            },
        );

        let await_runtime = AwaitMembersRuntime::default();
        let (client_tx, mut client_rx) = mpsc::unbounded_channel();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([
            (requester.to_string(), member(requester, swarm_id, "ready")),
            (peer.to_string(), member(peer, swarm_id, "running")),
        ])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            swarm_id.to_string(),
            HashSet::from([requester.to_string(), peer.to_string()]),
        )])));
        let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

        handle_comm_await_members(
            1,
            requester.to_string(),
            vec!["completed".to_string()],
            vec![],
            Some(60),
            &client_tx,
            &swarm_members,
            &swarms_by_id,
            &swarm_event_tx,
            &await_runtime,
        )
        .await;

        let started = Instant::now();
        let response = tokio::time::timeout(Duration::from_secs(1), client_rx.recv())
            .await
            .expect("response should arrive")
            .expect("channel should stay open");

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "persisted deadline should win over new timeout"
        );

        match response {
            ServerEvent::CommAwaitMembersResponse {
                completed, summary, ..
            } => {
                assert!(!completed, "persisted expired wait should time out");
                assert!(summary.contains("Timed out"));
            }
            other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn await_members_returns_persisted_final_response_after_reload_retry() {
        let (_env, _runtime_dir) = RuntimeEnvGuard::new();
        let swarm_id = "swarm-d";
        let requester = "req";
        let key = crate::server::await_members_state::request_key(
            requester,
            swarm_id,
            &[],
            &["completed".to_string()],
        );
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        crate::server::await_members_state::save_state(
            &crate::server::await_members_state::PersistedAwaitMembersState {
                key,
                session_id: requester.to_string(),
                swarm_id: swarm_id.to_string(),
                target_status: vec!["completed".to_string()],
                requested_ids: vec![],
                created_at_unix_ms: now_ms,
                deadline_unix_ms: now_ms + 60_000,
                final_response: Some(
                    crate::server::await_members_state::PersistedAwaitMembersResult {
                        completed: true,
                        members: vec![crate::protocol::AwaitedMemberStatus {
                            session_id: "peer-1".to_string(),
                            friendly_name: Some("peer-1".to_string()),
                            status: "completed".to_string(),
                            done: true,
                        }],
                        summary: "All 1 members are done: peer-1".to_string(),
                        resolved_at_unix_ms: now_ms,
                    },
                ),
            },
        );

        let await_runtime = AwaitMembersRuntime::default();
        let (client_tx, mut client_rx) = mpsc::unbounded_channel();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            requester.to_string(),
            member(requester, swarm_id, "ready"),
        )])));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
            swarm_id.to_string(),
            HashSet::from([requester.to_string()]),
        )])));
        let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(32);

        handle_comm_await_members(
            1,
            requester.to_string(),
            vec!["completed".to_string()],
            vec![],
            Some(60),
            &client_tx,
            &swarm_members,
            &swarms_by_id,
            &swarm_event_tx,
            &await_runtime,
        )
        .await;

        match client_rx.recv().await.expect("response should arrive") {
            ServerEvent::CommAwaitMembersResponse {
                completed,
                summary,
                members,
                ..
            } => {
                assert!(completed);
                assert_eq!(summary, "All 1 members are done: peer-1");
                assert_eq!(members.len(), 1);
                assert_eq!(members[0].session_id, "peer-1");
            }
            other => panic!("expected CommAwaitMembersResponse, got {other:?}"),
        }
    }
}

pub(super) async fn handle_client_debug_command(
    id: u64,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
) {
    let _ = client_event_tx.send(ServerEvent::Error {
        id,
        message: "ClientDebugCommand is for internal use only".to_string(),
        retry_after_secs: None,
    });
}

pub(super) fn handle_client_debug_response(
    id: u64,
    output: String,
    client_debug_response_tx: &broadcast::Sender<(u64, String)>,
) {
    let _ = client_debug_response_tx.send((id, output));
}

async fn require_coordinator_swarm(
    id: u64,
    req_session_id: &str,
    permission_error: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) -> Option<String> {
    let (swarm_id, is_coordinator) = {
        let members = swarm_members.read().await;
        let swarm_id = members
            .get(req_session_id)
            .and_then(|member| member.swarm_id.clone());
        let is_coordinator = if let Some(ref swarm_id) = swarm_id {
            let coordinators = swarm_coordinators.read().await;
            coordinators
                .get(swarm_id)
                .map(|coordinator| coordinator == req_session_id)
                .unwrap_or(false)
        } else {
            false
        };
        (swarm_id, is_coordinator)
    };

    if !is_coordinator {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: permission_error.to_string(),
            retry_after_secs: None,
        });
        return None;
    }

    match swarm_id {
        Some(swarm_id) => Some(swarm_id),
        None => {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: "Not in a swarm.".to_string(),
                retry_after_secs: None,
            });
            None
        }
    }
}
