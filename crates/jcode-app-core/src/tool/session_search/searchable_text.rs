//! Searchable text projection for transcript and tool messages.

use super::{ContentBlock, StoredMessage};

pub(super) fn searchable_message_text(msg: &StoredMessage, include_tools: bool) -> String {
    msg.content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            ContentBlock::ToolResult { content, .. } if include_tools => Some(content.clone()),
            ContentBlock::ToolUse { name, input, .. } if include_tools => {
                let input = input.to_string();
                Some(if input == "null" {
                    format!("[tool call: {name}]")
                } else {
                    format!("[tool call: {name}] {input}")
                })
            }
            ContentBlock::ToolUse { name, input, .. } => {
                let mut fields = Vec::new();
                if let Some(intent) = input.get("intent").and_then(serde_json::Value::as_str) {
                    fields.push(intent);
                }
                if name == "todo" {
                    for key in ["add", "update"] {
                        if let Some(items) = input.get(key).and_then(serde_json::Value::as_array) {
                            for item in items {
                                append_todo_task_text(item, &mut fields);
                            }
                        }
                    }
                    if let Some(phases) = input.get("phases").and_then(serde_json::Value::as_array)
                    {
                        for phase in phases {
                            if let Some(lanes) =
                                phase.get("lanes").and_then(serde_json::Value::as_array)
                            {
                                for lane in lanes {
                                    if let Some(items) =
                                        lane.get("items").and_then(serde_json::Value::as_array)
                                    {
                                        for item in items {
                                            append_todo_task_text(item, &mut fields);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                let text = fields
                    .into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                (!text.is_empty()).then_some(text)
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn append_todo_task_text<'a>(item: &'a serde_json::Value, fields: &mut Vec<&'a str>) {
    for key in ["name", "rename_to", "description"] {
        if let Some(text) = item.get(key).and_then(serde_json::Value::as_str) {
            fields.push(text);
        }
    }
}
