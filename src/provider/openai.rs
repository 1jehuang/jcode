use super::{EventStream, Provider};
use crate::auth::codex::CodexCredentials;
use crate::auth::oauth;
use crate::message::{
    ContentBlock, Message as ChatMessage, Role, StreamEvent, ToolDefinition,
    TOOL_OUTPUT_MISSING_TEXT,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{SinkExt, Stream, StreamExt};
use reqwest::header::HeaderValue;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const CHATGPT_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const RESPONSES_PATH: &str = "responses";
const DEFAULT_MODEL: &str = "gpt-5.3-codex-spark";
const ORIGINATOR: &str = "codex_cli_rs";
const CHATGPT_INSTRUCTIONS: &str = include_str!("../prompts/gpt-5.1-codex-max_prompt.md");

/// Maximum number of retries for transient errors
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const RETRY_BASE_DELAY_MS: u64 = 1000;
const WEBSOCKET_UPGRADE_REQUIRED_ERROR: StatusCode = StatusCode::UPGRADE_REQUIRED;
const WEBSOCKET_FALLBACK_NOTICE: &str = "falling back from websockets to https transport";
const WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS: u64 = 45;
const WEBSOCKET_IDLE_TIMEOUT_SECS: u64 = 30;
const WEBSOCKET_COMPLETION_TIMEOUT_SECS: u64 = 90;

/// Available OpenAI/Codex models
const AVAILABLE_MODELS: &[&str] = &[
    "codex-mini-latest",
    "gpt-5.3-codex",
    "gpt-5.3-codex-spark",
    "gpt-5.2-chat-latest",
    "gpt-5.2-codex",
    "gpt-5.2-pro",
    "gpt-5.1-codex-mini",
    "gpt-5.1-codex-max",
    "gpt-5.2",
    "gpt-5.1-chat-latest",
    "gpt-5.1",
    "gpt-5.1-codex",
    "gpt-5-chat-latest",
    "gpt-5-codex",
    "gpt-5-codex-mini",
    "gpt-5-pro",
    "gpt-5-mini",
    "gpt-5-nano",
    "gpt-5",
];

#[derive(Clone, Copy)]
enum OpenAITransportMode {
    Auto,
    WebSocket,
    HTTPS,
}

impl OpenAITransportMode {
    fn from_config(raw: Option<&str>) -> Self {
        let Some(raw) = raw else {
            return Self::Auto;
        };
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Self::Auto,
            "websocket" | "ws" | "wss" => Self::WebSocket,
            "https" | "http" | "sse" => Self::HTTPS,
            other => {
                crate::logging::warn(&format!(
                    "Unknown JCODE_OPENAI_TRANSPORT '{}'; using auto. Use: auto, websocket, or https.",
                    other
                ));
                Self::Auto
            }
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::WebSocket => "websocket",
            Self::HTTPS => "https",
        }
    }
}

