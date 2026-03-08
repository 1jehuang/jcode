use super::{Tool, ToolContext, ToolOutput};
use crate::plan::PlanItem;
use crate::protocol::{
    AgentInfo, AwaitedMemberStatus, ContextEntry, HistoryMessage, Request, ServerEvent,
    ToolCallSummary,
};
use crate::transport::SyncStream;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

const REQUEST_ID: u64 = 1;

fn socket_path() -> std::path::PathBuf {
    crate::storage::runtime_dir().join("jcode.sock")
}

fn send_request(request: Request) -> Result<ServerEvent> {
    send_request_with_timeout(request, None)
}

fn send_request_with_timeout(
    request: Request,
    timeout: Option<std::time::Duration>,
) -> Result<ServerEvent> {
    let path = socket_path();
    let mut stream = SyncStream::connect(&path)?;

    let read_timeout = timeout.unwrap_or(std::time::Duration::from_secs(30));
    stream.set_read_timeout(Some(read_timeout))?;

    let request_id = request.id();

    let json = serde_json::to_string(&request)? + "\n";
    stream.write_all(json.as_bytes())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    // Read lines until we find the terminal response for our request ID.
    // Skip: ack events, notification events, swarm_status broadcasts, etc.
    // Terminal events: done, error, comm_spawn_response, comm_await_members_response,
    //                  and any other typed response with matching id.
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(anyhow::anyhow!(
                "Connection closed before receiving response"
            ));
        }

        let value: Value = serde_json::from_str(line.trim())?;

        let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let event_id = value.get("id").and_then(|v| v.as_u64());

        match event_type {
            // Skip ack — not a response
            "ack" => continue,
            // Skip broadcast/async events that are not tied to our request
            "swarm_status"
            | "swarm_plan"
            | "swarm_plan_proposal"
            | "swarm_event"
            | "notification"
            | "soft_interrupt_injected"
            | "session"
            | "session_id"
            | "history"
            | "mcp_status"
            | "memory_injected"
            | "compaction"
            | "connection_type"
            | "connection_phase"
            | "upstream_provider"
            | "reloading"
            | "reload_progress"
            | "interrupted" => continue,
            // Terminal responses: match on our request id if present
            "done" | "error" => {
                if event_id == Some(request_id) || event_id.is_none() {
                    return Ok(serde_json::from_value(value)?);
                }
                // Wrong id — skip
                continue;
            }
            // All other typed responses (comm_spawn_response, etc.) — return them
            _ => {
                if event_id == Some(request_id) || event_id.is_none() {
                    return Ok(serde_json::from_value(value)?);
                }
                continue;
            }
        }
    }
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

fn format_context_entries(entries: &[ContextEntry]) -> ToolOutput {
    if entries.is_empty() {
        ToolOutput::new("No shared context found.")
    } else {
        let mut output = String::from("Shared context from other agents:\n\n");
        for entry in entries {
            let from = entry.from_name.as_deref().unwrap_or(&entry.from_session);
            output.push_str(&format!(
                "  {} (from {}): {}\n",
                entry.key, from, entry.value
            ));
        }
        ToolOutput::new(output)
    }
}

fn format_members(ctx: &ToolContext, members: &[AgentInfo]) -> ToolOutput {
    if members.is_empty() {
        ToolOutput::new("No other agents in this codebase.")
    } else {
        let mut output = String::from("Agents in this codebase:\n\n");
        for member in members {
            let name = member.friendly_name.as_deref().unwrap_or("unknown");
            let session = &member.session_id;
            let role = member.role.as_deref().unwrap_or("agent");
            let files = member.files_touched.join(", ");
            let is_me = session == &ctx.session_id;
            let role_label = if role != "agent" {
                format!(" [{}]", role)
            } else {
                String::new()
            };
            output.push_str(&format!(
                "  {}{} ({}){}\n",
                name,
                role_label,
                if is_me { "you" } else { session },
                if files.is_empty() {
                    String::new()
                } else {
                    format!("\n    Files: {}", files)
                }
            ));
        }
        ToolOutput::new(output)
    }
}

fn format_tool_summary(target: &str, calls: &[ToolCallSummary]) -> ToolOutput {
    if calls.is_empty() {
        ToolOutput::new(format!("No tool calls found for {}", target))
    } else {
        let mut output = format!("Tool call summary for {}:\n\n", target);
        for call in calls {
            output.push_str(&format!("  {} — {}\n", call.tool_name, call.brief_output));
        }
        ToolOutput::new(output)
    }
}

fn format_context_history(target: &str, messages: &[HistoryMessage]) -> ToolOutput {
    if messages.is_empty() {
        ToolOutput::new(format!("No conversation history for {}", target))
    } else {
        let mut output = format!(
            "Conversation context for {} ({} messages):\n\n",
            target,
            messages.len()
        );
        for msg in messages {
            let truncated = if msg.content.len() > 500 {
                format!("{}...", &msg.content[..500])
            } else {
                msg.content.clone()
            };
            output.push_str(&format!("[{}] {}\n\n", msg.role, truncated));
        }
        ToolOutput::new(output)
    }
}

