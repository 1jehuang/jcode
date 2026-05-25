#[test]
fn test_new_for_remote_restored_soft_interrupt_resend_triggers_dispatch_state() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-soft-interrupt-dispatch-{}", std::process::id());

    app.pending_soft_interrupts = vec!["sent interrupt".to_string()];
    app.pending_soft_interrupt_requests = vec![(55, "sent interrupt".to_string())];
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.interleave_message.is_none());
    assert_eq!(restored.queued_messages(), &["sent interrupt"]);
    assert!(restored.pending_queued_dispatch);
    assert!(restored.is_processing);
    assert!(matches!(restored.status, ProcessingStatus::Sending));
}

#[test]
fn test_new_for_remote_does_not_requeue_acked_pending_soft_interrupts() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-acked-{}", std::process::id());

    app.interleave_message = Some("local interleave".to_string());
    app.pending_soft_interrupts = vec!["already queued on server".to_string()];
    app.queued_messages.push("queued later".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert_eq!(
        restored.interleave_message.as_deref(),
        Some("local interleave")
    );
    assert_eq!(restored.queued_messages(), &["queued later"]);
}

#[test]
fn test_initial_history_bootstrap_preserves_restored_interleave_state() {
    with_temp_jcode_home(|| {
        let session_id = "session_reload_restore_interleave";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("reload restore".to_string()),
        );
        session.save().expect("save session for reload restore");

        let mut app = create_test_app();
        app.interleave_message = Some("interrupt after reload".to_string());
        app.pending_soft_interrupts = vec!["already sent interrupt".to_string()];
        app.pending_soft_interrupt_requests = vec![(55, "already sent interrupt".to_string())];
        app.queued_messages.push("queued followup".to_string());
        app.save_input_for_reload(session_id);

        let mut restored = App::new_for_remote(Some(session_id.to_string()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        restored.handle_server_event(
            crate::protocol::ServerEvent::History {
                id: 1,
                session_id: session_id.to_string(),
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
                side_panel: crate::side_panel::SidePanelSnapshot::default(),
            },
            &mut remote,
        );

        assert!(restored.interleave_message.is_none());
        assert_eq!(
            restored.queued_messages(),
            &[
                "interrupt after reload",
                "already sent interrupt",
                "queued followup"
            ]
        );
        assert!(
            restored.pending_soft_interrupts.is_empty(),
            "restored pending interrupts should remain represented by interleave + queue state"
        );
    });
}

#[test]
fn test_initial_history_bootstrap_skips_resubmit_when_prompt_already_in_history() {
    with_temp_jcode_home(|| {
        let session_id = "session_reload_prompt_already_in_history";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("reload prompt already in history".to_string()),
        );
        session.save().expect("save session for reload restore");

        let mut app = create_test_app();
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "continue implementing the fix".to_string(),
            images: vec![],
            is_system: false,
            system_reminder: None,
            auto_retry: false,
            retry_attempts: 0,
            retry_at: None,
        });
        app.save_input_for_reload(session_id);

        let mut restored = App::new_for_remote(Some(session_id.to_string()));
        assert!(restored.submit_input_on_startup);
        assert_eq!(restored.input, "continue implementing the fix");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        restored.handle_server_event(
            crate::protocol::ServerEvent::History {
                id: 1,
                session_id: session_id.to_string(),
                messages: vec![crate::protocol::HistoryMessage {
                    role: "user".to_string(),
                    content: "continue implementing the fix".to_string(),
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

        assert!(!restored.submit_input_on_startup);
        assert!(restored.input.is_empty());
        assert!(
            restored
                .display_messages()
                .iter()
                .any(|message| message.content.starts_with("Reload complete — continuing")),
            "server interruption recovery should continue using the restored server-side prompt"
        );
    });
}

#[test]
fn test_reload_progress_coalesces_into_single_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Reloading { new_socket: None },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ReloadProgress {
            step: "init".to_string(),
            message: "🔄 Starting hot-reload...".to_string(),
            success: None,
            output: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ReloadProgress {
            step: "verify".to_string(),
            message: "Binary verified".to_string(),
            success: Some(true),
            output: Some("size=68.4MB".to_string()),
        },
        &mut remote,
    );

    assert_eq!(app.display_messages().len(), 1);
    let reload_msg = &app.display_messages()[0];
    assert_eq!(reload_msg.role, "system");
    assert_eq!(reload_msg.title.as_deref(), Some("Reload"));
    assert_eq!(
        reload_msg.content,
        "🔄 Server reload initiated...\n[init] 🔄 Starting hot-reload...\n[verify] ✓ Binary verified\n```\nsize=68.4MB\n```"
    );
}

#[test]
fn test_handle_server_event_updates_connection_type() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionType {
            connection: "websocket".to_string(),
        },
        &mut remote,
    );

    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
}
