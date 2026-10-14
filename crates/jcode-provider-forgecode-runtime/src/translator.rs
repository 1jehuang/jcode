use jcode_message_types::StreamEvent;
use serde_json::Value;

use crate::parser::{CliOutput, ContentBlockInfo, DeltaInfo, SdkContentBlock, SseEvent};
use crate::to_internal_tool_name;

// ---------------------------------------------------------------------------
// Event translator
// ---------------------------------------------------------------------------

pub(crate) struct ForgeCodeEventTranslator {
    last_stop_reason: Option<String>,
    in_thinking_block: bool,
    in_tool_use_block: bool,
}

impl ForgeCodeEventTranslator {
    pub(crate) fn new() -> Self {
        Self {
            last_stop_reason: None,
            in_thinking_block: false,
            in_tool_use_block: false,
        }
    }

    pub(crate) fn handle_event(&mut self, event: SseEvent) -> Vec<StreamEvent> {
        match event {
            SseEvent::MessageStart { message } => {
                if let Some(usage) = message.get("usage") {
                    let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64());
                    let output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64());
                    let cache_creation_input_tokens = usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64());
                    let cache_read_input_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64());
                    if input_tokens.is_some()
                        || output_tokens.is_some()
                        || cache_creation_input_tokens.is_some()
                        || cache_read_input_tokens.is_some()
                    {
                        return vec![StreamEvent::TokenUsage {
                            input_tokens,
                            output_tokens,
                            cache_read_input_tokens,
                            cache_creation_input_tokens,
                        }];
                    }
                }
                Vec::new()
            }
            SseEvent::ContentBlockStart { content_block, .. } => match content_block {
                ContentBlockInfo::Text { .. } => Vec::new(),
                ContentBlockInfo::ToolUse { id, name } => {
                    self.in_tool_use_block = true;
                    vec![StreamEvent::ToolUseStart {
                        id,
                        name: to_internal_tool_name(&name),
                    }]
                }
                ContentBlockInfo::Thinking { .. } => {
                    self.in_thinking_block = true;
                    vec![StreamEvent::ThinkingStart]
                }
                ContentBlockInfo::Other => Vec::new(),
            },
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                DeltaInfo::TextDelta { text } => {
                    vec![StreamEvent::TextDelta(text)]
                }
                DeltaInfo::InputJsonDelta { partial_json } => {
                    vec![StreamEvent::ToolInputDelta(partial_json)]
                }
                DeltaInfo::ThinkingDelta { .. } => Vec::new(),
                DeltaInfo::SignatureDelta { .. } => Vec::new(),
                DeltaInfo::Other => Vec::new(),
            },
            SseEvent::ContentBlockStop { .. } => {
                if self.in_thinking_block {
                    self.in_thinking_block = false;
                    vec![StreamEvent::ThinkingEnd]
                } else if self.in_tool_use_block {
                    self.in_tool_use_block = false;
                    vec![StreamEvent::ToolUseEnd]
                } else {
                    Vec::new()
                }
            }
            SseEvent::MessageDelta { delta, usage } => {
                self.last_stop_reason = delta.stop_reason.clone();
                if let Some(usage) = usage
                    && (usage.input_tokens.is_some()
                        || usage.output_tokens.is_some()
                        || usage.cache_creation_input_tokens.is_some()
                        || usage.cache_read_input_tokens.is_some())
                {
                    return vec![StreamEvent::TokenUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                        cache_creation_input_tokens: usage.cache_creation_input_tokens,
                    }];
                }
                Vec::new()
            }
            SseEvent::MessageStop => vec![StreamEvent::MessageEnd {
                stop_reason: self.last_stop_reason.take(),
            }],
            SseEvent::Error { error } => vec![StreamEvent::Error {
                message: error.message,
                retry_after_secs: error.retry_after_secs,
            }],
            _ => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// CLI output parser
// ---------------------------------------------------------------------------

pub(crate) struct CliOutputParser {
    translator: ForgeCodeEventTranslator,
    saw_stream_events: bool,
    saw_message_end: bool,
}

impl CliOutputParser {
    pub(crate) fn new() -> Self {
        Self {
            translator: ForgeCodeEventTranslator::new(),
            saw_stream_events: false,
            saw_message_end: false,
        }
    }