#[derive(Debug)]
enum OpenAIStreamFailure {
    FallbackToHttps(anyhow::Error),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for OpenAIStreamFailure {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

#[derive(Clone, Copy)]
enum OpenAITransport {
    WebSocket,
    HTTPS,
}

impl OpenAITransport {
    fn as_str(self) -> &'static str {
        match self {
            Self::WebSocket => "websocket",
            Self::HTTPS => "https",
        }
    }
}

pub struct OpenAIProvider {
    client: Client,
    credentials: Arc<RwLock<CodexCredentials>>,
    model: Arc<RwLock<String>>,
    prompt_cache_key: Option<String>,
    prompt_cache_retention: Option<String>,
    reasoning_effort: Option<String>,
    transport_mode: OpenAITransportMode,
    websocket_disabled: Arc<AtomicBool>,
}

impl OpenAIProvider {
    pub fn new(credentials: CodexCredentials) -> Self {
        // Check for model override from environment
        let mut model =
            std::env::var("JCODE_OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        if !AVAILABLE_MODELS.contains(&model.as_str()) {
            crate::logging::info(&format!(
                "Warning: '{}' is not supported; falling back to '{}'",
                model, DEFAULT_MODEL
            ));
            model = DEFAULT_MODEL.to_string();
        }

        let prompt_cache_key = std::env::var("JCODE_OPENAI_PROMPT_CACHE_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let prompt_cache_retention = std::env::var("JCODE_OPENAI_PROMPT_CACHE_RETENTION")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let prompt_cache_retention = match prompt_cache_retention.as_deref() {
            Some("in_memory") | Some("24h") => prompt_cache_retention,
            Some(other) => {
                crate::logging::info(&format!(
                    "Warning: Unsupported JCODE_OPENAI_PROMPT_CACHE_RETENTION '{}'; expected 'in_memory' or '24h'",
                    other
                ));
                None
            }
            None => None,
        };
        let reasoning_effort = crate::config::config()
            .provider
            .openai_reasoning_effort
            .as_deref()
            .and_then(Self::normalize_reasoning_effort);
        let transport_mode = OpenAITransportMode::from_config(
            crate::config::config().provider.openai_transport.as_deref(),
        );

        Self {
            client: Client::new(),
            credentials: Arc::new(RwLock::new(credentials)),
            model: Arc::new(RwLock::new(model)),
            prompt_cache_key,
            prompt_cache_retention,
            reasoning_effort,
            transport_mode,
            websocket_disabled: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn get_access_token(&self) -> Result<String> {
        let tokens = self.credentials.read().await;
        if tokens.access_token.is_empty() {
            anyhow::bail!("OpenAI access token is empty");
        }

        if let Some(expires_at) = tokens.expires_at {
            let now = chrono::Utc::now().timestamp_millis();
            if expires_at < now + 300_000 && !tokens.refresh_token.is_empty() {
                drop(tokens);
                return self.refresh_access_token().await;
            }
        }

        Ok(tokens.access_token.clone())
    }

    async fn refresh_access_token(&self) -> Result<String> {
        let mut tokens = self.credentials.write().await;
        if tokens.refresh_token.is_empty() {
            anyhow::bail!("OpenAI refresh token is missing");
        }

        let refreshed = oauth::refresh_openai_tokens(&tokens.refresh_token).await?;
        let id_token = refreshed
            .id_token
            .clone()
            .or_else(|| tokens.id_token.clone());
        let account_id = tokens.account_id.clone();

        *tokens = CodexCredentials {
            access_token: refreshed.access_token,
            refresh_token: refreshed.refresh_token,
            id_token,
            account_id,
            expires_at: Some(refreshed.expires_at),
        };

        Ok(tokens.access_token.clone())
    }

    fn is_chatgpt_mode(credentials: &CodexCredentials) -> bool {
        !credentials.refresh_token.is_empty() || credentials.id_token.is_some()
    }

    fn should_prefer_websocket(model: &str) -> bool {
        model.contains("codex") || model.starts_with("gpt-5")
    }

    fn normalize_reasoning_effort(raw: &str) -> Option<String> {
        let value = raw.trim().to_lowercase();
        if value.is_empty() {
            return None;
        }
        match value.as_str() {
            "none" | "low" | "medium" | "high" | "xhigh" => Some(value),
            other => {
                crate::logging::info(&format!(
                    "Warning: Unsupported OpenAI reasoning effort '{}'; expected none|low|medium|high|xhigh. Using 'xhigh'.",
                    other
                ));
                Some("xhigh".to_string())
            }
        }
    }

    fn responses_url(credentials: &CodexCredentials) -> String {
        let base = if Self::is_chatgpt_mode(credentials) {
            CHATGPT_API_BASE
        } else {
            OPENAI_API_BASE
        };
        format!("{}/{}", base.trim_end_matches('/'), RESPONSES_PATH)
    }

    fn responses_ws_url(credentials: &CodexCredentials) -> String {
        let base = Self::responses_url(credentials);
        base.replace("https://", "wss://")
            .replace("http://", "ws://")
    }

    async fn model_id(&self) -> String {
        self.model.read().await.clone()
    }

    async fn send_request(&self, request: &Value, access_token: &str) -> Result<reqwest::Response> {
        let credentials = self.credentials.read().await;
        let url = Self::responses_url(&credentials);
        let mut builder = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json");

        if Self::is_chatgpt_mode(&credentials) {
            builder = builder.header("originator", ORIGINATOR);
            if let Some(account_id) = credentials.account_id.as_ref() {
                builder = builder.header("chatgpt-account-id", account_id);
            }
        }

        builder
            .json(request)
            .send()
            .await
            .context("Failed to send request to OpenAI API")
    }
}

fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "strict": false,
                "parameters": t.input_schema,
            })
        })
        .collect()
}

