//! Focused error rows regression tests.
use super::*;

#[test]
fn test_prepare_messages_keeps_multiline_error_rows() {
    let state = TestState {
        display_messages: vec![DisplayMessage::error(
            "/cloud failed. Nothing moved.\n\nError: remote failed\nCaused by:\n    host down",
        )],
        ..Default::default()
    };
    let rendered = prepare::prepare_messages(&state, 110, 30)
        .materialize_all_lines()
        .iter()
        .map(extract_line_text)
        .collect::<Vec<_>>();
    let all = rendered.join("\n");
    let first = rendered
        .iter()
        .position(|line| line.contains("✗ /cloud failed. Nothing moved."))
        .unwrap_or_else(|| panic!("missing headline: {all}"));
    let error = rendered
        .iter()
        .position(|line| line.contains("Error: remote failed"))
        .unwrap_or_else(|| panic!("missing error row: {all}"));
    let cause = rendered
        .iter()
        .position(|line| line.contains("    host down"))
        .unwrap_or_else(|| panic!("missing cause row: {all}"));
    assert!(first < error && error < cause, "rows collapsed: {all}");
    assert!(!rendered[first].contains("Error:"), "rows collapsed: {all}");
}

#[test]
fn cloud_failure_reason_is_visible_in_rendered_chat() {
    for width in [40, 100] {
        let state = TestState {
            display_messages: vec![DisplayMessage::error(
                "/cloud failed. Reason: Unreturned cloud changes would be overwritten. Recover the earlier session first.\n\nError: remote receive failed",
            )],
            ..Default::default()
        };
        let rendered = prepare::prepare_messages(&state, width, 40)
            .materialize_all_lines()
            .iter()
            .map(extract_line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Reason:"), "{rendered}");
        assert!(rendered.contains("session first."), "{rendered}");
    }
}
