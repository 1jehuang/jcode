// OAuth/account swap mid-session: a turn held on the previous account's
// rate/usage limit must resend promptly on the new credentials instead of
// sleeping until the old account's reset time (which can be hours away).

fn held_on_rate_limit_app(hold: Duration) -> App {
    let mut app = create_test_app();
    // The held turn runs on Claude; credential changes are scoped by provider.
    app.remote_provider_name = Some("Claude".to_string());
    let retry_at = Instant::now() + hold;
    app.rate_limit_reset = Some(retry_at);
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "finish the refactor".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app
}

fn account_changed_notices(app: &App) -> usize {
    app.display_messages()
        .iter()
        .filter(|m| m.content.contains("Account changed. Resending your message"))
        .count()
}

#[test]
fn test_credentials_changed_resends_turn_held_on_rate_limit() {
    let mut app = held_on_rate_limit_app(Duration::from_secs(3 * 3600));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: Some("anthropic".to_string()),
            account_label: None,
        },
        &mut remote,
    );

    let reset = app
        .rate_limit_reset
        .expect("the held turn must stay armed for resend");
    assert!(
        reset <= Instant::now(),
        "account change must pull the 3h hold forward to now"
    );
    assert_eq!(account_changed_notices(&app), 1);

    // A second auth broadcast (login + catalog refresh both fire) must not
    // add a duplicate notice or schedule a second resend.
    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: None,
            account_label: None,
        },
        &mut remote,
    );
    assert_eq!(account_changed_notices(&app), 1);

    rt.block_on(crate::tui::app::remote::handle_tick(&mut app, &mut remote));
    assert!(app.is_processing, "tick must resend the held turn");
    assert!(app.rate_limit_reset.is_none());
    assert_eq!(
        app.rate_limit_pending_message
            .as_ref()
            .map(|p| p.content.as_str()),
        Some("finish the refactor")
    );
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("Rate limit reset")),
        "resend after an account change must not claim the limit reset"
    );

    // Once resent, further broadcasts are inert: no double send.
    let sent_id = app.current_message_id;
    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: None,
            account_label: None,
        },
        &mut remote,
    );
    rt.block_on(crate::tui::app::remote::handle_tick(&mut app, &mut remote));
    assert_eq!(app.current_message_id, sent_id);
    assert!(app.rate_limit_reset.is_none());
    assert_eq!(account_changed_notices(&app), 1);
}

#[test]
fn test_credentials_changed_without_hold_is_a_noop() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let before = app.display_messages().len();

    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: None,
            account_label: None,
        },
        &mut remote,
    );

    assert!(app.rate_limit_reset.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert_eq!(app.display_messages().len(), before);
}

#[test]
fn test_credentials_changed_does_not_shortcut_network_wait() {
    let mut app = held_on_rate_limit_app(Duration::from_secs(5));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.status = ProcessingStatus::WaitingForNetwork {
        listener: "test".to_string(),
    };
    let reset = app.rate_limit_reset;

    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: None,
            account_label: None,
        },
        &mut remote,
    );

    assert_eq!(app.rate_limit_reset, reset, "offline holds are not auth holds");
    assert_eq!(account_changed_notices(&app), 0);
}

#[test]
fn test_rate_limit_error_for_turn_started_before_account_change_retries_promptly() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Turn in flight on the old account (possibly sleeping in a provider retry).
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
    app.processing_started = Some(Instant::now() - Duration::from_secs(30));

    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: None,
            account_label: None,
        },
        &mut remote,
    );
    assert!(app.is_processing, "an in-flight turn is not interrupted");

    // The old account's limit error lands after the swap.
    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 9,
            message: "rate limited".to_string(),
            retry_after_secs: Some(3 * 3600),
        },
        &mut remote,
    );

    let reset = app.rate_limit_reset.expect("turn must stay held for resend");
    assert!(
        reset <= Instant::now() + Duration::from_secs(5),
        "limit from the previous account must not hold the turn for hours"
    );
    assert!(app.rate_limit_pending_message.is_some());
    assert_eq!(account_changed_notices(&app), 1);
}

#[test]
fn test_rate_limit_error_without_account_change_keeps_full_hold() {
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
    app.processing_started = Some(Instant::now());

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 9,
            message: "rate limited".to_string(),
            retry_after_secs: Some(3 * 3600),
        },
        &mut remote,
    );

    let reset = app.rate_limit_reset.expect("rate limit hold");
    assert!(reset > Instant::now() + Duration::from_secs(3 * 3600 - 60));
    assert_eq!(account_changed_notices(&app), 0);
}

#[test]
fn test_local_account_switch_releases_rate_limit_hold() {
    let mut app = held_on_rate_limit_app(Duration::from_secs(3 * 3600));

    assert!(app.release_rate_limit_hold_after_credentials_changed(None));
    assert!(app.rate_limit_reset.expect("armed") <= Instant::now());
    assert_eq!(account_changed_notices(&app), 1);
    // Idempotent.
    assert!(!app.release_rate_limit_hold_after_credentials_changed(None));
    assert_eq!(account_changed_notices(&app), 1);
}

// Greptile PRRT_kwDOQz8JRs6nTkDO: a credential change for one provider must
// not resend a turn held on a different provider's exhausted account.
fn claude_held_app() -> App {
    let mut app = held_on_rate_limit_app(Duration::from_secs(3 * 3600));
    app.remote_provider_name = Some("Claude".to_string());
    app
}