fn build_responses_input(messages: &[ChatMessage]) -> Vec<Value> {
    use std::collections::{HashMap, HashSet};

    let missing_output = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);

    // Track the last position of tool outputs so we can detect future outputs.
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

    let mut items = Vec::new();
    let mut open_calls: HashSet<String> = HashSet::new();
    let mut pending_outputs: HashMap<String, String> = HashMap::new();
    let mut used_outputs: HashSet<String> = HashSet::new();
    let mut skipped_results = 0usize;
    let mut delayed_results = 0usize;
    let mut injected_missing = 0usize;

    for (idx, msg) in messages.iter().enumerate() {
        match msg.role {
            Role::User => {
                let mut content_parts: Vec<serde_json::Value> = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Image { media_type, data } => {
                            content_parts.push(serde_json::json!({
                                "type": "input_image",
                                "image_url": format!("data:{};base64,{}", media_type, data)
                            }));
                        }
                        ContentBlock::Text { text, .. } => {
                            content_parts.push(serde_json::json!({
                                "type": "input_text",
                                "text": text
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            // Flush any accumulated content_parts before tool result
                            if !content_parts.is_empty() {
                                items.push(serde_json::json!({
                                    "type": "message",
                                    "role": "user",
                                    "content": std::mem::take(&mut content_parts)
                                }));
                            }
                            if used_outputs.contains(tool_use_id.as_str()) {
                                skipped_results += 1;
                                continue;
                            }
                            let output = if is_error == &Some(true) {
                                format!("[Error] {}", content)
                            } else {
                                content.clone()
                            };
                            if open_calls.contains(tool_use_id.as_str()) {
                                items.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": tool_use_id,
                                    "output": output
                                }));
                                open_calls.remove(tool_use_id.as_str());
                                used_outputs.insert(tool_use_id.clone());
                            } else if pending_outputs.contains_key(tool_use_id.as_str()) {
                                skipped_results += 1;
                            } else {
                                pending_outputs.insert(tool_use_id.clone(), output);
                                delayed_results += 1;
                            }
                        }
                        _ => {}
                    }
                }
                if !content_parts.is_empty() {
                    items.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": content_parts
                    }));
                }
            }
            Role::Assistant => {
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            items.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{ "type": "output_text", "text": text }]
                            }));
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            let arguments = serde_json::to_string(&input).unwrap_or_default();
                            items.push(serde_json::json!({
                                "type": "function_call",
                                "name": name,
                                "arguments": arguments,
                                "call_id": id
                            }));

                            if let Some(output) = pending_outputs.remove(id.as_str()) {
                                items.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": id,
                                    "output": output
                                }));
                                used_outputs.insert(id.clone());
                            } else {
                                let has_future_output = tool_result_last_pos
                                    .get(id)
                                    .map(|pos| *pos > idx)
                                    .unwrap_or(false);
                                if has_future_output {
                                    open_calls.insert(id.clone());
                                } else {
                                    injected_missing += 1;
                                    items.push(serde_json::json!({
                                        "type": "function_call_output",
                                        "call_id": id,
                                        "output": missing_output.clone()
                                    }));
                                    used_outputs.insert(id.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Resolve any remaining open calls.
    for call_id in open_calls {
        if used_outputs.contains(&call_id) {
            continue;
        }
        if let Some(output) = pending_outputs.remove(&call_id) {
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output
            }));
        } else {
            injected_missing += 1;
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": missing_output.clone()
            }));
        }
    }

    if delayed_results > 0 {
        crate::logging::info(&format!(
            "[openai] Delayed {} tool output(s) to preserve call ordering",
            delayed_results
        ));
    }

    if !pending_outputs.is_empty() {
        skipped_results += pending_outputs.len();
    }

    if injected_missing > 0 {
        crate::logging::info(&format!(
            "[openai] Injected {} synthetic tool output(s) to prevent API error",
            injected_missing
        ));
    }
    if skipped_results > 0 {
        crate::logging::info(&format!(
            "[openai] Filtered {} orphaned tool result(s) to prevent API error",
            skipped_results
        ));
    }

    // Final safety pass: ensure every function_call has a matching function_call_output.
    // This prevents the OpenAI 400 "No tool output found" error if earlier logic misses a case.
    let mut output_ids: HashSet<String> = HashSet::new();
    for item in &items {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output") {
            if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                output_ids.insert(call_id.to_string());
            }
        }
    }

    let mut normalized: Vec<Value> = Vec::with_capacity(items.len());
    let mut extra_injected = 0;
    for item in items {
        let is_call = matches!(
            item.get("type").and_then(|v| v.as_str()),
            Some("function_call") | Some("custom_tool_call")
        );
        let call_id = item
            .get("call_id")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        normalized.push(item);

        if is_call {
            if let Some(call_id) = call_id {
                if !output_ids.contains(&call_id) {
                    extra_injected += 1;
                    output_ids.insert(call_id.clone());
                    normalized.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": missing_output.clone()
                    }));
                }
            }
        }
    }

    if extra_injected > 0 {
        crate::logging::info(&format!(
            "[openai] Safety-injected {} missing tool output(s) at request build",
            extra_injected
        ));
    }

    // Final pass: ensure each function_call is immediately followed by its output.
    let mut output_map: HashMap<String, Value> = HashMap::new();
    for item in &normalized {
        if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output") {
            if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                let is_missing = item
                    .get("output")
                    .and_then(|v| v.as_str())
                    .map(|v| v == missing_output)
                    .unwrap_or(false);
                match output_map.get(call_id) {
                    Some(existing) => {
                        let existing_missing = existing
                            .get("output")
                            .and_then(|v| v.as_str())
                            .map(|v| v == missing_output)
                            .unwrap_or(false);
                        if existing_missing && !is_missing {
                            output_map.insert(call_id.to_string(), item.clone());
                        }
                    }
                    None => {
                        output_map.insert(call_id.to_string(), item.clone());
                    }
                }
            }
        }
    }

    let mut ordered: Vec<Value> = Vec::with_capacity(normalized.len());
    let mut used_outputs: HashSet<String> = HashSet::new();
    let mut injected_ordered = 0usize;
    let mut dropped_orphans = 0usize;

    for item in normalized {
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let is_call = matches!(kind, "function_call" | "custom_tool_call");
        if is_call {
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string());
            ordered.push(item);
            if let Some(call_id) = call_id {
                if let Some(output_item) = output_map.get(&call_id) {
                    ordered.push(output_item.clone());
                    used_outputs.insert(call_id);
                } else {
                    injected_ordered += 1;
                    ordered.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": missing_output.clone()
                    }));
                    used_outputs.insert(call_id);
                }
            }
            continue;
        }

        if kind == "function_call_output" {
            if let Some(call_id) = item.get("call_id").and_then(|v| v.as_str()) {
                if used_outputs.contains(call_id) {
                    dropped_orphans += 1;
                    continue;
                }
            }
            dropped_orphans += 1;
            continue;
        }

        ordered.push(item);
    }

    if injected_ordered > 0 {
        crate::logging::info(&format!(
            "[openai] Inserted {} tool output(s) to enforce call ordering",
            injected_ordered
        ));
    }
    if dropped_orphans > 0 {
        crate::logging::info(&format!(
            "[openai] Dropped {} orphaned tool output(s) during re-ordering",
            dropped_orphans
        ));
    }

    ordered
}

#[derive(Deserialize, Debug)]
struct ResponseSseEvent {
    #[serde(rename = "type")]
    kind: String,
    item: Option<Value>,
    delta: Option<String>,
    response: Option<Value>,
    error: Option<Value>,
}

