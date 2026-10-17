//! Daemon-side KV (prompt) cache miss detection.
//!
//! The TUI historically detected cache misses on its own by diffing request
//! signatures against a baseline. Other clients (Jcode Desktop, SDK users)
//! never saw those alarms. This monitor runs inside the agent so every client
//! receives the same classified `KvCacheMiss` event.
//!
//! Flow per provider request:
//! 1. [`KvCacheMonitor::begin_request`] with the prompt-shape signature just
//!    before the request is sent.
//! 2. [`KvCacheMonitor::finish_request`] with the provider-reported usage once
//!    the stream completes. Returns a [`KvCacheMiss`] when a meaningful part of
//!    the previously cached prefix was not read back.

use std::time::{Duration, Instant};

/// Misses smaller than this are noise (provider cache block granularity).
pub const MIN_MISSED_TOKENS: u64 = 1_024;

/// Prompt-shape signature for one provider request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestSignature {
    pub system_static_hash: u64,
    pub tools_hash: u64,
    pub tool_count: usize,
    pub messages_hash: u64,
    pub message_hashes: Vec<u64>,
    pub message_count: usize,
}

impl RequestSignature {
    /// Whether `self` extends `previous` without modifying earlier messages.
    pub fn extends(&self, previous: &RequestSignature) -> bool {
        if previous.message_count > self.message_count {
            return false;
        }
        if !previous.message_hashes.is_empty() && !self.message_hashes.is_empty() {
            return self.message_hashes.len() >= previous.message_hashes.len()
                && self.message_hashes[..previous.message_hashes.len()] == previous.message_hashes;
        }
        previous.message_count == self.message_count && previous.messages_hash == self.messages_hash
    }
}

/// Provider identity and cache retention for one request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestRoute {
    pub provider: String,
    pub model: String,
    pub upstream_provider: Option<String>,
    /// Expected cache retention in seconds, when known.
    pub cache_ttl_secs: Option<u64>,
    /// Whether `cache_ttl_secs` is only an estimate (never treat as hard expiry).
    pub ttl_is_estimate: bool,
}

/// Why a request missed the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissReason {
    ProviderSwitch,
    ModelSwitch,
    UpstreamSwitch,
    Expired,
    SystemChanged,
    ToolsChanged,
    PrefixChanged,
    Compaction,
    ZeroRead,
    LowRead,
}

impl MissReason {
    /// Stable snake_case identifier used on the wire.
    pub fn id(self) -> &'static str {
        match self {
            Self::ProviderSwitch => "provider_switch",
            Self::ModelSwitch => "model_switch",
            Self::UpstreamSwitch => "upstream_switch",
            Self::Expired => "expired",
            Self::SystemChanged => "system_changed",
            Self::ToolsChanged => "tools_changed",
            Self::PrefixChanged => "prefix_changed",
            Self::Compaction => "compaction",
            Self::ZeroRead => "zero_read",
            Self::LowRead => "low_read",
        }
    }

    /// Human-readable explanation.
    pub fn detail(self) -> &'static str {
        match self {
            Self::ProviderSwitch => "provider changed",
            Self::ModelSwitch => "model changed",
            Self::UpstreamSwitch => "upstream provider changed",
            Self::Expired => "cache expired",
            Self::SystemChanged => "system prompt changed mid-session",
            Self::ToolsChanged => "tool set changed mid-session",
            Self::PrefixChanged => "an earlier message was modified",
            Self::Compaction => "provider automatically compacted the context",
            Self::ZeroRead => "provider reported no cache read",
            Self::LowRead => "provider read only part of the cache",
        }
    }

    /// Whether the harness itself changed the cached prefix. These should
    /// essentially never happen and indicate a bug unless documented.
    pub fn harness_caused(self) -> bool {
        matches!(
            self,
            Self::SystemChanged | Self::ToolsChanged | Self::PrefixChanged
        )
    }
}

/// A classified cache miss for one completed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvCacheMiss {
    pub reason: MissReason,
    /// Tokens expected to be served from cache (previous reusable boundary).
    pub expected_tokens: u64,
    /// Tokens the provider reported as read from cache.
    pub read_tokens: u64,
    /// Estimated tokens that had to be resent.
    pub missed_tokens: u64,
    /// Documented intentional invalidation that explains a harness-caused miss.
    pub documented_cause: Option<String>,
}

