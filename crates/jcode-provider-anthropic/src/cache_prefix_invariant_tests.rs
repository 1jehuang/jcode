//! Tests for `#[path]`-attributed module `cache_prefix_invariant_tests` of `lib.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

//! Deterministic proof that injecting a trailing memory message can never move
//! the Anthropic prefix-cache breakpoints off the stable assistant prefix.
//!
//! Anthropic caching is strict-prefix: a `cache_control` breakpoint caches every
//! token up to and including the block it sits on. `add_message_cache_breakpoint`
//! always anchors the two breakpoints on the two most recent *assistant* messages.
//! Memory is injected by the agent as a trailing *user* message (see
//! `turn_loops.rs` / `turn_streaming_mpsc.rs`). Therefore the breakpoint anchors,
//! and every token they cache, are identical with or without the memory suffix.
//! These tests pin that invariant so a refactor cannot silently break the cache.
use super::ApiContentBlock;
use jcode_message_types::ToolDefinition;
use serde_json::json;

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

/// A realistic warm conversation: user/assistant turns ending on a user message.
fn base_conversation() -> Vec<Message> {
    vec![
        text_msg(Role::User, "Q1"),
        text_msg(Role::Assistant, "A1"),
        text_msg(Role::User, "Q2"),
        text_msg(Role::Assistant, "A2"),
        text_msg(Role::User, "Q3"),
    ]
}

/// Returns the indices of ApiMessages that carry a cache_control breakpoint,
/// paired with the role of that message.
fn breakpoint_anchors(messages: &[ApiMessage]) -> Vec<(usize, String)> {
    messages
        .iter()
        .enumerate()
        .filter_map(|(i, msg)| {
            let has_bp = msg.content.iter().any(|block| {
                matches!(
                    block,
                    ApiContentBlock::Text {
                        cache_control: Some(_),
                        ..
                    } | ApiContentBlock::ToolUse {
                        cache_control: Some(_),
                        ..
                    }
                )
            });
            has_bp.then(|| (i, msg.role.clone()))
        })
        .collect()
}

/// Serialize only the prefix up to and including the last breakpoint. This is the
/// exact span Anthropic caches; if it is byte-identical across two requests, the
/// cache is guaranteed to be reused.
fn cached_prefix_json(messages: &[ApiMessage]) -> String {
    let last_bp = breakpoint_anchors(messages)
        .last()
        .map(|(idx, _)| *idx)
        .expect("expected at least one cache breakpoint");
    serde_json::to_string(&messages[..=last_bp]).expect("serialize cached prefix")
}

fn formatted_with_breakpoints(messages: &[Message]) -> Vec<ApiMessage> {
    let mut api = format_messages(messages, false);
    add_message_cache_breakpoint(&mut api, false);
    api
}

#[test]
fn breakpoints_anchor_on_assistant_messages_only() {
    let api = formatted_with_breakpoints(&base_conversation());
    let anchors = breakpoint_anchors(&api);
    assert!(!anchors.is_empty(), "expected breakpoints to be placed");
    for (idx, role) in &anchors {
        assert_eq!(
            role, "assistant",
            "breakpoint at message {idx} must be on an assistant message, got {role}"
        );
    }
}

#[test]
fn trailing_memory_message_does_not_move_breakpoints() {
    let base = base_conversation();
    let mut with_memory = base.clone();
    with_memory.push(text_msg(
        Role::User,
        "<memory>relevant recall injected for this turn</memory>",
    ));

    let base_api = formatted_with_breakpoints(&base);
    let mem_api = formatted_with_breakpoints(&with_memory);

    let base_anchors = breakpoint_anchors(&base_api);
    let mem_anchors = breakpoint_anchors(&mem_api);

    assert_eq!(
        base_anchors, mem_anchors,
        "memory suffix moved the cache breakpoints: {base_anchors:?} -> {mem_anchors:?}"
    );
}

#[test]
fn cached_prefix_is_byte_identical_with_and_without_memory() {
    let base = base_conversation();
    let mut with_memory = base.clone();
    with_memory.push(text_msg(
        Role::User,
        "<memory>turn-specific recall</memory>",
    ));

    let base_prefix = cached_prefix_json(&formatted_with_breakpoints(&base));
    let mem_prefix = cached_prefix_json(&formatted_with_breakpoints(&with_memory));

    assert_eq!(
        base_prefix, mem_prefix,
        "the cached prefix span differs once memory is appended; cache would be invalidated"
    );
}

