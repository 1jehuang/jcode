#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::{Tool, ToolContext, ToolOutput};
use crate::background::TaskResult;
use crate::plan::PlanItem;
use crate::protocol::{
    AgentInfo, AgentStatusSnapshot, AwaitedMemberStatus, CommDeliveryMode, ContextEntry,
    HistoryMessage, PlanGraphStatus, Request, ServerEvent, SwarmChannelInfo, TaskGraphNodeSpec,
    ToolCallSummary, comm_cleanup_candidate_session_ids, default_comm_await_target_statuses,
    default_comm_cleanup_target_statuses, default_comm_run_await_statuses,
    format_comm_awaited_members_with_reports, format_comm_channels, format_comm_context_entries,
    format_comm_context_history, format_comm_members, format_comm_plan_followup,
    format_comm_plan_status, format_comm_status_snapshot, format_comm_tool_summary,
    latest_assistant_comm_report, resolve_optional_comm_target_session,
};
use anyhow::Result;
use async_trait::async_trait;
use jcode_swarm_core::validate_swarm_tldr;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

const REQUEST_ID: u64 = 1;

/// Default number of workers `run_plan` keeps active at once for a **light**-mode
/// plan. Light mode is the cheap fan-out preset, so this stays small. Deep mode
/// instead uses `agents.swarm_max_concurrent_agents` (high, configurable).
const LIGHT_MODE_DEFAULT_CONCURRENCY: usize = 4;

mod transport;
use transport::{send_request, send_request_with_timeout};

fn fresh_spawn_request_nonce(ctx: &ToolContext) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{}-{}-{}", ctx.session_id, ctx.message_id, now_ms)
}

fn check_error(response: &ServerEvent) -> Option<&str> {
    if let ServerEvent::Error { message, .. } = response {
        Some(message)
    } else {
        None
    }
}

fn ensure_success(response: &ServerEvent) -> Result<()> {
    if let Some(message) = check_error(response) {
        Err(anyhow::anyhow!(message.to_string()))
    } else {
        Ok(())
    }
}

fn seed_node_id_collision(response: &ServerEvent) -> Option<&str> {
    let message = check_error(response)?;
    let (_, tail) = message.split_once("duplicate node id '")?;
    let (id, _) = tail.split_once('\'')?;
    (!id.is_empty()).then_some(id)
}

fn plan_graph_node_ids(summary: &PlanGraphStatus) -> HashSet<String> {
    summary
        .ready_ids
        .iter()
        .chain(&summary.blocked_ids)
        .chain(&summary.active_ids)
        .chain(&summary.completed_ids)
        .chain(&summary.failed_ids)
        .chain(&summary.cycle_ids)
        .chain(&summary.unresolved_dependency_ids)
        .cloned()
        .collect()
}

fn seed_retry_scope(ctx: &ToolContext) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ctx.session_id.hash(&mut hasher);
    ctx.message_id.hash(&mut hasher);
    format!("seed-{:08x}", hasher.finish() as u32)
}

/// Rename only seed ids that collide with the existing durable plan, then rewrite
/// intra-batch dependency edges to follow them. The scope is stable for a tool
/// turn, so retrying the same call produces the same ids and is itself idempotent.
fn remap_conflicting_seed_nodes(
    nodes: &[TaskGraphNodeSpec],
    occupied: &HashSet<String>,
    conflicting_id: &str,
    scope: &str,
) -> (Vec<TaskGraphNodeSpec>, Vec<(String, String)>) {
    let original_ids: HashSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
    let mut reserved = occupied.clone();
    reserved.extend(original_ids.iter().map(|id| (*id).to_string()));
    let mut mapping = HashMap::<String, String>::new();

    if occupied.contains(conflicting_id) && nodes.iter().any(|node| node.id == conflicting_id) {
        let node_id = conflicting_id.to_string();
        let base = format!("{conflicting_id}::{scope}");
        let mut candidate = base.clone();
        let mut discriminator = 2usize;
        while reserved.contains(&candidate) {
            candidate = format!("{base}-{discriminator}");
            discriminator += 1;
        }
        reserved.insert(candidate.clone());
        mapping.insert(node_id, candidate);
    }

    let remapped = nodes
        .iter()
        .cloned()
        .map(|mut node| {
            if let Some(id) = mapping.get(&node.id) {
                node.id = id.clone();
            }
            for dependency in &mut node.depends_on {
                if let Some(id) = mapping.get(dependency) {
                    *dependency = id.clone();
                }
            }
            node
        })
        .collect();
    let changes = nodes
        .iter()
        .filter_map(|node| {
            mapping
                .get(&node.id)
                .map(|mapped| (node.id.clone(), mapped.clone()))
        })
        .collect();
    (remapped, changes)
}

fn format_seed_remaps(changes: &[(String, String)]) -> String {
    changes
        .iter()
        .map(|(from, to)| format!("{from} -> {to}"))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn fetch_plan_status(session_id: &str) -> Result<PlanGraphStatus> {
    let request = Request::CommPlanStatus {
        id: REQUEST_ID,
        session_id: session_id.to_string(),
    };
    match send_request(request).await {
        Ok(ServerEvent::CommPlanStatusResponse { summary, .. }) => Ok(summary),
        Ok(response) => {
            ensure_success(&response)?;
            Err(anyhow::anyhow!("No plan status returned."))
        }
        Err(e) => Err(anyhow::anyhow!("Failed to get plan status: {}", e)),
    }
}

fn format_plan_followup(summary: &PlanGraphStatus) -> String {
    format_comm_plan_followup(summary)
}

fn default_cleanup_target_statuses() -> Vec<String> {
    default_comm_cleanup_target_statuses()
}

fn default_run_await_statuses() -> Vec<String> {
    default_comm_run_await_statuses()
}

fn cleanup_candidate_session_ids(
    owner_session_id: &str,
    members: &[AgentInfo],
    target_status: &[String],
    requested_session_ids: &[String],
    force: bool,
) -> Vec<String> {
    comm_cleanup_candidate_session_ids(
        owner_session_id,
        members,
        target_status,
        requested_session_ids,
        force,
    )
}

fn auto_assignment_needs_spawn(response: &ServerEvent) -> bool {
    check_error(response).is_some_and(|message| {
        message.contains(
            "No ready or completed swarm agents are available for automatic task assignment",
        )
    })
}

async fn fetch_swarm_members(session_id: &str) -> Result<Vec<AgentInfo>> {
    let request = Request::CommList {
        id: REQUEST_ID,
        session_id: session_id.to_string(),
    };
    match send_request(request).await {
        Ok(ServerEvent::CommMembers { members, .. }) => Ok(members),
        Ok(response) => {
            ensure_success(&response)?;
            Ok(Vec::new())
        }
        Err(e) => Err(anyhow::anyhow!("Failed to list swarm members: {}", e)),
    }
}

fn swarm_member_is_in_flight(member: &AgentInfo) -> bool {
    matches!(
        member.status.as_deref(),
        Some("queued" | "running" | "running_stale")
    )
}

fn coordination_in_flight_count(
    summary: &PlanGraphStatus,
    members: &[AgentInfo],
    current_session_id: &str,
) -> usize {
    summary.active_ids.len().max(
        members
            .iter()
            .filter(|member| member.session_id != current_session_id)
            .filter(|member| swarm_member_is_in_flight(member))
            .filter(|member| swarm_member_is_drivable_worker(member, current_session_id))
            .count(),
    )
}

/// Sessions `run_plan` should await as genuinely in-flight on *this* plan.
///
/// A member counts only when it is both in-flight (`queued`/`running`) **and** a
/// drivable worker for this run: headless, or owned by the coordinator
/// (`report_back_to_session_id == coordinator`). This deliberately excludes
/// independent, client-attached human sessions that merely share the swarm and
/// happen to sit in a `queued` status. Awaiting those would hang `run_plan`
/// forever even though every plan task is already terminal (they are never auto
/// driven), which is exactly the stall this scoping prevents.
///
/// Pure over an already-fetched member list so the coordination loop can reuse
/// one `CommList` snapshot for both in-flight scoping and failure-wave
/// classification.
fn in_flight_swarm_session_ids(members: &[AgentInfo], coordinator_session_id: &str) -> Vec<String> {
    members
        .iter()
        .filter(|member| member.session_id != coordinator_session_id)
        .filter(|member| swarm_member_is_in_flight(member))
        .filter(|member| swarm_member_is_drivable_worker(member, coordinator_session_id))
        .map(|member| member.session_id.clone())
        .collect()
}

/// Fetch-and-filter convenience over [`in_flight_swarm_session_ids`] for call
/// sites that do not otherwise need the member snapshot.
async fn fetch_in_flight_swarm_sessions(session_id: &str) -> Result<Vec<String>> {
    let members = fetch_swarm_members(session_id).await?;
    Ok(in_flight_swarm_session_ids(&members, session_id))
}

/// Whether `member` is a worker `run_plan` can rely on to autonomously execute an
/// assignment (and therefore one it is safe to await): a spawned headless worker,
/// or one owned by the coordinator that issued the run. Foreign client-attached
/// sessions are not drivable and must not gate `run_plan` completion.
fn swarm_member_is_drivable_worker(member: &AgentInfo, coordinator_session_id: &str) -> bool {
    member.is_headless.unwrap_or(false)
        || member.report_back_to_session_id.as_deref() == Some(coordinator_session_id)
}

async fn cleanup_swarm_workers(ctx: &ToolContext, params: &CommunicateInput) -> Result<String> {
    let members = fetch_swarm_members(&ctx.session_id).await?;
    let target_status = params
        .target_status
        .clone()
        .unwrap_or_else(default_cleanup_target_statuses);
    let session_ids = params.session_ids.clone().unwrap_or_default();
    let force = params.force.unwrap_or(false);
    let candidates = cleanup_candidate_session_ids(
        &ctx.session_id,
        &members,
        &target_status,
        &session_ids,
        force,
    );

    if candidates.is_empty() {
        return Ok(format!(
            "No cleanup candidates found. Default cleanup only stops sessions spawned by this coordinator with status in [{}].",
            target_status.join(", ")
        ));
    }

    Ok(stop_swarm_sessions(ctx, candidates, force).await.describe())
}

/// Result of stopping a batch of swarm sessions: which stops succeeded and
/// which failed (with reasons). Split from the human-readable formatting so
/// callers like the mid-run capacity recovery can count freed slots.
struct WorkerCleanupOutcome {
    stopped: Vec<String>,
    failed: Vec<String>,
}

impl WorkerCleanupOutcome {
    fn describe(&self) -> String {
        let mut output = String::new();
        if self.stopped.is_empty() {
            output.push_str("Stopped no swarm workers.");
        } else {
            output.push_str(&format!(
                "Stopped {} swarm worker(s): {}",
                self.stopped.len(),
                self.stopped.join(", ")
            ));
        }
        if !self.failed.is_empty() {
            output.push_str(&format!(
                "\nFailed to stop {} worker(s): {}",
                self.failed.len(),
                self.failed.join(", ")
            ));
        }
        output
    }
}

async fn stop_swarm_sessions(
    ctx: &ToolContext,
    candidates: Vec<String>,
    force: bool,
) -> WorkerCleanupOutcome {
    let mut stopped = Vec::new();
    let mut failed = Vec::new();
    for target in candidates {
        let request = Request::CommStop {
            id: REQUEST_ID,
            session_id: ctx.session_id.clone(),
            target_session: target.clone(),
            force: Some(force),
        };
        match send_request(request).await {
            Ok(response) => match ensure_success(&response) {
                Ok(()) => stopped.push(target),
                Err(error) => failed.push(format!("{} ({})", target, error)),
            },
            Err(error) => failed.push(format!("{} ({})", target, error)),
        }
    }
    WorkerCleanupOutcome { stopped, failed }
}

/// Free swarm member capacity mid-run by stopping finished workers owned by
/// this coordinator. `run_plan` spawns a fresh worker per node by default and
/// normally cleans up only at the end of the run, so on large plans membership
/// grows monotonically toward the swarm member cap and fresh spawns start
/// getting refused. `exclude` protects workers assigned earlier in this loop
/// whose queued status may not have propagated yet. Returns how many workers
/// were stopped.
///
/// Tradeoff: a stopped `ready` worker may have been a composite planner whose
/// synthesis node would otherwise be routed back to it (planner affinity).
/// Assignment falls back to a fresh or other eligible worker in that case,
/// which is an acceptable degradation when the alternative is aborting the run
/// at the member cap.
async fn cleanup_finished_workers_for_capacity(
    ctx: &ToolContext,
    exclude: &[String],
    reporter: &RunPlanReporter,
) -> usize {
    let Ok(members) = fetch_swarm_members(&ctx.session_id).await else {
        return 0;
    };
    let candidates: Vec<String> = cleanup_candidate_session_ids(
        &ctx.session_id,
        &members,
        &default_cleanup_target_statuses(),
        &[],
        false,
    )
    .into_iter()
    .filter(|session_id| !exclude.iter().any(|assigned| assigned == session_id))
    .collect();
    if candidates.is_empty() {
        return 0;
    }
    let outcome = stop_swarm_sessions(ctx, candidates, false).await;
    reporter
        .log(&format!("member-cap recovery: {}", outcome.describe()))
        .await;
    outcome.stopped.len()
}

/// How often the background progress card is refreshed from live plan state
/// while the driver is blocked awaiting workers.
const RUN_PLAN_PROGRESS_REFRESH_SECS: u64 = 15;

/// Whether the driver should abandon the current member-await and start a new
/// coordination loop because the plan's ready frontier grew while it was
/// blocked. Pure for unit testing.
///
/// `ready_baseline` is the set of ready item ids observed at the top of the
/// loop that started this await. Any *new* ready id means work the driver has
/// never had a chance to dispatch: a failed node re-queued via `swarm retry`,
/// a node unblocked by an externally-driven completion, or a gate-injected
/// gap. Comparing against the baseline (instead of `!ready.is_empty()`) is
/// what prevents wake storms: items that were already ready when the await
/// began (e.g. just-assigned tasks still momentarily `queued`, or ready nodes
/// that could not be assigned to any drivable worker) do not re-trigger, so a
/// permanently-stuck ready node wakes the driver at most once per await.
fn await_should_wake_for_new_ready(
    ready_baseline: &std::collections::HashSet<String>,
    summary: &PlanGraphStatus,
) -> bool {
    summary
        .ready_ids
        .iter()
        .any(|id| !ready_baseline.contains(id))
}

async fn await_swarm_progress(
    ctx: &ToolContext,
    session_ids: Vec<String>,
    timeout_minutes: u64,
    reporter: &RunPlanReporter,
    assignment_count: usize,
    ready_baseline: &std::collections::HashSet<String>,
) -> Result<()> {
    let request = Request::CommAwaitMembers {
        id: REQUEST_ID,
        session_id: ctx.session_id.clone(),
        target_status: default_run_await_statuses(),
        session_ids,
        mode: Some("any".to_string()),
        timeout_secs: Some(timeout_minutes.max(1) * 60),
        // run_plan needs the result inline to drive its coordination loop, so it
        // explicitly opts out of the background-by-default behavior.
        background: false,
        notify: false,
        wake: false,
    };
    let socket_timeout = std::time::Duration::from_secs(timeout_minutes.max(1) * 60 + 30);
    let await_members = send_request_with_timeout(request, Some(socket_timeout));
    tokio::pin!(await_members);

    // While blocked on the await (potentially many minutes), periodically
    // re-read live plan + member state and push it to the progress card.
    // Without this, worker completions and externally-assigned work (manual
    // `assign_task`) only surface at the driver's own wave boundaries, so the
    // card goes stale for the whole await. Refresh failures are ignored: the
    // card is best-effort and the await result is what drives the loop.
    let refresh_period = std::time::Duration::from_secs(RUN_PLAN_PROGRESS_REFRESH_SECS);
    let mut refresh =
        tokio::time::interval_at(tokio::time::Instant::now() + refresh_period, refresh_period);
    refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let response = loop {
        tokio::select! {
            result = &mut await_members => break result,
            _ = refresh.tick() => {
                let summary = match fetch_plan_status(&ctx.session_id).await {
                    Ok(summary) => summary,
                    Err(_) => continue,
                };
                if reporter.is_background() {
                    let live_active = fetch_in_flight_swarm_sessions(&ctx.session_id)
                        .await
                        .map(|sessions| sessions.len())
                        .unwrap_or(0);
                    let (completed, total, message) =
                        run_plan_progress_snapshot(&summary, live_active, assignment_count);
                    reporter.progress(completed, total, message).await;
                }
                // Ready frontier grew while blocked (a `swarm retry` re-queued
                // failed nodes, an external completion unblocked work, a gate
                // injected gaps): return to the coordination loop so the new
                // work is dispatched under the normal budget instead of
                // waiting out the current wave. The abandoned await is a
                // plain request future; dropping it cancels only our wait,
                // not the workers.
                if await_should_wake_for_new_ready(ready_baseline, &summary) {
                    reporter
                        .log("ready frontier grew during await (retry/requeue or external unblock); re-entering dispatch loop")
                        .await;
                    return Ok(());
                }
            }
        }
    };

    match response {
        Ok(ServerEvent::CommAwaitMembersResponse {
            completed, summary, ..
        }) => {
            if completed {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "Timed out waiting for swarm progress: {}",
                    summary
                ))
            }
        }
        Ok(response) => ensure_success(&response),
        Err(e) => Err(anyhow::anyhow!(
            "Failed while awaiting swarm progress: {}",
            e
        )),
    }
}

