//! DeepSeek prefix cache stability primitives (gap close on #145).
//!
//! When `profile=deepseek`, this module helps keep the message prefix
//! byte-stable across turns, maximizing DeepSeek's automatic
//! prefix-cache hit rate. Adapted from upstream PR
//! [1jehuang/jcode#194](https://github.com/1jehuang/jcode/pull/194).
//!
//! ## What's shipped here (minimal port)
//!
//! - `is_prefix_cache_stable_mode()` — detect if the user is running
//!   against DeepSeek (OpenRouter cache namespace / runtime provider /
//!   named profile env signals). The agent / turn loop checks this
//!   before applying any cache-stability strategy.
//! - `preflight_check_simple(estimate_tokens, ctx_max)` — local
//!   preflight that catches oversized payloads before sending.
//!   Returns whether action is needed + the over-cap ratio.
//! - `recommended_tool_result_cap_tokens()` — returns the recommended
//!   tool-result truncation size in tokens (3000) when prefix-cache
//!   stability mode is active.
//!
//! ## Out of scope (#145 follow-up)
//!
//! - Wiring `decide_after_usage` into the turn loop (history folding
//!   logic). Requires changes to agent.rs + turn_loops.rs that touch
//!   ~200 lines and need careful test coverage; will land separately.
//! - Tokenizer-aware request estimation. Currently uses chars/4
//!   heuristic — good enough for preflight, off for actual budgeting.
//!
//! ## Why split into a primitive PR
//!
//! Upstream's PR #194 is 531 lines mixing the detection + preflight
//! primitives (small, clear) with deep changes to agent state
//! machinery (large, risky). Landing the primitive first gives
//! reviewers something concrete to evaluate before the agent wiring
//! lands on top.

/// DeepSeek V4 / direct API context window (1M tokens).
pub const DEEPSEEK_V4_CONTEXT_TOKENS: usize = 1_000_000;

/// Default fallback context window for non-DeepSeek models.
pub const DEFAULT_CONTEXT_TOKENS: usize = 128_000;

/// Threshold at which we consider folding turn history.
pub const HISTORY_FOLD_THRESHOLD: f64 = 0.5;
/// Tail budget after a normal fold, as fraction of `ctx_max`.
pub const HISTORY_FOLD_TAIL_FRACTION: f64 = 0.2;
/// Aggressive fold threshold.
pub const HISTORY_FOLD_AGGRESSIVE_THRESHOLD: f64 = 0.7;
/// Aggressive tail fraction.
pub const HISTORY_FOLD_AGGRESSIVE_TAIL_FRACTION: f64 = 0.1;
/// Force-summary exit threshold.
pub const FORCE_SUMMARY_THRESHOLD: f64 = 0.8;
/// Emergency preflight threshold (over this → must take action).
pub const PREFLIGHT_EMERGENCY_THRESHOLD: f64 = 0.95;
/// Turn-end tool-result cap in tokens.
pub const TURN_END_RESULT_CAP_TOKENS: usize = 3000;

/// Detect if prefix-cache stable mode should be active for the
/// current process.
///
/// Checks three env signals (any one is enough):
///   - `JCODE_OPENROUTER_CACHE_NAMESPACE=deepseek`
///   - `JCODE_RUNTIME_PROVIDER=deepseek`
///   - `JCODE_NAMED_PROVIDER_PROFILE=deepseek`
///
/// Comparisons are case-insensitive + whitespace-trimmed.
pub fn is_prefix_cache_stable_mode() -> bool {
    for key in [
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_NAMED_PROVIDER_PROFILE",
    ] {
        if let Ok(val) = std::env::var(key)
            && val.trim().eq_ignore_ascii_case("deepseek")
        {
            return true;
        }
    }
    false
}

/// Resolve the context window in tokens for a given model id.
///
/// Returns [`DEEPSEEK_V4_CONTEXT_TOKENS`] when the model id mentions
/// DeepSeek's 1M-context family, [`DEFAULT_CONTEXT_TOKENS`] otherwise.
pub fn context_tokens_for_model(model: &str) -> usize {
    let lower = model.to_ascii_lowercase();
    if lower.contains("deepseek") && (lower.contains("v4") || lower.contains("1m")) {
        DEEPSEEK_V4_CONTEXT_TOKENS
    } else {
        DEFAULT_CONTEXT_TOKENS
    }
}

