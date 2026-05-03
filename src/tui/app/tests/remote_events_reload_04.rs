#[test]
fn test_remote_error_without_retry_recovers_pending_followups() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(10);
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(88, "acked interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 10,
            message: "provider failed hard".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.interleave_message.is_none());
    assert_eq!(
        app.queued_messages(),
        &["unsent interleave", "queued later"]
    );
    assert_eq!(app.pending_soft_interrupts, vec!["acked interleave"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(88, "acked interleave".to_string())]
    );

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));

    let last = app
        .display_messages()
        .last()
        .expect("missing error message");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "queued later");
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "error" && m.content == "provider failed hard")
    );
}

#[test]
fn test_remote_error_with_retryable_pending_schedules_retry() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 11,
            message: "provider failed hard".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("retryable continuation should remain pending");
    assert!(pending.auto_retry);
    assert_eq!(pending.retry_attempts, 1);
    assert!(pending.retry_at.is_some());
    assert!(app.rate_limit_reset.is_some());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Auto-retrying"))
    );
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

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 13,
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

#[test]
fn test_remote_non_retryable_error_stops_auto_poke_after_short_retry_budget() {
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
        retry_attempts: 2,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 14,
            message: "OpenAI API error 400 Bad Request: {\"error\":{\"type\":\"invalid_request_error\",\"code\":\"string_above_max_length\"}}".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(!app.auto_poke_incomplete_todos);
    assert!(app.queued_messages().is_empty());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.rate_limit_reset.is_none());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Auto-poke stopped"))
    );
}

#[test]
fn test_schedule_pending_remote_retry_respects_retry_limit() {
    let mut app = create_test_app();
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: App::AUTO_RETRY_MAX_ATTEMPTS,
        retry_at: None,
    });

    assert!(!app.schedule_pending_remote_retry("⚠ failed."));
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.rate_limit_reset.is_none());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "error" && m.content.contains("Auto-retry limit reached"))
    );
}

#[test]
fn test_info_widget_data_includes_connection_type() {
    let mut app = create_test_app();
    app.connection_type = Some("https".to_string());
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(data.connection_type.as_deref(), Some("https"));
}

#[test]
fn test_remote_tui_state_prefers_cached_model_during_brief_connecting_phase() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_123";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.save().expect("save remote session");

    let app = App::new_for_remote(Some(session_id.to_string()));

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");
    assert_eq!(
        crate::tui::TuiState::session_display_name(&app).as_deref(),
        Some("otter")
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_remote_tui_state_falls_back_to_cached_model_after_startup_phase_clears() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_124";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.save().expect("save remote session");

    let mut app = App::new_for_remote(Some(session_id.to_string()));
    app.clear_remote_startup_phase();

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_new_for_remote_uses_startup_stub_without_loading_full_transcript() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_stub_125";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg-startup-stub".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "hello from persisted history".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save remote session");

    let app = App::new_for_remote(Some(session_id.to_string()));
    assert_eq!(app.session_id(), session_id);
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(
        app.display_messages()[0].content,
        "hello from persisted history"
    );
    assert_eq!(app.session.messages.len(), 1);
    assert_eq!(app.remote_session_id.as_deref(), Some(session_id));
    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_remote_tui_state_shows_connected_after_startup_phase_clears_without_model() {
    let mut app = App::new_for_remote(None);
    app.remote_session_id = Some("session_connected_123".to_string());
    app.clear_remote_startup_phase();

    assert_eq!(crate::tui::TuiState::provider_model(&app), "connected");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "");
}

