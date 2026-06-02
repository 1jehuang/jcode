//! SSE (Server-Sent Events) chunk-arrival timeout for streaming providers.
//!
//! Issue #147: slow reasoning models (e.g. o1-style chains-of-thought, deep
//! Anthropic thinking blocks) can stall a stream for >180s while still being
//! healthy. The hard-coded `Duration::from_secs(180)` was triggering false
//! "Stream stalled" errors. Make it configurable so users with slow models
//! can extend the budget without forking jcode.
//!
//! Resolution order:
//!
//!   1. `JCODE_SSE_CHUNK_TIMEOUT_SECS` env var. Must be a positive integer
//!      (`u64`); 0 and non-numeric values fall through to (2).
//!   2. The provider-supplied default (`default_secs`).
//!
//! Each call site decides its default via the helper's argument so we can
//! keep the existing per-provider defaults (180s today) intact.

use std::time::Duration;

/// Resolve the SSE chunk-arrival timeout. See module docs for resolution
/// order.
pub(crate) fn chunk_timeout(default_secs: u64) -> Duration {
    let secs = chunk_timeout_secs(default_secs);
    Duration::from_secs(secs)
}

/// Same as `chunk_timeout`, but returns the raw seconds (useful for log
/// messages that need to interpolate the resolved value).
pub(crate) fn chunk_timeout_secs(default_secs: u64) -> u64 {
    parse_secs_env("JCODE_SSE_CHUNK_TIMEOUT_SECS").unwrap_or(default_secs)
}

fn parse_secs_env(key: &str) -> Option<u64> {
    let raw = std::env::var(key).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed: u64 = trimmed.parse().ok()?;
    if parsed == 0 { None } else { Some(parsed) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save_env(key: &str) -> Option<std::ffi::OsString> {
        let prev = std::env::var_os(key);
        crate::env::remove_var(key);
        prev
    }

    fn restore_env(key: &str, val: Option<std::ffi::OsString>) {
        match val {
            Some(v) => crate::env::set_var(key, v),
            None => crate::env::remove_var(key),
        }
    }

    #[test]
    fn falls_back_to_default_when_env_missing() {
        let _lock = crate::storage::lock_test_env();
        let prev = save_env("JCODE_SSE_CHUNK_TIMEOUT_SECS");
        assert_eq!(chunk_timeout_secs(180), 180);
        assert_eq!(chunk_timeout(180), Duration::from_secs(180));
        restore_env("JCODE_SSE_CHUNK_TIMEOUT_SECS", prev);
    }

    #[test]
    fn reads_positive_integer_from_env() {
        let _lock = crate::storage::lock_test_env();
        let prev = save_env("JCODE_SSE_CHUNK_TIMEOUT_SECS");
        crate::env::set_var("JCODE_SSE_CHUNK_TIMEOUT_SECS", "600");
        assert_eq!(chunk_timeout_secs(180), 600);
        restore_env("JCODE_SSE_CHUNK_TIMEOUT_SECS", prev);
    }

    #[test]
    fn rejects_zero_and_garbage() {
        let _lock = crate::storage::lock_test_env();
        let prev = save_env("JCODE_SSE_CHUNK_TIMEOUT_SECS");
        crate::env::set_var("JCODE_SSE_CHUNK_TIMEOUT_SECS", "0");
        assert_eq!(chunk_timeout_secs(180), 180);
        crate::env::set_var("JCODE_SSE_CHUNK_TIMEOUT_SECS", "abc");
        assert_eq!(chunk_timeout_secs(180), 180);
        crate::env::set_var("JCODE_SSE_CHUNK_TIMEOUT_SECS", "  ");
        assert_eq!(chunk_timeout_secs(180), 180);
        restore_env("JCODE_SSE_CHUNK_TIMEOUT_SECS", prev);
    }

    #[test]
    fn trims_whitespace() {
        let _lock = crate::storage::lock_test_env();
        let prev = save_env("JCODE_SSE_CHUNK_TIMEOUT_SECS");
        crate::env::set_var("JCODE_SSE_CHUNK_TIMEOUT_SECS", "  300  ");
        assert_eq!(chunk_timeout_secs(180), 300);
        restore_env("JCODE_SSE_CHUNK_TIMEOUT_SECS", prev);
    }
}
