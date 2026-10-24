fn completed_response_test_app() -> App {
    let mut app = create_test_app();
    app.is_remote = true;
    app.is_processing = true;
    app.current_message_id = Some(42);
    app.auto_poke_incomplete_todos = true;
    app.auto_poke_default_on = true;
    crate::todo::save_todos(
        &app.session.id,
        &[crate::todo::TodoItem {
            id: "done".into(),
            content: "Review the change".into(),
            status: "completed".into(),
            priority: "high".into(),
            confidence: Some(crate::todo::ConfidenceState::Validated),
            completion_confidence: Some(crate::todo::ConfidenceState::Validated),
            ..Default::default()
        }],
    )
    .unwrap();
    app
}

fn response_test_compaction() -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::Compaction {
        trigger: "automatic".into(),
        pre_tokens: Some(245_992),
        post_tokens: Some(33_478),
        tokens_saved: None,
        duration_ms: None,
        messages_dropped: None,
        messages_compacted: None,
        summary_chars: None,
        active_messages: None,
    }
}

fn finish_response_test_turn(app: &mut App, remote: &mut crate::tui::backend::RemoteConnection) {
    let ops = app.stream_buffer.flush();
    app.apply_stream_ops(ops);
    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, remote);
}

#[test]
fn final_response_followup_does_not_repeat_answer_across_compaction() {
    use crate::protocol::ServerEvent;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for compaction in [None, Some(false), Some(true)] {
            let mut app = completed_response_test_app();
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_server_event(
                ServerEvent::TextDelta {
                    text: "The review is complete.".into(),
                },
                &mut remote,
            );
            if compaction == Some(false) {
                app.handle_server_event(response_test_compaction(), &mut remote);
            }
            app.handle_server_event(
                ServerEvent::MessageEnd {
                    stop_reason: Some("end_turn".into()),
                },
                &mut remote,
            );
            if compaction == Some(true) {
                app.handle_server_event(response_test_compaction(), &mut remote);
            }
            finish_response_test_turn(&mut app, &mut remote);

            assert!(app.queued_messages.is_empty(), "compaction={compaction:?}");
            assert!(!app.pending_queued_dispatch);
            assert_eq!(
                app.display_messages()
                    .iter()
                    .filter(|message| message.role == "assistant"
                        && message.content == "The review is complete.")
                    .count(),
                1
            );
            assert!(app.todo_final_response_requested);
            assert!(app.final_response_todo_fingerprint.is_some());
            assert!(!app.schedule_auto_poke_followup_if_needed());

            // Reloading must not re-open the completed cycle either.
            app.save_input_for_reload(&app.session.id);
            let restored = App::restore_input_for_reload(&app.session.id).unwrap();
            let mut reloaded = create_test_app();
            reloaded.session.id = app.session.id.clone();
            reloaded.auto_poke_incomplete_todos = true;
            reloaded.auto_poke_default_on = true;
            reloaded.apply_restored_reload_input(restored);
            assert!(!reloaded.schedule_auto_poke_followup_if_needed());
        }
    });
}

#[test]
fn final_response_followup_still_requests_missing_or_incomplete_answer() {
    use crate::protocol::ServerEvent;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for events in [
            vec![ServerEvent::ReasoningDelta {
                text: "Need to wrap up".into(),
            }],
            vec![ServerEvent::TextDelta { text: " \n".into() }],
            vec![
                ServerEvent::TextDelta {
                    text: "I will check.".into(),
                },
                ServerEvent::ToolStart {
                    id: "call".into(),
                    name: "todo".into(),
                },
            ],
            vec![
                ServerEvent::ToolStart {
                    id: "call".into(),
                    name: "todo".into(),
                },
                ServerEvent::TextDelta {
                    text: "Tool-call commentary".into(),
                },
            ],
            vec![
                ServerEvent::TextDelta {
                    text: "Partial answer".into(),
                },
                ServerEvent::MessageEnd {
                    stop_reason: Some("max_tokens".into()),
                },
            ],
            vec![
                ServerEvent::TextDelta {
                    text: "Discard this answer".into(),
                },
                ServerEvent::RetryRollback { attempt: 1, max: 3 },
            ],
            vec![
                ServerEvent::TextDelta {
                    text: "Removed answer".into(),
                },
                ServerEvent::TextReplace {
                    text: String::new(),
                },
            ],
        ] {
            let mut app = completed_response_test_app();
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            let has_end = events
                .iter()
                .any(|event| matches!(event, ServerEvent::MessageEnd { .. }));
            for event in events {
                app.handle_server_event(event, &mut remote);
            }
            if !has_end {
                app.handle_server_event(ServerEvent::MessageEnd { stop_reason: None }, &mut remote);
            }
            finish_response_test_turn(&mut app, &mut remote);
            assert_eq!(
                app.queued_messages,
                [crate::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE]
            );
            assert!(app.pending_queued_dispatch);
        }
    });
}

