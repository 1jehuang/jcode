#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::{Tool, ToolContext, ToolOutput};
use crate::plan::PlanItem;
use crate::protocol::{
    AgentInfo, AwaitedMemberStatus, CommDeliveryMode, ContextEntry, HistoryMessage, Request,
    ServerEvent, SwarmChannelInfo, ToolCallSummary,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const REQUEST_ID: u64 = 1;

async fn send_request(request: Request) -> Result<ServerEvent> {
    send_request_with_timeout(request, None).await
}

async fn send_request_with_timeout(
    request: Request,
    timeout: Option<std::time::Duration>,
) -> Result<ServerEvent> {
    let path = crate::server::socket_path();
    let stream = crate::server::connect_socket(&path).await?;
    let (reader, mut writer) = stream.into_split();

    let request_id = request.id();
    let deadline = tokio::time::Instant::now()
        + timeout.unwrap_or(std::time::Duration::from_secs(30));

    let json = serde_json::to_string(&request)? + "\n";
    writer.write_all(json.as_bytes()).await?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    // Read lines until we find the terminal response for our request ID.
    // Skip: ack events, notification events, swarm_status broadcasts, etc.
    // Terminal events: done, error, comm_spawn_response, comm_await_members_response,
    //                  and any other typed response with matching id.
    loop {
        line.clear();
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("Timed out waiting for response")
        }
        let n = tokio::time::timeout(remaining, reader.read_line(&mut line)).await??;
        if n == 0 {
            return Err(anyhow::anyhow!(
                "Connection closed before receiving response"
            ));
        }

        let value: Value = serde_json::from_str(line.trim())?;

        let event_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let event_id = value.get("id").and_then(|v| v.as_u64());

        if event_type != "ack" && event_id != Some(request_id) {
            continue;
        }

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
            | "status_detail"
            | "upstream_provider"
            | "reloading"
            | "reload_progress"
            | "available_models_updated"
            | "side_panel_state"
            | "transcript"
            | "interrupted" => continue,
            // Terminal responses and typed request responses with matching ids.
            _ => return Ok(serde_json::from_value(value)?),
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
            let status = member.status.as_deref().unwrap_or("unknown");
            let is_me = session == &ctx.session_id;
            let role_label = if role != "agent" {
                format!(" [{}]", role)
            } else {
                String::new()
            };
            output.push_str(&format!(
                "  {}{} ({})\n    Status: {}{}{}\n",
                name,
                role_label,
                if is_me { "you" } else { session },
                status,
                member
                    .detail
                    .as_deref()
                    .map(|detail| format!(" — {}", detail))
                    .unwrap_or_default(),
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

fn default_await_target_statuses() -> Vec<String> {
    vec![
        "ready".to_string(),
        "completed".to_string(),
        "stopped".to_string(),
        "failed".to_string(),
    ]
}

fn format_channels(channels: &[SwarmChannelInfo]) -> ToolOutput {
    if channels.is_empty() {
        ToolOutput::new("No swarm channels found.")
    } else {
        let mut output = String::from("Swarm channels:\n\n");
        for channel in channels {
            output.push_str(&format!(
                "  #{} — {} subscriber{}\n",
                channel.channel,
                channel.member_count,
                if channel.member_count == 1 { "" } else { "s" }
            ));
        }
        ToolOutput::new(output)
    }
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
    plan_items: Option<Vec<PlanItem>>,
    #[serde(default)]
    target_status: Option<Vec<String>>,
    #[serde(default)]
    session_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_minutes: Option<u64>,
    #[serde(default)]
    wake: Option<bool>,
    #[serde(default)]
    delivery: Option<CommDeliveryMode>,
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
        "Coordinate agents. For spawn, prefer providing a prompt so the new agent starts with a concrete task instead of idling."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["share", "share_append", "read", "message", "broadcast", "dm", "channel", "list", "list_channels", "channel_members",
                             "propose_plan", "approve_plan", "reject_plan", "spawn", "stop", "assign_role",
                             "summary", "read_context", "resync_plan", "assign_task",
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
                    "type": "string"
                },
                "to_session": { "type": "string" },
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
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional max items for summary-style reads."
                },
                "task_id": { "type": "string" },
                "session_ids": {
                    "type": "array",
                    "items": {"type": "string"}
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
                };

                match send_request(request).await {
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

                match send_request(request).await {
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

                match send_request_with_timeout(request, Some(socket_timeout)).await {
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
                    Err(e) => Err(anyhow::anyhow!("Failed to await members: {}", e)),
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

#[cfg(test)]
mod tests {
    use super::{CommunicateInput, default_await_target_statuses, format_members};
    use crate::message::{Message, StreamEvent, ToolDefinition};
    use crate::protocol::{AgentInfo, Request, ServerEvent};
    use crate::provider::{EventStream, Provider};
    use crate::server::Server;
    use crate::tool::{Tool, ToolContext, ToolExecutionMode};
    use crate::transport::{ReadHalf, Stream, WriteHalf};
    use anyhow::Result;
    use async_trait::async_trait;
    use futures::StreamExt;
    use serde_json::json;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::CommunicateTool;

    #[test]
    fn tool_is_named_swarm() {
        assert_eq!(CommunicateTool::new().name(), "swarm");
    }

    #[test]
    fn schema_still_requires_action() {
        let schema = CommunicateTool::new().parameters_schema();
        assert_eq!(schema["required"], json!(["action"]));
    }

    #[test]
    fn schema_advertises_supported_swarm_fields() {
        let schema = CommunicateTool::new().parameters_schema();
        let props = schema["properties"]
            .as_object()
            .expect("swarm schema should have properties");

        assert!(props.contains_key("action"));
        assert!(props.contains_key("key"));
        assert!(props.contains_key("value"));
        assert!(props.contains_key("message"));
        assert!(props.contains_key("to_session"));
        assert!(props.contains_key("channel"));
        assert!(props.contains_key("proposer_session"));
        assert!(props.contains_key("reason"));
        assert!(props.contains_key("target_session"));
        assert!(props.contains_key("role"));
        assert!(props.contains_key("prompt"));
        assert!(props.contains_key("working_dir"));
        assert!(props.contains_key("limit"));
        assert!(props.contains_key("task_id"));
        assert!(props.contains_key("session_ids"));
        assert!(props.contains_key("target_status"));
        assert!(props.contains_key("timeout_minutes"));
        assert!(props.contains_key("wake"));
        assert!(props.contains_key("delivery"));
        assert!(props.contains_key("plan_items"));
        assert!(!props.contains_key("initial_message"));
        assert_eq!(props["delivery"]["enum"], json!(["notify", "interrupt", "wake"]));
        assert_eq!(
            props["plan_items"]["items"]["additionalProperties"],
            json!(true)
        );
    }

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let original = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.original.take() {
                crate::env::set_var(self.key, value);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    struct DelayedTestProvider {
        delay: Duration,
    }

    #[async_trait]
    impl Provider for DelayedTestProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            let delay = self.delay;
            let stream = futures::stream::once(async move {
                tokio::time::sleep(delay).await;
                Ok(StreamEvent::TextDelta("ok".to_string()))
            })
            .chain(futures::stream::once(async {
                Ok(StreamEvent::MessageEnd { stop_reason: None })
            }));
            Ok(Box::pin(stream))
        }

        fn name(&self) -> &str {
            "test"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self { delay: self.delay })
        }
    }

    struct RawClient {
        reader: BufReader<ReadHalf>,
        writer: WriteHalf,
        next_id: u64,
    }

    impl RawClient {
        async fn connect(path: &Path) -> Result<Self> {
            let stream = Stream::connect(path).await?;
            let (reader, writer) = stream.into_split();
            Ok(Self {
                reader: BufReader::new(reader),
                writer,
                next_id: 1,
            })
        }

        async fn send_request(&mut self, request: Request) -> Result<u64> {
            let id = request.id();
            let json = serde_json::to_string(&request)? + "\n";
            self.writer.write_all(json.as_bytes()).await?;
            Ok(id)
        }

        async fn read_event(&mut self) -> Result<ServerEvent> {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("server disconnected")
            }
            Ok(serde_json::from_str(&line)?)
        }

        async fn read_until<F>(
            &mut self,
            timeout: Duration,
            mut predicate: F,
        ) -> Result<ServerEvent>
        where
            F: FnMut(&ServerEvent) -> bool,
        {
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let event = tokio::time::timeout(remaining, self.read_event()).await??;
                if predicate(&event) {
                    return Ok(event);
                }
            }
        }

        async fn subscribe(&mut self, working_dir: &Path) -> Result<()> {
            let id = self.next_id;
            self.next_id += 1;
            self.send_request(Request::Subscribe {
                id,
                working_dir: Some(working_dir.display().to_string()),
                selfdev: None,
                target_session_id: None,
                client_instance_id: None,
                client_has_local_history: false,
                allow_session_takeover: false,
            })
            .await?;
            self.read_until(
                Duration::from_secs(5),
                |event| matches!(event, ServerEvent::Done { id: done_id } if *done_id == id),
            )
            .await?;
            Ok(())
        }

        async fn session_id(&mut self) -> Result<String> {
            let id = self.next_id;
            self.next_id += 1;
            self.send_request(Request::GetState { id }).await?;
            match self
                .read_until(Duration::from_secs(5), |event| {
                    matches!(event, ServerEvent::State { id: event_id, .. } if *event_id == id)
                })
                .await?
            {
                ServerEvent::State { session_id, .. } => Ok(session_id),
                other => anyhow::bail!("unexpected state response: {other:?}"),
            }
        }

        async fn send_message(&mut self, content: &str) -> Result<u64> {
            let id = self.next_id;
            self.next_id += 1;
            self.send_request(Request::Message {
                id,
                content: content.to_string(),
                images: vec![],
                system_reminder: None,
            })
            .await
        }

        async fn wait_for_done(&mut self, request_id: u64) -> Result<()> {
            self.read_until(
                Duration::from_secs(10),
                |event| matches!(event, ServerEvent::Done { id } if *id == request_id),
            )
            .await?;
            Ok(())
        }

        async fn comm_list(&mut self, session_id: &str) -> Result<Vec<AgentInfo>> {
            let id = self.next_id;
            self.next_id += 1;
            self.send_request(Request::CommList {
                id,
                session_id: session_id.to_string(),
            })
            .await?;
            match self
                .read_until(Duration::from_secs(5), |event| {
                    matches!(event, ServerEvent::CommMembers { id: event_id, .. } if *event_id == id)
                })
                .await?
            {
                ServerEvent::CommMembers { members, .. } => Ok(members),
                other => anyhow::bail!("unexpected comm_list response: {other:?}"),
            }
        }
    }

    async fn wait_for_server_socket(
        path: &Path,
        server_task: &mut tokio::task::JoinHandle<Result<()>>,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if server_task.is_finished() {
                let result = server_task.await?;
                return Err(anyhow::anyhow!(
                    "server exited before socket became ready: {:?}",
                    result
                ));
            }
            match Stream::connect(path).await {
                Ok(stream) => {
                    drop(stream);
                    return Ok(());
                }
                Err(err) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(err.into());
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
    }

    fn test_ctx(session_id: &str, working_dir: &Path) -> ToolContext {
        ToolContext {
            session_id: session_id.to_string(),
            message_id: "msg-1".to_string(),
            tool_call_id: "call-1".to_string(),
            working_dir: Some(working_dir.to_path_buf()),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        }
    }

    async fn wait_for_member_status(
        client: &mut RawClient,
        requester_session: &str,
        target_session: &str,
        expected_status: &str,
    ) -> Result<Vec<AgentInfo>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let members = client.comm_list(requester_session).await?;
            if members
                .iter()
                .find(|member| member.session_id == target_session)
                .and_then(|member| member.status.as_deref())
                == Some(expected_status)
            {
                return Ok(members);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "timed out waiting for member {} to reach status {}",
                    target_session,
                    expected_status
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn wait_for_member_presence(
        client: &mut RawClient,
        requester_session: &str,
        target_session: &str,
    ) -> Result<Vec<AgentInfo>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let members = client.comm_list(requester_session).await?;
            if members.iter().any(|member| member.session_id == target_session) {
                return Ok(members);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("timed out waiting for member {} to appear", target_session);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[test]
    fn default_await_members_targets_include_ready() {
        assert_eq!(
            default_await_target_statuses(),
            vec!["ready", "completed", "stopped", "failed"]
        );
    }

    #[test]
    fn spawn_initial_message_accepts_prompt_alias_and_prefers_explicit_initial_message() {
        let from_prompt: CommunicateInput = serde_json::from_value(serde_json::json!({
            "action": "spawn",
            "prompt": "review the diff"
        }))
        .expect("prompt alias should deserialize");
        assert_eq!(
            from_prompt.spawn_initial_message().as_deref(),
            Some("review the diff")
        );

        let preferred: CommunicateInput = serde_json::from_value(serde_json::json!({
            "action": "spawn",
            "initial_message": "preferred",
            "prompt": "fallback"
        }))
        .expect("spawn payload should deserialize");
        assert_eq!(
            preferred.spawn_initial_message().as_deref(),
            Some("preferred")
        );
    }

    #[test]
    fn communicate_input_accepts_delivery_and_share_append() {
        let delivery: CommunicateInput = serde_json::from_value(serde_json::json!({
            "action": "dm",
            "message": "ping",
            "to_session": "sess-2",
            "delivery": "wake"
        }))
        .expect("delivery mode should deserialize");
        assert_eq!(
            delivery.delivery,
            Some(crate::protocol::CommDeliveryMode::Wake)
        );

        let append: CommunicateInput = serde_json::from_value(serde_json::json!({
            "action": "share_append",
            "key": "task/123/notes",
            "value": "new line"
        }))
        .expect("share_append should deserialize");
        assert_eq!(append.action, "share_append");
    }

    #[test]
    fn format_members_includes_status_and_detail() {
        let ctx = ToolContext {
            session_id: "sess-self".to_string(),
            message_id: "msg-1".to_string(),
            tool_call_id: "call-1".to_string(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        };

        let output = format_members(
            &ctx,
            &[AgentInfo {
                session_id: "sess-peer".to_string(),
                friendly_name: Some("bear".to_string()),
                files_touched: vec!["src/main.rs".to_string()],
                status: Some("running".to_string()),
                detail: Some("working on tests".to_string()),
                role: Some("agent".to_string()),
            }],
        );

        assert!(output.output.contains("Status: running — working on tests"));
        assert!(output.output.contains("Files: src/main.rs"));
    }

    #[tokio::test]
    async fn communicate_list_and_await_members_work_end_to_end() {
        let _env_lock = crate::storage::lock_test_env();
        let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
        let repo_dir = std::env::current_dir().expect("repo cwd");
        let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
        let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

        let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
            delay: Duration::from_millis(300),
        });
        let server = Arc::new(Server::new(provider));
        let mut server_task = {
            let server = Arc::clone(&server);
            tokio::spawn(async move { server.run().await })
        };

        let socket_path = runtime_dir.path().join("jcode.sock");
        wait_for_server_socket(&socket_path, &mut server_task)
            .await
            .expect("server socket should be ready");

        let mut watcher = RawClient::connect(&socket_path)
            .await
            .expect("watcher should connect");
        let mut peer = RawClient::connect(&socket_path)
            .await
            .expect("peer should connect");
        watcher
            .subscribe(&repo_dir)
            .await
            .expect("watcher subscribe");
        peer.subscribe(&repo_dir).await.expect("peer subscribe");

        let watcher_session = watcher.session_id().await.expect("watcher session id");
        let peer_session = peer.session_id().await.expect("peer session id");

        let tool = CommunicateTool::new();
        let ctx = test_ctx(&watcher_session, &repo_dir);

        let list_output = tool
            .execute(json!({"action": "list"}), ctx.clone())
            .await
            .expect("communicate list should succeed");
        assert!(
            list_output.output.contains("Status: ready"),
            "expected communicate list to render member status, got: {}",
            list_output.output
        );

        let peer_message_id = peer
            .send_message("Reply with a short acknowledgement.")
            .await
            .expect("peer message request should send");

        let running_members =
            wait_for_member_status(&mut watcher, &watcher_session, &peer_session, "running")
                .await
                .expect("peer should enter running state");
        let running_peer = running_members
            .iter()
            .find(|member| member.session_id == peer_session)
            .expect("peer should be listed while running");
        assert_eq!(running_peer.status.as_deref(), Some("running"));

        let await_output = tool
            .execute(
                json!({
                    "action": "await_members",
                    "session_ids": [peer_session.clone()],
                    "timeout_minutes": 1
                }),
                ctx.clone(),
            )
            .await
            .expect("await_members should complete");
        assert!(
            await_output.output.contains("All members done."),
            "expected completion output, got: {}",
            await_output.output
        );
        assert!(
            await_output.output.contains("(ready)"),
            "expected await_members to treat ready as done, got: {}",
            await_output.output
        );

        peer.wait_for_done(peer_message_id)
            .await
            .expect("peer message should finish");

        let ready_members =
            wait_for_member_status(&mut watcher, &watcher_session, &peer_session, "ready")
                .await
                .expect("peer should return to ready state");
        let ready_peer = ready_members
            .iter()
            .find(|member| member.session_id == peer_session)
            .expect("peer should still be listed when ready");
        assert_eq!(ready_peer.status.as_deref(), Some("ready"));

        server_task.abort();
    }

    #[tokio::test]
    async fn communicate_spawn_with_prompt_and_summary_work_end_to_end() {
        let _env_lock = crate::storage::lock_test_env();
        let runtime_dir = tempfile::TempDir::new().expect("runtime tempdir");
        let repo_dir = std::env::current_dir().expect("repo cwd");
        let _runtime = EnvGuard::set("JCODE_RUNTIME_DIR", runtime_dir.path());
        let _debug = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

        let provider: Arc<dyn Provider> = Arc::new(DelayedTestProvider {
            delay: Duration::from_millis(100),
        });
        let server = Arc::new(Server::new(provider));
        let mut server_task = {
            let server = Arc::clone(&server);
            tokio::spawn(async move { server.run().await })
        };

        let socket_path = runtime_dir.path().join("jcode.sock");
        wait_for_server_socket(&socket_path, &mut server_task)
            .await
            .expect("server socket should be ready");

        let mut watcher = RawClient::connect(&socket_path)
            .await
            .expect("watcher should connect");
        watcher
            .subscribe(&repo_dir)
            .await
            .expect("watcher subscribe");

        let watcher_session = watcher.session_id().await.expect("watcher session id");
        let tool = CommunicateTool::new();
        let ctx = test_ctx(&watcher_session, &repo_dir);

        let spawn_output = tool
            .execute(
                json!({
                    "action": "spawn",
                    "prompt": "Reply with a short acknowledgement."
                }),
                ctx.clone(),
            )
            .await
            .expect("spawn with prompt should succeed");
        let spawned_session = spawn_output
            .output
            .strip_prefix("Spawned new agent: ")
            .expect("spawn output should include session id")
            .trim()
            .to_string();
        assert!(!spawned_session.is_empty(), "spawned session id should not be empty");

        wait_for_member_presence(&mut watcher, &watcher_session, &spawned_session)
            .await
            .expect("spawned member should appear in swarm list");

        let summary_output = tool
            .execute(
                json!({
                    "action": "summary",
                    "target_session": spawned_session
                }),
                ctx,
            )
            .await
            .expect("summary for spawned agent should succeed");
        assert!(
            summary_output.output.contains("Tool call summary for")
                || summary_output.output.contains("No tool calls found for"),
            "unexpected summary output: {}",
            summary_output.output
        );

        server_task.abort();
    }
}