/// Decide how many swarm workers `run_plan` keeps active at once.
///
/// Policy:
///   * an explicit `requested` limit always wins (clamped to >= 1);
///   * deep mode with no explicit limit fans out wide: use `deep_cap`, where
///     `0` means "no extra cap" (`usize::MAX`) so the whole ready set is
///     dispatched, bounded only by the swarm member cap;
///   * light mode with no explicit limit keeps the small, cheap fan-out default.
///
/// Pure and side-effect free so the concurrency contract is unit-testable
/// without a live swarm.
fn resolve_run_plan_concurrency(requested: Option<usize>, is_deep: bool, deep_cap: usize) -> usize {
    match requested {
        Some(explicit) => explicit.max(1),
        None if is_deep => {
            if deep_cap == 0 {
                usize::MAX
            } else {
                deep_cap
            }
        }
        None => LIGHT_MODE_DEFAULT_CONCURRENCY,
    }
}

/// Running tally of how well a `run_plan` drive used its concurrency budget.
///
/// Deep mode's promise is comprehensiveness through parallel fan-out, so a run
/// that finishes with peak parallelism ~1 despite a 32+ slot budget means the
/// graph was decomposed serially and the budget was wasted. Tracking this per
/// loop (max in-flight, plus how often open slots sat idle with no ready work)
/// turns "did we actually use the budget?" into a measured, reportable number
/// instead of a hope.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RunPlanUtilization {
    /// Highest number of simultaneously in-flight tasks observed.
    peak_in_flight: usize,
    /// Coordination loops observed.
    loops: usize,
    /// Loops where open worker slots existed but the plan had nothing ready to
    /// dispatch into them (budget idle due to graph narrowness, not the cap).
    starved_loops: usize,
}

impl RunPlanUtilization {
    /// Record one coordination loop. `open_slots` is `None` when the budget is
    /// unbounded (`concurrency_limit == usize::MAX`): an infinite budget has no
    /// meaningful starvation denominator, so only peak parallelism is tracked.
    fn record_loop(&mut self, in_flight: usize, open_slots: Option<usize>, dispatched: usize) {
        self.loops += 1;
        self.peak_in_flight = self.peak_in_flight.max(in_flight + dispatched);
        if let Some(open_slots) = open_slots
            && open_slots > 0
            && dispatched < open_slots
        {
            self.starved_loops += 1;
        }
    }

    /// Render the utilization line for the terminal report. In deep mode a
    /// starved run also gets an actionable hint, because the fix (wider
    /// decomposition) belongs to the model reading this output.
    fn report(&self, concurrency_limit: usize, is_deep: bool) -> String {
        let limit_label = if concurrency_limit == usize::MAX {
            "unbounded".to_string()
        } else {
            concurrency_limit.to_string()
        };
        let mut line = format!(
            "Budget utilization: peak {} of {} concurrent worker slot(s); {} of {} loop(s) had idle capacity with nothing ready.",
            self.peak_in_flight, limit_label, self.starved_loops, self.loops
        );
        let mostly_starved = self.loops > 0 && self.starved_loops * 2 >= self.loops;
        let ran_narrow = self.loops >= 3 && self.peak_in_flight <= 2;
        if is_deep && (mostly_starved || ran_narrow) {
            line.push_str(
                "\nDeep-mode hint: the graph ran much narrower than the agent budget. If coverage \
                 matters, expand remaining or follow-up work into MANY independent sibling nodes \
                 (depends_on only for real data dependencies) so the ready set fills the budget.",
            );
        }
        line
    }
}

/// Extract the background task id from its output file path
/// (`<task_id>.output`), mirroring the bash tool's convention so progress
/// updates can be routed back to the background task manager.
fn task_id_from_output_path(path: &std::path::Path) -> Option<&str> {
    path.file_name()?.to_str()?.strip_suffix(".output")
}

/// Progress/log sink for a `run_plan` execution.
///
/// In background mode this appends human-readable lines to the background
/// task's output file and pushes determinate progress (terminal/total plan
/// nodes) into the background task manager, so the UI renders a live swarm
/// progress card and `bg status` stays meaningful. In inline (blocking) mode
/// every method is a no-op.
struct RunPlanReporter {
    task_id: Option<String>,
    output_path: Option<std::path::PathBuf>,
}

impl RunPlanReporter {
    fn inline() -> Self {
        Self {
            task_id: None,
            output_path: None,
        }
    }

    fn background(output_path: &std::path::Path) -> Self {
        Self {
            task_id: task_id_from_output_path(output_path).map(str::to_string),
            output_path: Some(output_path.to_path_buf()),
        }
    }

    /// Whether this reporter feeds a live background progress card (inline
    /// reporters are no-ops, so refresh polling would be wasted requests).
    fn is_background(&self) -> bool {
        self.task_id.is_some()
    }

    async fn log(&self, line: &str) {
        let Some(path) = &self.output_path else {
            return;
        };
        use tokio::io::AsyncWriteExt;
        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
        {
            let _ = file.write_all(format!("{}\n", line).as_bytes()).await;
        }
    }

    async fn progress(&self, terminal: usize, total: usize, message: String) {
        let Some(task_id) = &self.task_id else {
            return;
        };
        let progress = crate::bus::BackgroundTaskProgress {
            kind: crate::bus::BackgroundTaskProgressKind::Determinate,
            percent: None,
            message: Some(message),
            current: Some(terminal as u64),
            total: Some(total as u64),
            unit: Some("nodes".to_string()),
            eta_seconds: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
            source: crate::bus::BackgroundTaskProgressSource::Reported,
        }
        .normalize();
        let _ = crate::background::global()
            .update_progress(task_id, progress)
            .await;
    }

    /// Record an explicit checkpoint (a JCODE_CHECKPOINT-style milestone) on
    /// the background task, so pause/alert moments surface as checkpoint events
    /// in the UI instead of only trailing the output log. No-op inline.
    async fn checkpoint(&self, message: &str) {
        self.log(message).await;
        let Some(task_id) = &self.task_id else {
            return;
        };
        let progress = crate::bus::BackgroundTaskProgress {
            kind: crate::bus::BackgroundTaskProgressKind::Indeterminate,
            percent: None,
            message: Some(message.to_string()),
            current: None,
            total: None,
            unit: None,
            eta_seconds: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
            source: crate::bus::BackgroundTaskProgressSource::Reported,
        }
        .normalize();
        let _ = crate::background::global()
            .update_checkpoint(task_id, progress)
            .await;
    }

    /// Rewrite the output file so `summary` leads and the progressive log
    /// trails it. Background completion previews take the first ~500 chars of
    /// the output file, so the terminal summary must come first for the
    /// agent's wake notification to be useful.
    async fn finalize(&self, summary: &str) {
        let Some(path) = &self.output_path else {
            return;
        };
        let log = tokio::fs::read_to_string(path).await.unwrap_or_default();
        let content = if log.trim().is_empty() {
            format!("{}\n", summary)
        } else {
            format!("{}\n\n--- run log ---\n{}", summary, log)
        };
        let _ = tokio::fs::write(path, content).await;
    }
}

/// Per-process registry of sessions with a `run_plan` driver claimed or
/// running. The duplicate-driver guard does its check-and-insert under this
/// one lock, so two `run_plan` calls racing in the same batch cannot both
/// pass. Deliberately per-process: a stale `Running` status file left on disk
/// by a previous (reloaded/crashed) server process must never block
/// restarting the driver.
fn run_plan_driver_claims() -> &'static std::sync::Mutex<HashMap<String, RunPlanDriverClaim>> {
    static CLAIMS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, RunPlanDriverClaim>>> =
        std::sync::OnceLock::new();
    CLAIMS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

enum RunPlanDriverClaim {
    /// Claimed, background task not spawned yet.
    Starting,
    /// Driver spawned as this background task.
    Running(String),
}

enum RunPlanDriverClaimResult {
    Claimed(RunPlanClaimGuard),
    /// A driver already holds the claim. Carries its task id when known
    /// (None while the winner is still between claim and spawn).
    AlreadyRunning(Option<String>),
}

/// RAII holder for a `Starting` claim. Dropping it without
/// [`RunPlanClaimGuard::record_task`] releases the claim, so a cancelled or
/// failed startup path cannot permanently block `run_plan` for the session.
struct RunPlanClaimGuard {
    session_id: String,
    defused: bool,
}

