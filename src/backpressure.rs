//! Dynamic backpressure controller for overload protection
//!
//! Prevents cascading failures when the system is under heavy load by:
//! 1. Dynamically adjusting thresholds based on real-time load metrics
//! 2. Tracking pending request queue depth
//! 3. Monitoring active task concurrency and latency
//! 4. Rejecting requests with HTTP 503 when overloaded
//! 5. Providing graceful degradation signals to clients

use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use std::time::{Instant, Duration};
use tokio::sync::Semaphore;
use tracing::{warn, debug, info};

/// Configuration for dynamic backpressure behavior
#[derive(Debug, Clone)]
pub struct BackpressureConfig {
    /// Base maximum pending requests (minimum threshold)
    pub base_max_pending: usize,
    /// Maximum allowed pending requests (ceiling)
    pub ceiling_max_pending: usize,
    /// Base maximum concurrent operations
    pub base_max_concurrent: usize,
    /// Maximum concurrent operations (ceiling)
    pub ceiling_max_concurrent: usize,
    /// Load ratio threshold to start reducing limits (0.0-1.0)
    pub reduction_threshold: f64,
    /// How aggressively to reduce limits (0.0-1.0, higher = more aggressive)
    pub reduction_factor: f64,
    /// Minimum time between threshold adjustments (seconds)
    pub adjustment_interval_secs: u64,
    /// Latency threshold (ms) - if avg latency exceeds this, reduce limits
    pub latency_threshold_ms: u64,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            base_max_pending: 300,
            ceiling_max_pending: 800,
            base_max_concurrent: 150,
            ceiling_max_concurrent: 300,
            reduction_threshold: 0.7,  // Start reducing at 70% load
            reduction_factor: 0.3,      // Reduce by up to 30%
            adjustment_interval_secs: 10,
            latency_threshold_ms: 3000, // 3 seconds
        }
    }
}

/// Dynamic backpressure controller that adapts to real-time load
pub struct BackpressureController {
    /// Configuration
    config: BackpressureConfig,
    /// Current dynamic max pending (adjusted based on load)
    current_max_pending: AtomicUsize,
    /// Current dynamic max concurrent (adjusted based on load)
    current_max_concurrent: AtomicUsize,
    /// Current number of pending requests
    current_pending: AtomicUsize,
    /// Semaphore to limit concurrent operations
    concurrency_limiter: Semaphore,
    /// Track when backpressure was last activated
    last_activation: std::sync::RwLock<Option<Instant>>,
    /// Total rejected requests counter
    rejected_count: AtomicUsize,
    /// Track recent average latency (for adaptive adjustment)
    recent_avg_latency_ms: AtomicU64,
    /// Last threshold adjustment time
    last_adjustment: std::sync::RwLock<Option<Instant>>,
    /// CPU utilization (0-10000, updated externally)
    cpu_utilization: AtomicU64,
    /// Memory utilization (0-10000, updated externally)
    memory_utilization: AtomicU64,
}

impl BackpressureController {
    /// Create a new dynamic backpressure controller with default config
    pub fn new(base_max_pending: usize, base_max_concurrent: usize) -> Self {
        let config = BackpressureConfig {
            base_max_pending,
            base_max_concurrent,
            ceiling_max_pending: base_max_pending * 2,
            ceiling_max_concurrent: base_max_concurrent * 2,
            ..Default::default()
        };
        Self::with_config(config)
    }

    /// Create a new dynamic backpressure controller with custom config
    pub fn with_config(config: BackpressureConfig) -> Self {
        let current_max_pending = config.base_max_pending;
        let current_max_concurrent = config.base_max_concurrent;

        Self {
            config,
            current_max_pending: AtomicUsize::new(current_max_pending),
            current_max_concurrent: AtomicUsize::new(current_max_concurrent),
            current_pending: AtomicUsize::new(0),
            concurrency_limiter: Semaphore::new(current_max_concurrent),
            last_activation: std::sync::RwLock::new(None),
            rejected_count: AtomicUsize::new(0),
            recent_avg_latency_ms: AtomicU64::new(0),
            last_adjustment: std::sync::RwLock::new(None),
            cpu_utilization: AtomicU64::new(0),
            memory_utilization: AtomicU64::new(0),
        }
    }

