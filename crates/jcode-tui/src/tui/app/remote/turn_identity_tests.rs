use super::*;
use crate::tui::app::tests::create_test_app;
use crate::tui::backend::RemoteConnection;

fn identity(generation: &str, turn_id: u64) -> crate::protocol::TurnIdentity {
    crate::protocol::TurnIdentity {
        runtime_generation: generation.into(),
        turn_id,
    }
}
fn start(app: &mut App, remote: &mut RemoteConnection, generation: &str, turn_id: u64) {
    handle_server_event(
        app,
        ServerEvent::TurnStarted {
            identity: identity(generation, turn_id),
        },
        remote,
    );
}
fn scoped(generation: &str, turn_id: u64, event: ServerEvent) -> ServerEvent {
    ServerEvent::TurnEvent {
        identity: identity(generation, turn_id),
        event: Box::new(event),
    }
}

#[test]
fn completed_turn_busy_continuation_retries_from_idle_without_another_terminal() {
    for auto_retry in [false, true] {
        let mut app = create_test_app();
        app.is_remote = true;
        app.auto_poke_incomplete_todos = false;
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let mut remote = RemoteConnection::dummy();
        remote.mark_history_loaded();
        start(&mut app, &mut remote, "runtime", 1);
        handle_server_event(
            &mut app,
            scoped("runtime", 1, ServerEvent::Done { id: 1 }),
            &mut remote,
        );
        assert!(!app.is_processing);
        runtime
            .block_on(crate::tui::app::remote::begin_remote_send(
                &mut app,
                &mut remote,
                String::new(),
                vec![],
                true,
                Some("Continue after finalization".into()),
                auto_retry,
                0,
            ))
            .unwrap();
        let rejected = app.current_message_id.unwrap();
        handle_server_event(
            &mut app,
            ServerEvent::Error {
                id: rejected,
                message: "Already processing a message".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert!(
            !app.is_processing,
            "completed turn must not be resurrected awaiting another terminal"
        );
        assert!(app.current_message_id.is_none());
        assert!(app.remote_resume_activity.is_none());
        assert!(app.rate_limit_pending_message.is_none());
        assert_eq!(
            app.hidden_queued_system_messages,
            ["Continue after finalization"]
        );
        assert!(app.remote_queue_retry_not_before.is_some());
        app.remote_queue_retry_not_before = Some(Instant::now() + Duration::from_secs(3600));
        runtime.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        runtime.block_on(crate::tui::app::remote::handle_tick(&mut app, &mut remote));
        assert!(
            !app.is_processing,
            "cooldown prevents tight busy-rejection retries"
        );
        assert_eq!(
            app.hidden_queued_system_messages,
            ["Continue after finalization"]
        );
        app.remote_queue_retry_not_before = Some(Instant::now() - Duration::from_millis(1));
        runtime.block_on(crate::tui::app::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(
            app.is_processing,
            "idle queue dispatch must retry without another old-turn event"
        );
        assert!(app.hidden_queued_system_messages.is_empty());
        assert_eq!(
            app.rate_limit_pending_message
                .as_ref()
                .unwrap()
                .system_reminder
                .as_deref(),
            Some("Continue after finalization")
        );
    }
}

#[test]
fn completed_turn_busy_user_send_restores_prompt_without_overwriting_new_draft() {
    for draft in ["", "new unsent draft"] {
        let mut app = create_test_app();
        app.is_remote = true;
        app.auto_poke_incomplete_todos = false;
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let mut remote = RemoteConnection::dummy();
        remote.mark_history_loaded();
        start(&mut app, &mut remote, "runtime", 1);
        handle_server_event(
            &mut app,
            scoped("runtime", 1, ServerEvent::Done { id: 1 }),
            &mut remote,
        );
        runtime
            .block_on(crate::tui::app::remote::begin_remote_send(
                &mut app,
                &mut remote,
                "rejected user prompt".into(),
                vec![],
                false,
                None,
                false,
                0,
            ))
            .unwrap();
        app.last_submitted_input = Some("rejected user prompt".into());
        app.input = draft.into();
        let rejected = app.current_message_id.unwrap();
        handle_server_event(
            &mut app,
            ServerEvent::Error {
                id: rejected,
                message: "Already processing a message".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert!(!app.is_processing);
        assert!(app.current_message_id.is_none());
        assert!(app.remote_resume_activity.is_none());
        assert_eq!(
            app.input,
            if draft.is_empty() {
                "rejected user prompt"
            } else {
                draft
            }
        );
        assert!(app.hidden_queued_system_messages.is_empty());
    }
}

#[test]
fn idle_bootstrap_settles_observer_after_disconnected_completion() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    start(&mut app, &mut remote, "runtime", 1);
    handle_server_event(
        &mut app,
        ServerEvent::TurnState {
            session_id: "session".into(),
            runtime_generation: "runtime".into(),
            latest_turn_id: 1,
            turn_id: None,
        },
        &mut remote,
    );
    assert!(!app.is_processing);
    assert!(app.remote_turn_fence.active.is_none());
    handle_server_event(
        &mut app,
        scoped("runtime", 1, ServerEvent::Done { id: 1 }),
        &mut remote,
    );
    assert!(!app.is_processing);
}

#[test]
fn local_send_rejection_preserves_running_observer_and_restores_prompt() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    start(&mut app, &mut remote, "runtime", 1);
    app.replace_streaming_text("external partial".into());
    app.current_message_id = Some(99);
    app.last_submitted_input = Some("rejected local prompt".into());
    app.status = ProcessingStatus::Sending;
    let activity = app.last_stream_activity;
    handle_server_event(
        &mut app,
        ServerEvent::Error {
            id: 99,
            message: "Already processing a message".into(),
            retry_after_secs: None,
        },
        &mut remote,
    );
    assert!(app.is_processing);
    assert_eq!(app.current_message_id, None);
    assert_eq!(app.streaming_text(), "external partial");
    assert_eq!(app.input, "rejected local prompt");
    assert_eq!(app.last_stream_activity, activity);
    assert_eq!(
        app.remote_turn_fence.active.as_ref(),
        Some(&identity("runtime", 1))
    );
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("Already processing")
    );
    handle_server_event(
        &mut app,
        scoped("runtime", 1, ServerEvent::Done { id: 1 }),
        &mut remote,
    );
    assert!(!app.is_processing);
}

#[test]
fn local_preacceptance_error_and_control_done_settle_without_active_turn() {
    for event in [
        ServerEvent::Error {
            id: 99,
            message: "Authentication failed".into(),
            retry_after_secs: None,
        },
        ServerEvent::Done { id: 99 },
    ] {
        let mut app = create_test_app();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let mut remote = RemoteConnection::dummy();
        handle_server_event(
            &mut app,
            ServerEvent::TurnState {
                session_id: "session".into(),
                runtime_generation: "runtime".into(),
                latest_turn_id: 0,
                turn_id: None,
            },
            &mut remote,
        );
        app.current_message_id = Some(99);
        app.is_processing = true;
        app.status = ProcessingStatus::Sending;
        handle_server_event(&mut app, event, &mut remote);
        assert!(!app.is_processing);
        assert_eq!(app.current_message_id, None);
    }
}

#[test]
fn paced_done_keeps_its_identity_across_reconnect_and_cannot_finish_new_turn() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    start(&mut app, &mut remote, "runtime", 1);
    handle_server_event(
        &mut app,
        scoped(
            "runtime",
            1,
            ServerEvent::TextDelta {
                text: "buffered response".into(),
            },
        ),
        &mut remote,
    );
    handle_server_event(
        &mut app,
        scoped("runtime", 1, ServerEvent::Done { id: 42 }),
        &mut remote,
    );
    assert_eq!(app.deferred_stream_done_id, Some(42));
    handle_server_event(
        &mut app,
        ServerEvent::TurnState {
            session_id: "session".into(),
            runtime_generation: "runtime".into(),
            latest_turn_id: 1,
            turn_id: None,
        },
        &mut remote,
    );
    assert_eq!(app.deferred_stream_done_id, Some(42));
    let ops = app.stream_buffer.flush();
    app.apply_stream_ops(ops);
    let id = app.deferred_stream_done_id.take().unwrap();
    let event = scoped("runtime", 1, ServerEvent::Done { id });
    handle_server_event(&mut app, event.clone(), &mut remote);
    assert!(!app.is_processing);
    start(&mut app, &mut remote, "runtime", 2);
    handle_server_event(&mut app, event, &mut remote);
    assert!(app.is_processing);
}

#[test]
fn explicit_session_switch_can_revisit_previous_runtime() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    for (session, generation) in [
        ("one", "runtime-one"),
        ("two", "runtime-two"),
        ("one", "runtime-one"),
    ] {
        handle_server_event(
            &mut app,
            ServerEvent::TurnState {
                session_id: session.into(),
                runtime_generation: generation.into(),
                latest_turn_id: 1,
                turn_id: Some(1),
            },
            &mut remote,
        );
        assert_eq!(
            app.remote_turn_fence.active.as_ref(),
            Some(&identity(generation, 1))
        );
    }
}

