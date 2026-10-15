//! Detection of reliable KV / prompt-cache problem signals for the TUI status bar.

use super::CacheTtlInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KvCacheProblemKind {
    /// The provider explicitly reported new cache creation on a turn where we expected
    /// an already-warm cache to be read instead.
    UnexpectedCacheCreation,
    /// The provider explicitly reported zero cached input tokens on a turn where this
    /// provider family should report cached tokens for a warm, cacheable conversation.
    ExpectedCacheReadMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KvCacheProblem {
    pub kind: KvCacheProblemKind,
    pub affected_tokens: Option<u64>,
}

impl KvCacheProblem {
    pub(crate) fn log_reason(self) -> &'static str {
        match self.kind {
            KvCacheProblemKind::UnexpectedCacheCreation => "unexpected_cache_creation",
            KvCacheProblemKind::ExpectedCacheReadMissing => "expected_cache_read_missing",
        }
    }
}

fn normalized_provider_matches(provider: &str, needle: &str) -> bool {
    provider.trim().to_ascii_lowercase().contains(needle)
}

fn provider_stack_contains(provider: &str, upstream_provider: Option<&str>, needle: &str) -> bool {
    let needle = &needle.to_ascii_lowercase();
    normalized_provider_matches(provider, needle)
        || upstream_provider
            .map(|upstream| normalized_provider_matches(upstream, needle))
            .unwrap_or(false)
}

fn provider_stack_contains_any(
    provider: &str,
    upstream_provider: Option<&str>,
    needles: &[&str],
) -> bool {
    needles
        .iter()
        .any(|needle| provider_stack_contains(provider, upstream_provider, needle))
}

fn supports_reliable_zero_cache_read_warning(
    provider: &str,
    upstream_provider: Option<&str>,
) -> bool {
    if provider_stack_contains_any(
        provider,
        upstream_provider,
        &["openai", "anthropic", "claude", "gemini", "google"],
    ) {
        return true;
    }

    // OpenRouter/Jcode-subscription routes can only be treated as reliable for zero-read
    // warnings once the upstream provider identifies a known cache-reporting family.
    // A bare OpenRouter route with cached_tokens=0 is not enough: some upstreams simply
    // do not implement prompt caching, and warning on those would make the UI untrustworthy.
    false
}

fn min_cacheable_input_tokens(provider: &str, upstream_provider: Option<&str>) -> u64 {
    if provider_stack_contains_any(provider, upstream_provider, &["gemini", "google"]) {
        // Be conservative for Gemini-style implicit caching. Several Gemini models have
        // higher minimums than OpenAI/Anthropic; a higher UI threshold avoids warning on
        // prompts that might legitimately be below the provider's cacheable size.
        4_096
    } else {
        1_024
    }
}

fn cache_expected_warm(cache_ttl: Option<&CacheTtlInfo>) -> bool {
    cache_ttl.map(|info| !info.is_cold).unwrap_or(false)
}

/// Detect a KV/prompt-cache problem that is reliable enough to surface in the UI.
///
/// This intentionally does **not** warn merely because a cache-hit metric is absent. A warning
/// requires all of the following:
/// - a multi-turn conversation where cache reuse should be possible;
/// - a prior completed turn still within the provider's expected cache TTL;
/// - explicit provider telemetry showing either a cache rewrite without a read, or an explicit
///   zero cache-read count from a known cache-reporting provider family;
/// - enough input tokens to be cacheable for read-only providers.
pub(crate) fn detect_kv_cache_problem(
    provider: &str,
    upstream_provider: Option<&str>,
    user_turn_count: usize,
    input_tokens: u64,
    cache_read: Option<u64>,
    cache_creation: Option<u64>,
    cache_ttl: Option<&CacheTtlInfo>,
) -> Option<KvCacheProblem> {
    if user_turn_count <= 2 || !cache_expected_warm(cache_ttl) {
        return None;
    }

    let cache_read_tokens = cache_read.unwrap_or(0);
    let cache_creation_tokens = cache_creation.unwrap_or(0);

    // Strongest signal: the provider explicitly says it created cache but read none.
    if cache_creation_tokens > 0 && cache_read_tokens == 0 {
        return Some(KvCacheProblem {
            kind: KvCacheProblemKind::UnexpectedCacheCreation,
            affected_tokens: Some(cache_creation_tokens),
        });
    }

    // Read-only telemetry providers (OpenAI/Gemini and known OpenRouter upstreams) do not expose
    // cache creation tokens. For those, an explicit zero read on a warm, cacheable conversation is
    // the reliable signal. Absence of the metric is ignored.
    if cache_read != Some(0) {
        return None;
    }

    if !supports_reliable_zero_cache_read_warning(provider, upstream_provider) {
        return None;
    }

    if input_tokens < min_cacheable_input_tokens(provider, upstream_provider) {
        return None;
    }

    Some(KvCacheProblem {
        kind: KvCacheProblemKind::ExpectedCacheReadMissing,
        affected_tokens: Some(input_tokens),
    })
}
