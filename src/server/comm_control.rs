#![cfg_attr(test, allow(clippy::items_after_test_module))]

use super::append_swarm_completion_report_instructions;
use super::swarm::{now_unix_ms, swarm_task_heartbeat_interval, touch_swarm_task_progress};
use super::swarm_mutation_state::{
    PersistedSwarmMutationResponse, begin_or_replay as begin_swarm_mutation_or_replay,
    finish_request as finish_swarm_mutation_request, request_key as swarm_mutation_request_key,
};
use super::{
    ClientConnectionInfo, SwarmEvent, SwarmEventType, SwarmMember, SwarmMutationRuntime,
    SwarmState, SwarmTaskProgress, VersionedPlan, broadcast_swarm_plan,
    broadcast_swarm_plan_with_previous, broadcast_swarm_status, fanout_session_event,
    persist_swarm_state_for, queue_soft_interrupt_for_session, record_swarm_event, truncate_detail,
    update_member_status, update_member_status_with_report,
};
use crate::agent::Agent;
use crate::plan::{
    TaskControlAction, assignment_affinities_for_task, assignment_loads,
    build_control_assignment_text, combine_assignment_text, explicit_task_blocked_reason,
    next_unassigned_runnable_item_id, task_control_action_allows_status, task_control_status_error,
    task_control_target_item_id,
};
use crate::protocol::{NotificationType, PlanGraphStatus, ServerEvent};
use jcode_agent_runtime::SoftInterruptSource;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};

type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;

fn filter_swarm_agent_candidates<'a>(
    members: &'a HashMap<String, SwarmMember>,
    req_session_id: &str,
    swarm_id: &str,
) -> Vec<&'a SwarmMember> {
    members
        .values()
        .filter(|member| {
            member.session_id != req_session_id
                && member.swarm_id.as_deref() == Some(swarm_id)
                && member.role == "agent"
                && matches!(member.status.as_str(), "ready" | "completed")
        })
        .collect()
}

#[derive(Clone, Debug)]
struct TaskSnapshot {
    content: String,
    status: String,
    assigned_to: Option<String>,
    progress: Option<SwarmTaskProgress>,
}

