use super::{EventStream, Provider};
use crate::auth::claude as claude_auth;
use crate::auth::oauth::{self, OAuthTokens};
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use tokio::sync::RwLock;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const BETA_HEADER: &str = "oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14";
const USER_AGENT: &str = "claude-code/2.0.76";

/// The system prompt prefix that identifies this as Claude Code
/// This is required for OAuth tokens bound to the Claude Code client
const CLAUDE_CODE_SYSTEM_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

pub struct ClaudeProvider {
    client: Client,
    tokens: Arc<RwLock<OAuthTokens>>,
}

impl ClaudeProvider {
    pub fn new(tokens: OAuthTokens) -> Self {
        Self {
            client: Client::new(),
            tokens: Arc::new(RwLock::new(tokens)),
        }
    }

    /// Ensure we have a valid access token, refreshing if needed
    async fn get_access_token(&self) -> Result<String> {
        let tokens = self.tokens.read().await;
        let now = chrono::Utc::now().timestamp_millis();

        // If token expires in less than 5 minutes, refresh it
        if tokens.expires_at < now + 300_000 {
            drop(tokens); // Release read lock

            let mut tokens = self.tokens.write().await;
            // Double-check after acquiring write lock
            if tokens.expires_at < now + 300_000 {
                eprintln!("Refreshing OAuth token...");
                let new_tokens = oauth::refresh_claude_tokens(&tokens.refresh_token).await?;
                *tokens = new_tokens;
            }
            Ok(tokens.access_token.clone())
        } else {
            Ok(tokens.access_token.clone())
        }
    }

    async fn refresh_access_token(&self) -> Result<String> {
        let refresh_token = { self.tokens.read().await.refresh_token.clone() };
        match oauth::refresh_claude_tokens(&refresh_token).await {
            Ok(new_tokens) => {
                let mut tokens = self.tokens.write().await;
                *tokens = new_tokens;
                Ok(tokens.access_token.clone())
            }
            Err(refresh_err) => {
                if let Ok(creds) = claude_auth::load_opencode_credentials() {
                    let fallback = OAuthTokens {
                        access_token: creds.access_token,
                        refresh_token: creds.refresh_token,
                        expires_at: creds.expires_at,
                        id_token: None,
                    };
                    let _ = oauth::save_claude_tokens(&fallback);
                    let mut tokens = self.tokens.write().await;
                    *tokens = fallback;
                    return Ok(tokens.access_token.clone());
                }
                Err(refresh_err)
            }
        }
    }

    async fn send_request<'a>(
        &self,
        request: &ApiRequest<'a>,
        access_token: &str,
    ) -> Result<reqwest::Response> {
        self.client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", BETA_HEADER)
            .header("Content-Type", "application/json")
            .header("User-Agent", USER_AGENT)
            .json(request)
            .send()
            .await
            .context("Failed to send request to Claude API")
    }
}

#[derive(Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: Vec<SystemBlock<'a>>,
    messages: Vec<ApiMessage>,
    tools: &'a [ToolDefinition],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<RequestMetadata>,
}

#[derive(Serialize)]
struct SystemBlock<'a> {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct RequestMetadata {
    user_id: String,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: serde_json::Value,
}

impl From<&Message> for ApiMessage {
    fn from(msg: &Message) -> Self {
        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };

        // Convert content blocks to API format
        let mut content_blocks = Vec::new();
        let mut text_parts = Vec::new();
        let mut all_text = true;

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    text_parts.push(text.as_str());
                    content_blocks.push(serde_json::json!({ "type": "text", "text": text }));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    all_text = false;
                    content_blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    all_text = false;
                    let mut obj = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content
                    });
                    if let Some(true) = is_error {
                        obj["is_error"] = serde_json::json!(true);
                    }
                    content_blocks.push(obj);
                }
            }
        }

        let content = if all_text {
            serde_json::Value::String(text_parts.join("\n\n"))
        } else {
            serde_json::Value::Array(content_blocks)
        };

        ApiMessage {
            role: role.to_string(),
            content,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: serde_json::Value },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ContentBlockInfo,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: DeltaInfo },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaInfo,
        #[serde(default)]
        usage: Option<UsageInfo>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: ErrorInfo },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum ContentBlockInfo {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum DeltaInfo {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize, Debug)]
