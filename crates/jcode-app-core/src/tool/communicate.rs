#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::{Tool, ToolContext, ToolOutput};
use crate::plan::PlanItem;
use crate::protocol::{
    AgentInfo, AgentStatusSnapshot, AwaitedMemberStatus, CommDeliveryMode, ContextEntry,
    HistoryMessage, PlanGraphStatus, Request, ServerEvent, SwarmChannelInfo, ToolCallSummary,
    comm_cleanup_candidate_session_ids, default_comm_await_target_statuses,
    default_comm_cleanup_target_statuses, format_comm_awaited_members_with_reports,
    format_comm_channels, format_comm_context_entries, format_comm_context_history,
    format_comm_members, format_comm_plan_followup, format_comm_plan_status,
    format_comm_status_snapshot, format_comm_tool_summary, latest_assistant_comm_report,
    resolve_optional_comm_target_session,
};
use anyhow::Result;
use async_trait::async_trait;
use jcode_swarm_core::validate_swarm_tldr;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

const REQUEST_ID: u64 = 1;

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

/// Whether `member` is a worker the coordinator can rely on to autonomously execute an
/// assignment (and therefore one it is safe to await): a spawned headless worker,
/// or one owned by the coordinator that issued the run. Foreign client-attached
/// sessions are not drivable and must not count toward its in-flight workers.
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

async fn spawn_assignment_session(ctx: &ToolContext, params: &CommunicateInput) -> Result<String> {
    let spawn_request = Request::CommSpawn {
        id: REQUEST_ID,
        session_id: ctx.session_id.clone(),
        working_dir: params.working_dir.clone(),
        initial_message: None,
        request_nonce: Some(fresh_spawn_request_nonce(ctx)),
        spawn_mode: params.spawn_mode.clone(),
        model: params.model.clone(),
        // Worker effort is not agent-controlled: the server applies
        // `agents.swarm_effort` (default "low").
        effort: None,
        label: None,
        agent: None,
        lane_id: None,
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
    ToolOutput::new(format_comm_plan_status(summary))
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

fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn cross_swarm_confirmation(swarm: &str, to_session: Option<&str>, message: &str) -> String {
    match to_session {
        Some(target) => format!("Cross-swarm DM sent to {target} in swarm '{swarm}': {message}"),
        None => format!("Cross-swarm DM sent to the coordinator of swarm '{swarm}': {message}"),
    }
}

/// Render the `list_swarms` directory: one line per live swarm with label,
/// id, coordinator, and member count. The caller's own swarm is marked.
pub(super) fn format_swarm_directory(swarms: &[crate::protocol::SwarmInfo]) -> String {
    if swarms.is_empty() {
        return "No live swarms.".to_string();
    }
    let mut out = String::from("Live swarms:\n\n");
    for swarm in swarms {
        let name = swarm.label.as_deref().unwrap_or("(unlabeled)");
        let own = if swarm.is_own { " [yours]" } else { "" };
        let coordinator = match (&swarm.coordinator_name, &swarm.coordinator_session_id) {
            (Some(name), Some(id)) => format!("{name} [{id}]"),
            (None, Some(id)) => id.clone(),
            _ => "none".to_string(),
        };
        out.push_str(&format!(
            "- {name}{own}\n  id: {}\n  coordinator: {coordinator}\n  members: {}\n",
            swarm.swarm_id, swarm.member_count
        ));
    }
    out.push_str(
        "\nCross-swarm DM: action=dm with to_swarm=<label or id> and optional to_session (defaults to that swarm's coordinator).",
    );
    out
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
        "Current coordinator model: {}\n",
        current_model.unwrap_or("unknown")
    ));
    match configured_swarm_model {
        Some(pin) if !pin.trim().is_empty() => {
            out.push_str(&format!("Configured agents.swarm_model default: {pin}\n"));
        }
        _ => out.push_str(
            "No agents.swarm_model default configured (workers inherit the coordinator's model).\n",
        ),
    }
    if model_routes.is_empty() {
        out.push_str("\nNo model routes reported. Omit model to use the configured default.");
        return out;
    }
    out.push_str("\nModel routes (informational only, overrides require agents.swarm_model_selection and an exact agents.swarm_model_allowlist entry):\n");
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
        Self::new_for_working_dir(None)
    }

    fn new_for_working_dir(working_dir: Option<&std::path::Path>) -> Self {
        const BASE_DESCRIPTION: &str =
            "Coordinate agents: spawn todo-lane workers, message them, and manage plans.";
        let swarm_prompt = crate::prompt::load_swarm_prompt(working_dir);
        let description = if swarm_prompt.is_empty() {
            BASE_DESCRIPTION.to_string()
        } else {
            format!("{BASE_DESCRIPTION}\n\nSwarm prompt:\n{swarm_prompt}")
        };
        Self { description }
    }
}

