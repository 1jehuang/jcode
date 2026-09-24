//! #1454 acceptance tests: the opt-in tool row time badge. When
//! `display.show_tool_timestamp` is enabled, completed tool rows carry an
//! " · HH:MM:SS" stamp after the token count (and any duration badge),
//! honoring `display.timestamp_tz`; the clock span is always neutral.

use super::*;

fn time_tool_msg(timestamp: Option<chrono::DateTime<chrono::Utc>>, ms: Option<u64>) -> DisplayMessage {
    DisplayMessage {
        role: "tool".to_string(),
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        duration_secs: None,
        title: None,
        tool_data: Some(ToolCall {
            id: "call-ts".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": "echo ok" }),
            intent: None,
            thought_signature: None,
        }),
        timestamp,
        tool_duration_ms: ms,
    }
}

fn rendered_first_row(msg: &DisplayMessage, width: usize) -> String {
    let lines = messages::render_tool_message(msg, width, crate::config::DiffDisplayMode::Off);
    lines
        .first()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .unwrap_or_default()
}

/// The opt-in stamp renders HH:MM:SS after the token count (and the duration
/// badge) through the real render_tool_message pipeline.
#[test]
fn test_tool_row_stamp_renders_when_enabled() {
    let _lock = viewport_snapshot_test_lock();
    let _config = isolate_config_home_with(
        "[display]\nshow_tool_timestamp = true\n",
    );
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:23:35Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = time_tool_msg(Some(stamp), Some(48_300));

    let row = rendered_first_row(&msg, 200);
    let expected = stamp.with_timezone(&chrono::Local).format("%H:%M:%S").to_string();
    assert!(row.contains(&expected), "stamp missing from row: {row}");
    assert!(row.contains("48.3s"), "duration badge missing: {row}");
    assert!(row.contains("tok"), "token badge must stay: {row}");
    let tok_pos = row.find("tok").expect("tokens present");
    let dur_pos = row.find("48.3s").expect("duration present");
    let stamp_pos = row.find(&expected).expect("stamp present");
    assert!(
        tok_pos < dur_pos && dur_pos < stamp_pos,
        "order must be token -> duration -> stamp: {row}"
    );
}

/// Default (option off): no stamp, regardless of stored timestamp.
#[test]
fn test_tool_row_stamp_absent_by_default() {
    let _lock = viewport_snapshot_test_lock();
    let _config = isolate_config_home();
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:23:35Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = time_tool_msg(Some(stamp), Some(48_300));

    let row = rendered_first_row(&msg, 200);
    assert!(row.contains("48.3s"), "duration badge stays: {row}");
    assert!(!row.contains("::"), "no time stamp expected: {row}");
}

/// End-to-end acceptance of the configured-offset request: 20:23:35Z stored
/// with display.timestamp_tz = "UTC+3" renders 23:23:35.
#[test]
fn test_tool_row_stamp_renders_in_configured_utc3() {
    let _lock = viewport_snapshot_test_lock();
    let _guard = isolate_config_home_with(
        "[display]\nshow_tool_timestamp = true\ntimestamp_tz = \"UTC+3\"\n",
    );
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:23:35Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = time_tool_msg(Some(stamp), Some(45));

    let row = rendered_first_row(&msg, 200);
    println!("observed UTC+3 row: {row}");
    assert!(
        row.contains("23:23:35"),
        "UTC+3 stamp (20:23Z + 3h = 23:23) missing: {row}"
    );
    assert!(row.contains("45ms"), "ms duration missing: {row}");
}

/// Exact offset arithmetic across the day boundary: 21:51:28Z at UTC+3 must
/// render 00:51:28 (next day). The fixed offset makes this independent of the
/// machine's timezone.
#[test]
fn test_tool_row_utc3_exact_offset_arithmetic_across_day_boundary() {
    let _lock = viewport_snapshot_test_lock();
    let _guard = isolate_config_home_with(
        "[display]\nshow_tool_timestamp = true\ntimestamp_tz = \"UTC+3\"\n",
    );
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T21:51:28Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = time_tool_msg(Some(stamp), Some(2_325));

    let row = rendered_first_row(&msg, 200);
    println!("observed: {row}");
    assert!(
        row.contains("00:51:28"),
        "21:51:28Z + 3h must be 00:51:28 UTC+3: {row}"
    );
    assert!(row.contains("2.3s"), "2_325ms must render 2.3s: {row}");
}

/// The clock span is always neutral blue-grey: it never inherits the duration
/// severity color (amber/red), whatever the duration is.
#[test]
fn test_tool_row_stamp_stays_neutral_for_any_duration_severity() {
    let _lock = viewport_snapshot_test_lock();
    let _config = isolate_config_home_with("[display]\nshow_tool_timestamp = true\n");
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:15:42Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);

    for ms in [700u64, 48_300, 87_456] {
        let msg = time_tool_msg(Some(stamp), Some(ms));
        let spans = &messages::render_tool_message(&msg, 200, crate::config::DiffDisplayMode::Off)
            .first()
            .expect("tool row rendered")
            .spans;

        let stamp_span = spans
            .iter()
            .find(|s| s.content.contains("20:15:42"))
            .unwrap_or_else(|| panic!("stamp span present for {ms}ms"));
        assert_eq!(
            stamp_span.style.fg,
            Some(rgb(120, 130, 145)),
            "stamp must stay neutral blue-grey for {ms}ms: {stamp_span:?}"
        );
    }
}

/// The stamp survives narrow widths as part of the preserved suffix: summary
/// truncates first, token badge, duration badge and stamp all stay.
#[test]
fn test_tool_row_stamp_survives_narrow_width() {
    let _lock = viewport_snapshot_test_lock();
    let _config = isolate_config_home_with(
        "[display]\nshow_tool_timestamp = true\ntimestamp_tz = \"UTC+3\"\n",
    );
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:15:42Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let mut msg = time_tool_msg(Some(stamp), Some(48_300));
    if let Some(tc) = msg.tool_data.as_mut() {
        tc.intent = Some("a very long intent summary that must truncate first".to_string());
    }

    for width in [40, 56, 72, 120] {
        let row = rendered_first_row(&msg, width);
        assert!(row.contains("23:15:42"), "stamp lost at width {width}: {row}");
        assert!(row.contains("48.3s"), "duration lost at width {width}: {row}");
        assert!(row.contains("tok"), "tokens lost at width {width}: {row}");
        let tok_pos = row.find("tok").expect("tokens present");
        let stamp_pos = row.find("23:15:42").expect("stamp present");
        assert!(
            tok_pos < stamp_pos,
            "stamp must trail the token badge at width {width}: {row}"
        );
    }
}

/// A garbage timestamp_tz falls back to local instead of breaking rendering.
#[test]
fn test_tool_row_stamp_garbage_tz_falls_back_to_local() {
    let _lock = viewport_snapshot_test_lock();
    let _guard = isolate_config_home_with(
        "[display]\nshow_tool_timestamp = true\ntimestamp_tz = \"Moscow\"\n",
    );
    let stamp = chrono::DateTime::parse_from_rfc3339("2026-09-23T20:23:35Z")
        .expect("parse stamp")
        .with_timezone(&chrono::Utc);
    let msg = time_tool_msg(Some(stamp), None);

    let row = rendered_first_row(&msg, 200);
    let expected = stamp.with_timezone(&chrono::Local).format("%H:%M:%S").to_string();
    assert!(
        row.contains(&expected),
        "garbage tz must fall back to local ({expected}): {row}"
    );
}