#[test]
fn test_remote_tui_state_hides_brief_connecting_phase_without_cached_model() {
    let _guard = crate::storage::lock_test_env();
    let prev_model = std::env::var_os("JCODE_MODEL");
    let prev_provider = std::env::var_os("JCODE_PROVIDER");
    crate::env::set_var("JCODE_MODEL", "unknown");
    crate::env::remove_var("JCODE_PROVIDER");

    let app = App::new_for_remote(None);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server…"
    );
    assert_eq!(crate::tui::TuiState::provider_name(&app), "");

    if let Some(prev_model) = prev_model {
        crate::env::set_var("JCODE_MODEL", prev_model);
    } else {
        crate::env::remove_var("JCODE_MODEL");
    }
    if let Some(prev_provider) = prev_provider {
        crate::env::set_var("JCODE_PROVIDER", prev_provider);
    } else {
        crate::env::remove_var("JCODE_PROVIDER");
    }
}

#[test]
fn test_remote_tui_state_prefers_configured_model_during_brief_connecting_phase() {
    let _guard = crate::storage::lock_test_env();
    let prev_model = std::env::var_os("JCODE_MODEL");
    let prev_provider = std::env::var_os("JCODE_PROVIDER");
    crate::env::set_var("JCODE_MODEL", "gpt-5.4");
    crate::env::set_var("JCODE_PROVIDER", "openai");

    let app = App::new_for_remote(None);

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");

    if let Some(prev_model) = prev_model {
        crate::env::set_var("JCODE_MODEL", prev_model);
    } else {
        crate::env::remove_var("JCODE_MODEL");
    }
    if let Some(prev_provider) = prev_provider {
        crate::env::set_var("JCODE_PROVIDER", prev_provider);
    } else {
        crate::env::remove_var("JCODE_PROVIDER");
    }
}

#[test]
fn test_remote_tui_state_shows_starting_server_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_server_spawning();

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "starting server…"
    );
}

#[test]
fn test_remote_tui_state_shows_loading_session_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "loading session…"
    );
}

#[test]
fn test_remote_tui_state_shows_startup_elapsed_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Connecting);
    app.remote_startup_phase_started =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(5));

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server… 5s"
    );
}

#[test]
fn test_remote_startup_phase_does_not_require_duplicate_status_notice() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Connecting);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server…"
    );
    assert_eq!(app.status_notice(), None);

    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);
    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "loading session…"
    );
    assert_eq!(app.status_notice(), None);
}

#[test]
fn test_remote_tui_state_shows_reconnecting_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Reconnecting { attempt: 3 });

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "reconnecting (3)…"
    );
}

#[test]
fn test_openai_compatible_login_preserves_profile_for_runtime_activation() {
    let mut app = create_test_app();

    app.start_login_provider(crate::provider_catalog::ZAI_LOGIN_PROVIDER);

    match app.pending_login {
        Some(crate::tui::app::PendingLogin::ApiKeyProfile {
            provider,
            openai_compatible_profile: Some(profile),
            ..
        }) => {
            assert_eq!(provider, "Z.AI");
            assert_eq!(profile.id, crate::provider_catalog::ZAI_PROFILE.id);
        }
        ref other => panic!("unexpected pending login state: {other:?}"),
    }
}

#[test]
fn test_info_widget_remote_openai_uses_remote_provider_for_usage_and_context() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.4".to_string());
    app.update_context_limit_for_model("gpt-5.4");

    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.provider_name.as_deref(), Some("OpenAI"));
    assert_eq!(data.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(data.context_limit, Some(1_000_000));
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::Unknown
    );
    assert_eq!(
        data.usage_info.as_ref().map(|info| info.provider),
        Some(crate::tui::info_widget::UsageProvider::OpenAI)
    );
}

#[test]
fn test_info_widget_remote_model_falls_back_to_model_provider_detection() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.4".to_string());
    app.update_context_limit_for_model("gpt-5.4");

    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.context_limit, Some(1_000_000));
    assert_eq!(
        data.usage_info.as_ref().map(|info| info.provider),
        Some(crate::tui::info_widget::UsageProvider::OpenAI)
    );
}

