use super::state::{MAX_EVENT_HISTORY, fanout_session_event};
use super::{SwarmEvent, SwarmEventType, SwarmMember, SwarmState, VersionedPlan};
use super::{persist_swarm_state_for, remove_persisted_swarm_state_for};
use crate::agent::Agent;
use crate::plan::{PlanItem, newly_ready_item_ids};
use crate::protocol::{NotificationType, ServerEvent};
use crate::session::Session;
use anyhow::Result;
use futures::future::try_join_all;
use jcode_swarm_core::{
    completion_notification_message, normalize_completion_report, truncate_detail,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, RwLock, broadcast};

fn status_age_secs(last_status_change: Instant) -> u64 {
    last_status_change.elapsed().as_secs()
}

/// Maximum number of live members (agents) in a single swarm. Re-exported from
/// `jcode_swarm_core` so the server, tools, and prompts all agree on the one
/// runaway-prevention cap. Swarms are root-only, one-level fan-out, and both
/// the configurable live-worker budget and this absolute cap apply.
pub(super) use jcode_swarm_core::MAX_SWARM_MEMBERS;

/// Walk the `report_back_to_session_id` chain upward from `session_id`,
/// returning the list of ancestor session ids (parent first, root last).
///
/// The spawner/parent edge is encoded by `report_back_to_session_id`: a child
/// spawned by `P` reports back to `P`. Walking that chain reconstructs the spawn
/// tree without persisting a separate parent field. Cycles (which should never
/// happen) are guarded against with a visited set.
pub(super) fn swarm_ancestors(
    members: &HashMap<String, SwarmMember>,
    session_id: &str,
) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(session_id.to_string());
    let mut current = session_id.to_string();
    while let Some(parent) = members
        .get(&current)
        .and_then(|member| member.report_back_to_session_id.clone())
    {
        if parent == current || !visited.insert(parent.clone()) {
            break;
        }
        ancestors.push(parent.clone());
        current = parent;
    }
    ancestors
}

/// Depth of `session_id` in the spawn tree: number of ancestors reachable via
/// the report-back chain. Root coordinators (no report-back owner) are depth 0.
///
/// Test-only: the spawn tree no longer enforces a depth cap, so production code
/// does not consult depth. Kept (behind `cfg(test)`) because the spawn-tree tests
/// assert ancestor-chain depth directly.
#[cfg(test)]
pub(super) fn swarm_spawn_depth(members: &HashMap<String, SwarmMember>, session_id: &str) -> u32 {
    swarm_ancestors(members, session_id).len() as u32
}

/// True when `ancestor` is `session_id` itself or any transitive spawner of it.
/// Used to decide whether a requester may manage (stop/control) a target: an
/// agent owns its entire spawned subtree.
pub(super) fn swarm_is_self_or_ancestor(
    members: &HashMap<String, SwarmMember>,
    ancestor: &str,
    session_id: &str,
) -> bool {
    ancestor == session_id
        || swarm_ancestors(members, session_id)
            .iter()
            .any(|candidate| candidate == ancestor)
}

/// Members spawned by `session_id`, directly or transitively.
pub(super) fn swarm_descendants(
    members: &HashMap<String, SwarmMember>,
    session_id: &str,
) -> Vec<String> {
    let mut descendants: Vec<String> = members
        .keys()
        .filter(|candidate| {
            candidate.as_str() != session_id
                && swarm_ancestors(members, candidate)
                    .iter()
                    .any(|ancestor| ancestor == session_id)
        })
        .cloned()
        .collect();
    descendants.sort();
    descendants
}

/// Stop the running turns of every worker `session_id` spawned (workers run
/// their own server-side turns, so cancelling only the coordinator left them
/// spending tokens). Idle workers are left alone. Returns the stopped ids.
pub(super) async fn cancel_spawned_descendant_turns(
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: Option<&Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>>,
    event_counter: Option<&Arc<std::sync::atomic::AtomicU64>>,
    swarm_event_tx: Option<&broadcast::Sender<SwarmEvent>>,
) -> Vec<String> {
    let descendants = swarm_descendants(&*swarm_members.read().await, session_id);
    let mut stopped = Vec::new();
    for descendant in descendants {
        let signals = crate::turn_cancel_registry::active_turn_signals(&descendant);
        if signals.is_empty() {
            continue;
        }
        for signal in &signals {
            signal.fire();
        }
        // Workers usually have no attached client; the status update below
        // is what the coordinator's swarm view shows.
        fanout_session_event(
            swarm_members,
            &descendant,
            ServerEvent::TurnStopped {
                reason: crate::protocol::TurnStopReason::Interrupted,
                message: format!("Stopped because its coordinator {session_id} was interrupted."),
                provider_stop_reason: None,
            },
        )
        .await;
        update_member_status(
            &descendant,
            "stopped",
            Some("coordinator interrupted".to_string()),
            swarm_members,
            swarms_by_id,
            event_history,
            event_counter,
            swarm_event_tx,
        )
        .await;
        stopped.push(descendant);
    }
    if !stopped.is_empty() {
        let names = stopped.join(",");
        crate::logging::info(&format!(
            "SWARM_CASCADE_CANCEL coordinator={session_id} stopped={names}"
        ));
    }
    stopped
}

#[cfg(test)]
#[path = "swarm_cascade_tests.rs"]
mod swarm_cascade_tests;

const DEFAULT_SWARM_STATUS_DEBOUNCE_MEMBER_THRESHOLD: usize = 2;
const DEFAULT_SWARM_STATUS_DEBOUNCE_MS: u64 = 75;
const DEFAULT_SWARM_TASK_HEARTBEAT_SECS: u64 = 10;
const DEFAULT_SWARM_TASK_STALE_AFTER_SECS: u64 = 45;
const DEFAULT_SWARM_TASK_SWEEP_INTERVAL_SECS: u64 = 5;
const DEFAULT_SWARM_TERMINAL_MEMBER_RETENTION_SECS: u64 = 24 * 60 * 60;
const DEFAULT_SWARM_TERMINAL_MEMBER_GC_INTERVAL_SECS: u64 = 60;
/// How long terminal members stay in live SwarmStatus broadcasts. Terminal
/// members remain queryable for the full retention window above, but
/// re-sending hundreds of long-finished members to every attached client on
/// every status change dominates broadcast payloads (measured ~240 KB of
/// member JSON resident per client with ~700 mostly-stopped members). Keep
/// them in broadcasts briefly so done/failed transition notices still fire,
/// then drop them from the live fan-out.
const DEFAULT_SWARM_STATUS_BROADCAST_TERMINAL_SECS: u64 = 15 * 60;
#[derive(Default, Clone, Copy)]
struct PendingSwarmStatusBroadcast {
    scheduled: bool,
    dirty: bool,
}

fn pending_swarm_status_broadcasts()
-> &'static StdMutex<HashMap<String, PendingSwarmStatusBroadcast>> {
    static PENDING: OnceLock<StdMutex<HashMap<String, PendingSwarmStatusBroadcast>>> =
        OnceLock::new();
    PENDING.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn swarm_status_debounce_member_threshold() -> usize {
    static CACHED: OnceLock<AtomicUsize> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let configured = std::env::var("JCODE_SWARM_STATUS_DEBOUNCE_MEMBER_THRESHOLD")
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_SWARM_STATUS_DEBOUNCE_MEMBER_THRESHOLD);
            AtomicUsize::new(configured)
        })
        .load(Ordering::Relaxed)
}

fn swarm_status_debounce_ms() -> u64 {
    static CACHED: OnceLock<AtomicU64> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let configured = std::env::var("JCODE_SWARM_STATUS_DEBOUNCE_MS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_SWARM_STATUS_DEBOUNCE_MS);
            AtomicU64::new(configured)
        })
        .load(Ordering::Relaxed)
}

fn configured_positive_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub(super) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn log_swarm_lifecycle(phase: &str, fields: Vec<(&str, String)>) {
    crate::logging::event_info(
        "SWARM_LIFECYCLE",
        Vec::from([("phase", phase.to_string())])
            .into_iter()
            .chain(fields)
            .collect::<Vec<_>>(),
    );
}

