use super::{EventStream, Provider};
use crate::auth::codex::CodexCredentials;
use crate::auth::oauth;
use crate::message::{
    ContentBlock, Message as ChatMessage, Role, StreamEvent, TOOL_OUTPUT_MISSING_TEXT,
    ToolDefinition,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{SinkExt, Stream, StreamExt as FuturesStreamExt};
use reqwest::header::HeaderValue;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock as StdRwLock};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const CHATGPT_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const RESPONSES_PATH: &str = "responses";
const DEFAULT_MODEL: &str = "gpt-5.4";
const ORIGINATOR: &str = "codex_cli_rs";
const CHATGPT_INSTRUCTIONS: &str = include_str!("../prompts/openai_chatgpt.md");
const SELFDEV_SECTION_HEADER: &str = "# Self-Development Mode";

/// Maximum number of retries for transient errors
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const RETRY_BASE_DELAY_MS: u64 = 1000;
const WEBSOCKET_UPGRADE_REQUIRED_ERROR: StatusCode = StatusCode::UPGRADE_REQUIRED;
const WEBSOCKET_FALLBACK_NOTICE: &str = "falling back from websockets to https transport";
const WEBSOCKET_CONNECT_TIMEOUT_SECS: u64 = 8;
const WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS: u64 = 8;
const WEBSOCKET_COMPLETION_TIMEOUT_SECS: u64 = 300;
/// Maximum age of a persistent WebSocket connection before forcing reconnect
const WEBSOCKET_PERSISTENT_MAX_AGE_SECS: u64 = 3000; // 50 min (server limit is 60 min)
const WEBSOCKET_MODEL_COOLDOWN_BASE_SECS: u64 = 600;
const WEBSOCKET_MODEL_COOLDOWN_MAX_SECS: u64 = 3600;
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 32_768;
static FALLBACK_TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(1);
static RECOVERED_TEXT_WRAPPED_TOOL_CALLS: AtomicU64 = AtomicU64::new(0);
static NORMALIZED_NULL_TOOL_ARGUMENTS: AtomicU64 = AtomicU64::new(0);
static REWRITTEN_ORPHAN_TOOL_OUTPUTS: AtomicU64 = AtomicU64::new(0);
static WEBSOCKET_COOLDOWNS: LazyLock<Arc<RwLock<HashMap<String, Instant>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));
static WEBSOCKET_FAILURE_STREAKS: LazyLock<Arc<RwLock<HashMap<String, u32>>>> =
    LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));

/// Available OpenAI/Codex models
const AVAILABLE_MODELS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.4-pro",
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

/// Persistent WebSocket connection state for incremental continuation.
/// Keeps the connection alive across turns so we can use `previous_response_id`
/// to send only new items instead of the full conversation each turn.
struct PersistentWsState {
    ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    last_response_id: String,
    connected_at: Instant,
    /// Number of messages sent in this conversation chain
    message_count: usize,
    /// Number of items we sent in the last full request (for detecting conversation changes)
    last_input_item_count: usize,
}

pub struct OpenAIProvider {
    client: Client,
    credentials: Arc<RwLock<CodexCredentials>>,
    model: Arc<RwLock<String>>,
    prompt_cache_key: Option<String>,
    prompt_cache_retention: Option<String>,
    max_output_tokens: Option<u32>,
    reasoning_effort: Arc<RwLock<Option<String>>>,
    service_tier: Arc<StdRwLock<Option<String>>>,
    transport_mode: Arc<RwLock<OpenAITransportMode>>,
    websocket_cooldowns: Arc<RwLock<HashMap<String, Instant>>>,
    websocket_failure_streaks: Arc<RwLock<HashMap<String, u32>>>,
    /// Persistent WebSocket connection for incremental continuation
    persistent_ws: Arc<Mutex<Option<PersistentWsState>>>,
}

impl OpenAIProvider {
    pub fn new(credentials: CodexCredentials) -> Self {
        // Check for model override from environment
        let mut model =
            std::env::var("JCODE_OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        if !crate::provider::known_openai_model_ids()
            .iter()
            .any(|known| known == &model)
        {
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
        let max_output_tokens = Self::load_max_output_tokens();
        let reasoning_effort = crate::config::config()
            .provider
            .openai_reasoning_effort
            .as_deref()
            .and_then(Self::normalize_reasoning_effort);
        let service_tier = Self::load_service_tier(
            crate::config::config()
                .provider
                .openai_service_tier
                .as_deref(),
        );
        let transport_mode = OpenAITransportMode::from_config(
            crate::config::config().provider.openai_transport.as_deref(),
        );

        Self {
            client: crate::provider::shared_http_client(),
            credentials: Arc::new(RwLock::new(credentials)),
            model: Arc::new(RwLock::new(model)),
            prompt_cache_key,
            prompt_cache_retention,
            max_output_tokens,
            reasoning_effort: Arc::new(RwLock::new(reasoning_effort)),
            service_tier: Arc::new(StdRwLock::new(service_tier)),
            transport_mode: Arc::new(RwLock::new(transport_mode)),
            websocket_cooldowns: Arc::clone(&WEBSOCKET_COOLDOWNS),
            websocket_failure_streaks: Arc::clone(&WEBSOCKET_FAILURE_STREAKS),
            persistent_ws: Arc::new(Mutex::new(None)),
        }
    }

    fn is_chatgpt_mode(credentials: &CodexCredentials) -> bool {
        !credentials.refresh_token.is_empty() || credentials.id_token.is_some()
    }

    fn chatgpt_instructions_with_selfdev(system: &str) -> String {
        if let Some(selfdev_section) = extract_selfdev_section(system) {
            format!("{}\n\n{}", CHATGPT_INSTRUCTIONS.trim_end(), selfdev_section)
        } else {
            CHATGPT_INSTRUCTIONS.to_string()
        }
    }

    fn should_prefer_websocket(model: &str) -> bool {
        !model.trim().is_empty()
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

    fn parse_max_output_tokens(raw: Option<&str>) -> Option<u32> {
        let raw = match raw {
            Some(value) => value.trim(),
            None => return Some(DEFAULT_MAX_OUTPUT_TOKENS),
        };
        if raw.is_empty() {
            return Some(DEFAULT_MAX_OUTPUT_TOKENS);
        }
        match raw.parse::<u32>() {
            Ok(0) => None,
            Ok(value) => Some(value),
            Err(_) => {
                crate::logging::warn(&format!(
                    "Invalid JCODE_OPENAI_MAX_OUTPUT_TOKENS='{}'; using default {}",
                    raw, DEFAULT_MAX_OUTPUT_TOKENS
                ));
                Some(DEFAULT_MAX_OUTPUT_TOKENS)
            }
        }
    }

    fn normalize_service_tier(raw: &str) -> Result<Option<String>> {
        let value = raw.trim().to_ascii_lowercase();
        if value.is_empty() {
            return Ok(None);
        }

        match value.as_str() {
            "fast" | "priority" => Ok(Some("priority".to_string())),
            "flex" => Ok(Some("flex".to_string())),
            "default" | "auto" | "none" | "off" => Ok(None),
            other => anyhow::bail!(
                "Unsupported OpenAI service tier '{}'; expected priority|fast|flex|default|off",
                other
            ),
        }
    }

    fn load_service_tier(raw: Option<&str>) -> Option<String> {
        let Some(raw) = raw else {
            return None;
        };
        match Self::normalize_service_tier(raw) {
            Ok(value) => value,
            Err(err) => {
                crate::logging::warn(&format!(
                    "{}; ignoring configured service tier override",
                    err
                ));
                None
            }
        }
    }

    fn load_max_output_tokens() -> Option<u32> {
        let raw = std::env::var("JCODE_OPENAI_MAX_OUTPUT_TOKENS").ok();
        let parsed = Self::parse_max_output_tokens(raw.as_deref());
        if raw.is_some() {
            match parsed {
                Some(value) => crate::logging::info(&format!(
                    "OpenAI max_output_tokens configured to {}",
                    value
                )),
                None => crate::logging::info(
                    "OpenAI max_output_tokens disabled (JCODE_OPENAI_MAX_OUTPUT_TOKENS=0)",
                ),
            }
        }
        parsed
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

    fn build_response_request(
        model_id: &str,
        instructions: String,
        input: &[Value],
        api_tools: &[Value],
        is_chatgpt_mode: bool,
        max_output_tokens: Option<u32>,
        reasoning_effort: Option<&str>,
        service_tier: Option<&str>,
        prompt_cache_key: Option<&str>,
        prompt_cache_retention: Option<&str>,
    ) -> Value {
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

        if !is_chatgpt_mode {
            if let Some(max_output_tokens) = max_output_tokens {
                request["max_output_tokens"] = serde_json::json!(max_output_tokens);
            }
        }

        if let Some(effort) = reasoning_effort {
            request["reasoning"] = serde_json::json!({ "effort": effort });
        }

        if let Some(service_tier) = service_tier {
            request["service_tier"] = serde_json::json!(service_tier);
        }

        if !is_chatgpt_mode {
            if let Some(key) = prompt_cache_key {
                request["prompt_cache_key"] = serde_json::json!(key);
            }
            if let Some(retention) = prompt_cache_retention {
                request["prompt_cache_retention"] = serde_json::json!(retention);
            }
        }

        request
    }

    async fn model_id(&self) -> String {
        let current = self.model.read().await.clone();
        let availability = crate::provider::model_availability_for_account(&current);

        match availability.state {
            crate::provider::AccountModelAvailabilityState::Unavailable => {
                if let Some(detail) = availability.reason {
                    crate::logging::info(&format!(
                        "Model '{}' currently unavailable ({}); selecting fallback",
                        current, detail
                    ));
                }
                if let Some(fallback) = crate::provider::get_best_available_openai_model() {
                    if fallback != current {
                        crate::logging::info(&format!(
                            "Model '{}' not available for account; falling back to '{}'",
                            current, fallback
                        ));
                        let mut w = self.model.write().await;
                        *w = fallback.clone();
                        return fallback;
                    }
                }
            }
            crate::provider::AccountModelAvailabilityState::Unknown => {
                if crate::provider::should_refresh_openai_model_catalog()
                    && crate::provider::begin_openai_model_catalog_refresh()
                {
                    let creds = self.credentials.read().await;
                    let token = creds.access_token.clone();
                    drop(creds);
                    crate::provider::refresh_openai_model_catalog_in_background(
                        token,
                        "openai-request-setup",
                    );
                }
            }
            crate::provider::AccountModelAvailabilityState::Available => {}
        }

        current.strip_suffix("[1m]").unwrap_or(&current).to_string()
    }
}

fn extract_selfdev_section(system: &str) -> Option<&str> {
    let start = system.find(SELFDEV_SECTION_HEADER)?;
    let end = if let Some(rel_end) = system[start + 1..].find("\n# ") {
        start + 1 + rel_end
    } else {
        system.len()
    };
    let section = system[start..end].trim();
    if section.is_empty() {
        None
    } else {
        Some(section)
    }
}

fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let compatible_schema = openai_compatible_schema(&t.input_schema);
            let supports_strict = schema_supports_strict(&compatible_schema);
            let parameters = if supports_strict {
                strict_normalize_schema(&compatible_schema)
            } else {
                compatible_schema
            };
            serde_json::json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "strict": supports_strict,
                "parameters": parameters,
            })
        })
        .collect()
}

fn openai_compatible_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                let normalized_key = if key == "oneOf" { "anyOf" } else { key };
                out.insert(normalized_key.to_string(), openai_compatible_schema(value));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(openai_compatible_schema).collect()),
        _ => schema.clone(),
    }
}

fn schema_supports_strict(schema: &Value) -> bool {
    fn check_map(map: &serde_json::Map<String, Value>) -> bool {
        let is_object_typed = match map.get("type") {
            Some(Value::String(t)) => t == "object",
            Some(Value::Array(types)) => types.iter().any(|v| v.as_str() == Some("object")),
            _ => false,
        };
        let has_properties = map
            .get("properties")
            .and_then(|v| v.as_object())
            .map(|props| !props.is_empty())
            .unwrap_or(false);

        if is_object_typed && !has_properties {
            return false;
        }
        if is_object_typed {
            if matches!(map.get("additionalProperties"), Some(Value::Bool(true))) {
                return false;
            }
            if matches!(map.get("additionalProperties"), Some(Value::Object(_))) {
                return false;
            }
        }

        map.values().all(schema_supports_strict)
    }

    match schema {
        Value::Object(map) => check_map(map),
        Value::Array(items) => items.iter().all(schema_supports_strict),
        _ => true,
    }
}

