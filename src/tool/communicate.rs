use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

fn socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    runtime_dir.join("jcode.sock")
}

fn send_request(request: &Value) -> Result<Value> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)?;

    let json = serde_json::to_string(request)? + "\n";
    stream.write_all(json.as_bytes())?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;

    // Skip ack, read next
    response.clear();
    reader.read_line(&mut response)?;

    let value: Value = serde_json::from_str(&response)?;
    Ok(value)
}

fn check_error(response: &Value) -> Option<String> {
    if response.get("type").and_then(|t| t.as_str()) == Some("error") {
        response.get("message").and_then(|m| m.as_str()).map(|s| s.to_string())
    } else {
        None
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
    limit: Option<usize>,
    #[serde(default)]
    task_id: Option<String>,
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
         - \"approve_plan\": (Coordinator only) Approve a plan proposal from another agent.\n\
         - \"reject_plan\": (Coordinator only) Reject a plan proposal with an optional reason.\n\
         - \"spawn\": (Coordinator only) Spawn a new agent session.\n\
         - \"stop\": (Coordinator only) Stop/destroy an agent session.\n\
         - \"assign_role\": (Coordinator only) Assign a role to an agent.\n\
         - \"summary\": Get a summary of another agent's recent tool calls.\n\
         - \"read_context\": Read another agent's full conversation context.\n\
         - \"resync_plan\": Re-sync the swarm plan to your session.\n\
         - \"assign_task\": (Coordinator only) Assign a plan task to a specific agent.\n\
         - \"subscribe_channel\": Subscribe to a named channel.\n\
         - \"unsubscribe_channel\": Unsubscribe from a named channel."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["share", "read", "message", "broadcast", "dm", "channel", "list",
                             "approve_plan", "reject_plan", "spawn", "stop", "assign_role",
                             "summary", "read_context", "resync_plan", "assign_task",
                             "subscribe_channel", "unsubscribe_channel"],
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

                let request = json!({
                    "type": "comm_share",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "key": key,
                    "value": value
                });

                match send_request(&request) {
                    Ok(_) => Ok(ToolOutput::new(format!(
                        "Shared with other agents: {} = {}",
                        key, value
                    ))),
                    Err(e) => Err(anyhow::anyhow!("Failed to share: {}", e)),
                }
            }

            "read" => {
                let request = json!({
                    "type": "comm_read",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "key": params.key
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(entries) = response.get("entries").and_then(|e| e.as_array()) {
                            if entries.is_empty() {
                                Ok(ToolOutput::new("No shared context found."))
                            } else {
                                let mut output =
                                    String::from("Shared context from other agents:\n\n");
                                for entry in entries {
                                    let key =
                                        entry.get("key").and_then(|k| k.as_str()).unwrap_or("?");
                                    let value =
                                        entry.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                                    let from = entry
                                        .get("from_name")
                                        .and_then(|f| f.as_str())
                                        .or_else(|| {
                                            entry.get("from_session").and_then(|f| f.as_str())
                                        })
                                        .unwrap_or("unknown");
                                    output.push_str(&format!(
                                        "  {} (from {}): {}\n",
                                        key, from, value
                                    ));
                                }
                                Ok(ToolOutput::new(output))
                            }
                        } else {
                            Ok(ToolOutput::new("No shared context found."))
                        }
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to read shared context: {}", e)),
                }
            }

            "message" | "broadcast" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for message action"))?;

                let request = json!({
                    "type": "comm_message",
                    "id": 1,
                    "from_session": ctx.session_id,
                    "message": message
                });

                match send_request(&request) {
                    Ok(_) => Ok(ToolOutput::new(format!(
                        "Message sent to other agents: {}",
                        message
                    ))),
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

                let request = json!({
                    "type": "comm_message",
                    "id": 1,
                    "from_session": ctx.session_id,
                    "message": message,
                    "to_session": to_session
                });

                match send_request(&request) {
                    Ok(_) => Ok(ToolOutput::new(format!(
                        "Direct message sent to {}: {}",
                        to_session, message
                    ))),
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

                let request = json!({
                    "type": "comm_message",
                    "id": 1,
                    "from_session": ctx.session_id,
                    "message": message,
                    "channel": channel
                });

                match send_request(&request) {
                    Ok(_) => Ok(ToolOutput::new(format!(
                        "Channel message sent to #{}: {}",
                        channel, message
                    ))),
                    Err(e) => Err(anyhow::anyhow!("Failed to send channel message: {}", e)),
                }
            }

            "list" => {
                let request = json!({
                    "type": "comm_list",
                    "id": 1,
                    "session_id": ctx.session_id
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(members) = response.get("members").and_then(|m| m.as_array()) {
                            if members.is_empty() {
                                Ok(ToolOutput::new("No other agents in this codebase."))
                            } else {
                                let mut output = String::from("Agents in this codebase:\n\n");
                                for member in members {
                                    let name = member
                                        .get("friendly_name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("unknown");
                                    let session = member
                                        .get("session_id")
                                        .and_then(|s| s.as_str())
                                        .unwrap_or("?");
                                    let role = member
                                        .get("role")
                                        .and_then(|r| r.as_str())
                                        .unwrap_or("agent");
                                    let files = member
                                        .get("files_touched")
                                        .and_then(|f| f.as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .filter_map(|v| v.as_str())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        })
                                        .unwrap_or_default();

                                    let is_me = session == ctx.session_id;
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
                                Ok(ToolOutput::new(output))
                            }
                        } else {
                            Ok(ToolOutput::new("No agents found."))
                        }
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list agents: {}", e)),
                }
            }

            "approve_plan" => {
                let proposer = params.proposer_session.ok_or_else(|| {
                    anyhow::anyhow!("'proposer_session' is required for approve_plan action")
                })?;

                let request = json!({
                    "type": "comm_approve_plan",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "proposer_session": proposer
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
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

                let request = json!({
                    "type": "comm_reject_plan",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "proposer_session": proposer,
                    "reason": params.reason
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
                        let reason_msg = params
                            .reason
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
                let request = json!({
                    "type": "comm_spawn",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "working_dir": params.working_dir,
                    "initial_message": params.initial_message
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
                        let new_id = response
                            .get("new_session_id")
                            .and_then(|s| s.as_str())
                            .unwrap_or("unknown");
                        Ok(ToolOutput::new(format!("Spawned new agent: {}", new_id)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to spawn agent: {}", e)),
                }
            }

            "stop" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for stop action")
                })?;

                let request = json!({
                    "type": "comm_stop",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "target_session": target
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
                        Ok(ToolOutput::new(format!("Stopped agent: {}", target)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to stop agent: {}", e)),
                }
            }

            "assign_role" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for assign_role action")
                })?;
                let role = params.role.ok_or_else(|| {
                    anyhow::anyhow!("'role' is required for assign_role action")
                })?;

                let request = json!({
                    "type": "comm_assign_role",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "target_session": target,
                    "role": role
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
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

                let request = json!({
                    "type": "comm_summary",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "target_session": target,
                    "limit": params.limit
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
                        if let Some(calls) =
                            response.get("tool_calls").and_then(|t| t.as_array())
                        {
                            if calls.is_empty() {
                                Ok(ToolOutput::new(format!(
                                    "No tool calls found for {}",
                                    target
                                )))
                            } else {
                                let mut output = format!("Tool call summary for {}:\n\n", target);
                                for call in calls {
                                    let name = call
                                        .get("tool_name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("?");
                                    let brief = call
                                        .get("brief_output")
                                        .and_then(|b| b.as_str())
                                        .unwrap_or("");
                                    output.push_str(&format!("  {} — {}\n", name, brief));
                                }
                                Ok(ToolOutput::new(output))
                            }
                        } else {
                            Ok(ToolOutput::new("No tool call data returned."))
                        }
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to get summary: {}", e)),
                }
            }

            "read_context" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for read_context action")
                })?;

                let request = json!({
                    "type": "comm_read_context",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "target_session": target
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
                        if let Some(messages) =
                            response.get("messages").and_then(|m| m.as_array())
                        {
                            if messages.is_empty() {
                                Ok(ToolOutput::new(format!(
                                    "No conversation history for {}",
                                    target
                                )))
                            } else {
                                let mut output =
                                    format!("Conversation context for {} ({} messages):\n\n", target, messages.len());
                                for msg in messages {
                                    let role = msg
                                        .get("role")
                                        .and_then(|r| r.as_str())
                                        .unwrap_or("?");
                                    let content = msg
                                        .get("content")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("");
                                    // Truncate long messages
                                    let truncated = if content.len() > 500 {
                                        format!("{}...", &content[..500])
                                    } else {
                                        content.to_string()
                                    };
                                    output.push_str(&format!("[{}] {}\n\n", role, truncated));
                                }
                                Ok(ToolOutput::new(output))
                            }
                        } else {
                            Ok(ToolOutput::new("No context data returned."))
                        }
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to read context: {}", e)),
                }
            }

            "resync_plan" => {
                let request = json!({
                    "type": "comm_resync_plan",
                    "id": 1,
                    "session_id": ctx.session_id
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
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

                let request = json!({
                    "type": "comm_assign_task",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "target_session": target,
                    "task_id": task_id,
                    "message": params.message
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
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

                let request = json!({
                    "type": "comm_subscribe_channel",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "channel": channel
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
                        Ok(ToolOutput::new(format!("Subscribed to #{}", channel)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to subscribe: {}", e)),
                }
            }

            "unsubscribe_channel" => {
                let channel = params.channel.ok_or_else(|| {
                    anyhow::anyhow!("'channel' is required for unsubscribe_channel action")
                })?;

                let request = json!({
                    "type": "comm_unsubscribe_channel",
                    "id": 1,
                    "session_id": ctx.session_id,
                    "channel": channel
                });

                match send_request(&request) {
                    Ok(response) => {
                        if let Some(err) = check_error(&response) {
                            return Err(anyhow::anyhow!("{}", err));
                        }
                        Ok(ToolOutput::new(format!("Unsubscribed from #{}", channel)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to unsubscribe: {}", e)),
                }
            }

            _ => Err(anyhow::anyhow!(
                "Unknown action '{}'. Valid actions: share, read, message, broadcast, dm, channel, list, \
                 approve_plan, reject_plan, spawn, stop, assign_role, summary, read_context, \
                 resync_plan, assign_task, subscribe_channel, unsubscribe_channel",
                params.action
            )),
        }
    }
}
