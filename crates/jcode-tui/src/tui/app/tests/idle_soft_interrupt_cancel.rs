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

/// A soft interrupt sent just as the turn ended reaches an idle server, which
/// starts it as its own turn. The client must not also requeue and resend it.
#[test]
fn stranded_soft_interrupt_recovery_requeues_only_what_server_still_queued() {
    use crate::protocol::ServerEvent;
    use crate::tui::backend::soft_interrupt_client_id;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for server_accepted in [true, false] {
            let mut app = create_test_app();
            app.is_remote = true;
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            app.track_pending_soft_interrupt(42, "next prompt".into());
            app.handle_server_event(ServerEvent::Ack { id: 42 }, &mut remote);

            rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
            let cancel_id = app
                .soft_interrupt_recovery
                .expect("recovery must ask the server before requeueing")
                .request_id()
                .unwrap();
            assert!(app.queued_messages.is_empty());
            assert!(!app.is_processing, "nothing may be sent while waiting");
            assert_eq!(app.pending_soft_interrupts, ["next prompt"]);

            let client_id = soft_interrupt_client_id(42);
            let (cancelled, not_queued) = if server_accepted {
                (vec![], vec![client_id])
            } else {
                (vec![client_id], vec![])
            };
            if server_accepted {
                app.handle_server_event(
                    ServerEvent::SoftInterruptInjected {
                        content: "next prompt".into(),
                        display_role: None,
                        point: "turn_start".into(),
                        tools_skipped: None,
                        client_ids: vec![],
                    },
                    &mut remote,
                );
            }
            app.handle_server_event(
                ServerEvent::SoftInterruptsCancelled {
                    id: cancel_id,
                    cancelled,
                    not_queued,
                },
                &mut remote,
            );
            assert!(app.soft_interrupt_recovery.is_none());
            assert!(app.pending_soft_interrupts.is_empty());
            assert!(app.sent_soft_interrupts.is_empty());
            if server_accepted {
                assert!(
                    app.queued_messages.is_empty(),
                    "an accepted interrupt must not be resent"
                );
                let shown = app
                    .display_messages
                    .iter()
                    .filter(|message| message.content == "next prompt")
                    .count();
                assert_eq!(shown, 1);
            } else {
                assert_eq!(app.queued_messages, ["next prompt"]);
            }
        }
    });
}

#[test]
fn stranded_soft_interrupt_not_acked_and_not_queued_is_requeued() {
    use crate::protocol::ServerEvent;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        app.track_pending_soft_interrupt(7, "lost prompt".into());
        rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
        let cancel_id = app.soft_interrupt_recovery.unwrap().request_id().unwrap();
        app.handle_server_event(
            ServerEvent::SoftInterruptsCancelled {
                id: cancel_id,
                cancelled: vec![],
                not_queued: vec![crate::tui::backend::soft_interrupt_client_id(7)],
            },
            &mut remote,
        );
        assert_eq!(app.queued_messages, ["lost prompt"]);
        assert!(app.pending_soft_interrupts.is_empty());
        assert!(app.pending_soft_interrupt_requests.is_empty());
    });
}

#[test]
fn soft_interrupt_clear_ack_fallback_retains_payload_until_confirmed() {
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        app.track_pending_soft_interrupt(41, "original".into());
        app.track_pending_soft_interrupt(42, "correction".into());
        app.queued_messages.push("later request".into());
        app.soft_interrupt_recovery = Some(remote::queue_recovery::SoftInterruptRecovery::ById(
            999,
            Instant::now() - Duration::from_secs(60),
        ));

        rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
        assert_eq!(app.pending_soft_interrupts, ["original", "correction"]);
        assert_eq!(app.pending_soft_interrupt_requests.len(), 2);
        assert_eq!(app.sent_soft_interrupts.len(), 2);
        assert_eq!(app.queued_messages, ["later request"]);
        assert!(!app.is_processing);
        let clear_id = app.soft_interrupt_recovery.unwrap().request_id().unwrap();
        for id in [999, u64::MAX] {
            app.handle_server_event(crate::protocol::ServerEvent::Ack { id }, &mut remote);
        }
        app.handle_server_event(
            crate::protocol::ServerEvent::SoftInterruptsCancelled {
                id: clear_id,
                cancelled: vec![crate::tui::backend::soft_interrupt_client_id(41)],
                not_queued: vec![],
            },
            &mut remote,
        );
        assert_eq!(app.pending_soft_interrupts, ["original", "correction"]);
        app.soft_interrupt_recovery =
            Some(remote::queue_recovery::SoftInterruptRecovery::ClearAll(
                clear_id,
                Instant::now() - Duration::from_secs(60),
            ));
        rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
        let retry_id = app.soft_interrupt_recovery.unwrap().request_id().unwrap();
        assert_ne!(retry_id, clear_id);
        app.handle_server_event(
            crate::protocol::ServerEvent::Ack { id: clear_id },
            &mut remote,
        );
        assert_eq!(app.pending_soft_interrupts, ["original", "correction"]);
        assert_eq!(app.queued_messages, ["later request"]);
        for _ in 0..2 {
            app.handle_server_event(
                crate::protocol::ServerEvent::Ack { id: retry_id },
                &mut remote,
            );
            assert_eq!(
                app.queued_messages,
                ["original", "correction", "later request"]
            );
            assert!(app.pending_soft_interrupts.is_empty());
            assert!(app.pending_soft_interrupt_requests.is_empty());
            assert!(app.sent_soft_interrupts.is_empty());
            assert!(app.soft_interrupt_recovery.is_none());
        }
    });
}