fn strict_normalize_schema(schema: &Value) -> Value {
    fn make_schema_nullable(schema: Value) -> Value {
        match schema {
            Value::Object(mut map) => {
                if let Some(Value::String(t)) = map.get("type").cloned() {
                    if t != "null" {
                        map.insert(
                            "type".to_string(),
                            Value::Array(vec![Value::String(t), Value::String("null".to_string())]),
                        );
                    }
                    return Value::Object(map);
                }

                if let Some(Value::Array(mut types)) = map.get("type").cloned() {
                    if !types.iter().any(|v| v.as_str() == Some("null")) {
                        types.push(Value::String("null".to_string()));
                    }
                    map.insert("type".to_string(), Value::Array(types));
                    return Value::Object(map);
                }

                if let Some(Value::Array(mut any_of)) = map.get("anyOf").cloned() {
                    if !any_of.iter().any(|v| {
                        v.get("type")
                            .and_then(|t| t.as_str())
                            .map(|t| t == "null")
                            .unwrap_or(false)
                    }) {
                        any_of.push(serde_json::json!({ "type": "null" }));
                    }
                    map.insert("anyOf".to_string(), Value::Array(any_of));
                    return Value::Object(map);
                }

                serde_json::json!({
                    "anyOf": [
                        Value::Object(map),
                        { "type": "null" }
                    ]
                })
            }
            other => serde_json::json!({
                "anyOf": [
                    other,
                    { "type": "null" }
                ]
            }),
        }
    }

    fn normalize_map(map: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
        let mut out = serde_json::Map::new();
        for (key, value) in map {
            let normalized = match key.as_str() {
                "properties" | "$defs" | "definitions" | "patternProperties" => match value {
                    Value::Object(children) => {
                        let mut rewritten = serde_json::Map::new();
                        for (child_key, child_value) in children {
                            rewritten
                                .insert(child_key.clone(), strict_normalize_schema(child_value));
                        }
                        Value::Object(rewritten)
                    }
                    _ => strict_normalize_schema(value),
                },
                "items" | "contains" | "if" | "then" | "else" | "not" => {
                    strict_normalize_schema(value)
                }
                "allOf" | "anyOf" | "oneOf" | "prefixItems" => match value {
                    Value::Array(items) => {
                        Value::Array(items.iter().map(strict_normalize_schema).collect())
                    }
                    _ => strict_normalize_schema(value),
                },
                _ => strict_normalize_schema(value),
            };
            out.insert(key.clone(), normalized);
        }

        let is_object_typed = match out.get("type") {
            Some(Value::String(t)) => t == "object",
            Some(Value::Array(types)) => types.iter().any(|v| v.as_str() == Some("object")),
            _ => false,
        };

        if let Some(Value::Object(properties)) = out.get("properties") {
            let existing_required: std::collections::HashSet<String> = out
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let mut required_all: Vec<String> = properties.keys().cloned().collect();
            required_all.sort();

            if let Some(Value::Object(props_mut)) = out.get_mut("properties") {
                for (prop_name, prop_schema) in props_mut.iter_mut() {
                    if !existing_required.contains(prop_name) {
                        *prop_schema = make_schema_nullable(prop_schema.clone());
                    }
                }
            }

            out.insert(
                "required".to_string(),
                Value::Array(required_all.into_iter().map(Value::String).collect()),
            );
        }

        if is_object_typed || out.contains_key("properties") {
            out.insert("additionalProperties".to_string(), Value::Bool(false));
        }

        out
    }

    match schema {
        Value::Object(map) => Value::Object(normalize_map(map)),
        Value::Array(items) => Value::Array(items.iter().map(strict_normalize_schema).collect()),
        _ => schema.clone(),
    }
}

fn parse_text_wrapped_tool_call(text: &str) -> Option<(String, String, String, String)> {
    let marker = "to=functions.";
    let marker_idx = text.find(marker)?;
    let after_marker = &text[marker_idx + marker.len()..];

    let mut tool_name_end = 0usize;
    for (idx, ch) in after_marker.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            tool_name_end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    if tool_name_end == 0 {
        return None;
    }

    let tool_name = after_marker[..tool_name_end].to_string();
    let remaining = &after_marker[tool_name_end..];
    let mut fallback: Option<(String, String, String, String)> = None;
    for (brace_idx, ch) in remaining.char_indices() {
        if ch != '{' {
            continue;
        }
        let slice = &remaining[brace_idx..];
        let mut stream = serde_json::Deserializer::from_str(slice).into_iter::<Value>();
        let parsed = match stream.next() {
            Some(Ok(value)) => value,
            Some(Err(_)) => continue,
            None => continue,
        };
        let consumed = stream.byte_offset();
        if !parsed.is_object() {
            continue;
        }

        let prefix = text[..marker_idx].trim_end().to_string();
        let suffix = remaining[brace_idx + consumed..].trim().to_string();
        let args = serde_json::to_string(&parsed).ok()?;
        if suffix.is_empty() {
            return Some((prefix, tool_name.clone(), args, suffix));
        }
        if fallback.is_none() {
            fallback = Some((prefix, tool_name.clone(), args, suffix));
        }
    }

    fallback
}

