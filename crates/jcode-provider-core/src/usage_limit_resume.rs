//! When to resume a turn that stopped on a subscription usage limit.
//!
//! A provider runtime fails a request fast when the account's usage limit
//! resets long after its own retry window. The turn is not broken, it just has
//! to wait for the reset. This module makes that reset machine-readable
//! ([`UsageLimitReset`] on the error chain, or parsed from the error text) and
//! owns the policy for scheduling a resume, so no caller can retry sooner than
//! the reported reset or loop without bound when every account is out.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Resumes allowed per turn after the first attempt hits a usage limit.
pub const MAX_USAGE_LIMIT_RESUMES: u32 = 3;

/// Wait used when the reported reset has already passed. Never retry sooner.
pub const PAST_RESET_RESUME_DELAY: Duration = Duration::from_secs(60);

/// Wait used when the limit reported no reset time at all.
pub const UNKNOWN_RESET_RESUME_DELAY: Duration = Duration::from_secs(15 * 60);

/// Random delay added after the reset, so many sessions do not all retry in
/// the same second.
pub const RESUME_JITTER_SECS: std::ops::RangeInclusive<u64> = 5..=30;

/// Error wrapper that records when a usage limit resets. Its display text is
/// the wrapped error's message and the wrapped error is its source, so
/// user-facing text and `chain()` downcasts to the original keep working.
#[derive(Debug)]
pub struct UsageLimitReset {
    source: anyhow::Error,
    resets_at: Option<SystemTime>,
}

impl UsageLimitReset {
    /// Time left until the reset, `Some(ZERO)` once it passed, `None` when
    /// the provider did not report one.
    pub fn reset_in(&self) -> Option<Duration> {
        self.resets_at.map(|at| {
            at.duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO)
        })
    }
}

impl fmt::Display for UsageLimitReset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.source, f)
    }
}

impl std::error::Error for UsageLimitReset {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Tag `error` as a usage limit that resets after `reset_in`.
pub fn with_usage_limit_reset(error: anyhow::Error, reset_in: Option<Duration>) -> anyhow::Error {
    anyhow::Error::new(UsageLimitReset {
        source: error,
        resets_at: reset_in.map(|delay| SystemTime::now() + delay),
    })
}

/// A usage limit found on an error, with the time left until it resets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsageLimitHint {
    /// `None` when neither the error nor its text names a reset time.
    /// `Some(ZERO)` when the named reset already passed.
    pub reset_in: Option<Duration>,
}

impl UsageLimitHint {
    /// Seconds until the reset, rounded up, for `retry_after_secs`.
    pub fn reset_in_secs(&self) -> Option<u64> {
        self.reset_in
            .map(|delay| delay.as_secs() + u64::from(delay.subsec_nanos() > 0))
    }
}

/// Detect a subscription usage limit on `error`: first the typed
/// [`UsageLimitReset`], then the error text (other runtimes, and
/// multi-account messages that name the earliest reset across accounts).
/// A precise typed reset wins over text parsed from the same error, since
/// the text is a rounded display form of it.
pub fn usage_limit_hint(error: &anyhow::Error) -> Option<UsageLimitHint> {
    let typed = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<UsageLimitReset>())
        .map(UsageLimitReset::reset_in);
    let text = error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ");
    let parsed = usage_limit_hint_from_text(&text, unix_now());
    match (typed, parsed) {
        (None, None) => None,
        (Some(reset), None) => Some(UsageLimitHint { reset_in: reset }),
        (None, Some(hint)) => Some(hint),
        (Some(Some(reset)), Some(_)) => Some(UsageLimitHint {
            reset_in: Some(reset),
        }),
        (Some(None), Some(hint)) => Some(hint),
    }
}

