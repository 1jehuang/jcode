use super::*;

#[test]
fn test_usage_data_default() {
    let data = UsageData::default();
    assert!(data.is_stale());
    assert_eq!(data.five_hour_percent(), "0%");
    assert_eq!(data.seven_day_percent(), "0%");
}

#[test]
fn test_usage_percent_format() {
    let data = UsageData {
        five_hour: 0.42,
        seven_day: 0.156,
        ..Default::default()
    };
    assert_eq!(data.five_hour_percent(), "42%");
    assert_eq!(data.seven_day_percent(), "16%");
}

#[test]
fn test_humanize_key() {
    assert_eq!(humanize_key("five_hour"), "Five Hour");
    assert_eq!(humanize_key("seven_day_opus"), "Seven Day Opus");
    assert_eq!(humanize_key("plan"), "Plan");
}

#[test]
fn test_get_sync_without_runtime_does_not_panic() {
    let result = std::panic::catch_unwind(get_sync);
    assert!(
        result.is_ok(),
        "get_sync should not require a Tokio runtime"
    );
}

#[test]
fn test_get_openai_usage_sync_without_runtime_does_not_panic() {
    let result = std::panic::catch_unwind(get_openai_usage_sync);
    assert!(
        result.is_ok(),
        "get_openai_usage_sync should not require a Tokio runtime"
    );
}

#[test]
fn test_usage_data_becomes_stale_when_reset_time_has_passed() {
    let data = UsageData {
        five_hour: 0.42,
        five_hour_resets_at: Some("2020-01-01T00:00:00Z".to_string()),
        fetched_at: Some(Instant::now()),
        ..Default::default()
    };

    assert!(
        data.is_stale(),
        "usage data should refresh once a reset window has passed"
    );
}

#[test]
fn test_openai_usage_data_becomes_stale_when_reset_time_has_passed() {
    let data = OpenAIUsageData {
        five_hour: Some(OpenAIUsageWindow {
            name: "5-hour".to_string(),
            usage_ratio: 0.42,
            resets_at: Some("2020-01-01T00:00:00Z".to_string()),
        }),
        fetched_at: Some(Instant::now()),
        ..Default::default()
    };

    assert!(
        data.is_stale(),
        "OpenAI usage data should refresh once a reset window has passed"
    );
}

#[test]
fn test_usage_data_display_snapshot_clears_passed_reset_window() {
    let data = UsageData {
        five_hour: 0.73,
        five_hour_resets_at: Some("2020-01-01T00:00:00Z".to_string()),
        seven_day: 0.41,
        seven_day_resets_at: Some("3020-01-01T00:00:00Z".to_string()),
        fetched_at: Some(Instant::now()),
        ..Default::default()
    };

    let snapshot = data.display_snapshot();
    assert_eq!(snapshot.five_hour, 0.0);
    assert!(snapshot.five_hour_resets_at.is_none());
    assert_eq!(snapshot.seven_day, 0.41);
    assert_eq!(
        snapshot.seven_day_resets_at.as_deref(),
        Some("3020-01-01T00:00:00Z")
    );
}

#[test]
fn test_openai_usage_data_display_snapshot_clears_passed_reset_window() {
    let data = OpenAIUsageData {
        five_hour: Some(OpenAIUsageWindow {
            name: "5-hour".to_string(),
            usage_ratio: 0.88,
            resets_at: Some("2020-01-01T00:00:00Z".to_string()),
        }),
        seven_day: Some(OpenAIUsageWindow {
            name: "7-day".to_string(),
            usage_ratio: 0.31,
            resets_at: Some("3020-01-01T00:00:00Z".to_string()),
        }),
        hard_limit_reached: true,
        fetched_at: Some(Instant::now()),
        ..Default::default()
    };

    let snapshot = data.display_snapshot();
    assert_eq!(
        snapshot.five_hour.as_ref().map(|w| w.usage_ratio),
        Some(0.0)
    );
    assert_eq!(
        snapshot
            .five_hour
            .as_ref()
            .and_then(|w| w.resets_at.as_deref()),
        None
    );
    assert_eq!(
        snapshot.seven_day.as_ref().map(|w| w.usage_ratio),
        Some(0.31)
    );
    assert!(!snapshot.hard_limit_reached);
}

