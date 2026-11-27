//! Gemini Interactions API (`POST /v1beta/interactions`) request building and
//! response decoding, used for native deferred tool loading.
//!
//! Tool retrieval (`{"type":"tool_search"}` plus `defer_loading: true` on a
//! function) only exists on the Interactions API, not `generateContent`. The
//! conversation is sent statelessly (`store: false`) with the full history as
//! `input` steps. Server-side retrieval returns `tool_search_call` and
//! `tool_search_result` steps (and `thought` steps) carrying opaque
//! `signature`s that must be replayed unchanged, so they are persisted as
//! [`ContentBlock::ProviderNative`] items under [`PROVIDER_NATIVE_GEMINI`].

use crate::gemini_compatible_schema;
use jcode_message_types::{ContentBlock, Message, Role, ToolCall, ToolDefinition};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::HashSet;

/// `ProviderNative` provider tag for replayable Interactions steps.
pub const PROVIDER_NATIVE_GEMINI: &str = "gemini";

/// Step types that must be replayed verbatim (with their signatures).
const NATIVE_STEP_TYPES: &[&str] = &["tool_search_call", "tool_search_result", "thought"];

/// Whether `model` supports Interactions tool retrieval (Gemini 3.6+ and
/// Gemini 4 families, per the tool retrieval docs).
pub fn model_supports_tool_retrieval(model: &str) -> bool {
    let model = model.trim().trim_start_matches("models/");
    let Some(rest) = model.strip_prefix("gemini-") else {
        return false;
    };
    let version: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = version.split('.');
    let major: u32 = match parts.next().and_then(|p| p.parse().ok()) {
        Some(major) => major,
        None => return false,
    };
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    major > 3 || (major == 3 && minor >= 6)
}

/// Tools referenced by a `ToolReference` (for example via `load_tools`).
/// The model calls those by name without a server-side search, which the API
/// rejects for still-deferred functions, so they are sent eagerly.
fn referenced_tools(messages: &[Message]) -> HashSet<&str> {
    messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::ToolReference { tool_name, .. } => Some(tool_name.as_str()),
            _ => None,
        })
        .collect()
}

/// Build the Interactions `tools` array. Adds `{"type":"tool_search"}`
/// whenever any definition stays deferred (required on every such request).
pub fn build_interaction_tools(tools: &[ToolDefinition], messages: &[Message]) -> Vec<Value> {
    let referenced = referenced_tools(messages);
    let mut any_deferred = false;
    let mut out: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let mut decl = json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": gemini_compatible_schema(&tool.input_schema),
            });
            if tool.defer_loading && !referenced.contains(tool.name.as_str()) {
                any_deferred = true;
                decl["defer_loading"] = Value::Bool(true);
            }
            decl
        })
        .collect();
    if any_deferred {
        out.insert(0, json!({"type": "tool_search"}));
    }
    out
}

