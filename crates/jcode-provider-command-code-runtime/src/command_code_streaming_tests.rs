use super::{ndjson::decode_line, project_context::project_context_cache, serializer::serialize_messages};
use jcode_message_types::{ContentBlock, Message, Role};

#[test]
fn command_code_ndjson_maps_events_and_drops_junk() {
    assert!(matches!(decode_line("blank"), None));
    assert!(matches!(decode_line(r#"{"type":"text-delta","text":"hi"}"#), Some(jcode_message_types::StreamEvent::TextDelta(_))));
    assert!(matches!(decode_line(r#"{"type":"finish","finishReason":"stop"}"#), Some(jcode_message_types::StreamEvent::MessageEnd { .. })));
}

#[test]
fn command_code_workspace_context_is_bounded() {
    let context = project_context_cache(std::env::current_dir().unwrap());
    assert!(context.entries.len() <= 64);
    assert!(context.agents.as_ref().map_or(true, |text| text.len() <= 32_768));
}

#[test]
fn command_code_pairing_synthesizes_cancelled_result() {
    let message = Message { role: Role::Assistant, content: vec![ContentBlock::ToolUse { id: "call-1".into(), name: "shell".into(), input: serde_json::json!({}), thought_signature: None }], timestamp: None, tool_duration_ms: None };
    let wire = serialize_messages(&[message]);
    assert!(wire.iter().any(|value| value.to_string().contains("cancelled")));
    let _ = ContentBlock::Text { text: String::new(), cache_control: None };
}
