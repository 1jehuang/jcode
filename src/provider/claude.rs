use super::{EventStream, Provider};
use crate::auth::claude::ClaudeCredentials;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const BETA_HEADER: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14";

pub struct ClaudeProvider {
    client: Client,
    credentials: ClaudeCredentials,
}

impl ClaudeProvider {
    pub fn new(credentials: ClaudeCredentials) -> Self {
        Self {
            client: Client::new(),
            credentials,
        }
    }
}

#[derive(Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<ApiMessage>,
    tools: &'a [ToolDefinition],
    stream: bool,
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
        let content: Vec<serde_json::Value> = msg
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => {
                    serde_json::json!({ "type": "text", "text": text })
                }
                ContentBlock::ToolUse { id, name, input } => {
                    serde_json::json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input
                    })
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let mut obj = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content
                    });
                    if let Some(true) = is_error {
                        obj["is_error"] = serde_json::json!(true);
                    }
                    obj
                }
            })
            .collect();

        ApiMessage {
            role: role.to_string(),
            content: serde_json::Value::Array(content),
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
    MessageDelta { delta: MessageDeltaInfo },
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
        }
    }

    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        // Look for complete SSE events (data: ...\n\n)
        while let Some(pos) = self.buffer.find("\n\n") {
            let event_str = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            // Parse "data: {...}" lines
            for line in event_str.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(event) = serde_json::from_str::<SseEvent>(data) {
                        if let Some(stream_event) = self.convert_event(event) {
                            return Some(stream_event);
                        }
                    }
                }
            }
        }
        None
    }

    fn convert_event(&self, event: SseEvent) -> Option<StreamEvent> {
        match event {
            SseEvent::ContentBlockStart { content_block, .. } => match content_block {
                ContentBlockInfo::Text { .. } => None,
                ContentBlockInfo::ToolUse { id, name } => {
                    Some(StreamEvent::ToolUseStart { id, name })
                }
            },
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                DeltaInfo::TextDelta { text } => Some(StreamEvent::TextDelta(text)),
                DeltaInfo::InputJsonDelta { partial_json } => {
                    Some(StreamEvent::ToolInputDelta(partial_json))
                }
            },
            SseEvent::ContentBlockStop { .. } => Some(StreamEvent::ToolUseEnd),
            SseEvent::MessageDelta { delta } => Some(StreamEvent::MessageEnd {
                stop_reason: delta.stop_reason,
            }),
            SseEvent::MessageStop => Some(StreamEvent::MessageEnd { stop_reason: None }),
            SseEvent::Error { error } => Some(StreamEvent::Error(error.message)),
            _ => None,
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

        let request = ApiRequest {
            model: "claude-sonnet-4-20250514",
            max_tokens: 8192,
            system,
            messages: api_messages,
            tools,
            stream: true,
        };

        let response = self
            .client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", self.credentials.access_token))
            .header("anthropic-version", API_VERSION)
            .header("anthropic-beta", BETA_HEADER)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Claude API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Claude API error {}: {}", status, body);
        }

        let stream = ClaudeStream::new(response.bytes_stream());
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "claude"
    }
}
