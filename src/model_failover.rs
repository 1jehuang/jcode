//! Issue #39: automatic failover to other models on rate-limit / 5xx.
//!
//! Primitive for an env-var-configured failover chain. When the
//! current model returns a rate-limit / 5xx / quota-exhausted error,
//! the agent walks the failover chain in order and retries with the
//! next model.
//!
//! ## Configuration
//!
//! ```bash
//! # Comma-separated chain of (provider:model) fallbacks, tried in order.
//! JCODE_FAILOVER_CHAIN=anthropic:claude-sonnet-4,openai:gpt-5.5,openrouter:google/gemini-2.5-pro
//! ```
//!
//! ## API
//!
//! ```rust
//! use jcode::model_failover;
//!
//! let chain = model_failover::current_chain();   // Vec<FailoverEntry>
//! for entry in chain {
//!     // try entry.provider with entry.model …
//! }
//!
//! // Detect retryable failures from a provider error string:
//! if model_failover::is_retryable(error_msg) {
//!     // advance to next failover entry
//! }
//! ```
//!
//! ## Out of scope (#39 follow-up)
//!
//! - Wiring chain advance into the agent's turn loop. Requires
//!   careful integration with cost accounting + provider switching
//!   without losing conversation state. Will land separately.
//! - Per-model cooldown windows (don't immediately retry a model
//!   that just rate-limited).

use std::time::Duration;

/// A single failover destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailoverEntry {
    /// Provider id (e.g. "anthropic", "openai", "openrouter",
    /// "openai-compatible"). Empty when the user didn't include a
    /// provider prefix.
    pub provider: String,
    /// Model id (e.g. "claude-sonnet-4", "gpt-5.5").
    pub model: String,
}

/// Parse the active failover chain from `JCODE_FAILOVER_CHAIN`.
/// Empty/unset env returns `Vec::new()`.
///
/// Format: `provider:model,provider:model,...`. Whitespace is
/// trimmed. Entries without a `:` separator are treated as bare
/// model ids (provider field empty — caller falls back to the
/// active provider).
pub fn current_chain() -> Vec<FailoverEntry> {
    let Ok(raw) = std::env::var("JCODE_FAILOVER_CHAIN") else {
        return Vec::new();
    };
    parse_chain(&raw)
}

pub fn parse_chain(raw: &str) -> Vec<FailoverEntry> {
    raw.split(',')
        .filter_map(|spec| {
            let spec = spec.trim();
            if spec.is_empty() {
                return None;
            }
            let (provider, model) = match spec.split_once(':') {
                Some((p, m)) => (p.trim().to_string(), m.trim().to_string()),
                None => (String::new(), spec.to_string()),
            };
            if model.is_empty() {
                return None;
            }
            Some(FailoverEntry { provider, model })
        })
        .collect()
}

