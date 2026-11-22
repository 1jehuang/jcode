fn reload_replay_history(
    session_id: &str,
    directive: crate::protocol::ReloadRecoverySnapshot,
) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::History {
        id: 1,
        session_id: session_id.to_string(),
        messages: vec![crate::protocol::HistoryMessage {
            response_stats: None,
            role: "assistant".to_string(),
            content: "Work interrupted by reload".to_string(),
            tool_calls: None,
            tool_data: None,
        }],
        messages_base_count: None,
        images_unchanged: false,
        images: vec![],
        provider_name: None,
        provider_model: None,
        subagent_model: None,
        agent_model_overrides: Default::default(),
        autoreview_enabled: None,
        autojudge_enabled: None,
        available_models: vec![],
        available_model_routes: vec![],
        mcp_servers: vec![],
        skills: vec![],
        total_tokens: None,
        token_usage_totals: None,
        all_sessions: vec![],
        client_count: None,
        is_canary: None,
        server_version: None,
        server_name: None,
        server_icon: None,
        server_has_update: None,
        was_interrupted: Some(true),
        reload_recovery: Some(directive),
        connection_type: None,
        status_detail: None,
        upstream_provider: None,
        provider_oauth_email: None,
        provider_oauth_account_label: None,
        resolved_credential: None,
        reasoning_effort: None,
        service_tier: None,
        available_service_tiers: None,
        compaction_mode: crate::config::CompactionMode::Reactive,
        activity: None,
        side_panel: Default::default(),
        applets: Default::default(),
    }
}

