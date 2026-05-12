//! Metrics collection for CarpAI SDK

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::time::Instant;

/// Metrics collector for tracking SDK performance
pub struct MetricsCollector {
    /// Total number of requests
    total_requests: AtomicU64,

    /// Successful requests
    successful_requests: AtomicU64,

    /// Failed requests
    failed_requests: AtomicU64,

    /// Cache hits
    cache_hits: AtomicU64,

    /// Cache misses
    cache_misses: AtomicU64,

    /// Total latency in milliseconds (sum of all request latencies)
    total_latency_ms: AtomicU64,

    /// Total tokens consumed
    total_tokens: AtomicU64,

    /// Whether metrics collection is enabled
    enabled: AtomicBool,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            total_latency_ms: AtomicU64::new(0),
            total_tokens: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
        }
    }

    /// Enable or disable metrics collection
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Record a completed request
    pub fn record_request(&self, latency_ms: u64, tokens: u32, success: bool, cached: bool) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }

        self.total_requests.fetch_add(1, Ordering::Relaxed);

        if success {
            self.successful_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
        }

        if cached {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.cache_misses.fetch_add(1, Ordering::Relaxed);
        }

        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
        self.total_tokens.fetch_add(tokens as u64, Ordering::Relaxed);
    }

    /// Get current metrics snapshot
    pub fn snapshot(&self) -> MetricsSnapshot {
        let total = self.total_requests.load(Ordering::Relaxed);
        let successful = self.successful_requests.load(Ordering::Relaxed);
        let failed = self.failed_requests.load(Ordering::Relaxed);
        let cache_hits = self.cache_hits.load(Ordering::Relaxed);
        let cache_misses = self.cache_misses.load(Ordering::Relaxed);
        let total_latency = self.total_latency_ms.load(Ordering::Relaxed);
        let total_tokens = self.total_tokens.load(Ordering::Relaxed);

        let avg_latency_ms = if total > 0 { Some(total_latency as f64 / total as f64) } else { None };
        let success_rate = if total > 0 { Some(successful as f64 / total as f64) } else { None };
        let cache_hit_rate = if (cache_hits + cache_misses) > 0 {
            Some(cache_hits as f64 / (cache_hits + cache_misses) as f64)
        } else {
            None
        };

        MetricsSnapshot {
            total_requests: total,
            successful_requests: successful,
            failed_requests: failed,
            cache_hits,
            cache_misses,
            cache_hit_rate,
            total_tokens,
            avg_latency_ms,
            success_rate,
        }
    }

    /// Reset all metrics
    pub fn reset(&self) {
        self.total_requests.store(0, Ordering::Relaxed);
        self.successful_requests.store(0, Ordering::Relaxed);
        self.failed_requests.store(0, Ordering::Relaxed);
        self.cache_hits.store(0, Ordering::Relaxed);
        self.cache_misses.store(0, Ordering::Relaxed);
        self.total_latency_ms.store(0, Ordering::Relaxed);
        self.total_tokens.store(0, Ordering::Relaxed);
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of current metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_hit_rate: Option<f64>,
    pub total_tokens: u64,
    pub avg_latency_ms: Option<f64>,
    pub success_rate: Option<f64>,
}

impl std::fmt::Display for MetricsSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Metrics:\n\
             - Total Requests: {}\n\
             - Success Rate: {:.1}%\n\
             - Avg Latency: {:.1}ms\n\
             - Cache Hit Rate: {:.1}%\n\
             - Total Tokens: {}",
            self.total_requests,
            self.success_rate.unwrap_or(0.0) * 100.0,
            self.avg_latency_ms.unwrap_or(0.0),
            self.cache_hit_rate.unwrap_or(0.0) * 100.0,
            self.total_tokens
        )
    }
}
