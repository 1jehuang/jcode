use super::{
    FALLBACK_TOOL_CALL_COUNTER, NORMALIZED_NULL_TOOL_ARGUMENTS, RECOVERED_TEXT_WRAPPED_TOOL_CALLS,
    extract_error_with_retry, is_websocket_fallback_notice,
};

fn truncated_stream_payload_context(data: &str) -> String {
    crate::util::truncate_str(&data.trim().replace("\n", "\\n"), 240).to_string()
}
use crate::message::StreamEvent;
use anyhow::Result;
use bytes::Bytes;
use futures::Stream;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::task::{Context as TaskContext, Poll};

pub(super) fn parse_text_wrapped_tool_call(text: &str) -> Option<(String, String, String, String)> {
    let marker = "to=functions.";
    let marker_idx = text.find(marker)?;
    let after_marker = &text[marker_idx + marker.len()..];

    let mut tool_name_end = 0usize;
    for (idx, ch) in after_marker.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            tool_name_end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    if tool_name_end == 0 {
        return None;
    }

    let tool_name = after_marker[..tool_name_end].to_string();
    let remaining = &after_marker[tool_name_end..];
    let mut fallback: Option<(String, String, String, String)> = None;
    for (brace_idx, ch) in remaining.char_indices() {
        if ch != '{' {
            continue;
        }
        let slice = &remaining[brace_idx..];
        let mut stream = serde_json::Deserializer::from_str(slice).into_iter::<Value>();
        let parsed = match stream.next() {
            Some(Ok(value)) => value,
            Some(Err(_)) => continue,
            None => continue,
        };
        let consumed = stream.byte_offset();
        if !parsed.is_object() {
            continue;
        }

        let prefix = text[..marker_idx].trim_end().to_string();
        let suffix = remaining[brace_idx + consumed..].trim().to_string();
        let args = serde_json::to_string(&parsed).ok()?;
        if suffix.is_empty() {
            return Some((prefix, tool_name.clone(), args, suffix));
        }
        if fallback.is_none() {
            fallback = Some((prefix, tool_name.clone(), args, suffix));
        }
    }

    fallback
}

