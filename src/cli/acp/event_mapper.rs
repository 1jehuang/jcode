use super::{AcpProfile, agent_message_chunk, parse_json_object, tool_kind, tool_title};
use crate::protocol::ServerEvent;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Track a session's tool state while converting daemon events into ACP updates.
pub(super) struct EventMapper {
    session_id: String,
    profile: AcpProfile,
    current_tool_id: Option<String>,
    tool_inputs: HashMap<String, String>,
}

impl EventMapper {
    /// Create per-session state for mapping daemon events to ACP updates.
    pub(super) fn new(session_id: String, profile: AcpProfile) -> Self {
        Self {
            session_id,
            profile,
            current_tool_id: None,
            tool_inputs: HashMap::new(),
        }
    }

    /// Convert one daemon event into zero or more ACP session updates.
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
                vec![json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": tool_id,
                    "rawInput": raw_input,
                })]
            }
            ServerEvent::ToolExec { id, name } => {
                self.current_tool_id = Some(id.clone());
                let mut update = json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": id,
                    "title": tool_title(&name),
                    "kind": tool_kind(&name),
                    "status": "in_progress",
                });
                if let Some(input) = self
                    .tool_inputs
                    .get(update["toolCallId"].as_str().unwrap_or_default())
                    && let Some(raw_input) = parse_json_object(input)
                    && let Some(object) = update.as_object_mut()
                {
                    object.insert("rawInput".to_string(), raw_input);
                }
                vec![update]
            }
            ServerEvent::ToolDone {
                id,
                name,
                output,
                error,
            } => vec![json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "title": tool_title(&name),
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
            })],
            // Keep the completed tool's text intact while exposing its image as ACP content.
            ServerEvent::SidePaneImages { session_id, images } if session_id == self.session_id => {
                images
                    .into_iter()
                    .filter(|image| {
                        matches!(
                            &image.source,
                            crate::session::RenderedImageSource::ToolResult { .. }
                        )
                    })
                    .map(|image| {
                        json!({
                            "sessionUpdate": "agent_message_chunk",
                            "content": {
                                "type": "image",
                                "mimeType": image.media_type,
                                "data": image.data,
                            }
                        })
                    })
                    .collect()
            }
            ServerEvent::GeneratedImage {
                id,
                path,
                output_format,
                revised_prompt,
                ..
            } => vec![json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "status": "completed",
                "content": [{
                    "type": "content",
                    "content": {
                        "type": "text",
                        "text": format!("Generated image: {path} ({output_format}){}", revised_prompt.map(|prompt| format!("\nRevised prompt: {prompt}")).unwrap_or_default()),
                    }
                }]
            })],
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
