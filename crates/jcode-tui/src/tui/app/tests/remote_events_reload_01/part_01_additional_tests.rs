use super::*;

#[test]
fn test_local_bus_dictation_completion_ignores_other_session() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some(session_id);

    let handled = crate::tui::app::local::handle_bus_event(
        &mut app,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_other".to_string(),
            session_id: Some("session_other".to_string()),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    );

    assert!(!handled);
    assert_eq!(app.input, "draft");
    assert!(app.dictation_in_flight);
}

#[test]
fn test_remote_bus_dictation_completion_ignores_other_session() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut remote = rt.block_on(async { crate::tui::backend::RemoteConnection::dummy() });
    app.is_remote = true;
    app.remote_session_id = Some("session_remote".to_string());
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some("session_remote".to_string());

    rt.block_on(crate::tui::app::remote::handle_bus_event(
        &mut app,
        &mut remote,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_other".to_string(),
            session_id: Some("session_other".to_string()),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    ));

    assert!(app.dictation_in_flight);
    assert_eq!(app.dictation_request_id.as_deref(), Some("dictation_123"));
}
