//! Direct Anthropic API provider
//!
//! Uses the Anthropic Messages API directly without the Python SDK.
//! This provides better control and eliminates the Python dependency.

use super::{EventStream, Provider, NativeToolResultSender};
use crate::auth;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;

/// Anthropic Messages API endpoint
const API_URL: &str = "https://api.anthropic.com/v1/messages";

/// Default model
const DEFAULT_MODEL: &str = "claude-opus-4-5-20251101";

/// API version header
const API_VERSION: &str = "2023-06-01";

/// Available models
pub const AVAILABLE_MODELS: &[&str] = &[
    "claude-opus-4-5-20251101",
    "claude-sonnet-4-20250514",
    "claude-haiku-4-5-20241022",
];

/// Direct Anthropic API provider
pub struct AnthropicProvider {
    client: Client,
    model: Arc<RwLock<String>>,
}

impl AnthropicProvider {
    pub fn new() -> Self {
        let model = std::env::var("JCODE_ANTHROPIC_MODEL")
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Self {
            client: Client::new(),
            model: Arc::new(RwLock::new(model)),
        }
    }

    /// Get the access token from credentials
    async fn get_access_token(&self) -> Result<String> {
        let creds = auth::claude::load_credentials()
            .context("Failed to load Claude credentials")?;
        Ok(creds.access_token)
    }

    /// Convert our Message type to Anthropic API format
    fn format_messages(&self, messages: &[Message]) -> Vec<ApiMessage> {
        messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };

                let content = self.format_content_blocks(&msg.content);

                ApiMessage {
                    role: role.to_string(),
                    content,
                }
            })
            .filter(|msg| !msg.content.is_empty())
            .collect()
    }

    /// Convert our ContentBlock to Anthropic API format
    fn format_content_blocks(&self, blocks: &[ContentBlock]) -> Vec<ApiContentBlock> {
        blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(ApiContentBlock::Text {
                    text: text.clone(),
                }),
                ContentBlock::ToolUse { id, name, input } => Some(ApiContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => Some(ApiContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: is_error.unwrap_or(false),
                }),
                _ => None, // Skip other block types (thinking, etc.)
            })
            .collect()
    }

    /// Convert tool definitions to Anthropic API format
    fn format_tools(&self, tools: &[ToolDefinition]) -> Vec<ApiTool> {
        tools
            .iter()
            .map(|tool| ApiTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect()
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let token = self.get_access_token().await?;
        let model = self.model.read().await.clone();

        // Format request
        let api_messages = self.format_messages(messages);
        let api_tools = self.format_tools(tools);

        let request = ApiRequest {
            model: model.clone(),
            max_tokens: 16384,
            system: if system.is_empty() {
                None
            } else {
                Some(system.to_string())
            },
            messages: api_messages,
            tools: if api_tools.is_empty() {
                None
            } else {
                Some(api_tools)
            },
            stream: true,
        };

        // Create channel for streaming events
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        // Clone what we need for the async task
        let client = self.client.clone();

        // Spawn task to handle streaming
        tokio::spawn(async move {
            if let Err(e) = stream_response(client, token, request, tx.clone()).await {
                let _ = tx.send(Err(e)).await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn model(&self) -> String {
        self.model.blocking_read().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        if !AVAILABLE_MODELS.contains(&model) {
            anyhow::bail!("Model {} not supported by Anthropic provider", model);
        }
        *self.model.blocking_write() = model.to_string();
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model.blocking_read().clone())),
        })
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        None // Direct API doesn't use native tool bridge
    }
}

/// Stream the response from Anthropic API
async fn stream_response(
    client: Client,
    token: String,
    request: ApiRequest,
    tx: mpsc::Sender<Result<StreamEvent>>,
) -> Result<()> {
    let response = client
        .post(API_URL)
        .header("x-api-key", &token)
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(&request)
        .send()
        .await
        .context("Failed to send request to Anthropic API")?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic API error ({}): {}", status, error_text);
    }

    // Parse SSE stream
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut current_tool_use: Option<ToolUseAccumulator> = None;
    let mut input_tokens: Option<u64> = None;
    let mut output_tokens: Option<u64> = None;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.context("Error reading stream chunk")?;
        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        // Process complete SSE events
        while let Some(event) = parse_sse_event(&mut buffer) {
            let events = process_sse_event(
                &event,
                &mut current_tool_use,
                &mut input_tokens,
                &mut output_tokens,
            );
            for stream_event in events {
                if tx.send(Ok(stream_event)).await.is_err() {
                    return Ok(()); // Receiver dropped
                }
            }
        }
    }

    // Send final token usage if we have it
    if input_tokens.is_some() || output_tokens.is_some() {
        let _ = tx
            .send(Ok(StreamEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }))
            .await;
    }

    Ok(())
}

/// Accumulator for tool_use blocks (input comes in chunks)
struct ToolUseAccumulator {
    id: String,
    name: String,
    input_json: String,
}