    pub(crate) fn handle_output(&mut self, output: CliOutput) -> Vec<StreamEvent> {
        match output {
            CliOutput::StreamEvent { event, .. } => {
                self.saw_stream_events = true;
                let parsed: SseEvent = match serde_json::from_value(event) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        return vec![StreamEvent::Error {
                            message: format!("Failed to parse ForgeCode CLI stream event: {}", err),
                            retry_after_secs: None,
                        }];
                    }
                };

                let events = self.translator.handle_event(parsed);
                if events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
                {
                    self.saw_message_end = true;
                }
                events
            }
            CliOutput::Assistant { message, .. } => {
                let blocks = parse_content_blocks(&message.content);
                let mut events = Vec::new();
                for block in blocks {
                    match block {
                        SdkContentBlock::Text { text } => {
                            if !self.saw_stream_events {
                                events.push(StreamEvent::TextDelta(text));
                            }
                        }
                        SdkContentBlock::ToolUse { id, name, input } => {
                            if !self.saw_stream_events {
                                events.push(StreamEvent::ToolUseStart {
                                    id,
                                    name: to_internal_tool_name(&name),
                                });
                                events.push(StreamEvent::ToolInputDelta(
                                    serde_json::to_string(&input).unwrap_or_default(),
                                ));
                                events.push(StreamEvent::ToolUseEnd);
                            }
                        }
                        SdkContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            let content_str = content
                                .map(|v| {
                                    if let Some(s) = v.as_str() {
                                        s.to_string()
                                    } else {
                                        serde_json::to_string(&v).unwrap_or_default()
                                    }
                                })
                                .unwrap_or_default();
                            events.push(StreamEvent::ToolResult {
                                tool_use_id,
                                content: content_str,
                                is_error: is_error.unwrap_or(false),
                            });
                        }
                        _ => {}
                    }
                }

                if !self.saw_message_end {
                    self.saw_message_end = true;
                    events.push(StreamEvent::MessageEnd { stop_reason: None });
                }

                events
            }
            CliOutput::User { message, .. } => {
                let blocks = parse_content_blocks(&message.content);
                let mut events = Vec::new();
                for block in blocks {
                    if let SdkContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = block
                    {
                        let content_str = content
                            .map(|v| {
                                if let Some(s) = v.as_str() {
                                    s.to_string()
                                } else {
                                    serde_json::to_string(&v).unwrap_or_default()
                                }
                            })
                            .unwrap_or_default();
                        events.push(StreamEvent::ToolResult {
                            tool_use_id,
                            content: content_str,
                            is_error: is_error.unwrap_or(false),
                        });
                    }
                }
                events
            }
            CliOutput::Result {
                usage,
                is_error,
                session_id,
            } => {
                let mut events = Vec::new();
                if let Some(usage) = usage
                    && (usage.input_tokens.is_some()
                        || usage.output_tokens.is_some()
                        || usage.cache_creation_input_tokens.is_some()
                        || usage.cache_read_input_tokens.is_some())
                {
                    events.push(StreamEvent::TokenUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        cache_read_input_tokens: usage.cache_read_input_tokens,
                        cache_creation_input_tokens: usage.cache_creation_input_tokens,
                    });
                }
                if let Some(sid) = session_id {
                    events.push(StreamEvent::SessionId(sid));
                }
                if is_error {
                    events.push(StreamEvent::Error {
                        message: "ForgeCode CLI reported an error".to_string(),
                        retry_after_secs: None,
                    });
                }
                if !self.saw_message_end {
                    self.saw_message_end = true;
                    events.push(StreamEvent::MessageEnd { stop_reason: None });
                }
                events
            }
            CliOutput::Error {
                message,
                retry_after_secs,
            } => vec![StreamEvent::Error {
                message,
                retry_after_secs,
            }],
            CliOutput::System { session_id } => {
                session_id.map(StreamEvent::SessionId).into_iter().collect()
            }
            CliOutput::Other => Vec::new(),
        }
    }
}

fn parse_content_blocks(content: &Value) -> Vec<SdkContentBlock> {
    match content {
        Value::String(text) => {
            vec![SdkContentBlock::Text { text: text.clone() }]
        }
        Value::Array(items) => items
            .iter()
            .filter_map(|item| serde_json::from_value(item.clone()).ok())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::CliMessage;
    use serde_json::json;

    #[test]
    fn test_parse_content_blocks_string() {
        let blocks = parse_content_blocks(&json!("hello"));
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            SdkContentBlock::Text { text } => assert_eq!(text, "hello"),
            _ => panic!("Expected Text block"),
        }
    }

    #[test]
    fn test_parse_content_blocks_array() {
        // The `SdkContentBlock` enum has a `#[serde(other)] Other` variant
        // that catches any content type not explicitly modelled (added when
        // the parser was made permissive to ignore unknown CLI output types
        // like `thinking`, `redacted_thinking`, image blocks, etc.). The
        // previous assertion (len == 1) predated the `Other` variant and
        // was masked because the test crate failed to compile (E0422
        // `CliMessage` not in scope) — see the CliMessage import fix that
        // unblocks the test build.
        let blocks = parse_content_blocks(&json!([
            {"type": "text", "text": "hi"},
            {"type": "other_thing"}
        ]));
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            SdkContentBlock::Text { text } => assert_eq!(text, "hi"),
            _ => panic!("Expected Text block at index 0"),
        }
        match &blocks[1] {
            SdkContentBlock::Other => {}
            _ => panic!("Expected Other block at index 1"),
        }
    }

    #[test]
    fn test_cli_output_parser_assistant_no_stream() {
        let mut parser = CliOutputParser::new();
        let output = CliOutput::Assistant {
            message: CliMessage {
                content: json!([
                    {"type": "text", "text": "hello world"}
                ]),
            },
            _session_id: None,
        };
        let events = parser.handle_output(output);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::TextDelta(t) if t == "hello world"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::MessageEnd { .. }))
        );
    }
}
