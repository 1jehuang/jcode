use super::{AcpProfile, agent_message_chunk, parse_json_object, tool_kind, tool_title};
use crate::protocol::ServerEvent;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Tracks streamed tool calls while converting daemon events to ACP updates.
pub(super) struct EventMapper {
    profile: AcpProfile,
    current_tool_id: Option<String>,
    tool_inputs: HashMap<String, String>,
}

impl EventMapper {
    /// Creates mapper state for one ACP profile.
    pub(super) fn new(profile: AcpProfile) -> Self {
        Self {
            profile,
            current_tool_id: None,
            tool_inputs: HashMap::new(),
        }
    }

    /// Maps a daemon event into ACP updates, retaining tool input by call ID.
    pub(super) fn map_event(&mut self, event: ServerEvent) -> Vec<Value> {
        match super::super::commands::unwrap_turn_event(event) {
            ServerEvent::TextDelta { text } => vec![agent_message_chunk(text)],
            ServerEvent::TextReplace { text } => vec![agent_message_chunk(text)],
            // ACP chunks cannot retract prior output. Surface the corrected
            // source links rather than silently discarding provider metadata.
            ServerEvent::TextSuffixReplace { new_text, .. } => {
                vec![agent_message_chunk(format!("\n{new_text}"))]
            }
            ServerEvent::ToolStart { id, name } => {
                self.current_tool_id = Some(id.clone());
                self.tool_inputs.entry(id.clone()).or_default();
                vec![json!({
                    "sessionUpdate": "tool_call",
                    "toolCallId": id,
                    "title": tool_title(&name),
                    "kind": tool_kind(&name),
                    "status": "pending",
                })]
            }
            ServerEvent::ToolInput { id, delta } => {
                // Older daemons omit the ID. Only those events should use the
                // most recently started tool, since keyed streams can overlap.
                let Some(tool_id) = id.or_else(|| self.current_tool_id.clone()) else {
                    return Vec::new();
                };
                let buffer = self.tool_inputs.entry(tool_id.clone()).or_default();
                buffer.push_str(&delta);
                let Some(raw_input) = parse_json_object(buffer) else {
                    return Vec::new();
                };
                let title = tool_intent(&raw_input).map(str::to_owned);
                let mut update = json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": tool_id,
                    "rawInput": raw_input,
                });
                if let Some(title) = title {
                    update["title"] = json!(title);
                }
                vec![update]
            }
            ServerEvent::ToolExec { id, name } => {
                self.current_tool_id = Some(id.clone());
                let raw_input = self
                    .tool_inputs
                    .get(&id)
                    .and_then(|input| parse_json_object(input));
                let mut update = json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": id,
                    "title": title_for_input(&name, raw_input.as_ref()),
                    "kind": tool_kind(&name),
                    "status": "in_progress",
                });
                if let Some(raw_input) = raw_input {
                    update["rawInput"] = raw_input;
                }
                vec![update]
            }
            ServerEvent::ToolDone {
                id,
                name,
                output,
                error,
            } => {
                let raw_input = self
                    .tool_inputs
                    .get(&id)
                    .and_then(|input| parse_json_object(input));
                vec![json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": id,
                    "title": title_for_input(&name, raw_input.as_ref()),
                    "kind": tool_kind(&name),
                    "status": if error.is_some() { "failed" } else { "completed" },
                    "content": [{
                        "type": "content",
                        "content": {
                            "type": "text",
                            "text": output,
                        }
                    }],
                    "rawOutput": {
                        "output": output,
                        "error": error,
                    }
                })]
            }
            ServerEvent::GeneratedImage {
                id,
                path,
                output_format,
                revised_prompt,
                ..
            } => {
                let mut text = format!("Generated image: {path} ({output_format})");
                if let Some(prompt) = revised_prompt {
                    text.push_str(&format!("\nRevised prompt: {prompt}"));
                }
                vec![json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "status": "completed",
                "content": [{
                    "type": "content",
                    "content": {
                        "type": "text",
                        "text": text,
                    }
                }]
                })]
            }
            ServerEvent::Compaction { trigger, .. } if self.profile.is_extended() => vec![json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": format!("\n[Jcode compacted context: {trigger}]\n"),
                }
            })],
            ServerEvent::SessionRenamed { display_title, .. } => vec![json!({
                "sessionUpdate": "session_info_update",
                "title": display_title,
            })],
            ServerEvent::McpStatus { servers } if self.profile.is_extended() => vec![json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": format!("\n[Jcode MCP status: {}]\n", servers.join(", ")),
                }
            })],
            _ => Vec::new(),
        }
    }
}

/// Returns a meaningful intent from a complete tool input, ignoring blank or non-string values.
fn tool_intent(input: &Value) -> Option<&str> {
    input
        .get("intent")
        .and_then(Value::as_str)
        .filter(|intent| !intent.trim().is_empty())
}

/// Keeps the tool's intent visible across later events, or uses its generic title when absent.
fn title_for_input(name: &str, input: Option<&Value>) -> String {
    input
        .and_then(tool_intent)
        .map(str::to_owned)
        .unwrap_or_else(|| tool_title(name))
}