fn parse_openai_response_event(
    data: &str,
    saw_text_delta: &mut bool,
    pending: &mut VecDeque<StreamEvent>,
) -> Option<StreamEvent> {
    if data == "[DONE]" {
        return Some(StreamEvent::MessageEnd { stop_reason: None });
    }

    if is_websocket_fallback_notice(data) {
        crate::logging::warn(&format!("OpenAI stream transport notice: {}", data.trim()));
        return None;
    }

    if data
        .to_lowercase()
        .contains("stream disconnected before completion")
    {
        return Some(StreamEvent::Error {
            message: data.to_string(),
            retry_after_secs: None,
        });
    }

    let event: ResponseSseEvent = match serde_json::from_str(data) {
        Ok(parsed) => parsed,
        Err(_) => return None,
    };

    match event.kind.as_str() {
        "response.output_text.delta" => {
            if let Some(delta) = event.delta {
                *saw_text_delta = true;
                return Some(StreamEvent::TextDelta(delta));
            }
        }
        "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
            if let Some(delta) = event.delta {
                return Some(StreamEvent::ThinkingDelta(delta));
            }
        }
        "response.reasoning.done" | "response.output_item.added" => {
            if let Some(item) = &event.item {
                if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                    return Some(StreamEvent::ThinkingStart);
                }
            }
        }
        "response.output_item.done" => {
            if let Some(item) = event.item {
                if let Some(event) = handle_openai_output_item(item, saw_text_delta, pending) {
                    return Some(event);
                }
            }
        }
        "response.completed" => {
            if let Some(response) = event.response {
                if let Some(usage_event) = extract_usage_from_response(&response) {
                    pending.push_back(usage_event);
                }
            }
            pending.push_back(StreamEvent::MessageEnd { stop_reason: None });
            return pending.pop_front();
        }
        "response.failed" | "response.error" | "error" => {
            crate::logging::warn(&format!(
                "OpenAI stream error event (type={}): response={:?}, error={:?}",
                event.kind, event.response, event.error
            ));
            let (message, retry_after_secs) =
                extract_error_with_retry(&event.response, &event.error);
            return Some(StreamEvent::Error {
                message,
                retry_after_secs,
            });
        }
        _ => {}
    }

    None
}

fn handle_openai_output_item(
    item: Value,
    saw_text_delta: &mut bool,
    pending: &mut VecDeque<StreamEvent>,
) -> Option<StreamEvent> {
    let item_type = item.get("type")?.as_str()?;
    match item_type {
        "function_call" | "custom_tool_call" => {
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let arguments = item
                .get("arguments")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("input").and_then(|v| v.as_str()))
                .unwrap_or("{}");

            pending.push_back(StreamEvent::ToolUseStart {
                id: call_id.clone(),
                name,
            });
            pending.push_back(StreamEvent::ToolInputDelta(arguments.to_string()));
            pending.push_back(StreamEvent::ToolUseEnd);
            return pending.pop_front();
        }
        "message" => {
            if *saw_text_delta {
                return None;
            }
            let mut text = String::new();
            if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                for entry in content {
                    let entry_type = entry.get("type").and_then(|v| v.as_str());
                    if matches!(entry_type, Some("output_text") | Some("text")) {
                        if let Some(t) = entry.get("text").and_then(|v| v.as_str()) {
                            text.push_str(t);
                        }
                    }
                }
            }
            if !text.is_empty() {
                return Some(StreamEvent::TextDelta(text));
            }
        }
        "reasoning" => {
            if let Some(summary_arr) = item.get("summary").and_then(|v| v.as_array()) {
                let mut summary_text = String::new();
                for summary_item in summary_arr {
                    if summary_item.get("type").and_then(|v| v.as_str()) == Some("summary_text") {
                        if let Some(text) = summary_item.get("text").and_then(|v| v.as_str()) {
                            if !summary_text.is_empty() {
                                summary_text.push('\n');
                            }
                            summary_text.push_str(text);
                        }
                    }
                }
                if !summary_text.is_empty() {
                    pending.push_back(StreamEvent::ThinkingStart);
                    pending.push_back(StreamEvent::ThinkingDelta(summary_text));
                    pending.push_back(StreamEvent::ThinkingEnd);
                    return pending.pop_front();
                }
            }
        }
        _ => {}
    }

    None
}

struct OpenAIResponsesStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    pending: VecDeque<StreamEvent>,
    saw_text_delta: bool,
}

impl OpenAIResponsesStream {
    fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: VecDeque::new(),
            saw_text_delta: false,
        }
    }

    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }

        while let Some(pos) = self.buffer.find("\n\n") {
            let event_str = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            let mut data_lines = Vec::new();
            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    data_lines.push(data);
                }
            }

            if data_lines.is_empty() {
                continue;
            }

            let data = data_lines.join("\n");
            if let Some(event) =
                parse_openai_response_event(&data, &mut self.saw_text_delta, &mut self.pending)
            {
                return Some(event);
            }
        }

        None
    }

    fn handle_output_item(&mut self, item: Value) -> Option<StreamEvent> {
        handle_openai_output_item(item, &mut self.saw_text_delta, &mut self.pending)
    }
}

fn extract_cached_input_tokens(usage: &Value) -> Option<u64> {
    usage
        .get("input_tokens_details")
        .or_else(|| usage.get("prompt_tokens_details"))
        .and_then(|details| details.get("cached_tokens"))
        .and_then(|v| v.as_u64())
}

fn extract_usage_from_response(response: &Value) -> Option<StreamEvent> {
    let usage = response.get("usage")?;
    let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64());
    let output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64());
    let cache_read_input_tokens = extract_cached_input_tokens(usage);
    if input_tokens.is_some() || output_tokens.is_some() || cache_read_input_tokens.is_some() {
        Some(StreamEvent::TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens: None,
        })
    } else {
        None
    }
}