fn reload_replay_context(session_id: &str) -> ReloadContext {
    ReloadContext {
        task_context: None,
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: session_id.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

#[test]
fn reload_recovery_execution_retries_are_not_delivery_replays() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_remote = true;
    let mut connection = crate::tui::backend::RemoteConnection::dummy();
    connection.mark_history_loaded();
    let directive = ReloadContext::recovery_directive(
        Some(&reload_replay_context("reload_retry")),
        true,
        "",
        None,
    )
    .unwrap();
    let original = directive.continuation_message;

    // Initial delivery must still exactly match the durable recovery record.
    rt.block_on(remote::begin_remote_send(
        &mut app,
        &mut connection,
        String::new(),
        vec![],
        true,
        Some(original.clone()),
        true,
        0,
    ))
    .unwrap();
    assert_eq!(
        app.rate_limit_pending_message
            .as_ref()
            .unwrap()
            .system_reminder
            .as_deref(),
        Some(original.as_str())
    );

    // Model a provider failure after acceptance and a confirmed fallback switch.
    // The server has already marked the original reload intent delivered.
    app.is_processing = false;
    app.pending_fallback_resend = Some(FallbackResendPayload {
        retry_attempts: 0,
        content: String::new(),
        images: vec![],
        is_system: true,
        auto_retry: true,
        system_reminder: Some(original.clone()),
        raw_input: None,
    });
    rt.block_on(remote::process_remote_followups(&mut app, &mut connection));
    assert!(app.is_processing);
    assert!(app.pending_fallback_resend.is_none());
    let pending = app.rate_limit_pending_message.as_ref().unwrap();
    assert!(
        pending.content.is_empty(),
        "do not echo another user prompt"
    );
    assert_eq!(pending.retry_attempts, 1);
    let retry = pending.system_reminder.clone().unwrap();
    assert_ne!(
        retry, original,
        "an exact replay is suppressed by the server"
    );
    assert!(retry.contains("Retry the interrupted execution"));
    assert!(
        retry.ends_with(&original),
        "preserve the recovery instructions"
    );

    // Same-route retries share the fix and must not grow the reminder each time.
    rt.block_on(remote::begin_remote_send(
        &mut app,
        &mut connection,
        String::new(),
        vec![],
        true,
        Some(retry.clone()),
        true,
        2,
    ))
    .unwrap();
    assert_eq!(
        app.rate_limit_pending_message
            .as_ref()
            .unwrap()
            .system_reminder
            .as_deref(),
        Some(retry.as_str())
    );
}

#[test]
fn reload_recovery_cloud_retry_marks_final_wire_reminder() {
    use tokio::io::AsyncBufReadExt;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for existing in [None, Some("Additional recovery instructions".to_string())] {
        let mut app = create_test_app();
        app.is_remote = true;
        let session = super::commands::active_session_id(&app);
        let original = ReloadContext::recovery_directive(
            Some(&reload_replay_context(&session)),
            true,
            "",
            None,
        )
        .unwrap()
        .continuation_message;
        app.cloud_recovery_context = Some((session, original.clone()));
        app.cloud_recovery_auto_pending = true;
        let mut connection = crate::tui::backend::RemoteConnection::dummy();
        let mut peer = tokio::io::BufReader::new(connection.take_dummy_peer().unwrap());
        rt.block_on(async {
            remote::begin_remote_send(
                &mut app,
                &mut connection,
                String::new(),
                vec![],
                true,
                existing.clone(),
                true,
                1,
            )
            .await
            .unwrap();
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(1), peer.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            let wire: serde_json::Value = serde_json::from_str(&line).unwrap();
            let reminder = wire["system_reminder"].as_str().unwrap();
            assert!(reminder.starts_with("<system-reminder>Retry the interrupted execution"));
            assert!(reminder.ends_with(&original));
            if let Some(existing) = existing.as_ref() {
                assert!(reminder.contains(existing));
            }
            assert!(app.cloud_recovery_context.is_none());
            assert!(!app.cloud_recovery_auto_pending);
            let pending = app.rate_limit_pending_message.clone().unwrap();
            assert_eq!(pending.system_reminder.as_deref(), Some(reminder));

            // A manual fallback can reset the retry budget. That must not
            // strip execution identity or add another wrapper on the wire.
            remote::begin_remote_send(
                &mut app,
                &mut connection,
                pending.content,
                pending.images,
                pending.is_system,
                pending.system_reminder,
                pending.auto_retry,
                0,
            )
            .await
            .unwrap();
            let mut retry_line = String::new();
            tokio::time::timeout(Duration::from_secs(1), peer.read_line(&mut retry_line))
                .await
                .unwrap()
                .unwrap();
            let retry_wire: serde_json::Value = serde_json::from_str(&retry_line).unwrap();
            assert_eq!(retry_wire["system_reminder"], wire["system_reminder"]);
        });
    }
}

#[test]
fn reload_recovery_same_route_tick_marks_execution_retry() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_remote = true;
    app.auto_poke_incomplete_todos = false;
    let mut connection = crate::tui::backend::RemoteConnection::dummy();
    connection.mark_history_loaded();
    let original = ReloadContext::recovery_directive(
        Some(&reload_replay_context("reload_same_route_retry")),
        true,
        "",
        None,
    )
    .unwrap()
    .continuation_message;
    rt.block_on(remote::begin_remote_send(
        &mut app,
        &mut connection,
        String::new(),
        vec![],
        true,
        Some(original.clone()),
        true,
        0,
    ))
    .unwrap();
    app.is_processing = false;
    assert!(app.schedule_pending_remote_retry("provider interrupted"));
    app.rate_limit_reset = Some(Instant::now() - Duration::from_secs(1));
    rt.block_on(remote::handle_tick(&mut app, &mut connection));
    let pending = app.rate_limit_pending_message.as_ref().unwrap();
    assert!(app.is_processing);
    assert_eq!(pending.retry_attempts, 1);
    let reminder = pending.system_reminder.as_deref().unwrap();
    assert!(reminder.starts_with("<system-reminder>Retry the interrupted execution"));
    assert!(reminder.ends_with(&original));
}

#[test]
fn reload_recovery_cloud_only_retry_marks_final_reminder_once() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_remote = true;
    let session = super::commands::active_session_id(&app);
    app.cloud_recovery_context = Some((session, "Recover the cloud environment".to_string()));
    let mut connection = crate::tui::backend::RemoteConnection::dummy();
    rt.block_on(remote::begin_remote_send(
        &mut app,
        &mut connection,
        String::new(),
        vec![],
        true,
        None,
        true,
        1,
    ))
    .unwrap();
    let reminder = app
        .rate_limit_pending_message
        .as_ref()
        .unwrap()
        .system_reminder
        .clone()
        .unwrap();
    assert!(reminder.starts_with("<system-reminder>Retry the interrupted execution"));
    assert!(reminder.ends_with("Recover the cloud environment"));
    assert!(app.cloud_recovery_context.is_none());
    rt.block_on(remote::begin_remote_send(
        &mut app,
        &mut connection,
        String::new(),
        vec![],
        true,
        Some(reminder.clone()),
        true,
        2,
    ))
    .unwrap();
    assert_eq!(
        app.rate_limit_pending_message
            .as_ref()
            .unwrap()
            .system_reminder
            .as_deref(),
        Some(reminder.as_str())
    );
}