#[test]
fn test_credentials_changed_scope_openai_credentials_change_keeps_claude_turn_held() {
    let mut app = claude_held_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: Some("openai".to_string()),
            account_label: None,
        },
        &mut remote,
    );

    let reset = app.rate_limit_reset.expect("hold stays armed");
    assert!(
        reset > Instant::now() + Duration::from_secs(3 * 3600 - 60),
        "an OpenAI change must not release a hold on the Claude account"
    );
    assert_eq!(account_changed_notices(&app), 0);
    assert!(
        !app.turn_predates_credentials_change(),
        "an unrelated provider change must not mark Claude limits as stale"
    );
}

#[test]
fn test_credentials_changed_scope_claude_credentials_change_releases_claude_turn() {
    for provider in ["anthropic", "claude"] {
        let mut app = claude_held_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        app.handle_server_event(
            crate::protocol::ServerEvent::CredentialsChanged {
                provider: Some(provider.to_string()),
                account_label: None,
            },
            &mut remote,
        );

        assert!(app.rate_limit_reset.expect("armed") <= Instant::now());
        assert_eq!(account_changed_notices(&app), 1, "provider={provider}");
    }
}

#[test]
fn test_credentials_changed_scope_unscoped_credentials_change_releases_claude_turn() {
    let mut app = claude_held_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: None,
            account_label: None,
        },
        &mut remote,
    );

    assert!(app.rate_limit_reset.expect("armed") <= Instant::now());
    assert_eq!(account_changed_notices(&app), 1);
}

#[test]
fn test_credentials_changed_scope_claude_change_keeps_openai_turn_held() {
    let mut app = held_on_rate_limit_app(Duration::from_secs(3 * 3600));
    app.remote_provider_name = Some("openai".to_string());

    assert!(!app.release_rate_limit_hold_after_credentials_changed(Some("claude")));
    assert_eq!(account_changed_notices(&app), 0);
    assert!(app.release_rate_limit_hold_after_credentials_changed(Some("openai-api")));
    assert_eq!(account_changed_notices(&app), 1);
}

#[test]
fn test_credentials_changed_scope_local_openai_account_switch_keeps_claude_turn_held() {
    let mut app = claude_held_app();

    assert!(!app.release_rate_limit_hold_after_credentials_changed(Some("openai")));
    assert_eq!(account_changed_notices(&app), 0);
    assert!(app.release_rate_limit_hold_after_credentials_changed(Some("anthropic")));
    assert_eq!(account_changed_notices(&app), 1);
}

#[test]
fn test_credentials_changed_for_another_label_does_not_resend_held_turn() {
    let mut app = held_on_rate_limit_app(Duration::from_secs(3 * 3600));
    app.set_window_account_from_info(&crate::protocol::SessionAccountInfo {
        provider: "claude".to_string(),
        label: Some("claude-otter".to_string()),
        pinned: true,
        is_default: false,
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Another window's account (same provider) re-logged in: this window's
    // otter is still exhausted, so the hold must stay.
    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: Some("anthropic".to_string()),
            account_label: Some("claude-fox".to_string()),
        },
        &mut remote,
    );
    let reset = app.rate_limit_reset.expect("hold kept");
    assert!(reset > Instant::now(), "another label must not release the hold");
    assert_eq!(account_changed_notices(&app), 0);

    // This window's own label changed: resend.
    app.handle_server_event(
        crate::protocol::ServerEvent::CredentialsChanged {
            provider: Some("anthropic".to_string()),
            account_label: Some("claude-otter".to_string()),
        },
        &mut remote,
    );
    assert!(app.rate_limit_reset.expect("armed") <= Instant::now());
    assert_eq!(account_changed_notices(&app), 1);
}

#[test]
fn test_session_account_changed_updates_window_account_and_explains_moves() {
    let mut app = create_test_app();
    app.remote_provider_name = Some("Claude".to_string());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.set_window_account_from_info(&crate::protocol::SessionAccountInfo {
        provider: "claude".to_string(),
        label: Some("claude-otter".to_string()),
        pinned: false,
        is_default: true,
    });
    let before = app.display_messages().len();
    app.handle_server_event(
        crate::protocol::ServerEvent::SessionAccountChanged {
            provider: "claude".to_string(),
            label: Some("claude-fox".to_string()),
            pinned: true,
            is_default: false,
            reason: Some("otter resets 14:05".to_string()),
        },
        &mut remote,
    );
    let account = app
        .window_account("claude")
        .expect("window account stored");
    assert_eq!(account.label.as_deref(), Some("claude-fox"));
    assert!(account.pinned);
    let last = app.display_messages().last().unwrap().content.clone();
    assert_eq!(app.display_messages().len(), before + 1);
    assert!(
        last.contains("⚡ This window moved from claude-otter to claude-fox (otter resets 14:05)"),
        "{last}"
    );

    // Without a reason (a reply to this window's own command) there is no
    // extra transcript line.
    app.handle_server_event(
        crate::protocol::ServerEvent::SessionAccountChanged {
            provider: "claude".to_string(),
            label: Some("claude-otter".to_string()),
            pinned: false,
            is_default: true,
            reason: None,
        },
        &mut remote,
    );
    assert_eq!(app.display_messages().len(), before + 1);
    let widget = app.info_widget_data().window_account.expect("widget account");
    assert_eq!(widget.label, "claude-otter");
    assert!(widget.is_default && !widget.pinned);
}