#[test]
fn final_response_followup_tracks_the_last_request_not_earlier_commentary() {
    use crate::protocol::ServerEvent;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for final_answer in [false, true] {
            let mut app = completed_response_test_app();
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_server_event(
                ServerEvent::TextDelta {
                    text: "Checking now.".into(),
                },
                &mut remote,
            );
            app.handle_server_event(
                ServerEvent::ToolStart {
                    id: "call".into(),
                    name: "todo".into(),
                },
                &mut remote,
            );
            app.handle_server_event(
                ServerEvent::MessageEnd {
                    stop_reason: Some("tool_use".into()),
                },
                &mut remote,
            );
            app.begin_remote_kv_cache_request(App::kv_cache_request_signature(
                &[],
                &[],
                "system",
                "",
            ));
            if final_answer {
                app.handle_server_event(
                    ServerEvent::TextDelta {
                        text: "The review is complete.".into(),
                    },
                    &mut remote,
                );
            }
            app.handle_server_event(ServerEvent::MessageEnd { stop_reason: None }, &mut remote);
            finish_response_test_turn(&mut app, &mut remote);
            assert_eq!(app.queued_messages.is_empty(), final_answer);
        }
    });
}

#[test]
fn final_response_followup_preserves_incomplete_and_validation_gates() {
    use crate::protocol::ServerEvent;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for incomplete in [false, true] {
            let mut app = completed_response_test_app();
            let mut todos = crate::todo::load_todos(&app.session.id).unwrap();
            if incomplete {
                todos[0].status = "pending".into();
            } else {
                todos[0].completion_confidence = Some(crate::todo::ConfidenceState::Plausible);
            }
            crate::todo::save_todos(&app.session.id, &todos).unwrap();
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_server_event(
                ServerEvent::TextDelta {
                    text: "Here is the outcome.".into(),
                },
                &mut remote,
            );
            app.handle_server_event(ServerEvent::MessageEnd { stop_reason: None }, &mut remote);
            finish_response_test_turn(&mut app, &mut remote);
            assert_eq!(app.queued_messages.len(), 1);
            assert_ne!(
                app.queued_messages[0],
                crate::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE
            );
            assert!(app.pending_queued_dispatch);
        }
    });
}

#[test]
fn final_response_followup_local_finish_does_not_repeat_answer() {
    with_temp_jcode_home(|| {
        let mut app = completed_response_test_app();
        app.is_remote = false;
        app.streaming
            .response_output
            .text("The review is complete.");
        app.streaming.response_output.finish(Some("end_turn"));
        local::finish_turn(&mut app);
        assert!(app.queued_messages.is_empty());
        assert!(app.final_response_todo_fingerprint.is_some());
    });
}

#[test]
fn final_response_followup_does_not_reuse_an_interrupted_answer() {
    with_temp_jcode_home(|| {
        let mut app = completed_response_test_app();
        app.is_remote = false;
        app.streaming.response_output.text("Unfinished turn");
        app.streaming.response_output.finish(Some("end_turn"));
        app.schedule_queued_dispatch_after_interrupt();
        assert!(!app.streaming.response_output.has_final_response());
        local::finish_turn(&mut app);
        assert_eq!(
            app.queued_messages,
            [crate::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE]
        );
    });
}
