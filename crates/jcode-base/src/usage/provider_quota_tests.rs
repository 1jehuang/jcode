use super::*;

#[test]
fn kimi_usage_limits_shape_parses_windowed_quotas() {
    let json = serde_json::json!({
        "limits": [
            {"window": {"duration": 5, "timeUnit": "HOUR"},
             "detail": {"limit": 100.0, "used": 8.0, "percentage": 8.0, "resetTime": "2026-10-04T10:00:00Z"}},
            {"window": {"duration": 7, "timeUnit": "DAY"},
             "detail": {"limit": 100.0, "remaining": 28.0, "resetTime": "2026-10-08T00:00:00Z"}},
            {"window": {"duration": 30, "timeUnit": "DAY"},
             "detail": {"limit": 100.0, "used": 45.0, "resetTime": "2026-11-01T00:00:00Z"}}
        ]
    });
    let limits = api_keys::parse_kimi_usage_limits(&json);
    assert_eq!(limits.len(), 3);
    // Windows rank 5-hour < Weekly < Monthly regardless of response order.
    assert_eq!(limits[0].name, "5-hour");
    assert!((limits[0].usage_percent - 8.0).abs() < f32::EPSILON);
    assert_eq!(limits[0].resets_at.as_deref(), Some("2026-10-04T10:00:00Z"));
    assert_eq!(limits[1].name, "Weekly");
    // Remaining-derived usage: 100 - 28 = 72%.
    assert!((limits[1].usage_percent - 72.0).abs() < 0.01);
    assert_eq!(limits[2].name, "Monthly");
    assert!((limits[2].usage_percent - 45.0).abs() < f32::EPSILON);
}

#[test]
fn kimi_usage_legacy_usages_map_shape() {
    let json = serde_json::json!({
        "usages": {
            "limit_5h": {"used_ratio": 0.08, "reset_time": "2026-10-04T10:00:00Z"},
            "limit_7d": {"used_ratio": 0.92, "reset_time": "2026-10-08T00:00:00Z"},
            "limit_30d": {"used_ratio": 0.10}
        }
    });
    let limits = api_keys::parse_kimi_usage_limits(&json);
    assert_eq!(limits.len(), 3);
    assert_eq!(limits[0].name, "5-hour");
    assert!((limits[0].usage_percent - 8.0).abs() < 0.01);
    assert_eq!(limits[1].name, "Weekly");
    assert!((limits[1].usage_percent - 92.0).abs() < 0.01);
    assert_eq!(limits[1].resets_at.as_deref(), Some("2026-10-08T00:00:00Z"));
    assert_eq!(limits[2].name, "Monthly");
    assert_eq!(limits[2].resets_at, None);
}

#[test]
fn kimi_usage_summary_row_falls_back_to_weekly_label() {
    let json = serde_json::json!({
        "usage": {"limit": 100.0, "used": 72.0, "resetTime": "2026-10-08T00:00:00Z"}
    });
    let limits = api_keys::parse_kimi_usage_limits(&json);
    assert_eq!(limits.len(), 1);
    assert_eq!(limits[0].name, "Weekly");
    assert!((limits[0].usage_percent - 72.0).abs() < 0.01);
}

#[test]
fn cursor_plan_usage_report_builds_named_limits() {
    let json = serde_json::json!({
        "membershipType": "pro",
        "billingCycleEnd": "1792835722000",
        "planUsage": {
            "totalPercentUsed": 27.0,
            "autoPercentUsed": 29.0,
            "apiPercentUsed": 150.0
        }
    });
    let report = provider_fetch::cursor_plan_usage_report(&json);
    assert_eq!(report.provider_name, "Cursor Pro");
    assert_eq!(report.limits.len(), 3);
    assert_eq!(report.limits[0].name, "Included");
    assert!((report.limits[0].usage_percent - 27.0).abs() < f32::EPSILON);
    assert_eq!(report.limits[2].name, "API");
    // Out-of-range percentages are clamped.
    assert!((report.limits[2].usage_percent - 100.0).abs() < f32::EPSILON);
    let reset = report.limits[0]
        .resets_at
        .as_deref()
        .expect("billing cycle end present");
    let dt = chrono::DateTime::parse_from_rfc3339(reset).expect("rfc3339 reset");
    assert_eq!(dt.timestamp(), 1792835722);
}

#[test]
fn cursor_plan_usage_report_prefers_team_spend_limit() {
    let json = serde_json::json!({
        "membershipType": "pro",
        "spendLimitUsage": {"limitType": "team"},
        "planUsage": {"totalPercentUsed": 40.0}
    });
    let report = provider_fetch::cursor_plan_usage_report(&json);
    assert_eq!(report.provider_name, "Cursor Team");
    assert_eq!(report.limits.len(), 1);
}

