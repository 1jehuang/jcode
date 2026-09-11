//! OpenCode Go's model-specific OpenAI Responses API adapter.
//!
//! OpenCode Go exposes a small set of models through `/responses` while the
//! remaining catalog continues to use `/chat/completions`.  Keep this adapter
//! local to the OpenRouter-compatible runtime so the protocol choice is made
//! from the configured OpenCode Go profile without changing other providers.

use bytes::Bytes;
use futures::Stream;
use jcode_message_types::{
    ContentBlock, Message, Role, StreamEvent, ToolDefinition, sanitize_tool_id,
};
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Models currently documented by OpenCode Go as Responses API models.
const RESPONSES_MODELS: &[&str] = &[
    "grok-4.6",
    "gpt-5.6-luna",
    "muse-spark-1.3-contributor",
    "muse-spark-1.2-contributor",
];

pub(crate) fn model_uses_responses_api(
    api_base: &str,
    profile_id: Option<&str>,
    model: &str,
) -> bool {
    let is_go_profile = profile_id.is_some_and(|id| id.eq_ignore_ascii_case("opencode-go"));
    let is_go_endpoint = api_base
        .to_ascii_lowercase()
        .contains("opencode.ai/zen/go/");
    (is_go_profile || is_go_endpoint)
        && RESPONSES_MODELS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(model.trim()))
}

pub(crate) fn build_request(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    model: &str,
    reasoning_effort: Option<&str>,
    max_output_tokens: Option<u32>,
    sanitize_schema: impl Fn(&Value) -> Value,
) -> Value {
    let input = build_input(messages);
    let mut request = serde_json::json!({
        "model": model,
        "input": input,
        "stream": true,
    });

    if !system.is_empty() {
        request["instructions"] = Value::String(system.to_string());
    }
    if let Some(effort) = reasoning_effort.filter(|value| *value != "none") {
        request["reasoning"] = serde_json::json!({ "effort": effort });
    }
    if let Some(max_output_tokens) = max_output_tokens {
        request["max_output_tokens"] = serde_json::json!(max_output_tokens);
    }
    if !tools.is_empty() {
        request["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": sanitize_schema(&tool.input_schema),
                        "strict": false,
                    })
                })
                .collect(),
        );
    }

    request
}

fn build_input(messages: &[Message]) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages {
        match message.role {
            Role::User => {
                let mut content = Vec::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            content.push(serde_json::json!({
                                "type": "input_text",
                                "text": text,
                            }));
                        }
                        ContentBlock::Image { media_type, data } => {
                            content.push(serde_json::json!({
                                "type": "input_image",
                                "image_url": format!("data:{media_type};base64,{data}"),
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            input.push(serde_json::json!({
                                "type": "function_call_output",
                                "call_id": sanitize_tool_id(tool_use_id),
                                "output": if is_error == &Some(true) {
                                    format!("[Error] {content}")
                                } else {
                                    content.clone()
                                },
                            }));
                        }
                        _ => {}
                    }
                }
                if !content.is_empty() {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": content,
                    }));
                }
            }
            Role::Assistant => {
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text, .. } => input.push(serde_json::json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{ "type": "output_text", "text": text }],
                        })),
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input: arguments,
                            ..
                        } => {
                            let arguments = if arguments.is_object() {
                                serde_json::to_string(arguments)
                                    .unwrap_or_else(|_| "{}".to_string())
                            } else {
                                "{}".to_string()
                            };
                            input.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": sanitize_tool_id(id),
                                "name": name,
                                "arguments": arguments,
                            }));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    input
}

#[derive(Default)]
struct ToolState {
    call_id: String,
    name: String,
    arguments: String,
}

pub(crate) struct ResponsesStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    pending: Vec<StreamEvent>,
    tools: HashMap<String, ToolState>,
    thinking: bool,
    ended: bool,
}