#[test]
fn test_info_widget_local_gemini_shows_oauth_auth_method() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = crate::auth::gemini::tokens_path().expect("gemini tokens path");
    crate::storage::write_json_secret(
        &path,
        &serde_json::json!({
            "access_token": "at-123",
            "refresh_token": "rt-456",
            "expires_at": 4102444800000i64,
            "email": "user@example.com"
        }),
    )
    .expect("write gemini tokens");
    crate::auth::AuthStatus::invalidate_cache();

    let app = create_gemini_test_app();
    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.provider_name.as_deref(), Some("gemini"));
    assert_eq!(data.model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::GeminiOAuth
    );
    assert!(data.usage_info.is_none());

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    crate::auth::AuthStatus::invalidate_cache();
}

#[test]
fn test_debug_command_message_respects_queue_mode() {
    let mut app = create_test_app();

    // Test 1: When not processing, should submit directly
    app.is_processing = false;
    let result = app.handle_debug_command("message:hello");
    assert!(
        result.starts_with("OK: submitted message"),
        "Expected submitted, got: {}",
        result
    );
    // The message should be processed for display/session storage while local
    // provider messages are not retained in `app.messages`.
    assert!(app.pending_turn);
    assert_eq!(app.messages.len(), 0);
    assert_eq!(app.display_messages.len(), 1);
    assert_eq!(app.session.messages.len(), 1);

    // Reset for next test
    app.pending_turn = false;
    app.messages.clear();
    app.display_messages.clear();
    app.session.messages.clear();

    // Test 2: When processing with queue_mode=true, should queue
    app.is_processing = true;
    app.queue_mode = true;
    let result = app.handle_debug_command("message:queued_msg");
    assert!(
        result.contains("queued"),
        "Expected queued, got: {}",
        result
    );
    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.queued_messages()[0], "queued_msg");

    // Test 3: When processing with queue_mode=false, should interleave
    app.queued_messages.clear();
    app.queue_mode = false;
    let result = app.handle_debug_command("message:interleave_msg");
    assert!(
        result.contains("interleave"),
        "Expected interleave, got: {}",
        result
    );
    assert_eq!(app.interleave_message.as_deref(), Some("interleave_msg"));
}

#[test]
fn test_debug_command_side_panel_latency_bench_reports_immediate_redraw() {
    let mut app = create_test_app();
    let result = app.handle_debug_command(
        r#"side-panel-latency:{"iterations":8,"warmup_iterations":2,"include_samples":false}"#,
    );
    let value: serde_json::Value =
        serde_json::from_str(&result).expect("side-panel latency bench should return JSON");

    assert_eq!(value.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        value["summary"]["scroll_only_count"].as_u64(),
        Some(0),
        "side-panel latency bench should observe immediate redraw events"
    );
    assert_eq!(
        value["summary"]["unchanged_scroll_count"].as_u64(),
        Some(0),
        "each injected event should change effective side-pane scroll"
    );
    assert!(
        value["summary"]["latency_ms"]["p95"]
            .as_f64()
            .unwrap_or_default()
            < 16.0,
        "headless side-panel p95 latency should stay within a 60fps frame budget: {}",
        result
    );
}

#[test]
fn test_debug_command_mermaid_flicker_bench_returns_json() {
    let mut app = create_test_app();
    let result = app.handle_debug_command("mermaid:flicker-bench 8");
    let value: serde_json::Value =
        serde_json::from_str(&result).expect("flicker bench should return JSON");

    assert_eq!(value["steps"].as_u64(), Some(8));
    assert!(
        value
            .get("protocol_supported")
            .and_then(|v| v.as_bool())
            .is_some(),
        "expected protocol_supported bool in result: {}",
        result
    );
    assert!(
        value.get("deltas").is_some(),
        "expected delta counters: {}",
        result
    );
}

