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
fn test_recent_window_close_boosts_old_transcript_above_newer_messages_and_saved_rows() {
    let mut old = make_session("closure_old", "old", false, SessionStatus::Active);
    old.last_message_time = Utc::now() - ChronoDuration::days(10);
    old.saved = true;
    let newer = make_session("closure_new", "newer", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![newer, old]);
    picker
        .recent_window_closures
        .insert("closure_old".into(), Utc::now());
    picker
        .live_presence
        .insert("closure_old".into(), live_presence("closure_old", true));
    picker.rebuild_items();
    let ids: Vec<_> = picker
        .visible_session_iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(ids, ["closure_old", "closure_new"]);
    let closed = picker.visible_session_iter().next().unwrap();
    let text = picker
        .render_session_item_lines(closed, false)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("just closed"), "{text}");
    assert!(!picker.session_recently_closed(picker.visible_session_iter().nth(1).unwrap()));
}

#[test]
fn test_recent_window_close_promotes_orphan_above_server_group_without_duplicates() {
    let grouped = make_session("closure_group", "group", false, SessionStatus::Closed);
    let old = make_session(
        "closure_orphan",
        "orphan",
        false,
        SessionStatus::Crashed { message: None },
    );
    let mut picker = SessionPicker::new_grouped(
        vec![ServerGroup {
            name: "server".into(),
            icon: "server".into(),
            version: "v1".into(),
            git_hash: String::new(),
            is_running: true,
            sessions: vec![grouped],
        }],
        vec![old],
    );
    picker
        .recent_window_closures
        .insert("closure_orphan".into(), Utc::now());
    picker.rebuild_items();
    let ids: Vec<_> = picker
        .visible_session_iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(ids, ["closure_orphan", "closure_group"]);
}

#[test]
fn test_recent_window_close_expires_and_rejects_future_timestamps_and_current_session() {
    let session = make_session("closure_expiry", "expiry", false, SessionStatus::Closed);
    let mut picker = SessionPicker::new(vec![session.clone()]);
    for time in [
        Utc::now() - ChronoDuration::seconds(31),
        Utc::now() + ChronoDuration::seconds(10),
    ] {
        picker
            .recent_window_closures
            .insert(session.id.clone(), time);
        assert!(!picker.session_recently_closed(&session));
    }
    picker
        .recent_window_closures
        .insert(session.id.clone(), Utc::now());
    assert!(picker.session_recently_closed(&session));
    picker.set_current_session_id(Some(session.id.clone()));
    assert!(!picker.session_recently_closed(&session));
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
