//! Regression coverage for the 400 "unexpected `tool_use_id` found in
//! `tool_result` blocks" rejection.
//!
//! Production shape (session_clover_1785560899476): a long-running `bash` tool
//! was still executing when a scheduled-task wakeup drove a new turn. The
//! missing tool-output repair saw an assistant `tool_use` with no result yet
//! and inserted a synthetic placeholder result; the real tool output landed
//! ~28s later and was appended as a second `tool_result` for the same id.
//! After same-role merging, the duplicate sits in a user message whose
//! preceding assistant turn no longer contains that `tool_use`, so Anthropic
//! rejects every subsequent request and the session is permanently wedged.

use super::*;
use jcode_message_types::{ContentBlock, Message, Role};

fn text_msg(role: Role, text: &str) -> Message {
    Message {
        role,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn tool_use(id: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "ls"}),
            thought_signature: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn tool_result(id: &str, content: &str, is_error: Option<bool>) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

/// Every tool_use_id may appear at most once across all tool_result blocks.
fn assert_unique_tool_results(messages: &[ApiMessage]) {
    let mut seen = std::collections::HashSet::new();
    for msg in messages {
        for block in &msg.content {
            if let ApiContentBlock::ToolResult { tool_use_id, .. } = block {
                assert!(
                    seen.insert(tool_use_id.clone()),
                    "duplicate tool_result for {tool_use_id}"
                );
            }
        }
    }
}

#[test]
fn placeholder_then_real_output_keeps_only_the_real_output() {
    let messages = vec![
        text_msg(Role::User, "Q"),
        tool_use("toolu_1"),
        tool_result("toolu_1", TOOL_OUTPUT_MISSING_TEXT, Some(true)),
        text_msg(Role::User, "[Scheduled task] wakeup"),
        tool_result("toolu_1", "real output", None),
    ];

    let formatted = format_messages(&messages, false);
    assert_unique_tool_results(&formatted);

    let kept: Vec<&str> = formatted
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ApiContentBlock::ToolResult {
                content: ToolResultContent::Text(text),
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(kept, vec!["real output"], "real output must win");
}

#[test]
fn real_output_then_placeholder_keeps_the_real_output() {
    let messages = vec![
        text_msg(Role::User, "Q"),
        tool_use("toolu_1"),
        tool_result("toolu_1", "real output", None),
        tool_result("toolu_1", TOOL_OUTPUT_MISSING_TEXT, Some(true)),
    ];

    let formatted = format_messages(&messages, false);
    assert_unique_tool_results(&formatted);
    assert!(
        formatted.iter().flat_map(|m| &m.content).any(|b| matches!(
            b,
            ApiContentBlock::ToolResult { content: ToolResultContent::Text(t), .. } if t == "real output"
        ))
    );
}

#[test]
fn duplicate_real_outputs_keep_the_first() {
    let messages = vec![
        text_msg(Role::User, "Q"),
        tool_use("toolu_1"),
        tool_result("toolu_1", "first", None),
        tool_result("toolu_1", "second", None),
    ];

    let formatted = format_messages(&messages, false);
    assert_unique_tool_results(&formatted);
    assert!(formatted.iter().flat_map(|m| &m.content).any(|b| matches!(
        b,
        ApiContentBlock::ToolResult { content: ToolResultContent::Text(t), .. } if t == "first"
    )));
}

#[test]
fn distinct_tool_ids_are_untouched() {
    let messages = vec![
        text_msg(Role::User, "Q"),
        tool_use("toolu_1"),
        tool_result("toolu_1", "a", None),
        tool_use("toolu_2"),
        tool_result("toolu_2", "b", None),
    ];

    let formatted = format_messages(&messages, false);
    assert_unique_tool_results(&formatted);
    let count = formatted
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ApiContentBlock::ToolResult { .. }))
        .count();
    assert_eq!(count, 2);
}

#[test]
fn message_left_empty_by_dedupe_is_dropped_and_roles_stay_valid() {
    // The duplicate is the sole block of its message; dropping it must not
    // leave an empty message in the request.
    let messages = vec![
        text_msg(Role::User, "Q"),
        tool_use("toolu_1"),
        tool_result("toolu_1", "real", None),
        tool_result("toolu_1", TOOL_OUTPUT_MISSING_TEXT, Some(true)),
        text_msg(Role::Assistant, "done"),
        text_msg(Role::User, "next"),
    ];

    let formatted = format_messages(&messages, false);
    assert_unique_tool_results(&formatted);
    assert!(formatted.iter().all(|m| !m.content.is_empty()));
    let roles: Vec<&str> = formatted.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "user", "assistant", "user"]
    );
}

#[test]
fn synthetic_interrupt_placeholder_text_is_also_treated_as_a_placeholder() {
    let messages = vec![
        text_msg(Role::User, "Q"),
        tool_use("toolu_1"),
        tool_result(
            "toolu_1",
            "[Session interrupted before tool execution completed]",
            Some(true),
        ),
        tool_result("toolu_1", "real output", None),
    ];

    let formatted = format_messages(&messages, false);
    assert_unique_tool_results(&formatted);
    assert!(
        formatted.iter().flat_map(|m| &m.content).any(|b| matches!(
            b,
            ApiContentBlock::ToolResult { content: ToolResultContent::Text(t), .. } if t == "real output"
        ))
    );
}