async fn task_snapshot_for(
    swarm_id: &str,
    task_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Option<TaskSnapshot> {
    let plans = swarm_plans.read().await;
    let plan = plans.get(swarm_id)?;
    let item = plan.items.iter().find(|item| item.id == task_id)?;
    Some(TaskSnapshot {
        content: item.content.clone(),
        status: item.status.clone(),
        assigned_to: item.assigned_to.clone(),
        progress: plan.task_progress.get(task_id).cloned(),
    })
}

async fn plan_graph_status_for(
    swarm_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> PlanGraphStatus {
    let plans = swarm_plans.read().await;
    let plan = plans.get(swarm_id);
    if let Some(plan) = plan {
        PlanGraphStatus::from_versioned_plan(swarm_id, plan, Some(8), Vec::new())
    } else {
        PlanGraphStatus::empty_for_swarm(swarm_id)
    }
}

async fn requeue_existing_assignment(
    swarm_id: &str,
    req_session_id: &str,
    assignee_session: &str,
    task_id: &str,
    assignment_summary: String,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Option<(String, HashSet<String>, usize)> {
    let now_ms = now_unix_ms();
    let mut plans = swarm_plans.write().await;
    let plan = plans.get_mut(swarm_id)?;
    let item = plan.items.iter_mut().find(|item| item.id == task_id)?;
    item.assigned_to = Some(assignee_session.to_string());
    item.status = "queued".to_string();
    plan.task_progress.insert(
        task_id.to_string(),
        SwarmTaskProgress {
            assigned_session_id: Some(assignee_session.to_string()),
            assignment_summary: Some(truncate_detail(&assignment_summary, 120)),
            assigned_at_unix_ms: Some(now_ms),
            ..SwarmTaskProgress::default()
        },
    );
    plan.version += 1;
    plan.participants.insert(req_session_id.to_string());
    plan.participants.insert(assignee_session.to_string());
    Some((
        item.content.clone(),
        plan.participants.clone(),
        plan.items.len(),
    ))
}

async fn active_swarm_member(
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<SwarmMember> {
    let members = swarm_members.read().await;
    members.get(session_id).cloned()
}

async fn task_agent_session(
    session_id: &str,
    sessions: &SessionAgents,
) -> Option<Arc<Mutex<Agent>>> {
    let guard = sessions.read().await;
    guard.get(session_id).cloned()
}

async fn resolve_assignment_target_session(
    req_session_id: &str,
    swarm_id: &str,
    requested_target: Option<&str>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Result<String, String> {
    let members = swarm_members.read().await;

    if let Some(target) = requested_target {
        if target == req_session_id {
            return Err("Coordinator cannot assign a swarm task to itself.".to_string());
        }
        let Some(member) = members.get(target) else {
            return Err(format!("Unknown session '{target}'"));
        };
        if member.swarm_id.as_deref() != Some(swarm_id) {
            return Err(format!(
                "Session '{}' is not in swarm '{}' and cannot receive this task.",
                target, swarm_id
            ));
        }
        return Ok(target.to_string());
    }

    let assignment_counts = {
        let plans = swarm_plans.read().await;
        plans
            .get(swarm_id)
            .map(assignment_loads)
            .unwrap_or_default()
    };

    let mut candidates = filter_swarm_agent_candidates(&members, req_session_id, swarm_id);

    candidates.sort_by(|left, right| {
        let left_load = assignment_counts
            .get(&left.session_id)
            .copied()
            .unwrap_or(0);
        let right_load = assignment_counts
            .get(&right.session_id)
            .copied()
            .unwrap_or(0);
        let left_rank = if left.status == "ready" { 0 } else { 1 };
        let right_rank = if right.status == "ready" { 0 } else { 1 };
        left_load
            .cmp(&right_load)
            .then_with(|| left_rank.cmp(&right_rank))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });

    candidates
        .first()
        .map(|member| member.session_id.clone())
        .ok_or_else(|| {
            "No ready or completed swarm agents are available for automatic task assignment."
                .to_string()
        })
}

async fn task_id_for_target_session(
    swarm_id: &str,
    target_session: &str,
    action: TaskControlAction,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Result<String, String> {
    let plans = swarm_plans.read().await;
    let Some(plan) = plans.get(swarm_id) else {
        return Err("No swarm plan exists for this swarm.".to_string());
    };
    task_control_target_item_id(&plan.items, target_session, action)
}

async fn next_unassigned_runnable_task_id(
    swarm_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Option<String> {
    let plans = swarm_plans.read().await;
    let plan = plans.get(swarm_id)?;
    next_unassigned_runnable_item_id(plan)
}

async fn resolve_assignment_target_for_task(
    req_session_id: &str,
    swarm_id: &str,
    task_id: &str,
    requested_target: Option<&str>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Result<String, String> {
    if requested_target.is_some() {
        return resolve_assignment_target_session(
            req_session_id,
            swarm_id,
            requested_target,
            swarm_members,
            swarm_plans,
        )
        .await;
    }

    let affinities = {
        let plans = swarm_plans.read().await;
        let Some(plan) = plans.get(swarm_id) else {
            return Err("No runnable unassigned tasks are available in the swarm plan".to_string());
        };
        assignment_affinities_for_task(plan, task_id)?
    };

    let members = swarm_members.read().await;
    let mut candidates = filter_swarm_agent_candidates(&members, req_session_id, swarm_id);

    candidates.sort_by(|left, right| {
        let left_carry = affinities
            .dependency_carryover
            .get(&left.session_id)
            .copied()
            .unwrap_or(0);
        let right_carry = affinities
            .dependency_carryover
            .get(&right.session_id)
            .copied()
            .unwrap_or(0);
        let left_meta = affinities
            .metadata_carryover
            .get(&left.session_id)
            .copied()
            .unwrap_or(0);
        let right_meta = affinities
            .metadata_carryover
            .get(&right.session_id)
            .copied()
            .unwrap_or(0);
        let left_load = affinities.loads.get(&left.session_id).copied().unwrap_or(0);
        let right_load = affinities
            .loads
            .get(&right.session_id)
            .copied()
            .unwrap_or(0);
        let left_rank = if left.status == "ready" { 0 } else { 1 };
        let right_rank = if right.status == "ready" { 0 } else { 1 };
        right_carry
            .cmp(&left_carry)
            .then_with(|| right_meta.cmp(&left_meta))
            .then_with(|| left_load.cmp(&right_load))
            .then_with(|| left_rank.cmp(&right_rank))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });

    candidates
        .first()
        .map(|member| member.session_id.clone())
        .ok_or_else(|| {
            "No ready or completed swarm agents are available for automatic task assignment."
                .to_string()
        })
}

#[expect(
    clippy::too_many_arguments,
    reason = "task execution restart needs session state, plan state, and event sinks together"
)]
fn spawn_assigned_task_run(
    agent_arc: Arc<Mutex<Agent>>,
    target_session: String,
    swarm_id: String,
    task_id: String,
    assignment_text: String,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
) {
    let assignment_text = append_swarm_completion_report_instructions(&assignment_text);
    tokio::spawn(async move {
        {
            let now_ms = now_unix_ms();
            let mut plans = swarm_plans.write().await;
            if let Some(plan) = plans.get_mut(&swarm_id)
                && let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id)
            {
                item.status = "running".to_string();
                let progress = plan.task_progress.entry(task_id.clone()).or_default();
                progress.assigned_session_id = Some(target_session.clone());
                progress.assignment_summary = Some(truncate_detail(&assignment_text, 120));
                progress.started_at_unix_ms = Some(now_ms);
                progress.last_heartbeat_unix_ms = Some(now_ms);
                progress.last_detail = Some(truncate_detail(&assignment_text, 120));
                progress.last_checkpoint_unix_ms = Some(now_ms);
                progress.checkpoint_summary = Some("task started".to_string());
                progress.completed_at_unix_ms = None;
                progress.stale_since_unix_ms = None;
                progress.heartbeat_count = Some(progress.heartbeat_count.unwrap_or(0) + 1);
                progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
                plan.version += 1;
            }
        }
        let swarm_state = SwarmState {
            members: Arc::clone(&swarm_members),
            swarms_by_id: Arc::clone(&swarms_by_id),
            plans: Arc::clone(&swarm_plans),
            coordinators: Arc::clone(&swarm_coordinators),
        };
        persist_swarm_state_for(&swarm_id, &swarm_state).await;
        broadcast_swarm_plan(
            &swarm_id,
            Some("task_running".to_string()),
            &swarm_plans,
            &swarm_members,
            &swarms_by_id,
        )
        .await;
        update_member_status(
            &target_session,
            "running",
            Some(truncate_detail(&assignment_text, 120)),
            &swarm_members,
            &swarms_by_id,
            Some(&event_history),
            Some(&event_counter),
            Some(&swarm_event_tx),
        )
        .await;

        let (heartbeat_stop_tx, mut heartbeat_stop_rx) = watch::channel(false);
        let heartbeat_task = {
            let target_session = target_session.clone();
            let swarm_id = swarm_id.clone();
            let task_id = task_id.clone();
            let swarm_members = Arc::clone(&swarm_members);
            let swarms_by_id = Arc::clone(&swarms_by_id);
            let swarm_plans = Arc::clone(&swarm_plans);
            let swarm_coordinators = Arc::clone(&swarm_coordinators);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(swarm_task_heartbeat_interval());
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval.tick().await;
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let revived = touch_swarm_task_progress(
                                &swarm_id,
                                &task_id,
                                Some(&target_session),
                                None,
                                None,
                                &swarm_members,
                                &swarms_by_id,
                                &swarm_plans,
                                &swarm_coordinators,
                            )
                            .await;
                            if revived {
                                broadcast_swarm_plan(
                                    &swarm_id,
                                    Some("task_heartbeat".to_string()),
                                    &swarm_plans,
                                    &swarm_members,
                                    &swarms_by_id,
                                )
                                .await;
                            }
                        }
                        changed = heartbeat_stop_rx.changed() => {
                            if changed.is_err() || *heartbeat_stop_rx.borrow() {
                                break;
                            }
                        }
                    }
                }
            })
        };

        let event_tx = task_progress_event_sender(
            target_session.clone(),
            swarm_id.clone(),
            task_id.clone(),
            Arc::clone(&swarm_members),
            Arc::clone(&swarms_by_id),
            Arc::clone(&swarm_plans),
            Arc::clone(&swarm_coordinators),
            Arc::clone(&event_history),
            Arc::clone(&event_counter),
            swarm_event_tx.clone(),
        );
        let start_message_index = {
            let agent = agent_arc.lock().await;
            agent.message_count()
        };
        let result = super::client_lifecycle::process_message_streaming_mpsc(
            Arc::clone(&agent_arc),
            &assignment_text,
            vec![],
            None,
            event_tx,
        )
        .await;
        let completion_report = if result.is_ok() {
            let agent = agent_arc.lock().await;
            agent.latest_assistant_text_after(start_message_index)
        } else {
            None
        };
        let _ = heartbeat_stop_tx.send(true);
        let _ = heartbeat_task.await;

        match result {
            Ok(_) => {
                let previous_items = {
                    let plans = swarm_plans.read().await;
                    plans
                        .get(&swarm_id)
                        .map(|plan| plan.items.clone())
                        .unwrap_or_default()
                };
                {
                    let now_ms = now_unix_ms();
                    let mut plans = swarm_plans.write().await;
                    if let Some(plan) = plans.get_mut(&swarm_id)
                        && let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id)
                    {
                        item.status = "done".to_string();
                        let progress = plan.task_progress.entry(task_id.clone()).or_default();
                        progress.last_heartbeat_unix_ms = Some(now_ms);
                        progress.last_checkpoint_unix_ms = Some(now_ms);
                        progress.checkpoint_summary = Some("task completed".to_string());
                        progress.completed_at_unix_ms = Some(now_ms);
                        progress.stale_since_unix_ms = None;
                        progress.checkpoint_count =
                            Some(progress.checkpoint_count.unwrap_or(0) + 1);
                        plan.version += 1;
                    }
                }
                let swarm_state = SwarmState {
                    members: Arc::clone(&swarm_members),
                    swarms_by_id: Arc::clone(&swarms_by_id),
                    plans: Arc::clone(&swarm_plans),
                    coordinators: Arc::clone(&swarm_coordinators),
                };
                persist_swarm_state_for(&swarm_id, &swarm_state).await;
                broadcast_swarm_plan_with_previous(
                    &swarm_id,
                    Some("task_completed".to_string()),
                    Some(&previous_items),
                    &swarm_plans,
                    &swarm_members,
                    &swarms_by_id,
                )
                .await;
                update_member_status_with_report(
                    &target_session,
                    "completed",
                    None,
                    completion_report,
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                    Some(&swarm_event_tx),
                )
                .await;
            }
            Err(error) => {
                {
                    let now_ms = now_unix_ms();
                    let mut plans = swarm_plans.write().await;
                    if let Some(plan) = plans.get_mut(&swarm_id)
                        && let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id)
                    {
                        item.status = "failed".to_string();
                        let progress = plan.task_progress.entry(task_id.clone()).or_default();
                        progress.last_heartbeat_unix_ms = Some(now_ms);
                        progress.last_checkpoint_unix_ms = Some(now_ms);
                        progress.checkpoint_summary =
                            Some(truncate_detail(&format!("task failed: {}", error), 120));
                        progress.completed_at_unix_ms = Some(now_ms);
                        progress.stale_since_unix_ms = None;
                        progress.checkpoint_count =
                            Some(progress.checkpoint_count.unwrap_or(0) + 1);
                        plan.version += 1;
                    }
                }
                let swarm_state = SwarmState {
                    members: Arc::clone(&swarm_members),
                    swarms_by_id: Arc::clone(&swarms_by_id),
                    plans: Arc::clone(&swarm_plans),
                    coordinators: Arc::clone(&swarm_coordinators),
                };
                persist_swarm_state_for(&swarm_id, &swarm_state).await;
                broadcast_swarm_plan(
                    &swarm_id,
                    Some("task_failed".to_string()),
                    &swarm_plans,
                    &swarm_members,
                    &swarms_by_id,
                )
                .await;
                update_member_status(
                    &target_session,
                    "failed",
                    Some(truncate_detail(&error.to_string(), 120)),
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                    Some(&swarm_event_tx),
                )
                .await;
            }
        }
    });
}

