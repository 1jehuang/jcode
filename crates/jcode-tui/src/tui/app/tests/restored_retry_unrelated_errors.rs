#[test]
fn test_restored_retry_failed_launch_after_completed_turn_returns_idle() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for transfer in [true, false] {
            for (input, output, cache_read_input, cache_creation_input) in [
                (100, 0, None, None),
                (0, 10, None, None),
                (0, 0, Some(0), None),
                (0, 0, None, Some(0)),
            ] {
                let session = "failed_launch_after_completed_turn";
                let mut app = create_test_app();
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                remote.set_session_id(session.into());
                remote.mark_history_loaded();
                let message_id = rt
                    .block_on(super::remote::begin_remote_send(
                        &mut app,
                        &mut remote,
                        "fresh prompt".into(),
                        vec![],
                        false,
                        None,
                        false,
                        0,
                    ))
                    .unwrap();
                app.handle_server_event(
                    crate::protocol::ServerEvent::TokenUsage {
                        input,
                        output,
                        cache_read_input,
                        cache_creation_input,
                    },
                    &mut remote,
                );
                app.handle_server_event(
                    crate::protocol::ServerEvent::Done { id: message_id },
                    &mut remote,
                );
                assert!(!app.is_processing);
                assert!(
                    app.has_streaming_footer_stats(),
                    "completed statistics remain visible"
                );
                app.restored_retries.push(PendingRemoteMessage {
                    content: "saved retry".into(),
                    images: vec![("image/png".into(), "payload".into())],
                    is_system: true,
                    system_reminder: Some("saved reminder".into()),
                    auto_retry: true,
                    retry_attempts: 2,
                    retry_at: None,
                });
                app.save_input_for_reload(session);
                app.pending_transfer_request = transfer;
                app.pending_split_request = !transfer;
                let launch_id = remote.next_request_id_for_test();
                rt.block_on(super::remote::process_remote_followups(
                    &mut app,
                    &mut remote,
                ));
                assert!(app.is_processing);
                assert!(matches!(app.status, ProcessingStatus::Sending));
                assert!(app.has_streaming_footer_stats());
                app.handle_server_event(
                    crate::protocol::ServerEvent::Done { id: message_id },
                    &mut remote,
                );
                assert!(
                    app.is_processing,
                    "a stale completion must not settle a new launch"
                );
                assert!(matches!(app.status, ProcessingStatus::Sending));
                // Neither a stale local error nor a generic server error is
                // an active turn merely because its old statistics remain.
                for id in [message_id, 0] {
                    app.handle_server_event(
                        crate::protocol::ServerEvent::Error {
                            id,
                            message: "stale error".into(),
                            retry_after_secs: None,
                        },
                        &mut remote,
                    );
                    assert!(app.is_processing);
                    assert!(matches!(app.status, ProcessingStatus::Sending));
                    assert!(!app.restored_retry_stopped);
                }
                let error = if transfer {
                    "Failed to compact session for transfer"
                } else {
                    "Failed to split session"
                };
                app.handle_server_event(
                    crate::protocol::ServerEvent::Error {
                        id: launch_id,
                        message: error.into(),
                        retry_after_secs: None,
                    },
                    &mut remote,
                );
                assert!(
                    !app.is_processing,
                    "matching failed launch must settle despite retained statistics"
                );
                assert!(matches!(app.status, ProcessingStatus::Idle));
                assert!(app.current_message_id.is_none());
                assert!(app.processing_started.is_none());
                assert!(app.last_stream_activity.is_none());
                assert!(
                    app.has_streaming_footer_stats(),
                    "launch failure must preserve completed statistics"
                );
                assert!(!app.restored_retry_stopped);
                assert!(
                    app.display_messages()
                        .iter()
                        .any(|message| message.content == error)
                );
                let reopened = App::new_for_remote(Some(session.into()));
                assert!(!reopened.restored_retry_stopped);
                assert_eq!(reopened.restored_retries.len(), 1);
                rt.block_on(super::remote::process_remote_followups(
                    &mut app,
                    &mut remote,
                ));
                let retry_id = app
                    .current_message_id
                    .expect("saved retry should dispatch after failed launch");
                assert_ne!(retry_id, launch_id);
                assert_eq!(
                    app.restored_retry_delivery.as_ref().unwrap().request_id,
                    retry_id
                );
                let pending = app.rate_limit_pending_message.as_ref().unwrap();
                assert_eq!(pending.retry_attempts, 2);
                assert_eq!(pending.images, vec![("image/png".into(), "payload".into())]);
                assert_eq!(pending.system_reminder.as_deref(), Some("saved reminder"));
                app.handle_server_event(
                    crate::protocol::ServerEvent::Done { id: retry_id },
                    &mut remote,
                );
                assert!(
                    App::new_for_remote(Some(session.into()))
                        .restored_retries
                        .is_empty()
                );
            }
        }
    });
}