fn earliest(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Text form of [`usage_limit_hint`]. Recognizes a usage limit (not a plain
/// rate limit) and reads every reset it names: the account-usage-limit marker
/// (`resets_at=<unix>`), "First reset: <account> at 14:05 (in 1h12m)", and
/// "resets in 3h 17m". The earliest one wins. Rounded display durations are
/// rounded up to the end of their smallest unit, so the result is never
/// earlier than the real reset.
pub fn usage_limit_hint_from_text(text: &str, now_unix: i64) -> Option<UsageLimitHint> {
    let lower = text.to_ascii_lowercase();
    let is_usage_limit = [
        "usage limit",
        "usage_limit",
        "[account-usage-limit",
        "out of usage",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if !is_usage_limit {
        return None;
    }

    let mut reset_in: Option<Duration> = None;
    let mut consider = |candidate: Duration| {
        reset_in = earliest(reset_in, Some(candidate));
    };
    for (index, _) in lower.match_indices("resets_at=") {
        let digits: String = lower[index + "resets_at=".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(at) = digits.parse::<i64>() {
            consider(Duration::from_secs(
                at.saturating_sub(now_unix).max(0) as u64
            ));
        }
    }
    for (index, _) in lower.match_indices("first reset:") {
        let rest = &lower[index..];
        let rest = &rest[..rest.find(')').unwrap_or(rest.len())];
        if let Some(at) = rest.find("(in ")
            && let Some(delay) = parse_compact_duration_ceil(&rest[at + "(in ".len()..])
        {
            consider(delay);
        }
    }
    for (index, _) in lower.match_indices("resets in ") {
        if let Some(delay) = parse_compact_duration_ceil(&lower[index + "resets in ".len()..]) {
            consider(delay);
        }
    }
    Some(UsageLimitHint { reset_in })
}

/// [`parse_compact_duration`] rounded up to the end of its smallest unit:
/// "59m" is anywhere in [59m, 60m), so it becomes 59m59s.
fn parse_compact_duration_ceil(text: &str) -> Option<Duration> {
    parse_compact_duration(text)
        .map(|(total, smallest_unit)| total + Duration::from_secs(smallest_unit - 1))
}

/// Parse a leading "1h12m", "3h 17m", "30d 4h 29m", "45s" or "5 minutes"
/// duration, with the smallest unit (in seconds) it named. `None` when the
/// text does not start with one.
fn parse_compact_duration(text: &str) -> Option<(Duration, u64)> {
    let mut chars = text.trim_start().chars().peekable();
    let mut total: u64 = 0;
    let mut smallest_unit: Option<u64> = None;
    loop {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let mut digits = String::new();
        while let Some(digit) = chars.peek().copied().filter(char::is_ascii_digit) {
            digits.push(digit);
            chars.next();
        }
        if digits.is_empty() {
            break;
        }
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let mut unit = String::new();
        while let Some(letter) = chars.peek().copied().filter(char::is_ascii_alphabetic) {
            unit.push(letter);
            chars.next();
        }
        let scale = match unit.as_str() {
            "d" | "day" | "days" => 86_400,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            _ => break,
        };
        let value = digits.parse::<u64>().ok()?;
        total = total.saturating_add(value.saturating_mul(scale));
        smallest_unit = Some(smallest_unit.map_or(scale, |unit: u64| unit.min(scale)));
    }
    smallest_unit.map(|unit| (Duration::from_secs(total), unit))
}

/// How long to wait before resuming after a usage limit that resets in
/// `reset_in` (see [`UsageLimitHint::reset_in`]). Never zero: a future reset
/// waits for the reset plus `jitter`, a past reset waits
/// [`PAST_RESET_RESUME_DELAY`], and an unknown reset waits
/// [`UNKNOWN_RESET_RESUME_DELAY`].
pub fn usage_limit_resume_delay(reset_in: Option<Duration>, jitter: Duration) -> Duration {
    match reset_in {
        Some(delay) if !delay.is_zero() => delay + jitter,
        Some(_) => PAST_RESET_RESUME_DELAY + jitter,
        None => UNKNOWN_RESET_RESUME_DELAY + jitter,
    }
}

/// A random jitter in [`RESUME_JITTER_SECS`].
pub fn usage_limit_resume_jitter() -> Duration {
    Duration::from_secs(rand::random_range(RESUME_JITTER_SECS))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    #[test]
    fn precise_typed_reset_wins_over_rounded_text_on_the_same_error() {
        let error = with_usage_limit_reset(
            anyhow::anyhow!("Usage limit reached for this Claude account; resets in 59m."),
            Some(Duration::from_secs(3569)),
        );
        let secs = usage_limit_hint(&error)
            .and_then(|hint| hint.reset_in_secs())
            .expect("reset");
        assert!((3568..=3569).contains(&secs), "{secs}");
    }

    #[test]
    fn text_only_rounded_reset_is_rounded_up() {
        let hint = usage_limit_hint_from_text("Usage limit reached; resets in 59m.", NOW)
            .expect("usage limit");
        assert!(hint.reset_in.expect("reset") >= Duration::from_secs(3540 + 59));
    }

    #[test]
    fn anthropic_fail_fast_message_is_a_usage_limit_with_its_reset() {
        let text = "Anthropic API error (429 Too Many Requests): {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"This request would exceed your account's rate limit. Please try again later.\"}} Usage limit reached for this Claude account; resets in 40m (2026-09-30 13:30 UTC).";
        assert_eq!(
            usage_limit_hint_from_text(text, NOW),
            Some(UsageLimitHint {
                reset_in: Some(Duration::from_secs(40 * 60 + 59))
            })
        );
    }

    #[test]
    fn plain_rate_limits_are_not_usage_limits() {
        for text in [
            "Anthropic API error (429 Too Many Requests): rate_limit_error",
            "Rate limited: Too many requests. Retry after 7s.",
            "connection reset by peer",
        ] {
            assert_eq!(usage_limit_hint_from_text(text, NOW), None, "{text}");
        }
    }

    #[test]
    fn earliest_reset_across_accounts_wins() {
        let text = format!(
            "Usage limit reached; resets in 3h 17m. [account-usage-limit resets_at={}] All 3 Claude accounts are out of usage. First reset: claude-fox at 14:05 (in 1h12m).",
            NOW + 5 * 3600
        );
        assert_eq!(
            usage_limit_hint_from_text(&text, NOW).unwrap().reset_in,
            Some(Duration::from_secs(3600 + 12 * 60 + 59))
        );
    }

    #[test]
    fn marker_reset_in_the_past_is_zero_and_missing_reset_is_none() {
        let past = format!("[account-usage-limit resets_at={}]", NOW - 30);
        assert_eq!(
            usage_limit_hint_from_text(&past, NOW).unwrap().reset_in,
            Some(Duration::ZERO)
        );
        assert_eq!(
            usage_limit_hint_from_text("Usage limit reached for this Claude account.", NOW)
                .unwrap()
                .reset_in,
            None
        );
    }

    #[test]
    fn resume_delay_is_never_sooner_than_the_reset_and_never_zero() {
        let jitter = Duration::from_secs(5);
        assert_eq!(
            usage_limit_resume_delay(Some(Duration::from_secs(20)), jitter),
            Duration::from_secs(25)
        );
        assert_eq!(
            usage_limit_resume_delay(Some(Duration::ZERO), Duration::ZERO),
            PAST_RESET_RESUME_DELAY
        );
        assert_eq!(
            usage_limit_resume_delay(None, Duration::ZERO),
            UNKNOWN_RESET_RESUME_DELAY
        );
        for _ in 0..50 {
            let jitter = usage_limit_resume_jitter();
            assert!(RESUME_JITTER_SECS.contains(&jitter.as_secs()));
        }
    }

    #[test]
    fn typed_reset_survives_context_and_keeps_the_display_text() {
        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Anthropic API error (429): limit")
            }
        }
        impl std::error::Error for Inner {}

        let error =
            with_usage_limit_reset(anyhow::Error::new(Inner), Some(Duration::from_secs(90)))
                .context("turn failed");
        assert!(
            error
                .chain()
                .any(|cause| cause.downcast_ref::<Inner>().is_some())
        );
        let hint = usage_limit_hint(&error).expect("typed usage limit");
        let secs = hint.reset_in_secs().unwrap();
        assert!((89..=90).contains(&secs), "{secs}");
    }
}
