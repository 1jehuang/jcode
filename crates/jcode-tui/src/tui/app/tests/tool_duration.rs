use super::*;

#[test]
fn native_search_duration_includes_execution_and_preserves_late_query() {
    use crate::protocol::ServerEvent;
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    for event in [
        ServerEvent::ToolStart {
            id: "ws_timed".into(),
            name: "web_search".into(),
        },
        ServerEvent::ToolInput {
            id: Some("ws_timed".into()),
            delta: r#"{"query":""}"#.into(),
        },
        ServerEvent::ToolExec {
            id: "ws_timed".into(),
            name: "web_search".into(),
        },
    ] {
        app.handle_server_event(event, &mut remote);
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    app.handle_server_event(
        ServerEvent::ToolInput {
            id: Some("ws_timed".into()),
            delta: r#"{"query":"jcode timing"}"#.into(),
        },
        &mut remote,
    );
    assert_eq!(remote.get_tool_input("ws_timed")["query"], "jcode timing");
    assert_eq!(app.streaming_tool_calls[0].input["query"], "jcode timing");
    app.handle_server_event(
        ServerEvent::ToolDone {
            id: "ws_timed".into(),
            name: "web_search".into(),
            output: "Searched: jcode timing".into(),
            error: None,
            duration_ms: None,
        },
        &mut remote,
    );
    let message = app.display_messages().last().unwrap();
    assert!(message.duration_secs.unwrap() >= 0.02);
    assert_eq!(
        message.tool_data.as_ref().unwrap().input["query"],
        "jcode timing"
    );
}

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

            
                duration_ms: None,
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
            duration_ms: Some(1_234),
            duration_ms: None,
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

            duration_ms: None,
        },
        &mut remote,
    );
    assert_eq!(app.display_messages().last().unwrap().duration_secs, None);
}