#[test]
fn test_restored_retry_unrelated_transfer_error_preserves_waiting_queue() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let session = "retry_transfer_error";
        let mut app = create_test_app();
        app.restored_retries.push(PendingRemoteMessage {
            content: "saved retry".into(),
            images: vec![("image/png".into(), "payload".into())],
            is_system: true,
            system_reminder: Some("saved reminder".into()),
            auto_retry: true,
            retry_attempts: 2,
            retry_at: None,
        });
        app.save_input_for_reload(session);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id(session.into());
        remote.mark_history_loaded();

        app.pending_transfer_request = true;
        let transfer_id = remote.next_request_id_for_test();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        assert!(app.current_message_id.is_none());
        assert!(app.restored_retry_delivery.is_none());
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: transfer_id,
                message: "Failed to load session for transfer: test error".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );

        assert!(!app.restored_retry_stopped);
        assert_eq!(app.restored_retries.len(), 1);
        let reopened = App::new_for_remote(Some(session.into()));
        assert!(!reopened.restored_retry_stopped);
        assert_eq!(reopened.restored_retries.len(), 1);
        assert!(app.display_messages().iter().any(|message| {
            message
                .content
                .contains("Failed to load session for transfer")
        }));
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let retry_id = app
            .current_message_id
            .expect("saved retry should dispatch without new input");
        assert_ne!(retry_id, transfer_id);
        let pending = app.rate_limit_pending_message.as_ref().unwrap();
        assert_eq!(pending.retry_attempts, 2);
        assert_eq!(pending.images, vec![("image/png".into(), "payload".into())]);
        assert_eq!(pending.system_reminder.as_deref(), Some("saved reminder"));
        app.handle_server_event(
            crate::protocol::ServerEvent::Done { id: retry_id },
            &mut remote,
        );
        assert!(
            App::new_for_remote(Some(session.into()))
                .restored_retries
                .is_empty()
        );
    });
}