#[test]
fn test_remote_transcript_send_uses_remote_submission_path() {
    let mut app = create_test_app();
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::remote::apply_remote_transcript_event(
            &mut app,
            &mut remote,
            "dictated hello".to_string(),
            crate::protocol::TranscriptMode::Send,
        )
        .await
    })
    .expect("remote transcript send should succeed");

    let last = app
        .display_messages()
        .last()
        .expect("user message displayed");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "[transcription] dictated hello");
    assert!(
        app.is_processing,
        "remote send should enter processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(
        app.current_message_id.is_some(),
        "remote request id should be assigned"
    );
    assert!(
        app.last_stream_activity.is_some(),
        "remote send should start stall timer from a real send"
    );
    assert!(
        !app.pending_turn,
        "remote transcript send must not use local pending_turn path"
    );
    assert!(
        app.input.is_empty(),
        "submitted transcript should clear input"
    );
    assert!(
        app.rate_limit_pending_message.is_some(),
        "remote send should populate retry state for the in-flight request"
    );
}

#[test]
fn test_remote_review_shows_processing_until_split_response() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/review".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("/review should launch split request");

    assert!(
        app.is_processing,
        "review launch should show client processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_none());
    assert_eq!(app.status_notice(), Some("Review launching".to_string()));
    assert!(app.pending_split_startup_message.is_some());
    assert_eq!(app.pending_split_label.as_deref(), Some("Review"));
    assert!(!app.pending_split_request);

    app.handle_server_event(
        crate::protocol::ServerEvent::SplitResponse {
            id: 1,
            new_session_id: "session_review_child".to_string(),
            new_session_name: "review_child".to_string(),
        },
        &mut remote,
    );

    assert!(
        !app.is_processing,
        "split response should clear transient launch state"
    );
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.pending_split_startup_message.is_none());
    assert!(app.pending_split_label.is_none());
}

#[test]
fn test_remote_super_space_routes_next_prompt_to_new_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.input = "hello from split".to_string();
        app.cursor_pos = app.input.len();

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        rt.block_on(app.handle_remote_key(KeyCode::Char(' '), KeyModifiers::SUPER, &mut remote))
            .expect("Super+Space should arm routing");
        assert!(app.route_next_prompt_to_new_session);

        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("armed prompt should launch split request");

        assert!(!app.route_next_prompt_to_new_session);
        assert!(app.pending_split_prompt.is_some());
        assert_eq!(app.pending_split_label.as_deref(), Some("Prompt"));
        assert!(app.is_processing);
        assert!(matches!(app.status, ProcessingStatus::Sending));
        assert!(app.current_message_id.is_none());

        app.handle_server_event(
            crate::protocol::ServerEvent::SplitResponse {
                id: 1,
                new_session_id: "session_prompt_child".to_string(),
                new_session_name: "prompt_child".to_string(),
            },
            &mut remote,
        );

        let restored = App::restore_input_for_reload("session_prompt_child")
            .expect("new prompt session should have startup submission saved");
        assert_eq!(restored.input, "hello from split");
        assert!(restored.submit_on_restore);
        assert!(restored.pending_images.is_empty());
        assert!(app.pending_split_prompt.is_none());
        assert!(app.pending_split_label.is_none());
    });
}

#[test]
fn test_remote_judge_shows_processing_until_split_response() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/judge".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("/judge should launch split request");

    assert!(
        app.is_processing,
        "judge launch should show client processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_none());
    assert_eq!(app.status_notice(), Some("Judge launching".to_string()));
    assert!(app.pending_split_startup_message.is_some());
    assert_eq!(app.pending_split_label.as_deref(), Some("Judge"));
    assert!(!app.pending_split_request);

    app.handle_server_event(
        crate::protocol::ServerEvent::SplitResponse {
            id: 1,
            new_session_id: "session_judge_child".to_string(),
            new_session_name: "judge_child".to_string(),
        },
        &mut remote,
    );

    assert!(
        !app.is_processing,
        "split response should clear transient launch state"
    );
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.pending_split_startup_message.is_none());
    assert!(app.pending_split_label.is_none());
}

// ====================================================================
