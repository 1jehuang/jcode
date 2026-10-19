#[test]
fn test_service_tier_auto_reports_do_not_flood_or_consume_confirmation() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let before = app.display_messages().len();
    for _ in 0..10 {
        for tier in ["auto", "default"] {
            app.handle_server_event(
                crate::protocol::ServerEvent::ServiceTierReported {
                    requested: None,
                    tier: tier.to_string(),
                },
                &mut remote,
            );
        }
    }
    assert_eq!(app.display_messages().len(), before);
    assert_eq!(
        app.remote_reported_service_tier,
        Some((None, "default".to_string()))
    );
    app.remote_service_tier = Some("priority".to_string());
    app.remote_reported_service_tier = None;
    app.remote_service_tier_confirmation_pending = true;
    app.handle_server_event(
        crate::protocol::ServerEvent::ServiceTierReported {
            requested: Some("priority".to_string()),
            tier: "auto".to_string(),
        },
        &mut remote,
    );
    assert!(app.remote_service_tier_confirmation_pending);
    assert!(app.remote_reported_service_tier.is_none());
    assert_eq!(app.display_messages().len(), before);
    app.handle_server_event(
        crate::protocol::ServerEvent::ServiceTierReported {
            requested: Some("priority".to_string()),
            tier: "default".to_string(),
        },
        &mut remote,
    );
    assert_eq!(app.display_messages().len(), before + 1);
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("Requested priority")
    );
}

#[test]
fn test_service_tier_reports_are_quiet_without_explicit_change() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.remote_service_tier = Some("ultrafast".into());
    let before = app.display_messages().len();
    for tier in ["default", "ultrafast", "default"] {
        // A new turn can clear the previously reported tier.
        app.remote_reported_service_tier = None;
        app.handle_server_event(
            crate::protocol::ServerEvent::ServiceTierReported {
                requested: Some("ultrafast".into()),
                tier: tier.into(),
            },
            &mut remote,
        );
        assert_eq!(app.display_messages().len(), before);
        assert_eq!(app.remote_reported_service_tier.as_ref().unwrap().1, tier);
    }
    // An explicit request must be confirmed even if the report is unchanged.
    app.remote_service_tier_confirmation_pending = true;
    for _ in 0..2 {
        app.handle_server_event(
            crate::protocol::ServerEvent::ServiceTierReported {
                requested: Some("ultrafast".into()),
                tier: "default".into(),
            },
            &mut remote,
        );
    }
    assert_eq!(app.display_messages().len(), before + 1);
    assert!(!app.remote_service_tier_confirmation_pending);
}

#[test]
fn test_service_tier_duplicate_acknowledgements_do_not_flood_transcript() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let event = |id, tier: Option<&str>, error: Option<&str>| {
        serde_json::from_value(serde_json::json!({
            "type": "service_tier_changed", "id": id,
            "service_tier": tier, "error": error
        }))
        .unwrap()
    };
    for processing in [false, true] {
        let mut app = create_test_app();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        app.is_processing = processing;
        let before = app.display_messages().len();
        for id in 1..=10 {
            app.handle_server_event(event(id, Some("priority"), None), &mut remote);
        }
        assert_eq!(app.display_messages().len(), before + 1);
        assert_eq!(app.remote_service_tier.as_deref(), Some("priority"));
        assert!(app.status_notice().is_some());
        // Actual transitions still get notices, including returning to Fast.
        app.handle_server_event(event(11, None, None), &mut remote);
        app.handle_server_event(event(12, Some("priority"), None), &mut remote);
        assert_eq!(app.display_messages().len(), before + 3);
        // Failures remain visible and do not change the configured tier.
        app.handle_server_event(event(13, None, Some("rejected")), &mut remote);
        assert_eq!(app.display_messages().len(), before + 4);
        assert_eq!(app.remote_service_tier.as_deref(), Some("priority"));
        assert!(
            app.display_messages()
                .last()
                .unwrap()
                .content
                .contains("rejected")
        );
    }
}