#[test]
fn test_restored_retry_unrelated_errors_preserve_active_message() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for owns_saved_retry in [false, true] {
            let session = format!("retry_unrelated_active_{owns_saved_retry}");
            let mut app = create_test_app();
            app.restored_retries.push(PendingRemoteMessage {
                content: "saved retry".into(),
                images: vec![],
                is_system: true,
                system_reminder: None,
                auto_retry: true,
                retry_attempts: 2,
                retry_at: None,
            });
            app.save_input_for_reload(&session);
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.set_session_id(session.clone());
            remote.mark_history_loaded();
            let stale_id = rt.block_on(remote.transfer()).unwrap();
            // Settle the launch so its later error is stale as well as unrelated.
            use crate::tui::backend::RemoteEventState;
            assert!(remote.finish_session_launch(stale_id));
            if owns_saved_retry {
                rt.block_on(super::remote::process_remote_followups(
                    &mut app,
                    &mut remote,
                ));
            } else {
                rt.block_on(super::remote::begin_remote_send(
                    &mut app,
                    &mut remote,
                    "fresh prompt".into(),
                    vec![],
                    false,
                    None,
                    false,
                    0,
                ))
                .unwrap();
            }
            let message_id = app.current_message_id.unwrap();
            app.handle_server_event(
                crate::protocol::ServerEvent::TextDelta {
                    text: "partial answer".into(),
                },
                &mut remote,
            );
            let stream_before = app.streaming.streaming_text.clone();
            let control_id = rt.block_on(remote.transfer()).unwrap();
            for (id, message, retry_after_secs) in [
                (
                    control_id,
                    "Failed to load session for transfer: test error",
                    None,
                ),
                (stale_id, "credential failure circuit breaker tripped", None),
                (stale_id, "rate limited", Some(60)),
                (
                    message_id + 100,
                    "Provider fallback available: test error",
                    None,
                ),
            ] {
                app.handle_server_event(
                    crate::protocol::ServerEvent::Error {
                        id,
                        message: message.into(),
                        retry_after_secs,
                    },
                    &mut remote,
                );
                assert_eq!(app.current_message_id, Some(message_id));
                assert!(app.is_processing);
                assert!(!app.restored_retry_stopped);
                assert_eq!(app.restored_retries.len(), 1);
                assert_eq!(app.pending_remote_is_restored_retry, owns_saved_retry);
                assert_eq!(
                    app.restored_retry_delivery
                        .as_ref()
                        .map(|delivery| delivery.request_id),
                    owns_saved_retry.then_some(message_id)
                );
                assert_eq!(app.streaming.streaming_text, stream_before);
                let pending = app.rate_limit_pending_message.as_ref().unwrap();
                assert_eq!(pending.retry_attempts, if owns_saved_retry { 2 } else { 0 });
                assert!(app.rate_limit_reset.is_none());
                assert!(!App::new_for_remote(Some(session.clone())).restored_retry_stopped);
            }
            app.handle_server_event(
                crate::protocol::ServerEvent::Done { id: message_id },
                &mut remote,
            );
            assert_eq!(app.restored_retries.len(), usize::from(!owns_saved_retry));
        }
    });
}

#[test]
fn test_restored_retry_unrelated_errors_preserve_attached_turn() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        // Server turns use id 0; turns from another client keep that client's id.
        for (terminal_id, event, succeeded) in [
            (
                0,
                crate::protocol::ServerEvent::TextDelta {
                    text: "attached turn".into(),
                },
                false,
            ),
            (
                999,
                crate::protocol::ServerEvent::TextDelta {
                    text: "attached turn".into(),
                },
                false,
            ),
            (
                0,
                crate::protocol::ServerEvent::ConnectionPhase {
                    phase: "connecting".into(),
                },
                false,
            ),
            (
                999,
                crate::protocol::ServerEvent::StatusDetail {
                    detail: "starting attached turn".into(),
                },
                false,
            ),
            (
                999,
                crate::protocol::ServerEvent::StatusDetail {
                    detail: "starting attached turn".into(),
                },
                true,
            ),
        ] {
            let session = format!("retry_attached_error_{terminal_id}");
            let mut app = create_test_app();
            app.remote_session_id = Some(session.clone());
            app.restored_retries.push(PendingRemoteMessage {
                content: "saved retry".into(),
                images: vec![],
                is_system: true,
                system_reminder: None,
                auto_retry: true,
                retry_attempts: 0,
                retry_at: None,
            });
            app.save_input_for_reload(&session);
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.set_session_id(session.clone());
            remote.mark_history_loaded();
            app.pending_transfer_request = true;
            let transfer_id = remote.next_request_id_for_test();
            rt.block_on(super::remote::process_remote_followups(
                &mut app,
                &mut remote,
            ));
            // A remote turn starts while the transfer response is in flight.
            app.handle_server_event(event, &mut remote);
            assert!(app.current_message_id.is_none());
            assert!(app.is_processing);
            let stream_before = app.streaming.streaming_text.clone();
            app.handle_server_event(
                crate::protocol::ServerEvent::Error {
                    id: transfer_id,
                    message: "Failed to compact session for transfer".into(),
                    retry_after_secs: None,
                },
                &mut remote,
            );
            assert!(app.is_processing);
            assert!(!app.restored_retry_stopped);
            assert_eq!(app.streaming.streaming_text, stream_before);
            assert!(!App::new_for_remote(Some(session.clone())).restored_retry_stopped);
            let terminal = if succeeded {
                crate::protocol::ServerEvent::Done { id: terminal_id }
            } else {
                crate::protocol::ServerEvent::Error {
                    id: terminal_id,
                    message: "provider failed hard".into(),
                    retry_after_secs: None,
                }
            };
            app.handle_server_event(terminal, &mut remote);
            assert!(!app.is_processing);
            assert_eq!(
                app.restored_retry_stopped, !succeeded,
                "only actual attached-turn failure should stop saved retries"
            );
            assert_eq!(
                App::new_for_remote(Some(session)).restored_retry_stopped,
                !succeeded
            );
        }
    });
}

