// Issue #1685: rate-limit and provider connection auto-resumes must be
// bounded per turn, then pause with the draft restored.

fn issue_1685_user_turn(
    content: &str,
    auto_retry: bool,
    retry_attempts: u8,
) -> PendingRemoteMessage {
    PendingRemoteMessage {
        content: content.to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry,
        retry_attempts,
        retry_at: None,
    }
}

fn issue_1685_error(
    id: u64,
    message: &str,
    retry_after_secs: Option<u64>,
) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::Error {
        id,
        message: message.to_string(),
        retry_after_secs,
    }
}

#[test]
fn issue_1685_openference_json_rate_limit_uses_retry_after_seconds_not_max_rpm() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.rate_limit_pending_message = Some(issue_1685_user_turn("continue the adapter", false, 0));
    app.is_processing = true;
    app.current_message_id = Some(41);

    app.handle_server_event(
        issue_1685_error(
            41,
            "OpenAI-compatible chat request failed\n  status: 429 Too Many Requests\n  response: {\"error\":\"Rate limit exceeded. Too many requests per minute.\",\"type\":\"rate_limit_error\",\"code\":\"rate_limit_exceeded\",\"retry_after_seconds\":2,\"max_rpm\":25}",
            None,
        ),
        &mut remote,
    );

    assert!(!app.is_processing);
    let held = app.rate_limit_pending_message.as_ref().expect("turn held");
    assert_eq!(held.retry_attempts, 1);
    let wait = app
        .rate_limit_reset
        .expect("retry scheduled")
        .saturating_duration_since(std::time::Instant::now());
    assert!(wait <= std::time::Duration::from_secs(2), "{wait:?}");
    assert!(wait > std::time::Duration::from_secs(1), "{wait:?}");
}

#[test]
fn issue_1685_stale_rate_limit_stops_after_bounded_resumes() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.auto_poke_incomplete_todos = true;
    app.rate_limit_pending_message = Some(issue_1685_user_turn("continue the adapter", false, 0));
    let error = "429 Too Many Requests: rate limit exceeded, retry after 30 seconds";

    for attempt in 1..=App::AUTO_RETRY_MAX_ATTEMPTS {
        app.last_submitted_input = Some("continue the adapter".to_string());
        app.is_processing = true;
        app.handle_server_event(
            issue_1685_error(u64::from(attempt), error, None),
            &mut remote,
        );
        let pending = app
            .rate_limit_pending_message
            .as_ref()
            .expect("held within budget");
        assert_eq!(pending.retry_attempts, attempt);
        assert!(app.rate_limit_reset.is_some());
    }

    app.is_processing = true;
    app.handle_server_event(issue_1685_error(99, error, None), &mut remote);
    assert!(
        app.rate_limit_pending_message.is_none(),
        "no further automatic resend"
    );
    assert!(app.rate_limit_reset.is_none());
    assert!(
        !app.auto_poke_incomplete_todos,
        "auto-poke must not restart the loop"
    );
    assert_eq!(app.input, "continue the adapter");
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.contains("Rate-limit retry limit reached"))
    );
}

#[test]
fn issue_1685_provider_dns_failure_has_bounded_resumes() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.auto_poke_incomplete_todos = true;
    app.auto_poke_default_on = true;
    app.rate_limit_pending_message = Some(issue_1685_user_turn("continue the adapter", false, 0));
    let error = "Failed to send OpenAI-compatible chat request: client error (Connect): dns error: failed to lookup address information: nodename nor servname provided, or not known";

    for attempt in 1..=App::AUTO_RETRY_MAX_ATTEMPTS {
        app.last_submitted_input = Some("continue the adapter".to_string());
        app.is_processing = true;
        app.handle_server_event(
            issue_1685_error(u64::from(attempt), error, None),
            &mut remote,
        );
        let pending = app
            .rate_limit_pending_message
            .as_ref()
            .expect("held within budget");
        assert_eq!(pending.retry_attempts, attempt);
        assert!(matches!(
            app.status,
            ProcessingStatus::WaitingForNetwork { .. }
        ));
        let wait = app
            .rate_limit_reset
            .unwrap()
            .saturating_duration_since(std::time::Instant::now());
        assert!(wait <= std::time::Duration::from_secs(5 * u64::from(attempt)));
    }

    app.is_processing = true;
    app.handle_server_event(issue_1685_error(99, error, None), &mut remote);
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.rate_limit_reset.is_none());
    assert!(!app.auto_poke_incomplete_todos);
    assert!(!app.auto_poke_default_on);
    assert_eq!(app.input, "continue the adapter");
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.contains("Connection retry limit reached"))
    );
}

#[test]
fn issue_1685_offline_probes_do_not_consume_retry_budget() {
    let mut app = create_test_app();
    app.rate_limit_pending_message = Some(issue_1685_user_turn("continue", true, 2));
    for _ in 0..10 {
        assert!(app.schedule_pending_remote_network_wait("network probe still failing"));
    }
    assert_eq!(
        app.rate_limit_pending_message
            .as_ref()
            .unwrap()
            .retry_attempts,
        2
    );
}

#[test]
fn issue_1685_huge_server_retry_hint_is_clamped() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.rate_limit_pending_message = Some(issue_1685_user_turn("continue", false, 0));
    app.handle_server_event(
        issue_1685_error(7, "rate limited", Some(u64::MAX)),
        &mut remote,
    );
    let wait = app
        .rate_limit_reset
        .expect("retry scheduled")
        .saturating_duration_since(std::time::Instant::now());
    assert!(wait <= std::time::Duration::from_secs(24 * 60 * 60));
    assert_eq!(
        app.rate_limit_pending_message
            .as_ref()
            .unwrap()
            .retry_attempts,
        1
    );
}
