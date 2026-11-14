//! OpenCode Go's Anthropic Messages protocol adapter.
//!
//! Part of the OpenCode Go catalog (the MiniMax and Qwen families) is only
//! served on the Anthropic `/messages` route, so those models fail when the
//! runtime sends them to `/chat/completions` (issue #1224). This adapter keeps
//! the third protocol arm self-contained, mirroring how
//! [`super::opencode_go_responses`] handles the `/responses` allowlist:
//! request building and streaming decode live here, while the endpoint choice
//! is made once by [`super::api_protocol::select_protocol`].
//!
//! Only the subset jcode actually replays is translated (text, image,
//! tool_use, tool_result, signed thinking). Provider-native server-tool blocks
//! have no Messages representation on this gateway and are dropped rather than
//! sent as something the API would reject.

use bytes::Bytes;
use futures::Stream;
use jcode_message_types::{
    ContentBlock, Message, Role, StreamEvent, ToolDefinition, sanitize_tool_id,
};
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};

/// `max_tokens` is mandatory on the Messages API. OpenCode Go does not publish
/// a per-model output cap, so profiles without an explicit limit get this
/// conservative default instead of an invalid request.
pub(crate) const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Minimal user turn appended when the conversation would otherwise end on an
/// assistant message. Some Anthropic-shaped models reject an assistant tail
/// ("does not support assistant message prefill") and jcode never intends to
/// prefill.
pub(crate) const CONTINUATION_USER_TURN: &str = "Continue.";

pub(crate) fn build_request(
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
    model: &str,
    max_tokens: u32,
    sanitize_schema: impl Fn(&Value) -> Value,
) -> Value {
    let mut request = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": build_messages(messages),
    });

    if !system.trim().is_empty() {
        request["system"] = Value::String(system.to_string());
    }
    if !tools.is_empty() {
        request["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": sanitize_schema(&tool.input_schema),
                    })
                })
                .collect(),
        );
    }

    request
}

fn build_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for message in messages {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        let mut content = build_content(&message.content);
        if content.is_empty() {
            continue;
        }
        if role == "user" {
            // The Messages API requires every tool_result answering the
            // previous assistant turn to lead the user message. The sort is
            // stable so sibling text/images keep their relative order.
            content.sort_by_key(|block| u8::from(block["type"] != "tool_result"));
        }
        if let Some(last) = out.last_mut()
            && last["role"] == role
            && let Some(collected) = last["content"].as_array_mut()
        {
            collected.extend(content);
            continue;
        }
        out.push(serde_json::json!({ "role": role, "content": content }));
    }

    if out
        .last()
        .is_some_and(|message| message["role"] == "assistant")
    {
        out.push(serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": CONTINUATION_USER_TURN }],
        }));
    }

    out
}

fn build_content(blocks: &[ContentBlock]) -> Vec<Value> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                out.push(serde_json::json!({ "type": "text", "text": text }));
            }
            ContentBlock::AnthropicThinking {
                thinking,
                signature,
            } => {
                out.push(serde_json::json!({
                    "type": "thinking",
                    "thinking": thinking,
                    "signature": signature,
                }));
            }
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                out.push(serde_json::json!({
                    "type": "tool_use",
                    "id": sanitize_tool_id(id),
                    "name": name,
                    "input": if input.is_object() { input.clone() } else { serde_json::json!({}) },
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let mut result = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": sanitize_tool_id(tool_use_id),
                    "content": content,
                });
                if *is_error == Some(true) {
                    result["is_error"] = Value::Bool(true);
                }
                out.push(result);
            }
            ContentBlock::Image { media_type, data } => {
                out.push(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    },
                }));
            }
            _ => {}
        }
    }
    out
}

pub(crate) struct MessagesStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    // Retain raw bytes until a complete SSE frame arrives: transport chunks can
    // split a UTF-8 character at any byte.
    buffer: Vec<u8>,
    pending: Vec<StreamEvent>,
    thinking: bool,
    tool_active: bool,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    saw_message_end: bool,
    ended: bool,
}