fn stream_text_or_recovered_tool_call(
    text: &str,
    pending: &mut VecDeque<StreamEvent>,
) -> Option<StreamEvent> {
    if text.is_empty() {
        return None;
    }

    if let Some((prefix, tool_name, arguments, suffix)) = parse_text_wrapped_tool_call(text) {
        let total = RECOVERED_TEXT_WRAPPED_TOOL_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
        crate::logging::warn(&format!(
            "[openai] Recovered text-wrapped tool call for '{}' (total={})",
            tool_name, total
        ));
        let suffix = sanitize_recovered_tool_suffix(&suffix);
        if !prefix.is_empty() {
            pending.push_back(StreamEvent::TextDelta(prefix));
        }
        pending.push_back(StreamEvent::ToolUseStart {
            id: format!(
                "fallback_text_call_{}",
                FALLBACK_TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            name: tool_name,
        });
        pending.push_back(StreamEvent::ToolInputDelta(arguments));
        pending.push_back(StreamEvent::ToolUseEnd);
        if !suffix.is_empty() {
            pending.push_back(StreamEvent::TextDelta(suffix));
        }
        return pending.pop_front();
    }

    Some(StreamEvent::TextDelta(text.to_string()))
}

fn sanitize_recovered_tool_suffix(suffix: &str) -> String {
    let trimmed = suffix.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let normalized = trimmed.trim_start_matches('"');

    if normalized.starts_with(",\"item_id\"")
        || normalized.starts_with(",\"output_index\"")
        || normalized.starts_with(",\"sequence_number\"")
        || normalized.starts_with(",\"call_id\"")
        || normalized.starts_with(",\"type\":\"response.")
        || (normalized.starts_with(',')
            && normalized.contains("\"item_id\"")
            && (normalized.contains("\"output_index\"")
                || normalized.contains("\"sequence_number\"")))
    {
        return String::new();
    }

    suffix.to_string()
}

fn orphan_tool_output_to_user_message(item: &Value, missing_output: &str) -> Option<Value> {
    let output_value = item.get("output")?;
    let output = if let Some(text) = output_value.as_str() {
        text.trim().to_string()
    } else {
        output_value.to_string()
    };
    if output.is_empty() || output == missing_output {
        return None;
    }

    let call_id = item
        .get("call_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown_call");

    Some(serde_json::json!({
        "type": "message",
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": format!("[Recovered orphaned tool output: {}]\n{}", call_id, output)
        }]
    }))
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
                                    "call_id": crate::message::sanitize_tool_id(tool_use_id),
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
                            let arguments = if input.is_object() {
                                serde_json::to_string(&input).unwrap_or_default()
                            } else {
                                "{}".to_string()
                            };
                            items.push(serde_json::json!({
                                "type": "function_call",
                                "name": name,
                                "arguments": arguments,
                                "call_id": crate::message::sanitize_tool_id(id)
                            }));

                            if let Some(output) = pending_outputs.remove(id.as_str()) {
                                items.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": crate::message::sanitize_tool_id(id),
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
                                        "call_id": crate::message::sanitize_tool_id(id),
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
                "call_id": crate::message::sanitize_tool_id(&call_id),
                "output": output
            }));
        } else {
            injected_missing += 1;
            items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": crate::message::sanitize_tool_id(&call_id),
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

    let mut rewritten_pending_orphans = 0usize;
    if !pending_outputs.is_empty() {
        let mut pending_entries: Vec<(String, String)> =
            std::mem::take(&mut pending_outputs).into_iter().collect();
        pending_entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (call_id, output) in pending_entries {
            let orphan_item = serde_json::json!({
                "type": "function_call_output",
                "call_id": crate::message::sanitize_tool_id(&call_id),
                "output": output,
            });
            if let Some(message_item) =
                orphan_tool_output_to_user_message(&orphan_item, &missing_output)
            {
                items.push(message_item);
                rewritten_pending_orphans += 1;
            } else {
                skipped_results += 1;
            }
        }
    }

    if injected_missing > 0 {
        crate::logging::info(&format!(
            "[openai] Injected {} synthetic tool output(s) to prevent API error",
            injected_missing
        ));
    }
    if rewritten_pending_orphans > 0 {
        let total = REWRITTEN_ORPHAN_TOOL_OUTPUTS
            .fetch_add(rewritten_pending_orphans as u64, Ordering::Relaxed)
            + rewritten_pending_orphans as u64;
        crate::logging::info(&format!(
            "[openai] Rewrote {} pending orphaned tool output(s) as user messages (total={})",
            rewritten_pending_orphans, total
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
    let mut dropped_duplicate_outputs = 0usize;
    let mut rewritten_orphans = 0usize;
    let mut skipped_empty_orphans = 0usize;

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
                    dropped_duplicate_outputs += 1;
                    continue;
                }
            }
            if let Some(message_item) = orphan_tool_output_to_user_message(&item, &missing_output) {
                ordered.push(message_item);
                rewritten_orphans += 1;
            } else {
                skipped_empty_orphans += 1;
            }
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
    if dropped_duplicate_outputs > 0 {
        crate::logging::info(&format!(
            "[openai] Dropped {} duplicate tool output(s) during re-ordering",
            dropped_duplicate_outputs
        ));
    }
    if rewritten_orphans > 0 {
        let total = REWRITTEN_ORPHAN_TOOL_OUTPUTS
            .fetch_add(rewritten_orphans as u64, Ordering::Relaxed)
            + rewritten_orphans as u64;
        crate::logging::info(&format!(
            "[openai] Rewrote {} orphaned tool output(s) as user messages (total={})",
            rewritten_orphans, total
        ));
    }
    if skipped_empty_orphans > 0 {
        crate::logging::info(&format!(
            "[openai] Skipped {} empty orphaned tool output(s) during re-ordering",
            skipped_empty_orphans
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
    item_id: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
    response: Option<Value>,
    error: Option<Value>,
}

#[derive(Debug, Clone, Default)]
struct StreamingToolCallState {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

fn normalize_openai_tool_arguments(raw_arguments: String) -> String {
    if raw_arguments.trim() == "null" {
        let total = NORMALIZED_NULL_TOOL_ARGUMENTS.fetch_add(1, Ordering::Relaxed) + 1;
        crate::logging::warn(&format!(
            "[openai] Normalized null tool arguments to empty object (total={})",
            total
        ));
        "{}".to_string()
    } else {
        raw_arguments
    }
}

fn streaming_tool_item_id(item: &Value) -> Option<String> {
    item.get("id")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("item_id").and_then(|v| v.as_str()))
        .map(|id| id.to_string())
}

fn stream_tool_call_from_state(
    item_id: Option<String>,
    mut state: StreamingToolCallState,
    pending: &mut VecDeque<StreamEvent>,
) -> Option<StreamEvent> {
    let tool_name = state.name.take().filter(|name| !name.is_empty())?;
    let raw_call_id = state
        .call_id
        .take()
        .filter(|id| !id.is_empty())
        .or(item_id)
        .unwrap_or_else(|| {
            format!(
                "fallback_text_call_{}",
                FALLBACK_TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed)
            )
        });
    let call_id = crate::message::sanitize_tool_id(&raw_call_id);
    let arguments = normalize_openai_tool_arguments(if state.arguments.is_empty() {
        "{}".to_string()
    } else {
        state.arguments
    });

    pending.push_back(StreamEvent::ToolUseStart {
        id: call_id,
        name: tool_name,
    });
    pending.push_back(StreamEvent::ToolInputDelta(arguments));
    pending.push_back(StreamEvent::ToolUseEnd);
    pending.pop_front()
}

fn parse_openai_response_event(
    data: &str,
    saw_text_delta: &mut bool,
    streaming_tool_calls: &mut HashMap<String, StreamingToolCallState>,
    completed_tool_items: &mut HashSet<String>,
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
                return stream_text_or_recovered_tool_call(&delta, pending);
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
                if matches!(
                    item.get("type").and_then(|v| v.as_str()),
                    Some("function_call") | Some("custom_tool_call")
                ) {
                    if let Some(item_id) = streaming_tool_item_id(item) {
                        let state = streaming_tool_calls.entry(item_id).or_default();
                        state.call_id = item
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| state.call_id.clone());
                        state.name = item
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| state.name.clone());
                        if let Some(arguments) = item
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .or_else(|| item.get("input").and_then(|v| v.as_str()))
                        {
                            state.arguments = arguments.to_string();
                        } else if let Some(input) = item.get("input") {
                            if input.is_object() || input.is_array() {
                                state.arguments = input.to_string();
                            }
                        }
                    }
                }
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(item_id) = event.item_id {
                let state = streaming_tool_calls.entry(item_id).or_default();
                if let Some(call_id) = event.call_id {
                    state.call_id = Some(call_id);
                }
                if let Some(name) = event.name {
                    state.name = Some(name);
                }
                if let Some(delta) = event.delta {
                    state.arguments.push_str(&delta);
                }
            }
        }
        "response.function_call_arguments.done" => {
            if let Some(item_id) = event.item_id {
                let mut state = streaming_tool_calls.remove(&item_id).unwrap_or_default();
                if let Some(call_id) = event.call_id {
                    state.call_id = Some(call_id);
                }
                if let Some(name) = event.name {
                    state.name = Some(name);
                }
                if let Some(arguments) = event.arguments {
                    state.arguments = arguments;
                }
                if let Some(tool_event) =
                    stream_tool_call_from_state(Some(item_id.clone()), state.clone(), pending)
                {
                    completed_tool_items.insert(item_id);
                    return Some(tool_event);
                }
                streaming_tool_calls.insert(item_id, state);
            }
        }
        "response.output_item.done" => {
            if let Some(item) = event.item {
                if let Some(item_id) = streaming_tool_item_id(&item) {
                    if completed_tool_items.contains(&item_id)
                        && matches!(
                            item.get("type").and_then(|v| v.as_str()),
                            Some("function_call") | Some("custom_tool_call")
                        )
                    {
                        completed_tool_items.remove(&item_id);
                        return None;
                    }
                }
                if let Some(event) = handle_openai_output_item(item, saw_text_delta, pending) {
                    return Some(event);
                }
            }
        }
        "response.incomplete" => {
            let stop_reason = event
                .response
                .as_ref()
                .and_then(extract_stop_reason_from_response)
                .or_else(|| Some("incomplete".to_string()));
            if let Some(response) = event.response {
                if let Some(usage_event) = extract_usage_from_response(&response) {
                    pending.push_back(usage_event);
                }
            }
            pending.push_back(StreamEvent::MessageEnd { stop_reason });
            return pending.pop_front();
        }
        "response.completed" => {
            let stop_reason = event
                .response
                .as_ref()
                .and_then(extract_stop_reason_from_response);
            if let Some(response) = event.response {
                if let Some(usage_event) = extract_usage_from_response(&response) {
                    pending.push_back(usage_event);
                }
            }
            pending.push_back(StreamEvent::MessageEnd { stop_reason });
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

fn extract_last_assistant_message_phase(response: &Value) -> Option<String> {
    let output = response.get("output")?.as_array()?;
    output.iter().rev().find_map(|item| {
        if item.get("type").and_then(|v| v.as_str()) != Some("message") {
            return None;
        }
        if item.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            return None;
        }
        item.get("phase")
            .and_then(|v| v.as_str())
            .map(|phase| phase.to_string())
    })
}

fn extract_stop_reason_from_response(response: &Value) -> Option<String> {
    let status = response.get("status").and_then(|v| v.as_str());
    if status == Some("completed") {
        if extract_last_assistant_message_phase(response).as_deref() == Some("commentary") {
            return Some("commentary".to_string());
        }
        return None;
    }

    let incomplete_reason = response
        .get("incomplete_details")
        .and_then(|v| v.get("reason"))
        .and_then(|v| v.as_str());

    if let Some(reason) = incomplete_reason {
        return Some(reason.to_string());
    }

    status
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
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
            let raw_arguments = item
                .get("arguments")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .or_else(|| {
                    item.get("input").and_then(|v| {
                        if v.is_object() || v.is_array() {
                            Some(v.to_string())
                        } else {
                            v.as_str().map(|s| s.to_string())
                        }
                    })
                })
                .unwrap_or_else(|| "{}".to_string());
            let arguments = normalize_openai_tool_arguments(raw_arguments);

            pending.push_back(StreamEvent::ToolUseStart {
                id: call_id.clone(),
                name,
            });
            pending.push_back(StreamEvent::ToolInputDelta(arguments));
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
            return stream_text_or_recovered_tool_call(&text, pending);
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
    streaming_tool_calls: HashMap<String, StreamingToolCallState>,
    completed_tool_items: HashSet<String>,
}

impl OpenAIResponsesStream {
    fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: VecDeque::new(),
            saw_text_delta: false,
            streaming_tool_calls: HashMap::new(),
            completed_tool_items: HashSet::new(),
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
            if let Some(event) = parse_openai_response_event(
                &data,
                &mut self.saw_text_delta,
                &mut self.streaming_tool_calls,
                &mut self.completed_tool_items,
                &mut self.pending,
            ) {
                return Some(event);
            }
        }

        None
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
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let input = build_responses_input(messages);
        let input_item_count = input.len();
        let api_tools = build_tools(tools);
        let model_id = self.model_id().await;
        let (instructions, is_chatgpt_mode) = {
            let credentials = self.credentials.read().await;
            let is_chatgpt = Self::is_chatgpt_mode(&credentials);
            let instructions = if is_chatgpt {
                Self::chatgpt_instructions_with_selfdev(system)
            } else {
                system.to_string()
            };
            (instructions, is_chatgpt)
        };
        let reasoning_effort = self.reasoning_effort.read().await.clone();
        let service_tier = self
            .service_tier
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        let request = Self::build_response_request(
            &model_id,
            instructions,
            &input,
            &api_tools,
            is_chatgpt_mode,
            self.max_output_tokens,
            reasoning_effort.as_deref(),
            service_tier.as_deref(),
            self.prompt_cache_key.as_deref(),
            self.prompt_cache_retention.as_deref(),
        );

        // --- Persistent WebSocket continuation path ---
        // Try to reuse an existing WebSocket connection with previous_response_id
        // to send only incremental input items instead of the full conversation.
        let persistent_ws = Arc::clone(&self.persistent_ws);
        let transport_mode_snapshot = self
            .transport_mode
            .try_read()
            .map(|g| *g)
            .unwrap_or(OpenAITransportMode::HTTPS);
        let use_websocket_transport = match transport_mode_snapshot {
            OpenAITransportMode::HTTPS => false,
            OpenAITransportMode::WebSocket => true,
            OpenAITransportMode::Auto => Self::should_prefer_websocket(&model_id),
        };

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        let credentials = Arc::clone(&self.credentials);
        let transport_mode = transport_mode_snapshot;
        let websocket_cooldowns = Arc::clone(&self.websocket_cooldowns);
        let websocket_failure_streaks = Arc::clone(&self.websocket_failure_streaks);
        let model_for_transport = model_id.clone();
        let client = self.client.clone();

        tokio::spawn(async move {
            // Attempt persistent WebSocket continuation first
            if use_websocket_transport {
                let continuation_result = try_persistent_ws_continuation(
                    &persistent_ws,
                    &request,
                    &input,
                    input_item_count,
                    &tx,
                )
                .await;

                match continuation_result {
                    PersistentWsResult::Success => {
                        record_websocket_success(
                            &websocket_cooldowns,
                            &websocket_failure_streaks,
                            &model_for_transport,
                        )
                        .await;
                        return;
                    }
                    PersistentWsResult::NotAvailable => {
                        crate::logging::info(
                            "No persistent WS connection available; using fresh connection",
                        );
                    }
                    PersistentWsResult::Failed(err) => {
                        crate::logging::warn(&format!(
                            "Persistent WS continuation failed: {}; using fresh connection",
                            err
                        ));
                        let mut guard = persistent_ws.lock().await;
                        *guard = None;
                    }
                }
            }

            // Normal path: fresh connection with full input (with retry logic)
            let mut last_error = None;
            let mut force_https_for_request = false;
            let mut skip_backoff_once = false;

            for attempt in 0..MAX_RETRIES {
                if attempt > 0 && !skip_backoff_once {
                    let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    crate::logging::info(&format!(
                        "Retrying OpenAI API request (attempt {}/{})",
                        attempt + 1,
                        MAX_RETRIES
                    ));
                }
                skip_backoff_once = false;

                let transport = if force_https_for_request {
                    OpenAITransport::HTTPS
                } else {
                    match transport_mode {
                        OpenAITransportMode::HTTPS => OpenAITransport::HTTPS,
                        OpenAITransportMode::WebSocket => OpenAITransport::WebSocket,
                        OpenAITransportMode::Auto => {
                            if !Self::should_prefer_websocket(&model_for_transport) {
                                OpenAITransport::HTTPS
                            } else if let Some(remaining) = websocket_cooldown_remaining(
                                &websocket_cooldowns,
                                &model_for_transport,
                            )
                            .await
                            {
                                crate::logging::info(&format!(
                                    "OpenAI websocket cooldown active for model='{}' ({}s remaining); using HTTPS",
                                    model_for_transport,
                                    remaining.as_secs()
                                ));
                                OpenAITransport::HTTPS
                            } else {
                                OpenAITransport::WebSocket
                            }
                        }
                    }
                };

                let transport_label = transport.as_str();
                let attempt_started = Instant::now();
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
                    stream_response_websocket_persistent(
                        Arc::clone(&credentials),
                        request.clone(),
                        tx.clone(),
                        Arc::clone(&persistent_ws),
                        input_item_count,
                    )
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
                    Ok(()) => {
                        if use_websocket {
                            record_websocket_success(
                                &websocket_cooldowns,
                                &websocket_failure_streaks,
                                &model_for_transport,
                            )
                            .await;
                        }
                        return;
                    }
                    Err(OpenAIStreamFailure::FallbackToHttps(error)) => {
                        let elapsed_ms = attempt_started.elapsed().as_millis();
                        crate::logging::warn(&format!(
                            "WebSocket fallback after {}ms: {}",
                            elapsed_ms, error
                        ));
                        force_https_for_request = true;
                        skip_backoff_once = true;
                        if matches!(transport_mode, OpenAITransportMode::Auto) {
                            let (streak, cooldown) = record_websocket_fallback(
                                &websocket_cooldowns,
                                &websocket_failure_streaks,
                                &model_for_transport,
                            )
                            .await;
                            crate::logging::warn(&format!(
                                "OpenAI websocket backoff for model='{}': streak={} cooldown={}s",
                                model_for_transport,
                                streak,
                                cooldown.as_secs()
                            ));
                        }
                        // Clear persistent state on fallback
                        {
                            let mut guard = persistent_ws.lock().await;
                            *guard = None;
                        }
                        last_error = Some(error);
                        continue;
                    }
                    Err(OpenAIStreamFailure::Other(error)) => {
                        let elapsed_ms = attempt_started.elapsed().as_millis();
                        let error_str = error.to_string().to_lowercase();
                        if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                            crate::logging::info(&format!(
                                "Transient error after {}ms, will retry: {}",
                                elapsed_ms, error
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
        if !crate::provider::known_openai_model_ids()
            .iter()
            .any(|known| known == model)
        {
            anyhow::bail!(
                "Unsupported OpenAI model '{}'. Use /model to choose from the models available to your account.",
                model,
            );
        }
        let availability = crate::provider::model_availability_for_account(model);
        if availability.state == crate::provider::AccountModelAvailabilityState::Unavailable {
            let detail = crate::provider::format_account_model_availability_detail(&availability)
                .unwrap_or_else(|| "not available for your account".to_string());
            anyhow::bail!(
                "The '{}' model is not available for your account right now ({}). \
                 Use /model to see available models.",
                model,
                detail
            );
        }
        if let Ok(mut current) = self.model.try_write() {
            *current = model.to_string();
            crate::provider::clear_model_unavailable_for_account(model);
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

    fn available_models_display(&self) -> Vec<String> {
        crate::provider::known_openai_model_ids()
    }

    fn reasoning_effort(&self) -> Option<String> {
        self.reasoning_effort
            .try_read()
            .ok()
            .and_then(|g| g.clone())
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        let normalized = Self::normalize_reasoning_effort(effort);
        match self.reasoning_effort.try_write() {
            Ok(mut guard) => {
                *guard = normalized;
                Ok(())
            }
            Err(_) => Err(anyhow::anyhow!(
                "Failed to acquire lock for reasoning effort"
            )),
        }
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        vec!["none", "low", "medium", "high", "xhigh"]
    }

    fn service_tier(&self) -> Option<String> {
        self.service_tier
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    fn set_service_tier(&self, service_tier: &str) -> Result<()> {
        let normalized = Self::normalize_service_tier(service_tier)?;
        let mut guard = self
            .service_tier
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = normalized;
        Ok(())
    }

    fn available_service_tiers(&self) -> Vec<&'static str> {
        vec!["priority", "flex"]
    }

    fn transport(&self) -> Option<String> {
        self.transport_mode
            .try_read()
            .ok()
            .map(|g| g.as_str().to_string())
    }

    fn set_transport(&self, transport: &str) -> Result<()> {
        let mode = match transport.trim().to_ascii_lowercase().as_str() {
            "auto" => OpenAITransportMode::Auto,
            "https" | "http" | "sse" => OpenAITransportMode::HTTPS,
            "websocket" | "ws" | "wss" => OpenAITransportMode::WebSocket,
            other => anyhow::bail!(
                "Unknown transport '{}'. Use: auto, https, or websocket.",
                other
            ),
        };
        match self.transport_mode.try_write() {
            Ok(mut guard) => {
                *guard = mode;
                Ok(())
            }
            Err(_) => Err(anyhow::anyhow!(
                "Cannot change transport while a request is in progress"
            )),
        }
    }

    fn available_transports(&self) -> Vec<&'static str> {
        vec!["auto", "https", "websocket"]
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        let model = self.model();
        crate::provider::context_limit_for_model_with_provider(&model, Some(self.name()))
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
            max_output_tokens: self.max_output_tokens,
            reasoning_effort: Arc::new(RwLock::new(self.reasoning_effort())),
            service_tier: Arc::new(StdRwLock::new(self.service_tier())),
            transport_mode: Arc::clone(&self.transport_mode),
            websocket_cooldowns: Arc::clone(&self.websocket_cooldowns),
            websocket_failure_streaks: Arc::clone(&self.websocket_failure_streaks),
            persistent_ws: Arc::new(Mutex::new(None)),
        })
    }

    async fn invalidate_credentials(&self) {
        if let Ok(credentials) = crate::auth::codex::load_credentials() {
            let mut guard = self.credentials.write().await;
            *guard = credentials;
        }

        let mut persistent_ws = self.persistent_ws.lock().await;
        *persistent_ws = None;
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
    use crate::message::ConnectionPhase;
    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Authenticating,
        }))
        .await;
    let access_token = openai_access_token(&credentials).await?;
    let creds = credentials.read().await;
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

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Connecting,
        }))
        .await;
    let connect_start = std::time::Instant::now();

    let response = builder
        .json(&request)
        .send()
        .await
        .context("Failed to send request to OpenAI API")
        .map_err(OpenAIStreamFailure::Other)?;

    let connect_ms = connect_start.elapsed().as_millis();
    crate::logging::info(&format!(
        "HTTP connection established in {}ms (status={})",
        connect_ms,
        response.status()
    ));

    if !response.status().is_success() {
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let body = response.text().await.unwrap_or_default();

        if let Some(reason) = classify_unavailable_model_error(status, &body) {
            if let Some(model_name) = request.get("model").and_then(|m| m.as_str()) {
                crate::provider::record_model_unavailable_for_account(model_name, &reason);
                crate::logging::warn(&format!(
                    "Recorded OpenAI model '{}' as unavailable: {}",
                    model_name, reason
                ));
            }
        }

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

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::WaitingForResponse,
        }))
        .await;

    let _ = tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "https/sse".to_string(),
        }))
        .await;

    // Stream the response
    let mut stream = OpenAIResponsesStream::new(response.bytes_stream());
    let mut saw_message_end = false;

    use futures::StreamExt;
    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => {
                if matches!(event, StreamEvent::MessageEnd { .. }) {
                    saw_message_end = true;
                }
                if let StreamEvent::Error { message, .. } = &event {
                    if let Some(model_name) = request.get("model").and_then(|m| m.as_str()) {
                        maybe_record_runtime_model_unavailable_from_stream_error(
                            model_name, message,
                        );
                    }
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

    if !saw_message_end {
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
            "OpenAI HTTPS stream ended before message completion marker"
        )));
    }

    Ok(())
}

