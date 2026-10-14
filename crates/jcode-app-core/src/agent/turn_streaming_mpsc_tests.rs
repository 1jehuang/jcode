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

/// Reference O(n) full scan, preserving the original precedence: the
/// `to=functions.` marker is checked before `+#+#`.
fn find_wrap_marker_full(text: &str) -> Option<usize> {
    text.find("to=functions.").or_else(|| text.find("+#+#"))
}

/// Simulate streaming `full` in arbitrary deltas and assert the incremental
/// scan finds the first marker position, matching a full rescan each step.
fn assert_incremental_matches(full: &str, chunk: usize) {
    let mut acc = String::new();
    let mut incremental_hit: Option<usize> = None;
    let bytes = full.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let mut end = (i + chunk).min(bytes.len());
        while end < bytes.len() && !full.is_char_boundary(end) {
            end += 1;
        }
        let delta = &full[i..end];
        acc.push_str(delta);
        if incremental_hit.is_none() {
            incremental_hit = find_wrap_marker_incremental(&acc, delta.len());
        }
        i = end;
    }
    // The earliest of either marker in the full text.
    let fn_pos = full.find("to=functions.");
    let plus_pos = full.find("+#+#");
    let expected = match (fn_pos, plus_pos) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    assert_eq!(
        incremental_hit, expected,
        "incremental scan mismatch for {full:?} chunk={chunk}"
    );
}

#[test]
fn wrap_marker_incremental_detects_markers_across_chunk_sizes() {
    let cases = [
        "plain answer with no marker at all",
        "answer then to=functions.foo({})",
        "answer then +#+# wrapped",
        "prefix +#+# and later to=functions.bar",
        "unicode 🔄 résumé then to=functions.baz",
        "",
        "to=functions.first",
        "+#+#",
    ];
    for case in cases {
        for chunk in [1usize, 2, 3, 5, 7, 100] {
            assert_incremental_matches(case, chunk);
        }
    }
}

#[test]
fn wrap_marker_incremental_finds_marker_straddling_delta_boundary() {
    // Feed "to=functions." split right in the middle so the marker only
    // exists once both halves are appended; the overlap window must catch it.
    let mut acc = String::new();
    acc.push_str("answer to=fun");
    assert_eq!(
        find_wrap_marker_incremental(&acc, "answer to=fun".len()),
        None
    );
    acc.push_str("ctions.tool");
    let hit = find_wrap_marker_incremental(&acc, "ctions.tool".len());
    assert_eq!(hit, find_wrap_marker_full(&acc));
    assert_eq!(hit, Some("answer ".len()));
}