impl RunPlanClaimGuard {
    /// Upgrade the claim to `Running(task_id)`. From here staleness is
    /// resolved via `BackgroundTaskManager::is_live_task`: once the driver
    /// task finishes (and is pruned from the live map), the next claim
    /// replaces this entry.
    fn record_task(mut self, task_id: &str) {
        let mut claims = run_plan_driver_claims()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        claims.insert(
            self.session_id.clone(),
            RunPlanDriverClaim::Running(task_id.to_string()),
        );
        self.defused = true;
    }
}

impl Drop for RunPlanClaimGuard {
    fn drop(&mut self) {
        if self.defused {
            return;
        }
        let mut claims = run_plan_driver_claims()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Only release a claim this guard still owns.
        if matches!(
            claims.get(&self.session_id),
            Some(RunPlanDriverClaim::Starting)
        ) {
            claims.remove(&self.session_id);
        }
    }
}

/// Atomically claim the `run_plan` driver slot for `session_id`.
///
/// Check-and-insert happens under one lock. An existing `Running` claim only
/// blocks while its background task is still live in this process; a claim
/// left by a finished (pruned) or pre-reload driver is replaced.
fn try_claim_run_plan_driver(
    manager: &crate::background::BackgroundTaskManager,
    session_id: &str,
) -> RunPlanDriverClaimResult {
    let mut claims = run_plan_driver_claims()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match claims.get(session_id) {
        Some(RunPlanDriverClaim::Starting) => {
            return RunPlanDriverClaimResult::AlreadyRunning(None);
        }
        Some(RunPlanDriverClaim::Running(task_id)) => {
            if manager.is_live_task(task_id) {
                return RunPlanDriverClaimResult::AlreadyRunning(Some(task_id.clone()));
            }
            // Stale claim: the driver task already finished or belonged to a
            // previous process image. Fall through and take over.
        }
        None => {}
    }
    claims.insert(session_id.to_string(), RunPlanDriverClaim::Starting);
    RunPlanDriverClaimResult::Claimed(RunPlanClaimGuard {
        session_id: session_id.to_string(),
        defused: false,
    })
}

/// Drive `run_plan` as a managed background task and return immediately.
///
/// The coordinating agent stays responsive: the plan loop runs inside the
/// shared `BackgroundTaskManager` (task id, progress card, `bg` tool
/// integration), and completion is delivered through the standard notify/wake
/// path like any other background task.
async fn run_swarm_plan_in_background(
    ctx: &ToolContext,
    params: CommunicateInput,
) -> Result<ToolOutput> {
    // Validate the plan inline so an empty/broken plan errors immediately
    // instead of as a delayed background failure.
    let initial_summary = fetch_plan_status(&ctx.session_id).await?;
    if initial_summary.item_count == 0 {
        return Ok(ToolOutput::new("No swarm plan items to run."));
    }

    // Refuse to start a second driver for the same session: two concurrent
    // run_plan loops would race on assignments and double-spawn workers. The
    // claim is check-and-insert under one lock, so two run_plan calls in the
    // same batch cannot both pass. Only drivers live in this process count; a
    // stale "running" status file left by a server reload must not block
    // restarting the driver (the claim map is per-process and dead task ids
    // fail the is_live_task check).
    let manager = crate::background::global();
    let claim = match try_claim_run_plan_driver(manager, &ctx.session_id) {
        RunPlanDriverClaimResult::Claimed(claim) => claim,
        RunPlanDriverClaimResult::AlreadyRunning(existing) => {
            return Ok(ToolOutput::new(match existing {
                Some(task_id) => format!(
                    "A swarm run_plan driver is already running for this session (task {}). \
                     Check it with `bg action=\"status\" task_id=\"{}\"` or `swarm plan_status` instead of starting another.",
                    task_id, task_id
                ),
                None => "A swarm run_plan driver is already starting for this session. \
                         Check it with `swarm plan_status` instead of starting another."
                    .to_string(),
            }));
        }
    };

    let notify = params.notify.unwrap_or(true);
    let wake = params.wake.unwrap_or(true);
    // Keep the display name free of the "·" separator used by the background
    // notification markdown header, or downstream parsing mis-splits the label.
    let display_name = format!(
        "run_plan ({} nodes, {} mode)",
        initial_summary.item_count, initial_summary.mode
    );

    let bg_ctx = ctx.clone();
    let info = crate::background::global()
        .spawn_with_notify(
            "swarm",
            Some(display_name.clone()),
            &ctx.session_id,
            notify,
            wake,
            move |output_path| async move {
                let reporter = RunPlanReporter::background(&output_path);
                match run_swarm_plan_to_terminal(&bg_ctx, &params, &reporter).await {
                    Ok(output) => {
                        reporter.finalize(&output.output).await;
                        Ok(TaskResult::completed(Some(0)))
                    }
                    Err(error) => {
                        let message = format!("run_plan failed: {}", error);
                        reporter.finalize(&message).await;
                        Ok(TaskResult::failed(None, message))
                    }
                }
            },
        )
        .await;
    claim.record_task(&info.task_id);

    let delivery_note = if wake {
        "You'll be woken with the result when the plan reaches a terminal state."
    } else if notify {
        "A notification will appear when the plan reaches a terminal state."
    } else {
        "Notifications disabled. Use the `bg` tool to check status."
    };
    let output = format!(
        "🐝 Swarm plan running in background.\n\n\
         Task ID: {}\n\
         Plan: {} node(s), {} mode\n\
         Output file: {}\n\n\
         {}\n\
         Check progress: use the `bg` tool with action=\"status\" and task_id=\"{}\", or `swarm plan_status`.\n\
         Note: a server reload stops this driver (workers keep running); rerun `swarm run_plan` to resume driving the same plan.",
        info.task_id,
        initial_summary.item_count,
        initial_summary.mode,
        info.output_file.display(),
        delivery_note,
        info.task_id,
    );

    Ok(ToolOutput::new(output)
        .with_title(format!("Swarm run_plan in background: {}", info.task_id))
        .with_metadata(json!({
            "background": true,
            "swarm": true,
            "task_id": info.task_id,
            "display_name": display_name,
            "output_file": info.output_file.to_string_lossy(),
            "status_file": info.status_file.to_string_lossy(),
        })))
}

/// Hint appended to every `run_plan` driver failure: the driver exits without
/// the end-of-run cleanup, so spawned workers keep running even when
/// `retain_agents=false`, and the caller must know how to stop or resume them.
const RUN_PLAN_WORKER_RETENTION_HINT: &str = "\nSpawned workers were retained; run `swarm cleanup` to stop them, rerun `swarm run_plan` to resume driving the same plan, or `swarm plan_status` to inspect.";

/// Append the worker-retention hint to a driver failure message, idempotently
/// so wrappers that re-report an already-hinted error do not duplicate it.
fn with_worker_retention_hint(message: String) -> String {
    if message.contains("swarm cleanup") {
        message
    } else {
        format!("{message}{RUN_PLAN_WORKER_RETENTION_HINT}")
    }
}

/// How the `run_plan` assignment loop should react to a `CommAssignNext` error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssignErrorAction {
    /// No more runnable work or no eligible workers: stop assigning this loop
    /// and continue with in-flight work.
    BreakGracefully,
    /// The swarm hit its total member cap so fresh spawns are refused: free
    /// finished owned workers and/or fall back to reusing ready workers instead
    /// of aborting the whole run.
    RecoverCapacity,
    /// Anything else is a real failure.
    Fail,
}

fn classify_assign_error(message: &str) -> AssignErrorAction {
    if message.contains("No runnable unassigned tasks")
        || message.contains("No ready or completed swarm agents")
    {
        AssignErrorAction::BreakGracefully
    } else if message.contains("Swarm member limit reached") {
        AssignErrorAction::RecoverCapacity
    } else {
        AssignErrorAction::Fail
    }
}

/// Next step for a slot whose assignment was refused by the member cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapRecoveryStep {
    /// Cleanup freed capacity: retry the slot keeping the fresh-spawn preference.
    RetryFresh,
    /// Nothing was freed: retry the slot in reuse-only mode (no spawning).
    RetryReuse,
    /// Recovery already ran and the cap still refuses this slot: stop assigning
    /// this loop and continue with in-flight work.
    GiveUp,
}

/// Pure recovery policy for a member-cap refusal, keyed on how many times this
/// slot already hit the cap (`cap_hits`) and how many workers the incremental
/// cleanup freed. Kept side-effect free so the fallback contract is unit
/// testable without a live swarm.
fn cap_recovery_step(cap_hits: usize, freed: usize) -> CapRecoveryStep {
    if cap_hits > 1 {
        CapRecoveryStep::GiveUp
    } else if freed > 0 {
        CapRecoveryStep::RetryFresh
    } else {
        CapRecoveryStep::RetryReuse
    }
}

/// Count each plan node at most once as terminal: completed, failed, blocked,
/// and cycle sets overlap in places (and failed nodes appear in none of the
/// legacy three), so a plain sum both over- and under-counts.
fn plan_terminal_node_count(summary: &PlanGraphStatus) -> usize {
    summary
        .completed_ids
        .iter()
        .chain(summary.failed_ids.iter())
        .chain(summary.blocked_ids.iter())
        .chain(summary.cycle_ids.iter())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Numbers for the `run_plan` background progress card. Pure for unit testing.
///
/// The percent-driving pair is `(completed, total)`: only *completed* nodes
/// count toward 100%, so a run where most nodes failed reads as mostly
/// unfinished instead of "98% complete" (failed/blocked counts are surfaced in
/// the message instead). `live_active` is the count of in-flight worker
/// sessions observed from member state; the card shows whichever of plan
/// execution state (`active_ids`) or live member state is larger, so nodes
/// assigned outside this driver (e.g. manual `assign_task`) still show as
/// active.
fn run_plan_progress_snapshot(
    summary: &PlanGraphStatus,
    live_active: usize,
    assignment_count: usize,
) -> (usize, usize, String) {
    let completed = summary.completed_ids.len();
    let active = summary.active_ids.len().max(live_active);
    let message = format!(
        "completed {} · failed {} · blocked {} · active {} · assignments {}",
        completed,
        summary.failed_ids.len(),
        summary.blocked_ids.len(),
        active,
        assignment_count
    );
    (completed, summary.item_count, message)
}

/// Terminal-state summary line for `run_plan`, including failed nodes so a run
/// with failures never reads like a clean finish. Pure for unit testing.
fn format_run_plan_terminal_summary(
    loop_count: usize,
    summary: &PlanGraphStatus,
    assignment_count: usize,
) -> String {
    let mut output = format!(
        "Swarm plan reached terminal/blocked state after {} loop(s). completed={} failed={} blocked={} cycles={} active={} assignments={}",
        loop_count,
        summary.completed_ids.len(),
        summary.failed_ids.len(),
        summary.blocked_ids.len(),
        summary.cycle_ids.len(),
        summary.active_ids.len(),
        assignment_count
    );
    if summary.mode.eq_ignore_ascii_case("deep") {
        output.push_str(&format!(
            "\nGrowth: {} seeded -> {} nodes ({} machinery-grown: expansions, gate-injected gaps, gates).",
            summary.seeded_count, summary.item_count, summary.grown_count
        ));
    }
    if !summary.failed_ids.is_empty() {
        output.push_str(&format!(
            "\nFailed nodes: {}. This run did NOT finish cleanly; inspect them with `swarm plan_status` and retry or salvage before trusting the result.",
            summary.failed_ids.join(", ")
        ));
        // Recorded failure reasons make the summary self-explanatory: a wave
        // of "task failed: ... 401 Unauthorized" lines names the root cause
        // without another plan_status round-trip.
        for id in &summary.failed_ids {
            if let Some(reason) = summary.failed_reasons.get(id) {
                output.push_str(&format!("\n  {}: {}", id, reason));
            }
        }
    }
    output
}

/// Minimum number of credential-failed workers that count as a wave rather
/// than an isolated bad worker.
const CREDENTIAL_FAILURE_WAVE_MIN_WORKERS: usize = 2;

/// How recent a worker's credential failure must be (via `status_age_secs`) to
/// count toward a wave. Old failed workers from a previous, already-diagnosed
/// wave must not re-trip the breaker after the user fixes auth and retries.
const CREDENTIAL_FAILURE_WAVE_WINDOW_SECS: u64 = 60;

/// A wave of worker failures that share one credential-shaped root cause.
///
/// When dispatched workers die within seconds of assignment with 401 /
/// `invalid_grant` / `authentication_error`-style errors, the credential is
/// broken for every worker on that route: assigning more nodes only fails more
/// of the plan. Detecting the wave lets `run_plan` pause dispatching and
/// surface the one real fix instead of silently burning the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CredentialFailureWave {
    /// Failed worker sessions in the wave.
    session_ids: Vec<String>,
    /// Representative failure detail (first observed).
    sample_detail: String,
    /// Provider named by the failing workers, when known (e.g. "anthropic").
    provider: Option<String>,
}