/// Recommended cooldown for a freshly rate-limited model in this
/// chain. Defaults to 30s; tunable via env so corporate users with
/// stricter quotas can extend.
pub fn rate_limit_cooldown() -> Duration {
    let secs = std::env::var("JCODE_FAILOVER_COOLDOWN_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(30);
    Duration::from_secs(secs)
}

/// Heuristic: should we advance to the next failover entry on this
/// error message? Matches common rate-limit / overload / 5xx
/// signals across providers. Conservative — only triggers on
/// clearly retryable errors.
pub fn is_retryable(error_msg: &str) -> bool {
    let lower = error_msg.to_ascii_lowercase();
    // Anthropic / OpenAI / OpenRouter common signals
    const SIGNALS: &[&str] = &[
        "rate_limit",
        "rate limit",
        "rate-limited",
        "ratelimit",
        "429",
        "503",
        "502",
        "504",
        "overloaded",
        "overloaded_error",
        "service unavailable",
        "bad gateway",
        "gateway timeout",
        "quota_exceeded",
        "quota exceeded",
        "insufficient_quota",
        "context_length_exceeded", // not technically retryable, but failover to a wider-context model can help
        "model_overloaded",
    ];
    SIGNALS.iter().any(|sig| lower.contains(sig))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save_keys(keys: &[&'static str]) -> Vec<(&'static str, Option<std::ffi::OsString>)> {
        keys.iter().map(|k| (*k, std::env::var_os(k))).collect()
    }
    fn restore(saved: Vec<(&'static str, Option<std::ffi::OsString>)>) {
        for (k, v) in saved {
            unsafe {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn empty_when_unset() {
        let _lock = crate::storage::lock_test_env();
        let saved = save_keys(&["JCODE_FAILOVER_CHAIN"]);
        crate::env::remove_var("JCODE_FAILOVER_CHAIN");
        assert!(current_chain().is_empty());
        restore(saved);
    }

    #[test]
    fn parses_provider_model_pairs() {
        let chain = parse_chain(
            "anthropic:claude-sonnet-4, openai:gpt-5.5, openrouter:google/gemini-2.5-pro",
        );
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].provider, "anthropic");
        assert_eq!(chain[0].model, "claude-sonnet-4");
        assert_eq!(chain[2].model, "google/gemini-2.5-pro");
    }

    #[test]
    fn parses_bare_model_ids() {
        let chain = parse_chain("gpt-5.5,claude-sonnet-4");
        assert_eq!(chain.len(), 2);
        assert!(chain[0].provider.is_empty());
        assert_eq!(chain[0].model, "gpt-5.5");
    }

    #[test]
    fn skips_blank_entries() {
        let chain = parse_chain(",,anthropic:claude,,,openai:gpt-5,");
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn current_chain_reads_env() {
        let _lock = crate::storage::lock_test_env();
        let saved = save_keys(&["JCODE_FAILOVER_CHAIN"]);
        crate::env::set_var(
            "JCODE_FAILOVER_CHAIN",
            "anthropic:claude-sonnet-4,openai:gpt-5.5",
        );
        let chain = current_chain();
        assert_eq!(chain.len(), 2);
        restore(saved);
    }

    #[test]
    fn cooldown_default_30s() {
        let _lock = crate::storage::lock_test_env();
        let saved = save_keys(&["JCODE_FAILOVER_COOLDOWN_SECS"]);
        crate::env::remove_var("JCODE_FAILOVER_COOLDOWN_SECS");
        assert_eq!(rate_limit_cooldown(), Duration::from_secs(30));
        restore(saved);
    }

    #[test]
    fn cooldown_respects_override() {
        let _lock = crate::storage::lock_test_env();
        let saved = save_keys(&["JCODE_FAILOVER_COOLDOWN_SECS"]);
        crate::env::set_var("JCODE_FAILOVER_COOLDOWN_SECS", "120");
        assert_eq!(rate_limit_cooldown(), Duration::from_secs(120));
        restore(saved);
    }

    #[test]
    fn is_retryable_catches_rate_limit() {
        assert!(is_retryable("Error 429: rate_limit"));
        assert!(is_retryable("rate limit exceeded"));
        assert!(is_retryable("RATE-LIMITED"));
    }

    #[test]
    fn is_retryable_catches_5xx() {
        assert!(is_retryable("503 Service Unavailable"));
        assert!(is_retryable("502 Bad Gateway"));
        assert!(is_retryable("504 gateway timeout"));
    }

    #[test]
    fn is_retryable_catches_overloaded() {
        assert!(is_retryable("Anthropic returned overloaded_error"));
        assert!(is_retryable("model_overloaded — please retry"));
    }

    #[test]
    fn is_retryable_catches_quota() {
        assert!(is_retryable("insufficient_quota"));
        assert!(is_retryable("quota exceeded for organization"));
    }

    #[test]
    fn is_retryable_rejects_user_errors() {
        assert!(!is_retryable("invalid_request_error: missing api key"));
        assert!(!is_retryable("400 Bad Request"));
        assert!(!is_retryable("404 Not Found"));
    }
}
