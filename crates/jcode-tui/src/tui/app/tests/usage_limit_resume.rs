// A usage limit on a turn the server started (scheduled task, swarm wake,
// DM) is resumed by the server. A usage limit on a turn the user typed is
// held and resent at the reset by the client, in remote and local mode.

const ANTHROPIC_FAIL_FAST_USAGE_LIMIT: &str = "Anthropic API error (429 Too Many Requests): {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"This request would exceed your account's rate limit. Please try again later.\"}} Usage limit reached for this Claude account; resets in 40m (2026-09-30 13:30 UTC).";

#[test]
fn test_server_initiated_usage_limit_shows_server_resume_time_without_client_resend() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // The client adopted a turn the server started (no message id of ours).
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = None;
    app.processing_started = Some(Instant::now());

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 0,
            message: ANTHROPIC_FAIL_FAST_USAGE_LIMIT.to_string(),
            retry_after_secs: Some(40 * 60 + 12),
        },
        &mut remote,
    );

    assert!(!app.is_processing, "the adopted turn is settled");
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(
        app.rate_limit_reset.is_none() && app.rate_limit_pending_message.is_none(),
        "the server owns the resume; the client must not schedule its own"
    );
    let expected_at = (chrono::Local::now() + chrono::Duration::seconds(40 * 60 + 12))
        .format("%H:%M")
        .to_string();
    let last = app.display_messages().last().expect("a notice");
    assert_eq!(last.role, "system");
    assert!(
        last.content.starts_with("⏳ Usage limit hit. The server will resume this at "),
        "{}",
        last.content
    );
    assert!(last.content.ends_with(&expected_at), "{}", last.content);
    assert!(
        !app.display_messages()
            .iter()
            .any(|message| message.role == "error"),
        "not shown as a plain error"
    );
}

#[test]
fn test_server_initiated_usage_limit_keeps_the_users_own_held_turn() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let held_until = Instant::now() + Duration::from_secs(600);
    app.rate_limit_reset = Some(held_until);
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "my own turn".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: Some(held_until),
    });

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 0,
            message: ANTHROPIC_FAIL_FAST_USAGE_LIMIT.to_string(),
            retry_after_secs: Some(120),
        },
        &mut remote,
    );

    assert_eq!(app.rate_limit_reset, Some(held_until));
    assert_eq!(
        app.rate_limit_pending_message
            .as_ref()
            .map(|pending| pending.content.as_str()),
        Some("my own turn")
    );
}

/// Guard: the user's own remote turn with this exact error is held and
/// resent at the reset (the server sends retry_after_secs; the text alone
/// also works).
#[test]
fn test_user_typed_remote_turn_with_usage_limit_is_held_until_the_reset() {
    for retry_after_secs in [Some(40 * 60), None] {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "finish the refactor".to_string(),
            images: vec![],
            is_system: false,
            system_reminder: None,
            auto_retry: false,
            retry_attempts: 0,
            retry_at: None,
        });
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(12);
        app.processing_started = Some(Instant::now());

        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 12,
                message: ANTHROPIC_FAIL_FAST_USAGE_LIMIT.to_string(),
                retry_after_secs,
            },
            &mut remote,
        );

        let reset = app
            .rate_limit_reset
            .expect("turn held for a resend at the reset");
        let wait = reset.saturating_duration_since(Instant::now());
        assert!(
            wait > Duration::from_secs(40 * 60 - 60) && wait <= Duration::from_secs(40 * 60),
            "{retry_after_secs:?}: {wait:?}"
        );
        assert_eq!(
            app.rate_limit_pending_message
                .as_ref()
                .map(|pending| pending.content.as_str()),
            Some("finish the refactor")
        );
        assert!(!app.is_processing);
    }
}

/// Local (non-remote) mode: a user-typed turn that hits the usage limit is
/// held and runs again once the reset passes, instead of only restoring the
/// prompt to the input box.
#[test]
fn test_user_typed_local_turn_with_usage_limit_is_retried_at_the_reset() {
    let mut app = create_test_app();
    assert!(!app.is_remote);
    app.last_submitted_input = Some("finish the refactor".to_string());

    app.handle_turn_error(ANTHROPIC_FAIL_FAST_USAGE_LIMIT);

    let reset = app
        .rate_limit_reset
        .expect("local turn held for a retry at the reset");
    let wait = reset.saturating_duration_since(Instant::now());
    assert!(
        wait > Duration::from_secs(40 * 60 - 60) && wait <= Duration::from_secs(40 * 60),
        "{wait:?}"
    );
    assert!(
        app.input.is_empty(),
        "the prompt stays in the transcript for the retry, not the input box"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.content.contains("Will auto-retry")),
        "the user is told when it retries"
    );

    // The tick after the reset retries the turn.
    app.rate_limit_reset = Some(Instant::now());
    let _ = super::local::handle_tick(&mut app);
    assert!(app.pending_turn, "the held turn runs again after the reset");
}
