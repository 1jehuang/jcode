#[test]
fn test_local_bus_dictation_completion_ignores_other_session() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some(session_id);

    let handled = crate::tui::app::local::handle_bus_event(
        &mut app,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_other".to_string(),
            session_id: Some("session_other".to_string()),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    );

    assert!(!handled);
    assert_eq!(app.input, "draft");
    assert!(app.dictation_in_flight);
}

#[test]
fn test_remote_bus_dictation_completion_ignores_other_session() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut remote = rt.block_on(async { crate::tui::backend::RemoteConnection::dummy() });
    app.is_remote = true;
    app.remote_session_id = Some("session_remote".to_string());
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some("session_remote".to_string());

    rt.block_on(crate::tui::app::remote::handle_bus_event(
        &mut app,
        &mut remote,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_other".to_string(),
            session_id: Some("session_other".to_string()),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    ));

    assert!(app.dictation_in_flight);
    assert_eq!(app.dictation_request_id.as_deref(), Some("dictation_123"));
}

#[test]
fn test_handle_server_event_transcript_send_prefixes_user_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Transcript {
            text: "dictated hello".to_string(),
            mode: crate::protocol::TranscriptMode::Send,
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("user message displayed");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "[transcription] dictated hello");
    assert!(app.messages.is_empty());
    assert!(matches!(
        app.session.messages.last().and_then(|message| message.content.last()),
        Some(crate::message::ContentBlock::Text { text, .. }) if text == "[transcription] dictated hello"
    ));
    assert!(
        app.pending_turn,
        "local transcript send should use normal submit path"
    );
}

#[test]
fn test_handle_server_event_session_close_requested_quits_client() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::SessionCloseRequested {
            reason: "Stopped by coordinator coord".to_string(),
        },
        &mut remote,
    );

    assert!(redraw);
    assert!(app.should_quit);
    let last = app
        .display_messages()
        .last()
        .expect("close message displayed");
    assert!(
        last.content
            .contains("Session close requested by coordinator")
    );
}

#[test]
fn test_handle_server_event_session_renamed_updates_remote_title() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_remote = true;
    app.remote_session_id = Some("session_remote_rename".to_string());
    app.session.title = Some("Generated title".to_string());

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::SessionRenamed {
            session_id: "session_remote_rename".to_string(),
            title: Some("Release planning".to_string()),
            display_title: "Release planning".to_string(),
        },
        &mut remote,
    );

    assert!(redraw);
    assert_eq!(
        app.session.custom_title.as_deref(),
        Some("Release planning")
    );
    assert_eq!(app.session.display_title(), Some("Release planning"));
    assert!(app.display_messages().iter().any(|message| {
        message
            .content
            .contains("Renamed session to **Release planning**")
    }));
}

#[test]
fn test_handle_server_event_history_clears_connection_type_on_session_change_when_missing() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_old".to_string());
    app.connection_type = Some("websocket".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_new".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_new"));
    assert_eq!(app.connection_type, None);
}

#[test]
fn test_handle_server_event_history_preserves_connection_type_for_same_session_when_missing() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_same".to_string());
    app.connection_type = Some("websocket".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_same".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_same"));
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
}

#[test]
fn test_handle_server_event_history_session_change_clears_pending_interleaves() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_old".to_string());
    app.queued_messages.push("queued later".to_string());
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(12, "acked interleave".to_string())];

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_new".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            reload_recovery: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_new"));
    assert!(app.queued_messages().is_empty());
    assert!(app.interleave_message.is_none());
    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
}

#[test]
fn test_handle_post_connect_marker_without_reload_context_does_not_queue_selfdev_continuation() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let session_id = "session_marker_only";
    let jcode_dir = crate::storage::jcode_dir().expect("jcode dir");
    std::fs::write(
        jcode_dir.join(format!("client-reload-pending-{}", session_id)),
        "Reloaded with build test123\n",
    )
    .expect("write client reload marker");

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    rt.block_on(super::remote::handle_post_connect(
        &mut app,
        &mut terminal,
        &mut remote,
        &mut state,
        Some(session_id),
    ))
    .expect("post connect should succeed");

    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.starts_with("Reload complete — continuing")),
        "marker-only reconnect should not queue selfdev continuation"
    );
    assert!(app.reload_info.is_empty());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.contains("✓ Reconnected successfully.")),
        "reconnect success message should still be shown"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_post_connect_defers_reload_followup_to_server_history_payload() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_hidden_reload_followup";
    crate::tool::selfdev::ReloadContext {
        task_context: Some("Investigate queued prompt delivery after reload".to_string()),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: session_id.to_string(),
        timestamp: "2026-03-26T00:00:00Z".to_string(),
    }
    .save()
    .expect("save reload context");

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some(session_id),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Ready));
    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.current_message_id.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.reload_info.is_empty());

    cleanup_reload_context_file(session_id);
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_post_connect_clears_deferred_dispatch_before_reload_followup() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_reload_deferred_dispatch";
    crate::tool::selfdev::ReloadContext {
        task_context: Some(
            "Verify deferred dispatch does not block reload continuation".to_string(),
        ),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: session_id.to_string(),
        timestamp: "2026-04-15T00:00:00Z".to_string(),
    }
    .save()
    .expect("save reload context");

    let mut app = create_test_app();
    app.pending_queued_dispatch = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some(session_id),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Ready));
    assert!(
        !app.pending_queued_dispatch,
        "post-connect should clear deferred dispatch before sending reload continuation"
    );
    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(
        app.is_processing,
        "reload continuation should still dispatch"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());

    cleanup_reload_context_file(session_id);
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_post_connect_requests_client_reload_after_server_reload_even_without_newer_binary() {
    use std::time::{Duration, SystemTime};

    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let mut app = create_test_app();
    app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.remote_session_id = Some("session_reload_after_reconnect".to_string());

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        server_reload_in_progress: true,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some("session_reload_after_reconnect"),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Quit));
    assert_eq!(
        app.reload_requested.as_deref(),
        Some("session_reload_after_reconnect")
    );
    assert!(app.should_quit);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_server_event_token_usage_uses_per_call_deltas() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming_tps_collect_output = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 10,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming_output_tokens, 30);
    assert_eq!(app.streaming_total_output_tokens, 30);
}