fn stream_text_or_recovered_tool_call(
    text: &str,
    pending: &mut VecDeque<StreamEvent>,
) -> Option<StreamEvent> {
    if text.is_empty() {
        return None;
    }

    if let Some((prefix, tool_name, arguments, suffix)) = parse_text_wrapped_tool_call(text) {
        let total = RECOVERED_TEXT_WRAPPED_TOOL_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
        crate::logging::warn(&format!(
            "[openai] Recovered text-wrapped tool call for '{}' (total={})",
            tool_name, total
        ));
        let suffix = sanitize_recovered_tool_suffix(&suffix);
        if !prefix.is_empty() {
            pending.push_back(StreamEvent::TextDelta(prefix));
        }
        pending.push_back(StreamEvent::ToolUseStart {
            id: format!(
                "fallback_text_call_{}",
                FALLBACK_TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            name: tool_name,
        });
        pending.push_back(StreamEvent::ToolInputDelta(arguments));
        pending.push_back(StreamEvent::ToolUseEnd);
        if !suffix.is_empty() {
            pending.push_back(StreamEvent::TextDelta(suffix));
        }
        return pending.pop_front();
    }

    Some(StreamEvent::TextDelta(text.to_string()))
}

fn sanitize_recovered_tool_suffix(suffix: &str) -> String {
    let trimmed = suffix.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let normalized = trimmed.trim_start_matches('"');

    if normalized.starts_with(",\"item_id\"")
        || normalized.starts_with(",\"output_index\"")
        || normalized.starts_with(",\"sequence_number\"")
        || normalized.starts_with(",\"call_id\"")
        || normalized.starts_with(",\"type\":\"response.")
        || (normalized.starts_with(',')
            && normalized.contains("\"item_id\"")
            && (normalized.contains("\"output_index\"")
                || normalized.contains("\"sequence_number\"")))
    {
        return String::new();
    }

    suffix.to_string()
}

#[derive(Deserialize, Debug)]
struct ResponseSseEvent {
    #[serde(rename = "type")]
    kind: String,
    item: Option<Value>,
    delta: Option<String>,
    item_id: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
    response: Option<Value>,
    error: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct StreamingToolCallState {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

fn normalize_openai_tool_arguments(raw_arguments: String) -> String {
    if raw_arguments.trim() == "null" {
        let total = NORMALIZED_NULL_TOOL_ARGUMENTS.fetch_add(1, Ordering::Relaxed) + 1;
        crate::logging::warn(&format!(
            "[openai] Normalized null tool arguments to empty object (total={})",
            total
        ));
        "{}".to_string()
    } else {
        raw_arguments
    }
}

fn streaming_tool_item_id(item: &Value) -> Option<String> {
    item.get("id")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("item_id").and_then(|v| v.as_str()))
        .map(|id| id.to_string())
}

fn stream_tool_call_from_state(
    item_id: Option<String>,
    mut state: StreamingToolCallState,
    pending: &mut VecDeque<StreamEvent>,
) -> Option<StreamEvent> {
    let tool_name = state.name.take().filter(|name| !name.is_empty())?;
    let raw_call_id = state
        .call_id
        .take()
        .filter(|id| !id.is_empty())
        .or(item_id)
        .unwrap_or_else(|| {
            format!(
                "fallback_text_call_{}",
                FALLBACK_TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed)
            )
        });
    let call_id = crate::message::sanitize_tool_id(&raw_call_id);
    let arguments = normalize_openai_tool_arguments(if state.arguments.is_empty() {
        "{}".to_string()
    } else {
        state.arguments
    });

    pending.push_back(StreamEvent::ToolUseStart {
        id: call_id,
        name: tool_name,
    });
    pending.push_back(StreamEvent::ToolInputDelta(arguments));
    pending.push_back(StreamEvent::ToolUseEnd);
    pending.pop_front()
}

pub(super) fn parse_openai_response_event(
    data: &str,
    saw_text_delta: &mut bool,
    streaming_tool_calls: &mut HashMap<String, StreamingToolCallState>,
    completed_tool_items: &mut HashSet<String>,
    pending: &mut VecDeque<StreamEvent>,
) -> Option<StreamEvent> {
    if data == "[DONE]" {
        return Some(StreamEvent::MessageEnd { stop_reason: None });
    }

    if is_websocket_fallback_notice(data) {
        crate::logging::warn(&format!("OpenAI stream transport notice: {}", data.trim()));
        return None;
    }

    if data
        .to_lowercase()
        .contains("stream disconnected before completion")
    {
        return Some(StreamEvent::Error {
            message: data.to_string(),
            retry_after_secs: None,
        });
    }

    let event: ResponseSseEvent = match serde_json::from_str(data) {
        Ok(parsed) => parsed,
        Err(error) => {
            crate::logging::warn(&format!(
                "OpenAI SSE JSON parse failed: {} payload={}",
                error,
                truncated_stream_payload_context(data)
            ));
            return None;
        }
    };

    match event.kind.as_str() {
        "response.output_text.delta" => {
            if let Some(delta) = event.delta {
                *saw_text_delta = true;
                return stream_text_or_recovered_tool_call(&delta, pending);
            }
        }
        "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
            if let Some(delta) = event.delta {
                return Some(StreamEvent::ThinkingDelta(delta));
            }
        }
        "response.reasoning.done" | "response.output_item.added" => {
            if let Some(item) = &event.item {
                if item.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                    return Some(StreamEvent::ThinkingStart);
                }
                if matches!(
                    item.get("type").and_then(|v| v.as_str()),
                    Some("function_call") | Some("custom_tool_call")
                ) && let Some(item_id) = streaming_tool_item_id(item)
                {
                    let state = streaming_tool_calls.entry(item_id).or_default();
                    state.call_id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| state.call_id.clone());
                    state.name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| state.name.clone());
                    if let Some(arguments) = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .or_else(|| item.get("input").and_then(|v| v.as_str()))
                    {
                        state.arguments = arguments.to_string();
                    } else if let Some(input) = item.get("input")
                        && (input.is_object() || input.is_array())
                    {
                        state.arguments = input.to_string();
                    }
                }
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(item_id) = event.item_id {
                let state = streaming_tool_calls.entry(item_id).or_default();
                if let Some(call_id) = event.call_id {
                    state.call_id = Some(call_id);
                }
                if let Some(name) = event.name {
                    state.name = Some(name);
                }
                if let Some(delta) = event.delta {
                    state.arguments.push_str(&delta);
                }
            }
        }
        "response.function_call_arguments.done" => {
            if let Some(item_id) = event.item_id {
                let mut state = streaming_tool_calls.remove(&item_id).unwrap_or_default();
                if let Some(call_id) = event.call_id {
                    state.call_id = Some(call_id);
                }
                if let Some(name) = event.name {
                    state.name = Some(name);
                }
                if let Some(arguments) = event.arguments {
                    state.arguments = arguments;
                }
                if let Some(tool_event) =
                    stream_tool_call_from_state(Some(item_id.clone()), state.clone(), pending)
                {
                    completed_tool_items.insert(item_id);
                    return Some(tool_event);
                }
                streaming_tool_calls.insert(item_id, state);
            }
        }
        "response.output_item.done" => {
            if let Some(item) = event.item {
                if let Some(item_id) = streaming_tool_item_id(&item)
                    && completed_tool_items.contains(&item_id)
                    && matches!(
                        item.get("type").and_then(|v| v.as_str()),
                        Some("function_call") | Some("custom_tool_call")
                    )
                {
                    completed_tool_items.remove(&item_id);
                    return None;
                }
                if let Some(event) = handle_openai_output_item(item, saw_text_delta, pending) {
                    return Some(event);
                }
            }
        }
        "response.incomplete" => {
            let stop_reason = event
                .response
                .as_ref()
                .and_then(extract_stop_reason_from_response)
                .or_else(|| Some("incomplete".to_string()));
            if let Some(response) = event.response
                && let Some(usage_event) = extract_usage_from_response(&response)
            {
                pending.push_back(usage_event);
            }
            pending.push_back(StreamEvent::MessageEnd { stop_reason });
            return pending.pop_front();
        }
        "response.completed" => {
            let stop_reason = event
                .response
                .as_ref()
                .and_then(extract_stop_reason_from_response);
            if let Some(response) = event.response
                && let Some(usage_event) = extract_usage_from_response(&response)
            {
                pending.push_back(usage_event);
            }
            pending.push_back(StreamEvent::MessageEnd { stop_reason });
            return pending.pop_front();
        }
        "response.failed" | "response.error" | "error" => {
            crate::logging::warn(&format!(
                "OpenAI stream error event (type={}): response={:?}, error={:?}",
                event.kind, event.response, event.error
            ));
            let (message, retry_after_secs) =
                extract_error_with_retry(&event.response, &event.error);
            return Some(StreamEvent::Error {
                message,
                retry_after_secs,
            });
        }
        _ => {}
    }

    None
}

