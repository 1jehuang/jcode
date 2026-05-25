#[test]
fn test_handle_server_event_service_tier_changed_mentions_next_request_when_streaming() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::ServiceTierChanged {
            id: 7,
            service_tier: Some("priority".to_string()),
            error: None,
        },
        &mut remote,
    );

    assert_eq!(app.remote_service_tier, Some("priority".to_string()));
    assert_eq!(
        app.status_notice(),
        Some("Fast: on (next request)".to_string())
    );

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(
        last.content,
        "✓ Fast mode on (Fast)\nApplies to the next request/turn. The current in-flight request keeps its existing tier."
    );
}

#[test]
fn test_reload_handoff_active_when_server_reload_flag_set() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let state = remote::RemoteRunState {
        server_reload_in_progress: true,
        ..Default::default()
    };

    assert!(remote::reload_handoff_active(&state));

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn test_reload_handoff_inactive_without_flag_or_marker() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let state = remote::RemoteRunState::default();

    assert!(!remote::reload_handoff_active(&state));

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn test_reload_handoff_active_when_reload_marker_present() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    crate::server::write_reload_state(
        "reload-marker-test",
        "test-hash",
        crate::server::ReloadPhase::Starting,
        None,
    );

    let state = remote::RemoteRunState {
        ..Default::default()
    };

    assert!(remote::reload_handoff_active(&state));

    crate::server::clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn test_reload_handoff_active_when_socket_ready_marker_present() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    crate::server::write_reload_state(
        "reload-marker-test",
        "test-hash",
        crate::server::ReloadPhase::SocketReady,
        None,
    );

    let state = remote::RemoteRunState::default();

    assert!(remote::reload_handoff_active(&state));

    crate::server::clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn test_handle_server_event_history_with_interruption_queues_continuation() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_test_123".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "I was working on something".to_string(),
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
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: Some(true),
            reload_recovery: None,
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

    assert!(app.display_messages().len() >= 2);
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
    let system_msg = app
        .display_messages()
        .iter()
        .find(|m| m.role == "system" && m.content.starts_with("Reload complete — continuing"))
        .expect("should have a short reload continuation message");
    assert!(system_msg.content.starts_with("Reload complete — continuing"));

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
fn test_handle_server_event_history_uses_server_owned_reload_recovery_directive() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_server_owned_reload".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "Reconnect me from server history".to_string(),
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
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            reload_recovery: Some(crate::protocol::ReloadRecoverySnapshot {
                reconnect_notice: Some("Reloaded with build srv1234".to_string()),
                continuation_message: "Server-owned reload continuation".to_string(),
            }),
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

    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    assert_eq!(
        app.hidden_queued_system_messages[0],
        "Server-owned reload continuation"
    );
    assert!(app.reload_info.iter().any(|line| line.contains("srv1234")));
}

#[test]
fn test_handle_server_event_history_without_interruption_does_not_queue() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_test_456".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "Normal response".to_string(),
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
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            reload_recovery: None,
            connection_type: Some("https/sse".to_string()),
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

    assert!(app.queued_messages().is_empty());
    assert_eq!(app.connection_type.as_deref(), Some("https/sse"));
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("interrupted"))
    );
}

#[test]
fn test_handle_server_event_history_after_reload_reports_no_continuation_needed() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.pending_reload_reconnect_status = Some(PendingReloadReconnectStatus::AwaitingHistory {
        session_id: Some("ses_reload_done".to_string()),
    });

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_reload_done".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "Finished before reload".to_string(),
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
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: Some(false),
            reload_recovery: None,
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

    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(app.pending_reload_reconnect_status.is_none());
    assert!(app.display_messages().iter().any(|m| {
        m.role == "system"
            && m.content.contains("no continuation needed")
            && m.content.contains("previous response had already finished")
    }));
}

#[test]
fn test_finalize_reload_reconnect_marker_only_does_not_queue_selfdev_continuation() {
    let mut app = create_test_app();
    app.reload_info
        .push("Reloaded with build abc1234".to_string());

    remote::finalize_reload_reconnect(
        &mut app,
        Some("ses_test_marker_only"),
        remote::ReloadReconnectHints {
            reload_ctx_for_session: None,
            has_client_reload_marker: true,
        },
        false,
    );

    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(app.reload_info.is_empty());
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.starts_with("Reload complete — continuing"))
    );
}

#[test]
fn test_same_session_fast_path_allowed_for_non_reload_reconnect() {
    assert!(remote::should_use_same_session_fast_path(
        true,
        Some("ses_same"),
        Some("ses_same"),
        true,
        false,
    ));
}

#[test]
fn test_same_session_fast_path_disabled_when_reload_needs_server_history() {
    assert!(!remote::should_use_same_session_fast_path(
        true,
        Some("ses_same"),
        Some("ses_same"),
        true,
        true,
    ));
}

