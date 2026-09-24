//! Acceptance tests for the tool row duration badge (#1453): how long each
//! tool call took, rendered after the token count with severity coloring.

use super::*;

/// Enable the opt-in badge for the duration-acceptance tests below. Each test
/// that flips this must reset it (see `reset_tool_duration_opt_in`), and tests
/// that assert the default-off behavior must not enable it at all.
fn enable_tool_duration_opt_in() {
    crate::tui::ui::tools_ui::tests_show_tool_duration_override::set(true);
}

fn reset_tool_duration_opt_in() {
    crate::tui::ui::tools_ui::tests_show_tool_duration_override::set(false);
}

fn duration_tool_msg(tool_duration_ms: Option<u64>) -> DisplayMessage {
    DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-dur".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo ok" }),
            intent: None,
            thought_signature: None,
        }),
        timestamp: None,
        tool_duration_ms,
    }
}

/// A completed tool row with a stored duration renders the compact duration
/// after the token count, through the real render_tool_message pipeline.
#[test]
fn test_tool_row_renders_duration_badge() {
    let _lock = viewport_snapshot_test_lock();
    enable_tool_duration_opt_in();
    let msg = duration_tool_msg(Some(48_300));

    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    reset_tool_duration_opt_in();
    let row: String = lines
        .first()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .unwrap_or_default();

    assert!(row.contains("48.3s"), "duration badge missing: {row}");
    assert!(row.contains("tok"), "token badge must stay: {row}");
    let tok_pos = row.find("tok").expect("tokens present");
    let dur_pos = row.find("48.3s").expect("duration present");
    assert!(tok_pos < dur_pos, "duration trails token badge: {row}");
}

/// With `display.show_tool_duration` at its default (false), rows carrying a
/// stored duration render no badge at all: the feature is strictly opt-in.
#[test]
fn test_tool_row_duration_badge_is_opt_in_default_off() {
    let _lock = viewport_snapshot_test_lock();
    // Override stays at its default false, mirroring an unset config key.
    let msg = duration_tool_msg(Some(48_300));

    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    let row: String = lines
        .first()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .unwrap_or_default();

    assert!(!row.contains("48.3s"), "badge must be opt-in: {row}");
    assert!(!row.contains("ms"), "no duration expected: {row}");
    assert!(row.contains("tok"), "token badge remains: {row}");
}

/// Rows without a duration (older sessions, synthetic messages) keep the
/// classic token-only badge: no empty separator pair.
#[test]
fn test_tool_row_without_duration_has_no_badge() {
    enable_tool_duration_opt_in();
    let msg = duration_tool_msg(None);
    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    reset_tool_duration_opt_in();
    let row: String = lines
        .first()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .unwrap_or_default();
    assert!(!row.contains("ms"), "no duration expected: {row}");
    assert!(!row.contains(" ·  "), "no empty separator pair: {row}");
}

/// Duration severity colors the badge: 48.3s is Warning-range amber. The
/// same span keeps neutral blue-grey below the warning threshold.
#[test]
fn test_tool_row_duration_badge_severity_colors() {
    let _lock = viewport_snapshot_test_lock();
    enable_tool_duration_opt_in();

    let warn = duration_tool_msg(Some(48_300));
    let lines = messages::render_tool_message(&warn, 200, crate::config::DiffDisplayMode::Off);
    let span = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("48.3s"))
        .expect("duration span present");
    assert_eq!(span.style.fg, Some(rgb(214, 184, 92)), "48.3s is amber");

    let normal = duration_tool_msg(Some(700));
    let lines = messages::render_tool_message(&normal, 200, crate::config::DiffDisplayMode::Off);
    let span = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("700ms"))
        .expect("duration span present");
    assert_eq!(
        span.style.fg,
        Some(rgb(120, 130, 145)),
        "700ms stays neutral blue-grey"
    );

    let danger = duration_tool_msg(Some(87_456));
    let lines = messages::render_tool_message(&danger, 200, crate::config::DiffDisplayMode::Off);
    reset_tool_duration_opt_in();
    let span = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("1m 27s"))
        .expect("duration span present");
    assert_eq!(span.style.fg, Some(rgb(224, 118, 118)), "1m27s is red");
}

/// The badge survives narrow widths as part of the preserved suffix: the
/// summary truncates first, the duration and token badges stay.
#[test]
fn test_tool_row_duration_badge_survives_narrow_width() {
    let _lock = viewport_snapshot_test_lock();
    enable_tool_duration_opt_in();
    let msg = DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-narrow".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "a-very-long-command" }),
            intent: Some("a very long intent summary that must truncate first".to_string()),
            thought_signature: None,
        }),
        timestamp: None,
        tool_duration_ms: Some(48_300),
    };

    for width in [40, 56, 72, 120] {
        let lines = messages::render_tool_message(&msg, width, crate::config::DiffDisplayMode::Off);
        let row: String = lines
            .first()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default();
        assert!(
            row.contains("48.3s"),
            "duration lost at width {width}: {row}"
        );
        assert!(row.contains("tok"), "tokens lost at width {width}: {row}");
    }
    reset_tool_duration_opt_in();
}

/// Observed-behavior proof for the "0.0s" noise complaint: near-instant
/// tools render milliseconds, never a bare "0.0s" or "0ms".
#[test]
fn test_tool_row_ms_duration_no_zero_noise() {
    let _lock = viewport_snapshot_test_lock();
    enable_tool_duration_opt_in();
    let msg = duration_tool_msg(Some(45));
    let lines = messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off);
    reset_tool_duration_opt_in();
    let row: String = lines
        .first()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .unwrap_or_default();
    println!("observed 45ms row: {row}");
    assert!(row.contains("45ms"), "ms duration missing: {row}");
    assert!(!row.contains("0.0s"), "0.0s banned: {row}");
    assert!(!row.contains("0ms"), "0ms banned: {row}");
}
