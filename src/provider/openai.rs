use super::{EventStream, Provider};
use crate::auth::codex::CodexCredentials;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

const API_URL: &str = "https://api.openai.com/v1/chat/completions";

pub struct OpenAIProvider {
    client: Client,
    credentials: CodexCredentials,
}

impl OpenAIProvider {
    pub fn new(credentials: CodexCredentials) -> Self {
        Self {
            client: Client::new(),
            credentials,
        }
    }
}

#[derive(Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    messages: Vec<ApiMessage>,
    tools: Vec<OpenAITool<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCallMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct ToolCallMessage {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: FunctionCall,
}

#[derive(Serialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct OpenAITool<'a> {
    #[serde(rename = "type")]
    tool_type: &'a str,
    function: OpenAIFunction<'a>,
}

#[derive(Serialize)]
struct OpenAIFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
}

fn convert_messages(messages: &[Message], system: &str) -> Vec<ApiMessage> {
    let mut result = vec![ApiMessage {
        role: "system".to_string(),
        content: Some(system.to_string()),
        tool_calls: None,
        tool_call_id: None,
    }];

    for msg in messages {
        match msg.role {
            Role::User => {
                // Collect text and tool results
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            result.push(ApiMessage {
                                role: "user".to_string(),
                                content: Some(text.clone()),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            result.push(ApiMessage {
                                role: "tool".to_string(),
                                content: Some(content.clone()),
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.clone()),
                            });
                        }
                        _ => {}
                    }
                }
            }
            Role::Assistant => {
                let mut text_content = String::new();
                let mut tool_calls = Vec::new();

                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            text_content.push_str(text);
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            tool_calls.push(ToolCallMessage {
                                id: id.clone(),
                                call_type: "function".to_string(),
                                function: FunctionCall {
                                    name: name.clone(),
                                    arguments: serde_json::to_string(input).unwrap_or_default(),
                                },
                            });
                        }
                        _ => {}
                    }
                }

                result.push(ApiMessage {
                    role: "assistant".to_string(),
                    content: if text_content.is_empty() {
                        None
                    } else {
                        Some(text_content)
                    },
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    tool_call_id: None,
                });
            }
        }
    }

    result
}

fn convert_tools(tools: &[ToolDefinition]) -> Vec<OpenAITool> {
    tools
        .iter()
        .map(|t| OpenAITool {
            tool_type: "function",
            function: OpenAIFunction {
                name: &t.name,
                description: &t.description,
                parameters: &t.input_schema,
            },
        })
        .collect()
}

#[derive(Deserialize, Debug)]
struct SseChunk {
    choices: Vec<Choice>,
}

#[derive(Deserialize, Debug)]
struct Choice {
    delta: Delta,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct Delta {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Deserialize, Debug)]
struct ToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<FunctionDelta>,
}

#[derive(Deserialize, Debug)]
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

/// Stream wrapper for OpenAI SSE events
struct OpenAIStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
}

impl Stream for OpenAIStream {
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

impl OpenAIStream {
    fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            current_tool_id: None,
            current_tool_name: None,
        }
    }

    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 1..].to_string();

            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    return Some(StreamEvent::MessageEnd { stop_reason: None });
                }

                if let Ok(chunk) = serde_json::from_str::<SseChunk>(data) {
                    if let Some(event) = self.convert_chunk(chunk) {
                        return Some(event);
                    }
                }
            }
        }
        None
    }

    fn convert_chunk(&mut self, chunk: SseChunk) -> Option<StreamEvent> {
        let choice = chunk.choices.first()?;

        // Check for finish
        if choice.finish_reason.is_some() {
            if self.current_tool_id.is_some() {
                self.current_tool_id = None;
                self.current_tool_name = None;
                return Some(StreamEvent::ToolUseEnd);
            }
            return Some(StreamEvent::MessageEnd {
                stop_reason: choice.finish_reason.clone(),
            });
        }

        // Text content
        if let Some(content) = &choice.delta.content {
            return Some(StreamEvent::TextDelta(content.clone()));
        }

        // Tool calls
        if let Some(tool_calls) = &choice.delta.tool_calls {
            for tc in tool_calls {
                // New tool call starting
                if let Some(id) = &tc.id {
                    let name = tc
                        .function
                        .as_ref()
                        .and_then(|f| f.name.clone())
                        .unwrap_or_default();

                    self.current_tool_id = Some(id.clone());
                    self.current_tool_name = Some(name.clone());

                    return Some(StreamEvent::ToolUseStart {
                        id: id.clone(),
                        name,
                    });
                }

                // Tool arguments delta
                if let Some(func) = &tc.function {
                    if let Some(args) = &func.arguments {
                        return Some(StreamEvent::ToolInputDelta(args.clone()));
                    }
                }
            }
        }

        None
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
    ) -> Result<EventStream> {
        let api_messages = convert_messages(messages, system);
        let api_tools = convert_tools(tools);

        let request = ApiRequest {
            model: "gpt-4o",
            messages: api_messages,
            tools: api_tools,
            stream: true,
        };

        let response = self
            .client
            .post(API_URL)
            .header(
                "Authorization",
                format!("Bearer {}", self.credentials.access_token),
            )
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to OpenAI API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API error {}: {}", status, body);
        }

        let stream = OpenAIStream::new(response.bytes_stream());
        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "openai"
    }
}
