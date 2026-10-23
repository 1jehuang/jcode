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

/// OpenAI reports cache reads in 128-token blocks and routinely leaves the
/// last ~1024 tokens of the previous request uncached, so a byte-identical
/// append still shows a 1024-1300 token shortfall. Across ~34k verified
/// prefix-matching websocket continuations, shortfalls cluster just above
/// 1024; treating them as misses produced constant false alarms.
pub const OPENAI_MIN_MISSED_TOKENS: u64 = 2_048;

/// Noise floor for a provider identity (`openai`, `openai-api`, `OpenAI`, ...).
pub fn min_missed_tokens_for_provider(provider: &str) -> u64 {
    if provider.trim().to_ascii_lowercase().starts_with("openai") {
        OPENAI_MIN_MISSED_TOKENS
    } else {
        MIN_MISSED_TOKENS
    }
}

/// Whether a provider drops its conversation prompt cache when the UTC date
/// changes. Observed on Anthropic: every session whose consecutive requests
/// straddle 00:00 UTC reads back only the tools/system prefix, even seconds
/// apart with a byte-identical prefix. (15 of 15 such straddles missed across
/// four days of logs, versus ~2% of same-day requests.) The harness cannot
/// avoid it, so such misses are attributed instead of reported as unexplained.
pub fn provider_rolls_cache_daily(provider: &str) -> bool {
    matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "anthropic" | "claude"
    )
}

/// UTC calendar day number, used to detect the provider's daily cache rollover.
pub fn utc_day_now() -> i64 {
    utc_day_of(std::time::SystemTime::now())
}

pub fn utc_day_of(time: std::time::SystemTime) -> i64 {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => (elapsed.as_secs() / 86_400) as i64,
        Err(before) => -((before.duration().as_secs() / 86_400) as i64) - 1,
    }
}

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
    /// OAuth account label bound to the request, when the route is account-scoped.
    /// Provider prompt caches are per account, so a failover to another
    /// account (for example after a 429) always starts cold.
    pub account: Option<String>,
    /// Expected cache retention in seconds, when known.
    pub cache_ttl_secs: Option<u64>,
    /// Whether `cache_ttl_secs` is only an estimate (never treat as hard expiry).
    pub ttl_is_estimate: bool,
}

/// Why a request missed the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissReason {
    ProviderSwitch,
    AccountSwitch,
    ModelSwitch,
    UpstreamSwitch,
    Expired,
    DailyRollover,
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
            Self::AccountSwitch => "account_switch",
            Self::ModelSwitch => "model_switch",
            Self::UpstreamSwitch => "upstream_switch",
            Self::Expired => "expired",
            Self::DailyRollover => "daily_rollover",
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
            Self::AccountSwitch => "account changed; prompt caches are per account",
            Self::ModelSwitch => "model changed",
            Self::UpstreamSwitch => "upstream provider changed",
            Self::Expired => "cache expired",
            Self::DailyRollover => {
                "provider resets its prompt cache at 00:00 UTC; not avoidable by the harness"
            }
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
    /// Estimated shortfall against the previous reusable prefix. A compaction
    /// changes that prefix, so this is not measured resent or avoidable input.
    pub missed_tokens: u64,
    /// Documented intentional invalidation that explains a harness-caused miss.
    pub documented_cause: Option<String>,
    /// Specific explanation (what changed, from what to what, timing).
    pub detail: String,
}

impl KvCacheMiss {
    /// One-line summary suitable for any client to display.
    pub fn message(&self) -> String {
        if self.reason == MissReason::Compaction {
            return format!(
                "KV cache refresh [provider_auto_compaction]: provider compacted the context; rebuilding cache is expected (previous reusable prefix ~{} tokens, cache read {}).",
                format_token_count(self.expected_tokens),
                format_token_count(self.read_tokens)
            );
        }
        let tokens = format_token_count(self.missed_tokens);
        let detail = if self.detail.is_empty() {
            self.reason.detail()
        } else {
            self.detail.as_str()
        };
        match (&self.documented_cause, self.reason.harness_caused()) {
            (Some(cause), _) => {
                format!("KV cache refresh [{cause}]: ~{tokens} tokens resent ({detail}).")
            }
            (None, true) => format!(
                "KV cache miss [{}]: ~{tokens} tokens resent ({detail}). This is likely a harness bug.",
                self.reason.id()
            ),
            (None, false) => format!(
                "KV cache miss [{}]: ~{tokens} tokens resent ({detail}).",
                self.reason.id()
            ),
        }
    }
}