impl MessagesStream {
    pub(crate) fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: Vec::new(),
            pending: Vec::new(),
            thinking: false,
            tool_active: false,
            input_tokens: None,
            output_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            saw_message_end: false,
            ended: false,
        }
    }

    /// Decode one complete SSE frame. The frame's `event:` line names the
    /// event; the JSON payload repeats it in `type`, which is the fallback.
    fn parse_frame(&mut self, raw: &str) {
        let mut event_type = String::new();
        let mut data_lines = Vec::new();
        for line in raw.lines() {
            let line = line.trim_start();
            if let Some(rest) = line.strip_prefix("event:") {
                event_type = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data_lines.push(rest.trim_start().to_string());
            }
        }
        let data = data_lines.join("\n");
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            return;
        };
        let kind = if event_type.is_empty() {
            value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            event_type
        };
        self.parse_event(&kind, &value);
    }

    fn parse_event(&mut self, kind: &str, value: &Value) {
        match kind {
            "message_start" => {
                if let Some(usage) = value.get("message").and_then(|m| m.get("usage")) {
                    self.input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
                    self.cache_read_input_tokens =
                        usage.get("cache_read_input_tokens").and_then(Value::as_u64);
                    self.cache_creation_input_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64);
                }
            }
            "content_block_start" => {
                let Some(block) = value.get("content_block") else {
                    return;
                };
                match block.get("type").and_then(Value::as_str) {
                    Some("thinking") | Some("redacted_thinking") => {
                        if !self.thinking {
                            self.pending.push(StreamEvent::ThinkingStart);
                            self.thinking = true;
                        }
                        if let Some(thinking) = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .filter(|text| !text.is_empty())
                        {
                            self.pending
                                .push(StreamEvent::ThinkingDelta(thinking.to_string()));
                        }
                    }
                    Some("tool_use") => {
                        let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        self.tool_active = true;
                        self.pending.push(StreamEvent::ToolUseStart {
                            id: sanitize_tool_id(id),
                            name: name.to_string(),
                        });
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let Some(delta) = value.get("delta") else {
                    return;
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            self.pending.push(StreamEvent::TextDelta(text.to_string()));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            self.pending
                                .push(StreamEvent::ToolInputDelta(partial.to_string()));
                        }
                    }
                    Some("thinking_delta") => {
                        if !self.thinking {
                            self.pending.push(StreamEvent::ThinkingStart);
                            self.thinking = true;
                        }
                        if let Some(thinking) = delta.get("thinking").and_then(Value::as_str) {
                            self.pending
                                .push(StreamEvent::ThinkingDelta(thinking.to_string()));
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(signature) = delta.get("signature").and_then(Value::as_str) {
                            self.pending
                                .push(StreamEvent::ThinkingSignatureDelta(signature.to_string()));
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if self.tool_active {
                    self.tool_active = false;
                    self.pending.push(StreamEvent::ToolUseEnd);
                } else if self.thinking {
                    self.thinking = false;
                    self.pending.push(StreamEvent::ThinkingEnd);
                }
            }
            "message_delta" => {
                if let Some(usage) = value.get("usage") {
                    if let Some(output) = usage.get("output_tokens").and_then(Value::as_u64) {
                        self.output_tokens = Some(output);
                    }
                    if let Some(input) = usage.get("input_tokens").and_then(Value::as_u64) {
                        self.input_tokens = Some(input);
                    }
                }
                self.push_usage();
                let stop_reason = value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.pending.push(StreamEvent::MessageEnd { stop_reason });
                self.saw_message_end = true;
            }
            "message_stop" => {
                if self.thinking {
                    self.thinking = false;
                    self.pending.push(StreamEvent::ThinkingEnd);
                }
                if !self.saw_message_end {
                    self.pending
                        .push(StreamEvent::MessageEnd { stop_reason: None });
                    self.saw_message_end = true;
                }
                self.ended = true;
            }
            "ping" => {
                self.pending.push(StreamEvent::ConnectionPhase {
                    phase: jcode_message_types::ConnectionPhase::Streaming,
                });
            }
            "error" => {
                let message = value
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .or_else(|| value.get("message").and_then(Value::as_str))
                    .unwrap_or("OpenCode Go Messages API error");
                self.pending.push(StreamEvent::Error {
                    message: message.to_string(),
                    retry_after_secs: None,
                });
                self.ended = true;
            }
            _ => {}
        }
    }

    fn push_usage(&mut self) {
        if self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.cache_read_input_tokens.is_none()
        {
            return;
        }
        self.pending.push(StreamEvent::TokenUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
        });
    }

    fn next_pending(&mut self) -> Option<StreamEvent> {
        if self.pending.is_empty() {
            None
        } else {
            Some(self.pending.remove(0))
        }
    }

    fn next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.next_pending() {
            return Some(event);
        }
        while let Some((pos, separator_len)) = [
            self.buffer
                .windows(2)
                .position(|bytes| bytes == b"\n\n")
                .map(|pos| (pos, 2)),
            self.buffer
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .map(|pos| (pos, 4)),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|(pos, _)| *pos)
        {
            let raw = String::from_utf8_lossy(&self.buffer[..pos]).into_owned();
            self.buffer.drain(..pos + separator_len);
            self.parse_frame(&raw);
            if let Some(event) = self.next_pending() {
                return Some(event);
            }
        }
        None
    }
}

