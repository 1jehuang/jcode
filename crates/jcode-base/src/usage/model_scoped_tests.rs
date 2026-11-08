use super::*;

#[test]
fn anthropic_model_scoped_fable_limit_survives_report_cache_roundtrip() {
    let usage = UsageData {
        five_hour: 0.1,
        seven_day: 0.2,
        model_scoped: vec![ModelScopedUsageWindow {
            model_name: "Fable".to_string(),
            utilization: 0.73,
            resets_at: Some("2026-08-10T00:00:00Z".to_string()),
        }],
        ..Default::default()
    };

    let report = provider_report_from_usage_data("Anthropic".to_string(), &usage);
    let fable = report
        .limits
        .iter()
        .find(|limit| limit.name == "7-day Fable window")
        .expect("Fable limit should be displayed");
    assert_eq!(fable.usage_percent, 73.0);
    assert_eq!(fable.resets_at.as_deref(), Some("2026-08-10T00:00:00Z"));

    let restored = usage_data_from_provider_report(&report);
    assert_eq!(restored.model_scoped, usage.model_scoped);
}

#[test]
fn anthropic_usage_response_deserializes_structured_fable_limit() {
    let response: UsageResponse = serde_json::from_str(
        r#"{
            "five_hour": {"utilization": 12.0, "resets_at": "2026-08-04T00:00:00Z"},
            "seven_day": {"utilization": 34.0, "resets_at": "2026-08-10T00:00:00Z"},
            "limits": [{
                "kind": "weekly_scoped",
                "percent": 56.0,
                "resets_at": "2026-08-11T00:00:00Z",
                "scope": {"model": {"display_name": "Fable"}}
            }]
        }"#,
    )
    .expect("structured Anthropic usage payload should deserialize");

    let fable = response.limits.first().expect("model-scoped limit");
    assert_eq!(fable.kind.as_deref(), Some("weekly_scoped"));
    assert_eq!(fable.percent, Some(56.0));
    assert_eq!(
        fable
            .scope
            .as_ref()
            .and_then(|scope| scope.model.as_ref())
            .and_then(|model| model.display_name.as_deref()),
        Some("Fable")
    );
}

#[test]
fn anthropic_model_scoped_exhaustion_matches_display_name_to_catalog_id() {
    let mut usage = UsageData {
        model_scoped: vec![ModelScopedUsageWindow {
            model_name: "Fable".to_string(),
            utilization: 1.0,
            resets_at: Some("2999-08-11T00:00:00Z".to_string()),
        }],
        ..Default::default()
    };

    assert!(usage.model_scoped_exhausted("claude-fable-5"));
    assert!(!usage.model_scoped_exhausted("claude-opus-5"));
    usage.last_error = Some("Usage API error (429)".into());
    assert!(!usage.model_scoped_exhausted("claude-fable-5"));
    usage.last_error = None;

    usage.model_scoped[0].resets_at = Some("2020-01-01T00:00:00Z".into());
    usage.refresh_after = Some(Instant::now() + Duration::from_secs(3600));
    assert!(!usage.is_stale());
    assert!(!usage.model_scoped_exhausted("claude-fable-5"));

    let below_limit = UsageData {
        model_scoped: vec![ModelScopedUsageWindow {
            model_name: "Claude Fable 5".to_string(),
            utilization: 0.98,
            resets_at: None,
        }],
        ..Default::default()
    };
    assert!(!below_limit.model_scoped_exhausted("claude-fable-5"));
}