fn is_ws_upgrade_required(err: &WsError) -> bool {
    match err {
        WsError::Http(response) => response.status() == WEBSOCKET_UPGRADE_REQUIRED_ERROR,
        _ => false,
    }
}

/// Stream the response from OpenAI API using websockets (legacy, non-persistent).
/// Kept for reference; the persistent variant `stream_response_websocket_persistent` is used instead.
#[allow(dead_code)]
async fn stream_response_websocket(
    credentials: Arc<RwLock<CodexCredentials>>,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<(), OpenAIStreamFailure> {
    use crate::message::ConnectionPhase;
    let request_model = request
        .get("model")
        .and_then(|m| m.as_str())
        .map(|m| m.to_string());

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

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Connecting,
        }))
        .await;
    let connect_start = std::time::Instant::now();

    let connect_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_CONNECT_TIMEOUT_SECS),
        connect_async(ws_request),
    )
    .await
    .map_err(|_| {
        OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
            "WebSocket connect timed out after {}s",
            WEBSOCKET_CONNECT_TIMEOUT_SECS
        ))
    })?;

    let (mut ws_stream, _response) = match connect_result {
        Ok((stream, response)) => {
            let connect_ms = connect_start.elapsed().as_millis();
            crate::logging::info(&format!(
                "WebSocket connection established in {}ms",
                connect_ms
            ));
            (stream, response)
        }
        Err(err) if is_ws_upgrade_required(&err) => {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "Falling back from websockets to HTTPS transport"
            )));
        }
        Err(err) => {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
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
    {
        let obj = request_event
            .as_object_mut()
            .expect("request_event is object");
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("response.create".to_string()),
        );
        obj.remove("stream");
        obj.remove("background");
    }

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
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut saw_response_completed = false;
    let mut saw_api_activity = false;
    let ws_started_at = Instant::now();
    let mut last_api_activity_at = ws_started_at;
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

        if !saw_api_activity
            && ws_started_at.elapsed() >= Duration::from_secs(WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS)
        {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream did not emit API activity within {}s",
                WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
            )));
        }

        let timeout_secs = websocket_next_activity_timeout_secs(
            ws_started_at,
            last_api_activity_at,
            saw_api_activity,
        )
        .ok_or_else(|| {
            OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream timed out waiting for {} websocket activity ({}s)",
                websocket_activity_timeout_kind(saw_api_activity),
                if saw_api_activity {
                    WEBSOCKET_COMPLETION_TIMEOUT_SECS
                } else {
                    WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
                }
            ))
        })?;
        let next_item = tokio::time::timeout(Duration::from_secs(timeout_secs), ws_stream.next())
            .await
            .map_err(|_| {
                OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                    "WebSocket stream timed out waiting for {} websocket activity ({}s)",
                    websocket_activity_timeout_kind(saw_api_activity),
                    timeout_secs
                ))
            })?;

        let Some(result) = next_item else {
            if saw_response_completed {
                return Ok(());
            }
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream ended before response.completed"
            )));
        };

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

                    let mut made_api_activity = if saw_api_activity {
                        is_websocket_activity_payload(&text)
                    } else {
                        is_websocket_first_activity_payload(&text)
                    };
                    if let Some(event) = parse_openai_response_event(
                        &text,
                        &mut saw_text_delta,
                        &mut streaming_tool_calls,
                        &mut completed_tool_items,
                        &mut pending,
                    ) {
                        if is_stream_activity_event(&event) {
                            made_api_activity = true;
                        }
                        if matches!(event, StreamEvent::MessageEnd { .. }) {
                            saw_response_completed = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if let Some(model_name) = request_model.as_deref() {
                                maybe_record_runtime_model_unavailable_from_stream_error(
                                    model_name, message,
                                );
                            }
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
                        if is_stream_activity_event(&event) {
                            made_api_activity = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if let Some(model_name) = request_model.as_deref() {
                                maybe_record_runtime_model_unavailable_from_stream_error(
                                    model_name, message,
                                );
                            }
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
                    if made_api_activity {
                        saw_api_activity = true;
                        last_api_activity_at = Instant::now();
                    }
                    if saw_response_completed {
                        return Ok(());
                    }
                }
                WsMessage::Ping(payload) => {
                    let _ = ws_stream.send(WsMessage::Pong(payload)).await;
                }
                WsMessage::Close(_) => {
                    if saw_response_completed {
                        return Ok(());
                    }
                    return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
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
}

/// Result of trying to continue on a persistent WebSocket connection
enum PersistentWsResult {
    Success,
    NotAvailable,
    Failed(String),
}

/// Try to continue a conversation on an existing persistent WebSocket connection
/// using `previous_response_id` to send only incremental input.
async fn try_persistent_ws_continuation(
    persistent_ws: &Arc<Mutex<Option<PersistentWsState>>>,
    request: &Value,
    input: &[Value],
    input_item_count: usize,
    tx: &mpsc::Sender<Result<StreamEvent>>,
) -> PersistentWsResult {
    let mut guard = persistent_ws.lock().await;
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PersistentWsResult::NotAvailable,
    };

    // Check connection age - reconnect before the 60-min server limit
    if state.connected_at.elapsed() >= Duration::from_secs(WEBSOCKET_PERSISTENT_MAX_AGE_SECS) {
        crate::logging::info("Persistent WS connection too old; forcing reconnect");
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }

    // The input array must be strictly growing for continuation to make sense.
    // If the input_item_count is less than or equal to last time, the conversation
    // was reset (e.g., after compaction) - we need a fresh connection.
    if input_item_count <= state.last_input_item_count {
        crate::logging::info(&format!(
            "Input items didn't grow ({} <= {}); conversation may have been compacted, reconnecting",
            input_item_count, state.last_input_item_count
        ));
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }

    // Compute incremental items: everything after the last_input_item_count
    let incremental_items: Vec<Value> = input[state.last_input_item_count..].to_vec();
    if incremental_items.is_empty() {
        crate::logging::info("No incremental items to send; need fresh request");
        *guard = None;
        return PersistentWsResult::NotAvailable;
    }

    let previous_response_id = state.last_response_id.clone();
    crate::logging::info(&format!(
        "Persistent WS continuation: previous_response_id={}, incremental_items={} (was {} now {})",
        previous_response_id,
        incremental_items.len(),
        state.last_input_item_count,
        input_item_count,
    ));

    // Build the incremental request - only include new items + previous_response_id
    let mut continuation_request = serde_json::json!({
        "type": "response.create",
        "previous_response_id": previous_response_id,
        "input": incremental_items,
    });

    // Copy over model, tools, and other settings from the original request
    if let Some(model) = request.get("model") {
        continuation_request["model"] = model.clone();
    }
    if let Some(tools) = request.get("tools") {
        continuation_request["tools"] = tools.clone();
    }
    if let Some(tool_choice) = request.get("tool_choice") {
        continuation_request["tool_choice"] = tool_choice.clone();
    }
    if let Some(instructions) = request.get("instructions") {
        continuation_request["instructions"] = instructions.clone();
    }
    if let Some(max_output_tokens) = request.get("max_output_tokens") {
        continuation_request["max_output_tokens"] = max_output_tokens.clone();
    }
    if let Some(reasoning) = request.get("reasoning") {
        continuation_request["reasoning"] = reasoning.clone();
    }
    if let Some(include) = request.get("include") {
        continuation_request["include"] = include.clone();
    }
    continuation_request["store"] = serde_json::json!(false);
    continuation_request["parallel_tool_calls"] = serde_json::json!(false);

    let request_text = match serde_json::to_string(&continuation_request) {
        Ok(t) => t,
        Err(e) => return PersistentWsResult::Failed(format!("serialize error: {}", e)),
    };

    let _ = tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "websocket/persistent".to_string(),
        }))
        .await;

    // Send the continuation request on the existing WebSocket
    if let Err(e) = state.ws_stream.send(WsMessage::Text(request_text)).await {
        return PersistentWsResult::Failed(format!("send error: {}", e));
    }

    // Stream the response, extracting the new response_id
    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut saw_response_completed = false;
    let mut pending: VecDeque<StreamEvent> = VecDeque::new();
    let mut new_response_id: Option<String> = None;
    let stream_started = Instant::now();
    let mut last_api_activity_at = stream_started;
    let mut saw_api_activity = false;

    loop {
        if stream_started.elapsed() >= Duration::from_secs(WEBSOCKET_COMPLETION_TIMEOUT_SECS) {
            return PersistentWsResult::Failed("completion timeout".to_string());
        }

        let timeout_secs = match websocket_next_activity_timeout_secs(
            stream_started,
            last_api_activity_at,
            saw_api_activity,
        ) {
            Some(timeout_secs) => timeout_secs,
            None => {
                return PersistentWsResult::Failed(format!(
                    "timed out waiting for {} websocket activity on persistent WS ({}s)",
                    websocket_activity_timeout_kind(saw_api_activity),
                    if saw_api_activity {
                        WEBSOCKET_COMPLETION_TIMEOUT_SECS
                    } else {
                        WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
                    }
                ));
            }
        };
        let next_item =
            match tokio::time::timeout(Duration::from_secs(timeout_secs), state.ws_stream.next())
                .await
            {
                Ok(item) => item,
                Err(_) => {
                    return PersistentWsResult::Failed(format!(
                        "timed out waiting for {} websocket activity on persistent WS ({}s)",
                        websocket_activity_timeout_kind(saw_api_activity),
                        timeout_secs
                    ));
                }
            };

        let Some(result) = next_item else {
            if saw_response_completed {
                break;
            }
            return PersistentWsResult::Failed(
                "persistent WS stream ended before response.completed".to_string(),
            );
        };

        match result {
            Ok(WsMessage::Text(text)) => {
                let text = text.to_string();
                if is_websocket_fallback_notice(&text) {
                    return PersistentWsResult::Failed("server requested fallback".to_string());
                }

                let mut made_api_activity = if saw_api_activity {
                    is_websocket_activity_payload(&text)
                } else {
                    is_websocket_first_activity_payload(&text)
                };

                // Extract response_id from response.created event
                if new_response_id.is_none() {
                    if let Ok(val) = serde_json::from_str::<Value>(&text) {
                        if val.get("type").and_then(|t| t.as_str()) == Some("response.created") {
                            if let Some(id) = val
                                .get("response")
                                .and_then(|r| r.get("id"))
                                .and_then(|id| id.as_str())
                            {
                                new_response_id = Some(id.to_string());
                                crate::logging::info(&format!(
                                    "Persistent WS got new response_id: {}",
                                    id
                                ));
                            }
                        }
                    }
                }

                if let Some(event) = parse_openai_response_event(
                    &text,
                    &mut saw_text_delta,
                    &mut streaming_tool_calls,
                    &mut completed_tool_items,
                    &mut pending,
                ) {
                    if is_stream_activity_event(&event) {
                        made_api_activity = true;
                    }
                    if matches!(event, StreamEvent::MessageEnd { .. }) {
                        saw_response_completed = true;
                    }
                    if let StreamEvent::Error { ref message, .. } = event {
                        if is_retryable_error(&message.to_lowercase()) {
                            return PersistentWsResult::Failed(format!(
                                "stream error: {}",
                                message
                            ));
                        }
                    }
                    if tx.send(Ok(event)).await.is_err() {
                        break; // Receiver dropped
                    }
                }
                while let Some(event) = pending.pop_front() {
                    if is_stream_activity_event(&event) {
                        made_api_activity = true;
                    }
                    if matches!(event, StreamEvent::MessageEnd { .. }) {
                        saw_response_completed = true;
                    }
                    if tx.send(Ok(event)).await.is_err() {
                        break;
                    }
                }
                if made_api_activity {
                    saw_api_activity = true;
                    last_api_activity_at = Instant::now();
                }
                if saw_response_completed {
                    break;
                }
            }
            Ok(WsMessage::Ping(payload)) => {
                let _ = state.ws_stream.send(WsMessage::Pong(payload)).await;
            }
            Ok(WsMessage::Close(_)) => {
                if saw_response_completed {
                    break;
                }
                return PersistentWsResult::Failed("server closed connection".to_string());
            }
            Ok(WsMessage::Pong(_)) | Ok(_) => {}
            Err(e) => {
                return PersistentWsResult::Failed(format!("ws error: {}", e));
            }
        }
    }

    // Update persistent state for next turn
    if let Some(resp_id) = new_response_id {
        state.last_response_id = resp_id;
        state.last_input_item_count = input_item_count;
        state.message_count += 1;
        crate::logging::info(&format!(
            "Persistent WS continuation success (chain length: {})",
            state.message_count
        ));
        PersistentWsResult::Success
    } else {
        // Got response but no response_id - can't chain further
        crate::logging::warn("Persistent WS: no response_id in response; breaking chain");
        *guard = None;
        PersistentWsResult::Success
    }
}

