//! GitHub API rate-limit detection and shared backoff for update checks.
//!
//! Unauthenticated `api.github.com` requests share a 60 req/hour per-IP bucket
//! with everything else on the machine (and everything behind the same NAT), so
//! an exhausted bucket is a normal condition rather than a failure. Detect it,
//! persist a backoff window every jcode process on the machine can see, and
//! label the error so UIs can stay quiet about it.

use super::update_metadata::UpdateMetadata;
use jcode_update_core::format_duration_estimate;
use std::time::{Duration, SystemTime};

/// How long to stop checking after GitHub reports the rate limit is exhausted
/// and gives us no usable reset hint.
pub(super) const RATE_LIMIT_BACKOFF_FALLBACK: Duration = Duration::from_secs(60 * 60);
/// Upper bound on a server-provided backoff, so a bogus reset header cannot
/// disable update checks indefinitely.
pub(super) const RATE_LIMIT_BACKOFF_MAX: Duration = Duration::from_secs(6 * 60 * 60);
/// Marker prefix on errors caused by GitHub API rate limiting, so callers can
/// present the situation as "checked too often" rather than a real failure.
pub const RATE_LIMIT_ERROR_PREFIX: &str = "GitHub API rate limit reached";

/// Detect a GitHub API rate-limit rejection, record a shared backoff window,
/// and return a recognizable error.
///
/// GitHub answers an exhausted bucket with 403 (or 429) plus
/// `x-ratelimit-remaining: 0`, so we distinguish it from a genuine
/// authorization failure and avoid hammering the API from every new session.
pub(super) fn rate_limit_error(response: &reqwest::blocking::Response) -> Option<anyhow::Error> {
    let status = response.status();
    let headers = response.headers();
    let header_num = |name: &str| -> Option<u64> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
    };
    let backoff = rate_limit_backoff(
        status.as_u16(),
        header_num("x-ratelimit-remaining"),
        header_num("retry-after"),
        header_num("x-ratelimit-reset"),
        SystemTime::now(),
    )?;
    record_rate_limit_backoff(backoff);
    // Keep this to one short line: it can surface in a status notice. The
    // remediation hint (GH_TOKEN / `gh auth login`) goes to the log instead.
    Some(anyhow::anyhow!("{}", RATE_LIMIT_ERROR_PREFIX))
}

/// How long to back off for a GitHub response, or `None` when the response is
/// not a rate-limit rejection.
///
/// Split out from the HTTP plumbing so the header interpretation is unit
/// testable: GitHub signals an exhausted bucket with 403/429 plus
/// `x-ratelimit-remaining: 0`, while a 403 with quota remaining is a genuine
/// authorization failure that must not silence update checks.
fn rate_limit_backoff(
    status: u16,
    remaining: Option<u64>,
    retry_after_secs: Option<u64>,
    reset_epoch_secs: Option<u64>,
    now: SystemTime,
) -> Option<Duration> {
    if status != 403 && status != 429 {
        return None;
    }
    if status == 403 && remaining != Some(0) {
        return None;
    }

    Some(
        retry_after_secs
            .map(Duration::from_secs)
            .or_else(|| {
                // checked_add: a garbage reset header must not panic here.
                let reset_at =
                    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(reset_epoch_secs?))?;
                reset_at.duration_since(now).ok()
            })
            .filter(|backoff| !backoff.is_zero())
            .unwrap_or(RATE_LIMIT_BACKOFF_FALLBACK)
            .min(RATE_LIMIT_BACKOFF_MAX),
    )
}

/// Persist the backoff window so every jcode process on this machine stops
/// checking, not just the one that saw the 403.
fn record_rate_limit_backoff(backoff: Duration) {
    let until = SystemTime::now() + backoff;
    if let Ok(mut metadata) = UpdateMetadata::load() {
        metadata.rate_limited_until = Some(until);
        metadata.last_check = SystemTime::now();
        let _ = metadata.save();
    }
    crate::logging::warn(&format!(
        "update: GitHub API rate limited; suppressing update checks for {}. Set GH_TOKEN/GITHUB_TOKEN or run `gh auth login` for a 5000 req/h quota.",
        format_duration_estimate(backoff)
    ));
}

/// Drop a stale backoff once a request succeeds again.
pub(super) fn clear_rate_limit_backoff() {
    if let Ok(mut metadata) = UpdateMetadata::load()
        && metadata.rate_limited_until.is_some()
    {
        metadata.rate_limited_until = None;
        let _ = metadata.save();
    }
}

/// True when this error came from GitHub API throttling rather than a real
/// update failure, so UIs can stay quiet about it.
pub fn is_rate_limit_error(error: &str) -> bool {
    error.contains(RATE_LIMIT_ERROR_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_non_rate_limit_statuses() {
        let now = SystemTime::now();
        assert!(rate_limit_backoff(200, None, None, None, now).is_none());
        assert!(rate_limit_backoff(404, None, None, None, now).is_none());
        // 403 with quota left is a real authorization failure.
        assert!(rate_limit_backoff(403, Some(37), None, None, now).is_none());
        assert!(rate_limit_backoff(403, None, None, None, now).is_none());
    }

    #[test]
    fn uses_reset_header() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let backoff = rate_limit_backoff(403, Some(0), None, Some(1_600), now).unwrap();
        assert_eq!(backoff, Duration::from_secs(600));
    }

    #[test]
    fn prefers_retry_after_and_clamps() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(
            rate_limit_backoff(429, None, Some(90), Some(9_999), now).unwrap(),
            Duration::from_secs(90)
        );
        // A bogus far-future reset must not disable checks forever.
        assert_eq!(
            rate_limit_backoff(429, Some(0), None, Some(u64::MAX / 2), now).unwrap(),
            RATE_LIMIT_BACKOFF_MAX
        );
        // Past/zero hints fall back to the default window.
        assert_eq!(
            rate_limit_backoff(429, Some(0), None, Some(10), now).unwrap(),
            RATE_LIMIT_BACKOFF_FALLBACK
        );
    }

    #[test]
    fn matches_wrapped_error_message() {
        assert!(is_rate_limit_error(&format!(
            "Check failed: {}; skipping",
            RATE_LIMIT_ERROR_PREFIX
        )));
        assert!(!is_rate_limit_error("Check failed: connection reset"));
    }
}
