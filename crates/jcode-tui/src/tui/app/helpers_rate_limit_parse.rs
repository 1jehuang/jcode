//! Rate-limit error parsing (reset/retry timing) for TUI auto-retry logic.
use std::time::Duration;

use super::parse_clock_time_to_duration;

/// Longest wait a held turn will be parked for before its automatic resume.
/// Server and body hints beyond this are clamped so a bogus or hostile value
/// cannot park a turn for days.
pub(crate) const MAX_TURN_HOLD_SECS: u64 = 24 * 60 * 60;

/// Find a JSON object embedded in a formatted provider error (for example
/// after `response:` or `OpenAI API error 429:`) and read its numeric
/// `retry_after_seconds`. Sibling limit fields such as `max_rpm` are ignored.
fn structured_retry_after_seconds(error: &str) -> Option<u64> {
    error.match_indices('{').take(8).find_map(|(idx, _)| {
        jcode_provider_core::retry_after::retry_after_body_seconds(&error[idx..])
    })
}

/// Parse rate limit reset time from error message
/// Returns the Duration until rate limit resets, if this is a rate limit error
pub(crate) fn parse_rate_limit_error(error: &str) -> Option<Duration> {
    if crate::provider::parse_failover_prompt_message(error).is_some() {
        return None;
    }
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

    // A compact JSON object is one whitespace token. Scanning its digits can
    // miss retry_after_seconds entirely or accidentally pick max_rpm instead.
    if error.contains("\"retry_after_seconds\"") {
        // Invalid structured delays stay invalid rather than falling through
        // to a loose numeric scan, which could mistake -1 for one second.
        return structured_retry_after_seconds(error)
            .map(|secs| Duration::from_secs(secs.min(MAX_TURN_HOLD_SECS)));
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

    let raw = reset_instant_field(error)?;
    let reset_at = match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(at) => at.with_timezone(&chrono::Utc),
        Err(_) => return None,
    };
    let until_reset = match (reset_at - now).to_std() {
        Ok(wait) => wait,
        // Reset instant already passed: resend promptly.
        Err(_) => Duration::ZERO,
    };
    (until_reset <= MAX_QUOTA_WINDOW_WAIT)
        .then(|| (until_reset + QUOTA_WINDOW_RESET_SLACK).min(MAX_QUOTA_WINDOW_WAIT))
}

/// Value of a JSON `"resets_at"`-style string field inside an error body.
fn reset_instant_field(error: &str) -> Option<&str> {
    [
        "\"resets_at\"",
        "\"reset_at\"",
        "\"resetsAt\"",
        "\"resetAt\"",
    ]
    .iter()
    .find_map(|key| {
        let after_key = &error[error.find(key)? + key.len()..];
        let value = after_key.trim_start().strip_prefix(':')?.trim_start();
        let value = value.strip_prefix('"')?;
        Some(&value[..value.find('"')?])
    })
}

#[cfg(test)]
#[cfg(test)]
mod rate_limit_parse_tests {
    use super::{MAX_TURN_HOLD_SECS, parse_quota_window_reset, parse_rate_limit_error};
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
    fn quota_reset_within_day_caps_clock_skew_allowance() {
        let reset = at("2026-09-24T09:00:00Z");
        for seconds in [MAX_TURN_HOLD_SECS - 14, MAX_TURN_HOLD_SECS] {
            let now = reset - chrono::Duration::seconds(seconds as i64);
            assert_eq!(
                parse_quota_window_reset(OPENFERENCE_WINDOW_ERROR, now),
                Some(Duration::from_secs(MAX_TURN_HOLD_SECS)),
                "reset in {seconds} seconds must be held"
            );
        }
        let now = reset - chrono::Duration::seconds(MAX_TURN_HOLD_SECS as i64 + 1);
        assert_eq!(
            parse_quota_window_reset(OPENFERENCE_WINDOW_ERROR, now),
            None
        );
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
    fn reset_field_variants_and_malformed_values() {
        let now = at("2026-09-24T08:00:00Z");
        let hour = Some(Duration::from_secs(3600 + 15));
        for body in [
            r#"quota exceeded {"resets_at" : "2026-09-24T09:00:00Z"}"#,
            r#"quota exceeded {"resetsAt":"2026-09-24T11:00:00+02:00"}"#,
            r#"limit exceeded {"reset_at":"2026-09-24T09:00:00.000Z"}"#,
        ] {
            assert_eq!(parse_quota_window_reset(body, now), hour, "{body}");
        }
        for body in [
            r#"quota exceeded {"resets_at":"tomorrow"}"#,
            r#"quota exceeded {"resets_at":1790240400}"#,
            r#"quota exceeded {"resets_at":"2026-09-24T09:00:00Z"#,
            r#"quota exceeded {"resets_at"}"#,
            "quota exceeded resets_at",
        ] {
            assert_eq!(parse_quota_window_reset(body, now), None, "{body}");
        }
    }

    #[test]
    fn account_pool_failover_prompt_never_schedules_same_account_retry() {
        let prompt = jcode_provider_core::ProviderFailoverPrompt {
            from_provider: "claude".into(),
            from_label: "Anthropic".into(),
            to_provider: "openai".into(),
            to_label: "OpenAI".into(),
            target_route: None,
            reason: "429 rate limit exceeded; retry in 60 seconds".into(),
            estimated_input_chars: 400,
            estimated_input_tokens: 100,
        };
        assert_eq!(parse_rate_limit_error(&prompt.to_error_message()), None);
        assert_eq!(
            parse_rate_limit_error("429 rate limit exceeded; retry in 60 seconds"),
            Some(Duration::from_secs(60)),
        );
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

    #[test]
    fn openference_json_retry_after_seconds_is_parsed() {
        let err = "OpenAI-compatible chat request failed\n  status: 429 Too Many Requests\n  response: {\"error\":\"Rate limit exceeded. Too many requests per minute.\",\"type\":\"rate_limit_error\",\"code\":\"rate_limit_exceeded\",\"retry_after_seconds\":2,\"max_rpm\":25}\nHint: check network connectivity";
        assert_eq!(parse_rate_limit_error(err), Some(Duration::from_secs(2)));
    }

    #[test]
    fn invalid_json_retry_after_does_not_use_max_rpm() {
        for value in ["-1", "null", "true", "\"soon\""] {
            let err = format!(
                "status: 429 Too Many Requests\n  response: {{\"retry_after_seconds\": {value}, \"max_rpm\": 25}}"
            );
            assert_eq!(parse_rate_limit_error(&err), None, "{err}");
        }
    }
}

#[cfg(test)]
mod anthropic_usage_limit_parse_tests {
    use super::parse_rate_limit_error;
    use std::time::Duration;

    /// The Anthropic runtime fails fast on a usage limit that resets hours
    /// away. The hold must pick up that reset time, not a bogus short retry.
    #[test]
    fn anthropic_fail_fast_usage_limit_message_parses_reset() {
        let err = "Anthropic API error (429 Too Many Requests): {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"This request would exceed your account's rate limit. Please try again later.\"}} Usage limit reached for this Claude account; resets in 3h 17m (2026-09-29 23:43 UTC).";
        assert_eq!(
            parse_rate_limit_error(err),
            Some(Duration::from_secs(3 * 3600 + 17 * 60))
        );
    }
}
