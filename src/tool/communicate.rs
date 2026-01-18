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
         - \"message\": Send a message to all other agents in the codebase.\n\
         - \"list\": See who else is working in this codebase and what files they've touched."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["share", "read", "message", "list"],
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
                    "description": "For 'message': the message to send to other agents."
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

            "message" => {
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
                                    output.push_str(&format!(
                                        "  {} ({}){}\n",
                                        name,
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

            _ => Err(anyhow::anyhow!(
                "Unknown action '{}'. Valid actions: share, read, message, list",
                params.action
            )),
        }
    }
}
