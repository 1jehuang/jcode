use super::{Session, StoredDisplayRole};
use crate::message::{ContentBlock, Role, ToolCall};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub tool_data: Option<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderedImageSource {
    UserInput,
    ToolResult { tool_name: String },
    Other { role: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderedImage {
    pub media_type: String,
    pub data: String,
    pub label: Option<String>,
    pub source: RenderedImageSource,
}

fn image_source_for_message(role: Role, tool: Option<&ToolCall>) -> RenderedImageSource {
    if let Some(tool) = tool {
        return RenderedImageSource::ToolResult {
            tool_name: tool.name.clone(),
        };
    }

    match role {
        Role::User => RenderedImageSource::UserInput,
        Role::Assistant => RenderedImageSource::Other {
            role: "assistant".to_string(),
        },
    }
}

fn fallback_image_label_for_tool(tool: &ToolCall) -> Option<String> {
    tool.input
        .get("file_path")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_attached_image_label(text: &str) -> Option<String> {
    let prefix = "[Attached image associated with the preceding tool result: ";
    let suffix = "]";
    text.trim()
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn render_images(session: &Session) -> Vec<RenderedImage> {
    render_messages_and_images(session).1
}

pub fn has_rendered_images(session: &Session) -> bool {
    session.messages.iter().any(|msg| {
        msg.content
            .iter()
            .any(|block| matches!(block, ContentBlock::Image { .. }))
    })
}

pub fn summarize_tool_calls(
    session: &Session,
    limit: usize,
) -> Vec<crate::protocol::ToolCallSummary> {
    let mut calls: Vec<crate::protocol::ToolCallSummary> = Vec::new();

    for msg in session.messages.iter().rev() {
        if calls.len() >= limit {
            break;
        }

        let text_summary = msg
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                ContentBlock::OpenAICompaction { .. } => Some("[OpenAI native compaction]"),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        for block in &msg.content {
            if calls.len() >= limit {
                break;
            }

            if let ContentBlock::ToolUse { name, input, .. } = block {
                let fallback = input.to_string();
                let brief = if text_summary.trim().is_empty() {
                    crate::util::truncate_str(&fallback, 200).to_string()
                } else {
                    crate::util::truncate_str(&text_summary, 200).to_string()
                };
                calls.push(crate::protocol::ToolCallSummary {
                    tool_name: name.clone(),
                    brief_output: brief,
                    timestamp_secs: msg.timestamp.map(|ts| ts.timestamp().max(0) as u64),
                });
            }
        }
    }

    calls.reverse();
    calls
}

/// Convert stored session messages into renderable messages (including tool output).
pub fn render_messages(session: &Session) -> Vec<RenderedMessage> {
    render_messages_and_images(session).0
}

pub fn render_messages_and_images(session: &Session) -> (Vec<RenderedMessage>, Vec<RenderedImage>) {
    let mut rendered: Vec<RenderedMessage> = Vec::new();
    let mut images: Vec<RenderedImage> = Vec::new();
    let mut tool_map: HashMap<String, ToolCall> = HashMap::new();

    for msg in &session.messages {
        let role = match msg.display_role {
            Some(StoredDisplayRole::System) => "system",
            Some(StoredDisplayRole::BackgroundTask) => "background_task",
            None => match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            },
        };
        let message_role = msg.role.clone();
        let mut text = String::new();
        let mut tool_calls: Vec<String> = Vec::new();
        let mut current_tool: Option<ToolCall> = None;
        let mut last_image_idx: Option<usize> = None;

        for block in &msg.content {
            match block {
                ContentBlock::Text { text: t, .. } => {
                    text.push_str(t);
                    if let Some(label) = parse_attached_image_label(t)
                        && let Some(last_idx) = last_image_idx
                        && let Some(image) = images.get_mut(last_idx)
                    {
                        image.label = Some(label);
                    }
                }
                ContentBlock::ToolUse { id, name, input } => {
                    let tool_call = ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        intent: ToolCall::intent_from_input(input),
                    };
                    tool_map.insert(id.clone(), tool_call);
                    tool_calls.push(name.clone());
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    if !text.is_empty() {
                        rendered.push(RenderedMessage {
                            role: role.to_string(),
                            content: std::mem::take(&mut text),
                            tool_calls: tool_calls.clone(),
                            tool_data: None,
                        });
                    }

                    let tool_data = tool_map.get(tool_use_id).cloned().or_else(|| {
                        Some(ToolCall {
                            id: tool_use_id.clone(),
                            name: "tool".to_string(),
                            input: serde_json::Value::Null,
                            intent: None,
                        })
                    });
                    current_tool = tool_data.clone();

                    rendered.push(RenderedMessage {
                        role: "tool".to_string(),
                        content: content.clone(),
                        tool_calls: Vec::new(),
                        tool_data,
                    });
                }
                ContentBlock::Reasoning { .. } => {}
                ContentBlock::Image { media_type, data } => {
                    images.push(RenderedImage {
                        media_type: media_type.clone(),
                        data: data.clone(),
                        label: current_tool
                            .as_ref()
                            .and_then(fallback_image_label_for_tool),
                        source: image_source_for_message(
                            message_role.clone(),
                            current_tool.as_ref(),
                        ),
                    });
                    last_image_idx = Some(images.len().saturating_sub(1));
                }
                ContentBlock::OpenAICompaction { .. } => {}
            }
        }

        if !text.is_empty() {
            rendered.push(RenderedMessage {
                role: role.to_string(),
                content: text,
                tool_calls,
                tool_data: None,
            });
        }
    }

    (rendered, images)
}
