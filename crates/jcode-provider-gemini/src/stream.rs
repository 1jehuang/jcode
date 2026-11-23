//! Shared SSE framing and chunk-to-[`StreamEvent`] decoding for Gemini
//! `streamGenerateContent?alt=sse` (both Cloud Code / Antigravity envelope
//! responses and direct Developer API responses).

use crate::{
    CodeAssistGenerateResponse, GeminiPromptFeedback, GeminiUsageMetadata,
    VertexGenerateContentResponse,
};
use anyhow::Result;
use jcode_message_types::StreamEvent;
use std::time::Instant;

/// Wire framing used by the SSE endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiSseEnvelope {
    /// Cloud Code / Antigravity `v1internal:streamGenerateContent?alt=sse`,
    /// where each SSE `data:` frame is a [`CodeAssistGenerateResponse`]
    /// (`{"response": {...}, "traceId": "..."}`).
    CodeAssist,
    /// Official Gemini Developer API `v1beta/models/{model}:streamGenerateContent?alt=sse`,
    /// where each SSE `data:` frame is a bare [`VertexGenerateContentResponse`].
    DeveloperApi,
}

/// Incremental Server-Sent Events parser for `streamGenerateContent?alt=sse`.
///
/// Feed UTF-8 text decoded from HTTP body chunks (using a UTF-8 stream decoder
/// so multi-byte codepoints split across chunks are preserved). Emits one
/// string per complete non-empty SSE `data:` event.
#[derive(Debug, Default)]
pub struct GeminiSseParser {
    line_buf: String,
    data_lines: Vec<String>,
}

impl GeminiSseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a UTF-8 text chunk and return all complete SSE `data` payloads.
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        let mut events = Vec::new();
        self.line_buf.push_str(chunk);
        while let Some(pos) = self.line_buf.find('\n') {
            let line = self.line_buf[..pos].trim_end_matches('\r').to_string();
            self.line_buf.drain(..=pos);
            if let Some(payload) = self.process_line(&line) {
                events.push(payload);
            }
        }
        events
    }

    /// Flush any trailing line/event at EOF.
    pub fn finish(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        if !self.line_buf.is_empty() {
            let line = std::mem::take(&mut self.line_buf);
            let trimmed = line.trim_end_matches('\r');
            if let Some(payload) = self.process_line(trimmed) {
                events.push(payload);
            }
        }
        if let Some(payload) = self.flush_event() {
            events.push(payload);
        }
        events
    }

    fn process_line(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            return self.flush_event();
        }
        if line.starts_with(':') {
            return None;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let value = rest.strip_prefix(' ').unwrap_or(rest);
            self.data_lines.push(value.to_string());
        }
        None
    }

    fn flush_event(&mut self) -> Option<String> {
        if self.data_lines.is_empty() {
            return None;
        }
        let joined = self.data_lines.join("\n");
        self.data_lines.clear();
        let trimmed = joined.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            return None;
        }
        Some(joined)
    }
}

/// Decode a single SSE `data` JSON payload into a [`VertexGenerateContentResponse`].
///
/// Surfaces inline JSON error frames (`{"error": {"message": ...}}`) as `Err`.
pub fn decode_sse_chunk(
    envelope: GeminiSseEnvelope,
    data: &str,
) -> Result<Option<VertexGenerateContentResponse>> {
    // Fast check for inline SSE error payloads before typed decoding.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(data)
        && let Some(err_obj) = value.get("error")
        && !err_obj.is_null()
    {
        let message = err_obj
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| err_obj.to_string());
        anyhow::bail!("Gemini stream error: {message}");
    }

    match envelope {
        GeminiSseEnvelope::CodeAssist => {
            let wrapped: CodeAssistGenerateResponse = serde_json::from_str(data)?;
            Ok(wrapped.response)
        }
        GeminiSseEnvelope::DeveloperApi => {
            let response: VertexGenerateContentResponse = serde_json::from_str(data)?;
            Ok(Some(response))
        }
    }
}

