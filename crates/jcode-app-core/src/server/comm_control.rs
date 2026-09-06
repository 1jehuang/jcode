#![cfg_attr(test, allow(clippy::items_after_test_module))]

use super::append_swarm_completion_report_instructions;
use super::swarm::{
    now_unix_ms, swarm_task_heartbeat_interval, swarm_task_stale_after, touch_swarm_task_progress,
};
use super::swarm_mutation_state::{
    PersistedSwarmMutationResponse, begin_or_join_in_flight as begin_swarm_mutation_no_replay,
    begin_or_replay as begin_swarm_mutation_or_replay,
    finish_request as finish_swarm_mutation_request, request_key as swarm_mutation_request_key,
};
use super::{
    ClientConnectionInfo, SwarmEvent, SwarmEventType, SwarmMember, SwarmMutationRuntime,
    SwarmState, SwarmTaskProgress, VersionedPlan, broadcast_swarm_plan,
    broadcast_swarm_plan_with_previous, broadcast_swarm_status, fanout_session_event,
    persist_swarm_state_for, queue_soft_interrupt_for_session, record_swarm_event,
    set_member_task_label, truncate_detail, update_member_status, update_member_status_with_report,
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

/// Eligible auto-assignment targets for a swarm task.
///
/// Auto-pick must only land on sessions that will actually *execute* the work
/// without further human action. In a shared swarm there can be many foreign
/// members: independent human TUIs and stale "zombie" sessions left over from
/// other runs. Assigning to those silently strands the task (a human session is
/// never auto-driven; a zombie has no live agent at all) and stalls `run_plan`.
///
/// So a member is only a free worker for *automatic* selection when it is a
/// worker this run owns and can drive:
///
/// - `is_headless`: a spawned in-process worker (always auto-driven), or
/// - owned by the requester (`report_back_to_session_id == req`): a worker this
///   coordinator spawned, including reusable ones that already returned `ready`.
///
/// Everything else (foreign humans, zombies) must be addressed with an explicit
/// `target_session`, which bypasses this filter; this only governs auto-pick.
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
                && is_drivable_auto_worker(member, req_session_id)
        })
        .collect()
}

/// Whether `member` can be auto-assigned a task and be relied on to run it.
/// See [`filter_swarm_agent_candidates`] for the rationale.
fn is_drivable_auto_worker(member: &SwarmMember, req_session_id: &str) -> bool {
    member.is_headless || member.report_back_to_session_id.as_deref() == Some(req_session_id)
}

/// Whether a candidate already holds an incomplete plan assignment.
///
/// Auto-pick must treat such a member as busy rather than reusable: assigning
/// more work to it queues large tasks serially on one agent while the swarm
/// still has spawn capacity (`spawn_if_needed` exists precisely to spawn a
/// fresh agent in that case). `loads` counts non-terminal plan items per
/// assignee (see [`assignment_loads`]).
fn member_has_active_assignment(session_id: &str, loads: &HashMap<String, usize>) -> bool {
    loads.get(session_id).copied().unwrap_or(0) > 0
}

/// Safety-net expiry for auto-pick claims that never reach an explicit
/// release (e.g. a request that dies between the pick and the plan write).
const AUTO_ASSIGN_CLAIM_TTL: std::time::Duration = std::time::Duration::from_secs(15);

/// In-process claims for auto-picked assignment targets, keyed by
/// `swarm_id\nsession_id`.
///
/// The busy check reads the *plan* (`assignment_loads`), but the plan only
/// records an assignment at a write that happens several awaits after the
/// target is picked. Concurrent `assign_task`/`assign_next` requests resolve
/// their targets inside that window (observed live: three auto-picks within
/// ~100ms all stacking onto the same worker), so the pick itself must be a
/// claim: the first resolver wins the member, later ones skip it and fall
/// back to another candidate or to `spawn_if_needed`. Claims are released
/// once the plan write records (or abandons) the assignment; the TTL only
/// reaps leaked claims.
fn auto_assign_claims() -> &'static std::sync::Mutex<HashMap<String, std::time::Instant>> {
    static CLAIMS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, std::time::Instant>>> =
        std::sync::OnceLock::new();
    CLAIMS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn auto_assign_claim_key(swarm_id: &str, session_id: &str) -> String {
    format!("{swarm_id}\n{session_id}")
}

/// Claim `session_id` as an auto-pick target. Returns false when another
/// in-flight request already picked it (and that claim has not expired).
fn try_claim_auto_assign_target(swarm_id: &str, session_id: &str) -> bool {
    let mut claims = auto_assign_claims()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = std::time::Instant::now();
    claims.retain(|_, claimed_at| now.duration_since(*claimed_at) < AUTO_ASSIGN_CLAIM_TTL);
    match claims.entry(auto_assign_claim_key(swarm_id, session_id)) {
        std::collections::hash_map::Entry::Occupied(_) => false,
        std::collections::hash_map::Entry::Vacant(vacant) => {
            vacant.insert(now);
            true
        }
    }
}

