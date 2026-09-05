use super::failover::*;
use super::quota::*;
use serde_json::json;
use std::time::Duration;

#[test]
fn command_code_quota_parses_wrapped_windows() {
    let parsed = parse_credits(&json!({"windowLimits": {"fiveHour": {"cap": 100, "used": 25, "resetAt": "later"}, "weekly": {"limit": 500, "usage": 50}}, "credits": 3}));
    assert_eq!(parsed.five_hour.unwrap().used, Some(25.0));
    assert_eq!(parsed.weekly.unwrap().cap, Some(500.0));
    assert_eq!(parsed.credits, Some(3.0));
}
#[test]
fn quota_cache_is_per_account() {
    let cache = CommandCodeQuotaCache::new();
    let value = CommandCodeCredits { credits: Some(1.0), ..Default::default() };
    cache.insert("a", value.clone());
    assert_eq!(cache.get("a"), Some(value));
    assert_eq!(cache.get("b"), None);
}
#[test]
fn failover_sticks_short_429_and_rotates_long_pre_stream() {
    let accounts = vec!["a".into(), "b".into()];
    assert_eq!(handle_command_code_error_failover("a", 429, Some(Duration::from_secs(2)), &accounts, false, 0), CommandCodeFailoverAction::StickWait { delay: Duration::from_secs(2) });
    assert_eq!(handle_command_code_error_failover("a", 429, Some(Duration::from_secs(6)), &accounts, false, 0), CommandCodeFailoverAction::Rotate { previous_account: "a".into(), next_account: "b".into() });
    assert_eq!(handle_command_code_error_failover("a", 429, Some(Duration::from_secs(6)), &accounts, true, 0), CommandCodeFailoverAction::NoAction);
}