#[test]
fn old_completion_and_error_cannot_idle_newer_external_turn() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    start(&mut app, &mut remote, "runtime", 1);
    start(&mut app, &mut remote, "runtime", 2);
    for event in [
        ServerEvent::Done { id: 10 },
        ServerEvent::Error {
            id: 10,
            message: "old".into(),
            retry_after_secs: None,
        },
    ] {
        assert!(!handle_server_event(
            &mut app,
            scoped("runtime", 1, event),
            &mut remote
        ));
        assert!(app.is_processing);
    }
    // Request IDs are intentionally unrelated to the authoritative turn ID.
    handle_server_event(
        &mut app,
        scoped("runtime", 2, ServerEvent::Done { id: 999 }),
        &mut remote,
    );
    assert!(!app.is_processing);
}

#[test]
fn old_runtime_start_output_and_terminal_rejected_after_bootstrap() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    start(&mut app, &mut remote, "old", 100);
    handle_server_event(
        &mut app,
        ServerEvent::TurnState {
            session_id: "session".into(),
            runtime_generation: "new".into(),
            latest_turn_id: 1,
            turn_id: Some(1),
        },
        &mut remote,
    );
    start(&mut app, &mut remote, "new", 1);
    start(&mut app, &mut remote, "old", 101);
    handle_server_event(
        &mut app,
        scoped(
            "old",
            100,
            ServerEvent::TextDelta {
                text: "stale".into(),
            },
        ),
        &mut remote,
    );
    handle_server_event(
        &mut app,
        scoped("old", 100, ServerEvent::Done { id: 1 }),
        &mut remote,
    );
    assert!(app.is_processing);
    assert!(app.streaming_text().is_empty());
    assert_eq!(
        app.remote_turn_fence.active.as_ref(),
        Some(&identity("new", 1))
    );
}