fn format_awaited_members(
    completed: bool,
    summary: &str,
    members: &[AwaitedMemberStatus],
) -> ToolOutput {
    let mut output = if completed {
        format!("All members done. {}\n", summary)
    } else {
        format!("Await incomplete. {}\n", summary)
    };

    if !members.is_empty() {
        output.push_str("\nMember statuses:\n");
        for member in members {
            let name = member
                .friendly_name
                .as_deref()
                .unwrap_or(&member.session_id);
            let icon = if member.done { "✓" } else { "✗" };
            output.push_str(&format!("  {} {} ({})\n", icon, name, member.status));
        }
    }

    ToolOutput::new(output)
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
    limit: Option<usize>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    plan_items: Option<Vec<PlanItem>>,
    #[serde(default)]
    target_status: Option<Vec<String>>,
    #[serde(default)]
    session_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_minutes: Option<u64>,
}

#[async_trait]
impl Tool for CommunicateTool {
    fn name(&self) -> &str {
        "communicate"
    }

    fn description(&self) -> &str {
        "Communicate with other agents working in the same codebase. Use this when you receive \
         a notification about another agent's activity, or to proactively coordinate with other agents.\n\n\
         Actions:\n\
         - \"share\": Share context (key/value) with other agents. They'll be notified.\n\
         - \"read\": Read shared context from other agents.\n\
         - \"broadcast\"/\"message\": Send a message to all other agents in the codebase.\n\
         - \"dm\": Send a direct message to a specific session.\n\
         - \"channel\": Send a message to a named channel in this swarm.\n\
         - \"list\": See who else is working in this codebase and what files they've touched.\n\
         - \"propose_plan\": Propose plan items to the swarm coordinator.\n\
         - \"approve_plan\": (Coordinator only) Approve a plan proposal from another agent.\n\
         - \"reject_plan\": (Coordinator only) Reject a plan proposal with an optional reason.\n\
         - \"spawn\": (Coordinator only) Spawn a new agent session.\n\
         - \"stop\": (Coordinator only) Stop/destroy an agent session.\n\
         - \"assign_role\": (Coordinator only) Assign a role to an agent.\n\
         - \"summary\": Get a summary of another agent's recent tool calls.\n\
         - \"read_context\": Read another agent's full conversation context.\n\
         - \"resync_plan\": Attach your session to the current swarm plan and re-sync.\n\
         - \"assign_task\": (Coordinator only) Assign a plan task to a specific agent.\n\
         - \"subscribe_channel\": Subscribe to a named channel.\n\
         - \"unsubscribe_channel\": Unsubscribe from a named channel.\n\
         - \"await_members\": Block until other agents reach a target status (e.g. completed/stopped). \
         Use this to wait for other agents to finish before proceeding with a task like cutting a release."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["share", "read", "message", "broadcast", "dm", "channel", "list",
                             "propose_plan", "approve_plan", "reject_plan", "spawn", "stop", "assign_role",
                             "summary", "read_context", "resync_plan", "assign_task",
                             "subscribe_channel", "unsubscribe_channel", "await_members"],
                    "description": "The communication action to perform"
                },
                "key": {
                    "type": "string",
                    "description": "For 'share': the context key. For 'read': optional specific key to read."
                },
                "value": {
                    "type": "string",
                    "description": "For 'share': the context value to share."
                },
                "message": {
                    "type": "string",
                    "description": "For 'message'/'broadcast'/'dm'/'channel': the message to send. For 'assign_task': optional additional message."
                },
                "to_session": {
                    "type": "string",
                    "description": "For 'dm': the target session ID."
                },
                "channel": {
                    "type": "string",
                    "description": "For 'channel'/'subscribe_channel'/'unsubscribe_channel': the channel name (without #)."
                },
                "proposer_session": {
                    "type": "string",
                    "description": "For 'approve_plan'/'reject_plan': the session ID of the agent who proposed the plan."
                },
                "reason": {
                    "type": "string",
                    "description": "For 'reject_plan': optional reason for rejection."
                },
                "target_session": {
                    "type": "string",
                    "description": "For 'stop'/'assign_role'/'summary'/'read_context'/'assign_task': the target session ID."
                },
                "role": {
                    "type": "string",
                    "enum": ["agent", "coordinator", "worktree_manager"],
                    "description": "For 'assign_role': the role to assign."
                },
                "working_dir": {
                    "type": "string",
                    "description": "For 'spawn': optional working directory for the new agent."
                },
                "initial_message": {
                    "type": "string",
                    "description": "For 'spawn': optional initial message to send to the new agent."
                },
                "limit": {
                    "type": "integer",
                    "description": "For 'summary': max number of tool calls to return (default 10)."
                },
                "task_id": {
                    "type": "string",
                    "description": "For 'assign_task': the ID of the task in the swarm plan to assign."
                },
                "target_status": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "For 'await_members': statuses that count as done (e.g. ['completed', 'stopped']). Defaults to ['completed', 'stopped', 'failed']."
                },
                "session_ids": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "For 'await_members': specific session IDs to watch. If omitted, watches all other members in the swarm."
                },
                "timeout_minutes": {
                    "type": "integer",
                    "description": "For 'await_members': max minutes to wait (default: 60)."
                },
                "plan_items": {
                    "type": "array",
                    "description": "For 'propose_plan': plan items to propose to the coordinator.",
                    "items": {
                        "type": "object",
                        "required": ["content", "status", "priority", "id"],
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "Brief description of the plan item"
                            },
                            "status": {
                                "type": "string",
                                "description": "queued, running, done, blocked, failed, etc."
                            },
                            "priority": {
                                "type": "string",
                                "description": "high, medium, low"
                            },
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for the plan item"
                            },
                            "blocked_by": {
                                "type": "array",
                                "items": {"type": "string"},
                                "description": "Optional item IDs this item depends on"
                            },
                            "assigned_to": {
                                "type": "string",
                                "description": "Optional session ID owner"
                            }
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: CommunicateInput = serde_json::from_value(input)?;

        match params.action.as_str() {
            "share" => {
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
                };

                match send_request(request) {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Shared with other agents: {} = {}",
                            key, value
                        )))
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

                match send_request(request) {
                    Ok(ServerEvent::CommContext { entries, .. }) => Ok(format_context_entries(&entries)),
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
                };

                match send_request(request) {
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
                };

                match send_request(request) {
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
                };

                match send_request(request) {
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

                match send_request(request) {
                    Ok(ServerEvent::CommMembers { members, .. }) => Ok(format_members(&ctx, &members)),
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No agents found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list agents: {}", e)),
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

                match send_request(request) {
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

                match send_request(request) {
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

                match send_request(request) {
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
                    initial_message: params.initial_message.clone(),
                };

                match send_request(request) {
                    Ok(ServerEvent::CommSpawnResponse { new_session_id, .. })
                        if !new_session_id.is_empty() =>
                    {
                        Ok(ToolOutput::new(format!("Spawned new agent: {}", new_session_id)))
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
                };

                match send_request(request) {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Stopped agent: {}", target)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to stop agent: {}", e)),
                }
            }

            "assign_role" => {
                let target_raw = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for assign_role action")
                })?;
                let role = params.role.ok_or_else(|| {
                    anyhow::anyhow!("'role' is required for assign_role action")
                })?;

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

                match send_request(request) {
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

                match send_request(request) {
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

                match send_request(request) {
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

                match send_request(request) {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("Swarm plan re-synced to your session."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to resync plan: {}", e)),
                }
            }

            "assign_task" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for assign_task action")
                })?;
                let task_id = params.task_id.ok_or_else(|| {
                    anyhow::anyhow!("'task_id' is required for assign_task action")
                })?;

                let request = Request::CommAssignTask {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                    task_id: task_id.clone(),
                    message: params.message.clone(),
                };

                match send_request(request) {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Task '{}' assigned to {}",
                            task_id, target
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to assign task: {}", e)),
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

                match send_request(request) {
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

                match send_request(request) {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Unsubscribed from #{}", channel)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to unsubscribe: {}", e)),
                }
            }

            "await_members" => {
                let target_status = params.target_status.unwrap_or_else(|| {
                    vec![
                        "completed".to_string(),
                        "stopped".to_string(),
                        "failed".to_string(),
                    ]
                });
                let session_ids = params.session_ids.unwrap_or_default();
                let timeout_minutes = params.timeout_minutes.unwrap_or(60);
                let timeout_secs = timeout_minutes * 60;

                let request = Request::CommAwaitMembers {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_status,
                    session_ids,
                    timeout_secs: Some(timeout_secs),
                };

                let socket_timeout = std::time::Duration::from_secs(timeout_secs + 30);

                match send_request_with_timeout(request, Some(socket_timeout)) {
                    Ok(ServerEvent::CommAwaitMembersResponse {
                        completed,
                        members,
                        summary,
                        ..
                    }) => Ok(format_awaited_members(completed, &summary, &members)),
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("Await completed."))
                    }
                    Err(e) => Err(anyhow::anyhow!(
                        "Failed to await members: {}",
                        e
                    )),
                }
            }

            _ => Err(anyhow::anyhow!(
                "Unknown action '{}'. Valid actions: share, read, message, broadcast, dm, channel, list, \
                 approve_plan, reject_plan, spawn, stop, assign_role, summary, read_context, \
                 resync_plan, assign_task, subscribe_channel, unsubscribe_channel, await_members",
                params.action
            )),
        }
    }
}