#[test]
fn test_reload_persisted_background_tasks_note_mentions_running_task() {
    let session_id = crate::id::new_id("ses_bg_note");
    let manager = crate::background::global();
    let info = manager.reserve_task_info();
    let started_at = chrono::Utc::now().to_rfc3339();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(manager.register_detached_task(
        &info,
        "bash",
        None,
        &session_id,
        std::process::id(),
        &started_at,
        true,
        false,
    ));

    let note = reload_persisted_background_tasks_note(&session_id);

    assert!(note.contains(&info.task_id));
    assert!(note.contains("Do not rerun those commands"));
    assert!(note.contains("bg action=\"status\""));

    cleanup_background_task_files(&info.task_id);
}

#[test]
fn test_finalize_reload_reconnect_mentions_persisted_background_task() {
    let _guard = crate::storage::lock_test_env();
    let mut app = create_test_app();
    let session_id = crate::id::new_id("ses_reload_bg");
    let reload_ctx = crate::tool::selfdev::ReloadContext {
        task_context: Some("Waiting for cargo build --release".to_string()),
        version_before: "v0.1.100".to_string(),
        version_after: "abc1234".to_string(),
        session_id: session_id.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    reload_ctx.save().expect("save reload context");

    let manager = crate::background::global();
    let info = manager.reserve_task_info();
    let started_at = chrono::Utc::now().to_rfc3339();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(manager.register_detached_task(
        &info,
        "bash",
        None,
        &session_id,
        std::process::id(),
        &started_at,
        true,
        false,
    ));

    remote::finalize_reload_reconnect(
        &mut app,
        Some(session_id.as_str()),
        remote::ReloadReconnectHints {
            reload_ctx_for_session: Some(reload_ctx.clone()),
            has_client_reload_marker: false,
        },
        false,
    );

    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    let continuation = &app.hidden_queued_system_messages[0];
    assert!(continuation.contains("Persisted background task(s)"));
    assert!(continuation.contains(&info.task_id));
    assert!(continuation.contains("Do not rerun those commands"));
    assert!(continuation.contains("bg action=\"output\""));

    cleanup_background_task_files(&info.task_id);
    cleanup_reload_context_file(&session_id);
}

#[test]
fn test_finalize_reload_reconnect_is_session_scoped_across_reconnect_order() {
    let _guard = crate::storage::lock_test_env();
    let mut app_a = create_test_app();
    let mut app_b = create_test_app();
    let session_a = crate::id::new_id("ses_reload_a");
    let session_b = crate::id::new_id("ses_reload_b");

    let ctx_a = crate::tool::selfdev::ReloadContext {
        task_context: Some("resume session A".to_string()),
        version_before: "old-a".to_string(),
        version_after: "new-a".to_string(),
        session_id: session_a.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    let ctx_b = crate::tool::selfdev::ReloadContext {
        task_context: Some("resume session B".to_string()),
        version_before: "old-b".to_string(),
        version_after: "new-b".to_string(),
        session_id: session_b.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    ctx_a.save().expect("save reload context a");
    ctx_b.save().expect("save reload context b");

    remote::finalize_reload_reconnect(
        &mut app_b,
        Some(session_b.as_str()),
        remote::ReloadReconnectHints {
            reload_ctx_for_session: Some(ctx_b.clone()),
            has_client_reload_marker: false,
        },
        false,
    );

    assert_eq!(app_b.hidden_queued_system_messages.len(), 1);
    assert!(app_b.hidden_queued_system_messages[0].contains("new-b"));
    assert!(
        crate::tool::selfdev::ReloadContext::peek_for_session(&session_a)
            .expect("peek session a")
            .is_some(),
        "session A context should remain available after session B reconnects first"
    );
    assert!(
        crate::tool::selfdev::ReloadContext::peek_for_session(&session_b)
            .expect("peek session b")
            .is_none(),
        "session B context should be consumed by its own reconnect"
    );

    remote::finalize_reload_reconnect(
        &mut app_a,
        Some(session_a.as_str()),
        remote::ReloadReconnectHints {
            reload_ctx_for_session: Some(ctx_a.clone()),
            has_client_reload_marker: false,
        },
        false,
    );

    assert_eq!(app_a.hidden_queued_system_messages.len(), 1);
    assert!(app_a.hidden_queued_system_messages[0].contains("new-a"));
    assert!(
        crate::tool::selfdev::ReloadContext::peek_for_session(&session_a)
            .expect("peek session a after consume")
            .is_none(),
        "session A context should be consumed only by session A reconnect"
    );
}

#[test]
fn test_finalize_reload_reconnect_supports_repeated_reload_cycles_for_same_session() {
    let _guard = crate::storage::lock_test_env();
    let session_id = crate::id::new_id("ses_reload_loop");

    for cycle in 0..3 {
        let mut app = create_test_app();
        let version_after = format!("loop-build-{}", cycle);
        let reload_ctx = crate::tool::selfdev::ReloadContext {
            task_context: Some(format!("reload loop cycle {}", cycle)),
            version_before: format!("loop-prev-{}", cycle),
            version_after: version_after.clone(),
            session_id: session_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        reload_ctx.save().expect("save loop reload context");

        remote::finalize_reload_reconnect(
            &mut app,
            Some(session_id.as_str()),
            remote::ReloadReconnectHints {
                reload_ctx_for_session: Some(reload_ctx.clone()),
                has_client_reload_marker: false,
            },
            false,
        );

        assert_eq!(app.hidden_queued_system_messages.len(), 1);
        assert!(app.hidden_queued_system_messages[0].contains(&version_after));
        assert!(
            crate::tool::selfdev::ReloadContext::peek_for_session(&session_id)
                .expect("peek loop reload context")
                .is_none(),
            "reload context should be consumed each cycle"
        );
    }
}

#[test]
fn test_handle_server_event_history_restores_side_panel_snapshot() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "/tmp/plan.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Plan\n```mermaid\nflowchart LR\nA-->B\n```".to_string(),
            updated_at_ms: 1,
        }],
    };

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_side_panel_history".to_string(),
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
            connection_type: Some("websocket".to_string()),
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: side_panel.clone(),
        },
        &mut remote,
    );

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(app.side_panel.pages.len(), 1);
    assert_eq!(
        app.side_panel
            .focused_page()
            .map(|page| page.title.as_str()),
        Some("Plan")
    );
}

