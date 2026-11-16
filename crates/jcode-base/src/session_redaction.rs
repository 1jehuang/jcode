use crate::message::{ContentBlock, SharedContent};

// Hold only changed fields while inspecting the immutable shared payload. This
// avoids both detaching safe messages and running secret redaction twice on
// changed messages. Opaque image and encrypted compaction fields stay untouched.
enum RedactedField {
    Text(String),
    Summary(Vec<String>),
    Json(serde_json::Value),
}

pub(super) fn redact_shared_content(content: &mut SharedContent) {
    let changes: Vec<_> = content
        .iter()
        .enumerate()
        .filter_map(|(index, block)| redacted_field(block).map(|field| (index, field)))
        .collect();
    if changes.is_empty() {
        return;
    }
    // Exactly one COW detach, after we know the output differs.
    let blocks = content.as_mut_slice();
    for (index, field) in changes {
        match (&mut blocks[index], field) {
            (
                ContentBlock::Text { text, .. }
                | ContentBlock::Reasoning { text }
                | ContentBlock::ReasoningTrace { text },
                RedactedField::Text(value),
            ) => *text = value,
            (ContentBlock::AnthropicThinking { thinking, .. }, RedactedField::Text(value)) => {
                *thinking = value
            }
            (ContentBlock::ToolResult { content, .. }, RedactedField::Text(value)) => {
                *content = value
            }
            (ContentBlock::OpenAIReasoning { summary, .. }, RedactedField::Summary(value)) => {
                *summary = value
            }
            (ContentBlock::ToolUse { input, .. }, RedactedField::Json(value)) => *input = value,
            (ContentBlock::ProviderNative { item, .. }, RedactedField::Json(value)) => {
                *item = value
            }
            _ => unreachable!("redacted field must match its source block"),
        }
    }
}

fn redacted_text(text: &str) -> Option<String> {
    let redacted = crate::message::redact_secrets(text);
    (redacted != text).then_some(redacted)
}

fn redacted_field(block: &ContentBlock) -> Option<RedactedField> {
    match block {
        ContentBlock::Text { text, .. }
        | ContentBlock::Reasoning { text }
        | ContentBlock::ReasoningTrace { text } => redacted_text(text).map(RedactedField::Text),
        ContentBlock::AnthropicThinking { thinking, .. } => {
            redacted_text(thinking).map(RedactedField::Text)
        }
        ContentBlock::OpenAIReasoning { summary, .. } => {
            let mut changed = None;
            for (index, text) in summary.iter().enumerate() {
                if let Some(value) = redacted_text(text) {
                    changed.get_or_insert_with(|| summary.clone())[index] = value;
                }
            }
            changed.map(RedactedField::Summary)
        }
        ContentBlock::ToolResult { content, .. } => redacted_text(content).map(RedactedField::Text),
        ContentBlock::ToolUse { input, .. } => redacted_json_value(input).map(RedactedField::Json),
        ContentBlock::ProviderNative { item, .. } => {
            redacted_json_value(item).map(RedactedField::Json)
        }
        ContentBlock::Image { .. }
        | ContentBlock::OpenAICompaction { .. }
        | ContentBlock::ToolReference { .. } => None,
    }
}

fn redacted_json_value(value: &serde_json::Value) -> Option<serde_json::Value> {
    use serde_json::Value;
    match value {
        Value::String(text) => redacted_text(text).map(Value::String),
        Value::Array(values) => {
            let mut changed = None;
            for (index, entry) in values.iter().enumerate() {
                if let Some(value) = redacted_json_value(entry) {
                    changed.get_or_insert_with(|| values.clone())[index] = value;
                }
            }
            changed.map(Value::Array)
        }
        Value::Object(map) => {
            let mut changed = None;
            for (key, entry) in map {
                let replacement = if is_sensitive_json_key(key) {
                    (entry.as_str() != Some("[REDACTED_SECRET]"))
                        .then(|| Value::String("[REDACTED_SECRET]".into()))
                } else {
                    redacted_json_value(entry)
                };
                if let Some(value) = replacement {
                    changed
                        .get_or_insert_with(|| map.clone())
                        .insert(key.clone(), value);
                }
            }
            changed.map(Value::Object)
        }
        _ => None,
    }
}

// The original eager implementation remains the independent test oracle.
#[cfg(test)]
pub(super) fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            *s = crate::message::redact_secrets(s);
        }
        serde_json::Value::Array(values) => {
            for entry in values {
                redact_json_value(entry);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, entry) in map.iter_mut() {
                if is_sensitive_json_key(key) {
                    *entry = serde_json::Value::String("[REDACTED_SECRET]".to_string());
                } else {
                    redact_json_value(entry);
                }
            }
        }
        _ => {}
    }
}

pub(super) fn is_sensitive_json_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("apikey")
        || normalized.ends_with("token")
        || normalized.ends_with("secret")
        || normalized.contains("password")
        || matches!(
            normalized.as_str(),
            "authorization" | "cookie" | "setcookie" | "privatekey" | "clientsecret"
        )
}