fn release_auto_assign_claim(swarm_id: &str, session_id: &str) {
    let mut claims = auto_assign_claims()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    claims.remove(&auto_assign_claim_key(swarm_id, session_id));
}

/// Error for an auto-pick that found no assignable worker. The leading
/// sentence is a stable contract: `spawn_if_needed`/`run_plan` match on it to
/// decide to spawn a fresh agent instead of failing the assignment.
fn no_auto_target_error(busy_skipped: usize) -> String {
    let mut message =
        "No ready or completed swarm agents are available for automatic task assignment."
            .to_string();
    if busy_skipped > 0 {
        message.push_str(&format!(
            " Skipped {busy_skipped} worker(s) that already have an active assignment or an \
             in-flight pick; spawn a fresh agent (spawn_if_needed/prefer_spawn) or wait for one \
             to finish."
        ));
    }
    message
}

/// Pick the first idle, unclaimed candidate from `candidates` (already ranked
/// by the caller) and claim it.
///
/// Members that already hold an incomplete plan assignment are skipped, as are
/// members another in-flight request just picked, so repeated auto-assignments
/// fan out across workers (or trigger a fresh spawn) instead of stacking
/// serially on one agent.
fn select_and_claim_auto_target(
    swarm_id: &str,
    candidates: &[&SwarmMember],
    loads: &HashMap<String, usize>,
) -> Result<String, String> {
    let mut busy_skipped = 0usize;
    for member in candidates {
        if member_has_active_assignment(&member.session_id, loads) {
            busy_skipped += 1;
            continue;
        }
        if try_claim_auto_assign_target(swarm_id, &member.session_id) {
            return Ok(member.session_id.clone());
        }
        busy_skipped += 1;
    }
    Err(no_auto_target_error(busy_skipped))
}

/// A double-assignment conflict: the task is already assigned and its
/// assignee shows recent activity.
struct ActiveAssignmentConflict {
    assignee: String,
    active_ago_ms: u64,
}

/// Guard predicate for double assignment.
///
/// Returns `Some(conflict)` when a direct `assign_task` must be rejected
/// because the item is already assigned and actively worked: it carries an
/// assignee, its status is in-flight (`queued`/`running`, not stale and not
/// terminal), and the assignment shows activity (heartbeat, start, or the
/// assignment itself) within `active_within_ms`.
///
/// Everything else stays assignable so legitimate recovery keeps working:
/// unassigned items, terminal items (explicit re-open), `running_stale` items
/// (the stale-assignee path), and assigned items whose last activity is older
/// than the window (the sweep just has not flipped them to stale yet). An
/// assigned item with no recorded activity at all is treated as stale,
/// mirroring `refresh_swarm_task_staleness`.
fn active_assignment_conflict(
    status: &str,
    assigned_to: Option<&str>,
    progress: Option<&SwarmTaskProgress>,
    now_ms: u64,
    active_within_ms: u64,
) -> Option<ActiveAssignmentConflict> {
    let assignee = assigned_to?;
    if !matches!(status, "queued" | "running") {
        return None;
    }
    let last_activity_ms = progress.and_then(|progress| {
        progress
            .last_heartbeat_unix_ms
            .or(progress.started_at_unix_ms)
            .or(progress.assigned_at_unix_ms)
    })?;
    let active_ago_ms = now_ms.saturating_sub(last_activity_ms);
    (active_ago_ms < active_within_ms).then(|| ActiveAssignmentConflict {
        assignee: assignee.to_string(),
        active_ago_ms,
    })
}

/// Rejection message for [`active_assignment_conflict`], naming the current
/// assignee and pointing at the explicit takeover paths.
fn active_assignment_error(task_id: &str, conflict: &ActiveAssignmentConflict) -> String {
    format!(
        "Task '{}' is already assigned to '{}' (active {}s ago); refusing to double-assign. \
         Use task_control reassign/replace to take over (the displaced worker is told to stand \
         down), retry to re-dispatch to the same assignee, or wait for the assignment to finish \
         or go stale.",
        task_id,
        conflict.assignee,
        conflict.active_ago_ms / 1000
    )
}

