//! # Rate Limiter Module
//!
//! Manages API rate limits to prevent collisions between perspective calls.
//! Ensures even spacing between calls to the same perspective to avoid rate limiting.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::debug;

use crate::perspectives::PerspectiveType;

/// State for tracking when each perspective was last called
#[derive(Debug, Clone)]
pub struct RateLimitState {
    /// When each perspective type was last called
    last_calls: HashMap<PerspectiveType, DateTime<Utc>>,
    /// Minimum interval between calls to the same perspective (seconds)
    min_interval_secs: u64,
    /// Global cooldown between any perspective calls (milliseconds)
    global_cooldown_ms: u64,
    /// When the last global call was made
    last_global_call: Option<Instant>,
}

impl RateLimitState {
    /// Create new rate limit state
    pub fn new(min_interval_secs: u64) -> Self {
        Self {
            last_calls: HashMap::new(),
            min_interval_secs,
            global_cooldown_ms: 100, // 100ms between any calls
            last_global_call: None,
        }
    }

    /// Set minimum interval between calls
    pub fn with_min_interval(mut self, secs: u64) -> Self {
        self.min_interval_secs = secs;
        self
    }

    /// Set global cooldown between calls
    pub fn with_global_cooldown(mut self, ms: u64) -> Self {
        self.global_cooldown_ms = ms;
        self
    }

    /// Check if a perspective can be called now
    pub fn can_call(&self, perspective: PerspectiveType) -> bool {
        self.time_since_last_call(perspective) >= Duration::from_secs(self.min_interval_secs)
    }

    /// Get time elapsed since last call to a perspective
    pub fn time_since_last_call(&self, perspective: PerspectiveType) -> Duration {
        match self.last_calls.get(&perspective) {
            Some(last) => Utc::now()
                .signed_duration_since(*last)
                .to_std()
                .unwrap_or(Duration::ZERO),
            None => Duration::MAX,
        }
    }

    /// Record a call for a perspective
    pub fn record_call(&mut self, perspective: PerspectiveType) {
        self.last_calls.insert(perspective, Utc::now());
        self.last_global_call = Some(Instant::now());
    }

    /// Calculate wait time before next call to a perspective
    pub fn wait_time(&self, perspective: PerspectiveType) -> Duration {
        let perspective_wait = if self.can_call(perspective) {
            Duration::ZERO
        } else {
            let elapsed = self.time_since_last_call(perspective);
            let required = Duration::from_secs(self.min_interval_secs);
            required.saturating_sub(elapsed)
        };

        let global_wait = self.global_wait_time();

        perspective_wait.max(global_wait)
    }

    /// Get global cooldown wait time
    pub fn global_wait_time(&self) -> Duration {
        match self.last_global_call {
            Some(last) => {
                let elapsed = last.elapsed();
                if elapsed >= Duration::from_millis(self.global_cooldown_ms) {
                    Duration::ZERO
                } else {
                    Duration::from_millis(self.global_cooldown_ms).saturating_sub(elapsed)
                }
            }
            None => Duration::ZERO,
        }
    }

    /// Reset state
    pub fn reset(&mut self) {
        self.last_calls.clear();
        self.last_global_call = None;
    }
}

impl Default for RateLimitState {
    fn default() -> Self {
        Self::new(2) // 2 second default interval
    }
}

/// Async rate limiter for use in concurrent contexts
#[derive(Debug, Clone)]
pub struct RateLimiter {
    /// Shared state
    state: Arc<RwLock<RateLimitState>>,
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new(min_interval_secs: u64) -> Self {
        Self {
            state: Arc::new(RwLock::new(RateLimitState::new(min_interval_secs))),
        }
    }

    /// Create with custom configuration
    pub fn with_config(min_interval_secs: u64, global_cooldown_ms: u64) -> Self {
        Self {
            state: Arc::new(RwLock::new(
                RateLimitState::new(min_interval_secs).with_global_cooldown(global_cooldown_ms),
            )),
        }
    }

