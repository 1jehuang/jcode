// Held retries must remain dispatchable across reconnect and local send failures.
#[test]
fn recovery_log_snapshot_correlates_state_without_payloads() {
    let mut app = create_test_app();
    app.remote_session_id = Some("remote-diagnostic-session".into());
    app.current_message_id = Some(42);
    app.input = "private draft never log".into();
    app.queued_messages.push("private queued text".into());
    app.hidden_queued_system_messages
        .push("private hidden context".into());
    app.pending_images
        .push(("image/png".into(), "private draft image".into()));
    let mut pending = held_retry_message(2, true);
    pending.content = "private sent text".into();
    pending.system_reminder = Some("private reminder".into());
    pending.images[0].1 = "private sent image".into();
    app.rate_limit_pending_message = Some(pending);
    app.rate_limit_reset = Some(Instant::now());
    let fields = app.recovery_log_fields("test_reason");
    let get = |key| {
        fields
            .iter()
            .find(|(name, _)| *name == key)
            .unwrap()
            .1
            .as_str()
    };
    assert_eq!(get("session_id"), "remote-diagnostic-session");
    assert_eq!(get("request_id"), "42");
    assert_eq!(get("attempt"), "2");
    assert_eq!(get("image_count"), "1");
    assert_eq!(get("draft_image_count"), "1");
    assert_eq!(get("queued_count"), "1");
    assert_eq!(get("hidden_queue_count"), "1");
    assert_eq!(get("retry_armed"), "true");
    assert!(!format!("{fields:?}").contains("private"));

    app.remote_session_id = None;
    app.rate_limit_pending_message = None;
    app.current_message_id = None;
    let fallback = app.recovery_log_fields("no_pending");
    assert!(fallback.contains(&("session_id", app.session.id.clone())));
    assert!(fallback.contains(&("request_id", "none".into())));
    assert!(fallback.contains(&("pending", "false".into())));
}

fn held_retry_message(attempts: u8, auto_retry: bool) -> PendingRemoteMessage {
    PendingRemoteMessage {
        jarvis_voice: false,
        content: "finish the interrupted task [image 1]".into(),
        images: vec![("image/png".into(), "aW1hZ2U=".into())],
        is_system: false,
        system_reminder: None,
        auto_retry,
        retry_attempts: attempts,
        retry_at: None,
    }
}

#[test]
fn held_retry_waits_for_bootstrap_active_turn_and_route_confirmation() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for blocker in ["history", "active turn", "route switch"] {
        let mut app = create_test_app();
        app.is_remote = true;
        let mut connection = crate::tui::backend::RemoteConnection::dummy();
        if blocker != "history" {
            connection.mark_history_loaded();
        }
        app.is_processing = blocker == "active turn";
        app.remote_model_switch_in_flight = blocker == "route switch";
        let deadline = Instant::now() - Duration::from_secs(1);
        app.rate_limit_reset = Some(deadline);
        app.rate_limit_pending_message = Some(held_retry_message(1, true));
        let request_id = connection.next_request_id_for_test();
        assert!(!rt.block_on(remote::dispatch_due_remote_retry(&mut app, &mut connection)));
        assert_eq!(
            connection.next_request_id_for_test(),
            request_id,
            "{blocker}"
        );
        assert_eq!(app.rate_limit_reset, Some(deadline), "{blocker}");
        assert!(app.rate_limit_pending_message.is_some());

        connection.mark_history_loaded();
        app.is_processing = false;
        app.remote_model_switch_in_flight = false;
        assert!(rt.block_on(remote::dispatch_due_remote_retry(&mut app, &mut connection)));
        assert!(app.is_processing, "retry dispatches once {blocker} clears");
        assert!(app.rate_limit_reset.is_none());
        assert_eq!(connection.next_request_id_for_test(), request_id + 1);
    }
}

