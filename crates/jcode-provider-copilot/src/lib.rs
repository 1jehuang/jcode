use jcode_message_types::{
    ContentBlock, Message as ChatMessage, Role, TOOL_OUTPUT_MISSING_TEXT, ToolDefinition,
    sanitize_tool_id,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub const COPILOT_API_VERSION: &str = "2025-04-01";

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

pub const FALLBACK_MODELS: &[&str] = &[
    "claude-sonnet-4.6",
    "claude-sonnet-4.5",
    "claude-haiku-4.5",
    "claude-opus-4.6",
    "claude-opus-4.6-fast",
    "claude-opus-4.5",
    "claude-sonnet-4",
    "gemini-3-pro-preview",
    "gpt-5.4",
    "gpt-5.4-pro",
    "gpt-5.3-codex",
    "gpt-5.2-codex",
    "gpt-5.2",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex",
    "gpt-5.1",
    "gpt-5.1-codex-mini",
    "gpt-5-mini",
    "gpt-4.1",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedCatalog {
    pub models: Vec<String>,
    pub fetched_at_rfc3339: String,
}

pub fn is_known_display_model(model: &str) -> bool {
    FALLBACK_MODELS.contains(&model)
}

pub fn max_token_parameter_for_model(model: &str) -> &'static str {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.starts_with("gpt-5") {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

pub fn add_max_token_parameter(body: &mut Value, model: &str, max_tokens: u32) {
    body[max_token_parameter_for_model(model)] = json!(max_tokens);
}

/// Build OpenAI-compatible messages array from jcode's message format.
///
/// Properly pairs tool_use blocks (in assistant messages) with their
/// corresponding tool_result blocks (in user messages), handling out-of-order
/// results and missing outputs.
pub fn build_messages(system: &str, messages: &[ChatMessage]) -> Vec<Value> {
    let mut result = Vec::new();
    let missing_output = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);

    if !system.is_empty() {
        result.push(json!({
            "role": "system",
            "content": system,
        }));
    }

    let mut tool_result_last_pos: HashMap<String, usize> = HashMap::new();
    for (idx, msg) in messages.iter().enumerate() {
        if let Role::User = msg.role {
            for block in &msg.content {
                if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                    tool_result_last_pos.insert(tool_use_id.clone(), idx);
                }
            }
        }
    }

    let mut tool_calls_seen: HashSet<String> = HashSet::new();
    let mut pending_tool_results: HashMap<String, String> = HashMap::new();
    let mut used_tool_results: HashSet<String> = HashSet::new();

    for (idx, msg) in messages.iter().enumerate() {
        match msg.role {
            Role::User => {
                let mut text_parts: Vec<&str> = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            text_parts.push(text.as_str());
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            if used_tool_results.contains(tool_use_id) {
                                continue;
                            }
                            let output = if is_error == &Some(true) {
                                format!("[Error] {}", content)
                            } else if content.is_empty() {
                                TOOL_OUTPUT_MISSING_TEXT.to_string()
                            } else {
                                content.clone()
                            };
                            if tool_calls_seen.contains(tool_use_id) {
                                result.push(json!({
                                    "role": "tool",
                                    "tool_call_id": sanitize_tool_id(tool_use_id),
                                    "content": output,
                                }));
                                used_tool_results.insert(tool_use_id.clone());
                            } else if !pending_tool_results.contains_key(tool_use_id) {
                                pending_tool_results.insert(tool_use_id.clone(), output);
                            }
                        }
                        _ => {}
                    }
                }

                let text = text_parts.join("\n");
                if !text.is_empty() {
                    result.push(json!({
                        "role": "user",
                        "content": text,
                    }));
                }
            }
            Role::Assistant => {
                let mut content_text = String::new();
                let mut tool_calls = Vec::new();
                let mut post_tool_outputs: Vec<(String, String)> = Vec::new();
                let mut missing_tool_outputs: Vec<String> = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            content_text.push_str(text);
                        }
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            let args = if input.is_object() {
                                input.to_string()
                            } else {
                                "{}".to_string()
                            };
                            tool_calls.push(json!({
                                "id": sanitize_tool_id(id),
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": args,
                                }
                            }));
                            tool_calls_seen.insert(id.clone());
                            if let Some(output) = pending_tool_results.remove(id) {
                                post_tool_outputs.push((id.clone(), output));
                                used_tool_results.insert(id.clone());
                            } else {
                                let has_future_output = tool_result_last_pos
                                    .get(id)
                                    .map(|pos| *pos > idx)
                                    .unwrap_or(false);
                                if !has_future_output {
                                    missing_tool_outputs.push(id.clone());
                                    used_tool_results.insert(id.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }

                let mut assistant_msg = json!({
                    "role": "assistant",
                });

                if !content_text.is_empty() {
                    assistant_msg["content"] = json!(content_text);
                }
                if !tool_calls.is_empty() {
                    assistant_msg["tool_calls"] = json!(tool_calls);
                }

                if !content_text.is_empty() || !tool_calls.is_empty() {
                    result.push(assistant_msg);

                    for (tool_call_id, output) in post_tool_outputs {
                        result.push(json!({
                            "role": "tool",
                            "tool_call_id": sanitize_tool_id(&tool_call_id),
                            "content": output,
                        }));
                    }

                    for missing_id in missing_tool_outputs {
                        result.push(json!({
                            "role": "tool",
                            "tool_call_id": sanitize_tool_id(&missing_id),
                            "content": missing_output.clone(),
                        }));
                    }
                }
            }
        }
    }

    result
}

/// Build OpenAI-compatible tools array.
pub fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": &t.name,
                    // Prompt-visible. Approximate token cost for this field:
                    // t.description_token_estimate().
                    "description": &t.description,
                    "parameters": &t.input_schema,
                }
            })
        })
        .collect()
}
