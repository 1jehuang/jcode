//! Evidence for cache-loss investigations. These are observations, not a claim
//! that we can inspect or distinguish eviction and routing inside a provider.
use super::*;

impl App {
    pub(super) fn kv_cache_diagnostic_fields(
        &self,
        request: &PendingKvCacheRequest,
        prompt_tokens: u64,
    ) -> String {
        let baseline = request.baseline.as_ref();
        let read = self.streaming.streaming_cache_read_tokens;
        let shortfall = baseline
            .zip(read)
            .map(|(b, r)| b.input_tokens.saturating_sub(r));
        let shortfall_pct = baseline
            .zip(shortfall)
            .map(|(b, missing)| ratio_pct(missing, b.input_tokens));
        // Completion-time age includes model generation and tool/transport delay.
        // Keep it in the existing field, but expose the actual observed idle gap.
        let idle_secs = baseline.zip(request.started_at).and_then(|(b, started)| {
            started
                .checked_duration_since(b.completed_at)
                .map(|age| age.as_secs())
        });
        let request_elapsed_ms = request.started_at.map(|start| start.elapsed().as_millis());
        let ttl_state = match (baseline.and_then(|b| b.cache_ttl_secs), idle_secs) {
            (Some(ttl), Some(idle)) if idle >= ttl => "elapsed_at_request_start",
            (Some(_), Some(_)) => "within_at_request_start",
            (Some(_), None) => "request_start_unknown",
            (None, _) => "unknown",
        };
        let route_matches = baseline.map(|b| {
            b.provider == request.provider
                && b.model == request.model
                && b.upstream_provider == request.upstream_provider
        });
        let upstream_matches = baseline.and_then(|b| {
            b.upstream_provider
                .as_ref()
                .zip(request.upstream_provider.as_ref())
                .map(|(previous, current)| previous == current)
        });
        let timing_source = match (request.started_at, self.is_remote) {
            (None, _) => "unavailable",
            (Some(_), true) => "client_request_event",
            (Some(_), false) => "local_request_start",
        };
        let signatures = baseline
            .and_then(|b| b.signature.as_ref())
            .zip(request.signature.as_ref());
        let static_prefix_matches = signatures.map(|(b, r)| {
            b.system_static_hash == r.system_static_hash
                && b.tools_hash == r.tools_hash
                && b.tool_count == r.tool_count
        });
        let generation_matches = baseline.map(|b| b.cache_generation == request.cache_generation);
        let loss_evidence = match (baseline, read, shortfall) {
            (None, _, _) => "no_baseline",
            (_, None, _) => "usage_not_reported",
            (_, _, Some(0)) => "no_shortfall",
            (_, _, Some(n)) if n < Self::kv_cache_min_missed_tokens(&request.provider) => {
                "below_miss_threshold"
            }
            _ if generation_matches == Some(false) => "cache_generation_changed",
            _ if route_matches == Some(false) => "route_metadata_changed",
            _ if static_prefix_matches == Some(false)
                || request.baseline_messages_prefix_matches == Some(false) =>
            {
                "local_prefix_changed"
            }
            _ if static_prefix_matches == Some(true)
                && request.baseline_messages_prefix_matches == Some(true) =>
            {
                "unexplained_prefix_shortfall"
            }
            _ => "insufficient_prefix_evidence",
        };
        format!(
            "session_id={:?} baseline_provider={:?} baseline_model={:?} baseline_upstream={:?} \
             cache_generation={} baseline_cache_generation={:?} prompt_tokens={} \
             cache_read_reported={} cache_write_reported={} prefix_shortfall_tokens={:?} prefix_shortfall_pct={:?} \
             idle_before_request_secs={:?} request_elapsed_ms={:?} request_timing_source={} ttl_state={} requested_ttl_secs={:?} \
             route_metadata_matches={:?} upstream_matches={:?} static_prefix_matches={:?} loss_evidence={} eviction_confirmed=false",
            self.kv_cache_session_id(),
            baseline.map(|b| b.provider.as_str()),
            baseline.map(|b| b.model.as_str()),
            baseline.and_then(|b| b.upstream_provider.as_deref()),
            request.cache_generation,
            baseline.map(|b| b.cache_generation),
            prompt_tokens,
            read.is_some(),
            self.streaming.streaming_cache_creation_tokens.is_some(),
            shortfall,
            shortfall_pct,
            idle_secs,
            request_elapsed_ms,
            timing_source,
            ttl_state,
            request.cache_ttl_secs,
            route_matches,
            upstream_matches,
            static_prefix_matches,
            loss_evidence,
        )
    }
}
