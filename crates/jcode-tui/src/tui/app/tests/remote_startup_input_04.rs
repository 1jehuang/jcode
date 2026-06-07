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