fn extract_last_assistant_message_phase(response: &Value) -> Option<String> {
    let output = response.get("output")?.as_array()?;
    output.iter().rev().find_map(|item| {
        if item.get("type").and_then(|v| v.as_str()) != Some("message") {
            return None;
        }
        if item.get("role").and_then(|v| v.as_str()) != Some("assistant") {
            return None;
        }
        item.get("phase")
            .and_then(|v| v.as_str())
            .map(|phase| phase.to_string())
    })
}

fn extract_stop_reason_from_response(response: &Value) -> Option<String> {
    let status = response.get("status").and_then(|v| v.as_str());
    if status == Some("completed") {
        if extract_last_assistant_message_phase(response).as_deref() == Some("commentary") {
            return Some("commentary".to_string());
        }
        return None;
    }

    let incomplete_reason = response
        .get("incomplete_details")
        .and_then(|v| v.get("reason"))
        .and_then(|v| v.as_str());

    if let Some(reason) = incomplete_reason {
        return Some(reason.to_string());
    }

    status
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

pub(super) fn handle_openai_output_item(
    item: Value,
    saw_text_delta: &mut bool,
    pending: &mut VecDeque<StreamEvent>,
) -> Option<StreamEvent> {
    let item_type = item.get("type")?.as_str()?;
    match item_type {
        "compaction" => {
            let encrypted_content = item
                .get("encrypted_content")
                .and_then(|v| v.as_str())
                .map(|value| value.to_string())?;
            return Some(StreamEvent::Compaction {
                trigger: "openai_native_auto".to_string(),
                pre_tokens: None,
                openai_encrypted_content: Some(encrypted_content),
            });
        }
        "function_call" | "custom_tool_call" => {
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let raw_arguments = item
                .get("arguments")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .or_else(|| {
                    item.get("input").and_then(|v| {
                        if v.is_object() || v.is_array() {
                            Some(v.to_string())
                        } else {
                            v.as_str().map(|s| s.to_string())
                        }
                    })
                })
                .unwrap_or_else(|| "{}".to_string());
            let arguments = normalize_openai_tool_arguments(raw_arguments);

            pending.push_back(StreamEvent::ToolUseStart {
                id: call_id.clone(),
                name,
            });
            pending.push_back(StreamEvent::ToolInputDelta(arguments));
            pending.push_back(StreamEvent::ToolUseEnd);
            return pending.pop_front();
        }
        "message" => {
            if *saw_text_delta {
                return None;
            }
            let mut text = String::new();
            if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                for entry in content {
                    let entry_type = entry.get("type").and_then(|v| v.as_str());
                    if matches!(entry_type, Some("output_text") | Some("text"))
                        && let Some(t) = entry.get("text").and_then(|v| v.as_str())
                    {
                        text.push_str(t);
                    }
                }
            }
            return stream_text_or_recovered_tool_call(&text, pending);
        }
        "reasoning" => {
            if let Some(summary_arr) = item.get("summary").and_then(|v| v.as_array()) {
                let mut summary_text = String::new();
                for summary_item in summary_arr {
                    if summary_item.get("type").and_then(|v| v.as_str()) == Some("summary_text")
                        && let Some(text) = summary_item.get("text").and_then(|v| v.as_str())
                    {
                        if !summary_text.is_empty() {
                            summary_text.push('\n');
                        }
                        summary_text.push_str(text);
                    }
                }
                if !summary_text.is_empty() {
                    pending.push_back(StreamEvent::ThinkingStart);
                    pending.push_back(StreamEvent::ThinkingDelta(summary_text));
                    pending.push_back(StreamEvent::ThinkingEnd);
                    return pending.pop_front();
                }
            }
        }
        _ => {}
    }

    None
}