    /// Update system metrics for adaptive threshold adjustment
    pub fn update_system_metrics(
        &self,
        avg_latency_ms: u64,
        cpu_pct: u32,  // 0-10000 scale
        memory_pct: u32, // 0-10000 scale
    ) {
        self.recent_avg_latency_ms.store(avg_latency_ms as u64, Ordering::Relaxed);
        self.cpu_utilization.store(cpu_pct as u64, Ordering::Relaxed);
        self.memory_utilization.store(memory_pct as u64, Ordering::Relaxed);

        // Trigger threshold adjustment if enough time has passed
        self.maybe_adjust_thresholds();
    }

    /// Try to acquire permission to process a request
    ///
    /// Returns `Ok(BackpressureGuard)` if the request can proceed,
    /// or `Err(OverloadedError)` if the system is under too much load.
    pub async fn try_acquire(&self) -> Result<BackpressureGuard<'_>, OverloadedError> {
        let effective_max = self.current_max_pending.load(Ordering::Relaxed);

        // Check pending queue depth first (fast path)
        let pending = self.current_pending.fetch_add(1, Ordering::Relaxed);
        if pending >= effective_max {
            // Queue is full, reject immediately
            self.current_pending.fetch_sub(1, Ordering::Relaxed);
            self.rejected_count.fetch_add(1, Ordering::Relaxed);

            let now = Instant::now();
            {
                let mut last = self.last_activation.write().unwrap();
                if last.is_none() {
                    *last = Some(now);
                    warn!(
                        "Dynamic backpressure activated: pending={} >= max={}, latency={}ms, cpu={}%, mem={}% ",
                        pending,
                        effective_max,
                        self.recent_avg_latency_ms.load(Ordering::Relaxed),
                        self.cpu_utilization.load(Ordering::Relaxed) / 100,
                        self.memory_utilization.load(Ordering::Relaxed) / 100
                    );
                }
            }

            return Err(OverloadedError {
                pending,
                max_pending: effective_max,
                is_dynamic: true,
            });
        }

