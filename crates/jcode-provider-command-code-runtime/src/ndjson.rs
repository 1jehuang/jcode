use anyhow::Result;
use futures::{Stream, StreamExt};
use jcode_message_types::StreamEvent;
use serde_json::Value;
use std::pin::Pin;

fn event(value: Value) -> Option<StreamEvent> {
    let obj = value.as_object()?;
    let kind = obj.get("type")?.as_str()?;
    match kind {
        "text-delta" => Some(StreamEvent::TextDelta(obj.get("text")?.as_str()?.to_owned())),
        "reasoning-delta" => Some(StreamEvent::ThinkingDelta(obj.get("text").or_else(|| obj.get("reasoning"))?.as_str()?.to_owned())),
        "tool-call" => Some(StreamEvent::NativeToolCall { request_id: obj.get("toolCallId").or_else(|| obj.get("id"))?.as_str()?.to_owned(), tool_name: obj.get("toolName").or_else(|| obj.get("name"))?.as_str()?.to_owned(), input: obj.get("input").cloned().unwrap_or(Value::Null) }),
        "finish-step" => Some(StreamEvent::TokenUsage { input_tokens: obj.get("inputTokens").and_then(Value::as_u64), output_tokens: obj.get("outputTokens").and_then(Value::as_u64), cache_read_input_tokens: obj.get("cacheRead").and_then(Value::as_u64), cache_creation_input_tokens: obj.get("cacheWrite").and_then(Value::as_u64) }),
        "finish" => Some(StreamEvent::MessageEnd { stop_reason: obj.get("finishReason").and_then(Value::as_str).map(str::to_owned) }),
        "error" => Some(StreamEvent::Error { message: obj.get("message").or_else(|| obj.get("error")).map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).unwrap_or_else(|| "Command Code stream error".into()), retry_after_secs: None }),
        _ => None,
    }
}

pub fn decode_line(line: &str) -> Option<StreamEvent> {
    let line = line.trim().strip_prefix("data:").map(str::trim).unwrap_or(line.trim());
    if line.is_empty() { return None; }
    serde_json::from_str::<Value>(line).ok().and_then(event)
}

pub fn decode_ndjson_stream<S>(input: Pin<Box<S>>) -> Result<jcode_provider_core::EventStream>
where S: Stream<Item = Result<String>> + Send + 'static {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        let mut input = input;
        let mut done = false;
        while let Some(line) = input.next().await {
            match line {
                Ok(line) => { for part in line.lines() { if let Some(value) = decode_line(part) { if matches!(value, StreamEvent::MessageEnd { .. }) { done = true; } if tx.send(Ok(value)).await.is_err() { return; } } } }
                Err(error) => { let _ = tx.send(Err(error)).await; return; }
            }
        }
        if !done { let _ = tx.send(Ok(StreamEvent::MessageEnd { stop_reason: None })).await; }
    });
    Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
}