#[test]
fn reload_recovery_prepared_send_failure_restores_draft_without_exiting() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_remote = true;
    let mut connection = crate::tui::backend::RemoteConnection::dummy();
    connection.mark_history_loaded();
    drop(connection.take_dummy_peer());
    let images = vec![("image/png".to_string(), "preserved-image".to_string())];
    let result = rt.block_on(remote::submit_prepared_remote_input(
        &mut app,
        &mut connection,
        input::PreparedInput {
            raw_input: "Continue the interrupted work".to_string(),
            expanded: "Expanded instructions for the interrupted work".to_string(),
            images: images.clone(),
        },
    ));
    assert!(
        result.is_ok(),
        "recoverable transport failures must not escape the TUI loop"
    );
    assert!(!app.is_processing);
    assert!(app.current_message_id.is_none());
    assert_eq!(app.input, "Continue the interrupted work [image 1]");
    assert_eq!(app.cursor_pos, app.input.len());
    assert_eq!(app.pending_images, images);
    assert!(
        !app.display_messages()
            .iter()
            .any(|message| message.role == "user")
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.content.contains("Failed to send message:"))
    );
    assert_eq!(
        app.last_submitted_input.as_deref(),
        Some("Continue the interrupted work")
    );
}

#[test]
fn reload_recovery_keyboard_send_failure_keeps_client_and_draft_alive() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "Keep this prompt".to_string();
    app.cursor_pos = app.input.len();
    let images = vec![("image/png".to_string(), "preserved-image".to_string())];
    app.pending_images = images.clone();
    let mut connection = crate::tui::backend::RemoteConnection::dummy();
    connection.mark_history_loaded();
    drop(connection.take_dummy_peer());
    let result =
        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut connection));
    assert!(result.is_ok());
    assert_eq!(app.input, "Keep this prompt [image 1]");
    assert_eq!(app.pending_images, images);
    assert!(!app.is_processing);
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.content.contains("Failed to send message:"))
    );
}

#[test]
fn reload_recovery_deferred_send_failure_preserves_newer_draft() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for after_model_switch in [false, true] {
        let mut app = create_test_app();
        app.is_remote = true;
        app.auto_poke_incomplete_todos = false;
        let mut connection = crate::tui::backend::RemoteConnection::dummy();
        app.remote_model_switch_in_flight = after_model_switch;
        let held_image = ("image/png".to_string(), "held-image".to_string());
        rt.block_on(remote::submit_prepared_remote_input(
            &mut app,
            &mut connection,
            input::PreparedInput {
                raw_input: "Held prompt [image 1]".to_string(),
                expanded: "Expanded held prompt".to_string(),
                images: vec![held_image.clone()],
            },
        ))
        .unwrap();
        let new_image = ("image/png".to_string(), "new-image".to_string());
        app.input = "Newer unsent draft [image 1]".to_string();
        app.cursor_pos = app.input.len();
        app.pending_images = vec![new_image.clone()];
        connection.mark_history_loaded();
        app.remote_model_switch_in_flight = false;
        drop(connection.take_dummy_peer());
        rt.block_on(remote::process_remote_followups(&mut app, &mut connection));
        assert_eq!(app.input, "Newer unsent draft [image 1] [image 2]");
        assert_eq!(app.pending_images, vec![new_image, held_image]);
        assert!(!app.is_processing);
        assert!(app.pending_prompt_after_model_switch.is_none());
        assert!(app.pending_prompt_before_history.is_none());
        assert!(app.display_messages().iter().any(|message| {
            message.content.contains("Held prompt [image 1]")
                && message.content.contains("draft text is preserved")
        }));
        assert!(
            app.display_messages()
                .iter()
                .any(|message| message.content.contains("Failed to send message:"))
        );
    }
}

#[test]
fn reload_recovery_preserves_visible_turn_timer_through_dispatch() {
    let _env_guard = crate::storage::lock_test_env();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for has_visible_timer in [true, false] {
        let mut app = create_test_app();
        app.is_remote = true;
        app.auto_poke_incomplete_todos = false;
        app.is_processing = true;
        let started = Instant::now() - Duration::from_secs(90);
        app.processing_started = Some(started);
        app.visible_turn_started = has_visible_timer.then_some(started);
        let session_id = crate::id::new_id("reload_timer");
        app.remote_session_id = Some(session_id.clone());
        let ctx = reload_replay_context(&session_id);
        ctx.save().unwrap();
        remote::finalize_reload_reconnect(
            &mut app,
            Some(&session_id),
            remote::ReloadReconnectHints {
                reload_ctx_for_session: Some(ctx),
                has_client_reload_marker: true,
            },
            true,
        );
        assert!(!app.is_processing);
        assert!(app.processing_started.is_none());
        assert_eq!(app.visible_turn_started, Some(started));
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
        assert!(app.is_processing);
        assert!(app.current_message_id.is_some());
        assert_eq!(app.visible_turn_started, Some(started));
        assert!(app.elapsed().unwrap() >= Duration::from_secs(90));
    }
}

