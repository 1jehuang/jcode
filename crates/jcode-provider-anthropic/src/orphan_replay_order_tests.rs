use super::*;

fn message(role: Role, content: Vec<ContentBlock>) -> Message {
    Message {
        role,
        content: content.into(),
        timestamp: None,
        tool_duration_ms: None,
    }
}
fn text(value: &str) -> ContentBlock {
    ContentBlock::Text {
        text: value.into(),
        cache_control: None,
    }
}

#[test]
fn recovered_orphan_keeps_surrounding_text_image_and_label_order() {
    for paired in [false, true] {
        for oauth in [false, true] {
            let mut messages = Vec::new();
            let mut content = Vec::new();
            if paired {
                messages.push(message(
                    Role::Assistant,
                    vec![ContentBlock::ToolUse {
                        id: "paired".into(),
                        name: "bash".into(),
                        input: json!({}),
                        thought_signature: None,
                    }],
                ));
                content.push(ContentBlock::ToolResult {
                    tool_use_id: "paired".into(),
                    content: "paired-output".into(),
                    is_error: None,
                });
            }
            content.extend([
                text("Earlier user context"),
                ContentBlock::ToolResult {
                    tool_use_id: "ghost".into(),
                    content: "orphan-output".into(),
                    is_error: Some(true),
                },
                text("[Attached image 1]"),
                ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "fixture-image".into(),
                },
                text("Later user context"),
            ]);
            messages.push(message(Role::User, content));
            let formatted = format_messages(&messages, oauth);
            let user = formatted.last().unwrap();
            assert_eq!(user.role, "user");
            let blocks = &user.content[usize::from(paired)..];
            if paired {
                assert!(
                    matches!(&user.content[0], ApiContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "paired")
                );
            }
            assert!(
                matches!(&blocks[0], ApiContentBlock::Text { text, .. } if text == "Earlier user context")
            );
            assert!(
                matches!(&blocks[1], ApiContentBlock::Text { text, .. } if text.contains("Recovered orphaned tool error: ghost") && text.ends_with("orphan-output"))
            );
            assert!(
                matches!(&blocks[2], ApiContentBlock::Text { text, .. } if text == "[Attached image 1]")
            );
            assert!(matches!(&blocks[3], ApiContentBlock::Image { .. }));
            assert!(
                matches!(&blocks[4], ApiContentBlock::Text { text, .. } if text == "Later user context")
            );
            assert_eq!(blocks.len(), 5);
        }
    }
}