/// Human-friendly token count: `987`, `4.12K`, `33.1K`, `336K`, `1.25M`.
pub fn format_token_count(value: u64) -> String {
    fn trim(text: String) -> String {
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            text
        }
    }
    let v = value as f64;
    if value >= 1_000_000 {
        format!("{}M", trim(format!("{:.2}", v / 1_000_000.0)))
    } else if value >= 100_000 {
        format!("{}K", (v / 1_000.0).round() as u64)
    } else if value >= 10_000 {
        format!("{}K", trim(format!("{:.1}", v / 1_000.0)))
    } else if value >= 1_000 {
        format!("{}K", trim(format!("{:.2}", v / 1_000.0)))
    } else {
        value.to_string()
    }
}

/// Compact duration for diagnostics: `45s`, `6m 2s`, `1h 5m`.
pub fn format_duration_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        let (m, s) = (secs / 60, secs % 60);
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    } else {
        let (h, m) = (secs / 3_600, (secs % 3_600) / 60);
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    }
}

fn or_unknown(value: Option<&str>) -> &str {
    value.filter(|v| !v.is_empty()).unwrap_or("default")
}

/// Inputs for a specific, human-readable explanation of one cache miss.
#[derive(Debug, Clone, Copy)]
pub struct MissContext<'a> {
    pub old_provider: &'a str,
    pub new_provider: &'a str,
    pub old_model: &'a str,
    pub new_model: &'a str,
    pub old_account: Option<&'a str>,
    pub new_account: Option<&'a str>,
    pub old_upstream: Option<&'a str>,
    pub new_upstream: Option<&'a str>,
    /// Seconds between the previous request completing and this one starting.
    pub idle_secs: Option<u64>,
    pub ttl_secs: Option<u64>,
    pub ttl_is_estimate: bool,
    pub expected_tokens: u64,
    pub read_tokens: u64,
    /// Index of the first previously sent message that changed, when known.
    pub first_changed_message: Option<usize>,
}

impl MissContext<'_> {
    fn timing(&self) -> String {
        let idle = self
            .idle_secs
            .map(|s| format!("{} after the previous call", format_duration_secs(s)));
        let ttl = self.ttl_secs.map(|t| {
            format!(
                "{}TTL {}",
                if self.ttl_is_estimate {
                    "estimated "
                } else {
                    ""
                },
                format_duration_secs(t)
            )
        });
        match (idle, ttl) {
            (Some(i), Some(t)) => format!("{i}, within {t}"),
            (Some(i), None) => i,
            (None, Some(t)) => t,
            (None, None) => String::new(),
        }
    }
}

