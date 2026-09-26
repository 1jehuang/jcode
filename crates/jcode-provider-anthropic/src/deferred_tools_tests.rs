//! Provider-native deferred tool loading (`defer_loading` + `tool_reference`).
//!
//! Live-verified against the Messages API: deferred definitions stay out of
//! the cached prefix, so adding one mid-session keeps the cache, while adding
//! an eager tool invalidates it. These tests pin the request shape that
//! guarantee depends on.

use super::*;
use jcode_message_types::{ContentBlock, Message, Role, ToolDefinition};
use serde_json::json;

fn def(name: &str) -> ToolDefinition {
    ToolDefinition::new(
        name,
        format!("{name} description"),
        json!({"type":"object","properties":{}}),
    )
}

fn msg(role: Role, content: Vec<ContentBlock>) -> Message {
    Message {
        role,
        content,
        timestamp: None,
        tool_duration_ms: None,
    }
}

fn tools_json(tools: &[ApiTool]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| serde_json::to_value(t).unwrap())
        .collect()
}

#[test]
fn deferred_tools_follow_eager_and_breakpoint_stays_on_last_eager() {
    let tools = vec![
        def("mcp__weather__forecast").deferred(),
        def("bash"),
        def("mcp_call").deferred(),
        def("read_file"),
    ];
    for oauth in [false, true] {
        let out = tools_json(&format_tools(&tools, oauth, false));
        let deferred_flags: Vec<bool> = out
            .iter()
            .map(|t| t.get("defer_loading").and_then(|v| v.as_bool()).unwrap_or(false))
            .collect();
        let first_deferred = deferred_flags.iter().position(|d| *d).unwrap();
        assert!(
            deferred_flags[first_deferred..].iter().all(|d| *d),
            "deferred tools must all come after eager ones: {deferred_flags:?}"
        );
        let breakpoints: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, t)| t.get("cache_control").is_some())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(breakpoints, vec![first_deferred - 1], "oauth={oauth}");
    }
}

#[test]
fn adding_deferred_tools_does_not_change_eager_prefix() {
    let base = vec![def("bash"), def("mcp_call").deferred()];
    let mut grown = base.clone();
    grown.push(def("mcp__github__create_issue").deferred());
    grown.push(def("mcp__weather__forecast").deferred());
    let eager_prefix = |tools: &[ToolDefinition]| {
        tools_json(&format_tools(tools, false, false))
            .into_iter()
            .filter(|t| t.get("defer_loading").is_none())
            .collect::<Vec<_>>()
    };
    assert_eq!(eager_prefix(&base), eager_prefix(&grown));
}

#[test]
fn all_deferred_list_is_sent_eagerly() {
    // The API rejects a request in which every tool is deferred.
    let out = tools_json(&format_tools(&[def("mcp_call").deferred()], false, false));
    assert_eq!(out.len(), 1);
    assert!(out[0].get("defer_loading").is_none());
    assert!(out[0].get("cache_control").is_some());
}

#[test]
fn eager_only_lists_serialize_without_defer_loading() {
    let out = tools_json(&format_tools(&[def("bash"), def("read")], false, false));
    assert!(out.iter().all(|t| t.get("defer_loading").is_none()));
    assert!(out[1].get("cache_control").is_some());
}

fn reference_conversation() -> Vec<Message> {
    vec![
        msg(
            Role::User,
            vec![ContentBlock::Text {
                text: "find weather tools".into(),
                cache_control: None,
            }],
        ),
        msg(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "mcp_search".into(),
                input: json!({"query": "weather"}),
                thought_signature: None,
            }],
        ),
        msg(
            Role::User,
            vec![
                ContentBlock::ToolResult {
                    tool_use_id: "toolu_1".into(),
                    content: "[{\"name\":\"mcp__weather__forecast\"}]".into(),
                    is_error: None,
                },
                ContentBlock::ToolReference {
                    tool_use_id: "toolu_1".into(),
                    tool_name: "mcp__weather__forecast".into(),
                },
                ContentBlock::ToolReference {
                    tool_use_id: "toolu_1".into(),
                    tool_name: "mcp__gone__tool".into(),
                },
            ],
        ),
    ]
}

#[test]
fn tool_reference_renders_as_reference_only_tool_result() {
    let tools = format_tools(
        &[
            def("bash"),
            def("mcp_search"),
            def("mcp__weather__forecast").deferred(),
        ],
        false,
        false,
    );
    let api = format_messages_with_tools(&reference_conversation(), false, &tools);
    let last = serde_json::to_value(&api[2]).unwrap();
    let content = last["content"].as_array().unwrap();
    // tool_result first (contiguous), carrying only available references.
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(
        content[0]["content"],
        json!([{"type": "tool_reference", "tool_name": "mcp__weather__forecast"}]),
        "references to unavailable tools must be dropped (400 otherwise), and \
         the tool_result must not mix references with other content"
    );
    // The original result text is preserved right after the tool_results.
    assert_eq!(content[1]["type"], "text");
    assert!(content[1]["text"].as_str().unwrap().contains("mcp__weather__forecast"));
}

#[test]
fn tool_reference_without_available_definition_keeps_plain_result() {
    let tools = format_tools(&[def("bash"), def("mcp_search")], false, false);
    let api = format_messages_with_tools(&reference_conversation(), false, &tools);
    let last = serde_json::to_value(&api[2]).unwrap();
    let content = last["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "tool_result");
    assert!(content[0]["content"].is_string());

    // Legacy entry point ignores references entirely.
    let legacy = serde_json::to_value(&format_messages(&reference_conversation(), false)[2]).unwrap();
    assert_eq!(legacy, last);
}