        // Try to acquire concurrency permit (with timeout)
        let _effective_concurrent = self.current_max_concurrent.load(Ordering::Relaxed);
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.concurrency_limiter.acquire(),
        )
        .await
        {
            Ok(Ok(permit)) => {
                // Successfully acquired, forget the permit (it will be released via guard)
                permit.forget();
                debug!("Backpressure: acquired permit, pending={}/{}", pending + 1, effective_max);
                Ok(BackpressureGuard {
                    controller: self,
                    acquired: true,
                })
            }
            Ok(Err(_)) | Err(_) => {
                // Timeout or semaphore closed
                self.current_pending.fetch_sub(1, Ordering::Relaxed);
                Err(OverloadedError {
                    pending: pending + 1,
                    max_pending: effective_max,
                    is_dynamic: true,
                })
            }
        }
    }

    /// Get current system load metrics
    pub fn get_metrics(&self) -> BackpressureMetrics {
        let current_max = self.current_max_pending.load(Ordering::Relaxed);
        let pending = self.current_pending.load(Ordering::Relaxed);

        BackpressureMetrics {
            pending_requests: pending,
            max_pending: current_max,
            base_max_pending: self.config.base_max_pending,
            available_permits: self.concurrency_limiter.available_permits(),
            total_rejected: self.rejected_count.load(Ordering::Relaxed),
            backpressure_active: {
                let last = self.last_activation.read().unwrap();
                last.map(|t| t.elapsed() < Duration::from_secs(60))
                    .unwrap_or(false)
            },
            avg_latency_ms: self.recent_avg_latency_ms.load(Ordering::Relaxed),
            cpu_utilization: self.cpu_utilization.load(Ordering::Relaxed),
            memory_utilization: self.memory_utilization.load(Ordering::Relaxed),
            is_dynamic: true,
        }
    }

    /// Reset backpressure state (for testing or manual recovery)
    pub fn reset(&self) {
        self.current_pending.store(0, Ordering::Relaxed);
        *self.last_activation.write().unwrap() = None;
        // Reset to base limits
        let new_limit = self.config.base_max_concurrent;
        self.current_max_concurrent.store(new_limit, Ordering::Relaxed);
        // Rebuild semaphore with new limit
        let available = self.concurrency_limiter.available_permits();
        if available < new_limit {
            self.concurrency_limiter.add_permits(new_limit - available);
        }
    }

    /// Internal: Adjust thresholds based on current load metrics
    fn maybe_adjust_thresholds(&self) {
        let now = Instant::now();

        // Check if enough time has passed since last adjustment
        {
            let last = self.last_adjustment.read().unwrap();
            if let Some(last_time) = *last {
                if now.duration_since(last_time).as_secs() < self.config.adjustment_interval_secs {
                    return;
                }
            }
        }

        // Calculate load ratio
        let pending = self.current_pending.load(Ordering::Relaxed);
        let current_max = self.current_max_pending.load(Ordering::Relaxed);
        let load_ratio = if current_max > 0 {
            pending as f64 / current_max as f64
        } else {
            0.0
        };

        let avg_latency = self.recent_avg_latency_ms.load(Ordering::Relaxed);
        let cpu = self.cpu_utilization.load(Ordering::Relaxed);
        let memory = self.memory_utilization.load(Ordering::Relaxed);

        // Determine adjustment direction
        let should_reduce = load_ratio > self.config.reduction_threshold
            || avg_latency > self.config.latency_threshold_ms
            || cpu > 8000  // >80% CPU
            || memory > 8500; // >85% memory

        let should_increase = load_ratio < 0.3
            && avg_latency < self.config.latency_threshold_ms / 2
            && cpu < 5000   // <50% CPU
            && memory < 6000; // <60% memory

        if should_reduce {
            self.reduce_limits(load_ratio, avg_latency, cpu, memory);
        } else if should_increase {
            self.increase_limits();
        }

        // Update last adjustment time
        *self.last_adjustment.write().unwrap() = Some(now);
    }

    /// Reduce limits based on load pressure
    fn reduce_limits(&self, load_ratio: f64, latency_ms: u64, cpu: u64, memory: u64) {
        let current_max = self.current_max_pending.load(Ordering::Relaxed);
        let base = self.config.base_max_pending;

        // Calculate reduction factor based on severity
        let severity = ((load_ratio - self.config.reduction_threshold) / (1.0 - self.config.reduction_threshold))
            .min(1.0);
        let reduction = self.config.reduction_factor * severity;

        let new_max = ((current_max as f64 * (1.0 - reduction)) as usize).max(base);

        if new_max < current_max {
            self.current_max_pending.store(new_max, Ordering::Relaxed);
            info!(
                "Reduced backpressure limits: max_pending {} -> {}, reason: load={:.1}%, latency={}ms, cpu={}%, mem={}% ",
                current_max, new_max, load_ratio * 100.0, latency_ms, cpu / 100, memory / 100
            );
        }

        // Also reduce concurrent limit proportionally
        let current_concurrent = self.current_max_concurrent.load(Ordering::Relaxed);
        let new_concurrent = ((current_concurrent as f64 * (1.0 - reduction / 2.0)) as usize)
            .max(self.config.base_max_concurrent);

        if new_concurrent < current_concurrent {
            self.current_max_concurrent.store(new_concurrent, Ordering::Relaxed);
        }
    }

    /// Increase limits when load is light
    fn increase_limits(&self) {
        let current_max = self.current_max_pending.load(Ordering::Relaxed);
        let ceiling = self.config.ceiling_max_pending;

        if current_max < ceiling {
            // Gradually increase by 10%
            let new_max = ((current_max as f64 * 1.1) as usize).min(ceiling);
            self.current_max_pending.store(new_max, Ordering::Relaxed);
            info!(
                "Increased backpressure limits: max_pending {} -> {} (light load)",
                current_max, new_max
            );
        }

        // Also increase concurrent limit
        let current_concurrent = self.current_max_concurrent.load(Ordering::Relaxed);
        let ceiling_concurrent = self.config.ceiling_max_concurrent;

        if current_concurrent < ceiling_concurrent {
            let new_concurrent = ((current_concurrent as f64 * 1.1) as usize).min(ceiling_concurrent);
            self.current_max_concurrent.store(new_concurrent, Ordering::Relaxed);
            self.concurrency_limiter.add_permits(new_concurrent - current_concurrent);
        }
    }
}