/// Specific explanation for a classified miss: what changed, from what to
/// what, and the timing that rules other causes in or out.
pub fn describe_miss(reason: MissReason, ctx: &MissContext<'_>) -> String {
    let prefix = format_token_count(ctx.expected_tokens);
    let read = format_token_count(ctx.read_tokens);
    match reason {
        MissReason::ProviderSwitch => format!(
            "provider switched {} -> {}; caches are not shared across providers",
            ctx.old_provider, ctx.new_provider
        ),
        MissReason::AccountSwitch => format!(
            "account switched {} -> {} (e.g. failover after a rate limit); prompt caches are per account",
            or_unknown(ctx.old_account),
            or_unknown(ctx.new_account)
        ),
        MissReason::ModelSwitch => format!(
            "model switched {} -> {}; caches are per model",
            ctx.old_model, ctx.new_model
        ),
        MissReason::UpstreamSwitch => format!(
            "upstream route switched {} -> {}; cache lives on the previous upstream",
            or_unknown(ctx.old_upstream),
            or_unknown(ctx.new_upstream)
        ),
        MissReason::Expired => match (ctx.idle_secs, ctx.ttl_secs) {
            (Some(idle), Some(ttl)) => format!(
                "idle {} exceeded the {} cache TTL",
                format_duration_secs(idle),
                format_duration_secs(ttl)
            ),
            _ => "cache retention elapsed".to_string(),
        },
        MissReason::DailyRollover => {
            "provider resets its prompt cache at 00:00 UTC; not avoidable by the harness"
                .to_string()
        }
        MissReason::SystemChanged => "system prompt changed mid-session".to_string(),
        MissReason::ToolsChanged => "tool set changed mid-session".to_string(),
        MissReason::PrefixChanged => match ctx.first_changed_message {
            Some(index) => {
                format!("earlier message #{index} was modified, invalidating everything after it")
            }
            None => "an earlier message was modified".to_string(),
        },
        MissReason::Compaction => "provider automatically compacted the context".to_string(),
        MissReason::ZeroRead | MissReason::LowRead => {
            let timing = ctx.timing();
            let timing = if timing.is_empty() {
                String::new()
            } else {
                format!(" {timing}")
            };
            let same = if ctx.old_account.is_some() && ctx.new_account.is_some() {
                "provider, model, account"
            } else {
                "provider, model"
            };
            format!(
                "provider read {read} of the {prefix}-token prefix{timing}; same {same}, system prompt, tools and message prefix, so the provider evicted or rerouted the cache"
            )
        }
    }
}

#[derive(Debug, Clone)]
struct Baseline {
    route: RequestRoute,
    signature: RequestSignature,
    reusable_cache_tokens: u64,
    completed_at: Instant,
    /// UTC day on which the baseline request was sent (its cache was written).
    utc_day: i64,
}