#[test]
fn reset_timestamp_from_json_value_handles_all_encodings() {
    // protobuf int64 JSON-encodes as a decimal string of milliseconds.
    let millis = serde_json::json!("1792835722000");
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&reset_timestamp_from_json_value(&millis).unwrap())
            .unwrap()
            .timestamp(),
        1792835722
    );
    let seconds = serde_json::json!(1792835722u64);
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&reset_timestamp_from_json_value(&seconds).unwrap())
            .unwrap()
            .timestamp(),
        1792835722
    );
    let iso = serde_json::json!("2026-10-24T10:15:22Z");
    assert_eq!(
        reset_timestamp_from_json_value(&iso).as_deref(),
        Some("2026-10-24T10:15:22Z")
    );
    assert_eq!(reset_timestamp_from_json_value(&serde_json::json!(0)), None);
}

#[test]
fn zai_coding_plan_limits_parse_token_and_time_windows() {
    let json = serde_json::json!({
        "data": {
            "limits": [
                {"type": "TOKENS_LIMIT", "unit": 6, "percentage": 72.0, "nextResetTime": "2026-10-08T00:00:00Z"},
                {"type": "TOKENS_LIMIT", "unit": 5, "percentage": 4.8, "nextResetTime": "2026-10-04T10:00:00Z"},
                {"type": "TIME_LIMIT", "currentValue": 3.0, "usage": 20.0, "nextResetTime": "2026-11-01T00:00:00Z"}
            ]
        }
    });
    let limits = api_keys::parse_zai_coding_plan_limits(&json);
    assert_eq!(limits.len(), 3);
    // 5-hour ranks before Weekly before MCP monthly.
    assert_eq!(limits[0].name, "5-hour");
    assert!((limits[0].usage_percent - 4.8).abs() < 0.01);
    assert_eq!(limits[0].resets_at.as_deref(), Some("2026-10-04T10:00:00Z"));
    assert_eq!(limits[1].name, "Weekly");
    assert!((limits[1].usage_percent - 72.0).abs() < f32::EPSILON);
    assert_eq!(limits[2].name, "MCP monthly");
    // TIME_LIMIT without percentage falls back to currentValue/usage.
    assert!((limits[2].usage_percent - 15.0).abs() < 0.01);
}

#[test]
fn zai_coding_plan_limits_tolerate_unwrapped_payload() {
    let json = serde_json::json!({
        "limits": [
            {"type": "TOKENS_LIMIT", "unit": 6, "percentage": 10.0}
        ]
    });
    let limits = api_keys::parse_zai_coding_plan_limits(&json);
    assert_eq!(limits.len(), 1);
    assert_eq!(limits[0].name, "Weekly");
    assert_eq!(limits[0].resets_at, None);
    // Unknown rows and empty payloads yield no limits (caller falls back).
    let empty =
        api_keys::parse_zai_coding_plan_limits(&serde_json::json!({"data": {"limits": []}}));
    assert!(empty.is_empty());
}

#[test]
fn kimi_usage_explicit_weekly_window_beats_summary() {
    let json = serde_json::json!({
        "usage": {"limit": 100.0, "used": 12.0, "resetTime": "2026-10-08T00:00:00Z"},
        "limits": [
            {"window": {"duration": 7, "timeUnit": "DAY"},
             "detail": {"limit": 100.0, "used": 72.0, "resetTime": "2026-10-09T00:00:00Z"}}
        ]
    });
    let limits = api_keys::parse_kimi_usage_limits(&json);
    assert_eq!(limits.len(), 1);
    assert_eq!(limits[0].name, "Weekly");
    assert!((limits[0].usage_percent - 72.0).abs() < 0.01);
    assert_eq!(limits[0].resets_at.as_deref(), Some("2026-10-09T00:00:00Z"));
}

#[test]
fn cursor_plan_usage_report_keeps_membership_without_usage_fields() {
    // Native logins can receive membership info without planUsage fields;
    // the plan label is still reported instead of falling back to the
    // API-key probe.
    let json = serde_json::json!({"membershipType": "pro"});
    let report = provider_fetch::cursor_plan_usage_report(&json);
    assert_eq!(report.provider_name, "Cursor Pro");
    assert!(report.limits.is_empty());
    assert!(report.error.is_none());
}

#[test]
fn zai_quota_url_follows_profile_region() {
    assert_eq!(
        api_keys::zai_quota_url("https://api.z.ai/api/coding/paas/v4"),
        "https://api.z.ai/api/monitor/usage/quota/limit"
    );
    assert_eq!(
        api_keys::zai_quota_url("https://open.bigmodel.cn/api/coding/paas/v4"),
        "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
    );
    // Custom proxies and unparsable bases default to the international host.
    assert_eq!(
        api_keys::zai_quota_url("https://proxy.example.com/v1"),
        "https://api.z.ai/api/monitor/usage/quota/limit"
    );
    assert_eq!(
        api_keys::zai_quota_url("not a url"),
        "https://api.z.ai/api/monitor/usage/quota/limit"
    );
}