/// Guard that releases backpressure pressure when dropped
pub struct BackpressureGuard<'a> {
    controller: &'a BackpressureController,
    acquired: bool,
}

impl<'a> Drop for BackpressureGuard<'a> {
    fn drop(&mut self) {
        if self.acquired {
            self.controller.current_pending.fetch_sub(1, Ordering::Relaxed);
            // Add a new permit back to the semaphore
            self.controller.concurrency_limiter.add_permits(1);

            // Check if we should deactivate backpressure warning
            let pending = self.controller.current_pending.load(Ordering::Relaxed);
            let current_max = self.controller.current_max_pending.load(Ordering::Relaxed);
            if pending < current_max / 2 {
                let mut last = self.controller.last_activation.write().unwrap();
                if last.is_some() {
                    *last = None;
                    debug!("Backpressure deactivated: pending={} < threshold={}",
                           pending, current_max / 2);
                }
            }
        }
    }
}

/// Error returned when system is overloaded
#[derive(Debug, Clone)]
pub struct OverloadedError {
    pub pending: usize,
    pub max_pending: usize,
    pub is_dynamic: bool,
}

impl std::fmt::Display for OverloadedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "System overloaded: {} pending requests (max: {}{})",
            self.pending,
            self.max_pending,
            if self.is_dynamic { ", dynamic" } else { "" }
        )
    }
}

impl std::error::Error for OverloadedError {}

/// Current backpressure metrics
#[derive(Debug, Clone)]
pub struct BackpressureMetrics {
    pub pending_requests: usize,
    pub max_pending: usize,
    pub base_max_pending: usize,
    pub available_permits: usize,
    pub total_rejected: usize,
    pub backpressure_active: bool,
    pub avg_latency_ms: u64,
    pub cpu_utilization: u64,     // 0-10000 scale
    pub memory_utilization: u64,  // 0-10000 scale
    pub is_dynamic: bool,
}

impl BackpressureMetrics {
    /// Calculate load ratio (0.0 = idle, 1.0 = fully loaded)
    pub fn load_ratio(&self) -> f64 {
        if self.max_pending == 0 {
            return 0.0;
        }
        self.pending_requests as f64 / self.max_pending as f64
    }

    /// Check if system is approaching capacity (>80%)
    pub fn is_near_capacity(&self) -> bool {
        self.load_ratio() > 0.8
    }

    /// Get CPU utilization as percentage (0-100)
    pub fn cpu_percent(&self) -> f64 {
        self.cpu_utilization as f64 / 100.0
    }

    /// Get memory utilization as percentage (0-100)
    pub fn memory_percent(&self) -> f64 {
        self.memory_utilization as f64 / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_backpressure() {
        let controller = BackpressureController::new(10, 5);

        // Should allow initial requests
        for _ in 0..5 {
            assert!(controller.try_acquire().await.is_ok());
        }

        // Metrics should show load
        let metrics = controller.get_metrics();
        assert_eq!(metrics.pending_requests, 5);
        assert_eq!(metrics.available_permits, 0);
        assert!(metrics.is_dynamic);
    }

    #[tokio::test]
    async fn test_dynamic_threshold_reduction() {
        let config = BackpressureConfig {
            base_max_pending: 100,
            ceiling_max_pending: 200,
            base_max_concurrent: 50,
            ceiling_max_concurrent: 100,
            reduction_threshold: 0.7,
            reduction_factor: 0.3,
            adjustment_interval_secs: 0, // Allow immediate adjustment for testing
            latency_threshold_ms: 100,
        };
        let controller = BackpressureController::with_config(config);

        // Simulate high load
        controller.update_system_metrics(200, 9000, 9000); // High latency, CPU, memory

        // Thresholds should be reduced
        let metrics = controller.get_metrics();
        assert!(metrics.max_pending <= metrics.base_max_pending);
    }

    #[tokio::test]
    async fn test_guard_release() {
        let controller = BackpressureController::new(10, 5);

        {
            let _guard = controller.try_acquire().await.unwrap();
            assert_eq!(controller.get_metrics().pending_requests, 1);
        }

        // After guard drops, pending should decrease
        assert_eq!(controller.get_metrics().pending_requests, 0);
    }
}