fn format_salvage_message(
    source_session: &str,
    source_name: Option<&str>,
    summaries: &[crate::protocol::ToolCallSummary],
    extra_message: Option<&str>,
) -> String {
    let label = source_name.unwrap_or(source_session);
    let mut output = format!(
        "Salvage prior progress from {}. Review this before continuing the task.\n\n",
        label
    );
    if summaries.is_empty() {
        output.push_str("No recorded tool call summary was available from the previous assignee.");
    } else {
        output.push_str("Recent prior activity:\n");
        for call in summaries.iter().take(12) {
            let result = if call.brief_output.trim().is_empty() {
                "no result summary"
            } else {
                call.brief_output.as_str()
            };
            output.push_str(&format!(
                "- {}: {}\n",
                call.tool_name,
                truncate_detail(result, 180)
            ));
        }
    }
    if let Some(extra) = extra_message {
        output.push_str("\n\nAdditional coordinator instructions:\n");
        output.push_str(extra);
    }
    output
}

#[expect(
    clippy::too_many_arguments,
    reason = "task progress fanout needs plan state, swarm membership, and event sinks together"
)]
fn task_progress_event_sender(
    session_id: String,
    swarm_id: String,
    task_id: String,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
) -> mpsc::UnboundedSender<ServerEvent> {
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerEvent>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let (detail, checkpoint_summary) = match &event {
                ServerEvent::StatusDetail { detail } => (Some(detail.clone()), None),
                ServerEvent::ToolStart { name, .. } => {
                    let summary = format!("tool start: {name}");
                    (Some(summary.clone()), Some(summary))
                }
                ServerEvent::ToolDone { name, error, .. } => {
                    let summary = if error.is_some() {
                        format!("tool error: {name}")
                    } else {
                        format!("tool done: {name}")
                    };
                    (Some(summary.clone()), Some(summary))
                }
                _ => (None, None),
            };

            if detail.is_some() || checkpoint_summary.is_some() {
                let revived = touch_swarm_task_progress(
                    &swarm_id,
                    &task_id,
                    Some(&session_id),
                    detail.clone(),
                    checkpoint_summary,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_plans,
                    &swarm_coordinators,
                )
                .await;
                if let Some(detail) = detail {
                    update_member_status(
                        &session_id,
                        "running",
                        Some(truncate_detail(&detail, 120)),
                        &swarm_members,
                        &swarms_by_id,
                        Some(&event_history),
                        Some(&event_counter),
                        Some(&swarm_event_tx),
                    )
                    .await;
                }
                if revived {
                    broadcast_swarm_plan(
                        &swarm_id,
                        Some("task_heartbeat".to_string()),
                        &swarm_plans,
                        &swarm_members,
                        &swarms_by_id,
                    )
                    .await;
                }
            }

            let _ = fanout_session_event(&swarm_members, &session_id, event).await;
        }
    });
    tx
}