#[test]
fn soft_interrupt_clear_ack_errors_retain_payload_and_reject_late_ack() {
    use crate::protocol::ServerEvent;
    use remote::queue_recovery::SoftInterruptRecovery;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut app = create_test_app();
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        app.track_pending_soft_interrupt(41, "same prompt".into());
        app.track_pending_soft_interrupt(42, "same prompt".into());
        app.queued_messages.push("later request".into());
        rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
        let by_id = app.soft_interrupt_recovery.unwrap().request_id().unwrap();
        app.handle_server_event(ServerEvent::Ack { id: by_id }, &mut remote);
        assert_eq!(
            app.pending_soft_interrupts.len(),
            2,
            "by-id Ack is not a verdict"
        );
        // Recovery errors are control traffic even if another turn has since started.
        app.is_processing = true;
        app.current_message_id = Some(777);
        app.handle_server_event(
            ServerEvent::Error {
                id: by_id,
                message: "store inaccessible".into(),
                retry_after_secs: None,
            },
            &mut remote,
        );
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(777));
        assert!(matches!(
            app.soft_interrupt_recovery,
            Some(SoftInterruptRecovery::RetryClear(_))
        ));
        app.is_processing = false;
        app.current_message_id = None;
        for _ in 0..2 {
            app.soft_interrupt_recovery = Some(SoftInterruptRecovery::RetryClear(
                Instant::now() - Duration::from_secs(60),
            ));
            rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
            let clear_id = app.soft_interrupt_recovery.unwrap().request_id().unwrap();
            app.handle_server_event(
                ServerEvent::Error {
                    id: clear_id,
                    message: "clear still failed".into(),
                    retry_after_secs: None,
                },
                &mut remote,
            );
            app.handle_server_event(ServerEvent::Ack { id: clear_id }, &mut remote);
            rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
            assert_eq!(app.pending_soft_interrupts, ["same prompt", "same prompt"]);
            assert_eq!(app.pending_soft_interrupt_requests.len(), 2);
            assert_eq!(app.sent_soft_interrupts.len(), 2);
            assert_eq!(app.queued_messages, ["later request"]);
            assert!(!app.is_processing);
            assert!(matches!(
                app.soft_interrupt_recovery,
                Some(SoftInterruptRecovery::RetryClear(_))
            ));
        }
    });
}

#[test]
fn soft_interrupt_clear_ack_unnamed_snapshot_and_failed_write_preserve_pending() {
    use remote::queue_recovery::SoftInterruptRecovery;
    with_temp_jcode_home(|| {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        for fail_write in [false, true] {
            let mut app = create_test_app();
            app.is_remote = true;
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            remote.mark_history_loaded();
            // Reloaded snapshots have no request IDs to cancel individually.
            app.pending_soft_interrupts.push("restored prompt".into());
            app.queued_messages.push("later request".into());
            if fail_write {
                drop(remote.take_dummy_peer().unwrap());
            }
            rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
            assert_eq!(app.pending_soft_interrupts, ["restored prompt"]);
            assert_eq!(app.queued_messages, ["later request"]);
            assert!(!app.is_processing);
            if fail_write {
                assert!(matches!(
                    app.soft_interrupt_recovery,
                    Some(SoftInterruptRecovery::RetryClear(_))
                ));
                rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
                assert_eq!(app.pending_soft_interrupts, ["restored prompt"]);
            } else {
                let clear_id = app.soft_interrupt_recovery.unwrap().request_id().unwrap();
                app.handle_server_event(
                    crate::protocol::ServerEvent::Ack { id: clear_id },
                    &mut remote,
                );
                assert_eq!(app.queued_messages, ["restored prompt", "later request"]);
            }
        }
    });
}

async fn process_followups_with_clear_ack(
    app: &mut App,
    remote: &mut crate::tui::backend::RemoteConnection,
) {
    remote::process_remote_followups(app, remote).await;
    assert!(!app.pending_soft_interrupts.is_empty());
    assert!(
        !app.is_processing,
        "the clear write must not authorize replay"
    );
    let clear_id = app.soft_interrupt_recovery.unwrap().request_id().unwrap();
    app.handle_server_event(crate::protocol::ServerEvent::Ack { id: clear_id }, remote);
    remote::process_remote_followups(app, remote).await;
}