/// Detect a credential-failure wave in a swarm member snapshot.
///
/// A wave exists when, with **zero completed plan nodes**, at least
/// [`CREDENTIAL_FAILURE_WAVE_MIN_WORKERS`] drivable workers sit in `failed`
/// status whose detail classifies as a credential failure (via the shared
/// [`crate::provider::error_looks_like_credential_failure`] classifier) and
/// whose failure is recent (`status_age_secs <= window_secs`). Pure over its
/// inputs so the breaker contract is unit-testable without a live swarm.
fn detect_credential_failure_wave(
    members: &[AgentInfo],
    coordinator_session_id: &str,
    completed_node_count: usize,
    window_secs: u64,
) -> Option<CredentialFailureWave> {
    if completed_node_count > 0 {
        return None;
    }
    let mut session_ids = Vec::new();
    let mut sample_detail: Option<String> = None;
    let mut provider: Option<String> = None;
    for member in members {
        if member.session_id == coordinator_session_id {
            continue;
        }
        if !swarm_member_is_drivable_worker(member, coordinator_session_id) {
            continue;
        }
        if member.status.as_deref() != Some("failed") {
            continue;
        }
        let Some(detail) = member.detail.as_deref() else {
            continue;
        };
        if !crate::provider::error_looks_like_credential_failure(detail) {
            continue;
        }
        // Require a known, recent failure age: stale failed workers (or ones
        // whose age did not propagate) must not re-trip the breaker.
        if !matches!(member.status_age_secs, Some(age) if age <= window_secs) {
            continue;
        }
        session_ids.push(member.session_id.clone());
        if sample_detail.is_none() {
            sample_detail = Some(detail.to_string());
        }
        if provider.is_none() {
            provider = member.provider_name.clone();
        }
    }
    if session_ids.len() < CREDENTIAL_FAILURE_WAVE_MIN_WORKERS {
        return None;
    }
    Some(CredentialFailureWave {
        session_ids,
        sample_detail: sample_detail.unwrap_or_default(),
        provider,
    })
}

/// The `jcode login` invocation most likely to fix a credential wave for
/// `provider`, mapping provider names to their login provider keys.
fn credential_login_fix_hint(provider: Option<&str>) -> String {
    let lowered = provider.map(str::to_ascii_lowercase);
    let target = match lowered.as_deref() {
        Some("anthropic" | "claude") => "claude",
        Some("openai" | "codex") => "openai",
        Some("google" | "gemini") => "gemini",
        Some(other) if !other.trim().is_empty() => other,
        _ => "<provider>",
    };
    format!("`jcode login --provider {target}`")
}

/// Actionable pause message for a credential-failure wave: names the failed
/// workers, the credential-shaped root cause, and the fix. Pure for unit
/// testing; used both as the run error and as the swarm broadcast body.
fn format_credential_failure_wave_error(wave: &CredentialFailureWave, window_secs: u64) -> String {
    format!(
        "run_plan paused dispatching: {count} worker(s) failed within {window_secs}s with \
         credential/auth failures and no plan node has completed (e.g. {first}: \"{sample}\"). \
         A broken credential (expired OAuth session, revoked refresh token, or invalid API key) \
         fails every worker on that route, so assigning more nodes would only fail more of the \
         plan. Fix auth first: run {login_hint} (or pin a working API-key route), then requeue \
         the failed nodes (`swarm retry`) and run `swarm run_plan` again.",
        count = wave.session_ids.len(),
        first = wave
            .session_ids
            .first()
            .map(String::as_str)
            .unwrap_or("worker"),
        sample = wave.sample_detail,
        login_hint = credential_login_fix_hint(wave.provider.as_deref()),
    )
}

/// Best-effort broadcast of a plan-level alert to the whole swarm, so live
/// members and attached UIs see why dispatch stopped.
async fn broadcast_plan_alert(ctx: &ToolContext, message: &str) -> Result<()> {
    let request = Request::CommMessage {
        id: REQUEST_ID,
        from_session: ctx.session_id.clone(),
        message: message.to_string(),
        to_session: None,
        channel: None,
        wake: None,
        delivery: None,
        tldr: Some("run_plan paused: credential failure wave; fix auth then retry".to_string()),
    };
    match send_request(request).await {
        Ok(response) => ensure_success(&response),
        Err(e) => Err(anyhow::anyhow!("Failed to broadcast plan alert: {}", e)),
    }
}

async fn run_swarm_plan_to_terminal(
    ctx: &ToolContext,
    params: &CommunicateInput,
    reporter: &RunPlanReporter,
) -> Result<ToolOutput> {
    // Every driver-failure exit (assignment failure, await timeout, stall,
    // max-loops, even a mid-run plan-status fetch error) leaves spawned workers
    // running because the end-of-run cleanup never executes, regardless of
    // retain_agents. Append the retention hint uniformly here so no failure
    // path can forget it.
    match run_swarm_plan_loop(ctx, params, reporter).await {
        Ok(output) => Ok(output),
        Err(error) => Err(anyhow::anyhow!(with_worker_retention_hint(
            error.to_string()
        ))),
    }
}

