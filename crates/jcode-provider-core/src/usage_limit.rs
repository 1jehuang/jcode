//! Stable marker for "this account is out of subscription usage".
//!
//! Runtimes report usage-limit 429s inside the response stream, as free text.
//! Account failover must not depend on that wording, so the runtimes append a
//! fixed marker (`[account-usage-limit resets_at=<unix>]`) when a 429 means
//! the account's usage is spent until a reset that is too far away to wait
//! for. A short 429 (reset within [`USAGE_LIMIT_SHORT_WAIT_SECS`], or a plain
//! `retry-after`) is not exhaustion and carries no marker: the runtime retries
//! it on the same account.

/// Marker prefix. Lowercase so lowercased error text still matches.
pub const ACCOUNT_USAGE_LIMIT_MARKER: &str = "[account-usage-limit";

/// A limit that resets within this many seconds is a short rate limit, not
/// account exhaustion.
pub const USAGE_LIMIT_SHORT_WAIT_SECS: i64 = 120;

/// Parsed marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountUsageLimit {
    /// Unix seconds when the limit resets, when the provider said so.
    pub resets_at: Option<i64>,
}

/// The marker text to append to an error message.
pub fn account_usage_limit_marker(resets_at: Option<i64>) -> String {
    match resets_at {
        Some(at) => format!("{ACCOUNT_USAGE_LIMIT_MARKER} resets_at={at}]"),
        None => format!("{ACCOUNT_USAGE_LIMIT_MARKER}]"),
    }
}

/// True when a limit that resets `resets_in_secs` from now (None = unknown)
/// should count as exhaustion. Unknown counts as exhaustion only when the
/// caller already knows it is a usage limit (not a plain rate limit).
pub fn is_far_usage_limit_reset(resets_in_secs: Option<i64>) -> bool {
    resets_in_secs.is_none_or(|secs| secs > USAGE_LIMIT_SHORT_WAIT_SECS)
}

/// Detect the marker in an error message.
pub fn classify_account_usage_limit(message: &str) -> Option<AccountUsageLimit> {
    let start = message.find(ACCOUNT_USAGE_LIMIT_MARKER)?;
    let rest = &message[start + ACCOUNT_USAGE_LIMIT_MARKER.len()..];
    let body = &rest[..rest.find(']')?];
    let resets_at = body
        .split_whitespace()
        .find_map(|part| part.strip_prefix("resets_at="))
        .and_then(|value| value.parse::<i64>().ok());
    Some(AccountUsageLimit { resets_at })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_roundtrip() {
        let text = format!(
            "Anthropic API error (429): limit. {}",
            account_usage_limit_marker(Some(1_787_286_694))
        );
        assert_eq!(
            classify_account_usage_limit(&text),
            Some(AccountUsageLimit {
                resets_at: Some(1_787_286_694)
            })
        );
        assert_eq!(
            classify_account_usage_limit(&text.to_lowercase()),
            Some(AccountUsageLimit {
                resets_at: Some(1_787_286_694)
            })
        );
        assert_eq!(
            classify_account_usage_limit(&account_usage_limit_marker(None)),
            Some(AccountUsageLimit { resets_at: None })
        );
    }

    #[test]
    fn plain_rate_limits_are_not_usage_limits() {
        for text in [
            "Anthropic API error (429 Too Many Requests): rate_limit_error",
            "Rate limited: Too many requests. Retry after 7s.",
            "usage limit reached",
            "[account-usage-limit resets_at=5",
        ] {
            assert_eq!(classify_account_usage_limit(text), None, "{text}");
        }
    }

    #[test]
    fn short_resets_are_not_far() {
        assert!(!is_far_usage_limit_reset(Some(30)));
        assert!(!is_far_usage_limit_reset(Some(120)));
        assert!(is_far_usage_limit_reset(Some(121)));
        assert!(is_far_usage_limit_reset(None));
    }
}