impl KvCacheMiss {
    /// One-line summary suitable for any client to display.
    pub fn message(&self) -> String {
        let tokens = compact_tokens(self.missed_tokens);
        match (&self.documented_cause, self.reason.harness_caused()) {
            (Some(cause), _) => format!(
                "KV cache refresh [{cause}]: ~{tokens} tokens resent ({}).",
                self.reason.detail()
            ),
            (None, true) => format!(
                "KV cache miss: ~{tokens} tokens resent ({}). This is likely a harness bug.",
                self.reason.detail()
            ),
            (None, false) => format!(
                "KV cache miss: ~{tokens} tokens resent ({}).",
                self.reason.detail()
            ),
        }
    }
}

fn compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{}K", value / 1_000)
    } else {
        value.to_string()
    }
}

#[derive(Debug, Clone)]
struct Baseline {
    route: RequestRoute,
    signature: RequestSignature,
    reusable_cache_tokens: u64,
    completed_at: Instant,
}

#[derive(Debug, Clone)]
struct Pending {
    route: RequestRoute,
    signature: RequestSignature,
    provider_compaction: bool,
}

/// Per-session miss detector. Reset whenever the provider-facing transcript is
/// intentionally replaced (compaction, rewind, clear).
#[derive(Debug, Clone, Default)]
pub struct KvCacheMonitor {
    baseline: Option<Baseline>,
    pending: Option<Pending>,
}

impl KvCacheMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.baseline = None;
        self.pending = None;
    }

    pub fn begin_request(&mut self, route: RequestRoute, signature: RequestSignature) {
        self.pending = Some(Pending {
            route,
            signature,
            provider_compaction: false,
        });
    }

    /// Mark compaction evidenced by the completed provider stream, not by a
    /// guessed token-count drop. This applies only to the current request.
    pub fn note_provider_compaction(&mut self) {
        if let Some(pending) = self.pending.as_mut() {
            pending.provider_compaction = true;
        }
    }

    /// Record completed usage. `reusable_cache_tokens` is the reusable cache
    /// boundary (read plus write, excluding uncached input for split accounting).
    /// `read_tokens` is `None` when the provider did not report cache reads,
    /// which is never treated as a miss.
    pub fn finish_request(
        &mut self,
        reusable_cache_tokens: u64,
        read_tokens: Option<u64>,
    ) -> Option<KvCacheMiss> {
        let pending = self.pending.take()?;
        let miss = self
            .baseline
            .as_ref()
            .and_then(|baseline| classify(baseline, &pending, read_tokens));
        if reusable_cache_tokens > 0 {
            self.baseline = Some(Baseline {
                route: pending.route,
                signature: pending.signature,
                reusable_cache_tokens,
                completed_at: Instant::now(),
            });
        }
        miss
    }
}

fn classify(
    baseline: &Baseline,
    pending: &Pending,
    read_tokens: Option<u64>,
) -> Option<KvCacheMiss> {
    let read_tokens = read_tokens?;
    let expected = baseline.reusable_cache_tokens;
    if expected == 0 {
        return None;
    }
    let missed = expected.saturating_sub(read_tokens);
    if missed < MIN_MISSED_TOKENS {
        return None;
    }
    let reason = reason(baseline, pending, read_tokens)?;
    let documented_cause = if reason == MissReason::Compaction {
        Some("provider_auto_compaction".to_string())
    } else {
        reason
            .harness_caused()
            .then(|| crate::cache_invalidation::most_recent_since(baseline.completed_at))
            .flatten()
            .map(|cause| cause.source.to_string())
    };
    Some(KvCacheMiss {
        reason,
        expected_tokens: expected,
        read_tokens,
        missed_tokens: missed,
        documented_cause,
    })
}

