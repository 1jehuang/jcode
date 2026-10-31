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
            timestamp: None,
            tool_duration_ms: None,
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
        crate::protocol::ServerEvent::MessageEnd {
            stop_reason: None,
        },
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