/// Stream response via WebSocket, saving the connection for reuse.
/// This replaces the old `stream_response_websocket` for the fresh-connection path.
async fn stream_response_websocket_persistent(
    credentials: Arc<RwLock<CodexCredentials>>,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    persistent_ws: Arc<Mutex<Option<PersistentWsState>>>,
    input_item_count: usize,
) -> Result<(), OpenAIStreamFailure> {
    use crate::message::ConnectionPhase;
    let request_model = request
        .get("model")
        .and_then(|m| m.as_str())
        .map(|m| m.to_string());

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

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Connecting,
        }))
        .await;
    let connect_start = std::time::Instant::now();

    let connect_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_CONNECT_TIMEOUT_SECS),
        connect_async(ws_request),
    )
    .await
    .map_err(|_| {
        OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
            "WebSocket connect timed out after {}s",
            WEBSOCKET_CONNECT_TIMEOUT_SECS
        ))
    })?;

    let (mut ws_stream, _response) = match connect_result {
        Ok((stream, response)) => {
            let connect_ms = connect_start.elapsed().as_millis();
            crate::logging::info(&format!(
                "WebSocket connection established in {}ms (persistent mode)",
                connect_ms
            ));
            (stream, response)
        }
        Err(err) if is_ws_upgrade_required(&err) => {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "Falling back from websockets to HTTPS transport"
            )));
        }
        Err(err) => {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "Failed to connect websocket stream: {}",
                err
            )));
        }
    };

    let _ = tx
        .send(Ok(StreamEvent::ConnectionType {
            connection: "websocket/persistent".to_string(),
        }))
        .await;

    let mut request_event = request;
    if !request_event.is_object() {
        return Err(OpenAIStreamFailure::Other(anyhow::anyhow!(
            "Invalid websocket request payload shape; expected an object"
        )));
    }
    {
        let obj = request_event
            .as_object_mut()
            .expect("request_event is object");
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("response.create".to_string()),
        );
        obj.remove("stream");
        obj.remove("background");
    }

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

    let mut saw_text_delta = false;
    let mut streaming_tool_calls = HashMap::new();
    let mut completed_tool_items = HashSet::new();
    let mut saw_response_completed = false;
    let mut saw_api_activity = false;
    let ws_started_at = Instant::now();
    let mut last_api_activity_at = ws_started_at;
    let mut pending: VecDeque<StreamEvent> = VecDeque::new();
    let mut response_id: Option<String> = None;
    let connected_at = Instant::now();

    loop {
        if !saw_response_completed
            && ws_started_at.elapsed() >= Duration::from_secs(WEBSOCKET_COMPLETION_TIMEOUT_SECS)
        {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream did not complete within {}s",
                WEBSOCKET_COMPLETION_TIMEOUT_SECS
            )));
        }

        if !saw_api_activity
            && ws_started_at.elapsed() >= Duration::from_secs(WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS)
        {
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream did not emit API activity within {}s",
                WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
            )));
        }

        let timeout_secs = websocket_next_activity_timeout_secs(
            ws_started_at,
            last_api_activity_at,
            saw_api_activity,
        )
        .ok_or_else(|| {
            OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream timed out waiting for {} websocket activity ({}s)",
                websocket_activity_timeout_kind(saw_api_activity),
                if saw_api_activity {
                    WEBSOCKET_COMPLETION_TIMEOUT_SECS
                } else {
                    WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS
                }
            ))
        })?;
        let next_item = tokio::time::timeout(Duration::from_secs(timeout_secs), ws_stream.next())
            .await
            .map_err(|_| {
                OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                    "WebSocket stream timed out waiting for {} websocket activity ({}s)",
                    websocket_activity_timeout_kind(saw_api_activity),
                    timeout_secs
                ))
            })?;

        let Some(result) = next_item else {
            if saw_response_completed {
                break;
            }
            return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
                "WebSocket stream ended before response.completed"
            )));
        };

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

                    // Extract response_id from response.created event
                    if response_id.is_none() {
                        if let Ok(val) = serde_json::from_str::<Value>(&text) {
                            if val.get("type").and_then(|t| t.as_str()) == Some("response.created")
                            {
                                if let Some(id) = val
                                    .get("response")
                                    .and_then(|r| r.get("id"))
                                    .and_then(|id| id.as_str())
                                {
                                    response_id = Some(id.to_string());
                                    crate::logging::info(&format!(
                                        "Fresh WS got response_id: {} (will save for continuation)",
                                        id
                                    ));
                                }
                            }
                        }
                    }

                    let mut made_api_activity = if saw_api_activity {
                        is_websocket_activity_payload(&text)
                    } else {
                        is_websocket_first_activity_payload(&text)
                    };
                    if let Some(event) = parse_openai_response_event(
                        &text,
                        &mut saw_text_delta,
                        &mut streaming_tool_calls,
                        &mut completed_tool_items,
                        &mut pending,
                    ) {
                        if is_stream_activity_event(&event) {
                            made_api_activity = true;
                        }
                        if matches!(event, StreamEvent::MessageEnd { .. }) {
                            saw_response_completed = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if let Some(model_name) = request_model.as_deref() {
                                maybe_record_runtime_model_unavailable_from_stream_error(
                                    model_name, message,
                                );
                            }
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
                        if is_stream_activity_event(&event) {
                            made_api_activity = true;
                        }
                        if let StreamEvent::Error { message, .. } = &event {
                            if let Some(model_name) = request_model.as_deref() {
                                maybe_record_runtime_model_unavailable_from_stream_error(
                                    model_name, message,
                                );
                            }
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
                    if made_api_activity {
                        saw_api_activity = true;
                        last_api_activity_at = Instant::now();
                    }
                    if saw_response_completed {
                        break;
                    }
                }
                WsMessage::Ping(payload) => {
                    let _ = ws_stream.send(WsMessage::Pong(payload)).await;
                }
                WsMessage::Close(_) => {
                    if saw_response_completed {
                        break;
                    }
                    return Err(OpenAIStreamFailure::FallbackToHttps(anyhow::anyhow!(
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

    // Save the WebSocket connection and response_id for reuse on next turn
    if let Some(resp_id) = response_id {
        let mut guard = persistent_ws.lock().await;
        crate::logging::info(&format!(
            "Saving persistent WS connection (response_id={}, items={})",
            resp_id, input_item_count
        ));
        *guard = Some(PersistentWsState {
            ws_stream,
            last_response_id: resp_id,
            connected_at,
            message_count: 1,
            last_input_item_count: input_item_count,
        });
    } else {
        crate::logging::info(
            "No response_id captured from WS stream; connection not saved for reuse",
        );
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

fn maybe_record_runtime_model_unavailable_from_stream_error(model: &str, message: &str) {
    let reason = classify_unavailable_model_error(StatusCode::BAD_REQUEST, message)
        .or_else(|| classify_unavailable_model_error(StatusCode::FORBIDDEN, message));

    if let Some(reason) = reason {
        crate::provider::record_model_unavailable_for_account(model, &reason);
        crate::logging::warn(&format!(
            "Recorded OpenAI model '{}' as unavailable from stream error: {}",
            model, reason
        ));
    }
}

fn classify_unavailable_model_error(status: StatusCode, body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();

    let mentions_model = lower.contains("model")
        || lower.contains("slug")
        || lower.contains("engine")
        || lower.contains("deployment");
    let unavailable = lower.contains("not available")
        || lower.contains("unavailable")
        || lower.contains("does not have access")
        || lower.contains("not enabled")
        || lower.contains("not found")
        || lower.contains("unknown model")
        || lower.contains("unsupported model")
        || lower.contains("invalid model");

    if !mentions_model || !unavailable {
        return None;
    }

    if status == StatusCode::NOT_FOUND
        || status == StatusCode::FORBIDDEN
        || status == StatusCode::BAD_REQUEST
        || status == StatusCode::UNPROCESSABLE_ENTITY
    {
        let trimmed = body.trim();
        let reason = if trimmed.is_empty() {
            format!("model denied by OpenAI API (status {})", status)
        } else {
            format!(
                "model denied by OpenAI API (status {}): {}",
                status, trimmed
            )
        };
        return Some(reason);
    }

    None
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
    let error_type = error.get("type").and_then(|v| v.as_str());
    let code = error.get("code").and_then(|v| v.as_str());

    let message_lower = message.to_lowercase();
    let message = match (error_type, code) {
        (Some(error_type), Some(code))
            if !message_lower.contains(&error_type.to_lowercase())
                && !message_lower.contains(&code.to_lowercase()) =>
        {
            format!("{} ({}): {}", error_type, code, message)
        }
        (Some(error_type), _) if !message_lower.contains(&error_type.to_lowercase()) => {
            format!("{}: {}", error_type, message)
        }
        (_, Some(code)) if !message_lower.contains(&code.to_lowercase()) => {
            format!("{}: {}", code, message)
        }
        _ => message,
    };

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
        || error_str.contains("ended before message completion marker")
        || error_str.contains("falling back from websockets to https transport")
        // Server errors (5xx)
        || error_str.contains("500 internal server error")
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
        // API-level server errors
        || error_str.contains("api_error")
        || error_str.contains("server_error")
        || error_str.contains("internal server error")
        || error_str.contains("an error occurred while processing your request")
        || error_str.contains("please include the request id")
}

fn is_websocket_fallback_notice(data: &str) -> bool {
    data.to_lowercase().contains(WEBSOCKET_FALLBACK_NOTICE)
}

fn is_stream_activity_event(_event: &StreamEvent) -> bool {
    true
}

fn is_websocket_activity_payload(data: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    let Some(kind) = value.get("type").and_then(|kind| kind.as_str()) else {
        return false;
    };
    kind.starts_with("response.") || kind == "error"
}

fn is_websocket_first_activity_payload(data: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    value
        .get("type")
        .and_then(|kind| kind.as_str())
        .map(|kind| !kind.is_empty())
        .unwrap_or(false)
}

fn websocket_remaining_timeout_secs(since: Instant, timeout_secs: u64) -> Option<u64> {
    let timeout = Duration::from_secs(timeout_secs);
    let elapsed = since.elapsed();
    if elapsed >= timeout {
        return None;
    }

    Some(timeout_secs.saturating_sub(elapsed.as_secs()).max(1))
}

fn websocket_next_activity_timeout_secs(
    ws_started_at: Instant,
    last_api_activity_at: Instant,
    saw_api_activity: bool,
) -> Option<u64> {
    if !saw_api_activity {
        websocket_remaining_timeout_secs(ws_started_at, WEBSOCKET_FIRST_EVENT_TIMEOUT_SECS)
    } else {
        websocket_remaining_timeout_secs(last_api_activity_at, WEBSOCKET_COMPLETION_TIMEOUT_SECS)
    }
}

fn websocket_activity_timeout_kind(saw_api_activity: bool) -> &'static str {
    if saw_api_activity { "next" } else { "first" }
}

fn normalize_transport_model(model: &str) -> Option<String> {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

async fn websocket_cooldown_remaining(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    model: &str,
) -> Option<Duration> {
    let key = normalize_transport_model(model)?;
    let now = Instant::now();

    {
        let guard = websocket_cooldowns.read().await;
        if let Some(until) = guard.get(&key) {
            if *until > now {
                return Some(*until - now);
            }
        }
    }

    let mut guard = websocket_cooldowns.write().await;
    if let Some(until) = guard.get(&key) {
        if *until > now {
            return Some(*until - now);
        }
        guard.remove(&key);
    }
    None
}

#[cfg(test)]
async fn set_websocket_cooldown(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    model: &str,
) {
    let Some(key) = normalize_transport_model(model) else {
        return;
    };

    let cooldown = Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS);
    let until = Instant::now() + cooldown;
    let mut guard = websocket_cooldowns.write().await;
    guard.insert(key, until);
}

async fn set_websocket_cooldown_for(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    model: &str,
    cooldown: Duration,
) {
    let Some(key) = normalize_transport_model(model) else {
        return;
    };

    let until = Instant::now() + cooldown;
    let mut guard = websocket_cooldowns.write().await;
    guard.insert(key, until);
}

async fn clear_websocket_cooldown(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    model: &str,
) {
    let Some(key) = normalize_transport_model(model) else {
        return;
    };

    let mut guard = websocket_cooldowns.write().await;
    guard.remove(&key);
}

fn websocket_cooldown_for_streak(streak: u32) -> Duration {
    let base = WEBSOCKET_MODEL_COOLDOWN_BASE_SECS as u128;
    let max = WEBSOCKET_MODEL_COOLDOWN_MAX_SECS as u128;
    let shift = streak.saturating_sub(1).min(16);
    let scaled = base.saturating_mul(1u128 << shift);
    Duration::from_secs(scaled.min(max) as u64)
}

async fn record_websocket_fallback(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    websocket_failure_streaks: &Arc<RwLock<HashMap<String, u32>>>,
    model: &str,
) -> (u32, Duration) {
    let Some(key) = normalize_transport_model(model) else {
        return (0, Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS));
    };

    let streak = {
        let mut guard = websocket_failure_streaks.write().await;
        let entry = guard.entry(key).or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    };

    let cooldown = websocket_cooldown_for_streak(streak);
    set_websocket_cooldown_for(websocket_cooldowns, model, cooldown).await;
    (streak, cooldown)
}

async fn record_websocket_success(
    websocket_cooldowns: &Arc<RwLock<HashMap<String, Instant>>>,
    websocket_failure_streaks: &Arc<RwLock<HashMap<String, u32>>>,
    model: &str,
) {
    clear_websocket_cooldown(websocket_cooldowns, model).await;
    let Some(key) = normalize_transport_model(model) else {
        return;
    };
    let streak = {
        let mut guard = websocket_failure_streaks.write().await;
        guard.remove(&key).unwrap_or(0)
    };
    if streak > 0 {
        crate::logging::info(&format!(
            "OpenAI websocket health reset for model='{}' after successful stream (previous streak={})",
            model, streak
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::codex::CodexCredentials;
    use anyhow::Result;
    use std::collections::{HashMap, HashSet};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};
    use std::time::{Duration, Instant};
    const BRIGHT_PEARL_WRAPPED_TOOL_CALL_FIXTURE: &str =
        include_str!("../../tests/fixtures/openai/bright_pearl_wrapped_tool_call.txt");
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }

        fn set_path(key: &'static str, value: &std::path::Path) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                crate::env::set_var(self.key, previous);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    struct LiveOpenAITestEnv {
        _lock: MutexGuard<'static, ()>,
        _jcode_home: EnvVarGuard,
        _transport: EnvVarGuard,
        _temp: tempfile::TempDir,
    }

    impl LiveOpenAITestEnv {
        fn new() -> Result<Option<Self>> {
            let lock = ENV_LOCK.lock().unwrap();
            let Some(source_auth) = real_codex_auth_path() else {
                return Ok(None);
            };

            let temp = tempfile::Builder::new()
                .prefix("jcode-openai-live-")
                .tempdir()?;
            let target_auth = temp
                .path()
                .join("external")
                .join(".codex")
                .join("auth.json");
            std::fs::create_dir_all(
                target_auth
                    .parent()
                    .expect("temp auth target should have a parent"),
            )?;
            std::fs::copy(source_auth, &target_auth)?;

            let jcode_home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
            let transport = EnvVarGuard::set("JCODE_OPENAI_TRANSPORT", "https");

            Ok(Some(Self {
                _lock: lock,
                _jcode_home: jcode_home,
                _transport: transport,
                _temp: temp,
            }))
        }
    }

    fn real_codex_auth_path() -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        let path = home.join(".codex").join("auth.json");
        path.exists().then_some(path)
    }

    async fn live_openai_catalog() -> Result<Option<crate::provider::OpenAIModelCatalog>> {
        let Some(_env) = LiveOpenAITestEnv::new()? else {
            return Ok(None);
        };
        let creds = crate::auth::codex::load_credentials()?;
        if !OpenAIProvider::is_chatgpt_mode(&creds) {
            return Ok(None);
        }

        let token = openai_access_token(&Arc::new(RwLock::new(creds))).await?;
        Ok(Some(
            crate::provider::fetch_openai_model_catalog(&token).await?,
        ))
    }

    async fn live_openai_smoke(model: &str, sentinel: &str) -> Result<Option<String>> {
        let Some(_env) = LiveOpenAITestEnv::new()? else {
            return Ok(None);
        };
        let creds = crate::auth::codex::load_credentials()?;
        if !OpenAIProvider::is_chatgpt_mode(&creds) {
            return Ok(None);
        }

        let provider = OpenAIProvider::new(creds);
        provider.set_model(model)?;
        let response = provider
            .complete_simple(&format!("Reply with exactly {}.", sentinel), "")
            .await?;
        Ok(Some(response))
    }

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
        assert!(provider.available_models().contains(&"gpt-5.1-codex-mini"));

        provider.set_model("gpt-5.1-codex").unwrap();
        assert_eq!(provider.model(), "gpt-5.1-codex");

        provider.set_model("gpt-5.1-codex-mini").unwrap();
        assert_eq!(provider.model(), "gpt-5.1-codex-mini");
    }

    #[test]
    fn test_service_tier_can_be_changed_while_a_request_snapshot_is_held() {
        let provider = Arc::new(OpenAIProvider::new(CodexCredentials {
            access_token: "test".to_string(),
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            expires_at: None,
        }));

        let read_guard = provider
            .service_tier
            .read()
            .expect("service tier read lock should be available");

        let (tx, rx) = std::sync::mpsc::channel();
        let provider_for_write = Arc::clone(&provider);
        let handle = std::thread::spawn(move || {
            let result = provider_for_write.set_service_tier("priority");
            tx.send(result).expect("send result from setter thread");
        });

        std::thread::sleep(Duration::from_millis(20));
        assert!(
            rx.try_recv().is_err(),
            "writer should wait for the in-flight snapshot to finish"
        );

        drop(read_guard);

        rx.recv()
            .expect("receive service tier setter result")
            .expect("service tier update should succeed once read lock is released");
        handle.join().expect("join setter thread");

        assert_eq!(provider.service_tier(), Some("priority".to_string()));
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
                tool_duration_ms: None,
            },
            ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls"}),
                }],
                timestamp: None,
                tool_duration_ms: None,
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
                tool_duration_ms: None,
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
                tool_duration_ms: None,
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
    fn test_build_responses_input_keeps_image_context_after_tool_output() {
        let messages = vec![
            ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"file_path": "screenshot.png"}),
                }],
                timestamp: None,
                tool_duration_ms: None,
            },
            ChatMessage {
                role: Role::User,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "Image: screenshot.png\nImage sent to model for vision analysis."
                            .to_string(),
                        is_error: None,
                    },
                    ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: "ZmFrZQ==".to_string(),
                    },
                    ContentBlock::Text {
                        text: "[Attached image associated with the preceding tool result: screenshot.png]"
                            .to_string(),
                        cache_control: None,
                    },
                ],
                timestamp: None,
                tool_duration_ms: None,
            },
        ];

        let items = build_responses_input(&messages);
        let mut output_pos = None;
        let mut image_msg_pos = None;

        for (idx, item) in items.iter().enumerate() {
            match item.get("type").and_then(|v| v.as_str()) {
                Some("function_call_output")
                    if item.get("call_id").and_then(|v| v.as_str()) == Some("call_1") =>
                {
                    output_pos = Some(idx);
                    assert_eq!(
                        item.get("output").and_then(|v| v.as_str()),
                        Some("Image: screenshot.png\nImage sent to model for vision analysis.")
                    );
                }
                Some("message") if item.get("role").and_then(|v| v.as_str()) == Some("user") => {
                    let Some(content) = item.get("content").and_then(|v| v.as_array()) else {
                        continue;
                    };
                    let has_image = content.iter().any(|part| {
                        part.get("type").and_then(|v| v.as_str()) == Some("input_image")
                    });
                    let has_label = content.iter().any(|part| {
                        part.get("type").and_then(|v| v.as_str()) == Some("input_text")
                            && part
                                .get("text")
                                .and_then(|v| v.as_str())
                                .map(|text| text.contains("screenshot.png"))
                                .unwrap_or(false)
                    });
                    if has_image && has_label {
                        image_msg_pos = Some(idx);
                    }
                }
                _ => {}
            }
        }

        assert!(output_pos.is_some(), "expected function call output item");
        assert!(
            image_msg_pos.is_some(),
            "expected follow-up user image message"
        );
        assert!(
            image_msg_pos.unwrap() > output_pos.unwrap(),
            "image context should stay after the tool output"
        );
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
                tool_duration_ms: None,
            },
            ChatMessage {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_b".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "whoami"}),
                }],
                timestamp: None,
                tool_duration_ms: None,
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
        assert!(is_retryable_error(
            "OpenAI HTTPS stream ended before message completion marker"
        ));
    }

    #[test]
    fn test_parse_max_output_tokens_defaults_to_safe_value() {
        assert_eq!(
            OpenAIProvider::parse_max_output_tokens(None),
            Some(DEFAULT_MAX_OUTPUT_TOKENS)
        );
        assert_eq!(
            OpenAIProvider::parse_max_output_tokens(Some("")),
            Some(DEFAULT_MAX_OUTPUT_TOKENS)
        );
    }

    #[test]
    fn test_parse_max_output_tokens_allows_disable_and_override() {
        assert_eq!(OpenAIProvider::parse_max_output_tokens(Some("0")), None);
        assert_eq!(
            OpenAIProvider::parse_max_output_tokens(Some("32768")),
            Some(32768)
        );
        assert_eq!(
            OpenAIProvider::parse_max_output_tokens(Some("not-a-number")),
            Some(DEFAULT_MAX_OUTPUT_TOKENS)
        );
    }

    #[test]
    fn test_build_response_request_for_gpt_5_4_1m_uses_base_model_without_extra_flags() {
        let request = OpenAIProvider::build_response_request(
            "gpt-5.4",
            "system".to_string(),
            &[],
            &[],
            true,
            Some(DEFAULT_MAX_OUTPUT_TOKENS),
            Some("xhigh"),
            Some("unused"),
            Some("unused"),
            None,
        );

        assert_eq!(request["model"], serde_json::json!("gpt-5.4"));
        assert!(request.get("model_context_window").is_none());
        assert!(request.get("max_output_tokens").is_none());
        assert!(request.get("prompt_cache_key").is_none());
        assert!(request.get("prompt_cache_retention").is_none());
    }

    #[test]
    fn test_build_response_request_omits_long_context_for_plain_gpt_5_4() {
        let request = OpenAIProvider::build_response_request(
            "gpt-5.4",
            "system".to_string(),
            &[],
            &[],
            true,
            Some(DEFAULT_MAX_OUTPUT_TOKENS),
            None,
            None,
            None,
            None,
        );

        assert!(request.get("model_context_window").is_none());
    }

    #[tokio::test]
    #[ignore = "requires real OpenAI OAuth credentials"]
    async fn live_openai_catalog_lists_gpt_5_4_family() -> Result<()> {
        let Some(catalog) = live_openai_catalog().await? else {
            eprintln!("skipping live OpenAI catalog test: no real OAuth credentials");
            return Ok(());
        };

        crate::provider::populate_context_limits(catalog.context_limits.clone());
        crate::provider::populate_account_models(catalog.available_models.clone());

        assert!(
            catalog
                .available_models
                .iter()
                .any(|model| model.starts_with("gpt-5.4")),
            "expected GPT-5.4 family in live catalog, got {:?}",
            catalog.available_models
        );
        assert!(
            crate::provider::known_openai_model_ids()
                .iter()
                .any(|model| model == "gpt-5.4"),
            "expected GPT-5.4 in display model list"
        );

        let reports_long_context = catalog
            .context_limits
            .get("gpt-5.4")
            .copied()
            .unwrap_or_default()
            >= 1_000_000;
        assert_eq!(
            crate::provider::known_openai_model_ids()
                .iter()
                .any(|model| model == "gpt-5.4[1m]"),
            reports_long_context,
            "displayed 1m alias should follow the live catalog"
        );

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires real OpenAI OAuth credentials"]
    async fn live_openai_gpt_5_4_and_fast_requests_succeed() -> Result<()> {
        let Some(catalog) = live_openai_catalog().await? else {
            eprintln!("skipping live OpenAI response test: no real OAuth credentials");
            return Ok(());
        };
        crate::provider::populate_context_limits(catalog.context_limits.clone());
        crate::provider::populate_account_models(catalog.available_models.clone());

        let Some(plain_response) = live_openai_smoke("gpt-5.4", "JCODE_GPT54_OK").await? else {
            eprintln!("skipping live OpenAI response test: no real OAuth credentials");
            return Ok(());
        };
        assert!(
            plain_response.contains("JCODE_GPT54_OK"),
            "unexpected GPT-5.4 response: {}",
            plain_response
        );

        if catalog
            .available_models
            .iter()
            .any(|model| model == "gpt-5.3-codex-spark")
        {
            let Some(fast_response) =
                live_openai_smoke("gpt-5.3-codex-spark", "JCODE_GPT53_SPARK_OK").await?
            else {
                eprintln!("skipping live OpenAI fast-model test: no real OAuth credentials");
                return Ok(());
            };
            assert!(
                fast_response.contains("JCODE_GPT53_SPARK_OK"),
                "unexpected gpt-5.3-codex-spark response: {}",
                fast_response
            );
        }

        if crate::provider::known_openai_model_ids()
            .iter()
            .any(|model| model == "gpt-5.4[1m]")
        {
            let Some(long_context_response) =
                live_openai_smoke("gpt-5.4[1m]", "JCODE_GPT54_1M_OK").await?
            else {
                eprintln!("skipping live OpenAI 1m test: no real OAuth credentials");
                return Ok(());
            };
            assert!(
                long_context_response.contains("JCODE_GPT54_1M_OK"),
                "unexpected GPT-5.4[1m] response: {}",
                long_context_response
            );
        }

        Ok(())
    }

    #[test]
    fn test_should_prefer_websocket_enabled_for_named_models() {
        assert!(OpenAIProvider::should_prefer_websocket(
            "gpt-5.3-codex-spark"
        ));
        assert!(OpenAIProvider::should_prefer_websocket("gpt-5.3-codex"));
        assert!(OpenAIProvider::should_prefer_websocket("gpt-5"));
        assert!(OpenAIProvider::should_prefer_websocket("codex-mini"));
        assert!(!OpenAIProvider::should_prefer_websocket(""));
    }

    #[test]
    fn test_openai_transport_mode_defaults_to_auto() {
        let mode = OpenAITransportMode::from_config(None);
        assert_eq!(mode.as_str(), "auto");
    }

    #[test]
    fn test_openai_transport_mode_auto_prefers_websocket_for_openai_models() {
        let mode = OpenAITransportMode::from_config(Some("auto"));
        assert_eq!(mode.as_str(), "auto");
        assert!(OpenAIProvider::should_prefer_websocket("gpt-5.4"));
    }

    #[tokio::test]
    async fn test_record_websocket_fallback_sets_cooldown_for_auto_default_models() {
        let cooldowns = Arc::new(RwLock::new(HashMap::new()));
        let streaks = Arc::new(RwLock::new(HashMap::new()));
        let model = "gpt-5.4";

        let (streak, cooldown) = record_websocket_fallback(&cooldowns, &streaks, model).await;
        assert_eq!(streak, 1);
        assert_eq!(
            cooldown,
            Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS)
        );
        assert!(
            websocket_cooldown_remaining(&cooldowns, model)
                .await
                .is_some(),
            "auto websocket default must still be guarded by cooldown after fallback"
        );
    }

    #[tokio::test]
    async fn test_websocket_cooldown_helpers_set_clear_and_expire() {
        let cooldowns = Arc::new(RwLock::new(HashMap::new()));
        let model = "gpt-5.3-codex";

        assert!(
            websocket_cooldown_remaining(&cooldowns, model)
                .await
                .is_none()
        );

        set_websocket_cooldown(&cooldowns, model).await;
        let remaining = websocket_cooldown_remaining(&cooldowns, model).await;
        assert!(remaining.is_some());

        clear_websocket_cooldown(&cooldowns, model).await;
        assert!(
            websocket_cooldown_remaining(&cooldowns, model)
                .await
                .is_none()
        );

        {
            let mut guard = cooldowns.write().await;
            guard.insert(model.to_string(), Instant::now() - Duration::from_secs(1));
        }
        assert!(
            websocket_cooldown_remaining(&cooldowns, model)
                .await
                .is_none()
        );
        assert!(!cooldowns.read().await.contains_key(model));
    }

    #[test]
    fn test_websocket_cooldown_for_streak_scales_and_caps() {
        assert_eq!(
            websocket_cooldown_for_streak(1),
            Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS)
        );
        assert_eq!(
            websocket_cooldown_for_streak(2),
            Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS * 2)
        );
        assert_eq!(
            websocket_cooldown_for_streak(3),
            Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS * 4)
        );
        assert_eq!(
            websocket_cooldown_for_streak(32),
            Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_MAX_SECS)
        );
    }

    #[tokio::test]
    async fn test_record_websocket_fallback_tracks_streak_and_cooldown() {
        let cooldowns = Arc::new(RwLock::new(HashMap::new()));
        let streaks = Arc::new(RwLock::new(HashMap::new()));
        let model = "gpt-5.3-codex-spark";

        let (streak1, cooldown1) = record_websocket_fallback(&cooldowns, &streaks, model).await;
        assert_eq!(streak1, 1);
        assert_eq!(
            cooldown1,
            Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS)
        );
        let remaining1 = websocket_cooldown_remaining(&cooldowns, model)
            .await
            .expect("cooldown should be set");
        assert!(remaining1 <= cooldown1);

        let (streak2, cooldown2) = record_websocket_fallback(&cooldowns, &streaks, model).await;
        assert_eq!(streak2, 2);
        assert_eq!(
            cooldown2,
            Duration::from_secs(WEBSOCKET_MODEL_COOLDOWN_BASE_SECS * 2)
        );
        let remaining2 = websocket_cooldown_remaining(&cooldowns, model)
            .await
            .expect("cooldown should be set");
        assert!(remaining2 <= cooldown2);

        record_websocket_success(&cooldowns, &streaks, model).await;
        assert!(
            websocket_cooldown_remaining(&cooldowns, model)
                .await
                .is_none()
        );
        let normalized = normalize_transport_model(model).expect("normalized model");
        assert!(!streaks.read().await.contains_key(&normalized));
    }

    #[test]
    fn test_websocket_activity_payload_detection() {
        assert!(is_websocket_activity_payload(
            r#"{"type":"response.created","response":{"id":"resp_1"}}"#
        ));
        assert!(is_websocket_activity_payload(
            r#"{"type":"response.reasoning.delta","delta":"thinking"}"#
        ));
        assert!(!is_websocket_activity_payload("not json"));
        assert!(!is_websocket_activity_payload(r#"{"foo":"bar"}"#));
    }

    #[test]
    fn test_websocket_first_activity_payload_counts_typed_control_events() {
        assert!(is_websocket_first_activity_payload(
            r#"{"type":"rate_limits.updated"}"#
        ));
        assert!(is_websocket_first_activity_payload(
            r#"{"type":"session.created","session":{}}"#
        ));
        assert!(!is_websocket_first_activity_payload(r#"{"foo":"bar"}"#));
        assert!(!is_websocket_first_activity_payload("not json"));
    }

    #[test]
    fn test_websocket_completion_timeout_is_long_enough_for_reasoning() {
        assert!(
            WEBSOCKET_COMPLETION_TIMEOUT_SECS >= 120,
            "completion timeout regressed to {}s; reasoning models may need several minutes",
            WEBSOCKET_COMPLETION_TIMEOUT_SECS
        );
    }

    #[test]
    fn test_stream_activity_event_treats_any_stream_event_as_activity() {
        assert!(is_stream_activity_event(&StreamEvent::ThinkingStart));
        assert!(is_stream_activity_event(&StreamEvent::ThinkingDelta(
            "working".to_string()
        )));
        assert!(is_stream_activity_event(&StreamEvent::TextDelta(
            "hello".to_string()
        )));
        assert!(is_stream_activity_event(&StreamEvent::MessageEnd {
            stop_reason: None
        }));
    }

    #[test]
    fn test_websocket_activity_payload_counts_response_completed() {
        assert!(is_websocket_activity_payload(
            r#"{"type":"response.completed","response":{"status":"completed"}}"#
        ));
    }

    #[test]
    fn test_websocket_activity_payload_counts_in_progress_events() {
        assert!(is_websocket_activity_payload(
            r#"{"type":"response.in_progress","response":{"status":"in_progress"}}"#
        ));
    }

    #[test]
    fn test_websocket_activity_payload_ignores_non_response_events() {
        assert!(!is_websocket_activity_payload(
            r#"{"type":"session.created","session":{}}"#
        ));
        assert!(!is_websocket_activity_payload(
            r#"{"type":"rate_limits.updated"}"#
        ));
        assert!(!is_websocket_activity_payload(r#"not json at all"#));
    }

    #[test]
    fn test_websocket_remaining_timeout_secs_uses_idle_time_budget() {
        let recent = Instant::now() - Duration::from_secs(2);
        let remaining = websocket_remaining_timeout_secs(recent, 8).expect("still within budget");
        assert!(
            (6..=7).contains(&remaining),
            "expected remaining idle budget near 6-7s, got {remaining}"
        );
    }

    #[test]
    fn test_websocket_remaining_timeout_secs_expires_after_budget() {
        let expired = Instant::now() - Duration::from_secs(9);
        assert!(websocket_remaining_timeout_secs(expired, 8).is_none());
    }

    #[test]
    fn test_websocket_next_activity_timeout_uses_request_start_before_first_event() {
        let ws_started_at = Instant::now() - Duration::from_secs(3);
        let last_api_activity_at = Instant::now() - Duration::from_secs(1);
        let remaining =
            websocket_next_activity_timeout_secs(ws_started_at, last_api_activity_at, false)
                .expect("first-event timeout should still be active");
        assert!(
            (5..=6).contains(&remaining),
            "expected first-event timeout near 5-6s, got {remaining}"
        );
    }

    #[test]
    fn test_websocket_next_activity_timeout_resets_after_api_activity() {
        let ws_started_at = Instant::now() - Duration::from_secs(299);
        let last_api_activity_at = Instant::now() - Duration::from_secs(2);
        let remaining =
            websocket_next_activity_timeout_secs(ws_started_at, last_api_activity_at, true)
                .expect("idle timeout should use last activity, not total request age");
        assert!(
            remaining >= WEBSOCKET_COMPLETION_TIMEOUT_SECS.saturating_sub(3),
            "expected full idle budget to reset after activity, got {remaining}"
        );
    }

    #[test]
    fn test_websocket_activity_timeout_kind_labels_first_and_next() {
        assert_eq!(websocket_activity_timeout_kind(false), "first");
        assert_eq!(websocket_activity_timeout_kind(true), "next");
    }

    #[test]
    fn test_normalize_transport_model_trims_and_lowercases() {
        assert_eq!(
            normalize_transport_model("  GPT-5.4  "),
            Some("gpt-5.4".to_string())
        );
        assert_eq!(normalize_transport_model("   \t\n  "), None);
    }

    #[tokio::test]
    async fn test_record_websocket_success_clears_normalized_keys() {
        let cooldowns = Arc::new(RwLock::new(HashMap::new()));
        let streaks = Arc::new(RwLock::new(HashMap::new()));
        let canonical = "gpt-5.4";

        record_websocket_fallback(&cooldowns, &streaks, canonical).await;
        assert!(
            websocket_cooldown_remaining(&cooldowns, canonical)
                .await
                .is_some()
        );

        record_websocket_success(&cooldowns, &streaks, " GPT-5.4 ").await;

        assert!(
            websocket_cooldown_remaining(&cooldowns, canonical)
                .await
                .is_none(),
            "success should clear normalized cooldown entries"
        );
        assert!(
            !streaks.read().await.contains_key(canonical),
            "success should clear normalized failure streak entries"
        );
    }

    #[test]
    fn test_build_response_request_includes_stream_for_http() {
        let request = OpenAIProvider::build_response_request(
            "gpt-5.4",
            "system".to_string(),
            &[],
            &[],
            false,
            Some(DEFAULT_MAX_OUTPUT_TOKENS),
            None,
            None,
            None,
            None,
        );
        assert_eq!(request["stream"], serde_json::json!(true));
        assert_eq!(request["store"], serde_json::json!(false));
    }

    #[test]
    fn test_websocket_payload_strips_stream_and_background() {
        let mut request = OpenAIProvider::build_response_request(
            "gpt-5.4",
            "system".to_string(),
            &[serde_json::json!({"role": "user", "content": "hello"})],
            &[],
            false,
            Some(DEFAULT_MAX_OUTPUT_TOKENS),
            None,
            None,
            None,
            None,
        );

        assert_eq!(request["stream"], serde_json::json!(true));

        request["background"] = serde_json::json!(true);

        let obj = request.as_object_mut().expect("request is object");
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("response.create".to_string()),
        );
        obj.remove("stream");
        obj.remove("background");

        assert!(
            request.get("stream").is_none(),
            "stream must be stripped for WebSocket payloads"
        );
        assert!(
            request.get("background").is_none(),
            "background must be stripped for WebSocket payloads"
        );
        assert_eq!(request["type"], serde_json::json!("response.create"));
    }

    #[test]
    fn test_websocket_payload_preserves_required_fields() {
        let mut request = OpenAIProvider::build_response_request(
            "gpt-5.4",
            "system prompt".to_string(),
            &[serde_json::json!({"role": "user", "content": "hello"})],
            &[serde_json::json!({"type": "function", "name": "bash"})],
            false,
            Some(16384),
            Some("high"),
            None,
            None,
            None,
        );

        let obj = request.as_object_mut().expect("request is object");
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("response.create".to_string()),
        );
        obj.remove("stream");
        obj.remove("background");

        assert_eq!(request["type"], "response.create");
        assert_eq!(request["model"], "gpt-5.4");
        assert_eq!(request["instructions"], "system prompt");
        assert!(request["input"].is_array());
        assert!(request["tools"].is_array());
        assert_eq!(request["max_output_tokens"], serde_json::json!(16384));
        assert_eq!(request["reasoning"], serde_json::json!({"effort": "high"}));
        assert_eq!(request["tool_choice"], "auto");
    }

    #[test]
    fn test_websocket_continuation_request_excludes_transport_fields() {
        let base_request = OpenAIProvider::build_response_request(
            "gpt-5.4",
            "system".to_string(),
            &[],
            &[serde_json::json!({"type": "function", "name": "bash"})],
            false,
            Some(DEFAULT_MAX_OUTPUT_TOKENS),
            None,
            None,
            None,
            None,
        );

        let mut continuation = serde_json::json!({
            "type": "response.create",
            "previous_response_id": "resp_abc123",
            "input": [{"role": "user", "content": "follow up"}],
        });

        if let Some(model) = base_request.get("model") {
            continuation["model"] = model.clone();
        }
        if let Some(tools) = base_request.get("tools") {
            continuation["tools"] = tools.clone();
        }
        if let Some(instructions) = base_request.get("instructions") {
            continuation["instructions"] = instructions.clone();
        }
        continuation["store"] = serde_json::json!(false);
        continuation["parallel_tool_calls"] = serde_json::json!(false);

        assert!(
            continuation.get("stream").is_none(),
            "continuation request must not include stream"
        );
        assert!(
            continuation.get("background").is_none(),
            "continuation request must not include background"
        );
        assert_eq!(continuation["type"], "response.create");
        assert_eq!(continuation["previous_response_id"], "resp_abc123");
        assert_eq!(continuation["model"], "gpt-5.4");
    }

    #[test]
    fn test_parse_openai_response_completed_captures_incomplete_stop_reason() {
        let data = r#"{"type":"response.completed","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#;
        let mut saw_text_delta = false;
        let mut streaming_tool_calls = HashMap::new();
        let mut completed_tool_items = HashSet::new();
        let mut pending = VecDeque::new();

        let event = parse_openai_response_event(
            data,
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        )
        .expect("expected message end");
        match event {
            StreamEvent::MessageEnd { stop_reason } => {
                assert_eq!(stop_reason.as_deref(), Some("max_output_tokens"));
            }
            other => panic!("expected MessageEnd, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_openai_response_completed_without_stop_reason() {
        let data = r#"{"type":"response.completed","response":{"status":"completed"}}"#;
        let mut saw_text_delta = false;
        let mut streaming_tool_calls = HashMap::new();
        let mut completed_tool_items = HashSet::new();
        let mut pending = VecDeque::new();

        let event = parse_openai_response_event(
            data,
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        )
        .expect("expected message end");
        match event {
            StreamEvent::MessageEnd { stop_reason } => {
                assert!(stop_reason.is_none());
            }
            other => panic!("expected MessageEnd, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_openai_response_completed_commentary_phase_sets_stop_reason() {
        let data = r#"{"type":"response.completed","response":{"status":"completed","output":[{"type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"Still working"}]}]}}"#;
        let mut saw_text_delta = false;
        let mut streaming_tool_calls = HashMap::new();
        let mut completed_tool_items = HashSet::new();
        let mut pending = VecDeque::new();

        let event = parse_openai_response_event(
            data,
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        )
        .expect("expected message end");
        match event {
            StreamEvent::MessageEnd { stop_reason } => {
                assert_eq!(stop_reason.as_deref(), Some("commentary"));
            }
            other => panic!("expected MessageEnd, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_openai_response_incomplete_emits_message_end_with_reason() {
        let data = r#"{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"content_filter"}}}"#;
        let mut saw_text_delta = false;
        let mut streaming_tool_calls = HashMap::new();
        let mut completed_tool_items = HashSet::new();
        let mut pending = VecDeque::new();

        let event = parse_openai_response_event(
            data,
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        )
        .expect("expected message end");
        match event {
            StreamEvent::MessageEnd { stop_reason } => {
                assert_eq!(stop_reason.as_deref(), Some("content_filter"));
            }
            other => panic!("expected MessageEnd, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_openai_response_function_call_arguments_streaming() {
        let mut saw_text_delta = false;
        let mut streaming_tool_calls = HashMap::new();
        let mut completed_tool_items = HashSet::new();
        let mut pending = VecDeque::new();

        let added = r#"{"type":"response.output_item.added","item":{"id":"fc_123","type":"function_call","call_id":"call_123","name":"batch","arguments":""}}"#;
        assert!(
            parse_openai_response_event(
                added,
                &mut saw_text_delta,
                &mut streaming_tool_calls,
                &mut completed_tool_items,
                &mut pending,
            )
            .is_none(),
            "output_item.added should just seed tool state"
        );

        let delta = r#"{"type":"response.function_call_arguments.delta","item_id":"fc_123","delta":"{\"tool_calls\":[{\"tool\":\"read\"}]"}"#;
        assert!(
            parse_openai_response_event(
                delta,
                &mut saw_text_delta,
                &mut streaming_tool_calls,
                &mut completed_tool_items,
                &mut pending,
            )
            .is_none(),
            "argument delta should accumulate state only"
        );

        let done = r#"{"type":"response.function_call_arguments.done","item_id":"fc_123","arguments":"{\"tool_calls\":[{\"tool\":\"read\"}]}"}"#;
        let first = parse_openai_response_event(
            done,
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        )
        .expect("expected tool start");

        match first {
            StreamEvent::ToolUseStart { id, name } => {
                assert_eq!(id, "call_123");
                assert_eq!(name, "batch");
            }
            other => panic!("expected ToolUseStart, got {:?}", other),
        }

        match pending.pop_front() {
            Some(StreamEvent::ToolInputDelta(delta)) => {
                let parsed: Value = serde_json::from_str(&delta).expect("valid args json");
                let tool_calls = parsed
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .expect("tool_calls array");
                assert_eq!(tool_calls.len(), 1);
            }
            other => panic!("expected ToolInputDelta, got {:?}", other),
        }

        assert!(matches!(pending.pop_front(), Some(StreamEvent::ToolUseEnd)));
        assert!(streaming_tool_calls.is_empty());
        assert!(completed_tool_items.contains("fc_123"));
    }

    #[test]
    fn test_parse_openai_response_output_item_done_skips_duplicate_after_arguments_done() {
        let mut saw_text_delta = false;
        let mut streaming_tool_calls = HashMap::new();
        let mut completed_tool_items = HashSet::from(["fc_123".to_string()]);
        let mut pending = VecDeque::new();

        let duplicate_done = r#"{"type":"response.output_item.done","item":{"id":"fc_123","type":"function_call","call_id":"call_123","name":"batch","arguments":"{\"tool_calls\":[]}"}}"#;
        let event = parse_openai_response_event(
            duplicate_done,
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        );

        assert!(event.is_none(), "duplicate function call should be skipped");
        assert!(pending.is_empty());
        assert!(!completed_tool_items.contains("fc_123"));
    }

    #[test]
    fn test_build_tools_sets_strict_true() {
        let defs = vec![ToolDefinition {
            name: "bash".to_string(),
            description: "run shell".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["command"],
                "properties": { "command": { "type": "string" } }
            }),
        }];
        let api_tools = build_tools(&defs);
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0]["strict"], serde_json::json!(true));
    }

    #[test]
    fn test_build_tools_disables_strict_for_free_form_object_nodes() {
        let defs = vec![ToolDefinition {
            name: "batch".to_string(),
            description: "batch calls".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["tool_calls"],
                "properties": {
                    "tool_calls": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["tool", "parameters"],
                            "properties": {
                                "tool": { "type": "string" },
                                "parameters": { "type": "object" }
                            }
                        }
                    }
                }
            }),
        }];
        let api_tools = build_tools(&defs);
        assert_eq!(api_tools.len(), 1);
        assert_eq!(api_tools[0]["strict"], serde_json::json!(false));
        assert_eq!(
            api_tools[0]["parameters"]["properties"]["tool_calls"]["items"]["properties"]["parameters"]
                ["type"],
            serde_json::json!("object")
        );
    }

    #[test]
    fn test_build_tools_normalizes_object_schema_additional_properties() {
        let defs = vec![ToolDefinition {
            name: "edit".to_string(),
            description: "apply edit".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "options": {
                        "type": "object",
                        "properties": {
                            "force": { "type": "boolean" }
                        }
                    },
                    "description": {
                        "type": "string"
                    }
                },
                "required": ["path"]
            }),
        }];
        let api_tools = build_tools(&defs);
        assert_eq!(
            api_tools[0]["parameters"]["additionalProperties"],
            serde_json::json!(false)
        );
        assert_eq!(
            api_tools[0]["parameters"]["properties"]["options"]["additionalProperties"],
            serde_json::json!(false)
        );
        assert_eq!(
            api_tools[0]["parameters"]["required"],
            serde_json::json!(["description", "options", "path"])
        );
        assert_eq!(
            api_tools[0]["parameters"]["properties"]["description"]["type"],
            serde_json::json!(["string", "null"])
        );
    }

    #[test]
    fn test_build_tools_rewrites_oneof_to_anyof_for_openai() {
        let defs = vec![ToolDefinition {
            name: "batch".to_string(),
            description: "batch calls".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["tool_calls"],
                "properties": {
                    "tool_calls": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "required": ["tool"],
                                    "properties": {
                                        "tool": { "type": "string" }
                                    }
                                }
                            ]
                        }
                    }
                }
            }),
        }];
        let api_tools = build_tools(&defs);
        assert!(api_tools[0]["parameters"]["properties"]["tool_calls"]["items"]["oneOf"].is_null());
        assert_eq!(
            api_tools[0]["parameters"]["properties"]["tool_calls"]["items"]["anyOf"][0]["type"],
            serde_json::json!("object")
        );
    }

    #[test]
    fn test_build_tools_keeps_strict_for_anyof_object_branches_with_properties() {
        let defs = vec![ToolDefinition {
            name: "schedule".to_string(),
            description: "schedule work".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["task"],
                "anyOf": [
                    {
                        "type": "object",
                        "required": ["wake_in_minutes"],
                        "properties": {
                            "wake_in_minutes": { "type": "integer" }
                        },
                        "additionalProperties": false
                    },
                    {
                        "type": "object",
                        "required": ["wake_at"],
                        "properties": {
                            "wake_at": { "type": "string" }
                        },
                        "additionalProperties": false
                    }
                ],
                "properties": {
                    "task": { "type": "string" },
                    "wake_in_minutes": { "type": "integer" },
                    "wake_at": { "type": "string" }
                }
            }),
        }];
        let api_tools = build_tools(&defs);
        assert_eq!(api_tools[0]["strict"], serde_json::json!(true));
        assert_eq!(
            api_tools[0]["parameters"]["anyOf"][0]["additionalProperties"],
            serde_json::json!(false)
        );
        assert_eq!(
            api_tools[0]["parameters"]["anyOf"][1]["additionalProperties"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn test_parse_text_wrapped_tool_call_prefers_trailing_json_object() {
        let text = "Status update\nassistant to=functions.batch commentary {}json\n{\"tool_calls\":[{\"tool\":\"read\",\"file_path\":\"src/main.rs\"}]}";
        let parsed = parse_text_wrapped_tool_call(text).expect("should parse wrapped tool call");
        assert_eq!(parsed.1, "batch");
        assert!(parsed.0.contains("Status update"));
        let args: Value = serde_json::from_str(&parsed.2).expect("valid args json");
        assert!(args.get("tool_calls").is_some());
    }

    #[test]
    fn test_handle_openai_output_item_normalizes_null_arguments() {
        let item = serde_json::json!({
            "type": "function_call",
            "call_id": "call_1",
            "name": "bash",
            "arguments": "null",
        });
        let mut saw_text_delta = false;
        let mut pending = VecDeque::new();
        let first = handle_openai_output_item(item, &mut saw_text_delta, &mut pending)
            .expect("expected tool event");

        match first {
            StreamEvent::ToolUseStart { id, name } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "bash");
            }
            _ => panic!("expected ToolUseStart"),
        }
        match pending.pop_front() {
            Some(StreamEvent::ToolInputDelta(delta)) => assert_eq!(delta, "{}"),
            _ => panic!("expected ToolInputDelta"),
        }
        assert!(matches!(pending.pop_front(), Some(StreamEvent::ToolUseEnd)));
    }

    #[test]
    fn test_handle_openai_output_item_recovers_bright_pearl_fixture() {
        let item = serde_json::json!({
            "type": "message",
            "content": [{
                "type": "output_text",
                "text": BRIGHT_PEARL_WRAPPED_TOOL_CALL_FIXTURE,
            }],
        });

        let mut saw_text_delta = false;
        let mut pending = VecDeque::new();
        let mut events = Vec::new();

        if let Some(first) = handle_openai_output_item(item, &mut saw_text_delta, &mut pending) {
            events.push(first);
        }
        while let Some(ev) = pending.pop_front() {
            events.push(ev);
        }

        let mut saw_prefix = false;
        let mut saw_tool = false;
        let mut saw_input = false;

        for event in events {
            match event {
                StreamEvent::TextDelta(text) => {
                    if text.contains("Status: I detected pre-existing local edits") {
                        saw_prefix = true;
                    }
                }
                StreamEvent::ToolUseStart { name, .. } => {
                    if name == "batch" {
                        saw_tool = true;
                    }
                }
                StreamEvent::ToolInputDelta(delta) => {
                    let args: Value = serde_json::from_str(&delta).expect("valid tool args");
                    let calls = args
                        .get("tool_calls")
                        .and_then(|v| v.as_array())
                        .expect("tool_calls array");
                    assert_eq!(calls.len(), 3);
                    saw_input = true;
                }
                _ => {}
            }
        }

        assert!(saw_prefix);
        assert!(saw_tool);
        assert!(saw_input);
    }

    #[test]
    fn test_build_responses_input_rewrites_orphan_tool_output_as_user_message() {
        let messages = vec![ChatMessage::tool_result(
            "call_orphan",
            "orphan result",
            false,
        )];

        let items = build_responses_input(&messages);
        let mut saw_rewritten_message = false;

        for item in &items {
            assert_ne!(
                item.get("type").and_then(|v| v.as_str()),
                Some("function_call_output")
            );
            if item.get("type").and_then(|v| v.as_str()) == Some("message")
                && item.get("role").and_then(|v| v.as_str()) == Some("user")
            {
                if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                    for part in content {
                        if part.get("type").and_then(|v| v.as_str()) == Some("input_text") {
                            let text = part.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if text.contains("[Recovered orphaned tool output: call_orphan]")
                                && text.contains("orphan result")
                            {
                                saw_rewritten_message = true;
                            }
                        }
                    }
                }
            }
        }

        assert!(saw_rewritten_message);
    }

    #[test]
    fn test_extract_selfdev_section_missing_returns_none() {
        let system = "# Environment\nDate: 2026-01-01\n\n# Available Skills\n- test";
        assert!(extract_selfdev_section(system).is_none());
    }

    #[test]
    fn test_extract_selfdev_section_stops_at_next_top_level_header() {
        let system = "# Environment\nDate: 2026-01-01\n\n# Self-Development Mode\nUse selfdev tool\n## selfdev Tool\nreload\n\n# Available Skills\n- test";
        let section = extract_selfdev_section(system).expect("expected self-dev section");
        assert!(section.starts_with("# Self-Development Mode"));
        assert!(section.contains("Use selfdev tool"));
        assert!(section.contains("## selfdev Tool"));
        assert!(!section.contains("# Available Skills"));
    }

    #[test]
    fn test_chatgpt_instructions_with_selfdev_appends_selfdev_block() {
        let system = "# Environment\nDate: 2026-01-01\n\n# Self-Development Mode\nUse selfdev tool\n\n# Available Skills\n- test";

        let instructions = OpenAIProvider::chatgpt_instructions_with_selfdev(system);
        assert!(instructions.contains("You are in the Jcode harness"));
        assert!(instructions.contains("# Self-Development Mode"));
        assert!(instructions.contains("Use selfdev tool"));
    }
}