#[test]
fn reload_recovery_delayed_history_after_dispatch_does_not_send_again() {
    let _env_guard = crate::storage::lock_test_env();
    let mut app = create_test_app();
    app.is_remote = true;
    app.auto_poke_incomplete_todos = false;
    let session_id = crate::id::new_id("reload_delayed_history");
    app.remote_session_id = Some(session_id.clone());
    let ctx = reload_replay_context(&session_id);
    ctx.save().unwrap();
    remote::finalize_reload_reconnect(
        &mut app,
        Some(&session_id),
        remote::ReloadReconnectHints {
            reload_ctx_for_session: Some(ctx.clone()),
            has_client_reload_marker: true,
        },
        false,
    );
    assert_eq!(app.hidden_queued_system_messages.len(), 1);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(app.current_message_id.is_some());
    let after_first_send = remote.next_request_id_for_test();

    // The server assembled this snapshot before accepting the first request.
    // It arrives after the local queue and retry slot have both been consumed.
    let directive = ReloadContext::recovery_directive(Some(&ctx), true, "", None).unwrap();
    app.handle_server_event(reload_replay_history(&session_id, directive), &mut remote);
    assert!(app.hidden_queued_system_messages.is_empty());
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert_eq!(remote.next_request_id_for_test(), after_first_send);
}

#[test]
fn reload_recovery_delayed_history_after_completion_does_not_restart_turn() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.auto_poke_incomplete_todos = false;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let session_id = "reload_finished_history";
    let directive =
        ReloadContext::recovery_directive(Some(&reload_replay_context(session_id)), true, "", None)
            .unwrap();
    let history = reload_replay_history(session_id, directive);
    app.handle_server_event(history.clone(), &mut remote);
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    let after_first_send = remote.next_request_id_for_test();
    assert!(app.hidden_queued_system_messages.is_empty());
    let id = app.current_message_id.unwrap();
    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "Task complete.".to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::MessageEnd { stop_reason: None },
        &mut remote,
    );
    app.handle_server_event(crate::protocol::ServerEvent::Done { id }, &mut remote);
    app.handle_server_event(history, &mut remote);
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.hidden_queued_system_messages.is_empty());
    assert_eq!(remote.next_request_id_for_test(), after_first_send);
}

#[test]
fn reload_recovery_history_then_local_context_preserves_active_turn() {
    let _env_guard = crate::storage::lock_test_env();
    let mut app = create_test_app();
    let session_id = crate::id::new_id("reload_history_first");
    let ctx = reload_replay_context(&session_id);
    ctx.save().unwrap();
    let directive =
        ReloadContext::recovery_directive(Some(&ctx), true, "different background note", None)
            .unwrap();
    assert!(remote::queue_reload_continuation_once(
        &mut app,
        &session_id,
        directive,
        "continue"
    ));
    app.hidden_queued_system_messages.clear();
    app.is_processing = true;
    // A resumed turn can be active without a locally assigned message id.
    app.current_message_id = None;
    remote::finalize_reload_reconnect(
        &mut app,
        Some(&session_id),
        remote::ReloadReconnectHints {
            reload_ctx_for_session: Some(ctx),
            has_client_reload_marker: true,
        },
        false,
    );
    assert!(app.is_processing);
    assert!(app.hidden_queued_system_messages.is_empty());
}

#[test]
fn reload_recovery_dedup_is_session_scoped_and_resets_for_new_reload() {
    let mut app = create_test_app();
    let directive = ReloadContext::recovery_directive(
        Some(&reload_replay_context("session-a")),
        true,
        "",
        None,
    )
    .unwrap();
    assert!(remote::queue_reload_continuation_once(
        &mut app,
        "session-a",
        directive.clone(),
        "continue"
    ));
    app.hidden_queued_system_messages.clear();
    assert!(!remote::queue_reload_continuation_once(
        &mut app,
        "session-a",
        directive.clone(),
        "continue"
    ));
    assert!(remote::queue_reload_continuation_once(
        &mut app,
        "session-b",
        directive.clone(),
        "continue"
    ));
    app.hidden_queued_system_messages.clear();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.handle_server_event(
        crate::protocol::ServerEvent::Reloading { new_socket: None },
        &mut remote,
    );
    assert!(remote::queue_reload_continuation_once(
        &mut app,
        "session-a",
        directive,
        "continue"
    ));
}
