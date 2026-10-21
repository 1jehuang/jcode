//! Unit checks for retained-child checkpoints and their memory limits.

use super::*;
use jcode_message_types::CacheControl;
use serde_json::json;

fn native_text(text: &str) -> NativeHistory {
    let mut native = NativeHistory::default();
    native.observe(&StreamEvent::TextDelta(text.into()));
    native
}

fn message(role: Role, content: Vec<ContentBlock>) -> Message {
    Message {
        role,
        content,
        timestamp: None,
        tool_duration_ms: None,
    }
}

#[test]
fn native_text_is_independent_of_stream_chunking() {
    let mut native = native_text("native ");
    native.observe(&StreamEvent::TextDelta("reply".into()));
    let seen = SeenHistory::new(&[Message::user("first")], native);
    assert_eq!(
        seen.missing_start(&[
            Message::user("first"),
            Message::assistant_text("native reply"),
            Message::user("next"),
        ]),
        Ok(2)
    );
}

#[test]
fn intervening_turns_start_after_the_exact_native_response() {
    let seen = SeenHistory::new(&[Message::user("first")], native_text("native reply"));
    let messages = [
        Message::user("first"),
        Message::assistant_text("native reply"),
        Message::user("foreign instruction"),
        Message::assistant_text("DIFFERENT_PROVIDER_RESPONSE"),
        Message::user("return"),
    ];
    assert_eq!(seen.missing_start(&messages), Ok(2));
    assert_eq!(
        seen.missing_start(&[
            Message::user("first"),
            Message::assistant_text("DIFFERENT_PROVIDER_RESPONSE"),
            Message::user("return"),
        ]),
        Err(())
    );
}

#[test]
fn prefix_edits_and_truncation_are_divergence() {
    let request = [
        Message::user("old"),
        Message::assistant_text("reply"),
        Message::user("first"),
    ];
    let seen = SeenHistory::new(&request, native_text("native reply"));
    assert_eq!(seen.missing_start(&request[..2]), Err(()));
    let mut edited = request.to_vec();
    edited[0] = Message::user("edited");
    edited.push(Message::assistant_text("native reply"));
    edited.push(Message::user("next"));
    assert_eq!(seen.missing_start(&edited), Err(()));
}

#[test]
fn changing_image_content_is_prefix_divergence() {
    let mut image = Message::user("look");
    image.content.push(ContentBlock::Image {
        media_type: "image/png".into(),
        data: "AAAA".into(),
    });
    let seen = SeenHistory::new(&[image.clone()], native_text("native reply"));
    if let ContentBlock::Image { data, .. } = &mut image.content[1] {
        *data = "BBBB".into();
    }
    assert_eq!(
        seen.missing_start(&[
            image,
            Message::assistant_text("native reply"),
            Message::user("next"),
        ]),
        Err(())
    );
}

#[test]
fn cache_metadata_and_transient_reminders_do_not_restart_the_child() {
    let reminder = Message::user("<system-reminder>old clock</system-reminder>");
    let seen = SeenHistory::new(
        &[Message::user("first"), reminder],
        native_text("native reply"),
    );
    let mut first = Message::user("first");
    first.tool_duration_ms = Some(123);
    if let ContentBlock::Text { cache_control, .. } = &mut first.content[0] {
        *cache_control = Some(CacheControl::ephemeral(None));
    }
    assert_eq!(
        seen.missing_start(&[
            first,
            Message::assistant_text("native reply"),
            Message::user("next"),
            Message::user("<system-reminder>new clock</system-reminder>"),
        ]),
        Ok(2)
    );
    assert_eq!(
        seen.missing_start(&[
            Message::user("first"),
            Message::user("<system-reminder>tool continuation clock</system-reminder>"),
            Message::assistant_text("native reply"),
        ]),
        Ok(3)
    );
}

#[test]
fn consecutive_user_inputs_do_not_resend_the_previous_request() {
    let seen = SeenHistory::new(&[Message::user("first")], NativeHistory::default());
    let messages = [Message::user("first"), Message::user("second")];
    let start = seen.missing_start(&messages).unwrap();
    let input = crate::prompt::turn_input(&messages[start..]);
    assert_eq!(input.texts, vec!["second"]);
}

#[test]
fn native_tool_results_are_recognized_and_changed_results_diverge() {
    let mut native = native_text("done");
    for event in [
        StreamEvent::ToolUseStart {
            id: "tool-1".into(),
            name: "Read".into(),
        },
        StreamEvent::ToolInputDelta(json!({"file_path": "/x"}).to_string()),
        StreamEvent::ToolUseEnd,
        StreamEvent::ToolResult {
            tool_use_id: "tool-1".into(),
            content: "output".into(),
            is_error: false,
        },
    ] {
        native.observe(&event);
    }
    let seen = SeenHistory::new(&[Message::user("first")], native);
    let mut messages = vec![
        Message::user("first"),
        message(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "done".into(),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: "tool-1".into(),
                    name: "Read".into(),
                    input: json!({"file_path": "/x"}),
                    thought_signature: None,
                },
            ],
        ),
        message(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".into(),
                content: "output".into(),
                is_error: None,
            }],
        ),
    ];
    assert_eq!(seen.missing_start(&messages), Ok(3));
    if let ContentBlock::ToolResult { content, .. } = &mut messages[2].content[0] {
        *content = "different provider output".into();
    }
    assert_eq!(seen.missing_start(&messages), Err(()));
}

#[test]
fn native_checkpoint_memory_is_capped_and_overflow_forces_reseeding() {
    let mut native = NativeHistory::default();
    for index in 0..=MAX_NATIVE_TOOLS {
        native.observe(&StreamEvent::ToolResult {
            tool_use_id: format!("tool-{index}"),
            content: "output".into(),
            is_error: false,
        });
    }
    assert_eq!(native.results.len(), MAX_NATIVE_TOOLS);
    assert!(native.overflow);
    assert!(!native.recognizes(&[]));

    let mut native = NativeHistory::default();
    native.observe(&StreamEvent::ToolUseStart {
        id: "tool".into(),
        name: "Read".into(),
    });
    native.observe(&StreamEvent::ToolInputDelta(
        "x".repeat(MAX_TOOL_INPUT_BYTES + 1),
    ));
    assert!(native.pending_tool.is_none());
    assert!(native.overflow);

    let mut native = NativeHistory::default();
    native.observe(&StreamEvent::TextDelta("x".repeat(2 * 1024 * 1024)));
    assert_eq!(native.text.bytes, 2 * 1024 * 1024);
    assert!(native.tools.is_empty() && native.results.is_empty());
    assert!(std::mem::size_of_val(&native) < 512);
}