#[derive(Debug, Clone)]
struct Pending {
    route: RequestRoute,
    signature: RequestSignature,
    provider_compaction: bool,
    utc_day: i64,
    started_at: Instant,
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
        self.begin_request_on_day(route, signature, utc_day_now());
    }

    /// [`Self::begin_request`] with an explicit UTC day (for tests).
    pub fn begin_request_on_day(
        &mut self,
        route: RequestRoute,
        signature: RequestSignature,
        utc_day: i64,
    ) {
        self.pending = Some(Pending {
            route,
            signature,
            provider_compaction: false,
            utc_day,
            started_at: Instant::now(),
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
                utc_day: pending.utc_day,
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
    if missed < min_missed_tokens_for_provider(&pending.route.provider) {
        return None;
    }
    let reason = reason(baseline, pending, read_tokens)?;
    let documented_cause = if reason == MissReason::Compaction {
        Some("provider_auto_compaction".to_string())
    } else if reason == MissReason::DailyRollover {
        Some("provider_daily_rollover".to_string())
    } else {
        reason
            .harness_caused()
            .then(|| crate::cache_invalidation::most_recent_since(baseline.completed_at))
            .flatten()
            .map(|cause| cause.source.to_string())
    };
    let (old, new) = (&baseline.route, &pending.route);
    let detail = describe_miss(
        reason,
        &MissContext {
            old_provider: &old.provider,
            new_provider: &new.provider,
            old_model: &old.model,
            new_model: &new.model,
            old_account: old.account.as_deref(),
            new_account: new.account.as_deref(),
            old_upstream: old.upstream_provider.as_deref(),
            new_upstream: new.upstream_provider.as_deref(),
            idle_secs: Some(
                pending
                    .started_at
                    .saturating_duration_since(baseline.completed_at)
                    .as_secs(),
            ),
            ttl_secs: old.cache_ttl_secs,
            ttl_is_estimate: old.ttl_is_estimate,
            expected_tokens: expected,
            read_tokens,
            first_changed_message: pending
                .signature
                .message_hashes
                .iter()
                .zip(baseline.signature.message_hashes.iter())
                .position(|(a, b)| a != b),
        },
    );
    Some(KvCacheMiss {
        reason,
        expected_tokens: expected,
        read_tokens,
        missed_tokens: missed,
        documented_cause,
        detail,
    })
}

fn reason(baseline: &Baseline, pending: &Pending, read: u64) -> Option<MissReason> {
    let (old, new) = (&baseline.route, &pending.route);
    if old.provider != new.provider {
        return Some(MissReason::ProviderSwitch);
    }
    if old.account.is_some() && new.account.is_some() && old.account != new.account {
        return Some(MissReason::AccountSwitch);
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
    // Harness-caused changes above stay visible as bugs. Only an otherwise
    // clean append that straddles 00:00 UTC is attributed to the provider.
    if baseline.utc_day != pending.utc_day && provider_rolls_cache_daily(&new.provider) {
        return Some(MissReason::DailyRollover);
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
            account: None,
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
    fn account_failover_is_an_account_switch_with_from_to_detail() {
        let mut monitor = KvCacheMonitor::new();
        let mut fox = route();
        fox.account = Some("fox".into());
        monitor.begin_request(fox, sig(&[10, 11]));
        monitor.finish_request(33_096, Some(33_000));
        let mut otter = route();
        otter.account = Some("otter".into());
        monitor.begin_request(otter, sig(&[10, 11, 12]));
        let miss = monitor.finish_request(40_204, Some(0)).expect("miss");
        assert_eq!(miss.reason, MissReason::AccountSwitch);
        assert!(!miss.reason.harness_caused());
        let message = miss.message();
        assert!(message.contains("[account_switch]"), "{message}");
        assert!(message.contains("fox -> otter"), "{message}");
        assert!(message.contains("~33.1K tokens"), "{message}");
    }

    #[test]
    fn token_counts_are_human_readable() {
        assert_eq!(format_token_count(987), "987");
        assert_eq!(format_token_count(1_000), "1K");
        assert_eq!(format_token_count(4_123), "4.12K");
        assert_eq!(format_token_count(33_096), "33.1K");
        assert_eq!(format_token_count(40_000), "40K");
        assert_eq!(format_token_count(336_400), "336K");
        assert_eq!(format_token_count(1_250_000), "1.25M");
        assert_eq!(format_token_count(2_000_000), "2M");
    }

    #[test]
    fn zero_read_detail_rules_out_harness_and_reports_timing() {
        let mut monitor = KvCacheMonitor::new();
        warm(&mut monitor);
        monitor.begin_request(route(), sig(&[10, 11, 12]));
        let miss = monitor.finish_request(52_000, Some(0)).expect("miss");
        assert_eq!(miss.reason, MissReason::ZeroRead);
        assert!(
            miss.detail.contains("read 0 of the 50K-token prefix"),
            "{}",
            miss.detail
        );
        assert!(miss.detail.contains("TTL 5m"), "{}", miss.detail);
        assert!(miss.detail.contains("evicted"), "{}", miss.detail);
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

    fn openai_route() -> RequestRoute {
        RequestRoute {
            provider: "openai-api".into(),
            model: "gpt-5.4".into(),
            upstream_provider: None,
            account: None,
            cache_ttl_secs: Some(1800),
            ttl_is_estimate: true,
        }
    }

    #[test]
    fn openai_tail_block_rounding_is_not_a_miss() {
        // Observed: byte-identical append, 1024-token-block cache read shortfall.
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request(openai_route(), sig(&[10]));
        assert_eq!(monitor.finish_request(74_368, Some(73_344)), None);
        monitor.begin_request(openai_route(), sig(&[10, 11]));
        assert_eq!(monitor.finish_request(75_267, Some(73_344)), None);
        monitor.begin_request(openai_route(), sig(&[10, 11, 12]));
        assert_eq!(monitor.finish_request(76_900, Some(73_344)), None);
    }

    #[test]
    fn openai_real_loss_is_still_reported() {
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request(openai_route(), sig(&[10]));
        monitor.finish_request(127_199, Some(127_000));
        monitor.begin_request(openai_route(), sig(&[10, 11]));
        let miss = monitor
            .finish_request(128_870, Some(110_208))
            .expect("16991-token loss");
        assert_eq!(miss.reason, MissReason::LowRead);
    }

    #[test]
    fn noise_floor_is_provider_specific() {
        assert_eq!(
            min_missed_tokens_for_provider("openai"),
            OPENAI_MIN_MISSED_TOKENS
        );
        assert_eq!(
            min_missed_tokens_for_provider("OpenAI"),
            OPENAI_MIN_MISSED_TOKENS
        );
        assert_eq!(
            min_missed_tokens_for_provider("openai-oauth"),
            OPENAI_MIN_MISSED_TOKENS
        );
        assert_eq!(min_missed_tokens_for_provider("claude"), MIN_MISSED_TOKENS);
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
        assert!(miss.message().contains("rebuilding cache is expected"));
        assert!(
            miss.message()
                .contains("previous reusable prefix ~217K tokens")
        );
        assert!(!miss.message().contains("tokens resent"));

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

    #[test]
    fn utc_midnight_straddle_is_a_provider_daily_rollover() {
        // Real incident: 621K-token prefix, 31K read after crossing 00:00 UTC.
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request_on_day(route(), sig(&[10, 11]), 20_744);
        monitor.finish_request(621_788, Some(620_865));
        monitor.begin_request_on_day(route(), sig(&[10, 11, 12]), 20_745);
        let miss = monitor.finish_request(622_773, Some(31_075)).expect("miss");
        assert_eq!(miss.reason, MissReason::DailyRollover);
        assert!(!miss.reason.harness_caused());
        assert_eq!(
            miss.documented_cause.as_deref(),
            Some("provider_daily_rollover")
        );
        assert!(
            miss.message()
                .starts_with("KV cache refresh [provider_daily_rollover]")
        );
    }

    #[test]
    fn harness_changes_still_win_over_daily_rollover() {
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request_on_day(route(), sig(&[10, 11]), 1);
        monitor.finish_request(50_000, Some(50_000));
        monitor.begin_request_on_day(route(), sig(&[10, 99, 12]), 2);
        let miss = monitor.finish_request(52_000, Some(2_000)).expect("miss");
        assert_eq!(miss.reason, MissReason::PrefixChanged);
    }

    #[test]
    fn same_utc_day_low_read_is_not_attributed_to_rollover() {
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request_on_day(route(), sig(&[10, 11]), 5);
        monitor.finish_request(50_000, Some(50_000));
        monitor.begin_request_on_day(route(), sig(&[10, 11, 12]), 5);
        let miss = monitor.finish_request(52_000, Some(2_000)).expect("miss");
        assert_eq!(miss.reason, MissReason::LowRead);
    }

    #[test]
    fn non_anthropic_day_change_is_not_attributed_to_rollover() {
        let mut openai = route();
        openai.provider = "openrouter".into();
        let mut monitor = KvCacheMonitor::new();
        monitor.begin_request_on_day(openai.clone(), sig(&[10, 11]), 5);
        monitor.finish_request(50_000, Some(50_000));
        monitor.begin_request_on_day(openai, sig(&[10, 11, 12]), 6);
        let miss = monitor.finish_request(52_000, Some(2_000)).expect("miss");
        assert_eq!(miss.reason, MissReason::LowRead);
    }

    #[test]
    fn utc_day_boundaries() {
        use std::time::{Duration, UNIX_EPOCH};
        assert_eq!(utc_day_of(UNIX_EPOCH + Duration::from_secs(86_399)), 0);
        assert_eq!(utc_day_of(UNIX_EPOCH + Duration::from_secs(86_400)), 1);
        assert_eq!(utc_day_of(UNIX_EPOCH - Duration::from_secs(1)), -1);
    }
}