/// Decision returned by [`preflight_check_simple`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreflightDecision {
    /// Whether action (compact / abort) is required.
    pub needs_action: bool,
    /// Ratio of estimate to context window. > 1.0 means estimate
    /// already exceeds the window.
    pub ratio: f64,
}

/// Local preflight given pre-computed token estimates.
///
/// `needs_action` triggers when `ratio > PREFLIGHT_EMERGENCY_THRESHOLD`.
pub fn preflight_check_simple(estimate_tokens: usize, ctx_max: usize) -> PreflightDecision {
    let ratio = if ctx_max > 0 {
        estimate_tokens as f64 / ctx_max as f64
    } else {
        0.0
    };
    PreflightDecision {
        needs_action: ratio > PREFLIGHT_EMERGENCY_THRESHOLD,
        ratio,
    }
}

/// Recommended cap for tool result truncation when prefix-cache
/// stability mode is active. Returns `None` when not active so
/// callers can skip the truncation pass.
pub fn recommended_tool_result_cap_tokens() -> Option<usize> {
    is_prefix_cache_stable_mode().then_some(TURN_END_RESULT_CAP_TOKENS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save(keys: &[&'static str]) -> Vec<(&'static str, Option<std::ffi::OsString>)> {
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
    const KEYS: &[&str] = &[
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_RUNTIME_PROVIDER",
        "JCODE_NAMED_PROVIDER_PROFILE",
    ];

    fn clear_keys() {
        for k in KEYS {
            unsafe {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    fn is_inactive_when_no_env_signal() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(KEYS);
        clear_keys();
        assert!(!is_prefix_cache_stable_mode());
        restore(saved);
    }

    #[test]
    fn detects_via_openrouter_namespace() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(KEYS);
        clear_keys();
        crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", "deepseek");
        assert!(is_prefix_cache_stable_mode());
        restore(saved);
    }

    #[test]
    fn detects_via_runtime_provider() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(KEYS);
        clear_keys();
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", "DeepSeek");
        // Case-insensitive.
        assert!(is_prefix_cache_stable_mode());
        restore(saved);
    }

    #[test]
    fn detects_via_named_profile() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(KEYS);
        clear_keys();
        crate::env::set_var("JCODE_NAMED_PROVIDER_PROFILE", "  deepseek  ");
        // Whitespace-trimmed.
        assert!(is_prefix_cache_stable_mode());
        restore(saved);
    }

    #[test]
    fn ignores_non_deepseek_values() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(KEYS);
        clear_keys();
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", "anthropic");
        assert!(!is_prefix_cache_stable_mode());
        restore(saved);
    }

    #[test]
    fn context_tokens_recognizes_deepseek_v4() {
        assert_eq!(
            context_tokens_for_model("deepseek-v4-flash"),
            DEEPSEEK_V4_CONTEXT_TOKENS
        );
        assert_eq!(
            context_tokens_for_model("deepseek-1m-instruct"),
            DEEPSEEK_V4_CONTEXT_TOKENS
        );
    }

    #[test]
    fn context_tokens_falls_back_to_default() {
        assert_eq!(
            context_tokens_for_model("claude-sonnet-4"),
            DEFAULT_CONTEXT_TOKENS
        );
        assert_eq!(
            context_tokens_for_model("deepseek-coder"),
            DEFAULT_CONTEXT_TOKENS,
            "deepseek without v4/1m suffix uses default"
        );
    }

    #[test]
    fn preflight_emergency_above_threshold() {
        let decision = preflight_check_simple(96_000, 100_000);
        assert!(decision.needs_action);
        assert!((decision.ratio - 0.96).abs() < 1e-9);
    }

    #[test]
    fn preflight_no_action_at_safe_ratio() {
        let decision = preflight_check_simple(50_000, 100_000);
        assert!(!decision.needs_action);
    }

    #[test]
    fn preflight_handles_zero_ctx_max() {
        let decision = preflight_check_simple(1000, 0);
        assert!(!decision.needs_action);
        assert_eq!(decision.ratio, 0.0);
    }

    #[test]
    fn recommended_cap_none_when_inactive() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(KEYS);
        clear_keys();
        assert_eq!(recommended_tool_result_cap_tokens(), None);
        restore(saved);
    }

    #[test]
    fn recommended_cap_some_when_active() {
        let _lock = crate::storage::lock_test_env();
        let saved = save(KEYS);
        clear_keys();
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", "deepseek");
        assert_eq!(
            recommended_tool_result_cap_tokens(),
            Some(TURN_END_RESULT_CAP_TOKENS)
        );
        restore(saved);
    }
}
