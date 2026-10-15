fn live_presence(session_id: &str, streaming: bool) -> crate::session::SessionPresence {
    crate::session::SessionPresence {
        session_id: session_id.to_string(),
        pid: std::process::id(),
        streaming,
        streaming_since: streaming
            .then(|| std::time::SystemTime::now() - std::time::Duration::from_secs(90)),
        internal: false,
    }
}

#[test]
fn test_active_filter_shows_only_live_sessions_ready_before_working() {
    let live_working = make_session("session_working", "alpha", false, SessionStatus::Active);
    let live_ready = make_session("session_ready", "beta", false, SessionStatus::Active);
    let dead = make_session("session_dead", "dead", false, SessionStatus::Closed);

    let mut picker = SessionPicker::new(vec![live_working, live_ready, dead]);
    picker.activate_active_filter();
    picker.set_live_presence_for_test(vec![
        live_presence("session_working", true),
        live_presence("session_ready", false),
    ]);

    let visible: Vec<&str> = picker
        .visible_session_iter()
        .map(|session| session.id.as_str())
        .collect();
    // Only live sessions appear; the ready one is triaged above the working one.
    assert_eq!(visible, vec!["session_ready", "session_working"]);

    let ready = picker
        .visible_session_iter()
        .find(|session| session.id == "session_ready")
        .expect("ready visible");
    let working = picker
        .visible_session_iter()
        .find(|session| session.id == "session_working")
        .expect("working visible");
    assert!(!picker.session_is_streaming(ready));
    assert!(picker.session_is_streaming(working));
    assert!(picker.session_streaming_duration(working).is_some());
}

#[test]
fn test_active_rows_render_working_and_ready_badges() {
    let live_working = make_session("session_working", "alpha", false, SessionStatus::Active);
    let live_ready = make_session("session_ready", "beta", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![live_working, live_ready]);
    picker.activate_active_filter();
    picker.set_live_presence_for_test(vec![
        live_presence("session_working", true),
        live_presence("session_ready", false),
    ]);

    let working = picker
        .visible_session_iter()
        .find(|session| session.id == "session_working")
        .cloned()
        .expect("working visible");
    let ready = picker
        .visible_session_iter()
        .find(|session| session.id == "session_ready")
        .cloned()
        .expect("ready visible");

    let working_lines = picker.render_session_item_lines(&working, false);
    let working_text = working_lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        working_text.contains("working 1m"),
        "expected working badge with duration, got: {working_text}"
    );

    let first_frame = picker
        .render_session_item_lines_at_frame(&working, false, 0)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let second_frame = picker
        .render_session_item_lines_at_frame(&working, false, 1)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(first_frame.contains('⠋'));
    assert!(second_frame.contains('⠙'));
    assert_ne!(first_frame, second_frame, "running glyph should animate");
    assert!(picker.has_visible_running_sessions());

    let ready_lines = picker.render_session_item_lines(&ready, false);
    let ready_text = ready_lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        ready_text.contains("ready"),
        "expected ready badge, got: {ready_text}"
    );
    // Live presence overrides the stale persisted "closed" status.
    assert!(
        !ready_text.contains("closed"),
        "live session must not render as closed: {ready_text}"
    );
}

#[test]
fn test_current_session_row_is_labeled() {
    let session = make_session("session_self", "self", false, SessionStatus::Active);
    let mut picker = SessionPicker::new(vec![session.clone()]);
    picker.set_current_session_id(Some("session_self".to_string()));

    let lines = picker.render_session_item_lines(&session, false);
    let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
    assert!(
        text.contains("current"),
        "expected current label, got: {text}"
    );
}

#[test]
fn test_maybe_refresh_live_presence_throttles_and_detects_changes() {
    let session = make_session("session_live", "live", false, SessionStatus::Active);
    let mut picker = SessionPicker::new(vec![session]);
    picker.activate_active_filter();
    // Freshly injected snapshot: refresh is throttled, nothing changes.
    picker.set_live_presence_for_test(vec![live_presence("session_live", true)]);
    assert!(!picker.maybe_refresh_live_presence());
}