#[test]
fn test_provider_usage_cache_is_not_fresh_after_reset_boundary() {
    let report = ProviderUsage {
        provider_name: "OpenAI".to_string(),
        limits: vec![UsageLimit {
            name: "5-hour window".to_string(),
            usage_percent: 100.0,
            resets_at: Some("2020-01-01T00:00:00Z".to_string()),
        }],
        ..Default::default()
    };

    assert!(!provider_usage_cache_is_fresh(
        Instant::now(),
        Instant::now(),
        &report,
    ));
}

#[test]
fn test_mask_email_censors_local_part() {
    assert_eq!(mask_email("jeremyh1@uw.edu"), "j***1@uw.edu");
    assert_eq!(mask_email("ab@example.com"), "a*@example.com");
}

#[test]
fn test_format_usage_bar() {
    let bar = format_usage_bar(50.0, 10);
    assert!(bar.contains("█████░░░░░"));
    assert!(bar.contains("50%"));

    let bar = format_usage_bar(0.0, 10);
    assert!(bar.contains("░░░░░░░░░░"));
    assert!(bar.contains("0%"));

    let bar = format_usage_bar(100.0, 10);
    assert!(bar.contains("██████████"));
    assert!(bar.contains("100%"));
}

#[test]
fn test_format_reset_time_past() {
    assert_eq!(format_reset_time("2020-01-01T00:00:00Z"), "now");
}

#[test]
fn test_format_reset_time_under_one_minute_rounds_up() {
    let timestamp = (chrono::Utc::now() + chrono::TimeDelta::seconds(30)).to_rfc3339();
    assert_eq!(format_reset_time(&timestamp), "1m");
}

#[test]
fn test_format_reset_time_uses_days_for_long_windows() {
    let timestamp =
        (chrono::Utc::now() + chrono::TimeDelta::hours(109) + chrono::TimeDelta::minutes(5))
            .to_rfc3339();
    assert_eq!(format_reset_time(&timestamp), "4d 13h");
}

#[test]
fn test_classify_openai_limits_recognizes_five_weekly_and_spark() {
    let limits = vec![
        UsageLimit {
            name: "Codex 5h".to_string(),
            usage_percent: 25.0,
            resets_at: Some("2026-01-01T00:00:00Z".to_string()),
        },
        UsageLimit {
            name: "Codex 1w".to_string(),
            usage_percent: 50.0,
            resets_at: Some("2026-01-07T00:00:00Z".to_string()),
        },
        UsageLimit {
            name: "Codex Spark".to_string(),
            usage_percent: 75.0,
            resets_at: Some("2026-01-02T00:00:00Z".to_string()),
        },
    ];

    let classified = openai_helpers::classify_openai_limits(&limits);

    assert_eq!(
        classified.five_hour.as_ref().map(|w| w.usage_ratio),
        Some(0.25)
    );
    assert_eq!(
        classified.seven_day.as_ref().map(|w| w.usage_ratio),
        Some(0.5)
    );
    assert_eq!(classified.spark.as_ref().map(|w| w.usage_ratio), Some(0.75));
}

#[test]
fn test_parse_usage_percent_supports_used_limit_shape() {
    let mut obj = serde_json::Map::new();
    obj.insert("used".to_string(), serde_json::json!(20));
    obj.insert("limit".to_string(), serde_json::json!(80));

    let percent = openai_helpers::parse_usage_percent_from_obj(&obj);
    assert_eq!(percent, Some(25.0));
}

#[test]
fn test_parse_usage_percent_supports_remaining_limit_shape() {
    let mut obj = serde_json::Map::new();
    obj.insert("remaining".to_string(), serde_json::json!(60));
    obj.insert("limit".to_string(), serde_json::json!(80));

    let percent = openai_helpers::parse_usage_percent_from_obj(&obj);
    assert_eq!(percent, Some(25.0));
}

#[test]
fn test_active_anthropic_usage_report_prefers_marked_account() {
    let results = vec![
        ProviderUsage {
            provider_name: "Anthropic - work".to_string(),
            ..Default::default()
        },
        ProviderUsage {
            provider_name: "Anthropic - personal ✦".to_string(),
            ..Default::default()
        },
    ];

    let active = active_anthropic_usage_report(&results)
        .expect("expected active anthropic report to be selected");
    assert_eq!(active.provider_name, "Anthropic - personal ✦");
}