fn tool_name_for(tool_use_id: &str, messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .flat_map(|message| &message.content)
        .find_map(|block| match block {
            ContentBlock::ToolUse { id, name, .. } if id == tool_use_id => Some(name.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "tool".to_string())
}

/// Convert jcode history into Interactions `input` steps.
pub fn build_interaction_steps(messages: &[Message]) -> Vec<Value> {
    let mut steps = Vec::new();
    for message in messages {
        match message.role {
            Role::User => {
                let mut content: Vec<Value> = Vec::new();
                let flush = |steps: &mut Vec<Value>, content: &mut Vec<Value>| {
                    if !content.is_empty() {
                        steps.push(json!({
                            "type": "user_input",
                            "content": std::mem::take(content),
                        }));
                    }
                };
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text, .. } if !text.is_empty() => {
                            content.push(json!({"type": "text", "text": text}));
                        }
                        ContentBlock::Image { media_type, data } => {
                            content.push(json!({
                                "type": "image",
                                "mime_type": media_type,
                                "data": data,
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content: result,
                            is_error,
                        } => {
                            flush(&mut steps, &mut content);
                            let mut step = json!({
                                "type": "function_result",
                                "name": tool_name_for(tool_use_id, messages),
                                "call_id": tool_use_id,
                                "result": [{"type": "text", "text": result}],
                            });
                            if is_error.unwrap_or(false) {
                                step["is_error"] = Value::Bool(true);
                            }
                            steps.push(step);
                        }
                        _ => {}
                    }
                }
                flush(&mut steps, &mut content);
            }
            Role::Assistant => {
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text, .. } if !text.is_empty() => {
                            steps.push(json!({
                                "type": "model_output",
                                "content": [{"type": "text", "text": text}],
                            }));
                        }
                        ContentBlock::ProviderNative { provider, item }
                            if provider == PROVIDER_NATIVE_GEMINI =>
                        {
                            steps.push(item.clone());
                        }
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input,
                            thought_signature,
                        } => {
                            let mut step = json!({
                                "type": "function_call",
                                "id": id,
                                "name": name,
                                "arguments": ToolCall::input_as_object(input),
                            });
                            if let Some(signature) =
                                thought_signature.as_ref().filter(|sig| !sig.is_empty())
                            {
                                step["signature"] = Value::String(signature.clone());
                            }
                            steps.push(step);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    steps
}

/// Full `POST /interactions` body.
pub fn build_interaction_request(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    system_instruction: Option<String>,
) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(model.to_string()));
    body.insert("store".into(), Value::Bool(false));
    body.insert(
        "input".into(),
        Value::Array(build_interaction_steps(messages)),
    );
    if let Some(system) = system_instruction.filter(|s| !s.trim().is_empty()) {
        body.insert("system_instruction".into(), Value::String(system));
    }
    let tools = build_interaction_tools(tools, messages);
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools));
    }
    Value::Object(body)
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct InteractionUsage {
    #[serde(default)]
    pub total_input_tokens: Option<u64>,
    #[serde(default)]
    pub total_output_tokens: Option<u64>,
    #[serde(default)]
    pub total_cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct InteractionResponse {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub steps: Vec<Value>,
    #[serde(default)]
    pub usage: Option<InteractionUsage>,
    #[serde(default)]
    pub errors: Option<Value>,
}

/// A decoded response step, in arrival order.
#[derive(Debug, Clone, PartialEq)]
pub enum InteractionEvent {
    Text(String),
    Thinking(String),
    /// Replay-only step (search call/result, thought), stored verbatim.
    Native(Value),
    FunctionCall {
        id: String,
        name: String,
        arguments: Value,
        signature: Option<String>,
    },
}

fn content_text(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// Decode response steps. Echoed input steps (`user_input`,
/// `function_result`) are skipped.
pub fn interaction_events(response: &InteractionResponse) -> Vec<InteractionEvent> {
    let mut events = Vec::new();
    for step in &response.steps {
        let kind = step.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "model_output" => {
                let text = content_text(step.get("content"));
                if !text.is_empty() {
                    events.push(InteractionEvent::Text(text));
                }
            }
            "function_call" => {
                let raw_id = step
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                events.push(InteractionEvent::FunctionCall {
                    id: raw_id,
                    name: step
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: step.get("arguments").cloned().unwrap_or_else(|| json!({})),
                    signature: step
                        .get("signature")
                        .and_then(Value::as_str)
                        .filter(|sig| !sig.is_empty())
                        .map(str::to_string),
                });
            }
            kind if NATIVE_STEP_TYPES.contains(&kind) => {
                if kind == "thought" {
                    let summary = content_text(step.get("summary"));
                    if !summary.is_empty() {
                        events.push(InteractionEvent::Thinking(summary));
                    }
                }
                events.push(InteractionEvent::Native(step.clone()));
            }
            _ => {}
        }
    }
    events
}

#[cfg(test)]
#[path = "interactions_tests.rs"]
mod tests;
