//! `streamGenerateContent?alt=sse` support. The Cloud Code backend streams a
//! turn as a series of partial `generateContent` responses; reasoning parts are
//! handed to the caller as they arrive so thinking renders live, while the
//! whole turn is still assembled into one response for the turn checks
//! (malformed, pseudo-tool-call, empty) that need to see all of it.

use jcode_provider_gemini::{
    CodeAssistGenerateResponse, GeminiCandidate, GeminiContent, GeminiPart, GeminiPromptFeedback,
    GeminiUsageMetadata, VertexGenerateContentResponse,
};
use serde_json::Value;

/// Splits a raw SSE byte stream into complete `data:` payloads. Bytes are
/// buffered until a full line arrives, so a multi-byte character split across
/// network chunks is never decoded in halves.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    data: String,
}

impl SseDecoder {
    /// Feed raw bytes; returns every event payload completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(newline) = self.buf.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=newline).collect();
            self.line(&line[..line.len() - 1], &mut events);
        }
        events
    }

    /// Flush a final event the server did not terminate with a blank line.
    pub fn finish(&mut self) -> Option<String> {
        let mut events = Vec::new();
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            self.line(&line, &mut events);
        }
        self.line(b"", &mut events);
        events.pop()
    }

    fn line(&mut self, line: &[u8], events: &mut Vec<String>) {
        let line = String::from_utf8_lossy(line);
        let line = line.strip_suffix('\r').unwrap_or(&line);
        if line.is_empty() {
            if !self.data.is_empty() {
                events.push(std::mem::take(&mut self.data));
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
}

/// Parse one streamed chunk. The backend reports mid-stream failures as an
/// `error` object in place of a response.
pub fn parse_stream_chunk(data: &str) -> Result<CodeAssistGenerateResponse, String> {
    let value: Value = serde_json::from_str(data)
        .map_err(|err| format!("invalid Antigravity stream chunk: {err}"))?;
    stream_chunk_from_value(value)
}

/// Chunks from a non-SSE body: a single response or a JSON array of them.
pub fn stream_chunks_from_json(value: Value) -> Result<Vec<CodeAssistGenerateResponse>, String> {
    match value {
        Value::Array(items) => items.into_iter().map(stream_chunk_from_value).collect(),
        other => Ok(vec![stream_chunk_from_value(other)?]),
    }
}

fn stream_chunk_from_value(value: Value) -> Result<CodeAssistGenerateResponse, String> {
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| error.to_string());
        return Err(format!("Antigravity stream error: {message}"));
    }
    serde_json::from_value(value).map_err(|err| format!("invalid Antigravity stream chunk: {err}"))
}

/// Assembles streamed chunks into the single response `generateContent` would
/// have returned.
#[derive(Debug, Default)]
pub struct StreamedTurn {
    trace_id: Option<String>,
    saw_candidate: bool,
    parts: Vec<GeminiPart>,
    finish_reason: Option<String>,
    finish_message: Option<String>,
    usage: Option<GeminiUsageMetadata>,
    prompt_feedback: Option<GeminiPromptFeedback>,
}

impl StreamedTurn {
    /// Merge one chunk and return the reasoning text it carried, in order.
    pub fn push_chunk(&mut self, chunk: CodeAssistGenerateResponse) -> Vec<String> {
        let mut thinking = Vec::new();
        if chunk.trace_id.is_some() {
            self.trace_id = chunk.trace_id;
        }
        let Some(response) = chunk.response else {
            return thinking;
        };
        if response.usage_metadata.is_some() {
            self.usage = response.usage_metadata;
        }
        if response.prompt_feedback.is_some() {
            self.prompt_feedback = response.prompt_feedback;
        }
        let Some(candidate) = response.candidates.and_then(|mut c| c.drain(..).next()) else {
            return thinking;
        };
        self.saw_candidate = true;
        if candidate.finish_reason.is_some() {
            self.finish_reason = candidate.finish_reason;
        }
        if candidate.finish_message.is_some() {
            self.finish_message = candidate.finish_message;
        }
        for part in candidate.content.map(|c| c.parts).unwrap_or_default() {
            if part.thought.unwrap_or(false)
                && let Some(text) = part.text.as_deref().filter(|text| !text.is_empty())
            {
                thinking.push(text.to_string());
            }
            self.merge_part(part);
        }
        thinking
    }

