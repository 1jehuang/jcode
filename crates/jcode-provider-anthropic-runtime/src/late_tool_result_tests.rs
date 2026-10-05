use super::*;

/// Reported 400: "`tool_use` ids were found without `tool_result` blocks
/// immediately after". The calls do have results, but a message was written
/// between the call and its results (a user interjection or a reload
/// continuation), so the results are not in the next message. Every
/// tool_use must still be answered in the very next user message.
#[tokio::test]
async fn test_tool_use_answered_later_still_gets_result_immediately_after() {
    let provider = AnthropicProvider::new();
    let msg = |role, content| Message {
        role,
        content,
        timestamp: None,
        tool_duration_ms: None,
    };
    let tool_use = |id: &str| ContentBlock::ToolUse {
        id: id.to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({}),
        thought_signature: None,
    };
    let tool_result = |id: &str| ContentBlock::ToolResult {
        tool_use_id: id.to_string(),
        content: format!("output of {id}"),
        is_error: None,
    };
    let text = |t: &str| ContentBlock::Text {
        text: t.to_string(),
        cache_control: None,
    };
    let messages = vec![
        msg(Role::User, vec![text("go")]),
        // The real session: two assistant messages back to back, the second
        // with its own calls, answered right away; the first message's calls
        // were answered only after more turns were written.
        msg(
            Role::Assistant,
            vec![text("Checking."), tool_use("tool_a"), tool_use("tool_b")],
        ),
        msg(Role::Assistant, vec![tool_use("tool_c")]),
        msg(Role::User, vec![tool_result("tool_c")]),
        msg(Role::Assistant, vec![text("Working on it.")]),
        msg(Role::User, vec![text("also check the logs")]),
        msg(Role::User, vec![tool_result("tool_b")]),
        msg(Role::User, vec![tool_result("tool_a")]),
        msg(Role::Assistant, vec![text("Done.")]),
    ];

    let formatted = provider.format_messages(&messages, false, &[]);
    for (i, m) in formatted.iter().enumerate() {
        let uses: Vec<&String> = m
            .content
            .iter()
            .filter_map(|b| match b {
                ApiContentBlock::ToolUse { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        if uses.is_empty() {
            continue;
        }
        let next = formatted
            .get(i + 1)
            .expect("a message follows every tool_use");
        assert_eq!(next.role, "user");
        for id in uses {
            assert!(
                next.content.iter().any(|b| matches!(
                    b,
                    ApiContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id
                )),
                "tool_use {id} has no tool_result immediately after: {}",
                serde_json::to_string_pretty(&formatted).unwrap()
            );
        }
    }
    // The real output is not lost.
    let all_text = serde_json::to_string(&formatted).unwrap();
    assert!(
        ["tool_a", "tool_b", "tool_c"]
            .iter()
            .all(|id| all_text.contains(&format!("output of {id}")))
    );
    // Roles still alternate.
    assert!(formatted.windows(2).all(|w| w[0].role != w[1].role));
}

/// A late tool_result must carry the blocks that belong to it (its image, the
/// image label and any deferred tool reference) when it is moved up, or the
/// model sees a partial tool output and the reference is dropped.
fn late_result_conversation(attachments: Vec<ContentBlock>) -> Vec<Message> {
    let msg = |role, content| Message {
        role,
        content,
        timestamp: None,
        tool_duration_ms: None,
    };
    let text = |t: &str| ContentBlock::Text {
        text: t.to_string(),
        cache_control: None,
    };
    let mut late = vec![ContentBlock::ToolResult {
        tool_use_id: "tool_a".to_string(),
        content: "output of tool_a".to_string(),
        is_error: None,
    }];
    late.extend(attachments);
    late.push(text("unrelated note"));
    vec![
        msg(Role::User, vec![text("go")]),
        msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "tool_a".to_string(),
                name: "mcp_search".to_string(),
                input: serde_json::json!({}),
                thought_signature: None,
            }],
        ),
        msg(Role::User, vec![text("also check the logs")]),
        msg(Role::Assistant, vec![text("Working on it.")]),
        msg(Role::User, late),
        msg(Role::Assistant, vec![text("Done.")]),
    ]
}

#[tokio::test]
async fn test_late_tool_result_moves_with_its_image_and_label() {
    let provider = AnthropicProvider::new();
    let label = "[Attached image associated with the preceding tool result: shot.png]";
    let messages = late_result_conversation(vec![
        ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "aW1n".to_string(),
        },
        ContentBlock::Text {
            text: label.to_string(),
            cache_control: None,
        },
    ]);
    let formatted = provider.format_messages(&messages, false, &[]);
    let dump = serde_json::to_string_pretty(&formatted).unwrap();

    assert_eq!(formatted[1].role, "assistant");
    let answer = &formatted[2];
    assert_eq!(answer.role, "user");
    let ApiContentBlock::ToolResult {
        tool_use_id,
        content: ToolResultContent::Blocks(blocks),
        ..
    } = &answer.content[0]
    else {
        panic!("result with image blocks must follow the tool_use: {dump}");
    };
    assert_eq!(tool_use_id, "tool_a");
    assert!(
        matches!(&blocks[..], [
            ToolResultContentBlock::Text { text: out },
            ToolResultContentBlock::Image { .. },
            ToolResultContentBlock::Text { text: l },
        ] if out == "output of tool_a" && l == label),
        "image and label must sit right after the result: {dump}"
    );
    // Nothing of the tool output is left behind in the later message.
    let later = &formatted[4];
    assert_eq!(later.role, "user");
    assert!(
        matches!(&later.content[..], [ApiContentBlock::Text { text, .. }] if text == "unrelated note"),
        "later message must keep only its unrelated text: {dump}"
    );
    assert!(formatted.windows(2).all(|w| w[0].role != w[1].role));
}

#[tokio::test]
async fn test_late_tool_result_moves_with_its_tool_reference() {
    let provider = AnthropicProvider::new();
    let messages = late_result_conversation(vec![ContentBlock::ToolReference {
        tool_use_id: "tool_a".to_string(),
        tool_name: "mcp__weather__forecast".to_string(),
    }]);
    let tools: Vec<ApiTool> = ["mcp_search", "mcp__weather__forecast"]
        .iter()
        .map(|name| ApiTool {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
            defer_loading: *name != "mcp_search",
        })
        .collect();
    let formatted = provider.format_messages(&messages, false, &tools);
    let dump = serde_json::to_string_pretty(&formatted).unwrap();

    let answer = &formatted[2];
    assert_eq!(answer.role, "user");
    assert!(
        matches!(&answer.content[0], ApiContentBlock::ToolResult {
            tool_use_id,
            content: ToolResultContent::Blocks(blocks),
            ..
        } if tool_use_id == "tool_a" && matches!(&blocks[..], [
            ToolResultContentBlock::ToolReference { tool_name }
        ] if tool_name == "mcp__weather__forecast")),
        "the reference must load at the moved result: {dump}"
    );
    // The result text moves out beside it, ahead of the later user text.
    assert!(
        matches!(&answer.content[1], ApiContentBlock::Text { text, .. } if text == "output of tool_a"),
        "{dump}"
    );
    let later = &formatted[4];
    assert!(
        matches!(&later.content[..], [ApiContentBlock::Text { text, .. }] if text == "unrelated note"),
        "later message must keep only its unrelated text: {dump}"
    );
}