#[test]
fn reconnect_same_turn_accepts_current_terminal_but_idle_snapshot_blocks_replay() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    start(&mut app, &mut remote, "runtime", 7);
    for _ in 0..2 {
        handle_server_event(
            &mut app,
            ServerEvent::TurnState {
                session_id: "session".into(),
                runtime_generation: "runtime".into(),
                latest_turn_id: 7,
                turn_id: Some(7),
            },
            &mut remote,
        );
    }
    handle_server_event(
        &mut app,
        scoped("runtime", 7, ServerEvent::Done { id: 3 }),
        &mut remote,
    );
    assert!(!app.is_processing);
    handle_server_event(
        &mut app,
        ServerEvent::TurnState {
            session_id: "session".into(),
            runtime_generation: "runtime".into(),
            latest_turn_id: 7,
            turn_id: Some(7),
        },
        &mut remote,
    );
    start(&mut app, &mut remote, "runtime", 7);
    assert!(!app.is_processing);
    assert!(app.remote_turn_fence.active.is_none());
}

#[test]
fn legacy_observer_stream_and_done_still_work() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    handle_server_event(
        &mut app,
        ServerEvent::TextDelta {
            text: "legacy".into(),
        },
        &mut remote,
    );
    assert!(app.is_processing);
    let ops = app.stream_buffer.flush();
    app.apply_stream_ops(ops);
    handle_server_event(&mut app, ServerEvent::Done { id: 42 }, &mut remote);
    assert!(!app.is_processing);
}

#[test]
fn unscoped_terminal_cannot_idle_scoped_observer() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = RemoteConnection::dummy();
    start(&mut app, &mut remote, "runtime", 1);
    handle_server_event(&mut app, ServerEvent::Done { id: 1 }, &mut remote);
    assert!(app.is_processing);
}
