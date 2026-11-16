//! Focused activity regression tests.
use super::*;

#[test]
fn test_activity_detail_prefers_intent_and_appends_summary() {
    tools_ui::tests_tool_call_details_override::set(true);
    let tool = ToolCall {
        id: "swarm-1".to_string(),
        name: "swarm".to_string(),
        input: serde_json::json!({
            "intent": "Spin up a worker for the parser fix",
            "action": "spawn",
            "prompt": "Fix the parser bug in crates/parser"
        }),
        intent: Some("Spin up a worker for the parser fix".to_string()),
        thought_signature: None,
    };

    let detail = tools_ui::get_tool_activity_detail(&tool);
    assert!(
        detail.starts_with("Spin up a worker for the parser fix"),
        "intent should lead the activity detail: {detail:?}"
    );
    assert!(
        detail.contains("spawn"),
        "technical summary should still appear: {detail:?}"
    );
    tools_ui::tests_tool_call_details_override::set(false);
}

/// When the `ToolCall.intent` field is not populated yet (e.g. streamed input
/// parsed but intent refresh missed), fall back to the raw `intent` input key.
#[test]
fn test_activity_detail_falls_back_to_input_intent_field() {
    let tool = ToolCall {
        id: "swarm-2".to_string(),
        name: "swarm".to_string(),
        input: serde_json::json!({
            "intent": "Check on worker progress",
            "action": "status",
            "target_session": "worker-1"
        }),
        intent: None,
        thought_signature: None,
    };

    let detail = tools_ui::get_tool_activity_detail(&tool);
    assert!(
        detail.starts_with("Check on worker progress"),
        "input intent should be used when the field is unset: {detail:?}"
    );
}

/// Without an intent, the activity detail matches the plain technical summary.
#[test]
fn test_activity_detail_without_intent_matches_summary() {
    let tool = ToolCall {
        id: "swarm-3".to_string(),
        name: "swarm".to_string(),
        input: serde_json::json!({ "action": "dm", "to_session": "worker-1", "message": "hello" }),
        intent: None,
        thought_signature: None,
    };

    let detail = tools_ui::get_tool_activity_detail(&tool);
    let summary = tools_ui::get_tool_summary(&tool);
    assert_eq!(detail, summary);
    assert!(!detail.is_empty());
}