/// Parse a single SSE event from the buffer
fn parse_sse_event(buffer: &mut String) -> Option<SseEvent> {
    // Look for complete event (ends with double newline)
    let event_end = buffer.find("\n\n")?;
    let event_str = buffer[..event_end].to_string();
    buffer.drain(..event_end + 2);

    let mut event_type = String::new();
    let mut data = String::new();

    for line in event_str.lines() {
        if let Some(rest) = line.strip_prefix("event: ") {
            event_type = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("data: ") {
            data = rest.to_string();
        }
    }

    if event_type.is_empty() && data.is_empty() {
        return None;
    }

    Some(SseEvent { event_type, data })
}

/// SSE event from the stream
struct SseEvent {
    event_type: String,
    data: String,
}

/// Process an SSE event and return StreamEvents if applicable
fn process_sse_event(
    event: &SseEvent,
    current_tool_use: &mut Option<ToolUseAccumulator>,
    input_tokens: &mut Option<u64>,
    output_tokens: &mut Option<u64>,
) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    match event.event_type.as_str() {
        "message_start" => {
            // Extract usage from message_start
            if let Ok(parsed) = serde_json::from_str::<MessageStartEvent>(&event.data) {
                if let Some(usage) = parsed.message.usage {
                    *input_tokens = usage.input_tokens.map(|t| t as u64);
                }
            }
        }
        "content_block_start" => {
            if let Ok(parsed) = serde_json::from_str::<ContentBlockStartEvent>(&event.data) {
                match parsed.content_block {
                    ApiContentBlockStart::Text { .. } => {
                        // Text block starting - nothing to emit yet
                    }
                    ApiContentBlockStart::ToolUse { id, name } => {
                        // Start accumulating tool use
                        *current_tool_use = Some(ToolUseAccumulator {
                            id: id.clone(),
                            name: name.clone(),
                            input_json: String::new(),
                        });
                        events.push(StreamEvent::ToolUseStart { id, name });
                    }
                }
            }
        }
        "content_block_delta" => {
            if let Ok(parsed) = serde_json::from_str::<ContentBlockDeltaEvent>(&event.data) {
                match parsed.delta {
                    ApiDelta::TextDelta { text } => {
                        events.push(StreamEvent::TextDelta(text));
                    }
                    ApiDelta::InputJsonDelta { partial_json } => {
                        if let Some(ref mut tool) = current_tool_use {
                            tool.input_json.push_str(&partial_json);
                        }
                        events.push(StreamEvent::ToolInputDelta(partial_json));
                    }
                }
            }
        }
        "content_block_stop" => {
            // If we were accumulating a tool_use, it's complete now
            if current_tool_use.take().is_some() {
                events.push(StreamEvent::ToolUseEnd);
            }
        }
        "message_delta" => {
            if let Ok(parsed) = serde_json::from_str::<MessageDeltaEvent>(&event.data) {
                if let Some(usage) = parsed.usage {
                    *output_tokens = usage.output_tokens.map(|t| t as u64);
                }
                if let Some(stop_reason) = parsed.delta.stop_reason {
                    events.push(StreamEvent::MessageEnd {
                        stop_reason: Some(stop_reason),
                    });
                }
            }
        }
        "message_stop" => {
            // Final message stop - we may have already sent MessageEnd via message_delta
        }
        "ping" => {
            // Keepalive, ignore
        }
        "error" => {
            crate::logging::error(&format!("Anthropic stream error: {}", event.data));
            events.push(StreamEvent::Error {
                message: event.data.clone(),
                retry_after_secs: None,
            });
        }
        _ => {
            // Unknown event type, ignore
        }
    }

    events
}

// ============================================================================
// API Types
// ============================================================================

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ApiTool>>,
    stream: bool,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: Vec<ApiContentBlock>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type")]
enum ApiContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Serialize)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: Value,
}

// Response types for SSE parsing

#[derive(Deserialize)]
struct MessageStartEvent {
    message: MessageStartMessage,
}

#[derive(Deserialize)]
struct MessageStartMessage {
    usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
struct ContentBlockStartEvent {
    #[allow(dead_code)]
    index: u32,
    content_block: ApiContentBlockStart,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ApiContentBlockStart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize)]
struct ContentBlockDeltaEvent {
    #[allow(dead_code)]
    index: u32,
    delta: ApiDelta,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ApiDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize)]
struct MessageDeltaEvent {
    delta: MessageDeltaDelta,
    usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
struct MessageDeltaDelta {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct UsageInfo {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_event() {
        let mut buffer = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n".to_string();
        let event = parse_sse_event(&mut buffer).unwrap();
        assert_eq!(event.event_type, "message_start");
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_available_models() {
        let provider = AnthropicProvider::new();
        let models = provider.available_models();
        assert!(models.contains(&"claude-opus-4-5-20251101"));
    }
}