impl ResponsesStream {
    pub(crate) fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: Vec::new(),
            tools: HashMap::new(),
            thinking: false,
            ended: false,
        }
    }

    fn parse_event(&mut self, data: &str) {
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "response.output_text.delta" => {
                if self.thinking {
                    self.pending.push(StreamEvent::ThinkingEnd);
                    self.thinking = false;
                }
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    self.pending.push(StreamEvent::TextDelta(delta.to_string()));
                }
            }
            "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
                if !self.thinking {
                    self.pending.push(StreamEvent::ThinkingStart);
                    self.thinking = true;
                }
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    self.pending
                        .push(StreamEvent::ThinkingDelta(delta.to_string()));
                }
            }
            "response.output_item.added" => {
                if let Some(item) = value.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("reasoning")
                    && !self.thinking
                {
                    self.pending.push(StreamEvent::ThinkingStart);
                    self.thinking = true;
                } else if let Some(item) = value.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("function_call")
                    && let Some(item_id) = item.get("id").and_then(Value::as_str)
                {
                    let state = self.tools.entry(item_id.to_string()).or_default();
                    state.call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or(item_id)
                        .to_string();
                    state.name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
                        state.arguments = arguments.to_string();
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(item_id) = value.get("item_id").and_then(Value::as_str) {
                    let state = self.tools.entry(item_id.to_string()).or_default();
                    if let Some(call_id) = value.get("call_id").and_then(Value::as_str) {
                        state.call_id = call_id.to_string();
                    }
                    if let Some(name) = value.get("name").and_then(Value::as_str) {
                        state.name = name.to_string();
                    }
                    if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                        state.arguments.push_str(delta);
                    }
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(item_id) = value.get("item_id").and_then(Value::as_str) {
                    let mut state = self.tools.remove(item_id).unwrap_or_default();
                    if let Some(call_id) = value.get("call_id").and_then(Value::as_str) {
                        state.call_id = call_id.to_string();
                    }
                    if let Some(name) = value.get("name").and_then(Value::as_str) {
                        state.name = name.to_string();
                    }
                    if let Some(arguments) = value.get("arguments").and_then(Value::as_str) {
                        state.arguments = arguments.to_string();
                    }
                    self.emit_tool(state, item_id);
                }
            }
            "response.output_item.done" => {
                if let Some(item) = value.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("function_call")
                {
                    let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                    if !item_id.is_empty() && self.tools.contains_key(item_id) {
                        let state = self.tools.remove(item_id).unwrap_or_default();
                        self.emit_tool(state, item_id);
                    }
                }
            }
            "response.completed" | "response.incomplete" => {
                if self.thinking {
                    self.pending.push(StreamEvent::ThinkingEnd);
                    self.thinking = false;
                }
                let response = value.get("response").unwrap_or(&value);
                if let Some(usage) = response.get("usage") {
                    let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
                    let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
                    if input_tokens.is_some() || output_tokens.is_some() {
                        self.pending.push(StreamEvent::TokenUsage {
                            input_tokens,
                            output_tokens,
                            cache_read_input_tokens: usage
                                .get("input_tokens_details")
                                .and_then(|details| details.get("cached_tokens"))
                                .and_then(Value::as_u64),
                            cache_creation_input_tokens: None,
                        });
                    }
                }
                let reason = response
                    .get("incomplete_details")
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.pending.push(StreamEvent::MessageEnd {
                    stop_reason: reason,
                });
                self.ended = true;
            }
            "response.failed" | "response.error" | "error" => {
                if self.thinking {
                    self.pending.push(StreamEvent::ThinkingEnd);
                    self.thinking = false;
                }
                let message = value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .or_else(|| value.get("message").and_then(Value::as_str))
                    .unwrap_or("OpenCode Go Responses API error");
                self.pending.push(StreamEvent::Error {
                    message: message.to_string(),
                    retry_after_secs: None,
                });
                self.ended = true;
            }
            _ => {}
        }
    }

    fn emit_tool(&mut self, state: ToolState, fallback_id: &str) {
        if state.name.is_empty() {
            return;
        }
        self.pending.push(StreamEvent::ToolUseStart {
            id: sanitize_tool_id(if state.call_id.is_empty() {
                fallback_id
            } else {
                &state.call_id
            }),
            name: state.name,
        });
        self.pending
            .push(StreamEvent::ToolInputDelta(if state.arguments.is_empty() {
                "{}".to_string()
            } else {
                state.arguments
            }));
        self.pending.push(StreamEvent::ToolUseEnd);
    }

    fn next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.pending.first().cloned() {
            self.pending.remove(0);
            return Some(event);
        }
        while let Some((pos, separator_len)) = [
            self.buffer.find("\n\n").map(|pos| (pos, 2)),
            self.buffer.find("\r\n\r\n").map(|pos| (pos, 4)),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|(pos, _)| *pos)
        {
            let raw = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + separator_len..].to_string();
            let data = raw
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            self.parse_event(&data);
            if let Some(event) = self.pending.first().cloned() {
                self.pending.remove(0);
                return Some(event);
            }
        }
        None
    }
}

