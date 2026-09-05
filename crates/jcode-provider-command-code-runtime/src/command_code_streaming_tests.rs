use super::{
    ndjson::decode_line, project_context::project_context_cache, serializer::serialize_messages,
};
use futures::StreamExt;
use jcode_message_types::{ContentBlock, Message, Role};

#[test]
fn command_code_ndjson_maps_events_and_drops_junk() {
    assert!(matches!(decode_line("blank"), None));
    assert!(matches!(
        decode_line(r#"{"type":"text-delta","text":"hi"}"#),
        Some(jcode_message_types::StreamEvent::TextDelta(_))
    ));
    assert!(matches!(
        decode_line(r#"{"type":"finish","finishReason":"stop"}"#),
        Some(jcode_message_types::StreamEvent::MessageEnd { .. })
    ));
}

#[test]
fn command_code_workspace_context_is_bounded() {
    let context = project_context_cache(std::env::current_dir().unwrap());
    assert!(context.entries.len() <= 64);
    assert!(
        context
            .agents
            .as_ref()
            .map_or(true, |text| text.len() <= 32_768)
    );
}

#[test]
fn command_code_pairing_synthesizes_cancelled_result() {
    let message = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call-1".into(),
            name: "shell".into(),
            input: serde_json::json!({}),
            thought_signature: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    };
    let wire = serialize_messages(&[message]);
    assert!(
        wire.iter()
            .any(|value| value.to_string().contains("cancelled"))
    );
    let _ = ContentBlock::Text {
        text: String::new(),
        cache_control: None,
    };
}

#[test]
fn command_code_tool_results_use_tool_role_and_output_shape() {
    let assistant = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "a".into(),
            name: "shell".into(),
            input: serde_json::json!({"x":1}),
            thought_signature: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    };
    let result = Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "a".into(),
            content: "ok".into(),
            is_error: Some(false),
        }],
        timestamp: None,
        tool_duration_ms: None,
    };
    let wire = serialize_messages(&[assistant, result]);
    assert_eq!(wire[1]["role"], "tool");
    assert_eq!(
        wire[1]["content"][0]["output"],
        serde_json::json!({"type":"text","value":"ok"})
    );
}

#[test]
fn command_code_orphan_and_image_results_are_repaired() {
    let orphan = Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "missing".into(),
            content: "lost".into(),
            is_error: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    };
    let image = Message {
        role: Role::User,
        content: vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc".into(),
        }],
        timestamp: None,
        tool_duration_ms: None,
    };
    let wire = serialize_messages(&[orphan, image]);
    assert!(
        wire.iter()
            .any(|v| v["role"] == "user" && v.to_string().contains("without adjacent"))
    );
    assert!(
        wire.iter()
            .any(|v| v.to_string().contains("data:image/png;base64,abc"))
    );
}

#[tokio::test]
async fn command_code_finish_step_usage_precedes_one_terminal() {
    let input = futures::stream::iter(vec![
        Ok::<_, anyhow::Error>(
            r#"{"type":"finish-step","totalUsage":{"inputTokens":3,"outputTokens":2}}"#.into(),
        ),
        Ok(r#"{"type":"finish","finishReason":"stop"}"#.into()),
    ]);
    let mut stream = super::ndjson::decode_ndjson_stream(Box::pin(input)).unwrap();
    let mut usage = false;
    let mut terminals = 0;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            jcode_message_types::StreamEvent::TokenUsage {
                input_tokens: Some(3),
                output_tokens: Some(2),
                ..
            } => usage = true,
            jcode_message_types::StreamEvent::MessageEnd { .. } => {
                assert!(usage);
                terminals += 1;
            }
            _ => {}
        }
    }
    assert_eq!(terminals, 1);
}