impl Stream for OpenAIResponsesStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.parse_next_event() {
                return Poll::Ready(Some(Ok(event)));
            }

            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        self.buffer.push_str(text);
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("Stream error: {}", e))));
                }
                Poll::Ready(None) => {
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>, // Not used by OpenAI provider
    ) -> Result<EventStream> {
        let input = build_responses_input(messages);
        let api_tools = build_tools(tools);
        let model_id = self.model_id().await;
        let (instructions, is_chatgpt_mode) = {
            let credentials = self.credentials.read().await;
            let is_chatgpt = Self::is_chatgpt_mode(&credentials);
            let instructions = if is_chatgpt {
                CHATGPT_INSTRUCTIONS.to_string()
            } else {
                system.to_string()
            };
            (instructions, is_chatgpt)
        };

        let mut request = serde_json::json!({
            "model": model_id,
            "instructions": instructions,
            "input": input,
            "tools": api_tools,
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
        });

        if let Some(ref effort) = self.reasoning_effort {
            request["reasoning"] = serde_json::json!({ "effort": effort });
        }

        if !is_chatgpt_mode {
            if let Some(key) = self.prompt_cache_key.as_ref() {
                request["prompt_cache_key"] = serde_json::json!(key);
            }
            if let Some(retention) = self.prompt_cache_retention.as_ref() {
                request["prompt_cache_retention"] = serde_json::json!(retention);
            }
        }

        // Create channel for streaming events
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        // Clone what we need for the async task
        let credentials = Arc::clone(&self.credentials);
        let transport_mode = self.transport_mode;
        let websocket_disabled = Arc::clone(&self.websocket_disabled);
        let model_for_transport = model_id.clone();
        let client = self.client.clone();

        // Spawn task to handle streaming with retry logic
        tokio::spawn(async move {
            let mut last_error = None;

            for attempt in 0..MAX_RETRIES {
                if attempt > 0 {
                    // Exponential backoff: 1s, 2s, 4s
                    let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    crate::logging::info(&format!(
                        "Retrying OpenAI API request (attempt {}/{})",
                        attempt + 1,
                        MAX_RETRIES
                    ));
                }

                let transport = if websocket_disabled.load(Ordering::Acquire) {
                    OpenAITransport::HTTPS
                } else {
                    match transport_mode {
                        OpenAITransportMode::HTTPS => OpenAITransport::HTTPS,
                        OpenAITransportMode::WebSocket => OpenAITransport::WebSocket,
                        OpenAITransportMode::Auto => {
                            if Self::should_prefer_websocket(&model_for_transport) {
                                OpenAITransport::WebSocket
                            } else {
                                OpenAITransport::HTTPS
                            }
                        }
                    }
                };

                let transport_label = transport.as_str();
                crate::logging::info(&format!(
                    "OpenAI stream attempt {}/{} using transport '{}'; model='{}'; mode='{}'",
                    attempt + 1,
                    MAX_RETRIES,
                    transport_label,
                    model_for_transport,
                    transport_mode.as_str()
                ));

                let use_websocket = matches!(transport, OpenAITransport::WebSocket);
                let result = if use_websocket {
                    stream_response_websocket(Arc::clone(&credentials), request.clone(), tx.clone())
                        .await
                } else {
                    stream_response(
                        client.clone(),
                        Arc::clone(&credentials),
                        request.clone(),
                        tx.clone(),
                    )
                    .await
                };

                match result {
                    Ok(()) => return, // Success
                    Err(OpenAIStreamFailure::FallbackToHttps(error)) => {
                        crate::logging::info(
                            "WebSocket fallback detected. Retrying using HTTPS transport for this session.",
                        );
                        websocket_disabled.store(true, Ordering::SeqCst);
                        last_error = Some(error);
                        continue;
                    }
                    Err(OpenAIStreamFailure::Other(error)) => {
                        let error_str = error.to_string().to_lowercase();
                        if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                            crate::logging::info(&format!(
                                "Transient error, will retry: {}",
                                error
                            ));
                            last_error = Some(error);
                            continue;
                        }
                        let _ = tx.send(Err(error)).await;
                        return;
                    }
                }
            }

            // All retries exhausted
            if let Some(e) = last_error {
                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Failed after {} retries: {}",
                        MAX_RETRIES,
                        e
                    )))
                    .await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> String {
        // Use try_read to avoid blocking - fall back to default if locked
        self.model
            .try_read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        if !AVAILABLE_MODELS.contains(&model) {
            anyhow::bail!(
                "Unsupported OpenAI model '{}'. Only supported model is '{}'.",
                model,
                DEFAULT_MODEL
            );
        }
        if let Ok(mut current) = self.model.try_write() {
            *current = model.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn reasoning_effort(&self) -> Option<String> {
        self.reasoning_effort.clone()
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        let model = self.model();
        crate::provider::context_limit_for_model(&model)
            .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let model = self.model();
        Arc::new(OpenAIProvider {
            client: self.client.clone(),
            credentials: Arc::clone(&self.credentials),
            model: Arc::new(RwLock::new(model)),
            prompt_cache_key: self.prompt_cache_key.clone(),
            prompt_cache_retention: self.prompt_cache_retention.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            transport_mode: self.transport_mode,
            websocket_disabled: Arc::clone(&self.websocket_disabled),
        })
    }
}

async fn openai_access_token(
    credentials: &Arc<RwLock<CodexCredentials>>,
) -> anyhow::Result<String> {
    let (access_token, refresh_token, needs_refresh) = {
        let tokens = credentials.read().await;
        if tokens.access_token.is_empty() {
            anyhow::bail!("OpenAI access token is empty");
        }

        let should_refresh = if let Some(expires_at) = tokens.expires_at {
            expires_at < chrono::Utc::now().timestamp_millis() + 300_000
                && !tokens.refresh_token.is_empty()
        } else {
            false
        };

        (
            tokens.access_token.clone(),
            tokens.refresh_token.clone(),
            should_refresh,
        )
    };

    if !needs_refresh {
        return Ok(access_token);
    }

    if refresh_token.is_empty() {
        return Ok(access_token);
    }

    let refreshed = oauth::refresh_openai_tokens(&refresh_token).await?;
    let mut tokens = credentials.write().await;
    let account_id = tokens.account_id.clone();
    let id_token = refreshed
        .id_token
        .clone()
        .or_else(|| tokens.id_token.clone());
    let new_access_token = refreshed.access_token.clone();

    *tokens = CodexCredentials {
        access_token: new_access_token.clone(),
        refresh_token: refreshed.refresh_token,
        id_token,
        account_id,
        expires_at: Some(refreshed.expires_at),
    };

    Ok(new_access_token)
}

/// Stream the response from OpenAI API
async fn stream_response(
    client: Client,
    credentials: Arc<RwLock<CodexCredentials>>,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<(), OpenAIStreamFailure> {
    let access_token = openai_access_token(&credentials).await?;
    let mut creds = credentials.read().await;
    let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    let url = OpenAIProvider::responses_url(&creds);
    let account_id = creds.account_id.clone();
    drop(creds);

    let mut builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json");

    if is_chatgpt_mode {
        builder = builder.header("originator", ORIGINATOR);
        if let Some(account_id) = account_id.as_ref() {
            builder = builder.header("chatgpt-account-id", account_id);
        }
    }

    let response = builder
        .json(&request)
        .send()
        .await
        .context("Failed to send request to OpenAI API")
        .map_err(OpenAIStreamFailure::Other)?;

    if !response.status().is_success() {
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let body = response.text().await.unwrap_or_default();

        // Check if we need to refresh token
        if should_refresh_token(status, &body) {
            // Token refresh needed - this is a retryable error
            return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                "Token refresh needed: {}",
                body
            )));
        }

        // For rate limits, include retry info in the error
        let msg = if status == StatusCode::TOO_MANY_REQUESTS {
            let wait_info = retry_after
                .map(|s| format!(" (retry after {}s)", s))
                .unwrap_or_default();
            format!("Rate limited{}: {}", wait_info, body)
        } else {
            format!("OpenAI API error {}: {}", status, body)
        };
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!("{}", msg)));
    }

    // Stream the response
    let mut stream = OpenAIResponsesStream::new(response.bytes_stream());

    use futures::StreamExt;
    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => {
                if let StreamEvent::Error { message, .. } = &event {
                    if is_retryable_error(&message.to_lowercase()) {
                        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                            "Stream error: {}",
                            message
                        )));
                    }
                }
                if tx.send(Ok(event)).await.is_err() {
                    // Receiver dropped, stop streaming
                    return Ok(());
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e)).await;
                return Ok(());
            }
        }
    }

    Ok(())
}