/// Decide whether a worker's just-finished turn should auto-mark its assigned
/// node `done`.
///
/// A turn must NOT force-complete a node when the worker decomposed it into a
/// composite (`expanded`): that node is now a synthesis/join point that has to
/// wait for its children (and, in deep mode, its critique/verify gate) before it
/// can close, and it will be re-woken to synthesize. Likewise a node the worker
/// already drove to a terminal status (e.g. via `complete_node`, or that failed)
/// must not be reopened/reclosed. A node that is `queued` at turn end was
/// re-queued mid-turn by someone else (`inject_gap` re-queuing its gate, a
/// reassign, a requeue): it is no longer this worker's to close, and force-doing
/// so would bypass gate artifact validation and strand injected gap nodes. Only
/// a plain, still-running atomic turn auto-completes.
/// What to do with a node whose worker turn ended while the node is still
/// marked running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnEndDisposition {
    /// Light mode: the worker just ran an atomic node; mark it done.
    AutoComplete,
    /// Deep mode, first offense: the worker never called `complete_node`, so
    /// re-queue the node for a fresh worker. Deep completion is artifact-or-
    /// nothing; silently marking it done would bypass artifact validation and
    /// every gate rule.
    RequeueNoArtifact,
    /// Deep mode, repeated offense: fail the node loudly instead of cycling
    /// workers forever. `task_control retry` / `requeue_failed` remain the
    /// recovery paths.
    FailNoArtifact,
    /// The node is already terminal, queued (expanded or gap-injected this
    /// turn), or otherwise not this turn's responsibility.
    LeaveAlone,
}

/// Decide the turn-end disposition for a node.
///
/// Light mode keeps the historical lenient behavior: a running atomic node
/// auto-completes, an expanded composite stays open for synthesis. Deep mode
/// abolishes auto-complete entirely — the typed artifact contract is only real
/// if there is no path to "done" that skips it. A running node at turn end
/// (atomic without `complete_node`, or a re-woken composite synthesis that
/// never synthesized) gets one fresh attempt, then fails.
fn turn_end_disposition(
    is_deep: bool,
    status: &str,
    expanded: bool,
    prior_no_artifact_requeues: u32,
) -> TurnEndDisposition {
    let running = matches!(status, "running" | "running_stale");
    if !running {
        return TurnEndDisposition::LeaveAlone;
    }
    if is_deep {
        return if prior_no_artifact_requeues == 0 {
            TurnEndDisposition::RequeueNoArtifact
        } else {
            TurnEndDisposition::FailNoArtifact
        };
    }
    if expanded {
        // The worker decomposed the node; it must stay open to synthesize later.
        return TurnEndDisposition::LeaveAlone;
    }
    TurnEndDisposition::AutoComplete
}

#[cfg(test)]
fn turn_end_should_auto_complete(status: &str, expanded: bool) -> bool {
    turn_end_disposition(false, status, expanded, 0) == TurnEndDisposition::AutoComplete
}

/// Assignment content for a (re-)dispatched node.
///
/// For a re-woken composite (`is_composite_synthesis`), the node's original
/// content is the now-stale decomposition brief, so replace it with an explicit
/// synthesis instruction that tells the planner to integrate its children and
/// finish with `complete_node`. Otherwise the original content is used verbatim.
fn composite_synthesis_content(
    item_id: &str,
    raw_content: &str,
    is_composite_synthesis: bool,
) -> String {
    if is_composite_synthesis {
        format!(
            "Synthesis turn for composite node '{item_id}'. Its children (and the deep-mode \
             critique/verify gate) are complete; their outputs are provided below. Read them, \
             write one synthesized result, and finish by calling `swarm complete_node` with \
             node_id=\"{item_id}\" and an artifact summarizing the integrated findings. Do NOT \
             call expand_node again. Original brief: {raw_content}"
        )
    } else {
        raw_content.to_string()
    }
}

#[derive(Clone, Debug)]
struct TaskSnapshot {
    content: String,
    status: String,
    assigned_to: Option<String>,
    progress: Option<SwarmTaskProgress>,
}