    /// Check if a perspective can be called
    pub async fn can_call(&self, perspective: PerspectiveType) -> bool {
        let state = self.state.read().await;
        state.can_call(perspective)
    }

    /// Wait until a perspective can be called, returning the wait time
    pub async fn wait_for_next(&self, perspective: PerspectiveType) -> Option<Duration> {
        let wait = {
            let state = self.state.read().await;
            state.wait_time(perspective)
        };

        if wait > Duration::ZERO {
            debug!("Rate limiting {}: waiting {:?}", perspective, wait);
            tokio::time::sleep(wait).await;
        }

        Some(wait).filter(|d| *d > Duration::ZERO)
    }

    /// Calculate how long to wait before calling a perspective
    pub async fn wait_duration(&self, perspective: PerspectiveType) -> Duration {
        let state = self.state.read().await;
        state.wait_time(perspective)
    }

    /// Mark that we're about to call a perspective (for state checking)
    pub fn should_wait(&self, perspective: PerspectiveType) -> bool {
        // This is a synchronous check - use in non-async contexts
        // For async contexts, use can_call or wait_for_next
        let state = self.state.blocking_read();
        !state.can_call(perspective)
    }

    /// Record a call for rate limiting
    pub async fn mark_call(&self, perspective: PerspectiveType) {
        let mut state = self.state.write().await;
        state.record_call(perspective);
    }

    /// Reset all rate limit state
    pub async fn reset(&self) {
        let mut state = self.state.write().await;
        state.reset();
    }

    /// Get time until next call to a perspective
    pub async fn time_until_next_call(&self, perspective: PerspectiveType) -> Duration {
        let state = self.state.read().await;
        let elapsed = state.time_since_last_call(perspective);
        let required = Duration::from_secs(state.min_interval_secs);

        if elapsed >= required {
            Duration::ZERO
        } else {
            required.saturating_sub(elapsed)
        }
    }

    /// Get all perspective call times (for debugging)
    pub async fn last_call_times(&self) -> HashMap<PerspectiveType, DateTime<Utc>> {
        let state = self.state.read().await;
        state.last_calls.clone()
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(2)
    }
}

/// Stagger strategy for avoiding rate limit collisions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaggerStrategy {
    /// No staggering
    None,
    /// Fixed delay between calls
    Fixed { ms: u64 },
    /// Adaptive delay based on call frequency
    Adaptive,
    /// Jittered delays to spread load
    Jitter { base_ms: u64, variance_ms: u64 },
}

impl Default for StaggerStrategy {
    fn default() -> Self {
        Self::Adaptive
    }
}

/// Stagger helper for coordinating multiple perspective calls
#[derive(Debug)]
pub struct StaggerHelper {
    strategy: StaggerStrategy,
}

impl StaggerHelper {
    /// Create a new stagger helper
    pub fn new(strategy: StaggerStrategy) -> Self {
        Self { strategy }
    }

    /// Calculate stagger delay for a perspective
    pub fn stagger_delay(&self, perspective: PerspectiveType, call_index: usize) -> Duration {
        match self.strategy {
            StaggerStrategy::None => Duration::ZERO,
            StaggerStrategy::Fixed { ms } => Duration::from_millis(ms * call_index as u64),
            StaggerStrategy::Adaptive => {
                // Adaptive: stagger based on perspective type and call order
                let base = 500; // 500ms base
                let perspective_offset = match perspective {
                    PerspectiveType::Advocate => 0,
                    PerspectiveType::Critic => 200,
                    PerspectiveType::Synthesizer => 400,
                };
                Duration::from_millis((base + perspective_offset) * (call_index as u64 + 1))
            }
            StaggerStrategy::Jitter {
                base_ms,
                variance_ms,
            } => {
                use std::time::SystemTime;
                // Use current time for some pseudo-randomness
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                let jitter = (now % variance_ms) + (call_index as u64 * 50);
                Duration::from_millis(base_ms + jitter)
            }
        }
    }
}