#[expect(
    clippy::too_many_arguments,
    reason = "role assignment coordinates sessions, swarm membership, coordinators, and event history"
)]
pub(super) async fn handle_comm_assign_role(
    id: u64,
    req_session_id: String,
    target_session: String,
    role: String,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    swarm_mutation_runtime: &SwarmMutationRuntime,
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

    let mutation_key = swarm_mutation_request_key(
        &req_session_id,
        "assign_role",
        &[swarm_id.clone(), target_session.clone(), role.clone()],
    );
    let Some(mutation_state) = begin_swarm_mutation_or_replay(
        swarm_mutation_runtime,
        &mutation_key,
        "assign_role",
        &req_session_id,
        id,
        client_event_tx,
    )
    .await
    else {
        return;
    };

    {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(&target_session) {
            member.role = role.clone();
        } else {
            finish_swarm_mutation_request(
                swarm_mutation_runtime,
                &mutation_state,
                PersistedSwarmMutationResponse::Error {
                    message: format!("Unknown session '{}'", target_session),
                    retry_after_secs: None,
                },
            )
            .await;
            return;
        }
    }

    if role == "coordinator" {
        {
            let mut coordinators = swarm_coordinators.write().await;
            coordinators.insert(swarm_id.clone(), target_session.clone());
        }
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(&req_session_id)
            && member.session_id != target_session
        {
            member.role = "agent".to_string();
        }
    }

    let swarm_state = SwarmState {
        members: Arc::clone(swarm_members),
        swarms_by_id: Arc::clone(swarms_by_id),
        plans: Arc::clone(swarm_plans),
        coordinators: Arc::clone(swarm_coordinators),
    };
    persist_swarm_state_for(&swarm_id, &swarm_state).await;

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
    finish_swarm_mutation_request(
        swarm_mutation_runtime,
        &mutation_state,
        PersistedSwarmMutationResponse::Done,
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "task assignment coordinates sessions, interrupts, connections, swarm plan state, and event history"
)]
pub(super) async fn handle_comm_assign_task(
    id: u64,
    req_session_id: String,
    target_session: Option<String>,
    task_id: Option<String>,
    message: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    soft_interrupt_queues: &super::SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    swarm_mutation_runtime: &SwarmMutationRuntime,
) {
    let requested_target_session = target_session.and_then(|target| {
        let trimmed = target.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });
    let requested_task_id = task_id.and_then(|task_id| {
        let trimmed = task_id.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    });

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

    let mutation_key = swarm_mutation_request_key(
        &req_session_id,
        "assign_task",
        &[
            swarm_id.clone(),
            requested_target_session
                .clone()
                .unwrap_or_else(|| "__next_available__".to_string()),
            requested_task_id
                .clone()
                .unwrap_or_else(|| "__next_runnable__".to_string()),
            message.clone().unwrap_or_default(),
        ],
    );
    let Some(mutation_state) = begin_swarm_mutation_or_replay(
        swarm_mutation_runtime,
        &mutation_key,
        "assign_task",
        &req_session_id,
        id,
        client_event_tx,
    )
    .await
    else {
        return;
    };

    let target_session = match resolve_assignment_target_session(
        &req_session_id,
        &swarm_id,
        requested_target_session.as_deref(),
        swarm_members,
        swarm_plans,
    )
    .await
    {
        Ok(target_session) => target_session,
        Err(message) => {
            finish_swarm_mutation_request(
                swarm_mutation_runtime,
                &mutation_state,
                PersistedSwarmMutationResponse::Error {
                    message,
                    retry_after_secs: None,
                },
            )
            .await;
            return;
        }
    };

    let (selected_task_id, task_content, participant_ids, plan_item_count, blocked_reason) = {
        let now_ms = now_unix_ms();
        let mut plans = swarm_plans.write().await;
        let plan = plans
            .entry(swarm_id.clone())
            .or_insert_with(VersionedPlan::new);
        let selected_task_id = requested_task_id
            .clone()
            .or_else(|| next_unassigned_runnable_item_id(plan));
        let blocked_reason = requested_task_id
            .as_deref()
            .and_then(|task_id| explicit_task_blocked_reason(plan, task_id));
        let found = if blocked_reason.is_some() {
            None
        } else {
            selected_task_id.as_ref().and_then(|selected_task_id| {
                plan.items
                    .iter_mut()
                    .find(|item| item.id == *selected_task_id)
            })
        };
        if let Some(item) = found {
            let content = item.content.clone();
            item.assigned_to = Some(target_session.clone());
            item.status = "queued".to_string();
            plan.task_progress.insert(
                item.id.clone(),
                SwarmTaskProgress {
                    assigned_session_id: Some(target_session.clone()),
                    assignment_summary: Some(truncate_detail(
                        &combine_assignment_text(&content, message.as_deref()),
                        120,
                    )),
                    assigned_at_unix_ms: Some(now_ms),
                    ..SwarmTaskProgress::default()
                },
            );
            plan.version += 1;
            plan.participants.insert(req_session_id.clone());
            plan.participants.insert(target_session.clone());
            (
                Some(item.id.clone()),
                Some(content),
                plan.participants.clone(),
                plan.items.len(),
                None,
            )
        } else {
            (None, None, HashSet::new(), 0, blocked_reason)
        }
    };

    let Some(selected_task_id) = selected_task_id else {
        let message = blocked_reason.unwrap_or_else(|| {
            requested_task_id.as_ref().map_or_else(
                || "No runnable unassigned tasks are available in the swarm plan".to_string(),
                |task_id| format!("Task '{}' not found in swarm plan", task_id),
            )
        });
        finish_swarm_mutation_request(
            swarm_mutation_runtime,
            &mutation_state,
            PersistedSwarmMutationResponse::Error {
                message,
                retry_after_secs: None,
            },
        )
        .await;
        return;
    };
    let Some(content) = task_content else {
        finish_swarm_mutation_request(
            swarm_mutation_runtime,
            &mutation_state,
            PersistedSwarmMutationResponse::Error {
                message: format!(
                    "Task '{}' could not be assigned because its content was unavailable.",
                    selected_task_id
                ),
                retry_after_secs: None,
            },
        )
        .await;
        return;
    };

    let swarm_state = SwarmState {
        members: Arc::clone(swarm_members),
        swarms_by_id: Arc::clone(swarms_by_id),
        plans: Arc::clone(swarm_plans),
        coordinators: Arc::clone(swarm_coordinators),
    };
    persist_swarm_state_for(&swarm_id, &swarm_state).await;

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
    let queued_task_prompt = append_swarm_completion_report_instructions(&notification);

    let target_agent = {
        let agent_sessions = sessions.read().await;
        agent_sessions.get(&target_session).cloned()
    };
    let _ = queue_soft_interrupt_for_session(
        &target_session,
        queued_task_prompt,
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
    if !target_has_client && let Some(agent_arc) = target_agent {
        let target_session_for_run = target_session.clone();
        let swarm_members_for_run = Arc::clone(swarm_members);
        let swarms_for_run = Arc::clone(swarms_by_id);
        let swarm_plans_for_run = Arc::clone(swarm_plans);
        let swarm_coordinators_for_run = Arc::clone(swarm_coordinators);
        let swarm_id_for_run = swarm_id.clone();
        let task_id_for_run = selected_task_id.clone();
        let event_history_for_run = Arc::clone(event_history);
        let event_counter_for_run = Arc::clone(event_counter);
        let swarm_event_tx_for_run = swarm_event_tx.clone();
        let assignment_text = combine_assignment_text(&content, message.as_deref());
        spawn_assigned_task_run(
            agent_arc,
            target_session_for_run,
            swarm_id_for_run,
            task_id_for_run,
            assignment_text,
            swarm_members_for_run,
            swarms_for_run,
            swarm_plans_for_run,
            swarm_coordinators_for_run,
            event_history_for_run,
            event_counter_for_run,
            swarm_event_tx_for_run,
        );
    }

    let plan_msg = format!(
        "Plan updated: task '{}' assigned to {}.",
        selected_task_id, target_session
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

    finish_swarm_mutation_request(
        swarm_mutation_runtime,
        &mutation_state,
        PersistedSwarmMutationResponse::AssignTask {
            task_id: selected_task_id,
            target_session,
        },
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "assign_next reuses task assignment orchestration and forwards the same runtime dependencies"
)]
pub(super) async fn handle_comm_assign_next(
    id: u64,
    req_session_id: String,
    target_session: Option<String>,
    working_dir: Option<String>,
    prefer_spawn: Option<bool>,
    spawn_if_needed: Option<bool>,
    message: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn crate::provider::Provider>,
    soft_interrupt_queues: &super::SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    mcp_pool: &Arc<crate::mcp::SharedMcpPool>,
    swarm_mutation_runtime: &SwarmMutationRuntime,
) {
    if target_session.is_none() {
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

        let Some(selected_task_id) = next_unassigned_runnable_task_id(&swarm_id, swarm_plans).await
        else {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: "No runnable unassigned tasks are available in the swarm plan".to_string(),
                retry_after_secs: None,
            });
            return;
        };

        let preferred_target = resolve_assignment_target_for_task(
            &req_session_id,
            &swarm_id,
            &selected_task_id,
            None,
            swarm_members,
            swarm_plans,
        )
        .await;

        if (prefer_spawn.unwrap_or(false) || spawn_if_needed.unwrap_or(false))
            && (prefer_spawn.unwrap_or(false) || preferred_target.is_err())
        {
            match super::comm_session::spawn_swarm_agent(
                &req_session_id,
                &swarm_id,
                working_dir.clone(),
                None,
                None,
                sessions,
                global_session_id,
                provider_template,
                swarm_members,
                swarms_by_id,
                swarm_coordinators,
                swarm_plans,
                event_history,
                event_counter,
                swarm_event_tx,
                mcp_pool,
                soft_interrupt_queues,
            )
            .await
            {
                Ok(spawned_session) => {
                    handle_comm_assign_task(
                        id,
                        req_session_id,
                        Some(spawned_session),
                        Some(selected_task_id),
                        message,
                        client_event_tx,
                        sessions,
                        soft_interrupt_queues,
                        client_connections,
                        swarm_members,
                        swarms_by_id,
                        swarm_plans,
                        swarm_coordinators,
                        event_history,
                        event_counter,
                        swarm_event_tx,
                        swarm_mutation_runtime,
                    )
                    .await;
                    return;
                }
                Err(error) => {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!("Failed to spawn preferred worker: {error}"),
                        retry_after_secs: None,
                    });
                    return;
                }
            }
        }

        match preferred_target {
            Ok(target_session) => {
                handle_comm_assign_task(
                    id,
                    req_session_id,
                    Some(target_session),
                    Some(selected_task_id),
                    message,
                    client_event_tx,
                    sessions,
                    soft_interrupt_queues,
                    client_connections,
                    swarm_members,
                    swarms_by_id,
                    swarm_plans,
                    swarm_coordinators,
                    event_history,
                    event_counter,
                    swarm_event_tx,
                    swarm_mutation_runtime,
                )
                .await;
            }
            Err(message) => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message,
                    retry_after_secs: None,
                });
            }
        }
        return;
    }

    handle_comm_assign_task(
        id,
        req_session_id,
        target_session,
        None,
        message,
        client_event_tx,
        sessions,
        soft_interrupt_queues,
        client_connections,
        swarm_members,
        swarms_by_id,
        swarm_plans,
        swarm_coordinators,
        event_history,
        event_counter,
        swarm_event_tx,
        swarm_mutation_runtime,
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "task control checks assignment state, delivery, and safe recovery paths together"
)]
pub(super) async fn handle_comm_task_control(
    id: u64,
    req_session_id: String,
    action: String,
    task_id: String,
    target_session: Option<String>,
    message: Option<String>,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    sessions: &SessionAgents,
    soft_interrupt_queues: &super::SessionInterruptQueues,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    swarm_mutation_runtime: &SwarmMutationRuntime,
) {
    let Some(action) = TaskControlAction::parse(&action) else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Unknown task control action. Use start, wake, resume, retry, reassign, replace, or salvage.".to_string(),
            retry_after_secs: None,
        });
        return;
    };

    let swarm_id = match require_coordinator_swarm(
        id,
        &req_session_id,
        "Only the coordinator can control assigned tasks.",
        client_event_tx,
        swarm_members,
        swarm_coordinators,
    )
    .await
    {
        Some(swarm_id) => swarm_id,
        None => return,
    };

    let task_id = if task_id.trim().is_empty() {
        let Some(target_session) = target_session.as_deref() else {
            let _ = client_event_tx.send(ServerEvent::Error {
                id,
                message: format!(
                    "task_id is required for {} unless target_session uniquely identifies an assigned task.",
                    action.as_str()
                ),
                retry_after_secs: None,
            });
            return;
        };
        match task_id_for_target_session(&swarm_id, target_session, action, swarm_plans).await {
            Ok(task_id) => task_id,
            Err(message) => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message,
                    retry_after_secs: None,
                });
                return;
            }
        }
    } else {
        task_id
    };

    let Some(snapshot) = task_snapshot_for(&swarm_id, &task_id, swarm_plans).await else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!("Task '{}' not found in swarm plan", task_id),
            retry_after_secs: None,
        });
        return;
    };

    if !task_control_action_allows_status(action, &snapshot.status) {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: task_control_status_error(action, &snapshot.status, &task_id),
            retry_after_secs: None,
        });
        return;
    }

    let current_assignee = snapshot.assigned_to.clone();
    let require_assignee = matches!(
        action,
        TaskControlAction::Start
            | TaskControlAction::Wake
            | TaskControlAction::Resume
            | TaskControlAction::Retry
            | TaskControlAction::Replace
            | TaskControlAction::Salvage
            | TaskControlAction::Reassign
    );
    if require_assignee && current_assignee.is_none() {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: format!(
                "Task '{}' is not currently assigned. Use assign_task to create the first assignment.",
                task_id
            ),
            retry_after_secs: None,
        });
        return;
    }

    match action {
        TaskControlAction::Start | TaskControlAction::Wake | TaskControlAction::Resume => {
            let Some(assignee) = current_assignee.clone() else {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Task '{}' no longer has an assignee. Use assign_task to create the first assignment.",
                        task_id
                    ),
                    retry_after_secs: None,
                });
                return;
            };
            if let Some(ref requested_target) = target_session
                && requested_target != &assignee
            {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Task '{}' is assigned to '{}', not '{}'. Use reassign or replace to change ownership.",
                        task_id, assignee, requested_target
                    ),
                    retry_after_secs: None,
                });
                return;
            }

            let assignment_text =
                build_control_assignment_text(action, &snapshot.content, message.as_deref());
            if snapshot.status != "queued"
                && requeue_existing_assignment(
                    &swarm_id,
                    &req_session_id,
                    &assignee,
                    &task_id,
                    assignment_text.clone(),
                    swarm_plans,
                )
                .await
                .is_some()
            {
                let swarm_state = SwarmState {
                    members: Arc::clone(swarm_members),
                    swarms_by_id: Arc::clone(swarms_by_id),
                    plans: Arc::clone(swarm_plans),
                    coordinators: Arc::clone(swarm_coordinators),
                };
                persist_swarm_state_for(&swarm_id, &swarm_state).await;
                broadcast_swarm_plan(
                    &swarm_id,
                    Some(format!("task_{}", action.as_str())),
                    swarm_plans,
                    swarm_members,
                    swarms_by_id,
                )
                .await;
            }

            let Some(agent_arc) = task_agent_session(&assignee, sessions).await else {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Assigned session '{}' is not available. Use replace or salvage to move the task to another agent.",
                        assignee
                    ),
                    retry_after_secs: None,
                });
                return;
            };
            let Some(_member) = active_swarm_member(&assignee, swarm_members).await else {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Assigned session '{}' is no longer in the swarm. Use replace or salvage to move the task.",
                        assignee
                    ),
                    retry_after_secs: None,
                });
                return;
            };

            let agent_is_idle = match agent_arc.try_lock() {
                Ok(guard) => {
                    drop(guard);
                    true
                }
                Err(_) => false,
            };

            if agent_is_idle {
                spawn_assigned_task_run(
                    agent_arc,
                    assignee.clone(),
                    swarm_id.clone(),
                    task_id.clone(),
                    assignment_text,
                    Arc::clone(swarm_members),
                    Arc::clone(swarms_by_id),
                    Arc::clone(swarm_plans),
                    Arc::clone(swarm_coordinators),
                    Arc::clone(event_history),
                    Arc::clone(event_counter),
                    swarm_event_tx.clone(),
                );
                let summary = plan_graph_status_for(&swarm_id, swarm_plans).await;
                let _ = client_event_tx.send(ServerEvent::CommTaskControlResponse {
                    id,
                    action: action.as_str().to_string(),
                    task_id: task_id.clone(),
                    target_session: Some(assignee.clone()),
                    status: "running".to_string(),
                    summary,
                });
                return;
            }

            if action == TaskControlAction::Wake {
                let assignment_text = append_swarm_completion_report_instructions(&assignment_text);
                let wake_message = format!(
                    "Coordinator requested you wake and continue task '{}'.\n\n{}",
                    task_id, assignment_text
                );
                let _ = queue_soft_interrupt_for_session(
                    &assignee,
                    wake_message,
                    false,
                    SoftInterruptSource::System,
                    soft_interrupt_queues,
                    sessions,
                )
                .await;
                let summary = plan_graph_status_for(&swarm_id, swarm_plans).await;
                let _ = client_event_tx.send(ServerEvent::CommTaskControlResponse {
                    id,
                    action: action.as_str().to_string(),
                    task_id: task_id.clone(),
                    target_session: Some(assignee.clone()),
                    status: "queued".to_string(),
                    summary,
                });
            } else {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Assigned session '{}' is currently busy. Use wake to queue the task, or retry once the agent is idle.",
                        assignee
                    ),
                    retry_after_secs: Some(1),
                });
            }
        }
        TaskControlAction::Retry => {
            let Some(assignee) = current_assignee.clone() else {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Task '{}' no longer has an assignee. Use assign_task to create the first assignment.",
                        task_id
                    ),
                    retry_after_secs: None,
                });
                return;
            };
            let retry_note = message.as_ref().map_or_else(
                || "Retry this assignment.".to_string(),
                |extra| {
                    format!(
                        "Retry this assignment.\n\nAdditional coordinator instructions:\n{}",
                        extra
                    )
                },
            );
            handle_comm_assign_task(
                id,
                req_session_id,
                Some(assignee),
                Some(task_id),
                Some(retry_note),
                client_event_tx,
                sessions,
                soft_interrupt_queues,
                client_connections,
                swarm_members,
                swarms_by_id,
                swarm_plans,
                swarm_coordinators,
                event_history,
                event_counter,
                swarm_event_tx,
                swarm_mutation_runtime,
            )
            .await;
        }
        TaskControlAction::Reassign | TaskControlAction::Replace | TaskControlAction::Salvage => {
            let Some(assignee) = current_assignee.clone() else {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Task '{}' no longer has an assignee. Use assign_task to create the first assignment.",
                        task_id
                    ),
                    retry_after_secs: None,
                });
                return;
            };
            let Some(new_target) = target_session else {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!("'target_session' is required for {}.", action.as_str()),
                    retry_after_secs: None,
                });
                return;
            };

            if new_target == assignee {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!("Task '{}' is already assigned to '{}'.", task_id, assignee),
                    retry_after_secs: None,
                });
                return;
            }

            if snapshot.status == "running" {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Task '{}' is actively running on '{}'. Wait, wake, or stop that agent before handing the task off.",
                        task_id, assignee
                    ),
                    retry_after_secs: Some(1),
                });
                return;
            }

            if action == TaskControlAction::Replace
                && !matches!(
                    snapshot.status.as_str(),
                    "queued" | "failed" | "stopped" | "crashed" | "running_stale"
                )
            {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: format!(
                        "Task '{}' is '{}' and cannot be safely replaced.",
                        task_id, snapshot.status
                    ),
                    retry_after_secs: None,
                });
                return;
            }

            let forwarded_message = if action == TaskControlAction::Salvage {
                let prior_name = active_swarm_member(&assignee, swarm_members)
                    .await
                    .and_then(|member| member.friendly_name);
                let summaries =
                    if let Some(agent_arc) = task_agent_session(&assignee, sessions).await {
                        if let Ok(agent) = agent_arc.try_lock() {
                            agent.get_tool_call_summaries(12)
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    };
                let mut salvage = format_salvage_message(
                    &assignee,
                    prior_name.as_deref(),
                    &summaries,
                    message.as_deref(),
                );
                if let Some(progress) = snapshot.progress.as_ref() {
                    if let Some(summary) = progress.checkpoint_summary.as_deref() {
                        salvage.push_str("\n\nLatest checkpoint summary:\n");
                        salvage.push_str(summary);
                    }
                    if let Some(detail) = progress.last_detail.as_deref() {
                        salvage.push_str("\n\nLatest recorded detail:\n");
                        salvage.push_str(detail);
                    }
                }
                Some(salvage)
            } else if action == TaskControlAction::Replace {
                Some(message.as_ref().map_or_else(
                    || format!("This task is replacing prior assignee '{}'.", assignee),
                    |extra| format!(
                        "This task is replacing prior assignee '{}'.\n\nAdditional coordinator instructions:\n{}",
                        assignee, extra
                    ),
                ))
            } else {
                message
            };

            handle_comm_assign_task(
                id,
                req_session_id,
                Some(new_target),
                Some(task_id),
                forwarded_message,
                client_event_tx,
                sessions,
                soft_interrupt_queues,
                client_connections,
                swarm_members,
                swarms_by_id,
                swarm_plans,
                swarm_coordinators,
                event_history,
                event_counter,
                swarm_event_tx,
                swarm_mutation_runtime,
            )
            .await;
        }
    }
}

#[cfg(test)]
#[path = "comm_control_tests.rs"]
mod tests;

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
