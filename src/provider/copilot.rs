use super::{EventStream, Provider};
use crate::auth::copilot as copilot_auth;
use crate::message::{
    ContentBlock, Message as ChatMessage, Role, StreamEvent, ToolDefinition,
    TOOL_OUTPUT_MISSING_TEXT,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

const FALLBACK_MODELS: &[&str] = &[
    "claude-sonnet-4",
    "claude-sonnet-4-6",
    "gpt-4o",
    "gpt-4.1",
    "gpt-5",
    "o3-mini",
    "o4-mini",
    "gemini-2.0-flash-001",
    "gemini-2.5-pro",
];

/// Context window sizes for Copilot models
fn copilot_context_window(model: &str) -> usize {
    match model {
        "claude-sonnet-4" | "claude-sonnet-4-6" | "claude-sonnet-4.6" => 128_000,
        "claude-opus-4-6" | "claude-opus-4.6" | "claude-opus-4.6-fast" => 200_000,
        "claude-opus-4.5" | "claude-opus-4-5" => 200_000,
        "claude-sonnet-4.5" | "claude-sonnet-4-5" => 200_000,
        "claude-haiku-4.5" | "claude-haiku-4-5" => 200_000,
        "gpt-4o" | "gpt-4o-mini" => 128_000,
        m if m.starts_with("gpt-4o") => 128_000,
        m if m.starts_with("gpt-4.1") => 128_000,
        m if m.starts_with("gpt-5") => 128_000,
        "o3-mini" | "o4-mini" => 128_000,
        m if m.starts_with("gemini-2.0-flash") => 1_000_000,
        m if m.starts_with("gemini-2.5") => 1_000_000,
        m if m.starts_with("gemini-3") => 1_000_000,
        _ => 128_000,
    }
}

/// Copilot API provider - uses GitHub Copilot's OpenAI-compatible API.
/// Authenticates via GitHub OAuth token, exchanges for Copilot bearer token,
/// and sends requests to api.githubcopilot.com.
pub struct CopilotApiProvider {
    client: reqwest::Client,
    model: Arc<RwLock<String>>,
    github_token: String,
    bearer_token: Arc<tokio::sync::RwLock<Option<copilot_auth::CopilotApiToken>>>,
    fetched_models: Arc<RwLock<Vec<String>>>,
}

impl CopilotApiProvider {
    pub fn new() -> Result<Self> {
        let github_token = copilot_auth::load_github_token()?;
        let model = std::env::var("JCODE_COPILOT_MODEL")
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Ok(Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            github_token,
            bearer_token: Arc::new(tokio::sync::RwLock::new(None)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
        })
    }

    pub fn has_credentials() -> bool {
        copilot_auth::has_copilot_credentials()
    }

    pub fn new_with_token(github_token: String) -> Self {
        let model = std::env::var("JCODE_COPILOT_MODEL")
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(RwLock::new(model)),
            github_token,
            bearer_token: Arc::new(tokio::sync::RwLock::new(None)),
            fetched_models: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Detect the user's Copilot tier and set the best default model.
    /// Call this after construction. Fetches a bearer token and queries /models.
    /// If JCODE_COPILOT_MODEL is set, this is a no-op (user override).
    pub async fn detect_tier_and_set_default(&self) {
        if std::env::var("JCODE_COPILOT_MODEL").is_ok() {
            crate::logging::info("Copilot model overridden via JCODE_COPILOT_MODEL, skipping tier detection");
            return;
        }

        let bearer = match self.get_bearer_token().await {
            Ok(t) => t,
            Err(e) => {
                crate::logging::info(&format!("Copilot tier detection: failed to get bearer token: {}", e));
                return;
            }
        };

        match copilot_auth::fetch_available_models(&self.client, &bearer).await {
            Ok(models) => {
                let model_ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
                let default = copilot_auth::choose_default_model(&models);
                crate::logging::info(&format!(
                    "Copilot tier detection: {} models available, default -> {}. Models: {}",
                    model_ids.len(),
                    default,
                    model_ids.join(", ")
                ));
                if let Ok(mut m) = self.model.try_write() {
                    *m = default;
                }
                if let Ok(mut fm) = self.fetched_models.try_write() {
                    *fm = model_ids;
                }
            }
            Err(e) => {
                crate::logging::info(&format!("Copilot tier detection: failed to fetch models: {}", e));
            }
        }
    }

    /// Get a valid Copilot bearer token, refreshing if expired
    async fn get_bearer_token(&self) -> Result<String> {
        {
            let guard = self.bearer_token.read().await;
            if let Some(ref token) = *guard {
                if !token.is_expired() {
                    return Ok(token.token.clone());
                }
            }
        }

        // Need to refresh
        let new_token =
            copilot_auth::exchange_github_token(&self.client, &self.github_token).await?;
        let token_str = new_token.token.clone();
        *self.bearer_token.write().await = Some(new_token);
        Ok(token_str)
    }

    /// Check if an error indicates token expiration
    fn is_auth_error(status: reqwest::StatusCode) -> bool {
        status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
    }

    /// Build OpenAI-compatible messages array from our message format
    fn build_messages(system: &str, messages: &[ChatMessage]) -> Vec<Value> {
        let mut result = Vec::new();

        // System message
        if !system.is_empty() {
            result.push(json!({
                "role": "system",
                "content": system,
            }));
        }

        for msg in messages {
            match msg.role {
                Role::User => {
                    let text = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    result.push(json!({
                        "role": "user",
                        "content": text,
                    }));
                }
                Role::Assistant => {
                    let mut content_text = String::new();
                    let mut tool_calls = Vec::new();

                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                content_text.push_str(text);
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                tool_calls.push(json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": input.to_string(),
                                    }
                                }));
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

                    result.push(assistant_msg);
                }
                _ => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } = block
                        {
                            let text = if content.is_empty() {
                                TOOL_OUTPUT_MISSING_TEXT.to_string()
                            } else {
                                content.clone()
                            };
                            result.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": text,
                            }));
                        }
                    }
                }
            }
        }

        result
    }

    /// Build OpenAI-compatible tools array
    fn build_tools(tools: &[ToolDefinition]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect()
    }

    /// Send a streaming request to Copilot API
    async fn stream_request(
        &self,
        messages: Vec<Value>,
        tools: Vec<Value>,
        tx: mpsc::Sender<Result<StreamEvent>>,
    ) {
        let model = self.model.read().unwrap().clone();
        let max_tokens: u32 = 16_384;

        // Try up to 2 times (initial + one retry after token refresh)
        for attempt in 0..2 {
            let bearer_token = match self.get_bearer_token().await {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };

            let mut body = json!({
                "model": model,
                "messages": messages,
                "max_tokens": max_tokens,
                "stream": true,
            });

            if !tools.is_empty() {
                body["tools"] = json!(tools);
            }

            let resp = self
                .client
                .post(format!("{}/chat/completions", copilot_auth::COPILOT_API_BASE))
                .header("Authorization", format!("Bearer {}", bearer_token))
                .header("Editor-Version", copilot_auth::EDITOR_VERSION)
                .header("Editor-Plugin-Version", copilot_auth::EDITOR_PLUGIN_VERSION)
                .header("Copilot-Integration-Id", copilot_auth::COPILOT_INTEGRATION_ID)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await;

            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(anyhow::anyhow!("Copilot API request failed: {}", e))).await;
                    return;
                }
            };

            let status = resp.status();

            // On auth error, invalidate token and retry
            if Self::is_auth_error(status) && attempt == 0 {
                *self.bearer_token.write().await = None;
                crate::logging::info("Copilot bearer token expired, refreshing...");
                continue;
            }

            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Copilot API error (HTTP {}): {}",
                        status,
                        body
                    )))
                    .await;
                return;
            }

            // Send connection type event
            let _ = tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: format!("copilot-api ({})", model),
                }))
                .await;

            // Process SSE stream
            self.process_sse_stream(resp, tx).await;
            return;
        }
    }

    async fn process_sse_stream(
        &self,
        resp: reqwest::Response,
        tx: mpsc::Sender<Result<StreamEvent>>,
    ) {
        use futures::StreamExt;

        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_args = String::new();
        let mut input_tokens: u64 = 0;
        let mut output_tokens: u64 = 0;

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx
                        .send(Err(anyhow::anyhow!("Stream error: {}", e)))
                        .await;
                    return;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines
            while let Some(line_end) = buffer.find('\n') {
                let line = buffer[..line_end].trim_end_matches('\r').to_string();
                buffer = buffer[line_end + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if data.trim() == "[DONE]" {
                        // Send usage info before done
                        if input_tokens > 0 || output_tokens > 0 {
                            let _ = tx
                                .send(Ok(StreamEvent::TokenUsage {
                                    input_tokens: Some(input_tokens),
                                    output_tokens: Some(output_tokens),
                                    cache_creation_input_tokens: None,
                                    cache_read_input_tokens: None,
                                }))
                                .await;
                        }
                        let _ = tx.send(Ok(StreamEvent::MessageEnd { stop_reason: None })).await;
                        return;
                    }

                    let parsed: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    // Extract usage if present
                    if let Some(usage) = parsed.get("usage") {
                        input_tokens = usage
                            .get("prompt_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        output_tokens = usage
                            .get("completion_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }

                    // Process choices
                    if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                        for choice in choices {
                            let delta = match choice.get("delta") {
                                Some(d) => d,
                                None => continue,
                            };

                            // Text content
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                                if !content.is_empty() {
                                    let _ = tx
                                        .send(Ok(StreamEvent::TextDelta(
                                            content.to_string(),
                                        )))
                                        .await;
                                }
                            }

                            // Tool calls
                            if let Some(tool_calls) =
                                delta.get("tool_calls").and_then(|t| t.as_array())
                            {
                                for tc in tool_calls {
                                    // New tool call start
                                    if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                        // Flush previous tool call if any
                                        if !current_tool_id.is_empty() {
                                            let _ = tx
                                                .send(Ok(StreamEvent::ToolUseEnd))
                                                .await;
                                        }
                                        current_tool_id = id.to_string();
                                        current_tool_name = tc
                                            .get("function")
                                            .and_then(|f| f.get("name"))
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        current_tool_args.clear();

                                        let _ = tx
                                            .send(Ok(StreamEvent::ToolUseStart {
                                                id: current_tool_id.clone(),
                                                name: current_tool_name.clone(),
                                            }))
                                            .await;
                                    }

                                    // Accumulate arguments
                                    if let Some(args) = tc
                                        .get("function")
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|a| a.as_str())
                                    {
                                        current_tool_args.push_str(args);
                                        let _ = tx
                                            .send(Ok(StreamEvent::ToolInputDelta(
                                                args.to_string(),
                                            )))
                                            .await;
                                    }
                                }
                            }

                            // Finish reason
                            if let Some(finish) =
                                choice.get("finish_reason").and_then(|f| f.as_str())
                            {
                                // Flush last tool call
                                if !current_tool_id.is_empty() {
                                    let _ = tx
                                        .send(Ok(StreamEvent::ToolUseEnd))
                                        .await;
                                    current_tool_id.clear();
                                    current_tool_name.clear();
                                    current_tool_args.clear();
                                }

                                let stop_reason = match finish {
                                    "stop" => "end_turn",
                                    "tool_calls" => "tool_use",
                                    "length" => "max_tokens",
                                    other => other,
                                };
                                let _ = tx
                                    .send(Ok(StreamEvent::MessageEnd {
                                        stop_reason: Some(stop_reason.to_string()),
                                    }))
                                    .await;
                            }
                        }
                    }
                }
            }
        }

        // Stream ended without [DONE]
        let _ = tx.send(Ok(StreamEvent::MessageEnd { stop_reason: None })).await;
    }
}