#[test]
fn test_handle_server_event_history_restores_active_resume_processing_state() {
    let _guard = crate::storage::lock_test_env();
    let mut app = App::new_for_remote(Some("ses_resume_active".to_string()));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_resume_active".to_string(),
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
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            reload_recovery: None,
            connection_type: Some("websocket".to_string()),
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: Some(crate::protocol::SessionActivitySnapshot {
                is_processing: true,
                current_tool_name: Some("batch".to_string()),
            }),
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert!(app.is_processing());
    assert!(app.processing_started.is_some());
    assert!(app.time_since_activity().is_some());
    assert!(matches!(app.status, ProcessingStatus::RunningTool(ref name) if name == "batch"));
}

#[test]
fn test_handle_server_event_side_panel_state_updates_snapshot() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("old".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "old".to_string(),
            title: "Old".to_string(),
            file_path: "/tmp/old.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "old".to_string(),
            updated_at_ms: 1,
        }],
    };
    app.diff_pane_scroll = 7;

    app.handle_server_event(
        crate::protocol::ServerEvent::SidePanelState {
            snapshot: crate::side_panel::SidePanelSnapshot {
                focused_page_id: Some("new".to_string()),
                pages: vec![crate::side_panel::SidePanelPage {
                    id: "new".to_string(),
                    title: "New".to_string(),
                    file_path: "/tmp/new.md".to_string(),
                    format: crate::side_panel::SidePanelPageFormat::Markdown,
                    source: crate::side_panel::SidePanelPageSource::Managed,
                    content: "# New".to_string(),
                    updated_at_ms: 2,
                }],
            },
        },
        &mut remote,
    );

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("new"));
    assert_eq!(app.side_panel.pages.len(), 1);
    assert_eq!(app.diff_pane_scroll, 0);
}

#[test]
fn test_remote_swarm_status_does_not_clobber_newer_session_history_on_disk() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_remote_preserve_history";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote preserve history".to_string()),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "older on-disk message".to_string(),
            cache_control: None,
        }],
    );
    session.save().expect("save initial session");

    let mut app = App::new_for_remote(Some(session_id.to_string()));
    app.remote_session_id = Some(session_id.to_string());
    app.swarm_enabled = true;

    // Simulate the shared server advancing the authoritative session file after the
    // remote client already loaded its shadow copy.
    let mut fresher = crate::session::Session::load(session_id).expect("load fresher session");
    fresher.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "newer server-side message".to_string(),
            cache_control: None,
        }],
    );
    fresher.save().expect("save fresher session");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::SwarmStatus { members: vec![] },
        &mut remote,
    );

    let persisted = crate::session::Session::load(session_id).expect("reload persisted session");
    assert_eq!(
        persisted.messages.len(),
        2,
        "remote UI persistence should not roll back newer server-written messages"
    );
    let last_text = persisted
        .messages
        .last()
        .and_then(|msg| {
            msg.content.iter().find_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
        })
        .expect("last message text");
    assert_eq!(last_text, "newer server-side message");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
