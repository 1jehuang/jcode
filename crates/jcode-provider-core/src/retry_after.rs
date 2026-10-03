//! Shared handling for provider `Retry-After` hints.
//!
//! Provider runtimes keep their own retry classification and request logic, but
//! parsing an untrusted server delay and carrying it through an `anyhow::Error`
//! should be consistent. Delays are capped so a malformed or hostile upstream
//! cannot stall a turn indefinitely.

use anyhow::Error;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use std::fmt;
use std::time::{Duration, Instant, SystemTime};

/// Longest server-requested delay a provider retry loop will honor.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Parse a `Retry-After` header as delta-seconds or an HTTP date.
///
/// Numeric values are parsed with saturation and then capped, so even an
/// arbitrarily long digit string is safe. Invalid values are ignored and let
/// the caller fall back to its normal exponential backoff.
pub fn retry_after(headers: &HeaderMap) -> Option<RetryAfter> {
    retry_after_delay_at(headers, SystemTime::now()).map(RetryAfter::new)
}

/// Parse a bounded retry delay from a provider JSON error body.
///
/// Some providers (e.g. Openference) answer 429 rate limits with the delay
/// only in the JSON body (`"retry_after_seconds": 2`) and no `Retry-After`
/// header. Only that exact numeric field is honored, wherever it is nested:
/// sibling limit fields such as `max_rpm` describe the account rather than
/// the wait, and string values are rejected so a malformed or hostile body
/// cannot stall a turn. Returns the raw delay, not a deadline, so callers
/// that must report the exact server-requested value keep full precision.
pub fn retry_after_body_delay(body: &str) -> Option<Duration> {
    let value = serde_json::Deserializer::from_str(body)
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()?;
    find_retry_after_seconds(&value)
        .map(|seconds| Duration::from_secs(seconds).min(MAX_RETRY_AFTER))
}

/// Body counterpart of [`retry_after`]: parse a JSON error body for a
/// server-requested delay and wrap it as a monotonic retry hint.
pub fn retry_after_body(body: &str) -> Option<RetryAfter> {
    retry_after_body_delay(body).map(RetryAfter::new)
}

/// Depth-first search for the first numeric, positive `retry_after_seconds`.
fn find_retry_after_seconds(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Object(map) => map
            .get("retry_after_seconds")
            .and_then(positive_seconds)
            .or_else(|| map.values().find_map(find_retry_after_seconds)),
        serde_json::Value::Array(items) => items.iter().find_map(find_retry_after_seconds),
        _ => None,
    }
}

/// Fractional hints round up: waiting slightly longer than asked never
/// trips the limit again. `as u64` saturates, and the caller caps the
/// result, so an oversized or hostile value stays bounded.
fn positive_seconds(value: &serde_json::Value) -> Option<u64> {
    let seconds = value.as_f64()?;
    (seconds.is_finite() && seconds > 0.0).then(|| seconds.ceil() as u64)
}

fn retry_after_delay_at(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if value.is_empty() {
        return None;
    }

    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        let max_secs = MAX_RETRY_AFTER.as_secs();
        let seconds = value.bytes().fold(0u64, |seconds, byte| {
            seconds
                .saturating_mul(10)
                .saturating_add(u64::from(byte - b'0'))
                .min(max_secs)
        });
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(
        retry_at
            .duration_since(now)
            .unwrap_or(Duration::ZERO)
            .min(MAX_RETRY_AFTER),
    )
}

/// A bounded server retry hint represented as a monotonic deadline.
#[derive(Clone, Copy, Debug)]
pub struct RetryAfter {
    deadline: Instant,
}

impl RetryAfter {
    fn new(delay: Duration) -> Self {
        Self {
            deadline: Instant::now() + delay,
        }
    }

