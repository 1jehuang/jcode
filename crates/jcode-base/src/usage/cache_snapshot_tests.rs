use super::*;

#[test]
fn account_snapshot_preserves_all_model_and_future_provider_windows() {
    let data = UsageData {
        model_scoped: vec![
            super::super::ModelScopedUsageWindow {
                model_name: "Fable".into(),
                utilization: 0.73,
                resets_at: None,
            },
            super::super::ModelScopedUsageWindow {
                model_name: "Future Zenith 界".into(),
                utilization: 0.61,
                resets_at: None,
            },
        ],
        seven_day_opus: Some(0.28),
        ..Default::default()
    };
    let snapshot = anthropic_snapshot_from_usage("models".into(), None, &data);
    assert_eq!(snapshot.additional_windows.len(), 3);
    assert_eq!(snapshot.additional_windows[0].label, "Fable 7d");
    assert_eq!(snapshot.additional_windows[0].usage_ratio, 0.73);
    assert_eq!(snapshot.additional_windows[1].label, "Future Zenith 界 7d");
    assert_eq!(snapshot.additional_windows[1].usage_ratio, 0.61);
    assert_eq!(snapshot.additional_windows[2].label, "Opus 7d");
    assert!(snapshot.summary().contains("Fable 7d 73%"));
    assert!(snapshot.summary().contains("Future Zenith 界 7d 61%"));

    let limits = [
        "5-hour",
        "7-day",
        "Codex Spark",
        "Future monthly",
        "Future daily",
    ]
    .into_iter()
    .map(|name| super::super::UsageLimit {
        name: name.into(),
        usage_percent: 32.0,
        resets_at: None,
    })
    .collect::<Vec<_>>();
    let usage = classify_openai_limits(&limits);
    assert_eq!(usage.additional_windows.len(), 2);
    let snapshot = openai_snapshot_from_usage("future".into(), None, &usage);
    assert_eq!(snapshot.additional_windows.len(), 3);
    assert_eq!(snapshot.additional_windows[0].label, "Codex Spark");
    assert_eq!(snapshot.additional_windows[1].label, "Future monthly");
    assert_eq!(snapshot.additional_windows[2].label, "Future daily");
    assert!(
        snapshot
            .additional_windows
            .iter()
            .all(|window| window.usage_ratio == 0.32)
    );
}

fn last_good() -> UsageData {
    UsageData {
        five_hour: 1.0,
        seven_day: 0.42,
        five_hour_resets_at: Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()),
        seven_day_resets_at: Some((chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339()),
        fetched_at: Some(Instant::now()),
        last_success_at: Some(Instant::now()),
        last_error: Some("HTTP 429".into()),
        ..Default::default()
    }
}

#[test]
fn stale_limits_preserve_error_but_never_exhaust_or_route() {
    let data = last_good();
    let snapshot = anthropic_snapshot_from_usage("stale-test".into(), None, &data);
    assert_eq!(snapshot.five_hour_ratio, Some(1.0));
    assert_eq!(snapshot.seven_day_ratio, Some(0.42));
    assert_eq!(snapshot.error.as_deref(), Some("HTTP 429"));
    assert!(!snapshot.exhausted);
}

#[test]
fn additional_model_windows_filter_expiry_invalid_values_and_unproven_stale_data() {
    let mut data = last_good();
    let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    data.model_scoped = vec![
        super::super::ModelScopedUsageWindow {
            model_name: "Fable".into(),
            utilization: 0.8,
            resets_at: Some(future),
        },
        super::super::ModelScopedUsageWindow {
            model_name: "Expired".into(),
            utilization: 1.0,
            resets_at: Some(past),
        },
        super::super::ModelScopedUsageWindow {
            model_name: "Invalid".into(),
            utilization: f32::NAN,
            resets_at: None,
        },
    ];
    let snapshot = anthropic_snapshot_from_usage("test".into(), None, &data);
    assert_eq!(snapshot.additional_windows.len(), 1);
    assert_eq!(snapshot.additional_windows[0].label, "Fable 7d");
    assert!(!snapshot.exhausted);
    data.last_success_at = None;
    assert!(
        anthropic_snapshot_from_usage("test".into(), None, &data)
            .additional_windows
            .is_empty()
    );
}

#[test]
fn stale_limits_require_provenance_and_each_unexpired_valid_window() {
    let snapshot = |data: &UsageData| anthropic_snapshot_from_usage("test".into(), None, data);
    let mut data = last_good();
    data.last_success_at = None;
    assert!(snapshot(&data).five_hour_ratio.is_none());
    assert!(snapshot(&data).seven_day_ratio.is_none());
    data = last_good();
    data.fetched_at = None;
    assert!(snapshot(&data).five_hour_ratio.is_none());
    data = last_good();
    for reset in [
        None,
        Some("invalid".into()),
        Some((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()),
    ] {
        data.five_hour_resets_at = reset;
        assert!(snapshot(&data).five_hour_ratio.is_none());
        assert_eq!(snapshot(&data).seven_day_ratio, Some(0.42));
        assert_eq!(snapshot(&data).resets_at, data.seven_day_resets_at);
    }
    data = last_good();
    for ratio in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        data.five_hour = ratio;
        assert!(snapshot(&data).five_hour_ratio.is_none());
    }
}

#[test]
fn successful_but_rolled_over_window_does_not_exhaust() {
    let mut data = last_good();
    data.last_error = None;
    data.five_hour_resets_at =
        Some((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
    let snapshot = anthropic_snapshot_from_usage("test".into(), None, &data);
    assert!(snapshot.five_hour_ratio.is_none());
    assert_eq!(snapshot.seven_day_ratio, Some(0.42));
    assert_eq!(snapshot.resets_at, data.seven_day_resets_at);
    assert!(!snapshot.exhausted);
    assert!(snapshot.error.is_none());
}