#[test]
fn test_handle_server_event_tool_start_pauses_tps_and_excludes_hidden_output_tokens() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming_tps_collect_output = true;
    app.streaming_tps_start = Some(Instant::now());

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );

    assert!(!app.streaming_tps_collect_output);
    assert!(app.streaming_tps_start.is_none());

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 25,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming_total_output_tokens, 0);

    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "hello".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming_tps_collect_output);
    assert!(app.streaming_tps_start.is_some());
}

#[test]
fn test_handle_server_event_message_end_marks_stream_as_finalizing_without_stall_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming_tps_collect_output = true;

    let needs_redraw =
        app.handle_server_event(crate::protocol::ServerEvent::MessageEnd, &mut remote);

    assert!(needs_redraw);
    assert!(app.stream_message_ended);
    assert!(matches!(app.status, ProcessingStatus::Streaming));
    assert!(app.streaming_tps_collect_output);
}

#[test]
fn test_handle_server_event_interrupted_clears_stream_state_and_sets_idle() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.processing_started = Some(Instant::now());
    app.current_message_id = Some(42);
    app.streaming_text = "partial".to_string();
    app.streaming_tool_calls.push(crate::message::ToolCall {
        id: "tool_1".to_string(),
        name: "bash".to_string(),
        input: serde_json::Value::Null,
        intent: None,
    });
    app.interleave_message = Some("queued interrupt".to_string());
    app.pending_soft_interrupts
        .push("pending soft interrupt".to_string());
    app.pending_soft_interrupt_requests
        .push((77, "pending soft interrupt".to_string()));

    remote.handle_tool_start("tool_1", "bash");
    remote.handle_tool_input("{\"command\":\"sleep 10\"}");
    remote.handle_tool_exec("tool_1", "edit");

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.current_message_id.is_none());
    assert!(app.streaming_text.is_empty());
    assert!(app.streaming_tool_calls.is_empty());
    assert!(app.interleave_message.is_none());
    assert_eq!(app.queued_messages(), &["queued interrupt"]);
    assert_eq!(app.pending_soft_interrupts, vec!["pending soft interrupt"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(77, "pending soft interrupt".to_string())]
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing interrupted message");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "Interrupted");
}

#[test]
fn test_remote_interrupted_defers_queued_followup_dispatch_by_one_cycle() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(app.pending_queued_dispatch);
    assert_eq!(app.queued_messages(), &["queued later"]);
    assert!(!app.is_processing);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert_eq!(app.queued_messages(), &["queued later"]);
    assert!(!app.is_processing);

    app.pending_queued_dispatch = false;
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());
}

#[test]
fn test_remote_interrupted_recovers_pending_interleaves_in_order() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(55, "acked interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(app.pending_queued_dispatch);
    assert_eq!(
        app.queued_messages(),
        &["unsent interleave", "queued later"]
    );
    assert_eq!(app.pending_soft_interrupts, vec!["acked interleave"]);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert_eq!(
        app.queued_messages(),
        &["acked interleave", "unsent interleave", "queued later"]
    );
    assert!(!app.is_processing);

    app.pending_queued_dispatch = false;
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));

    let user_messages: Vec<&str> = app
        .display_messages()
        .iter()
        .filter(|msg| msg.role == "user")
        .map(|msg| msg.content.as_str())
        .collect();
    assert_eq!(
        user_messages,
        vec!["acked interleave", "unsent interleave", "queued later"]
    );
}

#[test]
fn test_remote_done_recovers_stranded_soft_interrupt_as_queued_followup() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.pending_soft_interrupts = vec!["late interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(55, "late interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

    assert!(!app.is_processing);
    assert_eq!(app.pending_soft_interrupts, vec!["late interleave"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(55, "late interleave".to_string())]
    );
    assert_eq!(app.queued_messages(), &["queued later"]);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());

    let user_messages: Vec<&str> = app
        .display_messages()
        .iter()
        .filter(|msg| msg.role == "user")
        .map(|msg| msg.content.as_str())
        .collect();
    assert_eq!(user_messages, vec!["late interleave", "queued later"]);
}

#[test]
fn test_remote_done_auto_pokes_again_when_todos_remain() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Continue working".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.is_remote = true;
        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(42);

        let needs_redraw =
            app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

        assert!(needs_redraw);
        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("Continue working, or update the todo tool."));
    });
}
