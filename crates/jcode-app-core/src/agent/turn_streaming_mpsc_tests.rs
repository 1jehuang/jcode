#[test]
fn abnormal_incomplete_stop_excludes_natural_completion() {
    for reason in [None, Some("end_turn"), Some("stop")] {
        assert!(super::incomplete_turn_stop(reason).is_none());
    }
    for reason in ["max_tokens", "length", "tool_use"] {
        assert!(
            matches!(super::incomplete_turn_stop(Some(reason)), Some(crate::protocol::ServerEvent::TurnStopped {
            reason: crate::protocol::TurnStopReason::LimitReached,
            provider_stop_reason: Some(raw), ..
        }) if raw == reason)
        );
    }
}

use super::*;
use serde_json::json;

fn tool_call(name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "toolu_test".to_string(),
        name: name.to_string(),
        input,
        intent: None,
        thought_signature: None,
    }
}

#[test]
fn reload_interrupted_bg_wait_is_non_error_and_resumable() {
    let tc = tool_call(
        "bg",
        json!({"action": "wait", "task_id": "bg-123", "max_wait_seconds": 300}),
    );

    let (message, is_error) = reload_interrupted_tool_result(&tc, 1.2);

    assert!(!is_error);
    assert!(message.contains("Resume the wait"));
    assert!(message.contains("\"task_id\":\"bg-123\""));
}

#[test]
fn reload_interrupted_non_wait_tool_remains_error() {
    let tc = tool_call("bash", json!({"command": "sleep 10"}));

    let (message, is_error) = reload_interrupted_tool_result(&tc, 1.2);

    assert!(is_error);
    assert!(message.contains("interrupted by server reload"));
}
