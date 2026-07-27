use anyhow::Result;
use bytes::Bytes;
use futures::Stream;
use jcode_message_types::StreamEvent;
use serde_json::Value;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;

use crate::{PinSource, ProviderPin};

fn truncated_stream_payload_context(data: &str) -> String {
    jcode_core::util::truncate_str(&data.trim().replace('\n', "\\n"), 240).to_string()
}

/// Pop the next complete SSE event off the front of `buffer`.
///
/// Accepts both `\n\n` and `\r\n\r\n` event delimiters. Only handling `\n\n`
/// meant CRLF streams accumulated many events into one blob (see #565).
/// Draining in place avoids the O(buffer^2) copy of reassigning the buffer.
fn take_sse_event(buffer: &mut String) -> Option<String> {
    let crlf = buffer.find("\r\n\r\n");
    let lf = buffer.find("\n\n");
    let (pos, sep_len) = match (crlf, lf) {
        (Some(c), Some(l)) if c <= l => (c, 4),
        (Some(c), None) => (c, 4),
        (_, Some(l)) => (l, 2),
        (None, None) => return None,
    };
    let event = buffer[..pos].to_string();
    buffer.drain(..pos + sep_len);
    Some(event)
}

pub struct OpenRouterStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    /// Carries incomplete multi-byte UTF-8 sequences across chunk boundaries
    /// so split CJK characters are not dropped (#609).
    utf8: jcode_core::util::Utf8StreamDecoder,
    /// A JSON object the upstream proxy split across two SSE events, held back
    /// so it can be joined with the next event's payload (#609).
    partial_json: Option<String>,
    pending: VecDeque<StreamEvent>,
    tool_call_accumulators: std::collections::BTreeMap<u64, ToolCallAccumulator>,
    /// Track if we've emitted the provider info (only emit once)
    provider_emitted: bool,
    model: String,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    reasoning_buffer: String,
    finish_reason: Option<String>,
    message_end_emitted: bool,
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

