#[test]
fn test_handle_server_event_updates_status_detail() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::StatusDetail {
            detail: "reusing websocket".to_string(),
        },
        &mut remote,
    );

    assert_eq!(app.status_detail.as_deref(), Some("reusing websocket"));
}

#[test]
fn test_handle_server_event_transcript_replace_updates_input() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.input = "old draft".to_string();
    app.cursor_pos = app.input.len();

    app.handle_server_event(
        crate::protocol::ServerEvent::Transcript {
            text: "new dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Replace,
        },
        &mut remote,
    );

    assert_eq!(app.input, "new dictated text");
    assert_eq!(app.cursor_pos, app.input.len());
    assert_eq!(
        app.status_notice(),
        Some("Transcript replaced input".to_string())
    );
}

#[test]
fn test_local_bus_dictation_completion_applies_transcript() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some(session_id.clone());

    crate::tui::app::local::handle_bus_event(
        &mut app,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_123".to_string(),
            session_id: Some(session_id),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    );

    assert_eq!(app.input, "draft dictated text");
    assert_eq!(app.status_notice(), Some("Transcript appended".to_string()));
}

/// SwarmStatus snapshots must surface member lifecycle transitions (an agent
/// finishing) as a status notice, scoped to this session's spawn subtree so a
/// shared swarm's unrelated agents stay silent. Live member details render from
/// their own snapshot and must not churn the global transcript cache version.
#[test]
fn test_handle_server_event_swarm_status_announces_member_completion() {
    let member = |id: &str, status: &str, parent: Option<&str>| crate::protocol::SwarmMemberStatus {
        session_id: id.to_string(),
        friendly_name: Some(id.to_string()),
        status: status.to_string(),
        detail: None,
        task_label: None,
        role: None,
        is_headless: Some(true),
        live_attachments: None,
        status_age_secs: Some(1),
        output_tail: None,
        report_back_to_session_id: parent.map(str::to_string),
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    };

    let mut app = create_test_app();
    app.swarm_enabled = true;
    let self_id = app.session.id.clone();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let version_before = app.display_messages_version;

    // First snapshot: two of our workers running plus an unrelated agent.
    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::SwarmStatus {
            members: vec![
                member("ant", "running", Some(&self_id)),
                member("bat", "running", Some(&self_id)),
                member("stranger", "running", None),
            ],
        },
        &mut remote,
    );
    assert!(redraw, "swarm cards should redraw as soon as members arrive");
    assert_eq!(
        app.display_messages_version, version_before,
        "live swarm snapshots must not invalidate global transcript caches"
    );
    assert_eq!(app.remote_swarm_members.len(), 3);
    assert_eq!(
        app.status_notice(),
        None,
        "first snapshot has no transitions to announce"
    );

    // One of our workers finishes; the unrelated agent also finishes but must
    // not be announced (outside our spawn subtree).
    app.handle_server_event(
        crate::protocol::ServerEvent::SwarmStatus {
            members: vec![
                member("ant", "completed", Some(&self_id)),
                member("bat", "running", Some(&self_id)),
                member("stranger", "completed", None),
            ],
        },
        &mut remote,
    );
    assert_eq!(
        app.status_notice(),
        Some("🐝 ant done · 1/2 active".to_string())
    );
}
