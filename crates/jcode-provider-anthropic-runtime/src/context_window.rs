//! Context-window resolution for the Anthropic runtime.
//!
//! The default `Provider::context_window` uses the cache-free lookup, so a live
//! catalog limit for a Claude id that the static classifier does not know yet
//! never reached the TUI meter or the compaction budget and silently fell back
//! to 200K. Resolving through the cache-aware lookup fixes that (see #578).

/// Resolve the context window for `model` on the Anthropic route.
pub(crate) fn resolve(model: &str) -> usize {
    jcode_base::provider::context_limit_for_model_with_provider(model, Some("claude"))
        .unwrap_or(jcode_provider_core::DEFAULT_CONTEXT_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_native_1m_model_resolves_to_one_million() {
        assert_eq!(resolve("claude-opus-5"), 1_000_000);
    }

    #[test]
    fn standard_model_keeps_two_hundred_k() {
        assert_eq!(resolve("claude-sonnet-4-5"), 200_000);
    }
}
