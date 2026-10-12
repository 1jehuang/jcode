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

/// Kimi endpoints (OpenRouter `moonshotai/kimi-k2*`, the Kimi coding
/// endpoint's `kimi-for-coding`) are the only known senders of cumulative
/// reasoning snapshots, where every chunk repeats the full text so far and the
/// previous chunk must be stripped as a prefix (introduced for #322). Every
/// other backend streams incremental deltas that must be appended verbatim;
/// stripping unconditionally corrupted adjacent repeated deltas (#1779).
fn sends_cumulative_reasoning_snapshots(model: &str) -> bool {
    crate::is_kimi_model(model) || model.to_ascii_lowercase().contains("kimi")
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
    thought_signature: Option<String>,
    started: bool,
    emitted_id: String,
    emitted_arguments: usize,
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

    fn push_completed_tool_call(&mut self, index: u64, mut tc: ToolCallAccumulator) {
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

        Self::queue_tool_progress(&mut self.pending, index, &mut tc);
        self.pending.push_back(StreamEvent::ToolUseEndFor {
            id: tc.emitted_id.clone(),
        });
        if let Some(signature) = tc.thought_signature.filter(|value| !value.is_empty()) {
            self.pending.push_back(StreamEvent::ToolUseSignatureFor {
                id: tc.emitted_id,
                signature,
            });
        }
    }

    fn queue_tool_progress(
        pending: &mut VecDeque<StreamEvent>,
        index: u64,
        tc: &mut ToolCallAccumulator,
    ) {
        if !tc.started {
            // Positional fallback IDs restart every response. Keep the raw ID
            // in the accumulator for repeated-provider-ID comparisons.
            let id = if tc.id == format!("{}:{index}", tc.name) {
                jcode_core::id::new_id("toolu")
            } else {
                tc.id.clone()
            };
            tc.emitted_id = id.clone();
            pending.push_back(StreamEvent::ToolUseStart {
                id,
                name: tc.name.clone(),
            });
            tc.started = true;
        }
        let delta = &tc.arguments[tc.emitted_arguments..];
        if !delta.is_empty() {
            pending.push_back(StreamEvent::ToolInputDeltaFor {
                id: tc.emitted_id.clone(),
                delta: delta.to_string(),
            });
            tc.emitted_arguments = tc.arguments.len();
        }
    }

    fn flush_tool_call_accumulators(&mut self) {
        let calls = std::mem::take(&mut self.tool_call_accumulators);
        for (index, tc) in calls {
            self.push_completed_tool_call(index, tc);
        }
    }

    fn apply_tool_call_delta(
        &mut self,
        index: u64,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
        thought_signature: Option<&str>,
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
            self.push_completed_tool_call(index, previous);
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

        if let Some(signature) = thought_signature.filter(|value| !value.is_empty()) {
            tc.thought_signature = Some(signature.to_string());
        }
        if !tc.id.trim().is_empty() && !tc.name.trim().is_empty() {
            Self::queue_tool_progress(&mut self.pending, index, tc);
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
                            // Only Kimi endpoints are known to stream reasoning
                            // as cumulative snapshots (each chunk repeats the
                            // full text so far). Everywhere else deltas are
                            // incremental and must be appended verbatim:
                            // stripping any chunk that starts with the previous
                            // one silently dropped adjacent repeated deltas
                            // ("9","4","4","0" arrived as "940", #1779).
                            let reasoning_delta =
                                if sends_cumulative_reasoning_snapshots(&self.model)
                                    && reasoning_content.starts_with(&self.reasoning_buffer)
                                {
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
                                // Compatible APIs may send a complete JSON value
                                // instead of the usual string fragment. Preserve
                                // it, including invalid null/array arguments, so
                                // validation sees what the provider actually sent.
                                let arguments =
                                    function.and_then(|f| f.get("arguments")).map(|value| {
                                        match value.as_str() {
                                            Some(fragment) => std::borrow::Cow::Borrowed(fragment),
                                            None => std::borrow::Cow::Owned(value.to_string()),
                                        }
                                    });
                                self.apply_tool_call_delta(
                                    index,
                                    tc.get("id").and_then(|i| i.as_str()),
                                    function
                                        .and_then(|f| f.get("name"))
                                        .and_then(|n| n.as_str()),
                                    arguments.as_deref(),
                                    tc.get("extra_content")
                                        .and_then(|value| value.get("google"))
                                        .and_then(|value| value.get("thought_signature"))
                                        .and_then(|value| value.as_str()),
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
                        // Some proxies emit a finish reason after every delta, even
                        // while tool arguments are still streaming (#1326). Keep the
                        // accumulators until [DONE] or EOF, just like MessageEnd.
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
#[path = "stream_tests.rs"]
mod tests;