pub(super) fn swarm_task_heartbeat_interval() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_TASK_HEARTBEAT_SECS",
        DEFAULT_SWARM_TASK_HEARTBEAT_SECS,
    ))
}

pub(super) fn swarm_task_stale_after() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_TASK_STALE_AFTER_SECS",
        DEFAULT_SWARM_TASK_STALE_AFTER_SECS,
    ))
}

pub(super) fn swarm_task_sweep_interval() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_TASK_SWEEP_INTERVAL_SECS",
        DEFAULT_SWARM_TASK_SWEEP_INTERVAL_SECS,
    ))
}

/// How long terminal members remain visible in the active swarm listing. This
/// keeps completion reports available for inspection without allowing durable
/// history to grow forever.
pub(super) fn swarm_terminal_member_retention() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_TERMINAL_MEMBER_RETENTION_SECS",
        DEFAULT_SWARM_TERMINAL_MEMBER_RETENTION_SECS,
    ))
}

/// How often the live server removes terminal members whose retention window
/// has elapsed. Startup loading performs the same pruning synchronously.
pub(super) fn swarm_terminal_member_gc_interval() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_TERMINAL_MEMBER_GC_INTERVAL_SECS",
        DEFAULT_SWARM_TERMINAL_MEMBER_GC_INTERVAL_SECS,
    ))
}

/// How long terminal members remain included in live SwarmStatus broadcasts.
/// See [`DEFAULT_SWARM_STATUS_BROADCAST_TERMINAL_SECS`].
pub(super) fn swarm_status_broadcast_terminal_retention() -> Duration {
    Duration::from_secs(configured_positive_u64(
        "JCODE_SWARM_STATUS_BROADCAST_TERMINAL_SECS",
        DEFAULT_SWARM_STATUS_BROADCAST_TERMINAL_SECS,
    ))
}

/// Whether a member belongs in live SwarmStatus broadcasts: every live member,
/// plus terminal members whose status changed recently enough that clients may
/// still want to announce or display the transition.
pub(super) fn member_in_status_broadcast(member: &SwarmMember, retention: Duration) -> bool {
    !member_status_is_terminal(&member.status) || member.last_status_change.elapsed() < retention
}

/// Terminal members are historical records, not live agents. They remain
/// visible temporarily for reports and diagnostics but must not consume the
/// runaway-prevention spawn budget.
pub(super) fn member_status_is_terminal(status: &str) -> bool {
    matches!(
        status,
        "completed" | "done" | "failed" | "stopped" | "crashed" | "closed" | "disconnected"
    )
}

pub(super) fn member_consumes_swarm_capacity(member: &SwarmMember) -> bool {
    !member_status_is_terminal(&member.status)
}

pub(super) fn expired_terminal_member_ids(
    members: &HashMap<String, SwarmMember>,
    retention: Duration,
) -> Vec<String> {
    members
        .values()
        .filter(|member| member_status_is_terminal(&member.status))
        .filter(|member| member.last_status_change.elapsed() >= retention)
        .map(|member| member.session_id.clone())
        .collect()
}

/// Lifecycle statuses that mean a member can no longer drive an assignment:
/// the session's agent loop is gone, so no heartbeat or turn end will ever
/// arrive for tasks it holds.
pub(super) fn member_status_is_dead(status: &str) -> bool {
    matches!(status, "failed" | "stopped" | "crashed")
}

/// How long a finished spawned worker may sit idle before the server reaps it
/// (closes its client and removes the member). `0` disables reaping.
///
/// Spawned workers (visible windows and headless sessions) rely on their
/// coordinator calling `cleanup`, but ad hoc spawns and interrupted plans
/// leave them behind, where each idle client holds ~80-150 MB indefinitely.
/// The reaper is the backstop that keeps them from stacking up.
const DEFAULT_SWARM_IDLE_WORKER_REAP_SECS: u64 = 30 * 60;

pub(super) fn swarm_idle_worker_reap_after() -> Option<Duration> {
    let secs = std::env::var("JCODE_SWARM_IDLE_WORKER_REAP_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SWARM_IDLE_WORKER_REAP_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Spawned workers whose work is finished (`ready` report-back or a terminal
/// status) and whose status has not changed for at least `idle_after`.
/// Only sessions spawned by another agent (`report_back_to_session_id` set)
/// and not holding the coordinator role are eligible; user-created sessions
/// are never reaped.
pub(super) fn idle_spawned_worker_reap_candidates(
    members: &HashMap<String, SwarmMember>,
    idle_after: Duration,
) -> Vec<String> {
    members
        .values()
        .filter(|member| member.report_back_to_session_id.is_some())
        .filter(|member| member.role != "coordinator")
        .filter(|member| member.status == "ready" || member_status_is_terminal(&member.status))
        .filter(|member| member.last_status_change.elapsed() >= idle_after)
        .map(|member| member.session_id.clone())
        .collect()
}

/// Outcome of salvaging one dead member's plan assignments.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct DeadMemberSalvage {
    /// Tasks released back to `queued` for automatic re-dispatch.
    pub requeued_task_ids: Vec<String>,
    /// Tasks marked `failed` because the automatic reclaim cap was reached.
    pub failed_task_ids: Vec<String>,
}

impl DeadMemberSalvage {
    pub(super) fn is_empty(&self) -> bool {
        self.requeued_task_ids.is_empty() && self.failed_task_ids.is_empty()
    }

    /// Human-readable notification body for the coordinator/owner.
    fn describe(&self, worker_label: &str) -> String {
        let mut parts = vec![format!(
            "⚠ Worker {} died while holding swarm task assignment(s).",
            worker_label
        )];
        if !self.requeued_task_ids.is_empty() {
            parts.push(format!(
                "Requeued for automatic re-dispatch: {}.",
                self.requeued_task_ids.join(", ")
            ));
        }
        if !self.failed_task_ids.is_empty() {
            parts.push(format!(
                "Marked failed (automatic reclaim cap reached): {}. Use retry or assign_task to redispatch explicitly.",
                self.failed_task_ids.join(", ")
            ));
        }
        parts.push(
            "Queued tasks will be picked up by assign_next/run_plan; check plan_status for details."
                .to_string(),
        );
        parts.join(" ")
    }
}

/// Requeue (or, past [`crate::plan::MAX_DEAD_ASSIGNEE_RECLAIMS`], fail) every
/// non-terminal plan item assigned to `session_id`.
///
/// This is the eager counterpart to the assign-time stranded-task reclaim: a
/// worker that crashes, stops, or leaves the swarm mid-task leaves its items
/// `running`/`queued` and assigned to a corpse, where the scheduler cannot see
/// them and a driving `run_plan` stalls into its transient-stall error.
/// Salvaging at the moment the member dies converts that silent strand into
/// normal queued work. Uses the same per-node reclaim counter and cap as the
/// assign-time path so repeatedly lethal nodes fail loudly instead of cycling
/// workers forever.
fn salvage_plan_assignments_of(plan: &mut VersionedPlan, session_id: &str) -> DeadMemberSalvage {
    let now_ms = now_unix_ms();
    let mut outcome = DeadMemberSalvage::default();
    let assigned_ids: Vec<String> = plan
        .items
        .iter()
        .filter(|item| {
            item.assigned_to.as_deref() == Some(session_id)
                && !crate::plan::is_terminal_status(&item.status)
        })
        .map(|item| item.id.clone())
        .collect();
    for task_id in assigned_ids {
        let reclaims = plan
            .task_progress
            .get(&task_id)
            .and_then(|progress| progress.dead_assignee_reclaims)
            .unwrap_or(0);
        if reclaims >= crate::plan::MAX_DEAD_ASSIGNEE_RECLAIMS {
            if let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id) {
                item.status = "failed".to_string();
                item.assigned_to = None;
            }
            let progress = plan.task_progress.entry(task_id.clone()).or_default();
            progress.assigned_session_id = None;
            progress.completed_at_unix_ms = Some(now_ms);
            progress.stale_since_unix_ms = None;
            progress.checkpoint_summary = Some(truncate_detail(
                &format!(
                    "failed: assigned worker {} died and the automatic reclaim cap was reached",
                    session_id
                ),
                120,
            ));
            plan.version += 1;
            outcome.failed_task_ids.push(task_id);
        } else if crate::plan::reclaim_stranded_assignment(plan, &task_id) {
            if let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id) {
                item.status = "queued".to_string();
            }
            let progress = plan.task_progress.entry(task_id.clone()).or_default();
            progress.stale_since_unix_ms = None;
            outcome.requeued_task_ids.push(task_id);
        }
    }
    outcome
}