fn reason(baseline: &Baseline, pending: &Pending, read: u64) -> Option<MissReason> {
    let (old, new) = (&baseline.route, &pending.route);
    if old.provider != new.provider {
        return Some(MissReason::ProviderSwitch);
    }
    if old.model != new.model {
        return Some(MissReason::ModelSwitch);
    }
    if old.upstream_provider.is_some()
        && new.upstream_provider.is_some()
        && old.upstream_provider != new.upstream_provider
    {
        return Some(MissReason::UpstreamSwitch);
    }
    if let Some(ttl) = old.cache_ttl_secs
        && !old.ttl_is_estimate
        && baseline.completed_at.elapsed() >= Duration::from_secs(ttl)
    {
        return Some(MissReason::Expired);
    }
    let (prev, cur) = (&baseline.signature, &pending.signature);
    if prev.system_static_hash != cur.system_static_hash {
        return Some(MissReason::SystemChanged);
    }
    if prev.tools_hash != cur.tools_hash || prev.tool_count != cur.tool_count {
        return Some(MissReason::ToolsChanged);
    }
    // Route, expiry, system and tools changes remain independently actionable.
    // Native compaction explains a transcript-prefix or cache-read loss only.
    if pending.provider_compaction {
        return Some(MissReason::Compaction);
    }
    if !cur.extends(prev) {
        return Some(MissReason::PrefixChanged);
    }
    if read == 0 {
        return Some(MissReason::ZeroRead);
    }
    // The absolute tolerance was already checked against the reusable boundary.
    // A large warm prefix must not hide a real small cache loss.
    Some(MissReason::LowRead)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> RequestRoute {
        RequestRoute {
            provider: "claude".into(),
            model: "claude-opus".into(),
            upstream_provider: None,
            cache_ttl_secs: Some(300),
            ttl_is_estimate: false,
        }
    }

    fn sig(messages: &[u64]) -> RequestSignature {
        RequestSignature {
            system_static_hash: 1,
            tools_hash: 2,
            tool_count: 3,
            messages_hash: messages.iter().sum(),
            message_hashes: messages.to_vec(),
            message_count: messages.len(),
        }
    }

    fn warm(monitor: &mut KvCacheMonitor) {
        monitor.begin_request(route(), sig(&[10, 11]));
        assert_eq!(
            monitor.finish_request(50_000, Some(0)),
            None,
            "no baseline yet"
        );
    }

    #[test]
    fn healthy_append_is_not_a_miss() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        monitor.begin_request(route(), sig(&[10, 11, 12]));
        assert_eq!(monitor.finish_request(52_000, Some(49_800)), None);
    }

    #[test]
    fn small_real_loss_on_large_warm_prefix_is_not_hidden_by_percent() {
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request(route(), sig(&[10]));
        assert_eq!(monitor.finish_request(617_004, Some(615_629)), None);
        monitor.begin_request(route(), sig(&[10, 11]));
        let miss = monitor
            .finish_request(620_252, Some(615_222))
            .expect("real 1782-token loss");
        assert_eq!(miss.reason, MissReason::LowRead);
        assert_eq!(miss.missed_tokens, 1_782);
    }

    #[test]
    fn previous_cache_creation_is_reusable_but_uncached_tail_is_not() {
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request(route(), sig(&[10]));
        assert_eq!(monitor.finish_request(615_629 + 1_375, Some(615_629)), None);
        monitor.begin_request(route(), sig(&[10, 11]));
        assert_eq!(monitor.finish_request(617_004 + 3_248, Some(617_004)), None);
    }

    #[test]
    fn modified_prefix_is_a_harness_miss() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        monitor.begin_request(route(), sig(&[10, 99, 12]));
        let miss = monitor.finish_request(52_000, Some(4_000)).expect("miss");
        assert_eq!(miss.reason, MissReason::PrefixChanged);
        assert!(miss.reason.harness_caused());
        assert_eq!(miss.missed_tokens, 46_000);
        assert!(miss.message().contains("~46K tokens resent"));
    }

    #[test]
    fn tools_change_is_classified_even_when_read_looks_ok() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        let mut changed = sig(&[10, 11, 12]);
        changed.tools_hash = 7;
        monitor.begin_request(route(), changed);
        let miss = monitor.finish_request(52_000, Some(47_000)).expect("miss");
        assert_eq!(miss.reason, MissReason::ToolsChanged);
    }

    #[test]
    fn model_switch_is_not_harness_caused() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        let mut switched = route();
        switched.model = "claude-sonnet".into();
        monitor.begin_request(switched, sig(&[10, 11, 12]));
        let miss = monitor.finish_request(52_000, Some(0)).expect("miss");
        assert_eq!(miss.reason, MissReason::ModelSwitch);
        assert!(!miss.reason.harness_caused());
    }

    #[test]
    fn missing_cache_telemetry_never_reports() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        monitor.begin_request(route(), sig(&[99]));
        assert_eq!(monitor.finish_request(52_000, None), None);
    }

    #[test]
    fn native_compaction_zero_read_is_an_expected_refresh() {
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request(route(), sig(&[10, 11]));
        monitor.finish_request(216_517, Some(216_064));
        monitor.begin_request(route(), sig(&[10, 11, 12]));
        monitor.note_provider_compaction();
        let miss = monitor.finish_request(204_393, Some(0)).expect("refresh");
        assert_eq!(miss.reason, MissReason::Compaction);
        assert_eq!(miss.expected_tokens, 216_517);
        assert_eq!(miss.read_tokens, 0);
        assert_eq!(
            miss.documented_cause.as_deref(),
            Some("provider_auto_compaction")
        );
        assert!(!miss.reason.harness_caused());
        assert!(
            miss.message()
                .starts_with("KV cache refresh [provider_auto_compaction]")
        );
        assert!(!miss.message().contains("likely a harness bug"));

        // Evidence belongs to this response only. A subsequent unexplained
        // zero read must still be reported, even after a compacted response.
        monitor.begin_request(route(), sig(&[10, 11, 12, 13]));
        let miss = monitor.finish_request(205_000, Some(0)).expect("real miss");
        assert_eq!(miss.reason, MissReason::ZeroRead);
        assert_eq!(miss.documented_cause, None);
    }

    #[test]
    fn compaction_does_not_hide_independent_changes() {
        for expected in [
            MissReason::ModelSwitch,
            MissReason::SystemChanged,
            MissReason::ToolsChanged,
        ] {
            let mut monitor = KvCacheMonitor::new();
            warm(&mut monitor);
            let mut next_route = route();
            let mut next_sig = sig(&[99]);
            match expected {
                MissReason::ModelSwitch => next_route.model = "other".into(),
                MissReason::SystemChanged => next_sig.system_static_hash += 1,
                MissReason::ToolsChanged => next_sig.tools_hash += 1,
                _ => unreachable!(),
            }
            monitor.begin_request(next_route, next_sig);
            monitor.note_provider_compaction();
            let miss = monitor.finish_request(20_000, Some(0)).expect("miss");
            assert_eq!(miss.reason, expected);
        }
    }

    #[test]
    fn compaction_evidence_does_not_survive_retry_or_reset() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        monitor.begin_request(route(), sig(&[10, 11, 12]));
        monitor.note_provider_compaction();
        monitor.begin_request(route(), sig(&[10, 11, 12]));
        assert_eq!(
            monitor.finish_request(52_000, Some(0)).unwrap().reason,
            MissReason::ZeroRead
        );
        monitor.note_provider_compaction(); // No pending request: no effect.
        monitor.reset();
        warm(&mut monitor);
        monitor.begin_request(route(), sig(&[10, 11, 12]));
        assert_eq!(
            monitor.finish_request(52_000, Some(0)).unwrap().reason,
            MissReason::ZeroRead
        );
    }

    #[test]
    fn compaction_without_reported_loss_is_not_a_miss() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        monitor.begin_request(route(), sig(&[10, 11, 12]));
        monitor.note_provider_compaction();
        assert_eq!(monitor.finish_request(52_000, Some(50_000)), None);
        monitor.begin_request(route(), sig(&[99]));
        monitor.note_provider_compaction();
        assert_eq!(monitor.finish_request(20_000, None), None);
    }

    #[test]
    fn reset_drops_baseline() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        monitor.reset();
        monitor.begin_request(route(), sig(&[99]));
        assert_eq!(monitor.finish_request(52_000, Some(0)), None);
    }
}