impl Default for StaggerHelper {
    fn default() -> Self {
        Self::new(StaggerStrategy::default())
    }
}

/// Rate limit statistics
#[derive(Debug, Clone)]
pub struct RateLimitStats {
    pub total_calls: u64,
    pub blocked_calls: u64,
    pub total_wait_ms: u64,
    pub by_perspective: HashMap<PerspectiveType, PerspectiveStats>,
}

#[derive(Debug, Clone)]
pub struct PerspectiveStats {
    pub calls: u64,
    pub total_wait_ms: u64,
    pub avg_wait_ms: f64,
    pub last_call: Option<DateTime<Utc>>,
}

impl Default for PerspectiveStats {
    fn default() -> Self {
        Self {
            calls: 0,
            total_wait_ms: 0,
            avg_wait_ms: 0.0,
            last_call: None,
        }
    }
}

impl Default for RateLimitStats {
    fn default() -> Self {
        Self {
            total_calls: 0,
            blocked_calls: 0,
            total_wait_ms: 0,
            by_perspective: HashMap::new(),
        }
    }
}

/// Tracking rate limiter that records statistics
#[derive(Debug)]
pub struct TrackingRateLimiter<R: RateLimiterTrait> {
    inner: R,
    stats: Arc<RwLock<RateLimitStats>>,
}

impl<R: RateLimiterTrait> TrackingRateLimiter<R> {
    /// Create a new tracking rate limiter
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            stats: Arc::new(RwLock::new(RateLimitStats::default())),
        }
    }

    /// Get current statistics
    pub async fn stats(&self) -> RateLimitStats {
        self.stats.read().await.clone()
    }

    /// Record a call and update stats
    pub async fn record_call(&self, perspective: PerspectiveType, wait_ms: u64) {
        self.inner.mark_call(perspective).await;

        let mut stats = self.stats.write().await;
        stats.total_calls += 1;
        stats.total_wait_ms += wait_ms;

        if wait_ms > 0 {
            stats.blocked_calls += 1;
        }

        let p_stats = stats.by_perspective.entry(perspective).or_default();
        p_stats.calls += 1;
        p_stats.total_wait_ms += wait_ms;
        p_stats.avg_wait_ms = p_stats.total_wait_ms as f64 / p_stats.calls as f64;
        p_stats.last_call = Some(Utc::now());
    }
}

/// Trait for rate limiters (for testing/mocking)
#[async_trait::async_trait]
pub trait RateLimiterTrait: Send + Sync {
    /// Check if a perspective can be called
    async fn can_call(&self, perspective: PerspectiveType) -> bool;

    /// Record a call
    async fn mark_call(&self, perspective: PerspectiveType);
}

#[async_trait::async_trait]
impl RateLimiterTrait for RateLimiter {
    async fn can_call(&self, perspective: PerspectiveType) -> bool {
        RateLimiter::can_call(self, perspective).await
    }