/// Salvage `session_id`'s plan assignments in `swarm_id`, then persist,
/// broadcast the plan change, and notify the swarm coordinator so the death is
/// visible instead of silent. No-ops (and skips all I/O) when the member held
/// no non-terminal assignments.
pub(super) async fn salvage_assignments_of_dead_member(
    session_id: &str,
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) -> DeadMemberSalvage {
    let outcome = {
        let mut plans = swarm_plans.write().await;
        match plans.get_mut(swarm_id) {
            Some(plan) => salvage_plan_assignments_of(plan, session_id),
            None => DeadMemberSalvage::default(),
        }
    };
    if outcome.is_empty() {
        return outcome;
    }

    log_swarm_lifecycle(
        "dead_member_tasks_salvaged",
        vec![
            ("session_id", session_id.to_string()),
            ("swarm_id", swarm_id.to_string()),
            ("requeued_task_ids", outcome.requeued_task_ids.join(",")),
            ("failed_task_ids", outcome.failed_task_ids.join(",")),
        ],
    );

    let swarm_state = SwarmState {
        members: Arc::clone(swarm_members),
        swarms_by_id: Arc::clone(swarms_by_id),
        plans: Arc::clone(swarm_plans),
        coordinators: Arc::clone(swarm_coordinators),
    };
    persist_swarm_state_for(swarm_id, &swarm_state).await;
    broadcast_swarm_plan(
        swarm_id,
        Some("task_salvaged_dead_worker".to_string()),
        swarm_plans,
        swarm_members,
        swarms_by_id,
    )
    .await;
    notify_coordinator_of_salvage(
        session_id,
        swarm_id,
        &outcome,
        swarm_members,
        swarm_coordinators,
    )
    .await;
    outcome
}

/// Deliver a salvage notification to the swarm's current coordinator (when it
/// is not the dead session itself).
async fn notify_coordinator_of_salvage(
    session_id: &str,
    swarm_id: &str,
    outcome: &DeadMemberSalvage,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) {
    let coordinator_id = {
        let coordinators = swarm_coordinators.read().await;
        coordinators.get(swarm_id).cloned()
    };
    let Some(coordinator_id) = coordinator_id.filter(|id| id != session_id) else {
        return;
    };
    let label = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .and_then(|member| member.friendly_name.clone())
    }
    .unwrap_or_else(|| session_id[..8.min(session_id.len())].to_string());
    let _ = fanout_session_event(
        swarm_members,
        &coordinator_id,
        ServerEvent::Notification {
            from_session: session_id.to_string(),
            from_name: Some(label.clone()),
            notification_type: NotificationType::Message {
                scope: Some("swarm".to_string()),
                channel: None,
                tldr: None,
            },
            message: outcome.describe(&label),
        },
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "task progress touch updates durable progress plus swarm persistence and coordinator-facing state in one helper"
)]
pub(super) async fn touch_swarm_task_progress(
    swarm_id: &str,
    task_id: &str,
    assigned_session_id: Option<&str>,
    detail: Option<String>,
    checkpoint_summary: Option<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) -> bool {
    let now_ms = now_unix_ms();
    let revived = {
        let mut plans = swarm_plans.write().await;
        let Some(plan) = plans.get_mut(swarm_id) else {
            return false;
        };
        let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id) else {
            return false;
        };
        let progress = plan.task_progress.entry(task_id.to_string()).or_default();
        if let Some(session_id) = assigned_session_id {
            progress.assigned_session_id = Some(session_id.to_string());
        }
        // Heartbeats/checkpoints are proof of life for the assigned session:
        // fold them into the member activity clock so swarm status reflects
        // busy workers whose lifecycle status has not changed in a while.
        if let Some(session_id) = progress.assigned_session_id.as_deref() {
            crate::session_metrics::record_activity(session_id);
        }
        progress.last_heartbeat_unix_ms = Some(now_ms);
        progress.heartbeat_count = Some(progress.heartbeat_count.unwrap_or(0) + 1);
        if let Some(detail) = detail {
            progress.last_detail = Some(truncate_detail(&detail, 120));
        }
        if let Some(summary) = checkpoint_summary {
            progress.last_checkpoint_unix_ms = Some(now_ms);
            progress.checkpoint_summary = Some(truncate_detail(&summary, 120));
            progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
        }
        if item.status == "running_stale" {
            item.status = "running".to_string();
            progress.stale_since_unix_ms = None;
            plan.version += 1;
            true
        } else {
            false
        }
    };
    let swarm_state = SwarmState {
        members: Arc::clone(swarm_members),
        swarms_by_id: Arc::clone(swarms_by_id),
        plans: Arc::clone(swarm_plans),
        coordinators: Arc::clone(swarm_coordinators),
    };
    persist_swarm_state_for(swarm_id, &swarm_state).await;
    revived
}

pub(super) async fn refresh_swarm_task_staleness(
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) {
    let now_ms = now_unix_ms();
    let stale_after_ms = swarm_task_stale_after().as_millis() as u64;
    let changed_swarm_ids = {
        let mut plans = swarm_plans.write().await;
        let mut changed = Vec::new();
        for (swarm_id, plan) in plans.iter_mut() {
            let mut swarm_changed = false;
            for item in &mut plan.items {
                if !matches!(item.status.as_str(), "running" | "running_stale") {
                    continue;
                }
                let progress = plan.task_progress.entry(item.id.clone()).or_default();
                let last_heartbeat = progress
                    .last_heartbeat_unix_ms
                    .or(progress.started_at_unix_ms)
                    .or(progress.assigned_at_unix_ms);
                let is_stale = last_heartbeat
                    .map(|ts| now_ms.saturating_sub(ts) >= stale_after_ms)
                    .unwrap_or(true);
                match (item.status.as_str(), is_stale) {
                    ("running", true) => {
                        item.status = "running_stale".to_string();
                        progress.stale_since_unix_ms.get_or_insert(now_ms);
                        plan.version += 1;
                        swarm_changed = true;
                    }
                    ("running_stale", false) => {
                        item.status = "running".to_string();
                        progress.stale_since_unix_ms = None;
                        plan.version += 1;
                        swarm_changed = true;
                    }
                    _ => {}
                }
            }
            if swarm_changed {
                changed.push(swarm_id.clone());
            }
        }
        changed
    };

    for swarm_id in changed_swarm_ids {
        let swarm_state = SwarmState {
            members: Arc::clone(swarm_members),
            swarms_by_id: Arc::clone(swarms_by_id),
            plans: Arc::clone(swarm_plans),
            coordinators: Arc::clone(swarm_coordinators),
        };
        persist_swarm_state_for(&swarm_id, &swarm_state).await;
        broadcast_swarm_plan(
            &swarm_id,
            Some("task_staleness_changed".to_string()),
            swarm_plans,
            swarm_members,
            swarms_by_id,
        )
        .await;
    }

    // Second phase: salvage in-flight items whose assignee is dead. Staleness
    // marking above only reflects missing heartbeats; when the assigned member
    // is gone from the swarm or sits in a terminal lifecycle status, no
    // heartbeat or turn-end will ever arrive, so the item must be requeued
    // (or failed at the reclaim cap) instead of pulsing running_stale forever.
    // A terminal-status member gets a grace period before salvage: reload
    // recovery briefly marks resumable members `crashed` before restoring
    // them, and salvaging inside that window would double-assign their work.
    let salvage_grace = swarm_task_stale_after();
    let salvage_candidates: Vec<(String, String)> = {
        let plans = swarm_plans.read().await;
        let members = swarm_members.read().await;
        let mut pairs = std::collections::BTreeSet::new();
        for (swarm_id, plan) in plans.iter() {
            for item in &plan.items {
                if !matches!(item.status.as_str(), "running" | "running_stale" | "queued") {
                    continue;
                }
                let assignee = item.assigned_to.as_deref().or_else(|| {
                    plan.task_progress
                        .get(&item.id)
                        .and_then(|progress| progress.assigned_session_id.as_deref())
                });
                let Some(assignee) = assignee else {
                    continue;
                };
                let assignee_is_dead = match members.get(assignee) {
                    None => true,
                    Some(member) => {
                        member_status_is_dead(&member.status)
                            && member.last_status_change.elapsed() >= salvage_grace
                    }
                };
                if assignee_is_dead {
                    pairs.insert((swarm_id.clone(), assignee.to_string()));
                }
            }
        }
        pairs.into_iter().collect()
    };
    for (swarm_id, session_id) in salvage_candidates {
        salvage_assignments_of_dead_member(
            &session_id,
            &swarm_id,
            swarm_members,
            swarms_by_id,
            swarm_plans,
            swarm_coordinators,
        )
        .await;
    }
}

