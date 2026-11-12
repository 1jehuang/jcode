use super::*;
use jcode_usage_types::OpenAiResetCredits;
use serde_json::json;
use std::time::{Duration, Instant};

fn reset_usage() -> OpenAIUsageData {
    OpenAIUsageData {
        openai_reset_credits: Some(OpenAiResetCredits {
            available_count: 3,
            available_expirations: Vec::new(),
            account_label: Some("work".into()),
            ordinary_usage_allowed: Some(false),
        }),
        fetched_at: Some(Instant::now()),
        ..Default::default()
    }
}

#[test]
fn parses_official_banked_reset_summary_and_allowed_flag() {
    let parsed = parse_openai_usage_payload(&json!({
        "rate_limit_reset_credits": {"available_count": 3},
        "rate_limit": {"allowed": false}
    }));
    assert_eq!(parsed.available_reset_count, Some(3));
    assert_eq!(parsed.ordinary_usage_allowed, Some(false));
    for count in [json!(null), json!(-1), json!(1.5), json!("3"), json!(true)] {
        let parsed = parse_openai_usage_payload(&json!({
            "rate_limit_reset_credits": {"available_count": count}
        }));
        assert_eq!(parsed.available_reset_count, None);
    }
    let missing = parse_openai_usage_payload(&json!({}));
    assert_eq!(missing.available_reset_count, None);
    assert_eq!(missing.ordinary_usage_allowed, None);
    let zero = parse_openai_usage_payload(&json!({
        "rate_limit_reset_credits": {"available_count": 0},
        "rate_limit": {"allowed": true}
    }));
    assert_eq!(zero.available_reset_count, Some(0));
    assert_eq!(zero.ordinary_usage_allowed, Some(true));
}

#[test]
fn reset_hint_requires_fresh_matching_account_and_known_positive_count() {
    let usage = reset_usage();
    assert!(usage.banked_reset_available_for_account(Some("work")));
    assert!(!usage.banked_reset_available_for_account(Some("personal")));
    assert!(!usage.banked_reset_available_for_account(None));

    let mut default_scope = usage.clone();
    default_scope
        .openai_reset_credits
        .as_mut()
        .unwrap()
        .account_label = None;
    assert!(default_scope.banked_reset_available_for_account(None));
    assert!(!default_scope.banked_reset_available_for_account(Some("work")));

    let mut zero = usage.clone();
    zero.openai_reset_credits.as_mut().unwrap().available_count = 0;
    let mut unknown = usage.clone();
    unknown.openai_reset_credits = None;
    let mut stale = usage.clone();
    stale.fetched_at = Some(Instant::now() - Duration::from_secs(301));
    let mut unfetched = usage.clone();
    unfetched.fetched_at = None;
    let mut error = usage.clone();
    error.last_error = Some("network error".into());
    for invalid in [zero, unknown, stale, unfetched, error] {
        assert!(!invalid.banked_reset_available_for_account(Some("work")));
    }
}

#[test]
fn reset_hint_uses_allowed_flag_over_rounded_percent_or_hard_limit_flag() {
    let mut usage = reset_usage();
    usage.hard_limit_reached = true;
    usage.five_hour = Some(OpenAIUsageWindow {
        name: "5-hour".into(),
        usage_ratio: 1.0,
        resets_at: None,
    });
    usage
        .openai_reset_credits
        .as_mut()
        .unwrap()
        .ordinary_usage_allowed = Some(true);
    assert!(!usage.banked_reset_available_for_account(Some("work")));
    usage
        .openai_reset_credits
        .as_mut()
        .unwrap()
        .ordinary_usage_allowed = Some(false);
    assert!(usage.banked_reset_available_for_account(Some("work")));
}

#[test]
fn reset_hint_without_allowed_flag_requires_full_ordinary_window_or_hard_limit() {
    let mut usage = reset_usage();
    usage
        .openai_reset_credits
        .as_mut()
        .unwrap()
        .ordinary_usage_allowed = None;
    assert!(!usage.banked_reset_available_for_account(Some("work")));
    for ratio in [0.0, 0.95, 0.99, 0.999] {
        usage.five_hour = Some(OpenAIUsageWindow {
            name: "5-hour".into(),
            usage_ratio: ratio,
            resets_at: None,
        });
        assert!(!usage.banked_reset_available_for_account(Some("work")));
    }
    usage.five_hour.as_mut().unwrap().usage_ratio = 1.0;
    assert!(usage.banked_reset_available_for_account(Some("work")));
    usage.spark = usage.five_hour.take();
    assert!(!usage.banked_reset_available_for_account(Some("work")));
    usage.hard_limit_reached = true;
    assert!(usage.banked_reset_available_for_account(Some("work")));
}

#[test]
fn expired_window_never_suggests_reset_in_raw_or_display_snapshot() {
    let mut usage = reset_usage();
    usage.hard_limit_reached = true;
    usage.five_hour = Some(OpenAIUsageWindow {
        name: "5-hour".into(),
        usage_ratio: 1.0,
        resets_at: Some("2000-01-01T00:00:00Z".into()),
    });
    assert!(!usage.banked_reset_available_for_account(Some("work")));
    let snapshot = usage.display_snapshot();
    assert!(snapshot.openai_reset_credits.is_none());
    assert!(!snapshot.banked_reset_available_for_account(Some("work")));
}