async fn run_swarm_plan_loop(
    ctx: &ToolContext,
    params: &CommunicateInput,
    reporter: &RunPlanReporter,
) -> Result<ToolOutput> {
    let initial_summary = fetch_plan_status(&ctx.session_id).await?;
    let is_deep = initial_summary.mode.eq_ignore_ascii_case("deep");

    let configured_deep_cap = crate::config::config().agents.swarm_max_concurrent_agents;
    let concurrency_limit =
        resolve_run_plan_concurrency(params.concurrency_limit, is_deep, configured_deep_cap);
    let timeout_minutes = params.timeout_minutes.unwrap_or(60).max(1);
    let retain_agents = params.retain_agents.unwrap_or(false);
    let spawn_if_needed = params.spawn_if_needed.or(Some(true));
    // Default to a fresh worker per task-graph node. Reusing a worker that already
    // completed a *different* node carries that node's conversation into the next
    // assignment, and the model often just re-reports its prior result instead of
    // doing the new work (observed leaving gap/synthesis nodes stuck). The task-DAG
    // model assumes clean, isolated workers, so unless the caller explicitly opts
    // into reuse (`prefer_spawn=false`), prefer spawning a fresh worker per node.
    let prefer_spawn = params.prefer_spawn.or(Some(true));
    let mut assignment_count = 0usize;
    let mut loop_count = 0usize;
    let max_loops = 200usize;
    let mut utilization = RunPlanUtilization::default();
    // Consecutive loops where an active task exists but no drivable worker is
    // awaitable. This is normally a brief transition (a composite re-waking to
    // synthesize, or a just-finished task whose member status has not propagated),
    // so we back off and re-check a few times before declaring a real stall.
    let mut transient_stall_loops = 0usize;
    let max_transient_stall_loops = 5usize;

    loop {
        loop_count += 1;
        if loop_count > max_loops {
            return Err(anyhow::anyhow!(
                "run_plan exceeded {} coordination loops; leaving workers untouched for inspection",
                max_loops
            ));
        }

        let summary = fetch_plan_status(&ctx.session_id).await?;
        if summary.item_count == 0 {
            return Ok(ToolOutput::new("No swarm plan items to run."));
        }

        let members = fetch_swarm_members(&ctx.session_id).await?;
        let in_flight_sessions = in_flight_swarm_session_ids(&members, &ctx.session_id);

        // Credential-failure circuit breaker: when a wave of workers dies with
        // 401/invalid_grant-style auth errors and nothing has completed, the
        // credential is broken for every worker on that route. Pausing here
        // (before the terminal check) means even a fully-burned first wave
        // surfaces the root cause and the fix instead of a bare
        // "failed=N" terminal summary with no explanation.
        if let Some(wave) = detect_credential_failure_wave(
            &members,
            &ctx.session_id,
            summary.completed_ids.len(),
            CREDENTIAL_FAILURE_WAVE_WINDOW_SECS,
        ) {
            let message =
                format_credential_failure_wave_error(&wave, CREDENTIAL_FAILURE_WAVE_WINDOW_SECS);
            reporter.checkpoint(&message).await;
            if let Err(error) = broadcast_plan_alert(ctx, &message).await {
                reporter
                    .log(&format!(
                        "failed to broadcast credential-failure alert to the swarm: {error}"
                    ))
                    .await;
            }
            return Err(anyhow::anyhow!(message));
        }

        let terminal_count = plan_terminal_node_count(&summary);
        let (progress_completed, progress_total, progress_message) =
            run_plan_progress_snapshot(&summary, in_flight_sessions.len(), assignment_count);
        reporter
            .progress(progress_completed, progress_total, progress_message)
            .await;
        let no_more_runnable = summary.active_ids.is_empty()
            && summary.next_ready_ids.is_empty()
            && in_flight_sessions.is_empty();
        if no_more_runnable || terminal_count >= summary.item_count {
            let mut output =
                format_run_plan_terminal_summary(loop_count, &summary, assignment_count);
            output.push_str(&format!(
                "\n{}",
                utilization.report(concurrency_limit, is_deep)
            ));
            if !summary.low_confidence_ids.is_empty() {
                output.push_str(&format!(
                    "\nConfidence coverage: {} completed node(s) self-reported LOW confidence: {}. \
                     Consider seeding follow-up nodes to shore these up before trusting the result.",
                    summary.low_confidence_ids.len(),
                    summary.low_confidence_ids.join(", ")
                ));
            }
            if retain_agents {
                output.push_str("\nRetained spawned workers because retain_agents=true.");
            } else {
                // Run the automatic end-of-plan cleanup with a sanitized input:
                // `force`, `session_ids`, and `target_status` on the run_plan
                // call are meant for explicit stop/cleanup/await actions, and
                // leaking `force=true` here would force-stop every terminal
                // swarm member (including user-created idle sessions) instead
                // of only the workers this coordinator owns.
                let cleanup_params = CommunicateInput {
                    force: None,
                    session_ids: None,
                    target_status: None,
                    ..params.clone()
                };
                let cleanup = cleanup_swarm_workers(ctx, &cleanup_params).await?;
                output.push_str(&format!("\n{}", cleanup));
            }
            return Ok(ToolOutput::new(output));
        }

        let active_count = summary.active_ids.len().max(in_flight_sessions.len());
        let available_slots = concurrency_limit.saturating_sub(active_count);
        let mut assigned_sessions = Vec::new();
        // Member-cap fallback state, reset each coordination loop. When the swarm
        // hits its total member cap, fresh spawns are refused; instead of aborting
        // the whole run we first free finished owned workers (incremental cleanup)
        // and retry, then fall back to reuse-only assignment (no spawning), and
        // only after that stop assigning and continue with in-flight work.
        let mut cap_hits = 0usize;
        let mut reuse_only = false;
        let mut slots_remaining = available_slots;
        while slots_remaining > 0 {
            let request = Request::CommAssignNext {
                id: REQUEST_ID,
                session_id: ctx.session_id.clone(),
                target_session: params.target_session.clone(),
                working_dir: params.working_dir.clone(),
                prefer_spawn: if reuse_only {
                    Some(false)
                } else {
                    prefer_spawn
                },
                spawn_if_needed: if reuse_only {
                    Some(false)
                } else {
                    spawn_if_needed
                },
                message: params.message.clone(),
                model: params.model.clone(),
                effort: params.effort.clone(),
            };
            match send_request(request).await {
                Ok(ServerEvent::CommAssignTaskResponse {
                    task_id,
                    target_session,
                    ..
                }) => {
                    assignment_count += 1;
                    slots_remaining -= 1;
                    reporter
                        .log(&format!("assigned {} -> {}", task_id, target_session))
                        .await;
                    assigned_sessions.push(target_session);
                }
                Ok(ServerEvent::Error { message, .. }) => {
                    match classify_assign_error(&message) {
                        AssignErrorAction::BreakGracefully => break,
                        AssignErrorAction::RecoverCapacity => {
                            cap_hits += 1;
                            let freed = if cap_hits == 1 {
                                cleanup_finished_workers_for_capacity(
                                    ctx,
                                    &assigned_sessions,
                                    reporter,
                                )
                                .await
                            } else {
                                0
                            };
                            match cap_recovery_step(cap_hits, freed) {
                                CapRecoveryStep::RetryFresh => {
                                    // Cleanup freed member slots; retry this slot
                                    // with the fresh-spawn preference intact.
                                }
                                CapRecoveryStep::RetryReuse => {
                                    reuse_only = true;
                                    reporter
                                        .log(
                                            "member cap reached and no finished workers to free; \
                                             falling back to reusing ready workers (prefer_spawn=false)",
                                        )
                                        .await;
                                }
                                CapRecoveryStep::GiveUp => {
                                    reporter
                                        .log(
                                            "member cap still reached after recovery; \
                                             continuing with in-flight work",
                                        )
                                        .await;
                                    break;
                                }
                            }
                        }
                        AssignErrorAction::Fail => {
                            return Err(anyhow::anyhow!(message));
                        }
                    }
                }
                Ok(response) => ensure_success(&response)?,
                Err(e) => return Err(anyhow::anyhow!("Failed to assign next swarm task: {}", e)),
            }
        }
        utilization.record_loop(
            active_count,
            (concurrency_limit != usize::MAX).then_some(available_slots),
            assigned_sessions.len(),
        );

        let await_sessions = if assigned_sessions.is_empty() {
            in_flight_sessions
        } else {
            assigned_sessions
        };

        if await_sessions.is_empty() {
            if active_count > 0 {
                // An active task exists but nothing drivable is awaitable. This is
                // usually transient: a composite is re-waking to synthesize, or a
                // worker just finished and its member status has not propagated yet.
                // Re-check a few times with a short backoff before giving up, and
                // bail early if the plan reaches a terminal state in the meantime.
                transient_stall_loops += 1;
                if transient_stall_loops <= max_transient_stall_loops {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                return Err(anyhow::anyhow!(
                    "run_plan found {} active task(s) but no running swarm members to await after {} re-checks; inspect plan_status and member list before retrying",
                    active_count,
                    max_transient_stall_loops
                ));
            }
            // Nothing was assigned this loop, nothing is in flight, yet the plan is
            // not terminal. This means some non-terminal task cannot be driven, e.g.
            // it is already assigned to a session run_plan cannot drive (a foreign or
            // stale member). Spinning here would busy-loop to the max-loop cap, so
            // surface the stuck state with the offending tasks instead.
            let stuck: Vec<String> = summary
                .next_ready_ids
                .iter()
                .chain(summary.ready_ids.iter())
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            let detail = if stuck.is_empty() {
                "no ready tasks and no in-flight workers".to_string()
            } else {
                format!(
                    "runnable task(s) {} could not be assigned to any drivable worker",
                    stuck.join(", ")
                )
            };
            return Err(anyhow::anyhow!(
                "run_plan stalled after {} loop(s): {}. This usually means a task is assigned to a session run_plan cannot drive (foreign or stale member). Reassign with an explicit target_session, or clear the stale assignment, then retry.",
                loop_count,
                detail
            ));
        }
        // Baseline for requeue pickup: everything ready at the top of this
        // loop either gets assigned below or is known-undispatchable this
        // wave. Anything ready *beyond* this set while we block (a retried
        // failure, an external unblock) should cut the await short.
        let ready_baseline: std::collections::HashSet<String> =
            summary.ready_ids.iter().cloned().collect();
        await_swarm_progress(
            ctx,
            await_sessions,
            timeout_minutes,
            reporter,
            assignment_count,
            &ready_baseline,
        )
        .await?;
        // Real progress (an await completed); clear the transient-stall backoff so
        // a later genuine stall starts counting fresh.
        transient_stall_loops = 0;
    }
}

async fn spawn_assignment_session(ctx: &ToolContext, params: &CommunicateInput) -> Result<String> {
    let spawn_request = Request::CommSpawn {
        id: REQUEST_ID,
        session_id: ctx.session_id.clone(),
        working_dir: params.working_dir.clone(),
        initial_message: None,
        request_nonce: Some(fresh_spawn_request_nonce(ctx)),
        spawn_mode: params.spawn_mode.clone(),
        model: params.model.clone(),
        effort: params.effort.clone(),
        label: None,
    };

    match send_request(spawn_request).await {
        Ok(ServerEvent::CommSpawnResponse { new_session_id, .. }) if !new_session_id.is_empty() => {
            Ok(new_session_id)
        }
        Ok(spawn_response) => {
            ensure_success(&spawn_response)?;
            Err(anyhow::anyhow!(
                "Spawn succeeded but new session ID was not returned."
            ))
        }
        Err(e) => Err(anyhow::anyhow!(
            "Failed to spawn agent for task assignment: {}",
            e
        )),
    }
}

async fn assign_task_to_session(
    ctx: &ToolContext,
    params: &CommunicateInput,
    target_session: String,
    spawned_suffix: &str,
) -> Result<ToolOutput> {
    let retry_request = Request::CommAssignTask {
        id: REQUEST_ID,
        session_id: ctx.session_id.clone(),
        target_session: Some(target_session.clone()),
        task_id: params.task_id.clone(),
        message: params.message.clone(),
    };

    match send_request(retry_request).await {
        Ok(ServerEvent::CommAssignTaskResponse { task_id, .. }) => Ok(ToolOutput::new(format!(
            "Task '{}' assigned to {}{}",
            task_id, target_session, spawned_suffix
        ))),
        Ok(retry_response) => {
            ensure_success(&retry_response)?;
            Ok(ToolOutput::new(format!(
                "Assigned next runnable task to {}{}",
                target_session, spawned_suffix
            )))
        }
        Err(e) => Err(anyhow::anyhow!(
            "Failed to assign task after selecting {}: {}",
            target_session,
            e
        )),
    }
}

fn format_context_entries(entries: &[ContextEntry]) -> ToolOutput {
    ToolOutput::new(format_comm_context_entries(entries))
}

fn format_members(ctx: &ToolContext, members: &[AgentInfo]) -> ToolOutput {
    ToolOutput::new(format_comm_members(&ctx.session_id, members))
}

fn format_tool_summary(target: &str, calls: &[ToolCallSummary]) -> ToolOutput {
    ToolOutput::new(format_comm_tool_summary(target, calls))
}

fn format_status_snapshot(snapshot: &AgentStatusSnapshot) -> ToolOutput {
    ToolOutput::new(format_comm_status_snapshot(snapshot))
}

fn format_plan_status(summary: &PlanGraphStatus) -> ToolOutput {
    let mut output = format_comm_plan_status(summary);
    if let Some(budget_line) = plan_status_budget_line(
        summary,
        crate::config::config().agents.swarm_max_concurrent_agents,
    ) {
        output.push_str(&budget_line);
    }
    ToolOutput::new(output)
}

/// Deep-mode budget line for `plan_status`: how wide the ready frontier is
/// versus the concurrency budget, with a widen-the-graph nudge when the ready
/// set cannot fill the slots. This makes under-utilization visible at plan
/// time, before `run_plan` even starts, so the coordinator can restructure the
/// graph instead of discovering the waste after the run. Pure over its inputs
/// for unit testing; returns `None` for light plans.
fn plan_status_budget_line(summary: &PlanGraphStatus, deep_cap: usize) -> Option<String> {
    if !summary.mode.eq_ignore_ascii_case("deep") {
        return None;
    }
    let budget = resolve_run_plan_concurrency(None, true, deep_cap);
    let budget_label = if budget == usize::MAX {
        format!("{} (member cap)", jcode_swarm_core::MAX_SWARM_MEMBERS)
    } else {
        budget.to_string()
    };
    let ready_width = summary.ready_ids.len();
    let active_width = summary.active_ids.len();
    let mut line = format!(
        "  Parallel budget: {} concurrent worker slot(s); ready set is {} wide ({} active).\n",
        budget_label, ready_width, active_width
    );
    let effective_budget = if budget == usize::MAX {
        jcode_swarm_core::MAX_SWARM_MEMBERS
    } else {
        budget
    };
    // Nudge only when narrowness is structural: the frontier cannot fill the
    // budget while other non-terminal work exists but is serialized behind
    // depends_on edges. A small plan that is simply almost done gets no nudge.
    let frontier = ready_width + active_width;
    let terminal = summary.completed_ids.len() + summary.cycle_ids.len();
    let serialized_remaining = summary.item_count > terminal + frontier;
    if frontier < effective_budget && serialized_remaining {
        line.push_str(
            "  The ready frontier is narrower than the budget while more work waits behind \
             depends_on edges: prefer expand_node with MANY independent siblings (depends_on \
             only for real data dependencies) to widen it.\n",
        );
    }
    Some(line)
}

fn format_context_history(target: &str, messages: &[HistoryMessage]) -> ToolOutput {
    ToolOutput::new(format_comm_context_history(target, messages))
}

#[cfg(test)]
fn format_awaited_members(
    completed: bool,
    summary: &str,
    members: &[AwaitedMemberStatus],
) -> ToolOutput {
    format_awaited_members_with_reports(completed, summary, members, &HashMap::new())
}

fn latest_assistant_report(messages: &[HistoryMessage]) -> Option<String> {
    latest_assistant_comm_report(messages)
}

fn resolve_optional_target_session(target: Option<String>, current_session: &str) -> String {
    resolve_optional_comm_target_session(target, current_session)
}

fn format_awaited_members_with_reports(
    completed: bool,
    summary: &str,
    members: &[AwaitedMemberStatus],
    reports: &HashMap<String, String>,
) -> ToolOutput {
    ToolOutput::new(format_comm_awaited_members_with_reports(
        completed, summary, members, reports,
    ))
}

async fn fetch_awaited_member_reports(
    ctx: &ToolContext,
    members: &[AwaitedMemberStatus],
) -> HashMap<String, String> {
    let mut reports = HashMap::new();
    for member in members.iter().filter(|member| member.done) {
        let request = Request::CommReadContext {
            id: REQUEST_ID,
            session_id: ctx.session_id.clone(),
            target_session: member.session_id.clone(),
        };
        match send_request(request).await {
            Ok(ServerEvent::CommContextHistory { messages, .. }) => {
                if let Some(report) = latest_assistant_report(&messages) {
                    reports.insert(member.session_id.clone(), report);
                }
            }
            Ok(response) => {
                if check_error(&response).is_some() {
                    continue;
                }
            }
            Err(_) => continue,
        }
    }
    reports
}

fn default_await_target_statuses() -> Vec<String> {
    default_comm_await_target_statuses()
}

fn format_channels(channels: &[SwarmChannelInfo]) -> ToolOutput {
    ToolOutput::new(format_comm_channels(channels))
}

/// Render the swarm model catalog for the `list_models` action: the current
/// (spawn-default) model, any config pin, and one line per route with
/// availability, auth method, and a relative cost estimate.
fn format_swarm_model_list(
    current_model: Option<&str>,
    configured_swarm_model: Option<&str>,
    model_routes: &[jcode_provider_core::ModelRoute],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Current model (spawn default when no override): {}\n",
        current_model.unwrap_or("unknown")
    ));
    match configured_swarm_model {
        Some(pin) if !pin.trim().is_empty() => {
            out.push_str(&format!("Configured agents.swarm_model pin: {pin}\n"));
        }
        _ => out.push_str("No agents.swarm_model pin configured (workers inherit the coordinator's model unless a per-spawn model is passed).\n"),
    }
    if model_routes.is_empty() {
        out.push_str(
            "\nNo model routes reported. Spawn with a bare model name or omit model to inherit.",
        );
        return out;
    }
    out.push_str("\nAvailable model routes (pass as spawn model, e.g. 'gpt-5.5' or route-pinned 'openai-api:gpt-5.5'):\n");
    for route in model_routes {
        let availability = if route.available {
            ""
        } else {
            " [unavailable]"
        };
        let cost = route
            .estimated_reference_cost_micros()
            .map(|micros| format!(" ~${:.2}/ref-task", micros as f64 / 1_000_000.0))
            .unwrap_or_default();
        let detail = if route.detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", route.detail)
        };
        out.push_str(&format!(
            "- {} via {} [{}]{}{}{}\n",
            route.model, route.provider, route.api_method, availability, cost, detail
        ));
    }
    out.push_str("\nAlso pass effort (none|minimal|low|medium|high|xhigh|max) to set the spawned agent's reasoning effort.");
    out
}

pub struct CommunicateTool {
    /// Full tool description including the user-tunable swarm prompt
    /// (model-routing guidance loaded from `swarm-prompt.md`). Computed once at
    /// registry construction so `description()` can hand out a borrowed str.
    description: String,
}

impl CommunicateTool {
    pub fn new() -> Self {
        const BASE_DESCRIPTION: &str = "Coordinate agents. Any agent can spawn child agents, and those children can spawn their own, forming a recursive spawn tree with no depth limit (growth is bounded only by the total swarm member cap). For spawn, prefer providing a prompt so the new agent starts with a concrete task instead of idling. Spawned/assigned agents automatically report their final response back to the agent that spawned them; you can stop any agent in the subtree you spawned.\n\nCommunication: prefer structural dataflow (task-graph artifacts via complete_node) over chat, and DMs for point-to-point coordination. broadcast reaches only your spawned subtree (whole swarm for the coordinator) and should be rare; channels and shared-context are discouraged legacy primitives.";
        let swarm_prompt = crate::prompt::load_swarm_prompt(None);
        let description = if swarm_prompt.is_empty() {
            BASE_DESCRIPTION.to_string()
        } else {
            format!(
                "{BASE_DESCRIPTION}\n\nSwarm prompt (user-tunable via ~/.jcode/swarm-prompt.md):\n{swarm_prompt}"
            )
        };
        Self { description }
    }
}