impl OpenRouterStream {
    pub fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        model: String,
        provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            utf8: jcode_core::util::Utf8StreamDecoder::new(),
            partial_json: None,
            pending: VecDeque::new(),
            tool_call_accumulators: std::collections::BTreeMap::new(),
            provider_emitted: false,
            model,
            provider_pin,
            reasoning_buffer: String::new(),
            finish_reason: None,
            message_end_emitted: false,
        }
    }

    fn queue_message_end(&mut self) {
        if self.message_end_emitted {
            return;
        }

        self.flush_tool_call_accumulators();
        self.message_end_emitted = true;
        self.pending.push_back(StreamEvent::MessageEnd {
            stop_reason: self.finish_reason.take(),
        });
    }

    fn observe_provider(&mut self, provider: &str) {
        let mut pin = self
            .provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = pin.as_ref() {
            if existing.source == PinSource::Explicit && existing.model == self.model {
                return;
            }
            if existing.source == PinSource::Observed
                && existing.model == self.model
                && existing.provider == provider
            {
                return;
            }
        }

        *pin = Some(ProviderPin {
            model: self.model.clone(),
            provider: provider.to_string(),
            source: PinSource::Observed,
            allow_fallbacks: true,
            last_cache_read: None,
        });
    }

    fn refresh_cache_pin(&mut self, provider: &str) {
        let mut pin = self
            .provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = pin.as_mut()
            && existing.model == self.model
            && existing.provider == provider
        {
            existing.last_cache_read = Some(Instant::now());
        }
    }

    fn push_completed_tool_call(&mut self, tc: ToolCallAccumulator) {
        if tc.id.trim().is_empty() {
            jcode_logging::warn(&format!(
                "OpenRouter SSE dropped incomplete tool call for model {}: missing id (name={} args_len={})",
                self.model,
                tc.name,
                tc.arguments.len()
            ));
            return;
        }

        if tc.name.trim().is_empty() {
            jcode_logging::warn(&format!(
                "OpenRouter SSE dropped incomplete tool call for model {}: missing name (id={} args_len={})",
                self.model,
                tc.id,
                tc.arguments.len()
            ));
            return;
        }

        self.pending.push_back(StreamEvent::ToolUseStart {
            id: tc.id,
            name: tc.name,
        });
        self.pending
            .push_back(StreamEvent::ToolInputDelta(tc.arguments));
        self.pending.push_back(StreamEvent::ToolUseEnd);
    }

    fn flush_tool_call_accumulators(&mut self) {
        let calls = std::mem::take(&mut self.tool_call_accumulators);
        for (_index, tc) in calls {
            self.push_completed_tool_call(tc);
        }
    }

    fn apply_tool_call_delta(
        &mut self,
        index: u64,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
    ) {
        let incoming_id = id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        if self
            .tool_call_accumulators
            .get(&index)
            .is_some_and(|existing| {
                incoming_id.as_ref().is_some_and(|incoming_id| {
                    !existing.id.is_empty() && existing.id != *incoming_id
                })
            })
            && let Some(previous) = self.tool_call_accumulators.remove(&index)
        {
            self.push_completed_tool_call(previous);
        }

        let tc = self.tool_call_accumulators.entry(index).or_default();

        if tc.id.is_empty()
            && let Some(incoming_id) = incoming_id
        {
            tc.id = incoming_id;
        }

        if tc.name.trim().is_empty()
            && let Some(incoming_name) = name.map(str::trim).filter(|value| !value.is_empty())
        {
            tc.name = incoming_name.to_string();
        }

        if let Some(args) = arguments {
            tc.arguments.push_str(args);
        }
    }

    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }

        while let Some(event_str) = take_sse_event(&mut self.buffer) {
            // Collect every `data:` line in the event. Keeping only the last one
            // silently dropped content whenever multiple events landed in a
            // single parsed chunk (see #565).
            let mut data_lines = Vec::new();
            let mut saw_done = false;
            for line in event_str.lines() {
                if let Some(d) = jcode_core::util::sse_data_line(line.trim_end_matches('\r')) {
                    if d.trim() == "[DONE]" {
                        saw_done = true;
                    } else {
                        data_lines.push(d);
                    }
                }
            }

            if data_lines.is_empty() {
                if saw_done {
                    self.queue_message_end();
                    return self.pending.pop_front();
                }
                continue;
            }

            // Each `data:` line is its own JSON payload here. Push the extras
            // back onto the front of the buffer as standalone events so none of
            // them is dropped, then handle the first one now.
            let data = data_lines[0].to_string();
            let mut requeued = String::new();
            for extra in &data_lines[1..] {
                requeued.push_str("data: ");
                requeued.push_str(extra);
                requeued.push_str("\n\n");
            }
            if saw_done {
                requeued.push_str("data: [DONE]\n\n");
            }
            if !requeued.is_empty() {
                self.buffer.insert_str(0, &requeued);
            }
            // Re-join a JSON object the proxy split across two SSE events.
            let data = match self.partial_json.take() {
                Some(mut partial) => {
                    partial.push_str(&data);
                    partial
                }
                None => data,
            };
            let data = data.as_str();

            let parsed: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(error) => {
                    // Some proxies drop the `\n\n` event separator or split an
                    // object across two events, so a whole chunk of deltas was
                    // being discarded here (#609). Try to recover the individual
                    // objects before giving up.
                    if let Some(split) = jcode_core::util::split_concatenated_json(data) {
                        let mut requeued = String::new();
                        for object in &split.objects {
                            requeued.push_str("data: ");
                            requeued.push_str(object);
                            requeued.push_str("\n\n");
                        }
                        if let Some(partial) = &split.trailing_partial {
                            // Hold the truncated object back and prepend it to the
                            // next event's payload rather than dropping it.
                            self.partial_json = Some(partial.clone());
                        }
                        if !requeued.is_empty() {
                            self.buffer.insert_str(0, &requeued);
                            continue;
                        }
                        if split.trailing_partial.is_some() {
                            continue;
                        }
                    }
                    jcode_logging::warn(&format!(
                        "OpenRouter SSE JSON parse failed for model {}: {} payload={} ",
                        self.model,
                        error,
                        truncated_stream_payload_context(data)
                    ));
                    continue;
                }
            };

            // Extract upstream provider info (only emit once)
            // OpenRouter returns "provider" field indicating which provider handled the request
            if !self.provider_emitted
                && let Some(provider) = parsed.get("provider").and_then(|p| p.as_str())
            {
                self.provider_emitted = true;
                self.observe_provider(provider);
                self.pending.push_back(StreamEvent::UpstreamProvider {
                    provider: provider.to_string(),
                });
            }

            // Check for error
            if let Some(error) = parsed.get("error") {
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("OpenRouter error")
                    .to_string();
                return Some(StreamEvent::Error {
                    message,
                    retry_after_secs: None,
                });
            }

            // Parse choices
            if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                for choice in choices {
                    if let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) {
                        if let Some(reasoning_content) = delta
                            .get("reasoning_content")
                            .or_else(|| delta.get("reasoning"))
                            .and_then(|c| c.as_str())
                            && !reasoning_content.is_empty()
                        {
                            let reasoning_delta =
                                if reasoning_content.starts_with(&self.reasoning_buffer) {
                                    &reasoning_content[self.reasoning_buffer.len()..]
                                } else {
                                    reasoning_content
                                };
                            self.reasoning_buffer = reasoning_content.to_string();
                            if !reasoning_delta.is_empty() {
                                self.pending.push_back(StreamEvent::ThinkingDelta(
                                    reasoning_delta.to_string(),
                                ));
                            }
                        }

                        // Text content
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                            && !content.is_empty()
                        {
                            self.pending
                                .push_back(StreamEvent::TextDelta(content.to_string()));
                        }

                        // Tool calls
                        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array())
                        {
                            for tc in tool_calls {
                                let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                                let function = tc.get("function");
                                self.apply_tool_call_delta(
                                    index,
                                    tc.get("id").and_then(|i| i.as_str()),
                                    function
                                        .and_then(|f| f.get("name"))
                                        .and_then(|n| n.as_str()),
                                    function
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|a| a.as_str()),
                                );
                            }
                        }
                    }

                    // Check for finish reason
                    if let Some(finish_reason) =
                        choice.get("finish_reason").and_then(|f| f.as_str())
                    {
                        let finish_reason = finish_reason.trim();
                        if !finish_reason.is_empty() {
                            self.finish_reason = Some(finish_reason.to_string());
                        }
                        // Emit any pending tool calls.
                        self.flush_tool_call_accumulators();

                        // Don't emit MessageEnd here - wait for [DONE]
                    }
                }
            }

            // Extract usage if present
            if let Some(usage) = parsed.get("usage") {
                let input_tokens = usage.get("prompt_tokens").and_then(|t| t.as_u64());
                let output_tokens = usage.get("completion_tokens").and_then(|t| t.as_u64());

                // OpenRouter returns cached tokens in various formats depending on provider:
                // - "cached_tokens" (OpenRouter's unified field)
                // - "prompt_tokens_details.cached_tokens" (OpenAI-style)
                // - "cache_read_input_tokens" (Anthropic-style, passed through)
                let cache_read_input_tokens = usage
                    .get("cached_tokens")
                    .and_then(|t| t.as_u64())
                    .or_else(|| {
                        usage
                            .get("prompt_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(|t| t.as_u64())
                    })
                    .or_else(|| {
                        usage
                            .get("cache_read_input_tokens")
                            .and_then(|t| t.as_u64())
                    });

                // Cache creation tokens (Anthropic-style, passed through for some providers)
                let cache_creation_input_tokens = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64());

                // Refresh cache pin when we see cache activity
                if (cache_read_input_tokens.is_some() || cache_creation_input_tokens.is_some())
                    && let Some(provider) = parsed.get("provider").and_then(|p| p.as_str())
                {
                    self.refresh_cache_pin(provider);
                }

                if input_tokens.is_some()
                    || output_tokens.is_some()
                    || cache_read_input_tokens.is_some()
                {
                    self.pending.push_back(StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    });
                }
            }

            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
        }

        None
    }
}