#[test]
fn held_retry_failed_write_rearms_bounded_retry_and_retains_payload() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for auto_retry in [false, true] {
        let mut app = create_test_app();
        app.is_remote = true;
        let mut connection = crate::tui::backend::RemoteConnection::dummy();
        connection.mark_history_loaded();
        drop(connection.take_dummy_peer());
        let original = held_retry_message(1, auto_retry);
        app.rate_limit_pending_message = Some(original.clone());
        app.rate_limit_reset = Some(Instant::now() - Duration::from_secs(1));
        assert!(rt.block_on(remote::dispatch_due_remote_retry(&mut app, &mut connection)));
        assert!(!app.is_processing);
        assert!(
            app.rate_limit_reset
                .is_some_and(|deadline| deadline > Instant::now())
        );
        let retry = app.rate_limit_pending_message.as_ref().unwrap();
        assert_eq!(retry.content, original.content);
        assert_eq!(retry.images, original.images);
        assert_eq!(retry.retry_attempts, 2);
        assert!(retry.auto_retry);
    }
}

#[test]
fn held_retry_failed_write_at_limit_restores_draft_and_images() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_remote = true;
    app.auto_poke_incomplete_todos = true;
    let mut connection = crate::tui::backend::RemoteConnection::dummy();
    connection.mark_history_loaded();
    drop(connection.take_dummy_peer());
    let original = held_retry_message(App::AUTO_RETRY_MAX_ATTEMPTS, true);
    app.last_submitted_input = Some(original.content.clone());
    app.rate_limit_pending_message = Some(original.clone());
    app.rate_limit_reset = Some(Instant::now() - Duration::from_secs(1));
    assert!(rt.block_on(remote::dispatch_due_remote_retry(&mut app, &mut connection)));
    assert!(!app.is_processing);
    assert!(app.rate_limit_reset.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert_eq!(app.input, original.content);
    assert_eq!(app.pending_images, original.images);
    assert!(!app.auto_poke_incomplete_todos);
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.content.contains("Automatic retry limit reached"))
    );
}

#[test]
fn remote_disconnect_releases_unconfirmed_switch_and_restores_waiting_prompt() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for automatic in [false, true] {
        for new_draft in ["", "keep my newer draft"] {
            let mut app = create_test_app();
            app.is_remote = true;
            app.auto_poke_incomplete_todos = false;
            app.remote_model_switch_in_flight = true;
            app.pending_automatic_failover_request_id = automatic.then_some(42);
            app.input = new_draft.to_string();
            let image = ("image/png".to_string(), "aW1hZ2U=".to_string());
            app.pending_prompt_after_model_switch = Some(input::PreparedInput {
                raw_input: "use the selected model [image 1]".to_string(),
                expanded: "expanded prompt for selected model".to_string(),
                images: vec![image.clone()],
            });
            let mut state = remote::RemoteRunState::default();
            remote::handle_disconnect(&mut app, &mut state, None);
            assert!(!app.remote_model_switch_in_flight);
            assert!(app.pending_prompt_after_model_switch.is_none());
            assert!(app.pending_automatic_failover_request_id.is_none());
            assert_eq!(app.pending_images, vec![image]);
            assert!(app.input.starts_with(if new_draft.is_empty() {
                "use the selected model"
            } else {
                new_draft
            }));
            assert!(app.input.contains("[image 1]"));
            app.sync_model_picker_preview_from_input();
            assert_eq!(app.pending_images.len(), 1);
            assert!(!app.pending_images[0].1.is_empty());
            let mut connection = crate::tui::backend::RemoteConnection::dummy();
            connection.mark_history_loaded();
            let before = connection.next_request_id_for_test();
            rt.block_on(remote::process_remote_followups(&mut app, &mut connection));
            assert_eq!(
                connection.next_request_id_for_test(),
                before,
                "never send the waiting prompt on an unconfirmed route"
            );
            assert!(!app.is_processing);
        }
    }
}