#[test]
fn test_restored_retry_stale_error_during_backoff_preserves_deadline() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let session = "retry_stale_error_backoff";
        let mut app = create_test_app();
        app.restored_retries.push(PendingRemoteMessage {
            content: "saved retry".into(),
            images: vec![],
            is_system: true,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: 0,
            retry_at: None,
        });
        app.save_input_for_reload(session);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.set_session_id(session.into());
        remote.mark_history_loaded();
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));
        let message_id = app.current_message_id.unwrap();
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: message_id,
                message: "rate limited".into(),
                retry_after_secs: Some(60),
            },
            &mut remote,
        );
        let deadline = app.rate_limit_reset.expect("current error schedules retry");
        assert!(app.current_message_id.is_none());
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: message_id,
                message: "provider failed hard".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert_eq!(app.rate_limit_reset, Some(deadline));
        assert!(app.rate_limit_pending_message.is_some());
        assert!(!app.restored_retry_stopped);
        assert_eq!(app.restored_retries.len(), 1);
        app.save_input_for_reload(session);
        let saved = App::restore_input_for_reload(session).unwrap();
        assert_eq!(saved.restored_retries.len(), 1);
        assert!(saved.restored_retries[0].retry_at.unwrap() > Instant::now());
        assert!(!saved.restored_retry_stopped);
    });
}

#[test]
fn test_remote_non_retryable_error_gets_short_auto_poke_retry() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.auto_poke_incomplete_todos = true;
    app.queued_messages
        .push("You have 1 incomplete todo. Continue working, or update the todo tool.".to_string());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "You have 1 incomplete todo. Continue working, or update the todo tool."
            .to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.current_message_id = Some(12);
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 12,
            message: "OpenAI API error 400 Bad Request: {\"error\":{\"message\":\"Invalid 'input[0].encrypted_content': string too long. Expected a string with maximum length 10485760, but got a string with length 11237432 instead.\",\"type\":\"invalid_request_error\",\"code\":\"string_above_max_length\"}}".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.auto_poke_incomplete_todos);
    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("deterministic error should get a short retry budget");
    assert_eq!(pending.retry_attempts, 1);
    assert!(app.rate_limit_reset.is_some());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("attempt 1/2"))
    );

    // Retry backoff must dispatch another Message before its error can
    // advance the retry budget; an arbitrary second Error is unrelated.
    app.rate_limit_reset = Some(Instant::now());
    rt.block_on(super::remote::handle_tick(&mut app, &mut remote));
    let retry_id = app.current_message_id.expect("retry should be in flight");
    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: retry_id,
            message: "OpenAI API error 400 Bad Request: {\"error\":{\"type\":\"invalid_request_error\",\"code\":\"string_above_max_length\"}}".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.auto_poke_incomplete_todos);
    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("second deterministic error should still get final retry");
    assert_eq!(pending.retry_attempts, 2);
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("attempt 2/2"))
    );
}
