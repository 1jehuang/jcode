#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::{Tool, ToolContext, ToolOutput};
use crate::plan::PlanItem;
use crate::protocol::{
    AgentInfo, AgentStatusSnapshot, AwaitedMemberStatus, CommDeliveryMode, ContextEntry,
    HistoryMessage, PlanGraphStatus, Request, ServerEvent, SwarmChannelInfo, ToolCallSummary,
    comm_cleanup_candidate_session_ids, default_comm_await_target_statuses,
    default_comm_cleanup_target_statuses, default_comm_run_await_statuses,
    format_comm_awaited_members_with_reports, format_comm_channels, format_comm_context_entries,
    format_comm_context_history, format_comm_members, format_comm_plan_followup,
    format_comm_plan_status, format_comm_status_snapshot, format_comm_tool_summary,
    latest_assistant_comm_report, resolve_optional_comm_target_session,
};
use anyhow::Result;
use async_trait::async_trait;
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

    let mut output = String::new();
    if stopped.is_empty() {
        output.push_str("Stopped no swarm workers.");
    } else {
        output.push_str(&format!(
            "Stopped {} swarm worker(s): {}",
            stopped.len(),
            stopped.join(", ")
        ));
    }
    if !failed.is_empty() {
        output.push_str(&format!(
            "\nFailed to stop {} worker(s): {}",
            failed.len(),
            failed.join(", ")
        ));
    }
    Ok(output)
}

async fn await_swarm_progress(
    ctx: &ToolContext,
    session_ids: Vec<String>,
    timeout_minutes: u64,
) -> Result<()> {
    let request = Request::CommAwaitMembers {
        id: REQUEST_ID,
        session_id: ctx.session_id.clone(),
        target_status: default_run_await_statuses(),
        session_ids,
        mode: Some("any".to_string()),
        timeout_secs: Some(timeout_minutes.max(1) * 60),
    };
    let socket_timeout = std::time::Duration::from_secs(timeout_minutes.max(1) * 60 + 30);
    match send_request_with_timeout(request, Some(socket_timeout)).await {
        Ok(response) => ensure_success(&response),
        Err(e) => Err(anyhow::anyhow!(
            "Failed while awaiting swarm progress: {}",
            e
        )),
    }
}

async fn run_swarm_plan_to_terminal(
    ctx: &ToolContext,
    params: &CommunicateInput,
) -> Result<ToolOutput> {
    let concurrency_limit = params.concurrency_limit.unwrap_or(3).max(1);
    let timeout_minutes = params.timeout_minutes.unwrap_or(60).max(1);
    let retain_agents = params.retain_agents.unwrap_or(false);
    let spawn_if_needed = params.spawn_if_needed.or(Some(true));
    let mut assignment_count = 0usize;
    let mut loop_count = 0usize;
    let max_loops = 200usize;

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

        let terminal_count =
            summary.completed_ids.len() + summary.blocked_ids.len() + summary.cycle_ids.len();
        let no_more_runnable = summary.active_ids.is_empty() && summary.next_ready_ids.is_empty();
        if no_more_runnable || terminal_count >= summary.item_count {
            let mut output = format!(
                "Swarm plan reached terminal/blocked state after {} loop(s). completed={} blocked={} cycles={} active={} assignments={}",
                loop_count,
                summary.completed_ids.len(),
                summary.blocked_ids.len(),
                summary.cycle_ids.len(),
                summary.active_ids.len(),
                assignment_count
            );
            if retain_agents {
                output.push_str("\nRetained spawned workers because retain_agents=true.");
            } else {
                let cleanup = cleanup_swarm_workers(ctx, params).await?;
                output.push_str(&format!("\n{}", cleanup));
            }
            return Ok(ToolOutput::new(output));
        }

        let active_count = summary.active_ids.len();
        let available_slots = concurrency_limit.saturating_sub(active_count);
        let mut assigned_sessions = Vec::new();
        for _ in 0..available_slots {
            let request = Request::CommAssignNext {
                id: REQUEST_ID,
                session_id: ctx.session_id.clone(),
                target_session: params.target_session.clone(),
                working_dir: params.working_dir.clone(),
                prefer_spawn: params.prefer_spawn,
                spawn_if_needed,
                message: params.message.clone(),
            };
            match send_request(request).await {
                Ok(ServerEvent::CommAssignTaskResponse { target_session, .. }) => {
                    assignment_count += 1;
                    assigned_sessions.push(target_session);
                }
                Ok(ServerEvent::Error { message, .. })
                    if message.contains("No runnable unassigned tasks")
                        || message.contains("No ready or completed swarm agents") =>
                {
                    break;
                }
                Ok(response) => ensure_success(&response)?,
                Err(e) => return Err(anyhow::anyhow!("Failed to assign next swarm task: {}", e)),
            }
        }

        let await_sessions = if assigned_sessions.is_empty() {
            let members = fetch_swarm_members(&ctx.session_id).await?;
            members
                .into_iter()
                .filter(|member| member.session_id != ctx.session_id)
                .filter(|member| member.status.as_deref() == Some("running"))
                .map(|member| member.session_id)
                .collect::<Vec<_>>()
        } else {
            assigned_sessions
        };

        if await_sessions.is_empty() {
            if active_count > 0 {
                return Err(anyhow::anyhow!(
                    "run_plan found {} active task(s) but no running swarm members to await; inspect plan_status and member list before retrying",
                    active_count
                ));
            }
            continue;
        }
        await_swarm_progress(ctx, await_sessions, timeout_minutes).await?;
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

fn format_channels(channels: &[SwarmChannelInfo]) -> ToolOutput {
    ToolOutput::new(format_comm_channels(channels))
}

pub struct CommunicateTool;

impl CommunicateTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
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
}