impl Stream for OpenRouterStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.parse_next_event() {
                return Poll::Ready(Some(Ok(event)));
            }

            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    let text = self.utf8.decode(&bytes);
                    self.buffer.push_str(&text);
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("Stream error: {}", e))));
                }
                Poll::Ready(None) => {
                    // Flush any bytes held back mid-character, then force-close a
                    // buffer that never received a trailing blank line (#609).
                    let tail = self.utf8.flush();
                    if !tail.is_empty() {
                        self.buffer.push_str(&tail);
                    }
                    if !self.buffer.trim().is_empty() && !self.buffer.ends_with("\n\n") {
                        self.buffer.push_str("\n\n");
                        if let Some(event) = self.parse_next_event() {
                            return Poll::Ready(Some(Ok(event)));
                        }
                    }
                    // Stream ended - emit any pending tool call
                    self.flush_tool_call_accumulators();
                    if let Some(event) = self.pending.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    }
                    if !self.message_end_emitted {
                        self.message_end_emitted = true;
                        return Poll::Ready(Some(Ok(StreamEvent::MessageEnd {
                            stop_reason: self.finish_reason.take(),
                        })));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn drain_text(stream: &mut OpenRouterStream) -> String {
        let mut text = String::new();
        while let Some(event) = stream.parse_next_event() {
            match event {
                StreamEvent::TextDelta(delta) => text.push_str(&delta),
                StreamEvent::MessageEnd { .. } => break,
                _ => {}
            }
        }
        text
    }

    fn test_stream() -> OpenRouterStream {
        OpenRouterStream::new(
            futures::stream::empty(),
            "test-model".to_string(),
            Arc::new(std::sync::Mutex::new(None)),
        )
    }

    #[test]
    fn take_sse_event_splits_crlf_delimited_events() {
        let mut buffer = "data: a\r\n\r\ndata: b\r\n\r\n".to_string();
        assert_eq!(take_sse_event(&mut buffer).as_deref(), Some("data: a"));
        assert_eq!(take_sse_event(&mut buffer).as_deref(), Some("data: b"));
        assert_eq!(take_sse_event(&mut buffer), None);
    }

    #[test]
    fn parse_next_event_keeps_all_content_across_crlf_batched_events() {
        let mut stream = test_stream();
        stream.buffer = [
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}",
            "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}",
            "data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}",
            "data: [DONE]",
            "",
        ]
        .join("\r\n\r\n");

        assert_eq!(drain_text(&mut stream), "hello world!");
    }

    #[test]
    fn parse_next_event_keeps_all_data_lines_within_one_event() {
        // Several data: lines inside one \n\n-delimited block must all be kept.
        let mut stream = test_stream();
        stream.buffer = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"foo\"}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"bar\"}}]}\n",
            "data: [DONE]\n\n"
        )
        .to_string();

        assert_eq!(drain_text(&mut stream), "foobar");
    }

    /// Issue #609: proxies that drop the event separator, split an object across
    /// two events, or split a multi-byte character across TCP chunks must not
    /// cause silent data loss.
    #[test]
    fn concatenated_json_in_one_event_keeps_both_deltas() {
        let mut stream = test_stream();
        stream.buffer = concat!(
            r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#,
            r#"{"choices":[{"delta":{"content":" world"}}]}"#,
            "\n\ndata: [DONE]\n\n"
        )
        .to_string();

        assert_eq!(drain_text(&mut stream), "hello world");
    }

    #[test]
    fn concatenated_json_with_embedded_data_prefix_keeps_both_deltas() {
        let mut stream = test_stream();
        stream.buffer = concat!(
            r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#,
            r#"data: {"choices":[{"delta":{"content":" world"}}]}"#,
            "\n\ndata: [DONE]\n\n"
        )
        .to_string();

        assert_eq!(drain_text(&mut stream), "hello world");
    }

    #[test]
    fn object_split_across_two_events_is_rejoined() {
        let mut stream = test_stream();
        stream.buffer = concat!(
            r#"data: {"choices":[{"delta":{"content":"Hello "#,
            "\n\n",
            r#"data: world"}}]}"#,
            "\n\ndata: [DONE]\n\n"
        )
        .to_string();

        assert_eq!(drain_text(&mut stream), "Hello world");
    }

    #[test]
    fn tool_call_arguments_split_across_events_are_not_truncated() {
        // The reported symptom was `arguments must be a JSON object, got null`.
        let mut stream = test_stream();
        stream.buffer = concat!(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"write","arguments":"{\"path\":\"a.txt\""#,
            "\n\n",
            r#"data: ,\"content\":\"hi\"}"}}]}}]}"#,
            "\n\ndata: [DONE]\n\n"
        )
        .to_string();

        let mut args = String::new();
        while let Some(event) = stream.parse_next_event() {
            if let StreamEvent::ToolInputDelta(delta) = event {
                args.push_str(&delta);
            }
        }
        let parsed: Value =
            serde_json::from_str(&args).expect("tool arguments should be complete JSON");
        assert_eq!(parsed["path"], "a.txt");
        assert_eq!(parsed["content"], "hi");
    }

    #[test]
    fn multibyte_chars_split_across_tcp_chunks_survive_poll_next() {
        // Drive real bytes through poll_next, splitting mid-character.
        let payload =
            "data: {\"choices\":[{\"delta\":{\"content\":\"读取文件\"}}]}\n\ndata: [DONE]\n\n";
        let bytes = payload.as_bytes();
        // Split at every offset to sweep the chunk-boundary state space.
        for split in 0..bytes.len() {
            let chunks: Vec<Result<Bytes, reqwest::Error>> = vec![
                Ok(Bytes::copy_from_slice(&bytes[..split])),
                Ok(Bytes::copy_from_slice(&bytes[split..])),
            ];
            let mut stream = OpenRouterStream::new(
                futures::stream::iter(chunks),
                "test-model".to_string(),
                Arc::new(std::sync::Mutex::new(None)),
            );
            let text = futures::executor::block_on(async {
                let mut text = String::new();
                while let Some(Ok(event)) = stream.next().await {
                    if let StreamEvent::TextDelta(delta) = event {
                        text.push_str(&delta);
                    }
                }
                text
            });
            assert_eq!(text, "读取文件", "lost content at split offset {split}");
        }
    }

    #[test]
    fn stream_ending_without_a_blank_line_still_flushes_the_last_event() {
        let payload = "data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}";
        let chunks: Vec<Result<Bytes, reqwest::Error>> =
            vec![Ok(Bytes::copy_from_slice(payload.as_bytes()))];
        let mut stream = OpenRouterStream::new(
            futures::stream::iter(chunks),
            "test-model".to_string(),
            Arc::new(std::sync::Mutex::new(None)),
        );
        let text = futures::executor::block_on(async {
            let mut text = String::new();
            while let Some(Ok(event)) = stream.next().await {
                if let StreamEvent::TextDelta(delta) = event {
                    text.push_str(&delta);
                }
            }
            text
        });
        assert_eq!(text, "tail");
    }

    #[test]
    fn parse_next_event_ignores_malformed_json_chunks() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let mut stream = OpenRouterStream::new(
            futures::stream::empty(),
            "test-model".to_string(),
            provider_pin,
        );
        stream.buffer = "data: {not-json}

