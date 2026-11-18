//! Searchable text projection for transcript and tool messages.

use super::{ContentBlock, StoredMessage};
use std::borrow::Cow;

#[cfg(test)]
#[path = "searchable_text_tests.rs"]
mod tests;

pub(super) fn searchable_message_text(msg: &StoredMessage, include_tools: bool) -> Cow<'_, str> {
    // Plain single-block messages already have their exact searchable text.
    // Do not allocate a parts vector or copy their payload.
    if let [block] = msg.content.as_slice() {
        match block {
            ContentBlock::Text { text, .. } => return Cow::Borrowed(text),
            ContentBlock::ToolResult { content, .. } if include_tools => {
                return Cow::Borrowed(content);
            }
            _ => {}
        }
    }
    let mut parts: Vec<Cow<'_, str>> = msg
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(Cow::Borrowed(text.as_str())),
            ContentBlock::ToolResult { content, .. } if include_tools => {
                Some(Cow::Borrowed(content.as_str()))
            }
            ContentBlock::ToolUse { name, input, .. } if include_tools => {
                // Serialize into the final buffer, rather than first allocating JSON
                // and then copying it into a second formatted allocation.
                let capacity = if input.is_null() {
                    name.len() + 13
                } else {
                    32.max(name.len() + 14)
                };
                let mut text = Vec::with_capacity(capacity);
                text.extend_from_slice(b"[tool call: ");
                text.extend_from_slice(name.as_bytes());
                text.push(b']');
                if !input.is_null() {
                    text.push(b' ');
                    serde_json::to_writer(&mut text, input)
                        .expect("serializing a JSON value into memory cannot fail");
                }
                // SAFETY: the prefix/name and serde_json's output are valid UTF-8.
                Some(Cow::Owned(unsafe { String::from_utf8_unchecked(text) }))
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
                (!text.is_empty()).then_some(Cow::Owned(text))
            }
            _ => None,
        })
        .collect();
    match parts.len() {
        0 => Cow::Borrowed(""),
        1 => parts.pop().unwrap(),
        _ => Cow::Owned(parts.join("\n")),
    }
}

fn append_todo_task_text<'a>(item: &'a serde_json::Value, fields: &mut Vec<&'a str>) {
    for key in ["name", "rename_to", "description"] {
        if let Some(text) = item.get(key).and_then(serde_json::Value::as_str) {
            fields.push(text);
        }
    }
}