/// Swarm actions advertised to models. Real usage is dominated by messaging,
/// listing and spawning; the plan/task-graph actions (propose_plan,
/// assign_task, assign_next, resync_plan, ...) are subsumed by worker-owned
/// todo lanes, which the server dispatches itself, and lane-owning `spawn`. Unlisted actions still
/// execute for old transcripts and scripts, they are just not offered.
const ADVERTISED_SWARM_ACTIONS: &[&str] = &[
    "spawn",
    "message",
    "status",
    "plan_status",
    "list",
    "summary",
    "await_members",
    "stop",
    "resume",
    "retry",
    "list_models",
    "list_swarms",
];

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
    /// Cross-swarm DM target: swarm label or id (see list_swarms).
    #[serde(default)]
    to_swarm: Option<String>,
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
    force: Option<bool>,
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
    /// Optional model override, authorized by the user's opt-in and exact allowlist.
    #[serde(default)]
    model: Option<String>,
    /// Short human-readable label for a spawned agent shown in swarm UI.
    /// Required and nonblank for the explicit `spawn` action.
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    /// Phase (todo `stage`) a spawned worker's lane joins. Defaults to the
    /// active phase.
    #[serde(default, alias = "stage")]
    phase: Option<u32>,
}

impl CommunicateInput {
    fn spawn_initial_message(&self) -> Option<String> {
        self.initial_message
            .as_ref()
            .filter(|message| !message.trim().is_empty())
            .cloned()
            .or_else(|| {
                self.prompt
                    .as_ref()
                    .filter(|prompt| !prompt.trim().is_empty())
                    .cloned()
            })
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
        // `dm` was its own action; `message` with `to_session` is identical,
        // so route it there (one way to talk to an agent).
        "dm" | "dm_session" | "direct_message" | "whisper" => "message",
        "broadcast_all" | "announce" => "broadcast",
        "agents" | "members" | "list_agents" | "list_members" | "roster" => "list",
        "swarms" | "list_swarm" | "swarm_list" => "list_swarms",
        "label_swarm" | "name_swarm" | "rename_swarm" | "set_label" => "set_swarm_label",
        "models" | "model_list" | "list_model" | "list_providers" | "list_routes" => "list_models",
        "plan" | "status_plan" => "plan_status",
        "assign" => "assign_task",
        "kill" | "terminate" => "stop",
        _ => action,
    }
}

