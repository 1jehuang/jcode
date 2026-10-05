// A usage limit on a turn the server started (scheduled task, swarm wake,
// DM) is resumed by the server. A usage limit on a turn the user typed is
// held and resent at the reset by the client, in remote and local mode.

const ANTHROPIC_FAIL_FAST_USAGE_LIMIT: &str = "Anthropic API error (429 Too Many Requests): {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"This request would exceed your account's rate limit. Please try again later.\"}} Usage limit reached for this Claude account; resets in 40m (2026-09-30 13:30 UTC).";

#[test]
fn test_local_usage_limit_stale_reset_stops_after_three_resumes() {
    let mut app = create_test_app();
    let error =
        "API error (402 Payment Required): Usage limit reached [account-usage-limit resets_at=1]";
    app.input = "draft stays here".to_string();
    app.auto_poke_incomplete_todos = true;
    app.auto_poke_default_on = true;
    app.overnight_auto_poke = Some(super::OvernightAutoPokeState {
        run_id: "local-quota-test".to_string(),
        last_fingerprint: super::OvernightAutoPokeFingerprint {
            run_id: "local-quota-test".to_string(),
            status: String::new(),
            last_activity_at: String::new(),
            events_len: 0,
            task_total: 0,
            task_completed: 0,
            task_active: 0,
            task_blocked: 0,
            task_validated: 0,
            session_message_count: 0,
            review_notes_mtime: None,
            validation_files: 0,
        },
        stalled_turns: 0,
        error_turns: 0,
        total_pokes_sent: 0,
        diagnostic_sent: false,
        morning_report_poked: false,
        final_wrap_poked: false,
    });
    app.queued_messages.push("user followup".to_string());
    let transcript_len = app.messages.len();

    for attempt in 1..=3 {
        app.handle_turn_error(error);
        assert_eq!(app.local_usage_limit_resume_attempts, attempt);
        assert!(app.rate_limit_reset.unwrap() > Instant::now());
        super::local::finish_turn(&mut app);
        assert_eq!(app.local_usage_limit_resume_attempts, attempt);
        app.rate_limit_reset = Some(Instant::now());
        super::local::handle_tick(&mut app);
        assert!(app.pending_turn);
        app.pending_turn = false;
    }
    app.handle_turn_error(error);
    super::local::finish_turn(&mut app);
    assert!(app.rate_limit_reset.is_none());
    assert!(!app.pending_turn);
    assert!(!app.auto_poke_incomplete_todos);
    assert!(!app.auto_poke_default_on);
    assert!(app.overnight_auto_poke.is_none());
    assert_eq!(app.input, "draft stays here");
    assert_eq!(app.messages.len(), transcript_len);
    assert_eq!(app.queued_messages, vec!["user followup"]);
    for _ in 0..3 {
        super::local::handle_tick(&mut app);
        assert!(!app.pending_turn);
        assert!(app.rate_limit_reset.is_none());
    }
    // Another limit error cannot silently rearm the exhausted turn.
    app.handle_turn_error(error);
    assert!(app.rate_limit_reset.is_none());
}

#[test]
fn test_local_usage_limit_fresh_prompt_resets_resume_budget() {
    let mut app = create_test_app();
    app.local_usage_limit_resume_attempts = 3;
    app.input = "a fresh task".to_string();
    app.submit_input();
    assert_eq!(app.local_usage_limit_resume_attempts, 0);
    assert!(app.pending_turn);
    app.handle_turn_error(ANTHROPIC_FAIL_FAST_USAGE_LIMIT);
    assert_eq!(app.local_usage_limit_resume_attempts, 1);
    assert!(app.rate_limit_reset.is_some());
}

#[test]
fn test_local_usage_limit_credentials_reset_only_for_held_provider() {
    let mut app = create_test_app();
    app.remote_provider_name = Some("openai-api".to_string());
    app.local_usage_limit_resume_attempts = 3;
    assert!(!app.release_rate_limit_hold_after_credentials_changed(Some("claude")));
    assert_eq!(app.local_usage_limit_resume_attempts, 3);
    // Even an exhausted turn without a timer gets a fresh credential budget.
    assert!(!app.release_rate_limit_hold_after_credentials_changed(Some("openai-api")));
    assert_eq!(app.local_usage_limit_resume_attempts, 0);
    app.handle_turn_error(ANTHROPIC_FAIL_FAST_USAGE_LIMIT);
    assert_eq!(app.local_usage_limit_resume_attempts, 1);
    assert!(app.rate_limit_reset.is_some());
}

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
            server_resumes: true,
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
        last.content
            .starts_with("⏳ Usage limit hit. The server will resume this at "),
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
            server_resumes: true,
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
                server_resumes: false,
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
        wait > Duration::from_secs(40 * 60 - 60) && wait <= Duration::from_secs(40 * 60 + 59),
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

/// A terminal id-0 error that only carries a retry hint (an ordinary rate
/// limit, or the server giving up after its resumes) is a failure the user
/// must see. Only `server_resumes` promises a server resume.
#[test]
fn test_terminal_server_error_with_retry_hint_is_shown_as_an_error() {
    for (message, retry_after_secs) in [
        (ANTHROPIC_FAIL_FAST_USAGE_LIMIT.to_string(), Some(120)),
        (
            "Usage limit still reached after 3 automatic resumes; giving up.".to_string(),
            None,
        ),
    ] {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = None;
        app.processing_started = Some(Instant::now());

        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 0,
                message: message.clone(),
                retry_after_secs,
                server_resumes: false,
            },
            &mut remote,
        );

        assert!(
            !app.display_messages()
                .iter()
                .any(|m| m.content.contains("The server will resume this")),
            "{message}: no resume promised"
        );
        assert!(
            app.display_messages()
                .iter()
                .any(|m| m.role == "error" || m.content.contains(&message[..20])),
            "{message}: shown as a failure: {:?}",
            app.display_messages()
        );
    }
}

/// A server-owned resume notice that arrives while the user's own turn is in
/// flight is about a different turn. It must not settle that turn, count as a
/// failure of it, or schedule a resend.
#[test]
fn test_server_owned_resume_notice_during_users_turn_does_not_spend_its_retries() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "my own turn".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.processing_started = Some(Instant::now());

    for _ in 0..5 {
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 0,
                message: ANTHROPIC_FAIL_FAST_USAGE_LIMIT.to_string(),
                retry_after_secs: Some(120),
                server_resumes: true,
            },
            &mut remote,
        );
    }

    assert!(app.is_processing, "the user's turn keeps running");
    assert_eq!(app.current_message_id, Some(7));
    assert!(app.rate_limit_reset.is_none(), "no client resend is scheduled");
    assert_eq!(
        app.rate_limit_pending_message
            .as_ref()
            .map(|pending| pending.retry_attempts),
        Some(0),
        "the user's retry budget is untouched"
    );
    let last = app.display_messages().last().expect("a notice");
    assert!(
        last.content
            .starts_with("⏳ Usage limit hit. The server will resume this at "),
        "{}",
        last.content
    );
}