#[test]
fn test_usage_data_from_provider_report_maps_limits_and_extra_usage() {
    let report = ProviderUsage {
        provider_name: "Anthropic (Claude)".to_string(),
        limits: vec![
            UsageLimit {
                name: "5-hour window".to_string(),
                usage_percent: 25.0,
                resets_at: Some("2026-01-01T00:00:00Z".to_string()),
            },
            UsageLimit {
                name: "7-day window".to_string(),
                usage_percent: 50.0,
                resets_at: Some("2026-01-07T00:00:00Z".to_string()),
            },
            UsageLimit {
                name: "7-day Opus window".to_string(),
                usage_percent: 75.0,
                resets_at: Some("2026-01-08T00:00:00Z".to_string()),
            },
        ],
        extra_info: vec![(
            "Extra usage (long context)".to_string(),
            "enabled".to_string(),
        )],
        hard_limit_reached: false,
        error: None,
    };

    let usage = usage_data_from_provider_report(&report);

    assert_eq!(usage.five_hour, 0.25);
    assert_eq!(usage.seven_day, 0.5);
    assert_eq!(usage.seven_day_opus, Some(0.75));
    assert!(usage.extra_usage_enabled);
    assert_eq!(
        usage.five_hour_resets_at.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
}

#[test]
fn test_openai_usage_data_from_provider_report_preserves_error() {
    let report = ProviderUsage {
        provider_name: "OpenAI (ChatGPT)".to_string(),
        error: Some("API error (401 Unauthorized)".to_string()),
        ..Default::default()
    };

    let usage = openai_usage_data_from_provider_report(&report);

    assert_eq!(
        usage.last_error.as_deref(),
        Some("API error (401 Unauthorized)")
    );
    assert!(usage.five_hour.is_none());
    assert!(usage.seven_day.is_none());
}

#[test]
fn test_openai_usage_data_from_provider_report_preserves_hard_limit_flag() {
    let report = ProviderUsage {
        provider_name: "OpenAI (ChatGPT)".to_string(),
        hard_limit_reached: true,
        limits: vec![UsageLimit {
            name: "5-hour window".to_string(),
            usage_percent: 100.0,
            resets_at: None,
        }],
        ..Default::default()
    };

    let usage = openai_usage_data_from_provider_report(&report);

    assert!(usage.hard_limit_reached);
}

#[test]
fn test_openai_snapshot_does_not_treat_hard_limit_flag_as_exhausted() {
    let usage = OpenAIUsageData {
        hard_limit_reached: true,
        five_hour: Some(OpenAIUsageWindow {
            name: "5-hour window".to_string(),
            usage_ratio: 1.0,
            resets_at: Some("2026-01-01T00:00:00Z".to_string()),
        }),
        ..Default::default()
    };

    let snapshot = openai_snapshot_from_usage(
        "work".to_string(),
        Some("work@example.com".to_string()),
        &usage,
    );

    assert!(!snapshot.exhausted);
    assert_eq!(snapshot.five_hour_ratio, Some(1.0));
    assert_eq!(snapshot.seven_day_ratio, None);
}

#[test]
fn test_parse_openai_hard_limit_reached_detects_rate_limit_denials() {
    let json = serde_json::json!({
        "plan_type": "free",
        "rate_limit": {
            "allowed": false,
            "primary_window": {
                "used_percent": 100.0,
                "reset_at": 1_766_000_000
            }
        },
        "limit_reached": true
    });

    assert!(openai_helpers::parse_openai_hard_limit_reached(&json));
}

#[test]
fn test_parse_openai_hard_limit_reached_ignores_unrelated_allowed_flags() {
    let json = serde_json::json!({
        "plan_type": "free",
        "features": {
            "voice_mode": {
                "allowed": false
            }
        },
        "rate_limit": {
            "allowed": true
        }
    });

    assert!(!openai_helpers::parse_openai_hard_limit_reached(&json));
}

#[test]
fn test_parse_openai_usage_payload_prefers_wham_windows_and_additional_limits() {
    let json = serde_json::json!({
        "plan_type": "pro",
        "rate_limit": {
            "allowed": true,
            "primary_window": {
                "used_percent": 25.0,
                "reset_at": 1_766_000_000
            },
            "secondary_window": {
                "used_percent": 50.0,
                "reset_at": 1_766_086_400
            }
        },
        "additional_rate_limits": [{
            "limit_name": "Codex Spark",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 75.0,
                    "reset_at": 1_766_000_000
                }
            }
        }]
    });

    let parsed = openai_helpers::parse_openai_usage_payload(&json);

    assert_eq!(
        parsed.extra_info.first(),
        Some(&("Plan".to_string(), "pro".to_string()))
    );
    assert!(!parsed.hard_limit_reached);
    assert_eq!(parsed.limits.len(), 3);
    assert_eq!(parsed.limits[0].name, "5-hour window");
    assert_eq!(parsed.limits[0].usage_percent, 25.0);
    assert_eq!(parsed.limits[1].name, "7-day window");
    assert_eq!(parsed.limits[1].usage_percent, 50.0);
    assert_eq!(parsed.limits[2].name, "Codex Spark (5h)");
    assert_eq!(parsed.limits[2].usage_percent, 75.0);
}

