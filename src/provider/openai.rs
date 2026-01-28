use super::{EventStream, Provider};
use crate::auth::codex::CodexCredentials;
use crate::auth::oauth;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;

const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const CHATGPT_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const RESPONSES_PATH: &str = "responses";
const DEFAULT_MODEL: &str = "gpt-5.2-codex";
const ORIGINATOR: &str = "codex_cli_rs";
const CHATGPT_INSTRUCTIONS: &str = include_str!("../prompts/gpt-5.1-codex-max_prompt.md");

/// Maximum number of retries for transient errors
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const RETRY_BASE_DELAY_MS: u64 = 1000;

/// Available OpenAI/Codex models
const AVAILABLE_MODELS: &[&str] = &["gpt-5.2-codex"];

pub struct OpenAIProvider {
    client: Client,
    credentials: Arc<RwLock<CodexCredentials>>,
    model: Arc<RwLock<String>>,
    prompt_cache_key: Option<String>,
    prompt_cache_retention: Option<String>,
    reasoning_effort: Option<String>,
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

        Self {
            client: Client::new(),
            credentials: Arc::new(RwLock::new(credentials)),
            model: Arc::new(RwLock::new(model)),
            prompt_cache_key,
            prompt_cache_retention,
            reasoning_effort,
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

fn build_responses_input(messages: &[Message]) -> Vec<Value> {
    use std::collections::HashSet;

    // First pass: collect all tool call IDs from ToolUse blocks
    let mut tool_call_ids: HashSet<String> = HashSet::new();
    for msg in messages {
        if let Role::Assistant = msg.role {
            for block in &msg.content {
                if let ContentBlock::ToolUse { id, .. } = block {
                    tool_call_ids.insert(id.clone());
                }
            }
        }
    }

    // Second pass: build items, filtering orphaned tool results
    let mut items = Vec::new();
    let mut skipped_results = 0;

    for msg in messages {
        match msg.role {
            Role::User => {
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            items.push(serde_json::json!({
                                "type": "message",
                                "role": "user",
                                "content": [{ "type": "input_text", "text": text }]
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            // Skip orphaned tool results (no matching tool call)
                            if !tool_call_ids.contains(tool_use_id) {
                                skipped_results += 1;
                                crate::logging::info(&format!(
                                    "[openai] Skipping orphaned tool result with call_id: {}",
                                    tool_use_id
                                ));
                                continue;
                            }
                            // OpenAI expects output to be a string or array of objects, not an object
                            let output = if is_error == &Some(true) {
                                format!("[Error] {}", content)
                            } else {
                                content.clone()
                            };
                            items.push(serde_json::json!({
                                "type": "function_call_output",
                                "call_id": tool_use_id,
                                "output": output
                            }));
                        }
                        _ => {}
                    }
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
                            let arguments = serde_json::to_string(input).unwrap_or_default();
                            items.push(serde_json::json!({
                                "type": "function_call",
                                "name": name,
                                "arguments": arguments,
                                "call_id": id
                            }));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    if skipped_results > 0 {
        crate::logging::info(&format!(
            "[openai] Filtered {} orphaned tool result(s) to prevent API error",
            skipped_results
        ));
    }

    items
}

#[derive(Deserialize, Debug)]
struct ResponseSseEvent {
    #[serde(rename = "type")]
    kind: String,
    item: Option<Value>,
    delta: Option<String>,
    response: Option<Value>,
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
            if data == "[DONE]" {
                return Some(StreamEvent::MessageEnd { stop_reason: None });
            }

            let event: ResponseSseEvent = match serde_json::from_str(&data) {
                Ok(parsed) => parsed,
                Err(_) => continue,
            };

            match event.kind.as_str() {
                "response.output_text.delta" => {
                    if let Some(delta) = event.delta {
                        self.saw_text_delta = true;
                        return Some(StreamEvent::TextDelta(delta));
                    }
                }
                "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
                    // Reasoning/thinking delta - display as thinking content
                    if let Some(delta) = event.delta {
                        return Some(StreamEvent::ThinkingDelta(delta));
                    }
                }
                "response.reasoning.done" | "response.output_item.added" => {
                    // Check if this is a reasoning item starting
                    if let Some(item) = &event.item {
                        if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                            return Some(StreamEvent::ThinkingStart);
                        }
                    }
                }
                "response.output_item.done" => {
                    if let Some(item) = event.item {
                        if let Some(event) = self.handle_output_item(item) {
                            return Some(event);
                        }
                    }
                }
                "response.completed" => {
                    if let Some(response) = event.response {
                        if let Some(usage_event) = extract_usage_from_response(&response) {
                            self.pending.push_back(usage_event);
                        }
                    }
                    self.pending
                        .push_back(StreamEvent::MessageEnd { stop_reason: None });
                    return self.pending.pop_front();
                }
                "response.failed" | "error" => {
                    let (message, retry_after_secs) = extract_error_with_retry(&event.response);
                    return Some(StreamEvent::Error {
                        message,
                        retry_after_secs,
                    });
                }
                _ => {}
            }
        }

        None
    }

    fn handle_output_item(&mut self, item: Value) -> Option<StreamEvent> {
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

                self.pending.push_back(StreamEvent::ToolUseStart {
                    id: call_id.clone(),
                    name,
                });
                self.pending
                    .push_back(StreamEvent::ToolInputDelta(arguments.to_string()));
                self.pending.push_back(StreamEvent::ToolUseEnd);
                return self.pending.pop_front();
            }
            "message" => {
                if self.saw_text_delta {
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
                // Extract reasoning summary text from the item
                // OpenAI returns: {"type":"reasoning","summary":[{"type":"summary_text","text":"..."}]}
                if let Some(summary_arr) = item.get("summary").and_then(|v| v.as_array()) {
                    let mut summary_text = String::new();
                    for summary_item in summary_arr {
                        if summary_item.get("type").and_then(|v| v.as_str()) == Some("summary_text")
                        {
                            if let Some(text) = summary_item.get("text").and_then(|v| v.as_str()) {
                                if !summary_text.is_empty() {
                                    summary_text.push('\n');
                                }
                                summary_text.push_str(text);
                            }
                        }
                    }
                    if !summary_text.is_empty() {
                        // Emit thinking events: start, content, end
                        self.pending.push_back(StreamEvent::ThinkingStart);
                        self.pending
                            .push_back(StreamEvent::ThinkingDelta(summary_text));
                        self.pending.push_back(StreamEvent::ThinkingEnd);
                        return self.pending.pop_front();
                    }
                }
            }
            _ => {}
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
        messages: &[Message],
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
        let client = self.client.clone();
        let credentials = Arc::clone(&self.credentials);

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

                match stream_response(
                    client.clone(),
                    Arc::clone(&credentials),
                    request.clone(),
                    tx.clone(),
                )
                .await
                {
                    Ok(()) => return, // Success
                    Err(e) => {
                        let error_str = e.to_string().to_lowercase();
                        // Check if this is a transient/retryable error
                        if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                            crate::logging::info(&format!("Transient error, will retry: {}", e));
                            last_error = Some(e);
                            continue;
                        }
                        // Non-retryable or final attempt
                        let _ = tx.send(Err(e)).await;
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

    fn fork(&self) -> Arc<dyn Provider> {
        let model = self.model();
        Arc::new(OpenAIProvider {
            client: self.client.clone(),
            credentials: Arc::clone(&self.credentials),
            model: Arc::new(RwLock::new(model)),
            prompt_cache_key: self.prompt_cache_key.clone(),
            prompt_cache_retention: self.prompt_cache_retention.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
        })
    }
}

/// Stream the response from OpenAI API
async fn stream_response(
    client: Client,
    credentials: Arc<RwLock<CodexCredentials>>,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<()> {
    // Get access token (with potential refresh)
    let access_token = {
        let tokens = credentials.read().await;
        if tokens.access_token.is_empty() {
            anyhow::bail!("OpenAI access token is empty");
        }

        // Check if token needs refresh
        if let Some(expires_at) = tokens.expires_at {
            let now = chrono::Utc::now().timestamp_millis();
            if expires_at < now + 300_000 && !tokens.refresh_token.is_empty() {
                drop(tokens);
                // Refresh token
                let mut tokens = credentials.write().await;
                let refreshed = oauth::refresh_openai_tokens(&tokens.refresh_token).await?;
                let id_token = refreshed
                    .id_token
                    .clone()
                    .or_else(|| tokens.id_token.clone());
                let account_id = tokens.account_id.clone();

                *tokens = CodexCredentials {
                    access_token: refreshed.access_token.clone(),
                    refresh_token: refreshed.refresh_token,
                    id_token,
                    account_id,
                    expires_at: Some(refreshed.expires_at),
                };
                refreshed.access_token
            } else {
                tokens.access_token.clone()
            }
        } else {
            tokens.access_token.clone()
        }
    };

    let creds = credentials.read().await;
    let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
    let url = if is_chatgpt_mode {
        format!("{}/{}", CHATGPT_API_BASE.trim_end_matches('/'), RESPONSES_PATH)
    } else {
        format!("{}/{}", OPENAI_API_BASE.trim_end_matches('/'), RESPONSES_PATH)
    };

    let mut builder = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json");

    if is_chatgpt_mode {
        builder = builder.header("originator", ORIGINATOR);
        if let Some(account_id) = creds.account_id.as_ref() {
            builder = builder.header("chatgpt-account-id", account_id);
        }
    }
    drop(creds);

    let response = builder
        .json(&request)
        .send()
        .await
        .context("Failed to send request to OpenAI API")?;

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
            anyhow::bail!("Token refresh needed: {}", body);
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
        anyhow::bail!("{}", msg);
    }

    // Stream the response
    let mut stream = OpenAIResponsesStream::new(response.bytes_stream());

    use futures::StreamExt;
    while let Some(result) = stream.next().await {
        match result {
            Ok(event) => {
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

fn extract_error_with_retry(response: &Option<Value>) -> (String, Option<u64>) {
    let resp = match response.as_ref() {
        Some(r) => r,
        None => return ("OpenAI response stream error".to_string(), None),
    };

    let error = match resp.get("error") {
        Some(e) => e,
        None => return ("OpenAI response stream error".to_string(), None),
    };

    let message = error
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("OpenAI response stream error")
        .to_string();

    // Try to extract retry_after from error object or response metadata
    // OpenAI may include it in error.retry_after or response.retry_after
    let retry_after = error
        .get("retry_after")
        .and_then(|v| v.as_u64())
        .or_else(|| resp.get("retry_after").and_then(|v| v.as_u64()));

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
        // Stream/decode errors
        || error_str.contains("error decoding")
        || error_str.contains("error reading")
        || error_str.contains("unexpected eof")
        || error_str.contains("incomplete message")
        // Server errors (5xx)
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::codex::CodexCredentials;

    #[test]
    fn test_openai_supports_codex_52_model() {
        let creds = CodexCredentials {
            access_token: "test".to_string(),
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            expires_at: None,
        };

        let provider = OpenAIProvider::new(creds);
        assert!(provider.available_models().contains(&"gpt-5.2-codex"));

        provider.set_model("gpt-5.2-codex").unwrap();
        assert_eq!(provider.model(), "gpt-5.2-codex");
    }
}