fn swarm_broadcast_key(
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) -> String {
    format!(
        "{:p}:{:p}:{swarm_id}",
        Arc::as_ptr(swarm_members),
        Arc::as_ptr(swarms_by_id)
    )
}

async fn broadcast_swarm_status_now(
    session_ids: Vec<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) {
    if session_ids.is_empty() {
        return;
    }

    let members_guard = swarm_members.read().await;
    let broadcast_terminal_retention = swarm_status_broadcast_terminal_retention();
    let members_list: Vec<crate::protocol::SwarmMemberStatus> = session_ids
        .iter()
        .filter_map(|sid| {
            members_guard
                .get(sid)
                .filter(|m| member_in_status_broadcast(m, broadcast_terminal_retention))
                .map(|m| {
                    let tokens = crate::session_metrics::token_totals(&m.session_id);
                    let wait = crate::wait_registry::current(&m.session_id);
                    let context_limit = m
                        .runtime
                        .model
                        .as_deref()
                        .and_then(crate::provider::models::context_limit_for_model)
                        .map(|limit| limit as u64);
                    (m, tokens, context_limit, wait)
                })
                .map(
                    |(m, tokens, context_limit, wait)| crate::protocol::SwarmMemberStatus {
                        session_id: m.session_id.clone(),
                        friendly_name: m.friendly_name.clone(),
                        status: m.status.clone(),
                        detail: m.detail.clone(),
                        task_label: m.task_label.clone(),
                        role: Some(m.role.clone()),
                        is_headless: Some(m.is_headless),
                        live_attachments: Some(m.event_txs.len()),
                        status_age_secs: Some(status_age_secs(m.last_status_change)),
                        output_tail: m.output_tail.clone(),
                        report_back_to_session_id: m.report_back_to_session_id.clone(),
                        todo_progress: m.todo_progress,
                        todo_items: m.todo_items.clone(),
                        runtime: crate::protocol::SwarmMemberRuntime {
                            live_status: super::live_status::snapshot(&m.session_id),
                            wait_reason: wait.as_ref().map(|(reason, _)| reason.clone()).or_else(
                                || {
                                    (!matches!(
                                        m.status.as_str(),
                                        "running" | "streaming" | "thinking"
                                    ))
                                    .then(|| m.runtime.todo_blocked_reason.clone())
                                    .flatten()
                                },
                            ),
                            wait_age_secs: wait.map(|(_, age)| age),
                            todo_blocked_reason: m.runtime.todo_blocked_reason.clone(),
                            todo_statuses: m.runtime.todo_statuses.clone(),
                            model: m.runtime.model.clone(),
                            provider: m.runtime.provider.clone(),
                            auth_method: m.runtime.auth_method.clone(),
                            effort: m.runtime.effort.clone(),
                            elapsed_secs: if matches!(
                                m.status.as_str(),
                                "running" | "streaming" | "thinking"
                            ) {
                                Some(m.joined_at.elapsed().as_secs())
                            } else {
                                Some(m.runtime.elapsed_secs.unwrap_or(0))
                            },
                            activity_age_secs: crate::session_metrics::last_activity_age_secs(
                                &m.session_id,
                            ),
                            context_tokens: tokens.map(|(context, _)| context).filter(|t| *t > 0),
                            context_limit,
                            output_tokens: tokens.map(|(_, output)| output).filter(|t| *t > 0),
                        },
                    },
                )
        })
        .collect();

    drop(members_guard);
    let event = ServerEvent::SwarmStatus {
        members: members_list,
    };
    for sid in session_ids {
        let _ = fanout_session_event(swarm_members, &sid, event.clone()).await;
    }
}

/// Publish wait transitions immediately and refresh elapsed wait ages while a
/// session is parked, even when it emits no tokens or tool progress events.
pub(super) async fn monitor_wait_status(
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let mut changes = crate::wait_registry::subscribe_changes();
    let mut live_changes = super::live_status::subscribe_changes();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut previous = HashSet::<String>::new();
    loop {
        tokio::select! {
            result = changes.changed() => if result.is_err() { return; },
            result = live_changes.changed() => if result.is_err() { return; },
            _ = interval.tick() => {},
        }
        let current: HashSet<_> = crate::wait_registry::session_ids().into_iter().collect();
        let affected: HashSet<_> = {
            let members = swarm_members.read().await;
            previous
                .union(&current)
                .filter_map(|sid| members.get(sid).and_then(|member| member.swarm_id.clone()))
                .chain(
                    members
                        .values()
                        .filter(|member| {
                            matches!(member.status.as_str(), "running" | "streaming" | "thinking")
                        })
                        .filter_map(|member| member.swarm_id.clone()),
                )
                .collect()
        };
        previous = current;
        for swarm_id in affected {
            broadcast_swarm_status(&swarm_id, &swarm_members, &swarms_by_id).await;
        }
    }
}

pub(super) async fn broadcast_swarm_status(
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let session_ids: Vec<String> = {
        let swarms = swarms_by_id.read().await;
        swarms
            .get(swarm_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    };
    if session_ids.is_empty() {
        return;
    }

    if session_ids.len() < swarm_status_debounce_member_threshold() {
        broadcast_swarm_status_now(session_ids, swarm_members).await;
        return;
    }

    let key = swarm_broadcast_key(swarm_id, swarm_members, swarms_by_id);
    let should_spawn = {
        let mut pending = pending_swarm_status_broadcasts()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = pending.entry(key.clone()).or_default();
        if entry.scheduled {
            entry.dirty = true;
            false
        } else {
            entry.scheduled = true;
            entry.dirty = false;
            true
        }
    };

    if !should_spawn {
        return;
    }

    let swarm_id = swarm_id.to_string();
    let swarm_members = Arc::clone(swarm_members);
    let swarms_by_id = Arc::clone(swarms_by_id);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(swarm_status_debounce_ms())).await;
            let session_ids: Vec<String> = {
                let swarms = swarms_by_id.read().await;
                swarms
                    .get(&swarm_id)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default()
            };
            broadcast_swarm_status_now(session_ids, &swarm_members).await;

            let mut pending = pending_swarm_status_broadcasts()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(entry) = pending.get_mut(&key) else {
                break;
            };
            if entry.dirty {
                entry.dirty = false;
                continue;
            }
            pending.remove(&key);
            break;
        }
    });
}

/// Broadcast the authoritative swarm plan snapshot.
///
/// Plan snapshots are sent to explicit plan participants. If a plan has no
/// participants yet, fall back to all current swarm members.
pub(super) async fn broadcast_swarm_plan(
    swarm_id: &str,
    reason: Option<String>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    broadcast_swarm_plan_with_previous(
        swarm_id,
        reason,
        None,
        swarm_plans,
        swarm_members,
        swarms_by_id,
    )
    .await;
}