    /// Time still remaining on the hint. Time spent reading and classifying an
    /// error response counts toward the requested wait.
    pub fn remaining(self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// Error wrapper that preserves the provider's user-facing message while
/// carrying a parsed server retry deadline to the outer retry loop.
#[derive(Debug)]
struct RetryAfterError {
    message: String,
    retry_after: RetryAfter,
}

impl fmt::Display for RetryAfterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RetryAfterError {}

/// Build an error with an optional server retry hint without changing its
/// display text.
pub fn error_with_retry_after(message: String, retry_after: Option<RetryAfter>) -> Error {
    match retry_after {
        Some(retry_after) => Error::new(RetryAfterError {
            message,
            retry_after,
        }),
        None => Error::msg(message),
    }
}

/// Recover a server retry hint from a provider error, including through anyhow
/// context layers.
pub fn retry_after_from_error(error: &Error) -> Option<Duration> {
    error
        .chain()
        .find_map(|source| source.downcast_ref::<RetryAfterError>())
        .map(|error| error.retry_after.remaining())
}

/// Select the delay before a retry, preferring a validated server hint over
/// the provider's normal jittered exponential backoff.
pub fn retry_delay(attempt: u32, base_ms: u64, server_hint: Option<Duration>) -> Duration {
    server_hint.unwrap_or_else(|| crate::attempt_tracker::retry_backoff_delay(attempt, base_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(value: HeaderValue) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, value);
        headers
    }

    #[test]
    fn parses_delta_seconds_without_sleeping() {
        assert_eq!(
            retry_after_delay_at(
                &headers(HeaderValue::from_static("7")),
                SystemTime::UNIX_EPOCH,
            ),
            Some(Duration::from_secs(7))
        );
    }

    #[test]
    fn parses_http_date_relative_to_injected_clock() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let retry_at = now + Duration::from_secs(12);
        let value = HeaderValue::from_str(&httpdate::fmt_http_date(retry_at)).unwrap();
        assert_eq!(
            retry_after_delay_at(&headers(value), now),
            Some(Duration::from_secs(12))
        );
    }

    #[test]
    fn past_http_date_requests_no_additional_wait() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let value =
            HeaderValue::from_str(&httpdate::fmt_http_date(now - Duration::from_secs(30))).unwrap();
        assert_eq!(
            retry_after_delay_at(&headers(value), now),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn far_future_http_date_is_capped() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let value =
            HeaderValue::from_str(&httpdate::fmt_http_date(now + Duration::from_secs(3_600)))
                .unwrap();
        assert_eq!(
            retry_after_delay_at(&headers(value), now),
            Some(MAX_RETRY_AFTER)
        );
    }

    #[test]
    fn malformed_retry_after_is_ignored() {
        assert_eq!(
            retry_after_delay_at(
                &headers(HeaderValue::from_static("not-a-delay")),
                SystemTime::UNIX_EPOCH,
            ),
            None
        );
    }

    #[test]
    fn openference_json_retry_after_seconds_does_not_use_max_rpm() {
        let body = r#"{"error":"Rate limit exceeded. Too many requests per minute.","type":"rate_limit_error","code":"rate_limit_exceeded","retry_after_seconds":2,"max_rpm":25}"#;
        assert_eq!(retry_after_body_delay(body), Some(Duration::from_secs(2)));
    }

    #[test]
    fn body_retry_after_rounds_up_nested_fractional_delays() {
        assert_eq!(
            retry_after_body_delay(r#"{"error":{"retry_after_seconds":2.5}}"#),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn body_retry_after_is_bounded_and_zero_uses_default_backoff() {
        assert_eq!(retry_after_body_delay(r#"{"retry_after_seconds":0}"#), None);
        assert_eq!(
            retry_after_body_delay(r#"{"retry_after_seconds":18446744073709551615}"#),
            Some(MAX_RETRY_AFTER)
        );
    }

    #[test]
    fn invalid_body_retry_hints_are_ignored() {
        for body in [
            r#"{"retry_after_seconds":-1,"max_rpm":25}"#,
            r#"{"retry_after_seconds":null}"#,
            r#"{"retry_after_seconds":true}"#,
            r#"{"retry_after_seconds":"NaN"}"#,
            r#"{"retry_after_seconds":"inf"}"#,
            r#"{"retry_after_seconds":"soon"}"#,
            r#"{"retry_after_seconds":{}}"#,
            r#"{"max_rpm":25}"#,
            r#"{"retry_after_seconds":2"#,
            "null",
            "[]",
        ] {
            assert_eq!(retry_after_body_delay(body), None, "{body}");
        }
    }

    #[test]
    fn body_hint_survives_formatted_diagnostics_and_error_context() {
        let hint = retry_after_body("{\"retry_after_seconds\":2}\nHint: provider is rate limited");
        let error =
            error_with_retry_after("rate limited".to_string(), hint).context("request failed");
        let remaining = retry_after_from_error(&error).expect("body hint preserved");
        assert!(remaining <= Duration::from_secs(2));
        assert!(remaining > Duration::from_secs(1));
    }

    #[test]
    fn oversized_retry_after_is_capped_even_when_it_overflows_u64() {
        let value = HeaderValue::from_static("999999999999999999999999999999999999999999");
        assert_eq!(
            retry_after_delay_at(&headers(value), SystemTime::UNIX_EPOCH),
            Some(MAX_RETRY_AFTER)
        );
    }

    /// Openference answers 429 rate limits with the delay only in the JSON
    /// body (`retry_after_seconds: 2`, `max_rpm: 25`) and no `Retry-After`
    /// header. The exact 2s must survive parsing so bounded holds and retry
    /// delays match what the server asked for.
    #[test]
    fn openference_body_retry_after_seconds_is_parsed_exactly() {
        let body = "{\"error\":\"Rate limit exceeded. Too many requests per minute.\",\"type\":\"rate_limit_error\",\"code\":\"rate_limit_exceeded\",\"retry_after_seconds\":2,\"max_rpm\":25}";
        assert_eq!(retry_after_body_delay(body), Some(Duration::from_secs(2)));
    }

    #[test]
    fn body_retry_after_seconds_is_found_nested_in_error_object() {
        let body = "{\"error\":{\"message\":\"rate limit exceeded\",\"metadata\":{\"retry_after_seconds\":3}}}";
        assert_eq!(retry_after_body_delay(body), Some(Duration::from_secs(3)));
    }

    #[test]
    fn body_string_retry_after_seconds_and_limit_fields_are_ignored() {
        // A string hint and account limit fields must not be mistaken for
        // the wait; a malformed body falls back to the caller's backoff.
        assert_eq!(
            retry_after_body_delay("{\"retry_after_seconds\":\"2\"}"),
            None
        );
        assert_eq!(retry_after_body_delay("{\"max_rpm\":25}"), None);
        assert_eq!(
            retry_after_body_delay("{\"reset\":\"2026-10-03\",\"retry_at\":3600}"),
            None
        );
        assert_eq!(retry_after_body_delay("rate limited"), None);
        assert_eq!(retry_after_body_delay("{\"broken\":"), None);
    }

    #[test]
    fn body_retry_after_seconds_is_bounded() {
        assert_eq!(retry_after_body_delay("{\"retry_after_seconds\":0}"), None);
        assert_eq!(retry_after_body_delay("{\"retry_after_seconds\":-1}"), None);
        assert_eq!(
            retry_after_body_delay("{\"retry_after_seconds\":999999999}"),
            Some(MAX_RETRY_AFTER)
        );
        // Fractional hints round up to whole seconds.
        assert_eq!(
            retry_after_body_delay("{\"retry_after_seconds\":1.2}"),
            Some(Duration::from_secs(2))
        );
    }

    #[test]
    fn error_hint_round_trips_without_changing_message() {
        let error = error_with_retry_after(
            "rate limited".to_string(),
            Some(RetryAfter::new(Duration::from_secs(9))),
        )
        .context("request failed");
        assert_eq!(format!("{error:#}"), "request failed: rate limited");
        let remaining = retry_after_from_error(&error).unwrap();
        assert!(remaining <= Duration::from_secs(9));
        assert!(remaining > Duration::from_secs(8));
    }

    #[test]
    fn server_hint_replaces_backoff_without_sleeping() {
        assert_eq!(
            retry_delay(3, 10_000, Some(Duration::from_secs(4))),
            Duration::from_secs(4)
        );
    }

    #[test]
    fn elapsed_hint_does_not_add_another_wait() {
        let retry_after = RetryAfter {
            deadline: Instant::now() - Duration::from_secs(1),
        };
        let error = error_with_retry_after("rate limited".to_string(), Some(retry_after));
        assert_eq!(retry_after_from_error(&error), Some(Duration::ZERO));
    }
}
