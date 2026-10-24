use super::*;

#[test]
fn tool_duration_remote_completion_and_local_update() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    for (id, output, expected) in [
        ("header", "[tool timing: duration=2.4s] done", Some(2.4)),
        ("unknown", "done", None),
    ] {
        app.handle_server_event(
            crate::protocol::ServerEvent::ToolDone {
                id: id.into(),
                name: "bash".into(),
                output: output.into(),
                error: None,
            },
            &mut remote,
        );
        assert_eq!(
            app.display_messages().last().unwrap().duration_secs,
            expected
        );
    }
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolExec {
            id: "observed".into(),
            name: "bash".into(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolDone {
            id: "observed".into(),
            name: "bash".into(),
            output: "failed".into(),
            error: Some("failed".into()),
        },
        &mut remote,
    );
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .duration_secs
            .is_some()
    );
    app.set_latest_tool_duration("observed", 1.234);
    app.replace_latest_tool_display_message("observed", None, "updated".into());
    assert_eq!(
        app.display_messages().last().unwrap().duration_secs,
        Some(1.234)
    );
}

#[test]
fn tool_duration_replay_does_not_measure_playback_clock() {
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::ReplayRemoteState::default();
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolExec {
            id: "replay".into(),
            name: "bash".into(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolDone {
            id: "replay".into(),
            name: "bash".into(),
            output: "done".into(),
            error: None,
        },
        &mut remote,
    );
    assert_eq!(app.display_messages().last().unwrap().duration_secs, None);
}