#[derive(Clone, Deserialize)]
struct CommunicateInput {
    action: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    to_session: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    proposer_session: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    target_session: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    initial_message: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    spawn_if_needed: Option<bool>,
    #[serde(default)]
    prefer_spawn: Option<bool>,
    #[serde(default)]
    plan_items: Option<Vec<PlanItem>>,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    gate_id: Option<String>,
    /// Task-DAG node specs for task_graph/expand_node/inject_gap actions.
    #[serde(default)]
    nodes: Option<Vec<crate::protocol::TaskGraphNodeSpec>>,
    /// Handoff artifact (object) for complete_node.
    #[serde(default)]
    artifact: Option<serde_json::Value>,
    #[serde(default)]
    target_status: Option<Vec<String>>,
    #[serde(default)]
    session_ids: Option<Vec<String>>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    timeout_minutes: Option<u64>,
    #[serde(default)]
    wake: Option<bool>,
    #[serde(default)]
    background: Option<bool>,
    #[serde(default)]
    notify: Option<bool>,
    #[serde(default)]
    delivery: Option<CommDeliveryMode>,
    #[serde(default)]
    concurrency_limit: Option<usize>,
    #[serde(default)]
    force: Option<bool>,
    #[serde(default)]
    retain_agents: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    validation: Option<String>,
    #[serde(default)]
    follow_up: Option<String>,
    #[serde(default)]
    spawn_mode: Option<String>,
    /// One-line summary shown collapsed in the recipient's UI for long
    /// message/report bodies. Required when the body exceeds the collapse
    /// threshold.
    #[serde(default)]
    tldr: Option<String>,
    /// Per-spawn model override for spawn/assign_task/assign_next/run_plan
    /// spawns. Takes precedence over agents.swarm_model config.
    #[serde(default)]
    model: Option<String>,
    /// Reasoning effort for spawned agents (none|minimal|low|medium|high|xhigh|max).
    #[serde(default)]
    effort: Option<String>,
    /// Short human-readable label for a spawned agent shown in swarm UI.
    /// Required and nonblank for the explicit `spawn` action.
    #[serde(default)]
    label: Option<String>,
}

impl CommunicateInput {
    fn spawn_initial_message(&self) -> Option<String> {
        self.initial_message.clone().or_else(|| self.prompt.clone())
    }

    fn required_spawn_label(&self) -> anyhow::Result<String> {
        let label = self
            .label
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("'label' is required for spawn action"))?
            .trim();
        if label.is_empty() {
            return Err(anyhow::anyhow!(
                "'label' must not be blank for spawn action"
            ));
        }
        Ok(label.to_string())
    }
}

/// Map common action synonyms/typos to the canonical swarm action name. Models
/// frequently invent near-miss verbs (e.g. `inbox` for reading messages, `send`
/// for `message`), which previously produced an "Unknown action" error. Unknown
/// inputs are returned unchanged so the normal validation path still reports them.
fn canonical_swarm_action(action: &str) -> &str {
    match action.trim().to_ascii_lowercase().as_str() {
        "inbox" | "messages" | "check_messages" | "read_messages" | "read_inbox" => "read",
        "send" | "msg" | "send_message" => "message",
        "dm_session" | "direct_message" | "whisper" => "dm",
        "broadcast_all" | "announce" => "broadcast",
        "agents" | "members" | "list_agents" | "list_members" | "roster" => "list",
        "models" | "model_list" | "list_model" | "list_providers" | "list_routes" => "list_models",
        "plan" | "status_plan" => "plan_status",
        "assign" => "assign_task",
        "kill" | "terminate" => "stop",
        _ => action,
    }
}