fn is_ws_upgrade_required(err: &WsError) -> bool {
    match err {
        WsError::Http(response) => response.status() == WEBSOCKET_UPGRADE_REQUIRED_ERROR,
        _ => false,
    }
}

/// Stream the response from OpenAI API using websockets
async fn stream_response_websocket(
    credentials: Arc<RwLock<CodexCredentials>>,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<(), OpenAIStreamFailure> {
    use std::time::{Duration, Instant};

    let access_token = openai_access_token(&credentials).await?;
    let creds = credentials.read().await;
    let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    let ws_url = OpenAIProvider::responses_ws_url(&creds);
    let mut ws_request = ws_url.into_client_request().map_err(|err| {
        OpenAIStreamFailure::Other(anyhow::anyhow!(
            "Failed to build websocket request: {}",
            err
        ))
    })?;

    let auth_header =
        HeaderValue::from_str(&format!("Bearer {}", access_token)).map_err(|err| {
            OpenAIStreamFailure::Other(anyhow::anyhow!("Invalid Authorization header: {}", err))
        })?;
    ws_request
        .headers_mut()
        .insert("Authorization", auth_header);
    ws_request
        .headers_mut()
        .insert("Content-Type", HeaderValue::from_static("application/json"));

    if is_chatgpt_mode {
        ws_request
            .headers_mut()
            .insert("originator", HeaderValue::from_static(ORIGINATOR));
        if let Some(account_id) = creds.account_id.as_ref() {
            let account_header = HeaderValue::from_str(account_id).map_err(|err| {
                OpenAIStreamFailure::Other(anyhow::anyhow!(
                    "Invalid chatgpt-account-id header: {}",
                    err
                ))
            })?;
            ws_request
                .headers_mut()
                .insert("chatgpt-account-id", account_header);
        }
    }
    drop(creds);

    let (mut ws_stream, _response) = match connect_async(ws_request).await {
        Ok((stream, response)) => (stream, response),
        Err(err) if is_ws_upgrade_required(&err) => {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "Falling back from websockets to HTTPS transport"
            )));
        }
        Err(err) => {
            return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                "Failed to connect websocket stream: {}",
                err
            )));
        }
    };

    let mut request_event = request;
    if !request_event.is_object() {
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
            "Invalid websocket request payload shape; expected an object"
        )));
    }
    request_event
        .as_object_mut()
        .expect("request_event is object")
        .insert(
            "type".to_string(),
            serde_json::Value::String("response.create".to_string()),
        );

    let request_text = serde_json::to_string(&request_event).map_err(|err| {
        OpenAIStreamFailure::Other(anyhow::anyhow!(
            "Failed to serialize OpenAI websocket request: {}",
            err
        ))
    })?;
    ws_stream
        .send(WsMessage::Text(request_text))
        .await
        .map_err(|err| OpenAIStreamFailure::Other(anyhow::anyhow!(err)))?;

    use futures::StreamExt;
    let mut saw_text_delta = false;
    let mut saw_response_completed = false;
    let mut saw_any_message = false;
    let ws_started_at = Instant::now();
    let mut last_non_keepalive_message_at = Instant::now();
    let mut pending: VecDeque<StreamEvent> = VecDeque::new();

    loop {
        if !saw_response_completed
            && ws_started_at.elapsed() >= Duration::from_secs(WEBSOCKET_COMPLETION_TIMEOUT_SECS)
        {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream did not complete within {}s",
                WEBSOCKET_COMPLETION_TIMEOUT_SECS
            )));
        }

        if saw_any_message
            && last_non_keepalive_message_at.elapsed()
                >= Duration::from_secs(WEBSOCKET_IDLE_TIMEOUT_SECS)
        {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream stalled without non-keepalive events for {}s",
                WEBSOCKET_IDLE_TIMEOUT_SECS
            )));
        }

        let timeout_secs = if saw_any_message {
            WEBSOCKET_IDLE_TIMEOUT_SECS
        } else {
            WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
        };
        let next_item = tokio::time::timeout(Duration::from_secs(timeout_secs), ws_stream.next())
            .await
            .map_err(|_| {
                OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                    "WebSocket stream timed out waiting for {} event ({}s)",
                    if saw_any_message { "next" } else { "first" },
                    timeout_secs
                ))
            })?;

        let Some(result) = next_item else {
            break;
        };
        saw_any_message = true;

        match result {
            Ok(message) => match message {
                WsMessage::Text(text) => {
                    let text = text.to_string();
                    if is_websocket_fallback_notice(&text) {
                        return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                            "{} reported by websocket stream",
                            WEBSOCKET_FALLBACK_NOTICE
                        )));
                    }

                    let mut saw_progress_event = false;
                    if let Some(event) =
                        parse_openai_response_event(&text, &mut saw_text_delta, &mut pending)
                    {
                        saw_progress_event = true;
                        if matches!(event, StreamEvent::MessageEnd { .. }) {
                            saw_response_completed = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if is_retryable_error(&message.to_lowercase()) {
                                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                                    "Stream error: {}",
                                    message
                                )));
                            }
                        }
                        if tx.send(Ok(event)).await.is_err() {
                            return Ok(());
                        }
                    }
                    while let Some(event) = pending.pop_front() {
                        saw_progress_event = true;
                        if let StreamEvent::Error { message, .. } = &event {
                            if is_retryable_error(&message.to_lowercase()) {
                                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                                    "Stream error: {}",
                                    message
                                )));
                            }
                        }
                        if matches!(event, StreamEvent::MessageEnd { .. }) {
                            saw_response_completed = true;
                        }
                        if tx.send(Ok(event)).await.is_err() {
                            return Ok(());
                        }
                    }
                    if saw_progress_event {
                        last_non_keepalive_message_at = Instant::now();
                    }
                }
                WsMessage::Ping(payload) => {
                    let _ = ws_stream.send(WsMessage::Pong(payload)).await;
                }
                WsMessage::Close(_) => {
                    if saw_response_completed {
                        return Ok(());
                    }
                    return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                        "WebSocket stream closed before response.completed"
                    )));
                }
                WsMessage::Binary(_) => {
                    return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                        "Unexpected binary websocket event"
                    )));
                }
                WsMessage::Pong(_) => {}
                _ => {}
            },
            Err(err) => {
                return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
                    "Stream error: {}",
                    err
                )));
            }
        }
    }

    Ok(())
}

