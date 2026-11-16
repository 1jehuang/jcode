use super::*;
use crate::tui::app::tests::create_test_app;
use crate::tui::backend::RemoteConnection;

#[test]
fn stale_terminals_cannot_settle_or_touch_newer_work() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    app.is_processing = true;
    app.current_message_id = Some(99);
    app.status = ProcessingStatus::Sending;
    app.input = "unsent draft".into();
    app.replace_streaming_text("partial response".into());
    let activity = app.last_stream_activity;
    for event in [
        ServerEvent::Done { id: 98 },
        ServerEvent::Error {
            id: 98,
            message: "late failure".into(),
            retry_after_secs: None,
        },
        ServerEvent::Done { id: 100 },
    ] {
        assert!(!handle_server_event(&mut app, event, &mut remote));
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(99));
        assert!(matches!(app.status, ProcessingStatus::Sending));
        assert_eq!(app.input, "unsent draft");
        assert_eq!(app.streaming_text(), "partial response");
        assert_eq!(app.last_stream_activity, activity);
    }
    // Positive control: current terminal still settles the active turn.
    handle_server_event(&mut app, ServerEvent::Done { id: 99 }, &mut remote);
    assert!(!app.is_processing);
    assert_eq!(app.current_message_id, None);
}

#[test]
fn runtime_command_error_preserves_active_turn_and_clears_request_ownership() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut remote = RemoteConnection::dummy();
        let id = remote
            .runtime_command("config", vec!["invalid".into()])
            .await
            .unwrap();
        app.is_processing = true;
        app.pending_turn = true;
        app.current_message_id = Some(99);
        app.status = ProcessingStatus::Sending;
        app.input = "unsent draft".into();
        app.replace_streaming_text("partial response".into());
        let activity = app.last_stream_activity;
        assert!(handle_server_event(
            &mut app,
            ServerEvent::Error {
                id,
                message: "Session is busy. Retry this command after the current turn.".into(),
                retry_after_secs: None,
            },
            &mut remote
        ));
        assert!(app.is_processing);
        assert!(app.pending_turn);
        assert_eq!(app.current_message_id, Some(99));
        assert!(matches!(app.status, ProcessingStatus::Sending));
        assert_eq!(app.input, "unsent draft");
        assert_eq!(app.streaming_text(), "partial response");
        assert_eq!(app.last_stream_activity, activity);
        assert_eq!(app.display_messages.last().unwrap().role, "error");
        assert!(!remote.finish_runtime_command(id));

        let id = remote
            .runtime_command("poke", vec!["status".into()])
            .await
            .unwrap();
        assert!(handle_server_event(
            &mut app,
            ServerEvent::RuntimeCommandResult {
                id,
                command: "poke".into(),
                data: serde_json::json!({}),
                message: "Stopped".into(),
            },
            &mut remote
        ));
        assert!(!remote.finish_runtime_command(id));
        assert!(app.is_processing);
    });
}

#[test]
fn remote_poke_response_synchronizes_toggle_without_disrupting_turn() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _runtime_guard = runtime.enter();
    let mut app = create_test_app();
    let mut remote = RemoteConnection::dummy();
    app.is_processing = true;
    app.current_message_id = Some(99);
    app.input = "unsent draft".into();
    for enabled in [false, true, false] {
        assert!(handle_server_event(
            &mut app,
            ServerEvent::RuntimeCommandResult {
                id: 42,
                command: "poke".into(),
                data: serde_json::json!({"poke_enabled": enabled, "server_owned": true}),
                message: "Remote poke updated".into(),
            },
            &mut remote,
        ));
        assert_eq!(app.auto_poke_incomplete_todos, enabled);
        assert_eq!(app.auto_poke_default_on, enabled);
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(99));
        assert_eq!(app.input, "unsent draft");
        assert!(app.queued_messages.is_empty());
    }
}

#[test]
fn runtime_command_result_renders_without_starting_or_disrupting_turn() {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    for active in [false, true] {
        let mut app = create_test_app();
        let _runtime_guard = runtime.enter();
        let mut remote = RemoteConnection::dummy();
        app.input = "draft prompt".into();
        app.is_processing = active;
        app.status = if active {
            ProcessingStatus::Sending
        } else {
            ProcessingStatus::Idle
        };
        app.current_message_id = active.then_some(99);
        let activity = app.last_stream_activity;
        let count = app.display_messages.len();
        let event: ServerEvent = serde_json::from_value(serde_json::json!({
            "type": "runtime_command_result", "id": 42, "command": "agents",
            "data": {"enabled": true}, "message": "Agents enabled\nDaemon configuration updated."
        }))
        .unwrap();

        assert!(handle_server_event(&mut app, event, &mut remote));
        assert_eq!(app.display_messages.len(), count + 1);
        let notice = app.display_messages.last().unwrap();
        assert_eq!(notice.role, "system");
        assert_eq!(
            notice.content,
            "Agents enabled\nDaemon configuration updated."
        );
        assert_eq!(app.is_processing, active);
        assert_eq!(matches!(app.status, ProcessingStatus::Sending), active);
        assert_eq!(app.current_message_id, active.then_some(99));
        assert_eq!(app.last_stream_activity, activity);
        assert_eq!(app.input, "draft prompt");
    }
}