#[path = "swarm_model_schema.rs"]
mod swarm_model_schema;
use swarm_model_schema::{apply_agent_definitions, apply_swarm_model_policy};
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
                    "enum": ADVERTISED_SWARM_ACTIONS
                },
                "message": {
                    "type": "string",
                    "description": "Routes to to_session, then channel, else broadcasts (coordinator only)."
                },
                "tldr": {
                    "type": "string",
                    "description": "Summary under ~120 chars; auto-derived if omitted."
                },
                "to_session": {
                    "type": "string",
                    "description": "Agent ID/unique name; alias of target_session."
                },
                "to_swarm": {
                    "type": "string",
                    "description": "Swarm ID/label; targets to_session or its coordinator."
                },
                "channel": {
                    "type": "string",
                    "description": "Prefer DMs."
                },
                "target_session": {
                    "type": "string",
                    "description": "Alias of to_session."
                },
                "label": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Required for spawn: todo lane (source of truth)."
                },
                "phase": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Spawn lane phase; defaults to active phase."
                },
                "working_dir": {
                    "type": "string"
                },
                "prompt": {
                    "type": "string",
                    "description": "Extra spawn context, not lane tasks."
                },
                "initial_message": {
                    "type": "string",
                    "description": "Overrides prompt for spawn."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Max summary items."
                },
                "task_id": {
                    "type": "string",
                    "description": "Plan task; auto-selected if omitted."
                },
                "spawn_mode": {
                    "type": "string",
                    "enum": ["visible", "headless", "inline", "auto"],
                    "description": "Omit to use the user's configured mode. Set only when the user asks for one."
                },
                "session_ids": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "mode": {
                    "type": "string",
                    "enum": ["all", "any"],
                    "description": "await_members."
                },
                "target_status": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "await_members; defaults to ready/completed/stopped/failed."
                },
                "timeout_minutes": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "await_members; returned ID can be used in todo.waiting_on."
                },
                "notify": {
                    "type": "boolean",
                    "description": "await_members: default true."
                },
                "force": {
                    "type": "boolean",
                    "description": "stop: allow stopping non-owned/user-created sessions; default false."
                },
                "wake": {
                    "type": "boolean",
                    "description": "Wake on message/await completion; default true."
                },
                "delivery": {
                    "type": "string",
                    "enum": ["notify", "interrupt", "wake"]
                },
            }
        });

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
                    "action": { "type": "string", "enum": ["spawn"] },
                    // Gemini validates that every `required` name is defined in
                    // the same object's `properties` and rejects the whole
                    // request otherwise (issue #655), so declare `label` here
                    // instead of relying on the parent schema's declaration.
                    "label": { "type": "string", "minLength": 1 }
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

        apply_swarm_model_policy(&mut schema, &crate::config::config().agents);
        apply_agent_definitions(&mut schema);
        schema
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let mut params: CommunicateInput = serde_json::from_value(input)?;
        crate::config::config()
            .agents
            .validate_swarm_model_request(params.model.as_deref())
            .map_err(anyhow::Error::msg)?;

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
                let to_swarm = non_blank(params.to_swarm.clone());

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: to_session.clone(),
                    channel: channel.clone(),
                    wake: params.wake,
                    delivery: params.delivery,
                    tldr,
                    to_swarm: to_swarm.clone(),
                    // A worker that forgets `to_session` gets an error rather
                    // than broadcasting; the coordinator may still message all.
                    require_target: true,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        if let Some(swarm) = to_swarm {
                            return Ok(ToolOutput::new(cross_swarm_confirmation(
                                &swarm,
                                to_session.as_deref(),
                                &message,
                            )));
                        }
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
                // Prefer DMs; group sends are for rare
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
                    to_swarm: None,
                    require_target: false,
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
                let to_swarm = non_blank(params.to_swarm.clone());
                if to_swarm.is_none() && params.to_session.is_none() {
                    return Err(anyhow::anyhow!(
                        "'to_session' (or 'target_session') is required for dm action, or pass 'to_swarm' to DM another swarm's coordinator"
                    ));
                }
                let to_session = params.to_session.clone();

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: to_session.clone(),
                    channel: None,
                    delivery: params.delivery,
                    wake: params.wake,
                    tldr,
                    to_swarm: to_swarm.clone(),
                    require_target: false,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        if let Some(swarm) = to_swarm {
                            return Ok(ToolOutput::new(cross_swarm_confirmation(
                                &swarm,
                                to_session.as_deref(),
                                &message,
                            )));
                        }
                        let to_session = to_session.unwrap_or_default();
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
                    to_swarm: None,
                    require_target: false,
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

            "list_swarms" => {
                let request = Request::CommListSwarms {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                };
                match send_request(request).await {
                    Ok(ServerEvent::CommSwarms { swarms, .. }) => {
                        Ok(ToolOutput::new(format_swarm_directory(&swarms)))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No swarms found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list swarms: {}", e)),
                }
            }

            "set_swarm_label" => {
                let label = params
                    .label
                    .clone()
                    .or_else(|| params.value.clone())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "'label' is required for set_swarm_label (empty string clears it)"
                        )
                    })?;
                let request = Request::CommSetSwarmLabel {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    label: label.clone(),
                };
                match send_request(request).await {
                    Ok(ServerEvent::CommSwarms { swarms, .. }) => {
                        let applied = swarms
                            .iter()
                            .find(|swarm| swarm.is_own)
                            .and_then(|swarm| swarm.label.clone());
                        let head = match applied {
                            Some(label) => format!("Your swarm is now labeled '{label}'.\n\n"),
                            None => "Your swarm label was cleared.\n\n".to_string(),
                        };
                        Ok(ToolOutput::new(format!(
                            "{head}{}",
                            format_swarm_directory(&swarms)
                        )))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Swarm label set to '{label}'.")))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to set swarm label: {}", e)),
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

            "spawn" => {
                let label = params.required_spawn_label()?;
                // Every spawned worker owns one lane in the coordinator's todo
                // list: the lane named `label` in `phase` (default: the active
                // phase), created from the prompt when it does not exist. The
                // worker's own todo list is that lane and syncs only it.
                let prompt = params.spawn_initial_message();
                let (lane_id, seeded_count) = {
                    let todos = crate::todo::load_todos(&ctx.session_id).unwrap_or_default();
                    let title = prompt
                        .as_deref()
                        .and_then(|prompt| prompt.lines().map(str::trim).find(|l| !l.is_empty()))
                        .map(|line| line.chars().take(60).collect::<String>())
                        .unwrap_or_else(|| label.clone());
                    let (updated, lane_id) = crate::todo::stages::ensure_spawn_lane(
                        &todos,
                        &label,
                        params.phase,
                        &title,
                        prompt.as_deref(),
                        &{
                            let mut n = todos.len() + 1;
                            while todos.iter().any(|t| t.id == format!("lane-{n}")) {
                                n += 1;
                            }
                            format!("lane-{n}")
                        },
                    );
                    if updated != todos {
                        crate::todo::save_todos(&ctx.session_id, &updated)?;
                    }
                    let seeded_count =
                        crate::todo::stages::lane_todos_for_worker(&updated, &lane_id).len();
                    (lane_id, seeded_count)
                };
                let request = Request::CommSpawn {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    working_dir: params.working_dir.clone(),
                    initial_message: params.spawn_initial_message(),
                    request_nonce: None,
                    spawn_mode: params.spawn_mode.clone(),
                    model: params.model.clone(),
                    // Worker effort is not agent-controlled: the server applies
                    // `agents.swarm_effort` (default "low").
                    effort: None,
                    label: Some(label),
                    agent: params.agent.clone(),
                    lane_id: Some(lane_id.clone()),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommSpawnResponse { new_session_id, .. })
                        if !new_session_id.is_empty() =>
                    {
                        Ok(ToolOutput::new(format!(
                            "Spawned new agent: {new_session_id}\nLane {lane_id}: {seeded_count} todo item(s) prepopulated before startup and synced to your lane. The worker received its lane tasks automatically."
                        ))
                        .with_metadata(
                            serde_json::json!({"session_id": new_session_id, "lane": lane_id, "seeded_todo_count": seeded_count, "lane_synced": true}),
                        ))
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

            // Compatibility for old transcripts and scripts only. Worker output is
            // collected automatically, so this action is not advertised to models.
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
                    // Worker effort is not agent-controlled: the server applies
                    // `agents.swarm_effort` (default "low").
                    effort: None,
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

            "launch" | "run_plan" | "fill_slots" => Err(anyhow::anyhow!(
                "'{}' was removed. Mark a todo lane `owner: \"worker\"` (todo phases[].lanes[].owner, \
                 or update lane_owner) and the server spawns a worker for it as soon as its \
                 dependencies are met. Use `plan_status` to watch lanes.",
                params.action
            )),

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
                // CommAwaitMembers protocol remains available internally, but agents must not park an entire
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
                                "{}{}\n\n{}",
                                summary,
                                compatibility_note,
                                super::BACKGROUND_WAIT_GUIDANCE
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
                 propose_plan, approve_plan, reject_plan, spawn, stop, assign_role, status, plan_status, summary, read_context, \
                 resync_plan, assign_task, assign_next, cleanup, start, start_task, wake, resume, retry, reassign, replace, salvage, subscribe_channel, unsubscribe_channel, await_members. \
                 To read messages addressed to you, use action='read'.",
                params.action
            )),
        }
    }
}

#[cfg(test)]
#[path = "communicate_tests.rs"]
mod tests;