/// Whether `text` starts with the recovery marker `[previous tool call] <name>(`
/// used when unsigned historical tool calls were downgraded to text (#845).
pub fn is_pseudo_tool_call_text(text: &str) -> bool {
    let Some(call) = text.trim_start().strip_prefix("[previous tool call]") else {
        return false;
    };
    let call = call.trim_start();
    let name_len = call
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .map(char::len_utf8)
        .sum::<usize>();
    name_len > 0 && call[name_len..].trim_start().starts_with('(')
}

/// Whether `text` could still become a pseudo-tool-call once more streamed
/// characters are appended. Returns `false` immediately on chunk #1 for any
/// normal response that does not start with `[`.
pub fn may_become_pseudo_tool_call(text: &str) -> bool {
    const MARKER: &str = "[previous tool call]";
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.len() < MARKER.len() {
        return MARKER.starts_with(trimmed);
    }
    let Some(after_marker) = trimmed.strip_prefix(MARKER) else {
        return false;
    };
    let call = after_marker.trim_start();
    if call.is_empty() {
        return true;
    }
    let name_len = call
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .map(char::len_utf8)
        .sum::<usize>();
    if name_len == 0 {
        return false;
    }
    let rest = call[name_len..].trim_start();
    rest.is_empty() || rest.starts_with('(')
}

/// Result of feeding one decoded SSE response chunk into [`GeminiStreamAccumulator`].
#[derive(Debug)]
pub struct GeminiChunkResult {
    pub events: Vec<StreamEvent>,
    /// True when the pseudo-tool-call guard matched before any answer text was
    /// emitted; callers should flush `events` (e.g. closing an open thinking
    /// block) and retry with forced native function calling.
    pub pseudo_tool_call_detected: bool,
}

/// Final outcome when the SSE stream reaches EOF.
#[derive(Debug)]
pub enum GeminiStreamFinish {
    /// Normal completion; `events` ends with `StreamEvent::MessageEnd`.
    Completed { events: Vec<StreamEvent> },
    /// The stream ended with an abnormal `finish_reason` (e.g.
    /// `MALFORMED_FUNCTION_CALL`) and produced no answer text or tool calls.
    RetryableEmpty {
        finish_reason: String,
        finish_message: Option<String>,
        events: Vec<StreamEvent>,
    },
    /// A held prefix buffer resolved to a pseudo-tool-call at EOF.
    PseudoToolCall { events: Vec<StreamEvent> },
    /// Non-retryable stream failure (prompt blocked, no candidates, or safety
    /// stop with no content).
    Error {
        message: String,
        events: Vec<StreamEvent>,
    },
}

/// Stateful converter from Gemini SSE `VertexGenerateContentResponse` chunks
/// into jcode [`StreamEvent`]s.
pub struct GeminiStreamAccumulator {
    provider_label: &'static str,
    lowercase_stop_reason: bool,
    guard_pseudo_tool_call: bool,
    emitted_resolved_model: bool,
    thinking_open: bool,
    thinking_started_at: Option<Instant>,
    pending_signature: Option<String>,
    saw_candidate: bool,
    saw_content: bool,
    produced_output: bool,
    finish_reason: Option<String>,
    finish_message: Option<String>,
    prompt_feedback: Option<GeminiPromptFeedback>,
    pseudo_prefix_buf: String,
    pseudo_guard_settled: bool,
}

impl GeminiStreamAccumulator {
    pub fn new(
        provider_label: &'static str,
        lowercase_stop_reason: bool,
        guard_pseudo_tool_call: bool,
    ) -> Self {
        Self {
            provider_label,
            lowercase_stop_reason,
            guard_pseudo_tool_call,
            emitted_resolved_model: false,
            thinking_open: false,
            thinking_started_at: None,
            pending_signature: None,
            saw_candidate: false,
            saw_content: false,
            produced_output: false,
            finish_reason: None,
            finish_message: None,
            prompt_feedback: None,
            pseudo_prefix_buf: String::new(),
            pseudo_guard_settled: !guard_pseudo_tool_call,
        }
    }

    fn close_thinking(&mut self, events: &mut Vec<StreamEvent>) {
        if self.thinking_open {
            self.thinking_open = false;
            let duration_secs = self
                .thinking_started_at
                .take()
                .map(|started| started.elapsed().as_secs_f64())
                .unwrap_or(0.0);
            events.push(StreamEvent::ThinkingEnd);
            events.push(StreamEvent::ThinkingDone { duration_secs });
        }
    }

