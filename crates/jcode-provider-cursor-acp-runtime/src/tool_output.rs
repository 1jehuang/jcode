use jcode_message_types::StreamEvent;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct ToolOutputs {
    pending: HashMap<String, Snapshot>,
    finished: HashSet<String>,
}

#[derive(Default)]
struct Snapshot {
    content: Option<Value>,
    raw: Option<Value>,
}

impl ToolOutputs {
    pub(super) fn update(&mut self, update: &Value) -> Option<StreamEvent> {
        let id = update.get("toolCallId")?.as_str()?.to_owned();
        if id.is_empty() || self.finished.contains(&id) {
            return None;
        }
        let snapshot = self.pending.entry(id.clone()).or_default();
        if let Some(content) = update.get("content").filter(|v| !v.is_null()) {
            snapshot.content = Some(content.clone());
        }
        if let Some(raw) = update.get("rawOutput").filter(|v| !v.is_null()) {
            snapshot.raw = Some(raw.clone());
        }
        let status = update.get("status").and_then(Value::as_str).unwrap_or("");
        if !matches!(status, "completed" | "failed") {
            return None;
        }
        let snapshot = self.pending.remove(&id)?;
        self.finished.insert(id.clone());
        let is_error = status == "failed"
            || snapshot
                .raw
                .as_ref()
                .and_then(|v| v.get("exitCode"))
                .and_then(Value::as_i64)
                .is_some_and(|code| code != 0);
        let content = snapshot
            .raw
            .as_ref()
            .and_then(raw_text)
            .unwrap_or_else(|| standard_text(snapshot.content.as_ref()));
        Some(StreamEvent::ToolResult {
            tool_use_id: id,
            content,
            is_error,
        })
    }
}

fn raw_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => (!text.is_empty()).then(|| text.clone()),
        Value::Array(items) if items.is_empty() => None,
        Value::Object(object) if object.is_empty() => None,
        Value::Object(object) if object.contains_key("stdout") || object.contains_key("stderr") => {
            let stdout = value.get("stdout").and_then(Value::as_str).unwrap_or("");
            let stderr = value.get("stderr").and_then(Value::as_str).unwrap_or("");
            match (stdout.is_empty(), stderr.is_empty()) {
                (false, false) => Some(format!("{stdout}\n[stderr]\n{stderr}")),
                (false, true) => Some(stdout.to_owned()),
                (true, false) => Some(stderr.to_owned()),
                (true, true) => None,
            }
        }
        _ => Some(value.to_string()),
    }
}

fn standard_text(value: Option<&Value>) -> String {
    let parts: Vec<_> = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| match item.get("type")?.as_str()? {
            "content" => {
                let content = item.get("content")?;
                match content.get("type")?.as_str()? {
                    "text" => content.get("text")?.as_str().map(str::to_owned),
                    "resource" => {
                        let resource = content.get("resource")?;
                        let text = resource.get("text")?.as_str()?;
                        Some(match resource.get("uri").and_then(Value::as_str) {
                            Some(uri) => format!("{uri}\n{text}"),
                            None => text.to_owned(),
                        })
                    }
                    "resource_link" => content.get("uri")?.as_str().map(str::to_owned),
                    _ => None,
                }
            }
            "diff" => {
                let path = item.get("path")?.as_str()?;
                let new = item.get("newText")?.as_str()?;
                let old = item.get("oldText").and_then(Value::as_str);
                Some(match old {
                    Some(old) => format!("{path}\noldText:\n{old}\nnewText:\n{new}"),
                    None => format!("{path}\nnewText:\n{new}"),
                })
            }
            _ => None,
        })
        .collect();
    parts.join("\n")
}