pub(super) async fn broadcast_swarm_plan_with_previous(
    swarm_id: &str,
    reason: Option<String>,
    previous_items: Option<&[PlanItem]>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let (version, items, summary, mut participants): (
        u64,
        Vec<PlanItem>,
        crate::protocol::PlanGraphStatus,
        Vec<String>,
    ) = {
        let plans = swarm_plans.read().await;
        let Some(vp) = plans.get(swarm_id) else {
            return;
        };
        let newly_ready_ids = previous_items
            .map(|before| newly_ready_item_ids(before, &vp.items))
            .unwrap_or_default();
        let mut p: Vec<String> = vp.participants.iter().cloned().collect();
        p.sort();
        (
            vp.version,
            vp.items.clone(),
            crate::protocol::PlanGraphStatus::from_versioned_plan(
                swarm_id,
                vp,
                Some(3),
                newly_ready_ids,
            ),
            p,
        )
    };

    if participants.is_empty() {
        let swarms = swarms_by_id.read().await;
        participants = swarms
            .get(swarm_id)
            .map(|s| {
                let mut ids: Vec<String> = s.iter().cloned().collect();
                ids.sort();
                ids
            })
            .unwrap_or_default();
    }

    if participants.is_empty() {
        return;
    }

    let item_count = items.len();
    let reason_label = reason.clone().unwrap_or_else(|| "unspecified".to_string());
    let event = ServerEvent::SwarmPlan {
        swarm_id: swarm_id.to_string(),
        version,
        items,
        participants: participants.clone(),
        reason,
        summary: Some(summary),
    };

    let members = swarm_members.read().await;
    let participant_count = participants.len();
    let mut delivered_count = 0usize;
    // The coordinator's session todos are the user-facing view of this plan;
    // keep them in sync on every broadcast so all completion paths (turn
    // end, report, manual status) reach the pinned todo band.
    let coordinator_ids: Vec<String> = participants
        .iter()
        .filter(|sid| {
            members
                .get(*sid)
                .is_some_and(|member| member.role == "coordinator")
        })
        .cloned()
        .collect();
    let sync_items = match &event {
        ServerEvent::SwarmPlan { items, .. } if !coordinator_ids.is_empty() => Some(items.clone()),
        _ => None,
    };
    for sid in participants {
        if let Some(member) = members.get(&sid)
            && member.event_tx.send(event.clone()).is_ok()
        {
            delivered_count += 1;
        }
    }
    drop(members);
    if let Some(items) = sync_items {
        for coordinator in &coordinator_ids {
            super::swarm_todo_link::sync_plan(coordinator, &items);
        }
    }
    log_swarm_lifecycle(
        "plan_broadcast",
        vec![
            ("swarm_id", swarm_id.to_string()),
            ("version", version.to_string()),
            ("item_count", item_count.to_string()),
            ("participant_count", participant_count.to_string()),
            ("delivered_count", delivered_count.to_string()),
            ("reason", reason_label),
        ],
    );
}

/// Send the current swarm plan snapshot to ONE session (subscribe/resume
/// refresh). Unlike [`broadcast_swarm_plan`] this does not fan out to all
/// participants: reconnecting clients would otherwise show no plan graph
/// until the next plan mutation happens to broadcast.
pub(super) async fn send_swarm_plan_to_session(
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    let swarm_id = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .and_then(|member| member.swarm_id.clone())
    };
    let Some(swarm_id) = swarm_id else {
        return;
    };

    let event = {
        let plans = swarm_plans.read().await;
        let Some(vp) = plans.get(&swarm_id) else {
            return;
        };
        if vp.items.is_empty() {
            return;
        }
        let mut participants: Vec<String> = vp.participants.iter().cloned().collect();
        participants.sort();
        ServerEvent::SwarmPlan {
            swarm_id: swarm_id.clone(),
            version: vp.version,
            items: vp.items.clone(),
            participants,
            reason: Some("reconnect".to_string()),
            summary: Some(crate::protocol::PlanGraphStatus::from_versioned_plan(
                &swarm_id,
                vp,
                Some(3),
                Vec::new(),
            )),
        }
    };

    let members = swarm_members.read().await;
    if let Some(member) = members.get(session_id) {
        let _ = member.event_tx.send(event);
    }
}

pub(super) async fn rename_plan_participant(
    swarm_id: &str,
    old_session_id: &str,
    new_session_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    let mut plans = swarm_plans.write().await;
    if let Some(vp) = plans.get_mut(swarm_id) {
        vp.rename_session(old_session_id, new_session_id);
    }
}

pub(super) async fn remove_plan_participant(
    swarm_id: &str,
    session_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    let mut plans = swarm_plans.write().await;
    if let Some(vp) = plans.get_mut(swarm_id) {
        vp.participants.remove(session_id);
    }
}

pub(super) async fn remove_session_from_swarm(
    session_id: &str,
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    let started = Instant::now();
    log_swarm_lifecycle(
        "member_remove_start",
        vec![
            ("session_id", session_id.to_string()),
            ("swarm_id", swarm_id.to_string()),
        ],
    );
    // Capture the departing member's own spawner before any teardown. Some
    // callers remove the member from the map before calling us, so this is
    // best-effort: when unavailable the orphan-reparenting below falls back to
    // the swarm coordinator.
    let departing_parent: Option<String> = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .and_then(|member| member.report_back_to_session_id.clone())
    };
    // A leaving member can no longer drive its plan assignments (crash, stop,
    // disconnect, feature-off all funnel through here). Salvage before any
    // membership state is torn down so the coordinator notification can still
    // resolve names and fan out.
    salvage_assignments_of_dead_member(
        session_id,
        swarm_id,
        swarm_members,
        swarms_by_id,
        swarm_plans,
        swarm_coordinators,
    )
    .await;
    remove_plan_participant(swarm_id, session_id, swarm_plans).await;
    super::live_status::forget(session_id);

    {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(swarm_id) {
            swarm.remove(session_id);
            if swarm.is_empty() {
                swarms.remove(swarm_id);
            }
        }
    }

    let was_coordinator = {
        let coordinators = swarm_coordinators.read().await;
        coordinators
            .get(swarm_id)
            .map(|id| id == session_id)
            .unwrap_or(false)
    };

    let mut elected_coordinator = None;
    if was_coordinator {
        let new_coordinator = {
            let swarms = swarms_by_id.read().await;
            let members = swarm_members.read().await;
            swarms.get(swarm_id).and_then(|swarm| {
                swarm
                    .iter()
                    .filter_map(|id| {
                        members
                            .get(id)
                            .filter(|member| !member.is_headless)
                            .map(|_| id.clone())
                    })
                    .min()
            })
        };

        {
            let mut coordinators = swarm_coordinators.write().await;
            coordinators.remove(swarm_id);
            if let Some(ref new_id) = new_coordinator {
                coordinators.insert(swarm_id.to_string(), new_id.clone());
            }
        }

        if let Some(new_id) = new_coordinator {
            elected_coordinator = Some(new_id.clone());
            {
                let mut members = swarm_members.write().await;
                if let Some(member) = members.get_mut(&new_id) {
                    member.role = "coordinator".to_string();
                }
            }
            let mut plans = swarm_plans.write().await;
            if let Some(vp) = plans.get_mut(swarm_id) {
                vp.participants.insert(new_id.clone());
            }
            let members = swarm_members.read().await;
            if let Some(member) = members.get(&new_id) {
                let _ = member.event_tx.send(ServerEvent::Notification {
                    from_session: new_id.clone(),
                    from_name: member.friendly_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("swarm".to_string()),
                        channel: None,
                        tldr: None,
                    },
                    message: "You are now the coordinator for this swarm.".to_string(),
                });
            }
        }
    }

    {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(session_id) {
            member.role = "agent".to_string();
        }
    }

    // Reparent the departing member's direct children so the spawn tree never
    // holds dangling report-back edges. Orphaned subtrees would otherwise
    // silently change ownership semantics: stop permissions, subtree broadcast
    // scope, and completion report-back all walk this chain. Children are
    // attached to their grandparent when it is still a live member of this
    // swarm, otherwise to the current coordinator, otherwise they become
    // roots (report_back_to_session_id = None).
    let fallback_parent: Option<String> = {
        let grandparent_is_live = if let Some(ref parent) = departing_parent {
            parent != session_id && {
                let members = swarm_members.read().await;
                members
                    .get(parent)
                    .is_some_and(|member| member.swarm_id.as_deref() == Some(swarm_id))
            }
        } else {
            false
        };
        if grandparent_is_live {
            departing_parent.clone()
        } else {
            let coordinators = swarm_coordinators.read().await;
            coordinators
                .get(swarm_id)
                .filter(|coordinator| coordinator.as_str() != session_id)
                .cloned()
        }
    };
    let mut reparented: Vec<String> = Vec::new();
    // With no live fallback, keep the children pointing at their departing
    // spawner. The common cause is a TUI owner dropping off during a server
    // reload: promoting its workers to roots let one of them (a headless
    // worker) claim the coordinator slot, and the owner could no longer
    // resume, retry or wake its own workers when it reconnected moments
    // later. A dangling report-back edge is harmless: the ancestor walk stops
    // at a missing member and report-back delivery already tolerates an
    // absent recipient.
    if fallback_parent.is_some() {
        let mut members = swarm_members.write().await;
        for member in members.values_mut() {
            if member.swarm_id.as_deref() == Some(swarm_id)
                && member.report_back_to_session_id.as_deref() == Some(session_id)
            {
                member.report_back_to_session_id = fallback_parent
                    .clone()
                    .filter(|parent| parent != &member.session_id);
                reparented.push(member.session_id.clone());
            }
        }
    }
    if !reparented.is_empty() {
        log_swarm_lifecycle(
            "member_remove_reparent",
            vec![
                ("session_id", session_id.to_string()),
                ("swarm_id", swarm_id.to_string()),
                (
                    "new_parent",
                    fallback_parent
                        .clone()
                        .unwrap_or_else(|| "none (promoted to root)".to_string()),
                ),
                ("reparented_children", reparented.join(",")),
            ],
        );
    }

    if swarm_plans.read().await.contains_key(swarm_id) {
        let swarm_state = SwarmState {
            members: Arc::clone(swarm_members),
            swarms_by_id: Arc::clone(swarms_by_id),
            plans: Arc::clone(swarm_plans),
            coordinators: Arc::clone(swarm_coordinators),
        };
        persist_swarm_state_for(swarm_id, &swarm_state).await;
    } else {
        let swarm_state = SwarmState {
            members: Arc::clone(swarm_members),
            swarms_by_id: Arc::clone(swarms_by_id),
            plans: Arc::clone(swarm_plans),
            coordinators: Arc::clone(swarm_coordinators),
        };
        remove_persisted_swarm_state_for(swarm_id, &swarm_state).await;
    }

    let remaining_member_count = swarms_by_id
        .read()
        .await
        .get(swarm_id)
        .map(|members| members.len())
        .unwrap_or_default();
    log_swarm_lifecycle(
        "member_remove_done",
        vec![
            ("session_id", session_id.to_string()),
            ("swarm_id", swarm_id.to_string()),
            ("was_coordinator", was_coordinator.to_string()),
            (
                "new_coordinator_session_id",
                elected_coordinator.unwrap_or_else(|| "none".to_string()),
            ),
            ("remaining_member_count", remaining_member_count.to_string()),
            ("elapsed_ms", started.elapsed().as_millis().to_string()),
        ],
    );
    broadcast_swarm_status(swarm_id, swarm_members, swarms_by_id).await;
}

