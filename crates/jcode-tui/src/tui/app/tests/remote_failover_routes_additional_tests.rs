use super::*;

#[test]
fn test_remote_background_shortcut_during_connection_phase() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for modifiers in [KeyModifiers::ALT, KeyModifiers::CONTROL] {
        let mut app = create_test_app();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let mut peer = remote.take_dummy_peer().unwrap();
        app.is_remote = true;
        app.is_processing = true;
        app.status =
            ProcessingStatus::Connecting(crate::message::ConnectionPhase::WaitingForResponse);
        app.input = "keep draft".into();
        app.cursor_pos = app.input.len();
        rt.block_on(async {
            remote::handle_remote_key_event(
                &mut app,
                crossterm::event::KeyEvent::new(KeyCode::Char('b'), modifiers),
                &mut remote,
            )
            .await
            .unwrap();
            use tokio::io::AsyncBufReadExt;
            let mut line = String::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                tokio::io::BufReader::new(&mut peer).read_line(&mut line),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(matches!(
                serde_json::from_str::<crate::protocol::Request>(&line).unwrap(),
                crate::protocol::Request::BackgroundTool { .. }
            ));
        });
        assert_eq!(app.input, "keep draft");
        assert_eq!(app.cursor_pos, app.input.len());
    }
}

#[test]
fn test_remote_background_result_notice() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    for (moved, expected) in [
        (true, "Tool signalled to move to background"),
        (false, "No running tool to move to background"),
    ] {
        app.handle_server_event(
            crate::protocol::ServerEvent::BackgroundToolResult {
                id: 1,
                moved,
                tool_call_id: None,
                tool_name: None,
            },
            &mut remote,
        );
        assert_eq!(app.status_notice().as_deref(), Some(expected));
    }
}

#[test]
fn test_remote_background_shortcut_idle_keeps_word_navigation() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for modifiers in [KeyModifiers::ALT, KeyModifiers::CONTROL] {
        let mut app = create_test_app();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_remote = true;
        app.is_processing = false;
        app.status = ProcessingStatus::Idle;
        app.input = "keep draft".into();
        app.cursor_pos = app.input.len();
        let request_id = remote.next_request_id_for_test();
        rt.block_on(remote::handle_remote_key_event(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Char('b'), modifiers),
            &mut remote,
        ))
        .unwrap();
        assert_eq!(remote.next_request_id_for_test(), request_id);
        assert_eq!(app.input, "keep draft");
        assert_eq!(app.cursor_pos, 5);
    }
}

#[test]
fn test_remote_account_failover_prompt_does_not_hold_or_retry_same_account() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_remote = true;
    app.auto_poke_incomplete_todos = true;
    app.auto_poke_default_on = true;
    app.queued_messages
        .push(crate::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE.to_string());
    app.pending_queued_dispatch = true;
    app.remote_provider_name = Some("Anthropic".to_string());
    app.remote_provider_model = Some("claude-sonnet-4".to_string());
    app.remote_model_options = vec![
        claude_oauth_route("claude-sonnet-4"),
        claude_oauth_route("claude-opus-4"),
        openai_oauth_route("gpt-5.5"),
    ];
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        jarvis_voice: false,
        content: "hi".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.last_submitted_input = Some("hi".to_string());
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    let prompt = jcode_provider_core::ProviderFailoverPrompt {
        from_provider: "claude".into(),
        from_label: "Anthropic".into(),
        to_provider: "openai".into(),
        to_label: "OpenAI".into(),
        target_route: None,
        reason: "429 rate limit exceeded; retry in 60 seconds".into(),
        estimated_input_chars: 400,
        estimated_input_tokens: 100,
    };

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 21,
            message: prompt.to_error_message(),
            retry_after_secs: Some(60),
        },
        &mut remote,
    );

    assert!(app.rate_limit_reset.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(!app.auto_poke_incomplete_todos);
    assert!(!app.auto_poke_default_on);
    assert!(app.queued_messages.is_empty());
    assert!(!app.pending_queued_dispatch);
    assert!(!app.schedule_turn_end_followups());
    assert_eq!(app.input, "hi");
    assert!(app.pending_provider_failover.is_none());
    let offer = app
        .pending_fallback_offer
        .as_ref()
        .expect("manual fallback offer");
    assert_eq!(offer.selection.provider_label, "OpenAI");
    let resend = offer
        .remote_resend
        .as_ref()
        .expect("preserved failed payload");
    assert_eq!(resend.content, "hi");
    assert_eq!(resend.raw_input.as_deref(), Some("hi"));
    assert!(app.display_messages().iter().any(|message| {
        message.role == "system"
            && message.content.contains("Anthropic unavailable:")
            && message.content.contains("/model")
    }));
    assert!(
        !app.display_messages()
            .iter()
            .any(|message| { message.content.contains("[jcode-provider-failover]") })
    );
    assert!(!app.display_messages().iter().any(|message| {
        message.role == "system"
            && (message.content.contains("queued retry")
                || message.content.contains("Rate limited"))
    }));
}