fn should_refresh_token(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::UNAUTHORIZED {
        return true;
    }
    if status == StatusCode::FORBIDDEN {
        let lower = body.to_lowercase();
        return lower.contains("token")
            || lower.contains("expired")
            || lower.contains("unauthorized");
    }
    false
}

fn extract_error_with_retry(
    response: &Option<Value>,
    top_level_error: &Option<Value>,
) -> (String, Option<u64>) {
    // For "response.failed" events, the error is nested: response.error.message
    // For "error"/"response.error" events, the error is top-level: error.message
    let error = response
        .as_ref()
        .and_then(|r| r.get("error"))
        .or(top_level_error.as_ref());

    let error = match error {
        Some(e) => e,
        None => {
            // Last resort: check if response itself has a status_message or message
            if let Some(resp) = response.as_ref() {
                if let Some(msg) = resp
                    .get("status_message")
                    .or_else(|| resp.get("message"))
                    .and_then(|v| v.as_str())
                {
                    return (msg.to_string(), None);
                }
            }
            return (
                "OpenAI response stream error (no error details)".to_string(),
                None,
            );
        }
    };

    let message = error
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("OpenAI response stream error (unknown)")
        .to_string();

    // Try to extract retry_after from error object or response metadata
    let retry_after = error
        .get("retry_after")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            response
                .as_ref()
                .and_then(|r| r.get("retry_after"))
                .and_then(|v| v.as_u64())
        });

    (message, retry_after)
}