    async fn mark_call(&self, perspective: PerspectiveType) {
        RateLimiter::mark_call(self, perspective).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_state_creation() {
        let state = RateLimitState::new(5);
        assert!(state.can_call(PerspectiveType::Advocate));
        assert_eq!(state.min_interval_secs, 5);
    }

    #[test]
    fn rate_limit_state_record_call() {
        let mut state = RateLimitState::new(1);
        assert!(state.can_call(PerspectiveType::Advocate));

        state.record_call(PerspectiveType::Advocate);
        assert!(!state.can_call(PerspectiveType::Advocate));
        assert!(state.can_call(PerspectiveType::Critic));
    }

    #[test]
    fn rate_limit_state_wait_time() {
        let mut state = RateLimitState::new(5);
        state.record_call(PerspectiveType::Advocate);

        let wait = state.wait_time(PerspectiveType::Advocate);
        // Should wait approximately 5 seconds
        assert!(wait > Duration::from_secs(4));
        assert!(wait <= Duration::from_secs(5));
    }

    #[test]
    fn rate_limiter_creation() {
        let limiter = RateLimiter::new(3);
        assert_eq!(limiter.state.blocking_read().min_interval_secs, 3);
    }

    #[tokio::test]
    async fn rate_limiter_can_call() {
        let limiter = RateLimiter::new(1);

        assert!(limiter.can_call(PerspectiveType::Advocate).await);
        assert!(limiter.can_call(PerspectiveType::Critic).await);

        limiter.mark_call(PerspectiveType::Advocate).await;

        assert!(!limiter.can_call(PerspectiveType::Advocate).await);
        assert!(limiter.can_call(PerspectiveType::Critic).await);
    }

    #[tokio::test]
    async fn rate_limiter_mark_call() {
        let limiter = RateLimiter::new(1);

        limiter.mark_call(PerspectiveType::Advocate).await;
        limiter.mark_call(PerspectiveType::Critic).await;

        assert!(!limiter.can_call(PerspectiveType::Advocate).await);
        assert!(!limiter.can_call(PerspectiveType::Critic).await);
    }

    #[tokio::test]
    async fn rate_limiter_reset() {
        let limiter = RateLimiter::new(1);

        limiter.mark_call(PerspectiveType::Advocate).await;
        assert!(!limiter.can_call(PerspectiveType::Advocate).await);

        limiter.reset().await;
        assert!(limiter.can_call(PerspectiveType::Advocate).await);
    }

    #[test]
    fn stagger_strategy_fixed() {
        let helper = StaggerHelper::new(StaggerStrategy::Fixed { ms: 100 });

        let delay1 = helper.stagger_delay(PerspectiveType::Advocate, 0);
        let delay2 = helper.stagger_delay(PerspectiveType::Advocate, 1);

        assert_eq!(delay1, Duration::from_millis(0));
        assert_eq!(delay2, Duration::from_millis(100));
    }

    #[test]
    fn stagger_strategy_adaptive() {
        let helper = StaggerHelper::new(StaggerStrategy::Adaptive);

        let advocate_delay = helper.stagger_delay(PerspectiveType::Advocate, 0);
        let critic_delay = helper.stagger_delay(PerspectiveType::Critic, 0);

        // Advocate should have shorter delay than critic
        assert!(advocate_delay < critic_delay);
    }

    #[tokio::test]
    async fn tracking_rate_limiter() {
        let inner = RateLimiter::new(1);
        let tracker = TrackingRateLimiter::new(inner);

        tracker.record_call(PerspectiveType::Advocate, 0).await;
        tracker.record_call(PerspectiveType::Critic, 500).await;

        let stats = tracker.stats().await;
        assert_eq!(stats.total_calls, 2);
        assert_eq!(stats.blocked_calls, 1);
        assert_eq!(stats.total_wait_ms, 500);

        let advocate_stats = &stats.by_perspective[&PerspectiveType::Advocate];
        assert_eq!(advocate_stats.calls, 1);
        assert_eq!(advocate_stats.total_wait_ms, 0);
    }

    #[test]
    fn rate_limit_state_global_cooldown() {
        let mut state = RateLimitState::new(1).with_global_cooldown(200);

        state.record_call(PerspectiveType::Advocate);

        let global_wait = state.global_wait_time();
        assert!(global_wait > Duration::ZERO);
        assert!(global_wait <= Duration::from_millis(200));
    }

    #[tokio::test]
    async fn rate_limiter_with_config() {
        let limiter = RateLimiter::with_config(5, 300);

        limiter.mark_call(PerspectiveType::Advocate).await;

        let wait = limiter.wait_duration(PerspectiveType::Advocate).await;
        // Wait should be close to 5 seconds (within 100ms tolerance for timing)
        assert!(wait >= Duration::from_secs(4));
        assert!(wait <= Duration::from_secs(6));
    }
}
