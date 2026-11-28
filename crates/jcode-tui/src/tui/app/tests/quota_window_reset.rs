#[test]
fn test_remote_quota_window_reset_reaches_the_countdown() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let now = chrono::Utc::now();
    for (reset, expected_wait) in [
        (
            Some((now + chrono::Duration::hours(1)).to_rfc3339()),
            Some(3615),
        ),
        (
            Some((now + chrono::Duration::days(1)).to_rfc3339()),
            Some(86400),
        ),
        (Some((now + chrono::Duration::days(2)).to_rfc3339()), None),
        (Some("tomorrow".to_string()), None),
        (None, None),
    ] {
        let mut app = create_test_app();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            jarvis_voice: false,
            content: "held prompt".to_string(),
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
        let body = serde_json::json!({
            "error": "Request limit exceeded (1500 per 5 hours)",
            "type": "insufficient_quota",
            "resets_at": reset,
        });
        app.handle_server_event(
            crate::protocol::ServerEvent::Error {
                id: 9,
                message: format!("status: 402 Payment Required response: {body}"),
                // A short hint must not turn a permanent billing failure into a hold.
                retry_after_secs: Some(3),
            },
            &mut remote,
        );
        assert!(!app.is_processing);
        match expected_wait {
            Some(seconds) => {
                let wait = app
                    .rate_limit_reset
                    .expect("window quota should start a countdown")
                    .saturating_duration_since(Instant::now())
                    .as_secs();
                assert!((seconds - 5..=seconds).contains(&wait), "{reset:?}: {wait}");
                let pending = app
                    .rate_limit_pending_message
                    .as_ref()
                    .expect("held prompt");
                assert_eq!(pending.content, "held prompt");
                assert_eq!(pending.retry_attempts, 1);
            }
            None => assert!(app.rate_limit_reset.is_none(), "{reset:?}"),
        }
    }
}