/// Whether `member` is an idle headless worker a restart stopped and whose
/// owner has not yet been told (issue #1651).
fn member_awaits_unrestored_notice(member: &SwarmMember) -> bool {
    member.is_headless
        && member.status == "stopped"
        && member.detail.as_deref().is_some_and(|detail| {
            detail.contains(super::swarm_persistence::IDLE_WORKER_NOT_RESTORED_NOTE)
                && !detail.contains(super::swarm_persistence::OWNER_NOTIFIED_NOTE)
        })
        // Shutdown can leave a ready snapshot even for work that has a durable
        // continuation. Startup will restore that worker, so don't tell its
        // coordinator it had no work in flight and needs replacing.
        && !super::reload_recovery::has_pending_for_session(&member.session_id)
}

/// Tell `owner_session_id` about idle headless workers that the last server
/// restart stopped (issue #1651).
///
/// No headless process survives a restart, so an idle worker loads as
/// `stopped`. A TUI member re-marks itself ready on subscribe, but a headless
/// worker has nothing to re-attach, so without this its coordinator only
/// learns it is gone by polling the member list. Called when a session attaches
/// (subscribe or resume): workers that report back to it, or whose swarm it
/// coordinates, are listed in one model-facing interrupt and a best-effort UI
/// notification. Acknowledge only accepted model delivery, so an unavailable
/// owner can retry on reconnect. Returns the number of workers reported.
pub(super) async fn notify_owner_of_unrestored_idle_workers(
    owner_session_id: &str,
    swarm_state: &SwarmState,
    sessions: &super::SessionAgents,
    soft_interrupt_queues: &super::SessionInterruptQueues,
) -> usize {
    // Concurrent subscribe/resume requests must not both observe an unnotified
    // worker before either has queued its interrupt. This lock is independent
    // of the swarm snapshot lock, which persistence acquires below.
    static NOTICE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _notice_guard = NOTICE_LOCK.lock().await;
    let coordinated_swarms: HashSet<String> = {
        let coordinators = swarm_state.coordinators.read().await;
        coordinators
            .iter()
            .filter(|(_, coordinator)| coordinator.as_str() == owner_session_id)
            .map(|(swarm_id, _)| swarm_id.clone())
            .collect()
    };
    let (owner_name, reported) = {
        let members = swarm_state.members.read().await;
        let owner_name = members
            .get(owner_session_id)
            .and_then(|member| member.friendly_name.clone());
        let mut reported: Vec<(String, String, Option<String>)> = Vec::new();
        for member in members.values() {
            if member.session_id == owner_session_id || !member_awaits_unrestored_notice(member) {
                continue;
            }
            let owned_by_report_back =
                member.report_back_to_session_id.as_deref() == Some(owner_session_id);
            let owned_by_coordination = member.report_back_to_session_id.is_none()
                && member
                    .swarm_id
                    .as_ref()
                    .is_some_and(|swarm_id| coordinated_swarms.contains(swarm_id));
            if !(owned_by_report_back || owned_by_coordination) {
                continue;
            }
            reported.push((
                member.session_id.clone(),
                member
                    .friendly_name
                    .clone()
                    .unwrap_or_else(|| member.session_id.clone()),
                member.swarm_id.clone(),
            ));
        }
        reported.sort();
        (owner_name, reported)
    };
    if reported.is_empty() {
        return 0;
    }

    let names = reported
        .iter()
        .map(|(_, name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let count = reported.len();
    let message = if count == 1 {
        format!(
            "Idle headless worker {names} was stopped by the server restart and not restored. \
             It had no work in flight. Spawn a replacement if you still need it."
        )
    } else {
        format!(
            "{count} idle headless workers were stopped by the server restart and not restored: {names}. \
             They had no work in flight. Spawn replacements if you still need them."
        )
    };
    if !super::queue_soft_interrupt_for_session(
        owner_session_id,
        message.clone(),
        false,
        jcode_agent_runtime::SoftInterruptSource::System,
        soft_interrupt_queues,
        sessions,
    )
    .await
    {
        return 0;
    }
    {
        let mut members = swarm_state.members.write().await;
        for (session_id, _, _) in &reported {
            if let Some(member) = members.get_mut(session_id)
                && member_awaits_unrestored_notice(member)
            {
                member.detail = Some(format!(
                    "{} ({})",
                    member.detail.as_deref().unwrap_or_default(),
                    super::swarm_persistence::OWNER_NOTIFIED_NOTE
                ));
            }
        }
    }
    crate::logging::info(&format!(
        "Notifying {owner_session_id} of {count} idle headless worker(s) not restored after restart: {names}"
    ));
    let (first_session, _, _) = &reported[0];
    let _ = fanout_session_event(
        &swarm_state.members,
        owner_session_id,
        ServerEvent::Notification {
            from_session: first_session.clone(),
            from_name: owner_name,
            notification_type: NotificationType::Message {
                scope: Some("swarm".to_string()),
                channel: None,
                tldr: Some(format!(
                    "{count} idle worker{} not restored after restart",
                    if count == 1 { "" } else { "s" }
                )),
            },
            message,
        },
    )
    .await;

    let swarms: HashSet<String> = reported
        .iter()
        .filter_map(|(_, _, swarm_id)| swarm_id.clone())
        .collect();
    for swarm_id in swarms {
        persist_swarm_state_for(&swarm_id, swarm_state).await;
        broadcast_swarm_status(&swarm_id, &swarm_state.members, &swarm_state.swarms_by_id).await;
    }
    count
}

/// Set a member's stable task label, derived from its spawn prompt or task
/// assignment. Unlike `detail` (transient status text), the label survives
/// status churn so UIs can always answer "what was this agent for?". A later
/// assignment overwrites the label: the member is now doing that task.
pub(super) async fn set_member_task_label(
    session_id: &str,
    task_text: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) {
    let Some(label) = jcode_swarm_core::derive_swarm_task_label(task_text) else {
        return;
    };
    let mut members = swarm_members.write().await;
    if let Some(member) = members.get_mut(session_id) {
        member.task_label = Some(label);
    }
}

pub(super) async fn record_swarm_event(
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
    session_id: String,
    session_name: Option<String>,
    swarm_id: Option<String>,
    event: SwarmEventType,
) {
    let swarm_event = SwarmEvent {
        id: event_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        session_id,
        session_name,
        swarm_id,
        event,
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    };
    let _ = swarm_event_tx.send(swarm_event.clone());
    let mut history = event_history.write().await;
    history.push_back(swarm_event);
    if history.len() > MAX_EVENT_HISTORY {
        history.pop_front();
    }
}

pub(super) async fn record_swarm_event_for_session(
    session_id: &str,
    event: SwarmEventType,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    event_history: &Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let (session_name, swarm_id) = {
        let members = swarm_members.read().await;
        if let Some(member) = members.get(session_id) {
            (member.friendly_name.clone(), member.swarm_id.clone())
        } else {
            (None, None)
        }
    };
    record_swarm_event(
        event_history,
        event_counter,
        swarm_event_tx,
        session_id.to_string(),
        session_name,
        swarm_id,
        event,
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "member status updates need swarm membership, broadcast state, and optional event history sinks"
)]
pub(super) async fn update_member_status(
    session_id: &str,
    status: &str,
    detail: Option<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: Option<&Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>>,
    event_counter: Option<&Arc<std::sync::atomic::AtomicU64>>,
    swarm_event_tx: Option<&broadcast::Sender<SwarmEvent>>,
) {
    update_member_status_with_report(
        session_id,
        status,
        detail,
        None,
        swarm_members,
        swarms_by_id,
        event_history,
        event_counter,
        swarm_event_tx,
    )
    .await;
}

#[expect(
    clippy::too_many_arguments,
    reason = "member status updates need swarm membership, broadcast state, optional report text, and event history sinks"
)]
pub(super) async fn update_member_status_with_report(
    session_id: &str,
    status: &str,
    detail: Option<String>,
    completion_report: Option<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: Option<&Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>>,
    event_counter: Option<&Arc<std::sync::atomic::AtomicU64>>,
    swarm_event_tx: Option<&broadcast::Sender<SwarmEvent>>,
) {
    update_member_status_with_report_tldr(
        session_id,
        status,
        detail,
        completion_report,
        None,
        swarm_members,
        swarms_by_id,
        event_history,
        event_counter,
        swarm_event_tx,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "member status updates need swarm membership, broadcast state, optional report text, and event history sinks"
)]
pub(super) async fn update_member_status_with_report_tldr(
    session_id: &str,
    status: &str,
    detail: Option<String>,
    completion_report: Option<String>,
    report_tldr: Option<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: Option<&Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>>,
    event_counter: Option<&Arc<std::sync::atomic::AtomicU64>>,
    swarm_event_tx: Option<&broadcast::Sender<SwarmEvent>>,
) {
    let mut completion_report = normalize_completion_report(completion_report);
    let detail_present = detail.is_some();
    let (
        swarm_id,
        agent_name,
        member_changed,
        status_changed,
        old_status,
        _is_headless,
        report_back_to_session_id,
    ) = {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(session_id) {
            // Forced stops and crashes may abort the turn before its transcript
            // can be collected. Preserve the already streamed public tail before
            // the terminal transition clears the gallery buffer.
            if completion_report.is_none()
                && matches!(
                    status,
                    "ready" | "completed" | "done" | "failed" | "crashed" | "stopped"
                )
            {
                completion_report = member.output_tail.as_deref().and_then(|tail| {
                    jcode_swarm_core::recent_output_tail(
                        std::iter::once(tail),
                        jcode_swarm_core::SWARM_OUTPUT_TAIL_TOKENS,
                    )
                });
            }
            let previous_status = member.status.clone();
            let status_changed = member.status != status;
            let detail_changed = member.detail != detail;
            if status_changed
                || detail_changed
                || super::live_status::snapshot(session_id).is_none()
            {
                if matches!(status, "running" | "streaming" | "thinking") {
                    if !matches!(
                        previous_status.as_str(),
                        "running" | "streaming" | "thinking"
                    ) || super::live_status::snapshot(session_id).is_none()
                    {
                        super::live_status::begin(session_id);
                    }
                } else {
                    super::live_status::finish(session_id, status, detail.as_deref());
                }
            }
            let report_changed =
                completion_report.is_some() && member.latest_completion_report != completion_report;
            let member_changed = status_changed || detail_changed || report_changed;
            if status_changed {
                member.last_status_change = Instant::now();
                if matches!(status, "running" | "streaming" | "thinking") {
                    member.runtime.elapsed_secs = None;
                } else if matches!(
                    previous_status.as_str(),
                    "running" | "streaming" | "thinking"
                ) {
                    member.runtime.elapsed_secs = Some(member.joined_at.elapsed().as_secs());
                }
            }
            let name = member.friendly_name.clone();
            let is_headless = member.is_headless;
            let report_back_to_session_id = member.report_back_to_session_id.clone();
            member.status = status.to_string();
            member.detail = detail;
            // Clear any live output tail when the worker reaches a terminal or
            // idle state so the inline gallery viewport doesn't keep showing
            // stale in-progress text after the turn finishes.
            if matches!(
                status,
                "ready" | "completed" | "done" | "failed" | "crashed" | "stopped"
            ) {
                member.output_tail = None;
            }
            if completion_report.is_some() {
                member.latest_completion_report = completion_report.clone();
            }
            (
                member.swarm_id.clone(),
                name,
                member_changed,
                status_changed,
                previous_status,
                is_headless,
                report_back_to_session_id,
            )
        } else {
            (None, None, false, false, String::new(), false, None)
        }
    };
    if let Some(ref id) = swarm_id {
        if !member_changed {
            return;
        }

        log_swarm_lifecycle(
            "member_status_updated",
            vec![
                ("session_id", session_id.to_string()),
                ("swarm_id", id.clone()),
                ("old_status", old_status.clone()),
                ("new_status", status.to_string()),
                ("status_changed", status_changed.to_string()),
                ("detail_present", detail_present.to_string()),
                (
                    "completion_report_present",
                    completion_report.is_some().to_string(),
                ),
                (
                    "report_back_to_session_id",
                    report_back_to_session_id
                        .clone()
                        .unwrap_or_else(|| "none".to_string()),
                ),
            ],
        );

        if status_changed
            && let (Some(history), Some(counter), Some(tx)) =
                (event_history, event_counter, swarm_event_tx)
        {
            record_swarm_event(
                history,
                counter,
                tx,
                session_id.to_string(),
                agent_name.clone(),
                Some(id.clone()),
                SwarmEventType::StatusChange {
                    old_status: old_status.clone(),
                    new_status: status.to_string(),
                },
            )
            .await;
        }

        broadcast_swarm_status(id, swarm_members, swarms_by_id).await;

        let should_notify_coordinator = status_changed
            && ((status == "completed")
                || (report_back_to_session_id.is_some()
                    && matches!(old_status.as_str(), "running" | "streaming" | "thinking" | "running_stale")
                    && matches!(status, "ready" | "failed" | "stopped"))
                // A crash is never routine: notify whoever is responsible
                // (owner, else coordinator) whenever a member dies while it
                // was doing or holding work, so worker deaths cannot pass
                // silently.
                || (status == "crashed"
                    && matches!(
                        old_status.as_str(),
                        "running" | "running_stale" | "queued"
                    )));
        if should_notify_coordinator {
            let fallback_coordinator_id =
                if report_back_to_session_id.as_deref() == Some(session_id) {
                    None
                } else {
                    let members = swarm_members.read().await;
                    members
                        .values()
                        .find(|m| {
                            m.swarm_id.as_deref() == Some(id)
                                && m.role == "coordinator"
                                && m.session_id != session_id
                        })
                        .map(|m| m.session_id.clone())
                };
            let recipient_session_id = report_back_to_session_id
                .clone()
                .filter(|owner_id| owner_id != session_id)
                .or(fallback_coordinator_id);
            if let Some(recipient_session_id) = recipient_session_id {
                let name = agent_name
                    .as_deref()
                    .unwrap_or(&session_id[..8.min(session_id.len())]);
                let msg =
                    completion_notification_message(name, status, completion_report.as_deref());
                let _ = fanout_session_event(
                    swarm_members,
                    &recipient_session_id,
                    ServerEvent::Notification {
                        from_session: session_id.to_string(),
                        from_name: agent_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some("swarm".to_string()),
                            channel: None,
                            tldr: report_tldr.clone(),
                        },
                        message: msg,
                    },
                )
                .await;
            }
        }
    }
}