    fn emit_usage(usage: &GeminiUsageMetadata, events: &mut Vec<StreamEvent>) {
        let input_tokens = usage.input_token_count();
        let output_tokens = usage.output_token_count();
        let cache_read_input_tokens = usage.cached_content_token_count;
        if input_tokens.is_some() || output_tokens.is_some() || cache_read_input_tokens.is_some() {
            events.push(StreamEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens: None,
            });
        }
    }

    /// Process one decoded `VertexGenerateContentResponse` chunk from SSE.
    pub fn push_response(
        &mut self,
        response: VertexGenerateContentResponse,
        next_call_id: &mut impl FnMut() -> String,
    ) -> GeminiChunkResult {
        let mut events = Vec::new();

        if !self.emitted_resolved_model
            && let Some(model) = response
                .model_version
                .as_deref()
                .map(str::trim)
                .filter(|m| !m.is_empty())
        {
            self.emitted_resolved_model = true;
            events.push(StreamEvent::ResolvedModel {
                model: model.to_string(),
            });
        }

        if let Some(usage) = response.usage_metadata.as_ref() {
            Self::emit_usage(usage, &mut events);
        }

        if let Some(feedback) = response.prompt_feedback {
            self.prompt_feedback = Some(feedback);
        }

        let Some(candidate) = response
            .candidates
            .and_then(|mut candidates| candidates.drain(..).next())
        else {
            return GeminiChunkResult {
                events,
                pseudo_tool_call_detected: false,
            };
        };

        self.saw_candidate = true;
        if let Some(reason) = candidate
            .finish_reason
            .filter(|reason| !reason.trim().is_empty())
        {
            self.finish_reason = Some(reason);
        }
        if let Some(message) = candidate
            .finish_message
            .filter(|msg| !msg.trim().is_empty())
        {
            self.finish_message = Some(message);
        }

        if let Some(content) = candidate.content {
            self.saw_content = true;
            for part in content.parts {
                let part_signature = part
                    .thought_signature
                    .as_ref()
                    .filter(|sig| !sig.is_empty())
                    .cloned();

                if part.is_thought() {
                    if let Some(text) = part.text.filter(|text| !text.is_empty()) {
                        if !self.thinking_open {
                            self.thinking_open = true;
                            self.thinking_started_at = Some(Instant::now());
                            events.push(StreamEvent::ThinkingStart);
                        }
                        events.push(StreamEvent::ThinkingDelta(text));
                    }
                    if let Some(signature) = part_signature {
                        self.pending_signature = Some(signature);
                    }
                    continue;
                }

                if let Some(text) = part.text.filter(|text| !text.is_empty()) {
                    self.close_thinking(&mut events);
                    if self.guard_pseudo_tool_call && !self.pseudo_guard_settled {
                        self.pseudo_prefix_buf.push_str(&text);
                        if is_pseudo_tool_call_text(&self.pseudo_prefix_buf) {
                            self.pseudo_prefix_buf.clear();
                            return GeminiChunkResult {
                                events,
                                pseudo_tool_call_detected: true,
                            };
                        }
                        if !may_become_pseudo_tool_call(&self.pseudo_prefix_buf) {
                            self.pseudo_guard_settled = true;
                            let flushed = std::mem::take(&mut self.pseudo_prefix_buf);
                            self.produced_output = true;
                            events.push(StreamEvent::TextDelta(flushed));
                        }
                    } else {
                        self.produced_output = true;
                        events.push(StreamEvent::TextDelta(text));
                    }
                }

                if let Some(function_call) = part.function_call {
                    self.close_thinking(&mut events);
                    if !self.pseudo_prefix_buf.is_empty() {
                        let flushed = std::mem::take(&mut self.pseudo_prefix_buf);
                        self.pseudo_guard_settled = true;
                        events.push(StreamEvent::TextDelta(flushed));
                    } else {
                        self.pseudo_guard_settled = true;
                    }
                    self.produced_output = true;
                    let signature = part_signature.or_else(|| self.pending_signature.take());
                    let raw_call_id = function_call
                        .id
                        .filter(|id| !id.is_empty())
                        .unwrap_or_else(&mut *next_call_id);
                    let call_id = jcode_message_types::sanitize_tool_id(&raw_call_id);
                    events.push(StreamEvent::ToolUseStart {
                        id: call_id,
                        name: function_call.name,
                    });
                    events.push(StreamEvent::ToolInputDelta(function_call.args.to_string()));
                    events.push(StreamEvent::ToolUseEnd);
                    if let Some(signature) = signature {
                        events.push(StreamEvent::ToolUseSignature(signature));
                    }
                } else if let Some(signature) = part_signature {
                    // Standalone signature part; remember it for the next
                    // function call in this candidate or emit as a
                    // ThinkingSignatureDelta at end-of-stream.
                    self.pending_signature = Some(signature);
                }
            }
        }

        GeminiChunkResult {
            events,
            pseudo_tool_call_detected: false,
        }
    }

    /// Finalize the stream after EOF and produce trailing events + terminal status.
    pub fn finish(&mut self) -> GeminiStreamFinish {
        let mut events = Vec::new();

        if !self.pseudo_prefix_buf.is_empty() {
            let buffered = std::mem::take(&mut self.pseudo_prefix_buf);
            if self.guard_pseudo_tool_call && is_pseudo_tool_call_text(&buffered) {
                self.close_thinking(&mut events);
                return GeminiStreamFinish::PseudoToolCall { events };
            }
            self.produced_output = true;
            events.push(StreamEvent::TextDelta(buffered));
        }

        self.close_thinking(&mut events);

        if let Some(signature) = self.pending_signature.take() {
            events.push(StreamEvent::ThinkingSignatureDelta(signature));
        }

        if !self.saw_candidate {
            if let Some(feedback) = self.prompt_feedback.as_ref() {
                let block_reason = feedback.block_reason.as_deref().unwrap_or("unspecified");
                let detail = feedback
                    .block_reason_message
                    .as_deref()
                    .filter(|msg| !msg.trim().is_empty())
                    .map(|msg| format!(": {}", msg.trim()))
                    .unwrap_or_default();
                return GeminiStreamFinish::Error {
                    message: format!(
                        "{} blocked the prompt ({block_reason}){detail}",
                        self.provider_label
                    ),
                    events,
                };
            }
            return GeminiStreamFinish::Error {
                message: format!(
                    "{} returned no candidates for streamGenerateContent",
                    self.provider_label
                ),
                events,
            };
        }

        if !self.saw_content
            && matches!(
                self.finish_reason.as_deref(),
                Some("SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "RECITATION")
            )
        {
            let reason = self.finish_reason.as_deref().unwrap_or("unknown");
            let detail = self
                .finish_message
                .as_deref()
                .filter(|msg| !msg.trim().is_empty())
                .map(|msg| format!(": {}", msg.trim()))
                .unwrap_or_default();
            return GeminiStreamFinish::Error {
                message: format!(
                    "{} stopped without content ({reason}){detail}",
                    self.provider_label
                ),
                events,
            };
        }

        if !self.produced_output {
            let abnormal = self
                .finish_reason
                .as_deref()
                .map(|reason| {
                    !matches!(
                        reason.to_ascii_uppercase().as_str(),
                        "STOP" | "MAX_TOKENS" | "FINISH_REASON_UNSPECIFIED" | ""
                    )
                })
                .unwrap_or(false);
            if abnormal {
                return GeminiStreamFinish::RetryableEmpty {
                    finish_reason: self
                        .finish_reason
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    finish_message: self.finish_message.clone(),
                    events,
                };
            }
        }

        let stop_reason = self.finish_reason.as_ref().map(|reason| {
            if self.lowercase_stop_reason {
                reason.to_lowercase()
            } else {
                reason.clone()
            }
        });
        events.push(StreamEvent::MessageEnd { stop_reason });
        GeminiStreamFinish::Completed { events }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_handles_crlf_split_chunks_comments_and_done() {
        let mut parser = GeminiSseParser::new();
        let a = parser.push(": keepalive\r\ndata: {\"response\":");
        assert!(a.is_empty());
        let b = parser
            .push("{\"modelVersion\":\"gemini-4-neon\"}}\r\n\r\ndata: [DONE]\n\ndata: {\"a\":1}");
        assert_eq!(
            b,
            vec!["{\"response\":{\"modelVersion\":\"gemini-4-neon\"}}"]
        );
        let c = parser.finish();
        assert_eq!(c, vec!["{\"a\":1}"]);
    }

    #[test]
    fn streams_thoughts_live_then_answer_text_and_signature() {
        let mut acc = GeminiStreamAccumulator::new("Antigravity", false, true);
        let mut counter = 0usize;
        let mut next_id = || {
            counter += 1;
            format!("call-{counter}")
        };

        let chunk1 = decode_sse_chunk(
            GeminiSseEnvelope::CodeAssist,
            r#"{"response":{"modelVersion":"gemini-4-neon","candidates":[{"content":{"role":"model","parts":[{"text":"Step 1 reasoning\n\n","thought":true}]}}]}}"#,
        )
        .unwrap()
        .unwrap();
        let r1 = acc.push_response(chunk1, &mut next_id);
        assert!(!r1.pseudo_tool_call_detected);
        assert!(matches!(
            r1.events.as_slice(),
            [
                StreamEvent::ResolvedModel { model },
                StreamEvent::ThinkingStart,
                StreamEvent::ThinkingDelta(t),
            ] if model == "gemini-4-neon" && t == "Step 1 reasoning\n\n"
        ));

        let chunk2 = decode_sse_chunk(
            GeminiSseEnvelope::CodeAssist,
            r#"{"response":{"modelVersion":"gemini-4-neon","candidates":[{"content":{"role":"model","parts":[{"text":"Step 2 reasoning\n\n","thought":true}]}}]}}"#,
        )
        .unwrap()
        .unwrap();
        let r2 = acc.push_response(chunk2, &mut next_id);
        assert!(matches!(
            r2.events.as_slice(),
            [StreamEvent::ThinkingDelta(t)] if t == "Step 2 reasoning\n\n"
        ));

        let chunk3 = decode_sse_chunk(
            GeminiSseEnvelope::CodeAssist,
            r#"{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"Final answer."}]}}]}}"#,
        )
        .unwrap()
        .unwrap();
        let r3 = acc.push_response(chunk3, &mut next_id);
        assert!(matches!(
            r3.events.as_slice(),
            [
                StreamEvent::ThinkingEnd,
                StreamEvent::ThinkingDone { .. },
                StreamEvent::TextDelta(t),
            ] if t == "Final answer."
        ));

        let chunk4 = decode_sse_chunk(
            GeminiSseEnvelope::CodeAssist,
            r#"{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"","thoughtSignature":"sig-xyz"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":42}}}"#,
        )
        .unwrap()
        .unwrap();
        let r4 = acc.push_response(chunk4, &mut next_id);
        assert!(matches!(
            r4.events.as_slice(),
            [StreamEvent::TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(47),
                ..
            }]
        ));

        match acc.finish() {
            GeminiStreamFinish::Completed { events } => {
                assert!(matches!(
                    events.as_slice(),
                    [
                        StreamEvent::ThinkingSignatureDelta(sig),
                        StreamEvent::MessageEnd {
                            stop_reason: Some(reason)
                        }
                    ] if sig == "sig-xyz" && reason == "STOP"
                ));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn developer_api_thought_then_signed_tool_call_streams_cleanly() {
        let mut acc = GeminiStreamAccumulator::new("Gemini", true, false);
        let mut next_id = || "gen-1".to_string();

        let c1 = decode_sse_chunk(
            GeminiSseEnvelope::DeveloperApi,
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Let me check the weather.","thought":true}]}}]}"#,
        )
        .unwrap()
        .unwrap();
        let r1 = acc.push_response(c1, &mut next_id);
        assert!(matches!(
            r1.events.as_slice(),
            [StreamEvent::ThinkingStart, StreamEvent::ThinkingDelta(t)] if t == "Let me check the weather."
        ));

        let c2 = decode_sse_chunk(
            GeminiSseEnvelope::DeveloperApi,
            r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"get_weather","args":{"city":"SF"}},"thoughtSignature":"tool-sig-1"}]},"finishReason":"STOP"}]}"#,
        )
        .unwrap()
        .unwrap();
        let r2 = acc.push_response(c2, &mut next_id);
        assert!(matches!(
            r2.events.as_slice(),
            [
                StreamEvent::ThinkingEnd,
                StreamEvent::ThinkingDone { .. },
                StreamEvent::ToolUseStart { id, name },
                StreamEvent::ToolInputDelta(_),
                StreamEvent::ToolUseEnd,
                StreamEvent::ToolUseSignature(sig),
            ] if id == "gen-1" && name == "get_weather" && sig == "tool-sig-1"
        ));

        match acc.finish() {
            GeminiStreamFinish::Completed { events } => {
                assert!(matches!(
                    events.as_slice(),
                    [StreamEvent::MessageEnd {
                        stop_reason: Some(reason)
                    }] if reason == "stop"
                ));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn thought_only_followed_by_malformed_function_call_is_retryable_empty() {
        let mut acc = GeminiStreamAccumulator::new("Antigravity", false, true);
        let mut next_id = || "unused".to_string();

        let c1 = decode_sse_chunk(
            GeminiSseEnvelope::CodeAssist,
            r#"{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"Thinking about tool...","thought":true}]}}]}}"#,
        )
        .unwrap()
        .unwrap();
        let _ = acc.push_response(c1, &mut next_id);

        let c2 = decode_sse_chunk(
            GeminiSseEnvelope::CodeAssist,
            r#"{"response":{"candidates":[{"finishReason":"MALFORMED_FUNCTION_CALL","finishMessage":"bad call"}]}}"#,
        )
        .unwrap()
        .unwrap();
        let _ = acc.push_response(c2, &mut next_id);

        match acc.finish() {
            GeminiStreamFinish::RetryableEmpty {
                finish_reason,
                finish_message,
                events,
            } => {
                assert_eq!(finish_reason, "MALFORMED_FUNCTION_CALL");
                assert_eq!(finish_message.as_deref(), Some("bad call"));
                assert!(matches!(
                    events.as_slice(),
                    [StreamEvent::ThinkingEnd, StreamEvent::ThinkingDone { .. }]
                ));
            }
            other => panic!("expected RetryableEmpty, got {other:?}"),
        }
    }

    #[test]
    fn pseudo_tool_call_guard_catches_split_chunks_without_buffering_normal_text() {
        let mut acc = GeminiStreamAccumulator::new("Antigravity", false, true);
        let mut next_id = || "unused".to_string();

        // Split `[previous tool call] read(` across 3 chunks: must NOT emit TextDelta.
        for chunk in ["[prev", "ious tool call] ", "read({\"path\":\"a\"})"] {
            let resp = VertexGenerateContentResponse {
                model_version: None,
                candidates: Some(vec![crate::GeminiCandidate {
                    content: Some(crate::GeminiContent {
                        role: "model".into(),
                        parts: vec![crate::GeminiPart {
                            text: Some(chunk.into()),
                            ..Default::default()
                        }],
                    }),
                    finish_reason: None,
                    finish_message: None,
                }]),
                prompt_feedback: None,
                usage_metadata: None,
            };
            let res = acc.push_response(resp, &mut next_id);
            assert!(res.events.is_empty(), "must not leak pseudo tool call text");
            if chunk.contains('(') {
                assert!(res.pseudo_tool_call_detected);
            } else {
                assert!(!res.pseudo_tool_call_detected);
            }
        }

        // Normal text starting with `[1]` flushes immediately on chunk #1.
        let mut normal = GeminiStreamAccumulator::new("Antigravity", false, true);
        let resp = VertexGenerateContentResponse {
            model_version: None,
            candidates: Some(vec![crate::GeminiCandidate {
                content: Some(crate::GeminiContent {
                    role: "model".into(),
                    parts: vec![crate::GeminiPart {
                        text: Some("[1] Hello".into()),
                        ..Default::default()
                    }],
                }),
                finish_reason: None,
                finish_message: None,
            }]),
            prompt_feedback: None,
            usage_metadata: None,
        };
        let res = normal.push_response(resp, &mut next_id);
        assert!(!res.pseudo_tool_call_detected);
        assert!(matches!(
            res.events.as_slice(),
            [StreamEvent::TextDelta(t)] if t == "[1] Hello"
        ));
    }

    #[test]
    fn decode_sse_chunk_surfaces_inline_error_frame() {
        let err = decode_sse_chunk(
            GeminiSseEnvelope::CodeAssist,
            r#"{"error":{"code":500,"message":"Internal upstream error"}}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Internal upstream error"));
    }

    #[test]
    fn empty_thought_part_with_signature_does_not_emit_empty_thinking_delta() {
        let mut acc = GeminiStreamAccumulator::new("Gemini", false, false);
        let mut next_id = || "call_1".to_string();

        let r1 = acc.push_response(
            serde_json::from_value(serde_json::json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [
                            {"thought": true, "text": "", "thoughtSignature": "sig-only"},
                            {"text": "Final answer"}
                        ]
                    },
                    "finishReason": "STOP"
                }]
            }))
            .unwrap(),
            &mut next_id,
        );

        assert!(
            !r1.events.iter().any(|e| matches!(
                e,
                StreamEvent::ThinkingStart
                    | StreamEvent::ThinkingDelta(_)
                    | StreamEvent::ThinkingEnd
            )),
            "empty thought part must not emit ThinkingStart/ThinkingDelta: {:?}",
            r1.events
        );
        assert!(matches!(
            r1.events.as_slice(),
            [StreamEvent::TextDelta(t)] if t == "Final answer"
        ));

        let finish = acc.finish();
        let GeminiStreamFinish::Completed { events, .. } = finish else {
            panic!("expected Completed, got {finish:?}");
        };
        assert!(
            events.iter().any(
                |e| matches!(e, StreamEvent::ThinkingSignatureDelta(sig) if sig == "sig-only")
            ),
            "signature from empty thought part must still be preserved: {events:?}"
        );
    }

    #[test]
    fn thinking_request_mode_sets_dynamic_budget_or_level_and_downgrades() {
        use crate::{GeminiThinkingRequestMode, build_generation_config_for_thoughts_with_mode};

        let flash_full = build_generation_config_for_thoughts_with_mode(
            "gemini-3-flash",
            GeminiThinkingRequestMode::Full,
        )
        .unwrap()
        .thinking_config
        .unwrap();
        assert_eq!(flash_full.include_thoughts, Some(true));
        assert_eq!(flash_full.thinking_budget, Some(-1));
        assert_eq!(flash_full.thinking_level, None);

        let pro_high = build_generation_config_for_thoughts_with_mode(
            "gemini-3.1-pro-high",
            GeminiThinkingRequestMode::Full,
        )
        .unwrap()
        .thinking_config
        .unwrap();
        assert_eq!(pro_high.include_thoughts, Some(true));
        assert_eq!(pro_high.thinking_budget, None);
        assert_eq!(pro_high.thinking_level.as_deref(), Some("HIGH"));

        let claude_full = build_generation_config_for_thoughts_with_mode(
            "claude-opus-5-5-high",
            GeminiThinkingRequestMode::Full,
        )
        .unwrap()
        .thinking_config
        .unwrap();
        assert_eq!(claude_full.include_thoughts, Some(true));
        assert_eq!(claude_full.thinking_budget, None);
        assert_eq!(claude_full.thinking_level, None);

        assert_eq!(
            GeminiThinkingRequestMode::Full
                .downgrade_for_error("gemini-3-flash", "Unknown field thinkingBudget"),
            Some(GeminiThinkingRequestMode::IncludeThoughtsOnly)
        );
        assert_eq!(
            GeminiThinkingRequestMode::Full
                .downgrade_for_error("gemini-3-flash", "Unknown name thinkingConfig"),
            Some(GeminiThinkingRequestMode::Disabled)
        );
        assert_eq!(
            GeminiThinkingRequestMode::IncludeThoughtsOnly
                .downgrade_for_error("gemini-3-flash", "Unknown name thinkingConfig"),
            Some(GeminiThinkingRequestMode::Disabled)
        );
        assert_eq!(
            GeminiThinkingRequestMode::Full
                .downgrade_for_error("claude-opus-5-5-high", "Unknown field thinkingBudget"),
            Some(GeminiThinkingRequestMode::Disabled)
        );
        assert_eq!(
            GeminiThinkingRequestMode::Full
                .downgrade_for_error("gemini-3-flash", "HTTP 429 rate limited"),
            None
        );
    }
}