/// Attach the deep-mode execution contract to an assignment's content when the
/// plan is running deep.
///
/// This is the mechanism that makes the swarm's large agent budget actually get
/// used: every dispatched node carries an in-band directive telling the worker
/// it may `expand_node` into MANY parallel children and must close with a typed
/// artifact, and every gate carries the `inject_gap`-or-pass contract. Without
/// it, only the seeding session (which ran at `swarm-deep` effort) knows the
/// deep workflow, and freshly spawned workers execute serially. A re-woken
/// composite synthesis keeps its dedicated synthesis brief instead, since
/// re-expanding there would loop.
fn deep_mode_assignment_content(
    plan: &VersionedPlan,
    item_id: &str,
    is_composite_synthesis: bool,
    content: &str,
) -> String {
    if !plan.mode.eq_ignore_ascii_case("deep") || is_composite_synthesis {
        return content.to_string();
    }
    let is_gate = plan
        .node_meta
        .get(item_id)
        .map(|meta| meta.is_gate)
        .unwrap_or(false);
    if is_gate {
        // The gate's audit scope is its non-gate dependencies (composite gates
        // audit their siblings; the root gate audits the whole root set). The
        // server rejects a pass whose artifact does not account for each of
        // these by id, so the directive enumerates them up front instead of
        // letting the gate discover the rejection by trial and error.
        let audited_ids: Vec<String> = plan
            .items
            .iter()
            .find(|item| item.id == item_id)
            .map(|item| {
                item.blocked_by
                    .iter()
                    .filter(|dep| {
                        plan.node_meta
                            .get(dep.as_str())
                            .map(|meta| !meta.is_gate)
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // Completed scope nodes whose artifacts self-reported low confidence:
        // the strictest debts, named as priority probe targets.
        let audited: HashSet<&str> = audited_ids.iter().map(String::as_str).collect();
        let low_confidence_siblings: Vec<String> =
            jcode_plan::bridge::low_confidence_completed_ids(plan)
                .into_iter()
                .filter(|id| audited.contains(id.as_str()))
                .collect();
        jcode_swarm_core::append_deep_gate_instructions(
            content,
            item_id,
            &audited_ids,
            &low_confidence_siblings,
        )
    } else {
        jcode_swarm_core::append_deep_node_instructions(content, item_id)
    }
}

async fn task_snapshot_for(
    swarm_id: &str,
    task_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Option<TaskSnapshot> {
    let plans = swarm_plans.read().await;
    let plan = plans.get(swarm_id)?;
    let item = plan.items.iter().find(|item| item.id == task_id)?;
    // Hydrate with forward dataflow from completed upstream dependencies so
    // resume/start/wake re-injects the same artifact context an initial
    // assignment would carry, then attach the deep-mode contract the same way
    // the initial assignment path does.
    let hydrated = jcode_plan::bridge::hydrate_assignment(plan, task_id, &item.content);
    let is_composite_synthesis = plan
        .node_meta
        .get(task_id)
        .map(|meta| meta.expanded && !meta.is_gate)
        .unwrap_or(false);
    Some(TaskSnapshot {
        content: deep_mode_assignment_content(plan, task_id, is_composite_synthesis, &hydrated),
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

/// Re-queue a task on its existing assignee for a task-control restart
/// (currently only `resume` of a running/stale task reaches this).
///
/// The prior run's history (`started_at`, heartbeats, checkpoints, last
/// detail) is preserved rather than replaced: the requeue is a lifecycle
/// transition of the same assignment, and wiping the record would blind
/// staleness monitors and salvage flows to everything the previous run did.
/// Only the assignment-scoped fields are refreshed, and the terminal/stale
/// markers are cleared because the task is queued again.
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
    let progress = plan.task_progress.entry(task_id.to_string()).or_default();
    progress.assigned_session_id = Some(assignee_session.to_string());
    progress.assignment_summary = Some(truncate_detail(&assignment_summary, 120));
    progress.assigned_at_unix_ms = Some(now_ms);
    progress.completed_at_unix_ms = None;
    progress.stale_since_unix_ms = None;
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

    select_and_claim_auto_target(swarm_id, &candidates, &assignment_counts)
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

/// Test-only re-export of the private assignment resolver so the e2e tests can
/// assert composite re-wake routing without going through the full assign path.
#[cfg(test)]
pub(super) async fn resolve_assignment_target_for_task_test_hook(
    req_session_id: &str,
    swarm_id: &str,
    task_id: &str,
    requested_target: Option<&str>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Result<String, String> {
    resolve_assignment_target_for_task(
        req_session_id,
        swarm_id,
        task_id,
        requested_target,
        swarm_members,
        swarm_plans,
    )
    .await
}

async fn next_unassigned_runnable_task_id(
    swarm_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Option<String> {
    let plans = swarm_plans.read().await;
    let plan = plans.get(swarm_id)?;
    next_unassigned_runnable_item_id(plan)
}

/// Like [`next_unassigned_runnable_task_id`], but when no unassigned runnable
/// item exists, look for a runnable item *stranded* on a dead assignee (a
/// member whose lifecycle status is terminal, or that is no longer a swarm
/// member at all) and reclaim it so the caller can dispatch it normally.
///
/// This is the requeue-pickup path: `task_control retry` re-dispatches to the
/// existing assignee, so when that session died (e.g. an auth-failure wave)
/// the node sits `queued` + assigned-to-a-corpse, invisible to
/// `next_unassigned_runnable_item_id`, and a still-running `run_plan` driver
/// reports "No runnable unassigned tasks" forever. Reclaims are capped
/// per-node by [`crate::plan::MAX_DEAD_ASSIGNEE_RECLAIMS`] to respect the
/// repeat-failure policy: beyond the cap only explicit `retry`/`assign_task`
/// move the node.
async fn next_runnable_task_id_reclaiming_stranded(
    swarm_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Option<String> {
    if let Some(task_id) = next_unassigned_runnable_task_id(swarm_id, swarm_plans).await {
        return Some(task_id);
    }

    // Snapshot member liveness first so the plans write lock is not held
    // across the members read lock (avoids lock-order inversions with paths
    // that lock members before plans).
    let member_statuses: HashMap<String, String> = {
        let members = swarm_members.read().await;
        members
            .values()
            .filter(|member| member.swarm_id.as_deref() == Some(swarm_id))
            .map(|member| (member.session_id.clone(), member.status.clone()))
            .collect()
    };
    let assignee_is_dead = move |session_id: &str| -> bool {
        match member_statuses.get(session_id) {
            Some(status) => matches!(status.as_str(), "failed" | "stopped" | "crashed"),
            // Not a member of this swarm anymore: nothing can drive it.
            None => true,
        }
    };

    let mut plans = swarm_plans.write().await;
    let plan = plans.get_mut(swarm_id)?;
    let stranded_id = crate::plan::next_stranded_runnable_item_id(plan, &assignee_is_dead)?;
    if crate::plan::reclaim_stranded_assignment(plan, &stranded_id) {
        crate::logging::info(&format!(
            "swarm {}: reclaimed stranded task '{}' from dead assignee for re-dispatch",
            swarm_id, stranded_id
        ));
        Some(stranded_id)
    } else {
        None
    }
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

    // Composite owner re-wake affinity: a composite (expanded) node that has
    // become runnable again is the synthesis/join step. Prefer routing it back to
    // the agent that planned the decomposition (its recorded owner), so the same
    // planner integrates its children rather than a fresh worker. Only honor this
    // when that owner is still a live, eligible swarm member.
    {
        let plans = swarm_plans.read().await;
        if let Some(plan) = plans.get(swarm_id) {
            let planner = plan.node_meta.get(task_id).and_then(|meta| {
                (meta.expanded && !meta.is_gate)
                    .then(|| meta.planner.clone())
                    .flatten()
            });
            if let Some(owner) = planner
                && owner != req_session_id
            {
                let members = swarm_members.read().await;
                let owner_eligible =
                    filter_swarm_agent_candidates(&members, req_session_id, swarm_id)
                        .iter()
                        .any(|member| member.session_id == owner);
                // The planner affinity deliberately bypasses the busy check (the
                // synthesis needs its decomposition context), but not the race
                // claim: if another in-flight pick just took this member, fall
                // through to normal selection instead of double-booking it.
                if owner_eligible && try_claim_auto_assign_target(swarm_id, &owner) {
                    return Ok(owner);
                }
            }
        }
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

    select_and_claim_auto_target(swarm_id, &candidates, &affinities.loads)
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
        set_member_task_label(&target_session, &assignment_text, &swarm_members).await;
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
                let mut applied_disposition = TurnEndDisposition::LeaveAlone;
                {
                    let now_ms = now_unix_ms();
                    let mut plans = swarm_plans.write().await;
                    if let Some(plan) = plans.get_mut(&swarm_id)
                        && let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id)
                    {
                        // A worker turn ends in one of three ways for its node:
                        //  1. it decomposed the node via `expand_node` -> the node is
                        //     now a composite synthesis/join point that must stay
                        //     in-progress until its children (and deep-mode gate)
                        //     finish; it is re-woken later to synthesize.
                        //  2. it already finished the node via `complete_node` -> the
                        //     node is terminal and owned by no one.
                        //  3. it just ran and the node is still `running`.
                        // Case 3 is mode-dependent: light mode auto-completes
                        // (cheap fan-out, artifacts optional), deep mode never
                        // does — a deep node only closes through `complete_node`
                        // with a validated artifact, so an artifact-less turn is
                        // re-queued once to a fresh worker and then failed.
                        let expanded = plan
                            .node_meta
                            .get(&task_id)
                            .map(|m| m.expanded && !m.is_gate)
                            .unwrap_or(false);
                        let is_deep = plan.mode.eq_ignore_ascii_case("deep");
                        let prior_requeues = plan
                            .task_progress
                            .get(&task_id)
                            .and_then(|p| p.no_artifact_requeues)
                            .unwrap_or(0);
                        match turn_end_disposition(is_deep, &item.status, expanded, prior_requeues)
                        {
                            TurnEndDisposition::AutoComplete => {
                                applied_disposition = TurnEndDisposition::AutoComplete;
                                item.status = "done".to_string();
                                let progress =
                                    plan.task_progress.entry(task_id.clone()).or_default();
                                progress.last_heartbeat_unix_ms = Some(now_ms);
                                progress.last_checkpoint_unix_ms = Some(now_ms);
                                progress.checkpoint_summary = Some("task completed".to_string());
                                progress.completed_at_unix_ms = Some(now_ms);
                                progress.stale_since_unix_ms = None;
                                progress.checkpoint_count =
                                    Some(progress.checkpoint_count.unwrap_or(0) + 1);
                                plan.version += 1;
                            }
                            TurnEndDisposition::RequeueNoArtifact => {
                                applied_disposition = TurnEndDisposition::RequeueNoArtifact;
                                item.status = "queued".to_string();
                                item.assigned_to = None;
                                let progress =
                                    plan.task_progress.entry(task_id.clone()).or_default();
                                progress.assigned_session_id = None;
                                progress.no_artifact_requeues = Some(prior_requeues + 1);
                                progress.last_heartbeat_unix_ms = Some(now_ms);
                                progress.last_checkpoint_unix_ms = Some(now_ms);
                                progress.checkpoint_summary = Some(
                                    "requeued: deep-mode turn ended without a complete_node \
                                     artifact"
                                        .to_string(),
                                );
                                progress.stale_since_unix_ms = None;
                                progress.checkpoint_count =
                                    Some(progress.checkpoint_count.unwrap_or(0) + 1);
                                plan.version += 1;
                            }
                            TurnEndDisposition::FailNoArtifact => {
                                applied_disposition = TurnEndDisposition::FailNoArtifact;
                                item.status = "failed".to_string();
                                let progress =
                                    plan.task_progress.entry(task_id.clone()).or_default();
                                progress.last_heartbeat_unix_ms = Some(now_ms);
                                progress.last_checkpoint_unix_ms = Some(now_ms);
                                progress.checkpoint_summary = Some(
                                    "failed: repeated deep-mode turns ended without a \
                                     complete_node artifact"
                                        .to_string(),
                                );
                                progress.completed_at_unix_ms = Some(now_ms);
                                progress.stale_since_unix_ms = None;
                                progress.checkpoint_count =
                                    Some(progress.checkpoint_count.unwrap_or(0) + 1);
                                plan.version += 1;
                            }
                            TurnEndDisposition::LeaveAlone => {}
                        }
                    }
                }
                let swarm_state = SwarmState {
                    members: Arc::clone(&swarm_members),
                    swarms_by_id: Arc::clone(&swarms_by_id),
                    plans: Arc::clone(&swarm_plans),
                    coordinators: Arc::clone(&swarm_coordinators),
                };
                persist_swarm_state_for(&swarm_id, &swarm_state).await;
                let plan_reason = match applied_disposition {
                    TurnEndDisposition::RequeueNoArtifact => "task_requeued_no_artifact",
                    TurnEndDisposition::FailNoArtifact => "task_failed_no_artifact",
                    _ => "task_completed",
                };
                broadcast_swarm_plan_with_previous(
                    &swarm_id,
                    Some(plan_reason.to_string()),
                    Some(&previous_items),
                    &swarm_plans,
                    &swarm_members,
                    &swarms_by_id,
                )
                .await;
                // The worker's member status reflects its own turn (it ran to
                // completion) even when its node was requeued/failed for missing
                // an artifact: lifecycle and node state are separate axes, and a
                // "completed" worker is reusable for the requeued node.
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

/// How an assign_task request interacts with the durable mutation dedup layer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AssignDedupMode {
    /// Direct client requests: an identical request within the final-state
    /// TTL replays the persisted response instead of re-dispatching. This
    /// absorbs client-side retries of the same logical request.
    ReplayFinal,
    /// Task-control-driven dispatches (retry/reassign/replace/salvage): each
    /// invocation is a deliberate new attempt, so a persisted success from a
    /// previous identical attempt must not swallow the re-dispatch (a worker
    /// failing within the TTL would otherwise make the coordinator's retry a
    /// silent no-op). Concurrent in-flight duplicates still coalesce.
    AlwaysDispatch,
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
    handle_comm_assign_task_with_mode(
        id,
        req_session_id,
        target_session,
        task_id,
        message,
        AssignDedupMode::ReplayFinal,
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
    reason = "task assignment coordinates sessions, interrupts, connections, swarm plan state, and event history"
)]
async fn handle_comm_assign_task_with_mode(
    id: u64,
    req_session_id: String,
    target_session: Option<String>,
    task_id: Option<String>,
    message: Option<String>,
    dedup_mode: AssignDedupMode,
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

    let swarm_id = match require_plan_driver_swarm(
        id,
        &req_session_id,
        "Only the coordinator can assign tasks.",
        client_event_tx,
        swarm_members,
        swarm_plans,
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
    let mutation_state = match dedup_mode {
        AssignDedupMode::ReplayFinal => {
            begin_swarm_mutation_or_replay(
                swarm_mutation_runtime,
                &mutation_key,
                "assign_task",
                &req_session_id,
                id,
                client_event_tx,
            )
            .await
        }
        AssignDedupMode::AlwaysDispatch => {
            begin_swarm_mutation_no_replay(
                swarm_mutation_runtime,
                &mutation_key,
                "assign_task",
                &req_session_id,
                id,
                client_event_tx,
            )
            .await
        }
    };
    let Some(mutation_state) = mutation_state else {
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
        // Double-assignment guard: a direct assign_task naming an item that is
        // already assigned and actively worked is a coordination bug (observed
        // live: run_plan dispatched a node, then an explicit assign_task
        // silently re-assigned it to a second worker and both edited the same
        // files for minutes). Deliberate re-dispatch goes through task_control
        // (retry/reassign/replace/salvage), which uses AlwaysDispatch and is
        // exempt. Auto-selection only picks unassigned items, so only an
        // explicit task_id can conflict.
        let conflict_reason = if dedup_mode == AssignDedupMode::ReplayFinal {
            requested_task_id.as_deref().and_then(|task_id| {
                plan.items
                    .iter()
                    .find(|item| item.id == task_id)
                    .and_then(|item| {
                        active_assignment_conflict(
                            &item.status,
                            item.assigned_to.as_deref(),
                            plan.task_progress.get(task_id),
                            now_ms,
                            swarm_task_stale_after().as_millis() as u64,
                        )
                    })
                    .map(|conflict| active_assignment_error(task_id, &conflict))
            })
        } else {
            None
        };
        let blocked_reason = conflict_reason.or_else(|| {
            requested_task_id
                .as_deref()
                .and_then(|task_id| explicit_task_blocked_reason(plan, task_id))
        });
        let found_idx = if blocked_reason.is_some() {
            None
        } else {
            selected_task_id.as_ref().and_then(|selected_task_id| {
                plan.items
                    .iter()
                    .position(|item| item.id == *selected_task_id)
            })
        };
        if let Some(found_idx) = found_idx {
            // Resolve identity + forward-dataflow context before taking the
            // mutable borrow, so hydration can read sibling artifacts immutably.
            let item_id = plan.items[found_idx].id.clone();
            let raw_content = plan.items[found_idx].content.clone();
            // A re-woken composite is the synthesis/join step: its original content
            // was the (now-stale) decomposition brief, so replace it with an explicit
            // synthesis instruction. Without this the planner replays the old "expand
            // me" prompt and reports instead of calling `complete_node`, leaving the
            // composite `running_stale` forever.
            let is_composite_synthesis = plan
                .node_meta
                .get(&item_id)
                .map(|meta| meta.expanded && !meta.is_gate)
                .unwrap_or(false);
            let effective_content =
                composite_synthesis_content(&item_id, &raw_content, is_composite_synthesis);
            let hydrated =
                jcode_plan::bridge::hydrate_assignment(plan, &item_id, &effective_content);
            let content =
                deep_mode_assignment_content(plan, &item_id, is_composite_synthesis, &hydrated);

            // Index resolved under this same plan lock, so it stays valid.
            let item = &mut plan.items[found_idx];
            item.assigned_to = Some(target_session.clone());
            item.status = "queued".to_string();
            plan.task_progress.insert(
                item_id.clone(),
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
                Some(item_id.clone()),
                Some(content),
                plan.participants.clone(),
                plan.items.len(),
                None,
            )
        } else {
            (None, None, HashSet::new(), 0, blocked_reason)
        }
    };

    // The plan write above either recorded the assignment (from here
    // `assignment_loads` marks the target busy) or abandoned it (no runnable
    // task / blocked), so an auto-picked target's in-flight claim is released
    // in both cases. Explicit targets never claimed; `assign_next` releases
    // its own pre-claimed pick after this handler returns.
    if requested_target_session.is_none() {
        release_auto_assign_claim(&swarm_id, &target_session);
    }

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
    let assignment_text = combine_assignment_text(&content, message.as_deref());
    set_member_task_label(&target_session, &assignment_text, swarm_members).await;
    update_member_status(
        &target_session,
        "queued",
        Some(truncate_detail(&assignment_text, 120)),
        swarm_members,
        swarms_by_id,
        Some(event_history),
        Some(event_counter),
        Some(swarm_event_tx),
    )
    .await;

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
                tldr: None,
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
                    tldr: None,
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
    model: Option<String>,
    effort: Option<String>,
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
        let swarm_id = match require_plan_driver_swarm(
            id,
            &req_session_id,
            "Only the coordinator can assign tasks.",
            client_event_tx,
            swarm_members,
            swarm_plans,
            swarm_coordinators,
        )
        .await
        {
            Some(swarm_id) => swarm_id,
            None => return,
        };

        let Some(selected_task_id) =
            next_runnable_task_id_reclaiming_stranded(&swarm_id, swarm_plans, swarm_members).await
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
            // A prefer_spawn run can reach here with a successfully resolved
            // (and therefore claimed) reuse target it will never use; free it
            // for concurrent picks.
            if let Ok(unused_target) = &preferred_target {
                release_auto_assign_claim(&swarm_id, unused_target);
            }
            match super::comm_session::spawn_swarm_agent(
                &req_session_id,
                &swarm_id,
                working_dir.clone(),
                None,
                None,
                model.clone(),
                effort.clone(),
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
                client_connections,
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
                    Some(target_session.clone()),
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
                // The pick was claimed at resolve time; by now the assignment
                // either landed in the plan (busy via assignment_loads) or was
                // rejected, so the in-flight claim is spent either way.
                release_auto_assign_claim(&swarm_id, &target_session);
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

    let swarm_id = match require_plan_driver_swarm(
        id,
        &req_session_id,
        "Only the coordinator can control assigned tasks.",
        client_event_tx,
        swarm_members,
        swarm_plans,
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
            // Validate the assignee is actually available BEFORE mutating any
            // plan state. Resuming a plain-'running' task used to requeue it
            // (flipping it to 'queued' and rewriting its progress record)
            // first and only then discover the agent was missing or busy,
            // leaving a live task falsely queued with its run history mangled
            // even though the request was rejected.
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
            handle_comm_assign_task_with_mode(
                id,
                req_session_id,
                Some(assignee),
                Some(task_id),
                Some(retry_note),
                AssignDedupMode::AlwaysDispatch,
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

            let displaced_task_id = task_id.clone();
            let displaced_new_target = new_target.clone();
            let displaced_req_session = req_session_id.clone();
            handle_comm_assign_task_with_mode(
                id,
                req_session_id,
                Some(new_target),
                Some(task_id),
                forwarded_message,
                AssignDedupMode::AlwaysDispatch,
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

            // Tell the displaced worker to stand down, but only when the
            // takeover actually landed (the re-dispatch above can still fail,
            // e.g. on an unknown target session, and then the prior assignee
            // keeps the task). Without this the displaced worker keeps
            // editing the same files as its replacement until a human DMs it.
            let takeover_landed = {
                let swarm_plans = swarm_plans.read().await;
                swarm_plans
                    .get(&swarm_id)
                    .and_then(|plan| plan.items.iter().find(|item| item.id == displaced_task_id))
                    .is_some_and(|item| {
                        item.assigned_to.as_deref() == Some(displaced_new_target.as_str())
                    })
            };
            if takeover_landed {
                let stand_down = format!(
                    "Task '{}' has been handed off to '{}' by the coordinator ({}). Stop working \
                     on it immediately: do not make further edits or commits for that task. If \
                     you have uncommitted progress worth keeping, note it in a brief message to \
                     the coordinator, then stand down.",
                    displaced_task_id,
                    displaced_new_target,
                    action.as_str()
                );
                let _ = queue_soft_interrupt_for_session(
                    &assignee,
                    stand_down.clone(),
                    true,
                    SoftInterruptSource::System,
                    soft_interrupt_queues,
                    sessions,
                )
                .await;
                if let Some(member) = swarm_members.read().await.get(&assignee) {
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: displaced_req_session,
                        from_name: None,
                        notification_type: NotificationType::Message {
                            scope: Some("dm".to_string()),
                            channel: None,
                            tldr: None,
                        },
                        message: stand_down,
                    });
                }
            }
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

/// Authorize a session to drive task dispatch for its swarm plan.
///
/// Light mode keeps the single-coordinator rule: a coordinator is the one driver,
/// which matches the cheap fan-out preset. Deep mode follows the task-DAG
/// ownership model (see `docs/SWARM_TASK_GRAPH.md` section 2): the plan is a tree
/// of ownership over a graph, and the agent that seeded/participates in the graph
/// must be able to dispatch it even when another session already holds the
/// swarm-level coordinator slot. Without this, a deep-mode agent that joins a
/// shared swarm can seed a graph but is then blocked from spawning/assigning any
/// of it, so nothing ever runs.
///
/// Returns the swarm id when the caller is the coordinator, or (deep mode only) a
/// participant of the swarm's plan.
async fn require_plan_driver_swarm(
    id: u64,
    req_session_id: &str,
    permission_error: &str,
    client_event_tx: &mpsc::UnboundedSender<ServerEvent>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) -> Option<String> {
    let swarm_id = {
        let members = swarm_members.read().await;
        members
            .get(req_session_id)
            .and_then(|member| member.swarm_id.clone())
    };
    let Some(swarm_id) = swarm_id else {
        let _ = client_event_tx.send(ServerEvent::Error {
            id,
            message: "Not in a swarm.".to_string(),
            retry_after_secs: None,
        });
        return None;
    };

    let is_coordinator = {
        let coordinators = swarm_coordinators.read().await;
        coordinators
            .get(&swarm_id)
            .map(|coordinator| coordinator == req_session_id)
            .unwrap_or(false)
    };
    if is_coordinator {
        return Some(swarm_id);
    }

    // Deep mode: any participant of the plan may drive its own task graph.
    let is_deep_participant = {
        let plans = swarm_plans.read().await;
        plans
            .get(&swarm_id)
            .map(|plan| {
                jcode_plan::bridge::parse_mode(&plan.mode) == jcode_plan::dag::Mode::Deep
                    && plan.participants.contains(req_session_id)
            })
            .unwrap_or(false)
    };
    if is_deep_participant {
        return Some(swarm_id);
    }

    let _ = client_event_tx.send(ServerEvent::Error {
        id,
        message: permission_error.to_string(),
        retry_after_secs: None,
    });
    None
}