    /// Consecutive text parts of the same kind are one logical part split
    /// across chunks; join them so the turn checks see whole text.
    fn merge_part(&mut self, part: GeminiPart) {
        let is_text = |p: &GeminiPart| {
            p.text.is_some()
                && p.function_call.is_none()
                && p.function_response.is_none()
                && p.inline_data.is_none()
        };
        if let Some(last) = self.parts.last_mut()
            && is_text(last)
            && is_text(&part)
            && last.thought.unwrap_or(false) == part.thought.unwrap_or(false)
        {
            if let (Some(text), Some(more)) = (last.text.as_mut(), part.text.as_deref()) {
                text.push_str(more);
            }
            if part.thought_signature.is_some() {
                last.thought_signature = part.thought_signature;
            }
            return;
        }
        self.parts.push(part);
    }

    pub fn finish(self) -> CodeAssistGenerateResponse {
        let candidates = self.saw_candidate.then(|| {
            vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: "model".to_string(),
                    parts: self.parts,
                }),
                finish_reason: self.finish_reason,
                finish_message: self.finish_message,
            }]
        });
        CodeAssistGenerateResponse {
            trace_id: self.trace_id,
            response: Some(VertexGenerateContentResponse {
                candidates,
                prompt_feedback: self.prompt_feedback,
                usage_metadata: self.usage,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(json: &str) -> CodeAssistGenerateResponse {
        parse_stream_chunk(json).unwrap()
    }

    #[test]
    fn sse_decoder_handles_split_lines_crlf_and_multibyte_text() {
        let mut decoder = SseDecoder::default();
        let payload = "data: {\"t\":\"नमस्ते\"}\r\n\r\n".as_bytes();
        let (head, tail) = payload.split_at(14);
        assert!(decoder.push(head).is_empty());
        assert_eq!(decoder.push(tail), vec!["{\"t\":\"नमस्ते\"}".to_string()]);
        assert!(decoder.push(b": keep-alive\n\ndata: {\"a\":1}").is_empty());
        assert_eq!(decoder.finish(), Some("{\"a\":1}".to_string()));
    }

    #[test]
    fn streamed_turn_yields_thinking_live_and_assembles_one_response() {
        let mut turn = StreamedTurn::default();
        let first = turn.push_chunk(chunk(
            r#"{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"Let me ","thought":true}]}}]}}"#,
        ));
        assert_eq!(first, vec!["Let me ".to_string()]);
        let second = turn.push_chunk(chunk(
            r#"{"response":{"candidates":[{"content":{"parts":[{"text":"think.","thought":true},{"text":"Hello"}]}}]}}"#,
        ));
        assert_eq!(second, vec!["think.".to_string()]);
        let third = turn.push_chunk(chunk(
            r#"{"response":{"candidates":[{"content":{"parts":[{"text":" there","thoughtSignature":"sig"},{"functionCall":{"name":"bash","args":{"command":"ls"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":4}}}"#,
        ));
        assert!(third.is_empty());

        let response = turn.finish();
        let inner = response.response.unwrap();
        assert_eq!(
            inner.usage_metadata.unwrap().candidates_token_count,
            Some(4)
        );
        let candidate = &inner.candidates.unwrap()[0];
        assert_eq!(candidate.finish_reason.as_deref(), Some("STOP"));
        let parts = &candidate.content.as_ref().unwrap().parts;
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].text.as_deref(), Some("Let me think."));
        assert_eq!(parts[0].thought, Some(true));
        assert_eq!(parts[1].text.as_deref(), Some("Hello there"));
        assert_eq!(parts[1].thought_signature.as_deref(), Some("sig"));
        assert_eq!(parts[2].function_call.as_ref().unwrap().name, "bash");
    }

    #[test]
    fn stream_without_candidates_assembles_no_candidates() {
        let mut turn = StreamedTurn::default();
        turn.push_chunk(chunk(r#"{"traceId":"t1","response":{}}"#));
        let response = turn.finish();
        assert_eq!(response.trace_id.as_deref(), Some("t1"));
        assert!(response.response.unwrap().candidates.is_none());
    }

    #[test]
    fn stream_error_chunk_is_an_error() {
        let err = parse_stream_chunk(r#"{"error":{"code":429,"message":"quota exhausted"}}"#)
            .unwrap_err();
        assert!(err.contains("quota exhausted"), "{err}");
    }

    #[test]
    fn non_sse_body_accepts_a_single_response_or_an_array() {
        let one = stream_chunks_from_json(serde_json::json!({"response": {}})).unwrap();
        assert_eq!(one.len(), 1);
        let many = stream_chunks_from_json(serde_json::json!([{"response": {}}, {"response": {}}]))
            .unwrap();
        assert_eq!(many.len(), 2);
    }
}