/// Inline workers have no entry in the server's live-session map. Clean their
/// ephemeral environment on normal completion, errors, and future cancellation.
struct InlineWorkerEnv(String);

impl Drop for InlineWorkerEnv {
    fn drop(&mut self) {
        crate::session_env::remove_session_env(&self.0);
    }
}

pub(super) async fn run_swarm_task(
    agent: Arc<Mutex<Agent>>,
    description: &str,
    subagent_type: &str,
    prompt: &str,
) -> Result<String> {
    let started = Instant::now();
    let (provider, registry, session_id, working_dir, coordinator_model, provider_key, route) = {
        let agent = agent.lock().await;
        (
            agent.provider_fork(),
            agent.registry(),
            agent.session_id().to_string(),
            agent.working_dir().map(PathBuf::from),
            agent.provider_model(),
            agent.session_provider_key(),
            agent.session_route_api_method(),
        )
    };
    let parent_session_id = session_id.clone();
    let mut session = Session::create(
        Some(session_id),
        Some(format!("{} (@{} swarm)", description, subagent_type)),
    );
    let child_session_id = session.id.clone();
    session.model = Some(coordinator_model);
    // Inherit the coordinator's exact auth identity so the forked worker keeps
    // the same provider/auth route (OAuth vs API, openai-compatible profile)
    // instead of silently falling back to the config default on persistence.
    session.provider_key = provider_key;
    session.route_api_method = route;
    if let Some(dir) = working_dir {
        session.working_dir = Some(dir.display().to_string());
    }
    session.save()?;

    log_swarm_lifecycle(
        "task_start",
        vec![
            ("parent_session_id", parent_session_id.clone()),
            ("child_session_id", child_session_id.clone()),
            ("subagent_type", subagent_type.to_string()),
            ("description_chars", description.chars().count().to_string()),
            ("prompt_chars", prompt.chars().count().to_string()),
        ],
    );

    let mut allowed: HashSet<String> = registry.tool_names().await.into_iter().collect();
    for blocked in ["subagent", "task", "todo", "todowrite", "todoread"] {
        allowed.remove(blocked);
    }
    crate::config::config()
        .tools
        .apply_to_allowed_set(&mut allowed);

    let mut worker = Agent::new_with_session(provider, registry, session, Some(allowed));
    crate::session_env::inherit_session_env(&parent_session_id, &child_session_id);
    let _environment_cleanup = InlineWorkerEnv(child_session_id.clone());
    match worker.run_once_capture(prompt).await {
        Ok(output) => {
            log_swarm_lifecycle(
                "task_done",
                vec![
                    ("parent_session_id", parent_session_id),
                    ("child_session_id", child_session_id),
                    ("subagent_type", subagent_type.to_string()),
                    ("output_chars", output.chars().count().to_string()),
                    ("elapsed_ms", started.elapsed().as_millis().to_string()),
                ],
            );
            Ok(output)
        }
        Err(error) => {
            crate::logging::event_warn(
                "SWARM_LIFECYCLE",
                vec![
                    ("phase", "task_error".to_string()),
                    ("parent_session_id", parent_session_id),
                    ("child_session_id", child_session_id),
                    ("subagent_type", subagent_type.to_string()),
                    ("error", error.to_string()),
                    ("elapsed_ms", started.elapsed().as_millis().to_string()),
                ],
            );
            Err(error)
        }
    }
}