#[async_trait]
impl Provider for CopilotApiProvider {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let built_messages = Self::build_messages(system, messages);
        let built_tools = Self::build_tools(tools);

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        let provider = CopilotApiProvider {
            client: self.client.clone(),
            model: self.model.clone(),
            github_token: self.github_token.clone(),
            bearer_token: self.bearer_token.clone(),
            fetched_models: self.fetched_models.clone(),
        };

        tokio::spawn(async move {
            provider
                .stream_request(built_messages, built_tools, tx)
                .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "copilot"
    }

    fn model(&self) -> String {
        self.model
            .try_read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Copilot model cannot be empty");
        }
        if let Ok(mut current) = self.model.try_write() {
            *current = trimmed.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        FALLBACK_MODELS.to_vec()
    }

    fn available_models_display(&self) -> Vec<String> {
        if let Ok(models) = self.fetched_models.read() {
            if !models.is_empty() {
                return models.clone();
            }
        }
        FALLBACK_MODELS.iter().map(|m| (*m).to_string()).collect()
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn context_window(&self) -> usize {
        copilot_context_window(&self.model())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(CopilotApiProvider {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            github_token: self.github_token.clone(),
            bearer_token: self.bearer_token.clone(),
            fetched_models: self.fetched_models.clone(),
        })
    }
}