"
        .to_string();

        let event = stream.parse_next_event();

        assert!(event.is_none());
        assert!(stream.pending.is_empty());
        assert!(stream.tool_call_accumulators.is_empty());
    }

    #[test]
    fn parse_next_event_accepts_reasoning_delta_alias() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let mut stream = OpenRouterStream::new(
            futures::stream::empty(),
            "test-model".to_string(),
            provider_pin,
        );
        stream.buffer =
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"thinking\"}}]}\n\n".to_string();

        let event = stream.parse_next_event();

        assert!(matches!(event, Some(StreamEvent::ThinkingDelta(text)) if text == "thinking"));
    }

    #[test]
    fn parse_next_event_propagates_finish_reason_to_message_end() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let mut stream = OpenRouterStream::new(
            futures::stream::empty(),
            "test-model".to_string(),
            provider_pin,
        );
        stream.buffer =
            "data: {\"choices\":[{\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n".to_string();

        let event = stream.parse_next_event();

        assert!(matches!(
            event,
            Some(StreamEvent::MessageEnd { stop_reason: Some(reason) }) if reason == "length"
        ));
    }

    #[test]
    fn stream_eof_emits_message_end_with_finish_reason_without_done() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let bytes = Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"max_tokens\"}]}\n\n",
        );
        let mut stream = OpenRouterStream::new(
            futures::stream::once(async move { Ok(bytes) }),
            "test-model".to_string(),
            provider_pin,
        );

        let event = futures::executor::block_on(stream.next());

        assert!(matches!(
            event,
            Some(Ok(StreamEvent::MessageEnd { stop_reason: Some(reason) })) if reason == "max_tokens"
        ));
        assert!(futures::executor::block_on(stream.next()).is_none());
    }

    #[test]
    fn parse_next_event_coalesces_repeated_tool_call_id_chunks() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let mut stream =
            OpenRouterStream::new(futures::stream::empty(), "glm-5".to_string(), provider_pin);

        let chunk1 = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "bash", "arguments": ""}
                    }]
                }
            }]
        });
        let chunk2 = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"arguments": "{\"command\""}
                    }]
                }
            }]
        });
        let chunk3 = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"arguments": ":\"echo ok\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        stream.buffer =
            format!("data: {chunk1}\n\ndata: {chunk2}\n\ndata: {chunk3}\n\ndata: [DONE]\n\n");

        let mut events = Vec::new();
        for _ in 0..8 {
            if let Some(event) = stream.parse_next_event() {
                events.push(event);
            } else {
                break;
            }
        }

        assert_eq!(events.len(), 4, "events: {events:?}");
        assert!(matches!(
            &events[0],
            StreamEvent::ToolUseStart { id, name } if id == "call_1" && name == "bash"
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::ToolInputDelta(args) if args == "{\"command\":\"echo ok\"}"
        ));
        assert!(matches!(events[2], StreamEvent::ToolUseEnd));
        assert!(matches!(
            &events[3],
            StreamEvent::MessageEnd { stop_reason } if stop_reason.as_deref() == Some("tool_calls")
        ));
        assert!(stream.tool_call_accumulators.is_empty());
    }
}