#[test]
fn test_parse_openai_usage_payload_falls_back_to_nested_rate_limits() {
    let json = serde_json::json!({
        "plan": "team",
        "codex": {
            "rate_limits": [
                {
                    "name": "Codex 5h",
                    "used": 20,
                    "limit": 80,
                    "resets_at": "2026-01-01T00:00:00Z"
                },
                {
                    "name": "Codex 1w",
                    "remaining": 60,
                    "limit": 80,
                    "resets_at": "2026-01-07T00:00:00Z"
                }
            ]
        }
    });

    let parsed = openai_helpers::parse_openai_usage_payload(&json);

    assert_eq!(
        parsed.extra_info.first(),
        Some(&("Plan".to_string(), "team".to_string()))
    );
    assert_eq!(parsed.limits.len(), 2);
    assert_eq!(parsed.limits[0].name, "Codex 5h");
    assert_eq!(parsed.limits[0].usage_percent, 25.0);
    assert_eq!(parsed.limits[1].name, "Codex 1w");
    assert_eq!(parsed.limits[1].usage_percent, 25.0);
}

#[test]
fn test_account_usage_probe_prefers_best_available_alternative() {
    let probe = AccountUsageProbe {
        provider: MultiAccountProviderKind::OpenAI,
        current_label: "work".to_string(),
        accounts: vec![
            AccountUsageSnapshot {
                label: "work".to_string(),
                email: Some("work@example.com".to_string()),
                exhausted: true,
                five_hour_ratio: Some(1.0),
                seven_day_ratio: Some(1.0),
                resets_at: Some("2026-01-01T00:00:00Z".to_string()),
                error: None,
            },
            AccountUsageSnapshot {
                label: "backup".to_string(),
                email: Some("backup@example.com".to_string()),
                exhausted: false,
                five_hour_ratio: Some(0.45),
                seven_day_ratio: Some(0.10),
                resets_at: Some("2026-01-01T01:00:00Z".to_string()),
                error: None,
            },
            AccountUsageSnapshot {
                label: "secondary".to_string(),
                email: Some("secondary@example.com".to_string()),
                exhausted: false,
                five_hour_ratio: Some(0.70),
                seven_day_ratio: Some(0.20),
                resets_at: Some("2026-01-01T02:00:00Z".to_string()),
                error: None,
            },
        ],
    };

    let best = probe
        .best_available_alternative()
        .expect("expected alternative account");
    assert_eq!(best.label, "backup");

    let guidance = probe.switch_guidance().expect("expected switch guidance");
    assert!(guidance.contains("`backup`"));
    assert!(guidance.contains("/account openai switch backup"));
}

#[test]
fn test_account_usage_probe_detects_all_accounts_exhausted() {
    let probe = AccountUsageProbe {
        provider: MultiAccountProviderKind::Anthropic,
        current_label: "primary".to_string(),
        accounts: vec![
            AccountUsageSnapshot {
                label: "primary".to_string(),
                email: None,
                exhausted: true,
                five_hour_ratio: Some(1.0),
                seven_day_ratio: Some(1.0),
                resets_at: None,
                error: None,
            },
            AccountUsageSnapshot {
                label: "backup".to_string(),
                email: None,
                exhausted: true,
                five_hour_ratio: Some(1.0),
                seven_day_ratio: Some(1.0),
                resets_at: None,
                error: None,
            },
        ],
    };

    assert!(probe.current_exhausted());
    assert!(probe.all_accounts_exhausted());
    assert!(probe.best_available_alternative().is_none());
    assert!(probe.switch_guidance().is_none());
}
