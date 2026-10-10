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

#[cfg(test)]
#[cfg(test)]
mod rate_limit_parse_tests {
    use super::parse_rate_limit_error;
    use std::time::Duration;

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
