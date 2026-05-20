//! OpenTelemetry Metrics for Completion Engine
//!
//! This module provides observability for the completion system,
//! tracking performance, cache effectiveness, and user behavior.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Completion engine metrics collector
#[derive(Debug, Default)]
pub struct CompletionMetrics {
    // Counter metrics
    /// Total completion requests
    pub total_requests: AtomicU64,
    /// Cache hits (prefetch)
    pub cache_hits: AtomicU64,
    /// Cache misses
    pub cache_misses: AtomicU64,
    /// Completions accepted by user
    pub completions_accepted: AtomicU64,
    /// Completions rejected/ignored
    pub completions_rejected: AtomicU64,
    /// Prefetch requests triggered
    pub prefetch_requests: AtomicU64,
    /// Errors during completion
    pub errors: AtomicU64,

    // Histogram-like metrics (using running averages)
    /// Total latency accumulator (ms)
    latency_sum_ms: AtomicU64,
    /// Latency sample count
    latency_count: AtomicU64,
    /// P95 latency accumulator (for approximation)
    p95_latency_sum_ms: AtomicU64,
    /// P95 sample count
    p95_latency_count: AtomicU64,

    // Gauge metrics
    /// Current cache size
    pub cache_size: AtomicU64,
    /// Active learning patterns count
    pub learned_patterns: AtomicU64,
}