#[async_trait]
impl Tool for CommunicateTool {
    fn name(&self) -> &str {
        "swarm"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["share", "share_append", "read", "message", "broadcast", "dm", "channel", "list", "list_channels", "channel_members",
                             "propose_plan", "approve_plan", "reject_plan", "spawn", "stop", "assign_role",
                             "status", "report", "plan_status", "summary", "read_context", "resync_plan", "assign_task", "assign_next", "fill_slots", "run_plan", "cleanup",
                             "task_graph", "expand_node", "complete_node", "inject_gap",
                             "start", "start_task", "wake", "resume", "retry", "reassign", "replace", "salvage",
                             "subscribe_channel", "unsubscribe_channel", "await_members", "list_models"],
                    "description": "Action. Spawn requires a nonblank label and should include prompt with the initial task so the new agent starts useful work immediately. Use list_models to see which models/routes are available for per-spawn model selection."
                },
                "key": {
                    "type": "string",
                    "description": "Shared-context key for share/share_append/read. Discouraged: prefer the repo and typed node artifacts as the shared medium; use shared context only for small non-repo state."
                },
                "value": {
                    "type": "string"
                },
                "message": {
                    "type": "string",
                    "description": "Message body. For action=message, routes by fields provided: with to_session it is a DM, with channel it posts to that channel, with neither it broadcasts to your spawned subtree. For action=report, this is the completion report body."
                },
                "tldr": {
                    "type": "string",
                    "description": "One-line summary (aim for under 120 chars) of the message/report. Required for message/broadcast/dm/channel/report when the body is longer than 240 chars. The recipient's UI shows this collapsed with an expand control instead of the full body."
                },
                "status": {
                    "type": "string",
                    "description": "For action=report: completion status to record, usually ready, blocked, failed, or completed. Defaults to ready."
                },
                "validation": {
                    "type": "string",
                    "description": "For action=report: tests or validation performed."
                },
                "follow_up": {
                    "type": "string",
                    "description": "For action=report: blockers or follow-up work."
                },
                "to_session": {
                    "type": "string",
                    "description": "Target session for actions that address one agent (dm, and as an alias for target_session). Accepts an exact session ID or a unique friendly name within the swarm. Interchangeable with target_session. If a friendly name is ambiguous, run swarm list and use the exact session ID."
                },
                "channel": {
                    "type": "string",
                    "description": "Channel name. For action=channel (or action=message with a channel) the message goes to subscribers of this channel. Also used by subscribe_channel/unsubscribe_channel/channel_members. Discouraged: prefer DMs and task-graph artifacts over ad hoc channels."
                },
                "proposer_session": { "type": "string" },
                "reason": { "type": "string" },
                "target_session": {
                    "type": "string",
                    "description": "Target session for management actions (assign_role, summary, status, stop, start, resume, wake, etc.). Accepts an exact session ID or a unique friendly name. Interchangeable with to_session."
                },
                "role": {
                    "type": "string",
                    "enum": ["agent", "coordinator"]
                },
                "label": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Required for spawn. Short nonblank label shown on the spawned agent's chip in swarm UI (e.g. 'api reviewer')."
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory for spawn."
                },
                "prompt": {
                    "type": "string",
                    "description": "Preferred for spawn. Initial task/instructions for the new agent. Spawning without prompt usually creates an idle agent that needs follow-up assignment."
                },
                "initial_message": {
                    "type": "string",
                    "description": "Explicit initial task/instructions for spawn. If both initial_message and prompt are supplied, initial_message wins."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional max items for summary-style reads."
                },
                "task_id": {
                    "type": "string",
                    "description": "Optional plan task ID. If omitted for assign_task/assign_next, the coordinator picks a runnable task. If omitted for resume/wake/retry/start with target_session, the server resumes the unique assigned task for that session."
                },
                "spawn_if_needed": {
                    "type": "boolean",
                    "description": "For assign_task without an explicit target_session: if no reusable agent is available, spawn a fresh agent and retry the assignment automatically."
                },
                "prefer_spawn": {
                    "type": "boolean",
                    "description": "For assign_task without an explicit target_session: prefer a fresh spawned agent even if reusable workers are available."
                },
                "spawn_mode": {
                    "type": "string",
                    "enum": ["visible", "headless", "inline", "auto"],
                    "description": "Per-call spawn mode for swarm-created agents. Overrides agents.swarm_spawn_mode config when set. 'visible' opens a terminal window, 'headless' runs in-process with no UI, 'inline' runs in-process and renders a live gallery viewport in the coordinator, 'auto' tries visible then falls back to headless. Defaults to inline."
                },
                "model": {
                    "type": "string",
                    "description": "Optional model for the spawned agent (spawn, and spawns triggered by assign_task/assign_next/run_plan). Overrides the agents.swarm_model config pin for this call. Accepts a bare model name (e.g. 'gpt-5.5') or an auth-route-prefixed form (e.g. 'openai-api:gpt-5.5', 'claude-api:claude-fable-5'). Use 'inherit' to force coordinator inheritance. Omit to use the configured/coordinator default. Run action=list_models to see available models and routes."
                },
                "effort": {
                    "type": "string",
                    "enum": ["none", "minimal", "low", "medium", "high", "xhigh", "max"],
                    "description": "Optional reasoning effort for the spawned agent. Omit for the model's default. Only meaningful with spawn-creating actions."
                },
                "session_ids": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "mode": {
                    "type": "string",
                    "enum": ["all", "any", "deep", "light"],
                    "description": "For task_graph: deep enables comprehensive gated execution and light enables simple fan-out. For await_members: wait for all targeted members or wake when any targeted member matches."
                },
                "target_status": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional completion statuses for await_members. Defaults to ready/completed/stopped/failed."
                },
                "timeout_minutes": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional timeout for await_members."
                },
                "background": {
                    "type": "boolean",
                    "description": "For run_plan: run as a detached background task (default true); set false to block until the plan resolves. await_members is always asynchronous and ignores false so the agent stays responsive; its result is delivered later via notify/wake."
                },
                "notify": {
                    "type": "boolean",
                    "description": "For await_members/run_plan: surface a notification card when the background task resolves. Defaults to true."
                },
                "concurrency_limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Max swarm worker agents active at once. For fill_slots this is required. For run_plan it is optional and overrides the mode-based default (deep fans out wide up to agents.swarm_max_concurrent_agents; light uses a small default). Total agents over the whole run is still bounded only by the swarm member cap."
                },
                "force": {
                    "type": "boolean",
                    "description": "For stop/cleanup: allow stopping non-owned/user-created swarm sessions. Defaults to false."
                },
                "retain_agents": {
                    "type": "boolean",
                    "description": "For run_plan: keep spawned workers after the plan reaches a terminal state. Defaults to false, so owned workers are cleaned up."
                },
                "wake": {
                    "type": "boolean",
                    "description": "Optional wake hint for messages. For await_members/run_plan: wake this agent with the result when the background task resolves (default true); if false, only notify."
                },
                "delivery": {
                    "type": "string",
                    "enum": ["notify", "interrupt", "wake"],
                    "description": "Optional delivery mode for dm/channel messaging."
                },
                "plan_items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": true
                    }
                }
            }
        });

        // Task-DAG properties are added after the macro to keep `json!` nesting
        // depth under the macro recursion limit.
        if let Some(props) = schema
            .get_mut("properties")
            .and_then(|value| value.as_object_mut())
        {
            props.insert(
                "node_id".to_string(),
                json!({
                    "type": "string",
                    "description": "Task-DAG node id for expand_node/complete_node."
                }),
            );
            props.insert(
                "gate_id".to_string(),
                json!({
                    "type": "string",
                    "description": "Gate node id for inject_gap (a critique/verify gate the caller owns)."
                }),
            );
            props.insert(
                "nodes".to_string(),
                json!({
                    "type": "array",
                    "description": "Task-DAG node specs for task_graph (seed), expand_node (children), or inject_gap (gap/fix nodes). Each: {id, content, kind?, depends_on?, priority?}. kind is one of explore|implement|verify|fix|synthesize.",
                    "items": { "type": "object", "additionalProperties": true }
                }),
            );
            props.insert(
                "artifact".to_string(),
                json!({
                    "type": "object",
                    "description": "Typed handoff artifact for complete_node. In deep mode requires non-empty 'findings', a 'what_i_did_not_check' list, and a 'confidence' of low|medium|high (report low honestly; it routes follow-up work). Deep gates cannot pass while a low-confidence sibling is unaddressed: inject_gap or name the id in findings. Fields: findings, evidence[], edge_cases_considered[], validation, open_questions[], confidence, what_i_did_not_check[].",
                    "additionalProperties": true
                }),
            );
        }

        // `swarm` is a multi-action tool, so putting `label` in the top-level
        // `required` array would incorrectly require it for read/list/message and
        // every other action. Use mutually exclusive action branches instead:
        // the spawn branch requires label, while the non-spawn branch does not.
        // `anyOf` object branches are supported by our provider schema adapters
        // and avoid the less-portable JSON Schema `if`/`then` keywords.
        let non_spawn_actions: Vec<Value> = schema["properties"]["action"]["enum"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|action| action.as_str() != Some("spawn"))
            .cloned()
            .collect();
        schema["anyOf"] = json!([
            {
                "type": "object",
                "required": ["action", "label"],
                "properties": {
                    "action": { "type": "string", "enum": ["spawn"] }
                }
            },
            {
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": non_spawn_actions }
                }
            }
        ]);

        schema
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let mut params: CommunicateInput = serde_json::from_value(input)?;

        // `to_session` and `target_session` both name a single session id. Historically
        // different actions required different field names (e.g. `dm` wanted `to_session`
        // while `assign_role`/`summary`/`status`/`start`/`resume` wanted `target_session`),
        // which models frequently confuse, producing repeated "'to_session' is required" /
        // "'target_session' is required" errors. Treat the two fields as interchangeable
        // aliases so either name works for any action that targets a session.
        match (params.to_session.is_some(), params.target_session.is_some()) {
            (true, false) => params.target_session = params.to_session.clone(),
            (false, true) => params.to_session = params.target_session.clone(),
            _ => {}
        }

        // Normalize common action synonyms that models invent (e.g. `inbox`, `send`,
        // `msg`) so a near-miss verb maps to the real action instead of erroring out.
        params.action = canonical_swarm_action(&params.action).to_string();

        match params.action.as_str() {
            "share" | "share_append" => {
                let key = params
                    .key
                    .ok_or_else(|| anyhow::anyhow!("'key' is required for share action"))?;
                let value = params
                    .value
                    .ok_or_else(|| anyhow::anyhow!("'value' is required for share action"))?;

                let request = Request::CommShare {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    key: key.clone(),
                    value: value.clone(),
                    append: params.action == "share_append",
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        let verb = if params.action == "share_append" {
                            "Appended shared context"
                        } else {
                            "Shared with other agents"
                        };
                        Ok(ToolOutput::new(format!("{}: {} = {}", verb, key, value)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to share: {}", e)),
                }
            }

            "read" => {
                let request = Request::CommRead {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    key: params.key.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommContext { entries, .. }) => {
                        Ok(format_context_entries(&entries))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No shared context found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to read shared context: {}", e)),
                }
            }

            "message" => {
                // `message` is the general-purpose send: it routes by the fields
                // provided. With `to_session` it acts as a DM, with `channel` it
                // posts to that channel, and with neither it broadcasts to the
                // sender's spawned subtree (whole swarm only for the coordinator).
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for message action"))?;
                let tldr = validate_swarm_tldr(params.tldr.as_deref(), &message, "this message")
                    .map_err(|e| anyhow::anyhow!(e))?;
                let to_session = params.to_session.clone();
                let channel = params.channel.clone();

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: to_session.clone(),
                    channel: channel.clone(),
                    wake: params.wake,
                    delivery: params.delivery,
                    tldr,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        let confirmation = match (to_session, channel) {
                            (Some(target), _) => {
                                format!("Direct message sent to {}: {}", target, message)
                            }
                            (None, Some(channel)) => {
                                format!("Channel message sent to #{}: {}", channel, message)
                            }
                            (None, None) => {
                                format!("Broadcast sent to your spawned subtree: {}", message)
                            }
                        };
                        Ok(ToolOutput::new(confirmation))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to send message: {}", e)),
                }
            }

            "broadcast" => {
                // `broadcast` targets the sender's spawned subtree (the swarm
                // coordinator reaches the whole swarm). Any `to_session`/
                // `channel` is intentionally ignored so the action stays an
                // unambiguous group send; use `message`/`dm`/`channel` to target.
                // Prefer DMs or task-graph artifacts; group sends are for rare
                // coordination moments, not routine status updates.
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for broadcast action"))?;
                let tldr = validate_swarm_tldr(params.tldr.as_deref(), &message, "this broadcast")
                    .map_err(|e| anyhow::anyhow!(e))?;

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: None,
                    channel: None,
                    wake: params.wake,
                    delivery: params.delivery,
                    tldr,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Broadcast sent to your spawned subtree: {}",
                            message
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to broadcast message: {}", e)),
                }
            }

            "dm" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for dm action"))?;
                let tldr = validate_swarm_tldr(params.tldr.as_deref(), &message, "this DM")
                    .map_err(|e| anyhow::anyhow!(e))?;
                let to_session = params.to_session.ok_or_else(|| {
                    anyhow::anyhow!("'to_session' (or 'target_session') is required for dm action")
                })?;

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: Some(to_session.clone()),
                    channel: None,
                    delivery: params.delivery,
                    wake: params.wake,
                    tldr,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Direct message sent to {}: {}",
                            to_session, message
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to send DM: {}", e)),
                }
            }

            "channel" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for channel action"))?;
                let tldr =
                    validate_swarm_tldr(params.tldr.as_deref(), &message, "this channel message")
                        .map_err(|e| anyhow::anyhow!(e))?;
                let channel = params
                    .channel
                    .ok_or_else(|| anyhow::anyhow!("'channel' is required for channel action"))?;

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: None,
                    channel: Some(channel.clone()),
                    delivery: params.delivery,
                    wake: params.wake,
                    tldr,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Channel message sent to #{}: {}",
                            channel, message
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to send channel message: {}", e)),
                }
            }

            "list" => {
                let request = Request::CommList {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommMembers { members, .. }) => {
                        Ok(format_members(&ctx, &members))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No agents found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list agents: {}", e)),
                }
            }

            "list_channels" => {
                let request = Request::CommListChannels {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommChannels { channels, .. }) => {
                        Ok(format_channels(&channels))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No channels found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list channels: {}", e)),
                }
            }

            "channel_members" => {
                let channel = params.channel.ok_or_else(|| {
                    anyhow::anyhow!("'channel' is required for channel_members action")
                })?;
                let request = Request::CommChannelMembers {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    channel: channel.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommMembers { members, .. }) => {
                        let mut output = format!("Members subscribed to #{}:\n\n", channel);
                        if members.is_empty() {
                            output.push_str("  (none)\n");
                        } else {
                            for member in members {
                                let name = member.friendly_name.unwrap_or(member.session_id);
                                let status = member.status.unwrap_or_else(|| "unknown".to_string());
                                output.push_str(&format!("  {} ({})\n", name, status));
                            }
                        }
                        Ok(ToolOutput::new(output))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No channel members found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list channel members: {}", e)),
                }
            }

            "propose_plan" => {
                let items = params.plan_items.ok_or_else(|| {
                    anyhow::anyhow!("'plan_items' is required for propose_plan action")
                })?;
                if items.is_empty() {
                    return Err(anyhow::anyhow!(
                        "'plan_items' must include at least one item"
                    ));
                }
                let item_count = items.len() as u64;

                let request = Request::CommProposePlan {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    items,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Plan proposal submitted ({} items).",
                            item_count
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to propose plan: {}", e)),
                }
            }

            "approve_plan" => {
                let proposer = params.proposer_session.ok_or_else(|| {
                    anyhow::anyhow!("'proposer_session' is required for approve_plan action")
                })?;

                let request = Request::CommApprovePlan {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    proposer_session: proposer.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Approved plan proposal from {}",
                            proposer
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to approve plan: {}", e)),
                }
            }

            "reject_plan" => {
                let proposer = params.proposer_session.ok_or_else(|| {
                    anyhow::anyhow!("'proposer_session' is required for reject_plan action")
                })?;
                let reason = params.reason.clone();

                let request = Request::CommRejectPlan {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    proposer_session: proposer.clone(),
                    reason: reason.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        let reason_msg = reason
                            .as_ref()
                            .map(|r| format!(" (reason: {})", r))
                            .unwrap_or_default();
                        Ok(ToolOutput::new(format!(
                            "Rejected plan proposal from {}{}",
                            proposer, reason_msg
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to reject plan: {}", e)),
                }
            }

            "task_graph" | "seed_graph" => {
                let nodes = params
                    .nodes
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("'nodes' is required for task_graph action"))?;
                if nodes.is_empty() {
                    return Err(anyhow::anyhow!("'nodes' must include at least one node"));
                }
                let count = nodes.len();
                let mut seed_nodes = nodes.clone();
                let request = Request::CommSeedGraph {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    mode: params.mode.clone(),
                    nodes: seed_nodes.clone(),
                };
                let mut response = send_request(request)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to seed task graph: {}", e))?;
                let mut changes = Vec::new();
                let mut occupied = None;
                // At most one durable collision can be resolved per request. The
                // extra iteration consumes the final success response after all
                // colliding ids have been remapped.
                for _ in 0..=nodes.len() {
                    let Some(conflicting_id) = seed_node_id_collision(&response) else {
                        ensure_success(&response)?;
                        let suffix = if changes.is_empty() {
                            String::new()
                        } else {
                            format!(
                                " Renamed conflicting ids: {}.",
                                format_seed_remaps(&changes)
                            )
                        };
                        return Ok(ToolOutput::new(format!(
                            "Seeded task graph ({} nodes).{}",
                            count, suffix
                        )));
                    };
                    if occupied.is_none() {
                        let summary = fetch_plan_status(&ctx.session_id).await?;
                        occupied = Some(plan_graph_node_ids(&summary));
                    }
                    let occupied = occupied
                        .as_ref()
                        .expect("occupied ids were initialized above");
                    let (remapped, mut remaps) = remap_conflicting_seed_nodes(
                        &seed_nodes,
                        occupied,
                        conflicting_id,
                        &seed_retry_scope(&ctx),
                    );
                    if remaps.is_empty() {
                        ensure_success(&response)?;
                        unreachable!("a duplicate seed error should have returned above")
                    }
                    seed_nodes = remapped;
                    changes.append(&mut remaps);
                    response = send_request(Request::CommSeedGraph {
                        id: REQUEST_ID,
                        session_id: ctx.session_id.clone(),
                        mode: params.mode.clone(),
                        nodes: seed_nodes.clone(),
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to retry task graph seed: {}", e))?;
                }
                ensure_success(&response)?;
                unreachable!("seed retry loop only exhausts while the server returns collisions")
            }

            "expand_node" => {
                let node_id = params.node_id.clone().ok_or_else(|| {
                    anyhow::anyhow!("'node_id' is required for expand_node action")
                })?;
                let children = params.nodes.clone().ok_or_else(|| {
                    anyhow::anyhow!("'nodes' (children) is required for expand_node action")
                })?;
                if children.is_empty() {
                    return Err(anyhow::anyhow!("expand_node requires at least one child"));
                }
                let count = children.len();
                let request = Request::CommExpandNode {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    node_id: node_id.clone(),
                    children,
                };
                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Decomposed '{}' into {} children.",
                            node_id, count
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to expand node: {}", e)),
                }
            }

            "complete_node" => {
                let node_id = params.node_id.clone().ok_or_else(|| {
                    anyhow::anyhow!("'node_id' is required for complete_node action")
                })?;
                let artifact_json = match params.artifact.clone() {
                    Some(value) => serde_json::to_string(&value)
                        .map_err(|e| anyhow::anyhow!("invalid artifact: {}", e))?,
                    None => {
                        return Err(anyhow::anyhow!(
                            "'artifact' object is required for complete_node action"
                        ));
                    }
                };
                let request = Request::CommCompleteNode {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    node_id: node_id.clone(),
                    artifact_json,
                };
                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Completed node '{}'.", node_id)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to complete node: {}", e)),
                }
            }

            "inject_gap" => {
                let gate_id = params
                    .gate_id
                    .clone()
                    .or_else(|| params.node_id.clone())
                    .ok_or_else(|| {
                        anyhow::anyhow!("'gate_id' is required for inject_gap action")
                    })?;
                let nodes = params
                    .nodes
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("'nodes' is required for inject_gap action"))?;
                if nodes.is_empty() {
                    return Err(anyhow::anyhow!("inject_gap requires at least one node"));
                }
                let count = nodes.len();
                let request = Request::CommInjectGap {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    gate_id: gate_id.clone(),
                    nodes,
                };
                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Injected {} gap node(s) from gate '{}'.",
                            count, gate_id
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to inject gap nodes: {}", e)),
                }
            }

            "spawn" => {
                let label = params.required_spawn_label()?;
                let request = Request::CommSpawn {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    working_dir: params.working_dir.clone(),
                    initial_message: params.spawn_initial_message(),
                    request_nonce: None,
                    spawn_mode: params.spawn_mode.clone(),
                    model: params.model.clone(),
                    effort: params.effort.clone(),
                    label: Some(label),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommSpawnResponse { new_session_id, .. })
                        if !new_session_id.is_empty() =>
                    {
                        Ok(ToolOutput::new(format!(
                            "Spawned new agent: {}",
                            new_session_id
                        )))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Err(anyhow::anyhow!(
                            "Spawn succeeded but new session ID was not returned."
                        ))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to spawn agent: {}", e)),
                }
            }

            "list_models" => {
                let request = Request::CommListModels {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                };
                match send_request(request).await {
                    Ok(ServerEvent::CommListModelsResponse {
                        current_model,
                        configured_swarm_model,
                        model_routes,
                        ..
                    }) => Ok(ToolOutput::new(format_swarm_model_list(
                        current_model.as_deref(),
                        configured_swarm_model.as_deref(),
                        &model_routes,
                    ))),
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No model catalog returned."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list models: {}", e)),
                }
            }

            "stop" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for stop action")
                })?;

                let request = Request::CommStop {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                    force: params.force,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Stopped agent: {}", target)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to stop agent: {}", e)),
                }
            }

            "cleanup" => cleanup_swarm_workers(&ctx, &params)
                .await
                .map(ToolOutput::new),

            "assign_role" => {
                let target_raw = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for assign_role action")
                })?;
                let role = params
                    .role
                    .ok_or_else(|| anyhow::anyhow!("'role' is required for assign_role action"))?;

                // Resolve "current" to the caller's own session ID
                let target = if target_raw == "current" {
                    ctx.session_id.clone()
                } else {
                    target_raw
                };

                let request = Request::CommAssignRole {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                    role: role.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Assigned role '{}' to {}",
                            role, target
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to assign role: {}", e)),
                }
            }

            "status" => {
                let target =
                    resolve_optional_target_session(params.target_session, &ctx.session_id);

                let request = Request::CommStatus {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommStatusResponse { snapshot, .. }) => {
                        Ok(format_status_snapshot(&snapshot))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No status snapshot returned."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to get status snapshot: {}", e)),
                }
            }

            "report" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for report action"))?;
                let tldr = validate_swarm_tldr(params.tldr.as_deref(), &message, "this report")
                    .map_err(|e| anyhow::anyhow!(e))?;
                let request = Request::CommReport {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    status: params.status,
                    message,
                    validation: params.validation,
                    follow_up: params.follow_up,
                    tldr,
                };
                match send_request(request).await {
                    Ok(ServerEvent::CommReportResponse {
                        status, message, ..
                    }) => Ok(ToolOutput::new(format!(
                        "Report recorded with status `{status}`. {message}"
                    ))),
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("Report recorded."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to record report: {}", e)),
                }
            }

            "plan_status" => {
                let summary = fetch_plan_status(&ctx.session_id).await?;
                Ok(format_plan_status(&summary))
            }

            "summary" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for summary action")
                })?;

                let request = Request::CommSummary {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                    limit: params.limit,
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommSummaryResponse { tool_calls, .. }) => {
                        Ok(format_tool_summary(&target, &tool_calls))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No tool call data returned."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to get summary: {}", e)),
                }
            }

            "read_context" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for read_context action")
                })?;

                let request = Request::CommReadContext {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommContextHistory { messages, .. }) => {
                        Ok(format_context_history(&target, &messages))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No context data returned."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to read context: {}", e)),
                }
            }

            "resync_plan" => {
                let request = Request::CommResyncPlan {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("Swarm plan re-synced to your session."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to resync plan: {}", e)),
                }
            }

            "assign_task" => {
                let target = params
                    .target_session
                    .clone()
                    .unwrap_or_else(|| "next available agent".to_string());
                let spawn_if_needed = params.spawn_if_needed.unwrap_or(false);
                let prefer_spawn = params.prefer_spawn.unwrap_or(false);

                if prefer_spawn && params.target_session.is_none() {
                    let spawned_session = spawn_assignment_session(&ctx, &params).await?;
                    return assign_task_to_session(
                        &ctx,
                        &params,
                        spawned_session,
                        " (spawned by planner preference)",
                    )
                    .await;
                }

                let request = Request::CommAssignTask {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: params.target_session.clone(),
                    task_id: params.task_id.clone(),
                    message: params.message.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommAssignTaskResponse {
                        task_id,
                        target_session,
                        ..
                    }) => {
                        let mut output =
                            format!("Task '{}' assigned to {}", task_id, target_session);
                        if let Ok(summary) = fetch_plan_status(&ctx.session_id).await {
                            output.push_str(&format!("\n{}", format_plan_followup(&summary)));
                        }
                        Ok(ToolOutput::new(output))
                    }
                    Ok(response)
                        if spawn_if_needed
                            && params.target_session.is_none()
                            && auto_assignment_needs_spawn(&response) =>
                    {
                        let spawned_session = spawn_assignment_session(&ctx, &params).await?;
                        assign_task_to_session(
                            &ctx,
                            &params,
                            spawned_session,
                            " (spawned automatically)",
                        )
                        .await
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        let msg = params.task_id.as_deref().map_or_else(
                            || format!("Assigned next runnable task to {}", target),
                            |task_id| format!("Task '{}' assigned to {}", task_id, target),
                        );
                        Ok(ToolOutput::new(msg))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to assign task: {}", e)),
                }
            }

            "assign_next" => {
                let target = params
                    .target_session
                    .clone()
                    .unwrap_or_else(|| "next available agent".to_string());

                let request = Request::CommAssignNext {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: params.target_session.clone(),
                    working_dir: params.working_dir.clone(),
                    prefer_spawn: params.prefer_spawn,
                    spawn_if_needed: params.spawn_if_needed,
                    message: params.message.clone(),
                    model: params.model.clone(),
                    effort: params.effort.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommAssignTaskResponse {
                        task_id,
                        target_session,
                        ..
                    }) => Ok(ToolOutput::new(format!(
                        "Task '{}' assigned to {}",
                        task_id, target_session
                    ))),
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Assigned next runnable task to {}",
                            target
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to assign next task: {}", e)),
                }
            }

            "fill_slots" => {
                let concurrency_limit = params.concurrency_limit.ok_or_else(|| {
                    anyhow::anyhow!("'concurrency_limit' is required for fill_slots action")
                })?;

                let summary = fetch_plan_status(&ctx.session_id).await?;
                let members = fetch_swarm_members(&ctx.session_id).await?;

                let active_count =
                    coordination_in_flight_count(&summary, &members, &ctx.session_id);
                if active_count >= concurrency_limit {
                    return Ok(ToolOutput::new(format!(
                        "Window already full: {} active/in-flight task(s) >= limit {}",
                        active_count, concurrency_limit
                    )));
                }

                let mut assignments = Vec::new();
                let available_slots = concurrency_limit.saturating_sub(active_count);
                for _ in 0..available_slots {
                    let request = Request::CommAssignNext {
                        id: REQUEST_ID,
                        session_id: ctx.session_id.clone(),
                        target_session: params.target_session.clone(),
                        working_dir: params.working_dir.clone(),
                        prefer_spawn: params.prefer_spawn,
                        spawn_if_needed: params.spawn_if_needed,
                        message: params.message.clone(),
                        model: params.model.clone(),
                        effort: params.effort.clone(),
                    };

                    match send_request(request).await {
                        Ok(ServerEvent::CommAssignTaskResponse {
                            task_id,
                            target_session,
                            ..
                        }) => assignments.push(format!("{} -> {}", task_id, target_session)),
                        Ok(ServerEvent::Error { message, .. })
                            if message.contains("No runnable unassigned tasks")
                                || message.contains("No ready or completed swarm agents") =>
                        {
                            break;
                        }
                        Ok(response) => {
                            ensure_success(&response)?;
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("Failed to fill slots: {}", e));
                        }
                    }
                }

                if assignments.is_empty() {
                    Ok(ToolOutput::new(format!(
                        "No assignments made. Active: {}, limit: {}",
                        active_count, concurrency_limit
                    )))
                } else {
                    let mut output = format!(
                        "Filled {} slot(s):\n{}",
                        assignments.len(),
                        assignments
                            .into_iter()
                            .map(|line| format!("- {}", line))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    if let Ok(summary) = fetch_plan_status(&ctx.session_id).await {
                        output.push_str(&format!("\n{}", format_plan_followup(&summary)));
                    }
                    Ok(ToolOutput::new(output))
                }
            }

            "run_plan" => {
                // Background-by-default: the plan driver runs as a managed
                // background task (progress card, bg tool, notify/wake) so the
                // coordinating agent stays responsive. Pass background=false
                // to block inline until the plan reaches a terminal state.
                if params.background.unwrap_or(true) {
                    run_swarm_plan_in_background(&ctx, params.clone()).await
                } else {
                    run_swarm_plan_to_terminal(&ctx, &params, &RunPlanReporter::inline()).await
                }
            }

            "start" | "start_task" | "wake" | "resume" | "retry" | "reassign" | "replace"
            | "salvage" => {
                let task_id = match params.task_id.clone() {
                    Some(task_id) => task_id,
                    None if params.target_session.is_some() => String::new(),
                    None => {
                        return Err(anyhow::anyhow!(
                            "'task_id' is required for {} action unless 'target_session' uniquely identifies the assigned task. Use `swarm list`/`swarm plan_status` to inspect assignments, or pass task_id explicitly.",
                            params.action
                        ));
                    }
                };
                if matches!(params.action.as_str(), "reassign" | "replace" | "salvage")
                    && params.target_session.is_none()
                {
                    return Err(anyhow::anyhow!(
                        "'target_session' is required for {} action",
                        params.action
                    ));
                }

                let control_action = if params.action == "start_task" {
                    "start".to_string()
                } else {
                    params.action.clone()
                };

                let request = Request::CommTaskControl {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    action: control_action.clone(),
                    task_id: task_id.clone(),
                    target_session: params.target_session.clone(),
                    message: params.message.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommTaskControlResponse {
                        task_id,
                        action,
                        target_session,
                        status,
                        summary,
                        ..
                    }) => {
                        let mut output = format!("Task '{}' {}", task_id, action);
                        if let Some(target_session) = target_session {
                            output.push_str(&format!(" -> {}", target_session));
                        }
                        output.push_str(&format!("\nStatus: {}", status));
                        if !summary.next_ready_ids.is_empty() {
                            output.push_str(&format!(
                                "\nNext ready: {}",
                                summary.next_ready_ids.join(", ")
                            ));
                        }
                        if !summary.newly_ready_ids.is_empty() {
                            output.push_str(&format!(
                                "\nNewly ready: {}",
                                summary.newly_ready_ids.join(", ")
                            ));
                        }
                        Ok(ToolOutput::new(output))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        let target_suffix = params
                            .target_session
                            .as_deref()
                            .map(|target| format!(" -> {}", target))
                            .unwrap_or_default();
                        Ok(ToolOutput::new(format!(
                            "Task '{}' {}{}",
                            task_id, params.action, target_suffix
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to {} task: {}", control_action, e)),
                }
            }

            "subscribe_channel" => {
                let channel = params.channel.ok_or_else(|| {
                    anyhow::anyhow!("'channel' is required for subscribe_channel action")
                })?;

                let request = Request::CommSubscribeChannel {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    channel: channel.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Subscribed to #{}", channel)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to subscribe: {}", e)),
                }
            }

            "unsubscribe_channel" => {
                let channel = params.channel.ok_or_else(|| {
                    anyhow::anyhow!("'channel' is required for unsubscribe_channel action")
                })?;

                let request = Request::CommUnsubscribeChannel {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    channel: channel.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Unsubscribed from #{}", channel)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to unsubscribe: {}", e)),
                }
            }

            "await_members" => {
                let target_status = params
                    .target_status
                    .unwrap_or_else(default_await_target_statuses);
                let mut session_ids = params.session_ids.unwrap_or_default();
                if let Some(target_session) = params.target_session.clone()
                    && !session_ids.iter().any(|id| id == &target_session)
                {
                    session_ids.push(target_session);
                }
                let timeout_minutes = params.timeout_minutes.unwrap_or(60);
                let timeout_secs = timeout_minutes * 60;
                // Public member waits are always asynchronous. The blocking
                // CommAwaitMembers protocol remains available internally for the
                // run_plan coordination loop, but agents must not park an entire
                // turn waiting on a worker or a long-lived socket.
                let blocking_was_requested = params.background == Some(false);
                let background = true;
                let notify = params.notify.unwrap_or(true);
                let wake = params.wake.unwrap_or(true);

                let request = Request::CommAwaitMembers {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_status,
                    session_ids,
                    mode: params.mode.clone(),
                    timeout_secs: Some(timeout_secs),
                    background,
                    notify,
                    wake,
                };

                // Background waits return promptly with a snapshot; only blocking
                // waits need the long socket timeout that covers the full wait.
                let socket_timeout = if background {
                    std::time::Duration::from_secs(30)
                } else {
                    std::time::Duration::from_secs(timeout_secs + 30)
                };

                match send_request_with_timeout(request, Some(socket_timeout)).await {
                    Ok(ServerEvent::CommAwaitMembersResponse {
                        completed,
                        members,
                        summary,
                        background_started,
                        ..
                    }) => {
                        if background_started {
                            let compatibility_note = if blocking_was_requested {
                                "\n\n(Blocking member waits are no longer supported; this wait was started asynchronously.)"
                            } else {
                                "\n\n(You can keep working; this wait runs in the background.)"
                            };
                            return Ok(ToolOutput::new(format!(
                                "{}{}",
                                summary, compatibility_note
                            )));
                        }
                        let reports = fetch_awaited_member_reports(&ctx, &members).await;
                        Ok(format_awaited_members_with_reports(
                            completed, &summary, &members, &reports,
                        ))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("Await completed."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to await members: {}", e)),
                }
            }

            _ => Err(anyhow::anyhow!(
                "Unknown action '{}'. Valid actions: share, share_append, read, message, broadcast, dm, channel, list, list_channels, channel_members, \
                 propose_plan, approve_plan, reject_plan, spawn, stop, assign_role, status, report, plan_status, summary, read_context, \
                 resync_plan, assign_task, assign_next, fill_slots, run_plan, cleanup, start, start_task, wake, resume, retry, reassign, replace, salvage, subscribe_channel, unsubscribe_channel, await_members. \
                 To read messages addressed to you, use action='read'.",
                params.action
            )),
        }
    }
}

#[cfg(test)]
#[path = "communicate_tests.rs"]
mod tests;