impl Stream for MessagesStream {
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
                    self.buffer.extend_from_slice(&bytes);
                }
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("Stream error: {error}"))));
                }
                Poll::Ready(None) => {
                    if !self.buffer.is_empty() {
                        let tail = std::mem::take(&mut self.buffer);
                        self.parse_frame(String::from_utf8_lossy(&tail).trim());
                        if let Some(event) = self.next_pending() {
                            return Poll::Ready(Some(Ok(event)));
                        }
                    }
                    if !self.ended {
                        if self.thinking {
                            self.thinking = false;
                            self.pending.push(StreamEvent::ThinkingEnd);
                        }
                        if !self.saw_message_end {
                            self.saw_message_end = true;
                            self.pending
                                .push(StreamEvent::MessageEnd { stop_reason: None });
                        }
                        self.ended = true;
                        if let Some(event) = self.next_pending() {
                            return Poll::Ready(Some(Ok(event)));
                        }
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
    fn opencode_go_messages_request_uses_anthropic_wire_shape() {
        let tool = ToolDefinition::new(
            "read",
            "Read a file",
            serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        );
        let request = build_request(
            &[Message::user("hello")],
            &[tool],
            "system prompt",
            "minimax-m2.7",
            4096,
            Value::clone,
        );
        assert_eq!(request["model"], "minimax-m2.7");
        assert_eq!(request["max_tokens"], 4096);
        assert_eq!(request["stream"], true);
        assert_eq!(request["system"], "system prompt");
        assert_eq!(request["messages"][0]["role"], "user");
        assert_eq!(request["messages"][0]["content"][0]["type"], "text");
        assert_eq!(request["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(request["tools"][0]["name"], "read");
        assert_eq!(request["tools"][0]["input_schema"]["type"], "object");
        // The Messages API has no OpenAI chat fields.
        assert!(request.get("stream_options").is_none());
        assert!(request.get("tool_choice").is_none());
    }

    #[test]
    fn opencode_go_messages_request_keeps_tool_history_alternating() {
        let tool = ToolDefinition::new("read", "Read a file", serde_json::json!({}));
        let messages = [
            Message::user("inspect the file"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({"path": "a.txt"}),
                    thought_signature: None,
                }],
                timestamp: None,
                tool_duration_ms: None,
            },
            Message {
                role: Role::User,
                content: vec![
                    ContentBlock::Text {
                        text: "here is the result".to_string(),
                        cache_control: None,
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "call_1".to_string(),
                        content: "contents".to_string(),
                        is_error: None,
                    },
                ],
                timestamp: None,
                tool_duration_ms: None,
            },
        ];
        let request = build_request(
            &messages,
            &[tool],
            "",
            "qwen3.8-max",
            DEFAULT_MAX_TOKENS,
            Value::clone,
        );
        assert!(request.get("system").is_none());
        let sent = request["messages"].as_array().unwrap();
        assert_eq!(sent.len(), 3);
        // tool_use ids are sanitized but stable, and the result leads the turn.
        assert_eq!(sent[1]["content"][0]["type"], "tool_use");
        assert_eq!(sent[1]["content"][0]["input"]["path"], "a.txt");
        assert_eq!(sent[2]["content"][0]["type"], "tool_result");
        assert_eq!(sent[2]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(sent[2]["content"][1]["type"], "text");
    }

    #[test]
    fn opencode_go_messages_request_merges_same_role_and_ends_on_user() {
        let messages = [
            Message::user("first"),
            Message::user("second"),
            Message::assistant_text("partial"),
        ];
        let request = build_request(
            &messages,
            &[],
            "",
            "minimax-m2.5",
            DEFAULT_MAX_TOKENS,
            Value::clone,
        );
        let sent = request["messages"].as_array().unwrap();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0]["role"], "user");
        assert_eq!(sent[0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(sent[1]["role"], "assistant");
        assert_eq!(sent[2]["role"], "user");
        assert_eq!(sent[2]["content"][0]["text"], CONTINUATION_USER_TURN);
    }

    #[test]
    fn opencode_go_messages_stream_decodes_text_tool_usage_and_stop() {
        let payload = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12}}}\n",
            "\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
            "\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n",
            "\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_9\",\"name\":\"read\",\"input\":{}}}\n",
            "\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n",
            "\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n",
            "\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n",
            "\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n",
            "\n",
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let events = runtime.block_on(async {
            let mut stream =
                MessagesStream::new(futures::stream::iter(vec![Ok(Bytes::from(payload))]));
            let mut events = Vec::new();
            while let Some(event) = stream.next().await {
                events.push(event.expect("messages event"));
            }
            events
        });
        assert!(matches!(&events[0], StreamEvent::TextDelta(text) if text == "hi"));
        assert!(
            matches!(&events[1], StreamEvent::ToolUseStart { id, name } if id == "call_9" && name == "read")
        );
        assert!(
            matches!(&events[2], StreamEvent::ToolInputDelta(partial) if partial == "{\"path\":")
        );
        assert!(matches!(events[3], StreamEvent::ToolUseEnd));
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::TokenUsage {
                input_tokens: Some(12),
                output_tokens: Some(7),
                ..
            }
        )));
        assert!(events.iter().any(
            |event| matches!(event, StreamEvent::MessageEnd { stop_reason: Some(reason) } if reason == "tool_use")
        ));
    }

    #[test]
    fn opencode_go_messages_stream_preserves_unicode_at_every_byte_split() {
        let text = "café 中文 🦀 e\u{301}";
        let payload = format!(
            "event: content_block_delta\ndata: {}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": text},
            })
        )
        .into_bytes();
        for split in 0..=payload.len() {
            let chunks = vec![
                Ok(Bytes::copy_from_slice(&payload[..split])),
                Ok(Bytes::copy_from_slice(&payload[split..])),
            ];
            let events = futures::executor::block_on(
                MessagesStream::new(futures::stream::iter(chunks)).collect::<Vec<_>>(),
            )
            .into_iter()
            .collect::<anyhow::Result<Vec<_>>>()
            .expect("messages events");
            assert!(
                matches!(&events[0], StreamEvent::TextDelta(value) if value == text),
                "split {split}: {:?}",
                events[0]
            );
            assert!(matches!(events[1], StreamEvent::MessageEnd { .. }));
        }
    }
}