#[test]
fn different_memory_each_turn_keeps_identical_cached_prefix() {
    // The memory content changes every turn. Because it is a trailing user message
    // placed *after* the newest assistant breakpoint, the cached prefix must remain
    // identical regardless of what memory is injected.
    let base = base_conversation();
    let cached = cached_prefix_json(&formatted_with_breakpoints(&base));

    for memory in [
        "<memory>recall A</memory>",
        "<memory>completely different recall B with more text</memory>",
        "",
    ] {
        let mut msgs = base.clone();
        if !memory.is_empty() {
            msgs.push(text_msg(Role::User, memory));
        }
        let candidate = cached_prefix_json(&formatted_with_breakpoints(&msgs));
        assert_eq!(
            cached, candidate,
            "memory variant {memory:?} changed the cached prefix span"
        );
    }
}

fn tool_def(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} description"),
        input_schema: json!({"type":"object","properties":{}}),
        defer_loading: false,
    }
}

#[test]
fn format_tools_removes_top_level_combinators_for_anthropic_api() {
    let tool = ToolDefinition {
        name: "custom".to_string(),
        description: "schema compatibility regression".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {"type": "string"},
                "nested_union": {
                    "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]
                }
            },
            "required": ["action"],
            "oneOf": [
                {"type": "object", "properties": {"label": {"type": "string"}}},
                {"type": "object", "properties": {"task_id": {"type": "string"}}}
            ],
            "allOf": [
                {"type": "object", "properties": {"intent": {"type": "string"}}, "required": ["intent"]}
            ]
        }),
        defer_loading: false,
    };

    let formatted = format_tools(&[tool], false, false);
    let schema = &formatted[0].input_schema;
    for keyword in ["oneOf", "anyOf", "allOf"] {
        assert!(
            schema.get(keyword).is_none(),
            "Anthropic rejects top-level {keyword}: {schema}"
        );
    }
    for property in ["action", "nested_union", "label", "task_id", "intent"] {
        assert!(
            schema["properties"].get(property).is_some(),
            "missing merged property {property}: {schema}"
        );
    }
    assert!(
        schema["properties"]["nested_union"].get("anyOf").is_some(),
        "nested combinators remain supported and should not be flattened"
    );
    assert_eq!(schema["required"], json!(["action", "intent"]));
}

#[test]
fn oauth_format_tools_keeps_full_custom_toolset() {
    // Registry includes builtins (remapped) plus extra tools that must survive.
    let registry = vec![
        tool_def("bash"),
        tool_def("read"),
        tool_def("write"),
        tool_def("edit"),
        tool_def("glob"),
        tool_def("grep"),
        tool_def("subagent"),
        tool_def("websearch"),
        tool_def("webfetch"),
        tool_def("browser"),
        tool_def("codesearch"),
        tool_def("memory"),
    ];

    let formatted = format_tools(&registry, true, false);
    let names: Vec<&str> = formatted.iter().map(|t| t.name.as_str()).collect();

    // Curated builtins are present under their OAuth names.
    for builtin in ["Bash", "Read", "Agent", "Write", "Edit", "Glob", "Grep"] {
        assert!(
            names.contains(&builtin),
            "missing builtin {builtin} in {names:?}"
        );
    }
    // The previously-dropped custom tools are now forwarded.
    for custom in ["websearch", "webfetch", "browser", "codesearch", "memory"] {
        assert!(
            names.contains(&custom),
            "custom tool {custom} was dropped on OAuth; got {names:?}"
        );
    }
    // No duplicate Agent/Bash/Read from the registry remap.
    assert_eq!(names.iter().filter(|n| **n == "Agent").count(), 1);
    assert_eq!(names.iter().filter(|n| **n == "Bash").count(), 1);
    assert_eq!(names.iter().filter(|n| **n == "Read").count(), 1);
}

#[test]
fn oauth_format_tools_drops_builtins_missing_from_registry() {
    // subagent and glob no longer exist in the registry, so the curated
    // Agent/Glob builtins must not be advertised (see #572).
    let registry = vec![
        tool_def("bash"),
        tool_def("read"),
        tool_def("write"),
        tool_def("edit"),
        tool_def("agentgrep"),
    ];
    let formatted = format_tools(&registry, true, false);
    let names: Vec<&str> = formatted.iter().map(|t| t.name.as_str()).collect();

    for ghost in ["Agent", "Glob", "Grep", "Skill"] {
        assert!(
            !names.contains(&ghost),
            "advertised ghost builtin {ghost} without a backing registry tool: {names:?}"
        );
    }
    for present in ["Bash", "Read", "Write", "Edit", "agentgrep"] {
        assert!(names.contains(&present), "missing {present} in {names:?}");
    }
}

#[test]
fn oauth_format_tools_places_single_cache_breakpoint_on_last_tool() {
    let registry = vec![tool_def("bash"), tool_def("websearch")];
    let formatted = format_tools(&registry, true, false);
    let with_cache: Vec<&str> = formatted
        .iter()
        .filter(|t| t.cache_control.is_some())
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(with_cache.len(), 1, "expected exactly one cache breakpoint");
    assert_eq!(
        formatted.last().map(|t| t.name.as_str()),
        with_cache.first().copied(),
        "cache breakpoint must be on the final tool"
    );
}
