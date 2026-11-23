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

/// Parse a single token made exclusively of one or more unit-suffixed duration
/// parts, such as `"2h"`, `"5m."`, `"2h16m14s."`, or `"7773.393684450s"`.
/// Rejects arbitrary identifiers like `"gemini-4-neon-low"` that merely happen
/// to contain digits.
fn parse_unit_duration_token(token: &str) -> Option<Duration> {
    let mut s = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if s.is_empty() || !s.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let mut total_secs: u64 = 0;
    let mut saw_part = false;
    while !s.is_empty() {
        let int_len = s.bytes().take_while(|b| b.is_ascii_digit()).count();
        if int_len == 0 {
            return None;
        }
        let int_val: u64 = s[..int_len].parse().ok()?;
        s = &s[int_len..];
        let mut has_nonzero_frac = false;
        if let Some(rest) = s.strip_prefix('.') {
            let frac_len = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
            if frac_len == 0 {
                return None;
            }
            has_nonzero_frac = rest[..frac_len].bytes().any(|b| b != b'0');
            s = &rest[frac_len..];
        }
        let unit_len = s.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
        if unit_len == 0 {
            return None;
        }
        let unit = &s[..unit_len];
        s = &s[unit_len..];
        let mult: u64 = match unit {
            "d" => 86400,
            "h" => 3600,
            "m" | "min" => 60,
            "s" | "sec" => 1,
            _ => return None,
        };
        let part_secs = int_val
            .checked_mul(mult)?
            .checked_add(u64::from(has_nonzero_frac && mult == 1))?;
        total_secs = total_secs.checked_add(part_secs)?;
        saw_part = true;
    }
    saw_part.then(|| Duration::from_secs(total_secs))
}

/// Parse a standalone positive integer token, stripping only surrounding
/// punctuation so embedded model digits like `"gemini-4-neon-low"` are never
/// misread as seconds.
fn parse_plain_seconds_token(token: &str) -> Option<u64> {
    let cleaned = token.trim_matches(|c: char| {
        matches!(
            c,
            '.' | ',' | ':' | ';' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '!' | '?'
        )
    });
    if cleaned.is_empty() || !cleaned.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    cleaned.parse::<u64>().ok()
}

/// Parse a short window of tokens after a `retry` or `reset` keyword,
/// preferring unit-suffixed durations (`2h 5m`, `2h16m14s`, `7773.39s`) over
/// bare integer seconds.
fn parse_duration_after_keyword(after: &str) -> Option<Duration> {
    let mut unit_total = Duration::ZERO;
    let mut saw_unit = false;
    for word in after.split_whitespace().take(8) {
        if let Some(part) = parse_unit_duration_token(word) {
            unit_total = unit_total.saturating_add(part);
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
    for word in after.split_whitespace().take(8) {
        if let Some(secs) = parse_plain_seconds_token(word)
            && secs > 0
            && secs < 86400
        {
            return Some(Duration::from_secs(secs));
        }
    }
    None
}

/// Parse rate limit reset time from error message
/// Returns the Duration until rate limit resets, if this is a rate limit error
pub(crate) fn parse_rate_limit_error(error: &str) -> Option<Duration> {
    if crate::provider::parse_failover_prompt_message(error).is_some() {
        return None;
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

    if let Some(idx) = error_lower.find("retry")
        && let Some(duration) = parse_duration_after_keyword(&error_lower[idx..])
    {
        return Some(duration);
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
        return parse_duration_after_keyword(&error_lower[idx..]);
    }

    None
}

#[cfg(test)]
#[cfg(test)]
mod rate_limit_parse_tests {
    use super::parse_rate_limit_error;
    use std::time::Duration;

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

    #[test]
    fn compact_unit_duration_and_model_name_digits_are_handled_correctly() {
        let compact = "429 Too Many Requests: Rate limit reached. Resets in 2h16m14s. \
                       {\"metadata\": {\"model\": \"gemini-4-neon-low\"}}";
        assert_eq!(
            parse_rate_limit_error(compact),
            Some(Duration::from_secs(2 * 3600 + 16 * 60 + 14))
        );

        let no_delay_with_model = "429 Too Many Requests: quota reset pending for model \
                                   \"gemini-4-neon-low\"";
        assert_eq!(parse_rate_limit_error(no_delay_with_model), None);

        let retry_delay = "429 Too Many Requests: {\"retryDelay\": \"7773.393684450s\"}";
        assert_eq!(
            parse_rate_limit_error(retry_delay),
            Some(Duration::from_secs(7774))
        );
    }
}