impl Stream for ResponsesStream {
    type Item = anyhow::Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.next_event() {
                return Poll::Ready(Some(Ok(event)));
            }
            if self.ended {
                return Poll::Ready(None);
            }
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&bytes));
                }
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("Stream error: {error}"))));
                }
                Poll::Ready(None) => {
                    if !self.buffer.trim().is_empty() {
                        let tail = std::mem::take(&mut self.buffer);
                        self.parse_event(tail.trim());
                        if let Some(event) = self.pending.first().cloned() {
                            self.pending.remove(0);
                            return Poll::Ready(Some(Ok(event)));
                        }
                    }
                    if !self.ended {
                        if self.thinking {
                            self.pending.push(StreamEvent::ThinkingEnd);
                            self.thinking = false;
                            if let Some(event) = self.pending.first().cloned() {
                                self.pending.remove(0);
                                return Poll::Ready(Some(Ok(event)));
                            }
                        }
                        self.ended = true;
                        return Poll::Ready(Some(Ok(StreamEvent::MessageEnd {
                            stop_reason: None,
                        })));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[test]
    fn only_open_code_go_responses_models_use_responses() {
        assert!(model_uses_responses_api(
            "https://opencode.ai/zen/go/v1",
            Some("opencode-go"),
            "gpt-5.6-luna"
        ));
        assert!(!model_uses_responses_api(
            "https://opencode.ai/zen/go/v1",
            Some("opencode-go"),
            "kimi-k2.7-code"
        ));
        assert!(!model_uses_responses_api(
            "https://opencode.ai/zen/v1",
            Some("opencode"),
            "gpt-5.6-luna"
        ));
    }

    #[test]
    fn request_uses_responses_input_and_instructions() {
        let request = build_request(
            &[Message::user("hello")],
            &[],
            "system",
            "gpt-5.6-luna",
            Some("high"),
            Some(128),
            Value::clone,
        );
        assert_eq!(request["model"], "gpt-5.6-luna");
        assert_eq!(request["instructions"], "system");
        assert_eq!(request["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(request["reasoning"]["effort"], "high");
        assert_eq!(request["max_output_tokens"], 128);
    }

    #[test]
    fn responses_stream_handles_crlf_reasoning_text_and_usage() {
        let payload = concat!(
            "event: response.reasoning.delta\r\n",
            "data: {\"type\":\"response.reasoning.delta\",\"delta\":\"plan\"}\r\n",
            "\r\n",
            "event: response.output_text.delta\r\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"done\"}\r\n",
            "\r\n",
            "event: response.completed\r\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\r\n",
            "\r\n",
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let events = runtime.block_on(async {
            let mut stream =
                ResponsesStream::new(futures::stream::iter(vec![Ok(Bytes::from(payload))]));
            let mut events = Vec::new();
            while let Some(event) = stream.next().await {
                events.push(event.expect("response event"));
            }
            events
        });
        assert!(matches!(events[0], StreamEvent::ThinkingStart));
        assert!(matches!(events[1], StreamEvent::ThinkingDelta(ref text) if text == "plan"));
        assert!(matches!(events[2], StreamEvent::ThinkingEnd));
        assert!(matches!(events[3], StreamEvent::TextDelta(ref text) if text == "done"));
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::TokenUsage {
                input_tokens: Some(3),
                output_tokens: Some(2),
                ..
            }
        )));
        assert!(matches!(
            events.last(),
            Some(StreamEvent::MessageEnd { .. })
        ));
    }
}