/// Check if an error is transient and should be retried
fn is_retryable_error(error_str: &str) -> bool {
    // Network/connection errors
    error_str.contains("connection reset")
        || error_str.contains("connection closed")
        || error_str.contains("connection refused")
        || error_str.contains("broken pipe")
        || error_str.contains("timed out")
        || error_str.contains("timeout")
        || error_str.contains("failed to send request to openai api")
        // Stream/decode errors
        || error_str.contains("error decoding")
        || error_str.contains("error reading")
        || error_str.contains("unexpected eof")
        || error_str.contains("incomplete message")
        || error_str.contains("stream disconnected before completion")
        || error_str.contains("falling back from websockets to https transport")
        // Server errors (5xx)
        || error_str.contains("500 internal server error")
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
        // API-level server errors
        || error_str.contains("api_error")
        || error_str.contains("internal server error")
}

fn is_websocket_fallback_notice(data: &str) -> bool {
    data.to_lowercase().contains(WEBSOCKET_FALLBACK_NOTICE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::codex::CodexCredentials;

    #[test]
    fn test_openai_supports_codex_models() {
        let creds = CodexCredentials {
            access_token: "test".to_string(),
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            expires_at: None,
        };

        let provider = OpenAIProvider::new(creds);
        assert!(provider.available_models().contains(&"gpt-5.2-codex"));
        assert!(provider.available_models().contains(&"codex-mini-latest"));
        assert!(provider.available_models().contains(&"gpt-5.1-codex-mini"));

        provider.set_model("gpt-5.1-codex").unwrap();
        assert_eq!(provider.model(), "gpt-5.1-codex");

        provider.set_model("gpt-5.1-codex-mini").unwrap();
        assert_eq!(provider.model(), "gpt-5.1-codex-mini");
    }

    #[test]
    fn test_build_responses_input_injects_missing_tool_output() {
        let expected_missing = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
        let messages = vec![
            ChatMessage {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "hi".to_string(),
                    cache_control: None,
                }],
                timestamp: None,
            },
            ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                }],
                timestamp: None,
            },
        ];

        let items = build_responses_input(&messages);
        let mut saw_call = false;
        let mut saw_output = false;

        for item in &items {
            let item_type = item.get("type").and_then(|v| v.as_str());
            match item_type {
                Some("function_call") => {
                    if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") {
                        saw_call = true;
                    }
                }
                Some("function_call_output") => {
                    if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") {
                        let output = item.get("output").and_then(|v| v.as_str());
                        assert_eq!(output, Some(expected_missing.as_str()));
                        saw_output = true;
                    }
                }
                _ => {}
            }
        }

        assert!(saw_call);
        assert!(saw_output);
    }

    #[test]
    fn test_build_responses_input_preserves_tool_output() {
        let messages = vec![
            ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                }],
                timestamp: None,
            },
            ChatMessage::tool_result("call_1", "ok", false),
        ];

        let items = build_responses_input(&messages);
        let mut outputs = Vec::new();

        for item in &items {
            if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_1")
            {
                if let Some(output) = item.get("output").and_then(|v| v.as_str()) {
                    outputs.push(output.to_string());
                }
            }
        }

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0], "ok");
    }

    #[test]
    fn test_build_responses_input_reorders_early_tool_output() {
        let messages = vec![
            ChatMessage::tool_result("call_1", "ok", false),
            ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                }],
                timestamp: None,
            },
        ];

        let items = build_responses_input(&messages);
        let mut call_pos = None;
        let mut output_pos = None;
        let mut outputs = Vec::new();

        for (idx, item) in items.iter().enumerate() {
            let item_type = item.get("type").and_then(|v| v.as_str());
            match item_type {
                Some("function_call") => {
                    if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") {
                        call_pos = Some(idx);
                    }
                }
                Some("function_call_output") => {
                    if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") {
                        output_pos = Some(idx);
                        if let Some(output) = item.get("output").and_then(|v| v.as_str()) {
                            outputs.push(output.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        assert!(call_pos.is_some());
        assert!(output_pos.is_some());
        assert!(output_pos.unwrap() > call_pos.unwrap());
        assert_eq!(outputs, vec!["ok".to_string()]);
    }

    #[test]
    fn test_build_responses_input_injects_only_missing_outputs() {
        let expected_missing = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
        let messages = vec![
            ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_a".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "pwd"}),
                }],
                timestamp: None,
            },
            ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_b".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "whoami"}),
                }],
                timestamp: None,
            },
            ChatMessage::tool_result("call_b", "done", false),
        ];

        let items = build_responses_input(&messages);
        let mut output_a = None;
        let mut output_b = None;

        for item in &items {
            if item.get("type").and_then(|v| v.as_str()) == Some("function_call_output") {
                match item.get("call_id").and_then(|v| v.as_str()) {
                    Some("call_a") => {
                        output_a = item
                            .get("output")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string());
                    }
                    Some("call_b") => {
                        output_b = item
                            .get("output")
                            .and_then(|v| v.as_str())
                            .map(|v| v.to_string());
                    }
                    _ => {}
                }
            }
        }

        assert_eq!(output_a.as_deref(), Some(expected_missing.as_str()));
        assert_eq!(output_b.as_deref(), Some("done"));
    }

    #[test]
    fn test_openai_retryable_error_patterns() {
        assert!(is_retryable_error(
            "stream disconnected before completion: transport error"
        ));
        assert!(is_retryable_error(
            "falling back from websockets to https transport. stream disconnected before completion"
        ));
    }
}
