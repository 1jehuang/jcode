#[test]
fn test_metadata_only_history_preserves_fast_restored_startup_state() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_fast_resume_meta_42";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("resume me".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg-fast-resume".to_string(),
        role: crate::message::Role::Assistant,
        content: vec![crate::message::ContentBlock::Text {
            text: "restored locally before connect".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save fast resume session");

    let mut app = App::new_for_remote(Some(session_id.to_string()));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard_rt = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: session_id.to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("openai".to_string()),
            provider_model: Some("gpt-5.4".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![session_id.to_string()],
            client_count: Some(1),
            is_canary: Some(false),
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            reload_recovery: None,
            connection_type: Some("https".to_string()),
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

    let assistant_messages: Vec<_> = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "assistant")
        .collect();
    assert_eq!(assistant_messages.len(), 1);
    assert_eq!(
        assistant_messages[0].content,
        "restored locally before connect"
    );
    assert_eq!(app.remote_session_id.as_deref(), Some(session_id));
    assert_eq!(app.connection_type.as_deref(), Some("https"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_duplicate_history_for_same_session_is_ignored_after_fast_path_restore() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("ses_fast_path".to_string());
    app.push_display_message(DisplayMessage::assistant(
        "local restored state".to_string(),
    ));
    remote.mark_history_loaded();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_fast_path".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "server history replay".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
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
            was_interrupted: Some(true),
            connection_type: Some("websocket".to_string()),
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

    let assistant_messages: Vec<_> = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "assistant")
        .collect();
    assert_eq!(assistant_messages.len(), 1);
    assert_eq!(assistant_messages[0].content, "local restored state");
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
    assert!(app.queued_messages().is_empty());
    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    assert!(app.hidden_queued_system_messages[0].contains("interrupted by a server reload"));
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.starts_with("Reload complete — continuing"))
    );
}

#[test]
fn test_compacted_history_marker_scroll_queues_lazy_load() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.replace_display_messages(vec![DisplayMessage::system(
        "Earlier conversation compacted — 128 historical messages hidden from the UI. Scroll to the top to load older history.",
    )]);

    let state = app.compacted_history_lazy_state();
    assert_eq!(state.total_messages, 128);
    assert_eq!(state.visible_messages, 0);
    assert_eq!(state.remaining_messages, 128);

    app.auto_scroll_paused = true;
    app.scroll_offset = 5;
    app.scroll_up(5);

    assert_eq!(app.scroll_offset, 0);
    assert_eq!(app.take_pending_compacted_history_load(), Some(64));
}

#[test]
fn test_local_compacted_history_marker_scroll_expands_from_session() {
    let mut app = create_test_app();
    app.session.add_message(
        crate::message::Role::User,
        vec![crate::message::ContentBlock::Text {
            text: "old prompt".to_string(),
            cache_control: None,
        }],
    );
    app.session.add_message(
        crate::message::Role::Assistant,
        vec![crate::message::ContentBlock::Text {
            text: "old response".to_string(),
            cache_control: None,
        }],
    );
    app.session.add_message(
        crate::message::Role::User,
        vec![crate::message::ContentBlock::Text {
            text: "current prompt".to_string(),
            cache_control: None,
        }],
    );
    app.session.compaction = Some(crate::session::StoredCompactionState {
        summary_text: "old prompt and response".to_string(),
        openai_encrypted_content: None,
        covers_up_to_turn: 2,
        original_turn_count: 2,
        compacted_count: 2,
    });

    let rendered = crate::session::render_messages(&app.session)
        .into_iter()
        .map(|msg| DisplayMessage {
            role: msg.role,
            content: msg.content,
            tool_calls: msg.tool_calls,
            duration_secs: None,
            title: None,
            tool_data: msg.tool_data,
        })
        .collect();
    app.replace_display_messages(rendered);
    assert_eq!(app.compacted_history_lazy_state().remaining_messages, 2);

    app.auto_scroll_paused = true;
    app.scroll_offset = 0;
    app.scroll_up(1);

    assert_eq!(app.take_pending_compacted_history_load(), None);
    assert_eq!(app.compacted_history_lazy_state().visible_messages, 2);
    assert_eq!(app.compacted_history_lazy_state().remaining_messages, 0);
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.content == "old prompt")
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.content == "old response")
    );
}

#[test]
fn test_compacted_history_event_applies_expanded_window() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_session_id = Some("session_lazy_history".to_string());
    app.push_display_message(DisplayMessage::assistant("existing tail"));
    app.scroll_offset = 12;
    app.auto_scroll_paused = false;

    let needs_redraw = app.handle_server_event(
        crate::protocol::ServerEvent::CompactedHistory {
            id: 8,
            session_id: "session_lazy_history".to_string(),
            messages: vec![
                crate::protocol::HistoryMessage {
                    role: "system".to_string(),
                    content: "Earlier conversation compacted — 64 older historical messages hidden. Showing 64 of 128 compacted messages. Scroll to the top to load more.".to_string(),
                    tool_calls: None,
                    tool_data: None,
                },
                crate::protocol::HistoryMessage {
                    role: "assistant".to_string(),
                    content: "older response".to_string(),
                    tool_calls: None,
                    tool_data: None,
                },
                crate::protocol::HistoryMessage {
                    role: "user".to_string(),
                    content: "current prompt".to_string(),
                    tool_calls: None,
                    tool_data: None,
                },
            ],
            images: vec![],
            compacted_total: 128,
            compacted_visible: 64,
            compacted_remaining: 64,
        },
        &mut remote,
    );

    assert!(needs_redraw);
    assert_eq!(app.display_messages().len(), 3);
    assert_eq!(app.display_messages()[1].content, "older response");
    assert_eq!(app.display_messages()[2].content, "current prompt");
    assert!(app.auto_scroll_paused);
    assert_eq!(app.scroll_offset, 0);
    let state = app.compacted_history_lazy_state();
    assert_eq!(state.total_messages, 128);
    assert_eq!(state.visible_messages, 64);
    assert_eq!(state.remaining_messages, 64);
}

#[test]
fn test_remote_error_with_retry_after_keeps_pending_for_auto_retry() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

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
    app.current_message_id = Some(9);

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 9,
            message: "rate limited".to_string(),
            retry_after_secs: Some(3),
        },
        &mut remote,
    );

    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.current_message_id.is_none());
    assert!(app.rate_limit_reset.is_some());
    assert!(app.rate_limit_pending_message.is_some());

    let last = app
        .display_messages()
        .last()
        .expect("missing rate-limit status message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Will auto-retry in 3 seconds"));
}