impl CommunicateInput {
    fn spawn_initial_message(&self) -> Option<String> {
        self.initial_message.clone().or_else(|| self.prompt.clone())
    }
}

#[async_trait]
impl Tool for CommunicateTool {
    fn name(&self) -> &str {
        "swarm"
    }

    fn description(&self) -> &str {
        "Coordinate agents. For spawn, prefer providing a prompt so the new agent starts with a concrete task instead of idling. Spawned/assigned agents automatically report their final response back to the owning coordinator."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["share", "share_append", "read", "message", "broadcast", "dm", "channel", "list", "list_channels", "channel_members",
                             "propose_plan", "approve_plan", "reject_plan", "spawn", "stop", "assign_role",
                             "status", "report", "plan_status", "summary", "read_context", "resync_plan", "assign_task", "assign_next", "fill_slots", "run_plan", "cleanup",
                             "start", "start_task", "wake", "resume", "retry", "reassign", "replace", "salvage",
                             "subscribe_channel", "unsubscribe_channel", "await_members"],
                    "description": "Action. For spawn, prefer including prompt with the initial task so the new agent starts useful work immediately."
                },
                "key": {
                    "type": "string"
                },
                "value": {
                    "type": "string"
                },
                "message": {
                    "type": "string",
                    "description": "Message body. For action=report, this is the completion report body."
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
                    "description": "DM target. Accepts an exact session ID or a unique friendly name within the swarm. If a friendly name is ambiguous, run swarm list and use the exact session ID."
                },
                "channel": { "type": "string" },
                "proposer_session": { "type": "string" },
                "reason": { "type": "string" },
                "target_session": { "type": "string" },
                "role": {
                    "type": "string",
                    "enum": ["agent", "coordinator", "worktree_manager"]
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
                    "enum": ["visible", "headless", "auto"],
                    "description": "Per-call spawn mode for swarm-created agents. Overrides agents.swarm_spawn_mode config when set. Defaults to visible/headed behavior."
                },
                "session_ids": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "mode": {
                    "type": "string",
                    "enum": ["all", "any"],
                    "description": "For await_members: wait for all targeted members or wake when any targeted member matches."
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
                "concurrency_limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "For fill_slots: desired maximum number of active swarm tasks."
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
                    "description": "Optional wake hint for messages."
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
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: CommunicateInput = serde_json::from_value(input)?;

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

            "message" | "broadcast" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for message action"))?;

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: None,
                    channel: None,
                    wake: params.wake,
                    delivery: None,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Message sent to other agents: {}",
                            message
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to send message: {}", e)),
                }
            }

            "dm" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for dm action"))?;
                let to_session = params
                    .to_session
                    .ok_or_else(|| anyhow::anyhow!("'to_session' is required for dm action"))?;

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: Some(to_session.clone()),
                    channel: None,
                    delivery: params.delivery,
                    wake: params.wake,
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

            "spawn" => {
                let request = Request::CommSpawn {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    working_dir: params.working_dir.clone(),
                    initial_message: params.spawn_initial_message(),
                    request_nonce: None,
                    spawn_mode: params.spawn_mode.clone(),
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
                let request = Request::CommReport {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    status: params.status,
                    message,
                    validation: params.validation,
                    follow_up: params.follow_up,
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

                let active_count = summary.active_ids.len();
                if active_count >= concurrency_limit {
                    return Ok(ToolOutput::new(format!(
                        "Window already full: {} active task(s) >= limit {}",
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

            "run_plan" => run_swarm_plan_to_terminal(&ctx, &params).await,

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

                let request = Request::CommAwaitMembers {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_status,
                    session_ids,
                    mode: params.mode.clone(),
                    timeout_secs: Some(timeout_secs),
                };

                let socket_timeout = std::time::Duration::from_secs(timeout_secs + 30);

                match send_request_with_timeout(request, Some(socket_timeout)).await {
                    Ok(ServerEvent::CommAwaitMembersResponse {
                        completed,
                        members,
                        summary,
                        ..
                    }) => {
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
                 resync_plan, assign_task, assign_next, fill_slots, run_plan, cleanup, start, start_task, wake, resume, retry, reassign, replace, salvage, subscribe_channel, unsubscribe_channel, await_members",
                params.action
            )),
        }
    }
}

#[cfg(test)]
#[path = "communicate_tests.rs"]
mod tests;
