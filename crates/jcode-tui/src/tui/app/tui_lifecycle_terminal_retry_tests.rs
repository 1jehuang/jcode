use super::*;

#[test]
fn recovered_images_keep_stable_markers_with_existing_attachments() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let old = ("image/png".to_string(), "old-image".to_string());
    let newer = ("image/png".to_string(), "newer-image".to_string());
    for has_newer_draft in [true, false] {
        let mut app = super::super::tests::create_test_app();
        app.pending_images = vec![newer.clone(), old.clone()];
        if has_newer_draft {
            app.input = "New draft [image 1]".to_string();
        }
        let payload = super::super::FallbackResendPayload {
            content: "Old [image 1] and [image 2]".to_string(),
            images: vec![old.clone(), newer.clone()],
            is_system: false,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: 1,
            raw_input: None,
        };
        app.restore_failover_payload(payload.clone());
        app.sync_model_picker_preview_from_input();
        let expected = if has_newer_draft {
            "New draft [image 1] [image 2]"
        } else {
            "Old [image 2] and [image 1]"
        };
        assert_eq!(app.input, expected);
        assert_eq!(app.pending_images, vec![newer.clone(), old.clone()]);
        app.restore_failover_payload(payload);
        app.sync_model_picker_preview_from_input();
        assert_eq!(app.input, expected, "repeated restoration is idempotent");
        assert_eq!(app.pending_images, vec![newer.clone(), old.clone()]);
    }
}

#[test]
fn terminal_remote_retry_stops_preserve_user_attachments() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    for stop in ["budget", "pause", "credential"] {
        let mut app = super::super::tests::create_test_app();
        let image = ("image/png".to_string(), "original-image".to_string());
        app.last_submitted_input = Some("Inspect this screenshot".to_string());
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "Expanded screenshot instructions".to_string(),
            images: vec![image.clone()],
            is_system: false,
            system_reminder: None,
            auto_retry: true,
            retry_attempts: App::AUTO_RETRY_MAX_ATTEMPTS,
            retry_at: Some(Instant::now()),
        });
        app.rate_limit_reset = Some(Instant::now());
        match stop {
            "budget" => assert!(!app.schedule_pending_remote_retry("interrupted")),
            "pause" => app.pause_exhausted_auto_resume("Paused".to_string(), "Paused"),
            "credential" => app.trip_credential_failure_breaker("401 unauthorized"),
            _ => unreachable!(),
        }
        assert_eq!(app.input, "Inspect this screenshot [image 1]", "{stop}");
        assert_eq!(app.pending_images, vec![image], "{stop}");
        assert!(app.rate_limit_pending_message.is_none(), "{stop}");
        assert!(app.rate_limit_reset.is_none(), "{stop}");
    }
}

#[test]
fn terminal_remote_retry_preserves_draft_and_deduplicates_images() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = super::super::tests::create_test_app();
    let image = ("image/png".to_string(), "original-image".to_string());
    app.input = "Keep my new draft [image 1]".to_string();
    app.last_submitted_input = Some("Failed prompt".to_string());
    app.pending_images.push(image.clone());
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "Expanded failed prompt".to_string(),
        images: vec![image.clone()],
        is_system: false,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: App::AUTO_RETRY_MAX_ATTEMPTS,
        retry_at: None,
    });
    app.pause_exhausted_auto_resume("Paused".to_string(), "Paused");
    assert_eq!(app.input, "Keep my new draft [image 1]");
    assert_eq!(app.pending_images, vec![image]);
    assert!(app.display_messages().iter().any(|message| {
        message.content.contains("Failed prompt")
            && message.content.contains("draft text is preserved")
    }));
}
