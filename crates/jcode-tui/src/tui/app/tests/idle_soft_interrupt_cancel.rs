#[test]
fn idle_soft_interrupt_cancel_settles_pre_stream_turn_without_replaying_prompt() {
    use crate::protocol::ServerEvent;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for done_first in [false, true] {
            let mut app = create_test_app();
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            let identity = crate::protocol::TurnIdentity {
                runtime_generation: "idle-interrupt-runtime".into(),
                turn_id: 1,
            };
            let scoped = |event| ServerEvent::TurnEvent {
                identity: identity.clone(),
                event: Box::new(event),
            };
            app.pending_soft_interrupt_requests = vec![(42, "correction".into())];
            app.pending_soft_interrupts = vec!["correction".into()];
            app.handle_server_event(ServerEvent::Ack { id: 42 }, &mut remote);
            // Receipt alone must not discard a prompt still queued on server.
            assert!(app.pending_soft_interrupt_requests.is_empty());
            assert_eq!(app.pending_soft_interrupts, ["correction"]);
            app.handle_server_event(
                ServerEvent::SoftInterruptInjected {
                    content: "correction".into(),
                    display_role: None,
                    point: "turn_start".into(),
                    tools_skipped: None,
                    client_ids: vec![],
                },
                &mut remote,
            );
            app.handle_server_event(
                ServerEvent::TurnStarted {
                    identity: identity.clone(),
                },
                &mut remote,
            );
            app.handle_server_event(
                scoped(ServerEvent::ConnectionPhase {
                    phase: "waiting for response".into(),
                }),
                &mut remote,
            );
            assert!(app.is_processing);
            assert!(app.current_message_id.is_none());
            assert!(matches!(app.status, ProcessingStatus::Connecting(_)));
            // Legacy cancel traffic must not bypass the identity fence.
            app.handle_server_event(ServerEvent::Interrupted, &mut remote);
            assert!(app.is_processing);
            let terminals = if done_first {
                [ServerEvent::Done { id: 42 }, ServerEvent::Interrupted]
            } else {
                [ServerEvent::Interrupted, ServerEvent::Done { id: 42 }]
            };
            for event in terminals {
                app.handle_server_event(scoped(event), &mut remote);
                assert!(
                    !app.is_processing,
                    "first terminal must settle pre-stream work"
                );
                assert!(matches!(app.status, ProcessingStatus::Idle));
                assert!(app.processing_started.is_none());
            }
            assert!(app.pending_soft_interrupts.is_empty());
            assert!(!app.pending_queued_dispatch);
            rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
            assert!(!app.is_processing);
            assert!(app.queued_messages.is_empty());
            assert_eq!(
                app.display_messages()
                    .iter()
                    .filter(|m| m.content == "correction")
                    .count(),
                1
            );

            // Late background compaction is informational, not a new turn.
            let compaction = serde_json::from_value(serde_json::json!({
                "type": "compaction", "trigger": "reactive", "pre_tokens": 1000,
                "post_tokens": 100, "tokens_saved": 900, "duration_ms": 1000,
                "messages_dropped": null, "messages_compacted": 10,
                "summary_chars": 100, "active_messages": 2
            }))
            .unwrap();
            app.handle_server_event(compaction, &mut remote);
            assert!(!app.is_processing);
            assert!(app.processing_started.is_none());
            assert!(app.pending_soft_interrupts.is_empty());
        }
    });
}
