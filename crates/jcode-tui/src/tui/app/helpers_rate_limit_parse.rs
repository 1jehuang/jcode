//! Rate-limit error parsing (reset/retry timing) for TUI auto-retry logic.
use std::time::Duration;

use super::parse_clock_time_to_duration;

/// Parse rate limit reset time from error message
/// Returns the Duration until rate limit resets, if this is a rate limit error
pub(crate) fn parse_rate_limit_error(error: &str) -> Option<Duration> {
    if let Some(duration) = parse_quota_window_reset(error, chrono::Utc::now()) {
        return Some(duration);
    }

    let error_lower = error.to_lowercase();

    if !error_lower.contains("rate limit")
        && !error_lower.contains("rate_limit")
        && !error_lower.contains("429")
        && !error_lower.contains("too many requests")
        && !error_lower.contains("hit your limit")
    {
        return None;
    }

    if let Some(idx) = error_lower.find("retry") {
        let after = &error_lower[idx..];
        for word in after.split_whitespace() {
            if let Ok(secs) = word
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
                && secs > 0
                && secs < 86400
            {
                return Some(Duration::from_secs(secs));
            }
        }
    }

    if let Some(idx) = error_lower.find("resets") {
        let after = &error_lower[idx..];
        for word in after.split_whitespace() {
            let word = word.trim_matches(|c: char| c == '·' || c == ' ');
            if (word.ends_with("am") || word.ends_with("pm"))
                && let Some(duration) = parse_clock_time_to_duration(word)
            {
                return Some(duration);
            }
        }
    }

    if let Some(idx) = error_lower.find("reset") {
        let after = &error_lower[idx..];
        // Unit-suffixed durations like "resets in 30d 4h 29m" (OpenAI usage
        // limit messages). Without this, "30d" would parse as 30 seconds and
        // schedule a bogus 30s auto-retry against a limit that resets in days.
        let mut unit_total = Duration::ZERO;
        let mut saw_unit = false;
        for word in after.split_whitespace().take(8) {
            let digits: String = word.chars().take_while(|c| c.is_ascii_digit()).collect();
            let rest = &word[digits.len()..];
            if digits.is_empty() {
                continue;
            }
            let value: u64 = match digits.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let secs = match rest.trim_matches(|c: char| !c.is_ascii_alphabetic()) {
                "d" => Some(value * 86400),
                "h" => Some(value * 3600),
                "m" | "min" => Some(value * 60),
                "s" | "sec" => Some(value),
                _ => None,
            };
            if let Some(secs) = secs {
                unit_total += Duration::from_secs(secs);
                saw_unit = true;
            }
        }
        if saw_unit {
            // Only auto-retry within a day; longer windows should be treated
            // as terminal by the caller (fallback offer / stop auto-poke).
            if unit_total > Duration::ZERO && unit_total < Duration::from_secs(86400) {
                return Some(unit_total);
            }
            return None;
        }
        for word in after.split_whitespace() {
            if let Ok(secs) = word
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u64>()
                && secs > 0
                && secs < 86400
            {
                return Some(Duration::from_secs(secs));
            }
        }
    }

    None
}

/// Longest usage-window wait the client will hold a failed turn for.
const MAX_QUOTA_WINDOW_WAIT: Duration = Duration::from_secs(24 * 3600);
/// Slack added past the provider's reset instant so clock skew does not
/// resend a moment early and hit the same exhausted window.
const QUOTA_WINDOW_RESET_SLACK: Duration = Duration::from_secs(15);