struct UsageInfo {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct MessageDeltaInfo {
    stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct ErrorInfo {
    message: String,
}

/// Stream wrapper for SSE events
struct ClaudeStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    pending: VecDeque<StreamEvent>,
    last_stop_reason: Option<String>,
}

impl Stream for ClaudeStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // Check if we have a complete event in the buffer
            if let Some(event) = self.parse_next_event() {
                return Poll::Ready(Some(Ok(event)));
            }

            // Try to get more data
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

impl ClaudeStream {
    fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: VecDeque::new(),
            last_stop_reason: None,
        }
    }

    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }

        // Look for complete SSE events (data: ...\n\n)
        while let Some(pos) = self.buffer.find("\n\n") {
            let event_str = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            // Parse "data: {...}" lines
            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(event) = serde_json::from_str::<SseEvent>(data) {
                        let events = self.convert_event(event);
                        if !events.is_empty() {
                            self.pending.extend(events);
                            return self.pending.pop_front();
                        }
                    }
                }
            }
        }
        None
    }

    fn convert_event(&mut self, event: SseEvent) -> Vec<StreamEvent> {
        match event {
            SseEvent::MessageStart { message } => {
                if let Some(usage) = message.get("usage") {
                    let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64());
                    let output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64());
                    if input_tokens.is_some() || output_tokens.is_some() {
                        return vec![StreamEvent::TokenUsage {
                            input_tokens,
                            output_tokens,
                        }];
                    }
                }
                Vec::new()
            }
            SseEvent::ContentBlockStart { content_block, .. } => match content_block {
                ContentBlockInfo::Text { .. } => Vec::new(),
                ContentBlockInfo::ToolUse { id, name } => {
                    vec![StreamEvent::ToolUseStart { id, name }]
                }
            },
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                DeltaInfo::TextDelta { text } => vec![StreamEvent::TextDelta(text)],
                DeltaInfo::InputJsonDelta { partial_json } => {
                    vec![StreamEvent::ToolInputDelta(partial_json)]
                }
            },
            SseEvent::ContentBlockStop { .. } => vec![StreamEvent::ToolUseEnd],
            SseEvent::MessageDelta { delta, usage } => {
                self.last_stop_reason = delta.stop_reason.clone();
                if let Some(usage) = usage {
                    if usage.input_tokens.is_some() || usage.output_tokens.is_some() {
                        return vec![StreamEvent::TokenUsage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                        }];
                    }
                }
                Vec::new()
            }
            SseEvent::MessageStop => vec![StreamEvent::MessageEnd {
                stop_reason: self.last_stop_reason.take(),
            }],
            SseEvent::Error { error } => vec![StreamEvent::Error(error.message)],
            _ => Vec::new(),
        }
    }
}

#[async_trait]
impl Provider for ClaudeProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
    ) -> Result<EventStream> {
        let api_messages: Vec<ApiMessage> = messages.iter().map(|m| m.into()).collect();

        // Match Claude Code: send the spoof header as its own system block.
        let mut system_blocks = Vec::with_capacity(2);
        system_blocks.push(SystemBlock {
            block_type: "text",
            text: CLAUDE_CODE_SYSTEM_PREFIX,
        });
        if !system.trim().is_empty() {
            system_blocks.push(SystemBlock {
                block_type: "text",
                text: system,
            });
        }

        let request = ApiRequest {
            model: "claude-sonnet-4-20250514",
            max_tokens: 8192,
            system: system_blocks,
            messages: api_messages,
            tools,
            stream: true,
            metadata: None,
        };

        let access_token = self.get_access_token().await?;

        let response = self.send_request(&request, &access_token).await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if should_refresh_token(status, &body) {
                let refreshed_token = self.refresh_access_token().await?;
                let retry_response = self.send_request(&request, &refreshed_token).await?;
                if !retry_response.status().is_success() {
                    let retry_status = retry_response.status();
                    let retry_body = retry_response.text().await.unwrap_or_default();
                    anyhow::bail!("Claude API error {}: {}", retry_status, retry_body);
                }
                let stream = ClaudeStream::new(retry_response.bytes_stream());
                return Ok(Box::pin(stream));
            }
            anyhow::bail!("Claude API error {}: {}", status, body);
        }

        let stream = ClaudeStream::new(response.bytes_stream());
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "claude"
    }
}

fn should_refresh_token(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::UNAUTHORIZED {
        return true;
    }
    if status == StatusCode::FORBIDDEN {
        let lower = body.to_lowercase();
        return lower.contains("oauth")
            || lower.contains("token")
            || lower.contains("permission_error")
            || lower.contains("authentication_error");
    }
    false
}
