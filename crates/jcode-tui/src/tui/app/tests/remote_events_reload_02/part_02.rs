#[test]
fn test_handle_remote_disconnect_preserves_pending_interleaves_for_reconnect() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(44, "acked interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);

    assert!(!app.is_processing);
    assert!(app.interleave_message.is_none());
    assert_eq!(
        app.queued_messages(),
        &["unsent interleave", "queued later"]
    );
    assert_eq!(app.pending_soft_interrupts, vec!["acked interleave"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(44, "acked interleave".to_string())]
    );

    remote.mark_history_loaded();
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
fn test_replace_display_message_content_bumps_version() {
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::system("old reconnect status".to_string()));
    let before = app.display_messages_version;

    assert!(app.replace_display_message_content(0, "new reconnect status".to_string()));
    assert_eq!(app.display_messages[0].content, "new reconnect status");
    assert_ne!(app.display_messages_version, before);

    let after_change = app.display_messages_version;
    assert!(app.replace_display_message_content(0, "new reconnect status".to_string()));
    assert_eq!(app.display_messages_version, after_change);
}

#[test]
fn test_replace_latest_tool_display_message_updates_latest_match_and_bumps_version() {
    let mut app = create_test_app();
    let tool_call = crate::message::ToolCall {
        id: "tool-1".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "src/main.rs"}),
        intent: None,
    };

    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: "placeholder 1".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: Some("old title".to_string()),
        tool_data: Some(tool_call.clone()),
    });
    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: "placeholder 2".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(tool_call),
    });
    let before = app.display_messages_version;

    assert!(app.replace_latest_tool_display_message(
        "tool-1",
        Some("new title".to_string()),
        "final output".to_string(),
    ));
    assert_eq!(app.display_messages()[0].content, "placeholder 1");
    assert_eq!(
        app.display_messages()[0].title.as_deref(),
        Some("old title")
    );
    assert_eq!(app.display_messages()[1].content, "final output");
    assert_eq!(
        app.display_messages()[1].title.as_deref(),
        Some("new title")
    );
    assert_ne!(app.display_messages_version, before);

    let after_change = app.display_messages_version;
    assert!(app.replace_latest_tool_display_message(
        "tool-1",
        Some("new title".to_string()),
        "final output".to_string(),
    ));
    assert_eq!(app.display_messages_version, after_change);
}

#[test]
fn test_push_display_message_coalesces_repeated_single_line_system_messages() {
    let mut app = create_test_app();

    app.push_display_message(DisplayMessage::system(
        "✓ Reconnected successfully.".to_string(),
    ));
    let before = app.display_messages_version;
    app.push_display_message(DisplayMessage::system(
        "✓ Reconnected successfully.".to_string(),
    ));
    app.push_display_message(DisplayMessage::system(
        "✓ Reconnected successfully.".to_string(),
    ));

    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(
        app.display_messages()[0].content,
        "✓ Reconnected successfully. [×3]"
    );
    assert_ne!(app.display_messages_version, before);
}

#[test]
fn test_push_display_message_does_not_coalesce_multiline_system_messages() {
    let mut app = create_test_app();
    let message = "Reload complete\ncontinuing";

    app.push_display_message(DisplayMessage::system(message.to_string()));
    app.push_display_message(DisplayMessage::system(message.to_string()));

    assert_eq!(app.display_messages().len(), 2);
    assert_eq!(app.display_messages()[0].content, message);
    assert_eq!(app.display_messages()[1].content, message);
}

#[test]
fn test_remove_display_message_bumps_version() {
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::system(
        "temporary reconnect status".to_string(),
    ));
    let before = app.display_messages_version;

    let removed = app
        .remove_display_message(0)
        .expect("message should be removed");
    assert_eq!(removed.content, "temporary reconnect status");
    assert!(app.display_messages.is_empty());
    assert_ne!(app.display_messages_version, before);
}

#[test]
fn test_handle_remote_disconnect_retryable_pending_schedules_retry() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });

    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);

    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("retryable continuation should remain pending");
    assert!(pending.auto_retry);
    assert_eq!(pending.retry_attempts, 1);
    assert!(pending.retry_at.is_some());
    assert!(app.rate_limit_reset.is_some());
}

#[test]
fn test_handle_server_event_compaction_shows_completion_message_in_remote_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.provider_session_id = Some("provider-session".to_string());
    app.session.provider_session_id = Some("provider-session".to_string());
    app.context_warning_shown = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::Compaction {
            trigger: "semantic".to_string(),
            pre_tokens: Some(12_345),
            post_tokens: Some(4_321),
            tokens_saved: Some(8_024),
            duration_ms: Some(1_532),
            messages_dropped: None,
            messages_compacted: Some(24),
            summary_chars: Some(987),
            active_messages: Some(10),
        },
        &mut remote,
    );

    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
    assert!(!app.context_warning_shown);
    assert_eq!(app.status_notice(), Some("Context compacted".to_string()));

    let last = app
        .display_messages()
        .last()
        .expect("missing compaction message");
    assert_eq!(last.role, "system");
    assert_eq!(
        last.content,
        "📦 Context compacted (semantic) - older messages were summarized to stay within the context window.\n\nTook 1.5s · before ~12,345 tokens · now ~4,321 tokens (2.2% of window) · saved ~8,024 tokens · summarized 24 messages · summary 987 chars · kept 10 recent messages live"
    );
}

#[test]
fn test_handle_server_event_compaction_mode_changed_updates_remote_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::CompactionModeChanged {
            id: 7,
            mode: crate::config::CompactionMode::Semantic,
            error: None,
        },
        &mut remote,
    );

    assert_eq!(
        app.remote_compaction_mode,
        Some(crate::config::CompactionMode::Semantic)
    );
    assert_eq!(
        app.status_notice(),
        Some("Compaction: semantic".to_string())
    );

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(last.content, "✓ Compaction mode → semantic");
}
