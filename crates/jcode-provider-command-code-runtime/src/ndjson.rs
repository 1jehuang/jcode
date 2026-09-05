use anyhow::Result;
use futures::{Stream, StreamExt};
use jcode_message_types::StreamEvent;
use serde_json::Value;
use std::pin::Pin;

fn event(value: Value) -> Option<StreamEvent> {
    let obj = value.as_object()?;
    let kind = obj.get("type")?.as_str()?;
    match kind {
        "text-delta" => Some(StreamEvent::TextDelta(
            obj.get("text")?.as_str()?.to_owned(),
        )),
        "reasoning-delta" => Some(StreamEvent::ThinkingDelta(
            obj.get("text")
                .or_else(|| obj.get("reasoning"))?
                .as_str()?
                .to_owned(),
        )),
        "tool-call" => Some(StreamEvent::NativeToolCall {
            request_id: obj
                .get("toolCallId")
                .or_else(|| obj.get("id"))?
                .as_str()?
                .to_owned(),
            tool_name: obj
                .get("toolName")
                .or_else(|| obj.get("name"))?
                .as_str()?
                .to_owned(),
            input: obj
                .get("input")
                .or_else(|| obj.get("args"))
                .cloned()
                .unwrap_or(Value::Null),
        }),
        "finish-step" => Some(StreamEvent::MessageEnd {
            stop_reason: obj
                .get("rawFinishReason")
                .or_else(|| obj.get("finishReason"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "finish" => {
            let reason = obj
                .get("rawFinishReason")
                .or_else(|| obj.get("finishReason"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if reason.as_deref() == Some("error") {
                return Some(StreamEvent::Error {
                    message: "Command Code upstream ended the turn with finishReason \"error\""
                        .into(),
                    retry_after_secs: None,
                });
            }
            Some(StreamEvent::MessageEnd {
                stop_reason: reason,
            })
        }
        "usage" => {
            let usage = obj
                .get("totalUsage")
                .or_else(|| obj.get("usage"))
                .unwrap_or(&Value::Null);
            Some(StreamEvent::TokenUsage {
                input_tokens: usage.get("inputTokens").and_then(Value::as_u64),
                output_tokens: usage.get("outputTokens").and_then(Value::as_u64),
                cache_read_input_tokens: usage
                    .pointer("/inputTokenDetails/cacheReadTokens")
                    .and_then(Value::as_u64),
                cache_creation_input_tokens: usage
                    .pointer("/inputTokenDetails/cacheWriteTokens")
                    .and_then(Value::as_u64),
            })
        }
        "error" => {
            let error = obj
                .get("error")
                .and_then(|v| v.get("message").or_else(|| v.as_str().map(|_| v)))
                .or_else(|| obj.get("message"));
            Some(StreamEvent::Error {
                message: error
                    .and_then(Value::as_str)
                    .unwrap_or("Command Code stream error")
                    .to_owned(),
                retry_after_secs: None,
            })
        }
        _ => None,
    }
}

pub fn decode_line(line: &str) -> Option<StreamEvent> {
    let line = line
        .trim()
        .strip_prefix("data:")
        .map(str::trim)
        .unwrap_or(line.trim());
    if line.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(line).ok().and_then(event)
}

pub fn decode_ndjson_stream<S>(input: Pin<Box<S>>) -> Result<jcode_provider_core::EventStream>
where
    S: Stream<Item = Result<String>> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        let mut input = input;
        let mut done = false;
        while let Some(line) = input.next().await {
            match line {
                Ok(line) => {
                    for part in line.lines() {
                        if let Ok(raw) = serde_json::from_str::<Value>(
                            part.trim()
                                .strip_prefix("data:")
                                .map(str::trim)
                                .unwrap_or(part.trim()),
                        ) && raw.get("type").and_then(Value::as_str) == Some("finish-step")
                            && !done
                        {
                            if let Some(usage) = raw.get("totalUsage").or_else(|| raw.get("usage"))
                            {
                                let _ = tx
                                    .send(Ok(StreamEvent::TokenUsage {
                                        input_tokens: usage
                                            .get("inputTokens")
                                            .and_then(Value::as_u64),
                                        output_tokens: usage
                                            .get("outputTokens")
                                            .and_then(Value::as_u64),
                                        cache_read_input_tokens: usage
                                            .pointer("/inputTokenDetails/cacheReadTokens")
                                            .and_then(Value::as_u64),
                                        cache_creation_input_tokens: usage
                                            .pointer("/inputTokenDetails/cacheWriteTokens")
                                            .and_then(Value::as_u64),
                                    }))
                                    .await;
                            }
                        }
                        if let Some(value) = decode_line(part) {
                            if done {
                                continue;
                            }
                            if matches!(value, StreamEvent::MessageEnd { .. }) {
                                done = true;
                            }
                            if tx.send(Ok(value)).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error)).await;
                    return;
                }
            }
        }
        if !done {
            let _ = tx
                .send(Ok(StreamEvent::MessageEnd { stop_reason: None }))
                .await;
        }
    });
    Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
}