pub(super) async fn run_swarm_message(agent: Arc<Mutex<Agent>>, message: &str) -> Result<String> {
    let started = Instant::now();
    log_swarm_lifecycle(
        "message_start",
        vec![("message_chars", message.chars().count().to_string())],
    );
    let working_dir = {
        let agent = agent.lock().await;
        agent.working_dir().map(|dir| dir.to_string())
    };
    let working_dir_hint = working_dir
        .as_deref()
        .map(|dir| format!("Working directory: {}\n", dir))
        .unwrap_or_default();

    let planner_prompt = format!(
        "{working_dir_hint}You are a task planner. Break the request into 2-4 subtasks. \
Return ONLY a JSON array of objects with keys: description, prompt, subagent_type. \
No extra text.\n\nRequest:\n{message}"
    );

    let plan_text = {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&planner_prompt).await?
    };

    let mut tasks = parse_swarm_tasks(&plan_text);
    if tasks.is_empty() {
        tasks.push(SwarmTaskSpec {
            description: "Main task".to_string(),
            prompt: message.to_string(),
            subagent_type: Some("general".to_string()),
        });
    }
    log_swarm_lifecycle(
        "message_plan_done",
        vec![
            ("task_count", tasks.len().to_string()),
            ("plan_chars", plan_text.chars().count().to_string()),
        ],
    );

    let task_futures = tasks.iter().map(|task| {
        let agent = agent.clone();
        let working_dir_hint = working_dir_hint.clone();
        let description = task.description.clone();
        let prompt = format!("{working_dir_hint}{}", task.prompt);
        let subagent_type = task
            .subagent_type
            .clone()
            .unwrap_or_else(|| "general".to_string());
        async move {
            let output = run_swarm_task(agent, &description, &subagent_type, &prompt).await?;
            Ok::<(String, String), anyhow::Error>((description, output))
        }
    });
    let task_outputs = try_join_all(task_futures).await?;

    let mut integration_prompt = String::new();
    integration_prompt.push_str(
        "You are the coordinator. Complete the original request using the subagent outputs below. ",
    );
    integration_prompt.push_str("Do not stop early; run any requested tests and fix failures.\n\n");
    integration_prompt.push_str("Original request:\n");
    integration_prompt.push_str(message);
    integration_prompt.push_str("\n\nSubagent outputs:\n");
    for (desc, output) in &task_outputs {
        integration_prompt.push_str(&format!("\n--- {} ---\n{}\n", desc, output));
    }
    integration_prompt.push_str("\nNow complete the task.\n");

    let final_output = {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&integration_prompt).await?
    };

    log_swarm_lifecycle(
        "message_done",
        vec![
            ("task_count", task_outputs.len().to_string()),
            ("output_chars", final_output.chars().count().to_string()),
            ("elapsed_ms", started.elapsed().as_millis().to_string()),
        ],
    );

    Ok(final_output)
}

#[derive(Debug, Deserialize)]
struct SwarmTaskSpec {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
}

fn parse_swarm_tasks(text: &str) -> Vec<SwarmTaskSpec> {
    if let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(text) {
        return tasks;
    }

    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']'))
        && start < end
        && let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(&text[start..=end])
    {
        return tasks;
    }

    Vec::new()
}

#[cfg(test)]
#[path = "swarm_tests.rs"]
mod tests;
