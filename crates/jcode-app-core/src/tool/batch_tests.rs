use super::*;
use serde_json::json;

#[test]
fn test_normalize_flat_params() {
    let input = json!({
        "tool_calls": [
            {"tool": "read", "file_path": "file1.txt"},
            {"tool": "read", "file_path": "file2.txt"}
        ]
    });

    let normalized = normalize_batch_input(input);
    let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
    assert_eq!(parsed.tool_calls.len(), 2);
    assert_eq!(parsed.tool_calls[0].tool, "read");
    let params = parsed.tool_calls[0].parameters.as_ref().unwrap();
    assert_eq!(params["file_path"], "file1.txt");
}

#[test]
fn test_normalize_already_nested() {
    let input = json!({
        "tool_calls": [
            {"tool": "read", "parameters": {"file_path": "file1.txt"}}
        ]
    });

    let normalized = normalize_batch_input(input);
    let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
    assert_eq!(parsed.tool_calls.len(), 1);
    let params = parsed.tool_calls[0].parameters.as_ref().unwrap();
    assert_eq!(params["file_path"], "file1.txt");
}

#[test]
fn test_normalize_forwards_top_level_intent_into_nested_parameters() {
    let input = json!({
        "tool_calls": [{
            "tool": "read",
            "intent": "Inspect the batch renderer",
            "parameters": {"file_path": "src/tui/ui_messages.rs"}
        }]
    });

    let normalized = normalize_batch_input(input);
    let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
    let params = parsed.tool_calls[0].parameters.as_ref().unwrap();

    assert_eq!(params["intent"], "Inspect the batch renderer");
    assert_eq!(params["file_path"], "src/tui/ui_messages.rs");
}

#[test]
fn test_normalize_name_key_to_tool() {
    let input = json!({
        "tool_calls": [
            {"name": "read", "parameters": {"file_path": "file1.txt"}},
            {"name": "grep", "pattern": "foo", "path": "src/"}
        ]
    });

    let normalized = normalize_batch_input(input);
    let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
    assert_eq!(parsed.tool_calls.len(), 2);
    assert_eq!(parsed.tool_calls[0].tool, "read");
    let params0 = parsed.tool_calls[0].parameters.as_ref().unwrap();
    assert_eq!(params0["file_path"], "file1.txt");
    assert_eq!(parsed.tool_calls[1].tool, "grep");
    let params1 = parsed.tool_calls[1].parameters.as_ref().unwrap();
    assert_eq!(params1["pattern"], "foo");
}

#[test]
fn test_normalize_mixed_tool_and_name_keys() {
    let input = json!({
        "tool_calls": [
            {"tool": "read", "parameters": {"file_path": "a.rs"}},
            {"name": "read", "parameters": {"file_path": "b.rs"}},
            {"tool": "grep", "pattern": "test"}
        ]
    });

    let normalized = normalize_batch_input(input);
    let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
    assert_eq!(parsed.tool_calls.len(), 3);
    assert_eq!(parsed.tool_calls[0].tool, "read");
    assert_eq!(parsed.tool_calls[1].tool, "read");
    assert_eq!(parsed.tool_calls[2].tool, "grep");
}

#[test]
fn test_normalize_arguments_aliases_to_parameters() {
    let input = json!({
        "tool_calls": [
            {"tool": "read", "arguments": {"file_path": "a.rs"}},
            {"tool": "read", "args": {"file_path": "b.rs"}},
            {"tool": "read", "input": {"file_path": "c.rs"}}
        ]
    });

    let normalized = normalize_batch_input(input);
    let parsed: BatchInput = serde_json::from_value(normalized).unwrap();

    assert_eq!(parsed.tool_calls.len(), 3);
    assert_eq!(
        parsed.tool_calls[0].parameters.as_ref().unwrap()["file_path"],
        "a.rs"
    );
    assert_eq!(
        parsed.tool_calls[1].parameters.as_ref().unwrap()["file_path"],
        "b.rs"
    );
    assert_eq!(
        parsed.tool_calls[2].parameters.as_ref().unwrap()["file_path"],
        "c.rs"
    );
}

#[test]
fn test_schema_only_requires_tool() {
    let schema = BatchTool::new(Registry {
        tools: std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        skills: std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::skill::SkillRegistry::default(),
        )),
        compaction: std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::compaction::CompactionManager::new(),
        )),
    })
    .parameters_schema();

    assert_eq!(
        schema["properties"]["tool_calls"]["items"]["required"],
        // Nested batch entries require `intent` alongside `tool` so every
        // fanned-out call carries a display label, matching the central
        // intent requirement in `ensure_intent_in_schema` (8505080a6).
        json!(["tool", "intent"])
    );
    assert_eq!(
        schema["properties"]["tool_calls"]["items"]["additionalProperties"],
        json!(true)
    );
    assert_eq!(
        schema["properties"]["tool_calls"]["items"]["properties"]["tool"]["description"],
        json!("Tool name.")
    );
    assert!(schema["properties"]["tool_calls"]["items"]["properties"]["intent"].is_object());
    assert!(schema["properties"]["tool_calls"]["items"]["properties"]["parameters"].is_null());
}

#[test]
fn test_schema_keeps_flat_generic_subcall_shape() {
    let schema = generic_batch_schema();

    assert!(schema["properties"]["tool_calls"]["description"].is_null());
    assert!(schema["properties"]["tool_calls"]["items"]["description"].is_null());
    assert_eq!(
        schema["properties"]["tool_calls"]["items"]["properties"]
            .as_object()
            .map(|props| props.len()),
        Some(2)
    );
    assert!(schema["properties"]["tool_calls"]["items"]["oneOf"].is_null());
}

#[test]
fn subcall_level_accept_large_output_is_forwarded_into_parameters() {
    // Models place the flag beside `tool` rather than inside `parameters`, the
    // same mistake they already make with `intent`. The guard runs per sub-call
    // on that sub-call's parameters, so a flag left at the wrong level is
    // silently dropped and the sub-call withheld again.
    let input = serde_json::json!({
        "tool_calls": [{
            "tool": "agentgrep",
            "accept_large_output": true,
            "parameters": { "query": "x" },
        }]
    });
    let out = super::normalize_batch_input(input);
    assert_eq!(
        out["tool_calls"][0]["parameters"][jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY],
        serde_json::json!(true),
        "flag beside `tool` must reach the sub-call parameters"
    );
}

#[test]
fn subcall_level_accept_large_output_does_not_override_an_explicit_value() {
    // An explicit `false` inside parameters is a deliberate choice for that one
    // sub-call and must win over a blanket flag beside `tool`.
    let input = serde_json::json!({
        "tool_calls": [{
            "tool": "agentgrep",
            "accept_large_output": true,
            "parameters": { "query": "x", "accept_large_output": false },
        }]
    });
    let out = super::normalize_batch_input(input);
    assert_eq!(
        out["tool_calls"][0]["parameters"][jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY],
        serde_json::json!(false),
        "explicit per-subcall value must win"
    );
}
