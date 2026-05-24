//! Usage quota tracking and enforcement
//!
//! This module provides per-tenant/per-user quota management:
//! - Token usage tracking (daily/monthly)
//! - Request rate limiting
//! - Quota enforcement with graceful degradation

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};

/// Quota limits for a tenant or user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageQuota {
    /// Maximum tokens per day
    pub max_daily_tokens: u64,

    /// Maximum tokens per month
    pub max_monthly_tokens: u64,

    /// Maximum requests per minute
    pub max_rpm: u64,

    /// Maximum concurrent requests
    pub max_concurrent_requests: u64,

    /// Maximum context window size
    pub max_context_tokens: u64,
}

impl Default for UsageQuota {
    fn default() -> Self {
        Self {
            max_daily_tokens: 100_000,
            max_monthly_tokens: 2_000_000,
            max_rpm: 60,
            max_concurrent_requests: 10,
            max_context_tokens: 200_000,
        }
    }
}

/// Current usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    /// Tokens used today
    pub daily_tokens: u64,

    /// Tokens used this month
    pub monthly_tokens: u64,

    /// Requests in current minute
    pub current_rpm: u64,

    /// Current concurrent requests
    pub concurrent_requests: u64,

    /// Last reset time for daily counter
    pub daily_reset_at: DateTime<Utc>,

    /// Last reset time for monthly counter
    pub monthly_reset_at: DateTime<Utc>,
}

impl Default for UsageStats {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            daily_tokens: 0,
            monthly_tokens: 0,
            current_rpm: 0,
            concurrent_requests: 0,
            daily_reset_at: now,
            monthly_reset_at: now,
        }
    }
}

/// Quota check result
#[derive(Debug, Clone)]
pub enum QuotaCheckResult {
    /// Within quota limits
    Allowed {
        remaining_daily_tokens: u64,
        remaining_monthly_tokens: u64,
    },

    /// Daily token limit exceeded
    DailyLimitExceeded {
        used: u64,
        limit: u64,
    },

    /// Monthly token limit exceeded
    MonthlyLimitExceeded {
        used: u64,
        limit: u64,
    },

    /// Rate limit exceeded
    RateLimitExceeded {
        current_rpm: u64,
        limit: u64,
    },

    /// Concurrent request limit exceeded
    ConcurrentLimitExceeded {
        current: u64,
        limit: u64,
    },
}

impl QuotaCheckResult {
    pub fn is_allowed(&self) -> bool {
        matches!(self, QuotaCheckResult::Allowed { .. })
    }
}

/// Thread-safe quota tracker for a single tenant/user
pub struct QuotaTracker {
    quota: Arc<RwLock<UsageQuota>>,
    stats: Arc<RwLock<UsageStats>>,
}

impl QuotaTracker {
    /// Create a new quota tracker
    pub fn new(quota: UsageQuota) -> Self {
        Self {
            quota: Arc::new(RwLock::new(quota)),
            stats: Arc::new(RwLock::new(UsageStats::default())),
        }
    }

    /// Check if a request is within quota limits
    pub async fn check_quota(&self) -> QuotaCheckResult {
        let quota = self.quota.read().await;
        let stats = self.stats.read().await;

        // Check daily token limit
        if stats.daily_tokens >= quota.max_daily_tokens {
            return QuotaCheckResult::DailyLimitExceeded {
                used: stats.daily_tokens,
                limit: quota.max_daily_tokens,
            };
        }

        // Check monthly token limit
        if stats.monthly_tokens >= quota.max_monthly_tokens {
            return QuotaCheckResult::MonthlyLimitExceeded {
                used: stats.monthly_tokens,
                limit: quota.max_monthly_tokens,
            };
        }

        // Check rate limit
        if stats.current_rpm >= quota.max_rpm {
            return QuotaCheckResult::RateLimitExceeded {
                current_rpm: stats.current_rpm,
                limit: quota.max_rpm,
            };
        }

        // Check concurrent limit
        if stats.concurrent_requests >= quota.max_concurrent_requests {
            return QuotaCheckResult::ConcurrentLimitExceeded {
                current: stats.concurrent_requests,
                limit: quota.max_concurrent_requests,
            };
        }

        QuotaCheckResult::Allowed {
            remaining_daily_tokens: quota.max_daily_tokens - stats.daily_tokens,
            remaining_monthly_tokens: quota.max_monthly_tokens - stats.monthly_tokens,
        }
    }

    /// Record token usage after a successful request
    pub async fn record_usage(&self, tokens_used: u64) {
        let mut stats = self.stats.write().await;
        stats.daily_tokens += tokens_used;
        stats.monthly_tokens += tokens_used;
    }

    /// Increment concurrent request count
    pub async fn increment_concurrent(&self) {
        let mut stats = self.stats.write().await;
        stats.concurrent_requests += 1;
    }

    /// Decrement concurrent request count
    pub async fn decrement_concurrent(&self) {
        let mut stats = self.stats.write().await;
        if stats.concurrent_requests > 0 {
            stats.concurrent_requests -= 1;
        }
    }

    /// Get current usage statistics
    pub async fn get_stats(&self) -> UsageStats {
        self.stats.read().await.clone()
    }

    /// Update quota limits
    pub async fn update_quota(&self, new_quota: UsageQuota) {
        *self.quota.write().await = new_quota;
    }
}

/// Multi-tenant quota enforcer
pub struct QuotaEnforcer {
    trackers: Arc<RwLock<HashMap<String, Arc<QuotaTracker>>>>,
    default_quota: UsageQuota,
}

impl QuotaEnforcer {
    /// Create a new quota enforcer with default limits
    pub fn new(default_quota: UsageQuota) -> Self {
        Self {
            trackers: Arc::new(RwLock::new(HashMap::new())),
            default_quota,
        }
    }

    /// Get or create a quota tracker for a tenant/user
    pub async fn get_tracker(&self, key: &str) -> Arc<QuotaTracker> {
        let mut trackers = self.trackers.write().await;

        if let Some(tracker) = trackers.get(key) {
            Arc::clone(tracker)
        } else {
            let tracker = Arc::new(QuotaTracker::new(self.default_quota.clone()));
            trackers.insert(key.to_string(), Arc::clone(&tracker));
            tracker
        }
    }

    /// Set custom quota for a specific tenant/user
    pub async fn set_custom_quota(&self, key: &str, quota: UsageQuota) {
        let trackers = self.trackers.read().await;
        if let Some(tracker) = trackers.get(key) {
            tracker.update_quota(quota).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_quota_check_allowed() {
        let tracker = QuotaTracker::new(UsageQuota::default());
        let result = tracker.check_quota().await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_quota_record_and_check() {
        let quota = UsageQuota {
            max_daily_tokens: 100,
            ..Default::default()
        };
        let tracker = QuotaTracker::new(quota);

        // Should be allowed initially
        assert!(tracker.check_quota().await.is_allowed());

        // Record usage
        tracker.record_usage(50).await;

        // Should still be allowed with 50 remaining
        match tracker.check_quota().await {
            QuotaCheckResult::Allowed { remaining_daily_tokens, .. } => {
                assert_eq!(remaining_daily_tokens, 50);
            }
            _ => panic!("Expected Allowed"),
        }

        // Exceed quota
        tracker.record_usage(60).await;
        match tracker.check_quota().await {
            QuotaCheckResult::DailyLimitExceeded { used, limit } => {
                assert_eq!(used, 110);
                assert_eq!(limit, 100);
            }
            _ => panic!("Expected DailyLimitExceeded"),
        }
    }
}