impl CompletionMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a completion request
    pub fn record_request(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache hit
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Record completion acceptance
    pub fn record_acceptance(&self) {
        self.completions_accepted.fetch_add(1, Ordering::Relaxed);
    }

    /// Record completion rejection
    pub fn record_rejection(&self) {
        self.completions_rejected.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a prefetch request
    pub fn record_prefetch(&self) {
        self.prefetch_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an error
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Record completion latency
    pub fn record_latency(&self, latency_ms: u64) {
        self.latency_sum_ms.fetch_add(latency_ms, Ordering::Relaxed);
        self.latency_count.fetch_add(1, Ordering::Relaxed);

        // Track high latencies for P95 approximation
        if latency_ms > 100 {
            self.p95_latency_sum_ms.fetch_add(latency_ms, Ordering::Relaxed);
            self.p95_latency_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Update cache size gauge
    pub fn update_cache_size(&self, size: usize) {
        self.cache_size.store(size as u64, Ordering::Relaxed);
    }

    /// Update learned patterns count
    pub fn update_learned_patterns(&self, count: usize) {
        self.learned_patterns.store(count as u64, Ordering::Relaxed);
    }

    /// Get average latency (ms)
    pub fn get_avg_latency_ms(&self) -> f64 {
        let sum = self.latency_sum_ms.load(Ordering::Relaxed);
        let count = self.latency_count.load(Ordering::Relaxed);
        if count == 0 {
            0.0
        } else {
            sum as f64 / count as f64
        }
    }

    /// Get approximate P95 latency (ms)
    pub fn get_p95_latency_ms(&self) -> f64 {
        let sum = self.p95_latency_sum_ms.load(Ordering::Relaxed);
        let count = self.p95_latency_count.load(Ordering::Relaxed);
        if count == 0 {
            self.get_avg_latency_ms() * 1.5 // Approximation
        } else {
            sum as f64 / count as f64
        }
    }

    /// Get cache hit rate
    pub fn get_cache_hit_rate(&self) -> f64 {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// Get acceptance rate
    pub fn get_acceptance_rate(&self) -> f64 {
        let accepted = self.completions_accepted.load(Ordering::Relaxed);
        let rejected = self.completions_rejected.load(Ordering::Relaxed);
        let total = accepted + rejected;
        if total == 0 {
            0.0
        } else {
            accepted as f64 / total as f64
        }
    }

    /// Generate Prometheus-compatible metrics text
    pub fn generate_prometheus_metrics(&self) -> String {
        let mut output = String::new();

        // Counter metrics
        output.push_str(&format!(
            "# HELP jcode_completion_requests_total Total completion requests\n# TYPE jcode_completion_requests_total counter\njcode_completion_requests_total {}\n",
            self.total_requests.load(Ordering::Relaxed)
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_cache_hits_total Cache hits\n# TYPE jcode_completion_cache_hits_total counter\njcode_completion_cache_hits_total {}\n",
            self.cache_hits.load(Ordering::Relaxed)
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_cache_misses_total Cache misses\n# TYPE jcode_completion_cache_misses_total counter\njcode_completion_cache_misses_total {}\n",
            self.cache_misses.load(Ordering::Relaxed)
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_acceptances_total User acceptances\n# TYPE jcode_completion_acceptances_total counter\njcode_completion_acceptances_total {}\n",
            self.completions_accepted.load(Ordering::Relaxed)
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_rejections_total User rejections\n# TYPE jcode_completion_rejections_total counter\njcode_completion_rejections_total {}\n",
            self.completions_rejected.load(Ordering::Relaxed)
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_prefetch_requests_total Prefetch requests\n# TYPE jcode_completion_prefetch_requests_total counter\njcode_completion_prefetch_requests_total {}\n",
            self.prefetch_requests.load(Ordering::Relaxed)
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_errors_total Completion errors\n# TYPE jcode_completion_errors_total counter\njcode_completion_errors_total {}\n",
            self.errors.load(Ordering::Relaxed)
        ));

        // Gauge metrics
        output.push_str(&format!(
            "# HELP jcode_completion_cache_size Current cache size\n# TYPE jcode_completion_cache_size gauge\njcode_completion_cache_size {}\n",
            self.cache_size.load(Ordering::Relaxed)
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_learned_patterns Learned behavior patterns\n# TYPE jcode_completion_learned_patterns gauge\njcode_completion_learned_patterns {}\n",
            self.learned_patterns.load(Ordering::Relaxed)
        ));

        // Histogram-like metrics
        output.push_str(&format!(
            "# HELP jcode_completion_latency_ms_avg Average completion latency\n# TYPE jcode_completion_latency_ms_avg gauge\njcode_completion_latency_ms_avg {:.2}\n",
            self.get_avg_latency_ms()
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_latency_ms_p95 P95 completion latency\n# TYPE jcode_completion_latency_ms_p95 gauge\njcode_completion_latency_ms_p95 {:.2}\n",
            self.get_p95_latency_ms()
        ));

        // Summary metrics
        output.push_str(&format!(
            "# HELP jcode_completion_cache_hit_rate Cache hit rate\n# TYPE jcode_completion_cache_hit_rate gauge\njcode_completion_cache_hit_rate {:.4}\n",
            self.get_cache_hit_rate()
        ));

        output.push_str(&format!(
            "# HELP jcode_completion_acceptance_rate Acceptance rate\n# TYPE jcode_completion_acceptance_rate gauge\njcode_completion_acceptance_rate {:.4}\n",
            self.get_acceptance_rate()
        ));

        output
    }

    /// Reset all metrics (for testing)
    #[cfg(test)]
    pub fn reset(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.cache_hits.store(0, Ordering::Relaxed);
        self.cache_misses.store(0, Ordering::Relaxed);
        self.completions_accepted.store(0, Ordering::Relaxed);
        self.completions_rejected.store(0, Ordering::Relaxed);
        self.prefetch_requests.store(0, Ordering::Relaxed);
        self.errors.store(0, Ordering::Relaxed);
        self.latency_sum_ms.store(0, Ordering::Relaxed);
        self.latency_count.store(0, Ordering::Relaxed);
        self.p95_latency_sum_ms.store(0, Ordering::Relaxed);
        self.p95_latency_count.store(0, Ordering::Relaxed);
        self.cache_size.store(0, Ordering::Relaxed);
        self.learned_patterns.store(0, Ordering::Relaxed);
    }
}

/// Global metrics instance
static GLOBAL_METRICS: once_cell::sync::Lazy<CompletionMetrics> =
    once_cell::sync::Lazy::new(|| CompletionMetrics::new());

/// Get global metrics collector
pub fn get_metrics() -> &'static CompletionMetrics {
    &GLOBAL_METRICS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_recording() {
        let metrics = CompletionMetrics::new();

        metrics.record_request();
        metrics.record_cache_hit();
        metrics.record_latency(50);
        metrics.record_acceptance();

        assert_eq!(metrics.total_requests.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.cache_hits.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.completions_accepted.load(Ordering::Relaxed), 1);
        assert!((metrics.get_avg_latency_ms() - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_cache_hit_rate() {
        let metrics = CompletionMetrics::new();

        metrics.record_cache_hit();
        metrics.record_cache_hit();
        metrics.record_cache_miss();

        assert!((metrics.get_cache_hit_rate() - 0.6667).abs() < 0.01);
    }

    #[test]
    fn test_prometheus_output() {
        let metrics = CompletionMetrics::new();
        metrics.record_request();
        metrics.record_cache_hit();

        let output = metrics.generate_prometheus_metrics();
        assert!(output.contains("jcode_completion_requests_total 1"));
        assert!(output.contains("jcode_completion_cache_hits_total 1"));
    }
}