pub(super) struct OpenAIResponsesStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    pending: VecDeque<StreamEvent>,
    saw_text_delta: bool,
    streaming_tool_calls: HashMap<String, StreamingToolCallState>,
    completed_tool_items: HashSet<String>,
}

impl OpenAIResponsesStream {
    pub(super) fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: VecDeque::new(),
            saw_text_delta: false,
            streaming_tool_calls: HashMap::new(),
            completed_tool_items: HashSet::new(),
        }
    }

    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }

        while let Some(pos) = self.buffer.find("\n\n") {
            let event_str = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            let mut data_lines = Vec::new();
            for line in event_str.lines() {
                if let Some(data) = crate::util::sse_data_line(line) {
                    data_lines.push(data);
                }
            }

            if data_lines.is_empty() {
                continue;
            }

            let data = data_lines.join("\n");
            if let Some(event) = parse_openai_response_event(
                &data,
                &mut self.saw_text_delta,
                &mut self.streaming_tool_calls,
                &mut self.completed_tool_items,
                &mut self.pending,
            ) {
                return Some(event);
            }
        }

        None
    }
}

fn extract_cached_input_tokens(usage: &Value) -> Option<u64> {
    usage
        .get("input_tokens_details")
        .or_else(|| usage.get("prompt_tokens_details"))
        .and_then(|details| details.get("cached_tokens"))
        .and_then(|v| v.as_u64())
}

fn extract_usage_from_response(response: &Value) -> Option<StreamEvent> {
    let usage = response.get("usage")?;
    let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64());
    let output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64());
    let cache_read_input_tokens = extract_cached_input_tokens(usage);
    if input_tokens.is_some() || output_tokens.is_some() || cache_read_input_tokens.is_some() {
        Some(StreamEvent::TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens: None,
        })
    } else {
        None
    }
}

impl Stream for OpenAIResponsesStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.parse_next_event() {
                return Poll::Ready(Some(Ok(event)));
            }

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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_wrapped_tool_call_rejects_non_object_json() {
        let text = "prefix to=functions.read [1,2,3]";
        let parsed = parse_text_wrapped_tool_call(text);
        assert!(parsed.is_none());
    }

    #[test]
    fn parse_openai_response_event_ignores_malformed_json_chunks() {
        let mut saw_text_delta = false;
        let mut streaming_tool_calls = HashMap::new();
        let mut completed_tool_items = HashSet::new();
        let mut pending = VecDeque::new();

        let event = parse_openai_response_event(
            "{not-json}",
            &mut saw_text_delta,
            &mut streaming_tool_calls,
            &mut completed_tool_items,
            &mut pending,
        );

        assert!(event.is_none());
        assert!(!saw_text_delta);
        assert!(streaming_tool_calls.is_empty());
        assert!(completed_tool_items.is_empty());
        assert!(pending.is_empty());
    }
}