/// Parse a usage-window reset carried as an absolute timestamp in the error
/// body, e.g. Openference:
/// `402 Payment Required {"error":"Request limit exceeded (1500 per 5 hours)...",
///  "code":"window_quota_exceeded","resets_at":"2026-09-24T09:00:00.000Z"}`.
///
/// These windows reset hours later, so the caller holds the turn and resends
/// it once the window reopens instead of failing it. Balance or credit errors
/// without a reset instant are not matched and still fail.
pub(crate) fn parse_quota_window_reset(
    error: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<Duration> {
    static RESETS_AT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#""(?:resets_at|reset_at|resetsAt|resetAt)"\s*:\s*"([^"]+)""#)
            .expect("valid resets_at regex")
    });

    let lower = error.to_lowercase();
    let is_limit = [
        "quota",
        "limit exceeded",
        "rate limit",
        "rate_limit",
        "limit reached",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if !is_limit {
        return None;
    }

    let raw = RESETS_AT.captures(error)?.get(1)?.as_str();
    let reset_at = chrono::DateTime::parse_from_rfc3339(raw)
        .ok()?
        .with_timezone(&chrono::Utc);
    let wait = (reset_at - now).to_std().unwrap_or(Duration::ZERO) + QUOTA_WINDOW_RESET_SLACK;
    (wait <= MAX_QUOTA_WINDOW_WAIT).then_some(wait)
}

#[cfg(test)]
#[cfg(test)]
mod rate_limit_parse_tests {
    use super::{parse_quota_window_reset, parse_rate_limit_error};
    use std::time::Duration;

    const OPENFERENCE_WINDOW_ERROR: &str = "OpenAI-compatible chat request failed\n  endpoint: https://api.openference.com/v1/chat/completions\n  model: GLM-5.3\n  auth: JCODE_PROVIDER_OPENCODE_OPENFERENCE_API_KEY\n  status: 402 Payment Required\n  response: {\"error\":\"Request limit exceeded (1500 per 5 hours). Top up your balance to continue.\",\"type\":\"insufficient_quota\",\"code\":\"window_quota_exceeded\",\"resets_at\":\"2026-09-24T09:00:00.000Z\"}\nHint: check network connectivity";

    fn at(ts: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn openference_window_quota_waits_until_resets_at() {
        let wait = parse_quota_window_reset(OPENFERENCE_WINDOW_ERROR, at("2026-09-24T07:30:00Z"));
        assert_eq!(wait, Some(Duration::from_secs(90 * 60 + 15)));
    }

    #[test]
    fn elapsed_resets_at_retries_promptly() {
        let wait = parse_quota_window_reset(OPENFERENCE_WINDOW_ERROR, at("2026-09-24T09:10:00Z"));
        assert_eq!(wait, Some(Duration::from_secs(15)));
    }

    #[test]
    fn resets_at_beyond_a_day_is_not_held() {
        let wait = parse_quota_window_reset(OPENFERENCE_WINDOW_ERROR, at("2026-09-22T09:00:00Z"));
        assert_eq!(wait, None);
    }

    #[test]
    fn balance_error_without_reset_still_fails() {
        let err = r#"status: 402 Payment Required response: {"error":"Insufficient balance","type":"insufficient_quota"}"#;
        assert_eq!(
            parse_quota_window_reset(err, at("2026-09-24T07:30:00Z")),
            None
        );
        assert_eq!(parse_rate_limit_error(err), None);
    }

    #[test]
    fn unrelated_resets_at_field_is_ignored() {
        let err =
            r#"status: 500 response: {"error":"internal","resets_at":"2026-09-24T09:00:00Z"}"#;
        assert_eq!(
            parse_quota_window_reset(err, at("2026-09-24T07:30:00Z")),
            None
        );
    }

    #[test]
    fn openference_error_is_scheduled_by_the_rate_limit_parser() {
        assert!(parse_rate_limit_error(OPENFERENCE_WINDOW_ERROR).is_some());
    }

    #[test]
    fn usage_limit_reset_in_days_does_not_schedule_bogus_short_retry() {
        // "30d" must not be misread as 30 seconds.
        let err = "Rate limited: The usage limit has been reached. Plan: team. \
                   Resets in 30d 4h 29m (2026-08-21 04:31 UTC).";
        assert_eq!(parse_rate_limit_error(err), None);
    }

    #[test]
    fn unit_suffixed_reset_within_a_day_is_parsed() {
        let err = "429 rate limit exceeded. Resets in 2h 5m.";
        assert_eq!(
            parse_rate_limit_error(err),
            Some(Duration::from_secs(2 * 3600 + 5 * 60))
        );
    }

    #[test]
    fn plain_retry_seconds_still_parse() {
        let err = "429 Too Many Requests: retry after 30 seconds";
        assert_eq!(parse_rate_limit_error(err), Some(Duration::from_secs(30)));
    }
}
