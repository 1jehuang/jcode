use super::*;

#[test]
fn ndjson_preserves_provider_reported_service_tier_without_inferring_it() {
    let mut state = NdjsonRunState::default();
    assert_eq!(state.served_service_tier, None);
    let mut output = Vec::new();
    for (requested, served) in [
        (Some("ultrafast"), "default"),
        (Some("priority"), "priority"),
        (None, "default"),
    ] {
        emit_ndjson_event(
            &mut output,
            &mut state,
            crate::protocol::ServerEvent::ServiceTierReported {
                requested: requested.map(str::to_owned),
                tier: served.to_owned(),
            },
        )
        .expect("emit tier metadata");
        assert_eq!(state.requested_service_tier.as_deref(), requested);
        assert_eq!(state.served_service_tier.as_deref(), Some(served));
    }
    let events: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        serde_json::json!({
            "type": "service_tier", "requested": "ultrafast", "tier": "default",
            "report_status": "reported_mismatch"
        })
    );
    assert!(events[2]["requested"].is_null());
}

#[test]
fn ndjson_service_tier_status_never_infers_confirmation() {
    for (requested, reported, expected) in [
        (None, None, "not_reported"),
        (Some("ultrafast"), Some("auto"), "unresolved"),
        (Some("ultrafast"), Some("default"), "reported_mismatch"),
        (Some("ultrafast"), Some("ultrafast"), "reported_match"),
        (Some("fast"), Some("priority"), "reported_match"),
        (Some("off"), Some("standard"), "reported_match"),
        (None, Some("default"), "reported_match"),
        (Some("flex"), Some("default"), "reported_mismatch"),
        (Some("future-tier"), Some("default"), "reported_mismatch"),
    ] {
        let state = NdjsonRunState {
            requested_service_tier: requested.map(str::to_string),
            served_service_tier: reported.map(str::to_string),
            ..Default::default()
        };
        assert_eq!(state.service_tier_report_status(), expected);
        assert!(!state.service_tier_report_status().contains("confirmed"));
    }
}

#[test]
fn server_reload_report_preserves_json_fields_and_exit_status_contract() {
    let cases = [
        (false, false, false, false),
        (true, true, true, false),
        (true, false, true, false),
        (true, false, false, true),
    ];

    for (had_listener, already_current, handoff_ready, should_fail) in cases {
        let report = ServerReloadReport {
            socket: "/tmp/jcode.sock".to_string(),
            had_listener,
            forced: false,
            reloaded: had_listener && !already_current,
            already_current,
            handoff_ready,
            detail: "test reload outcome".to_string(),
        };

        let json = serde_json::to_value(&report).expect("serialize reload report");
        assert_eq!(json["had_listener"], had_listener);
        assert_eq!(json["already_current"], already_current);
        assert_eq!(json["handoff_ready"], handoff_ready);

        let result = validate_server_reload_report(&report);
        assert_eq!(result.is_err(), should_fail, "report: {json}");
        if should_fail {
            assert!(
                result
                    .expect_err("not-ready handoff must fail")
                    .to_string()
                    .contains("never became ready")
            );
        }
    }
}

#[tokio::test]
async fn server_reload_without_listener_is_a_successful_json_noop() {
    let _env_lock = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME", "JCODE_SOCKET"]);
    let home = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", home.path());
    crate::env::set_var("JCODE_SOCKET", home.path().join("isolated.sock"));

    let mut output = Vec::new();
    let result = run_server_reload_command_to(false, true, &mut output).await;

    result.expect("reload without a listener must be a successful no-op");
    let report: serde_json::Value =
        serde_json::from_slice(&output).expect("reload --json must emit one JSON report");
    assert_eq!(report["had_listener"], false);
    assert_eq!(report["already_current"], false);
    assert_eq!(report["handoff_ready"], false);
    assert_eq!(report["reloaded"], false);
    assert_eq!(report["forced"], false);
    assert!(
        report["detail"]
            .as_str()
            .is_some_and(|detail| { detail.contains("No running jcode server found") })
    );
}
