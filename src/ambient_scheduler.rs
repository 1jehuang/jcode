//! Adaptive usage calculator for ambient mode scheduling.
//!
//! Tracks per-call token usage (user vs ambient), maintains a rolling usage log,
//! and computes adaptive intervals for ambient cycles based on rate limit headroom.

use crate::storage;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Usage record types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum UsageSource {
    User,
    Ambient,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub timestamp: DateTime<Utc>,
    pub source: UsageSource,
    pub tokens_input: u32,
    pub tokens_output: u32,
    pub provider: String,
}

impl UsageRecord {
    pub fn total_tokens(&self) -> u64 {
        self.tokens_input as u64 + self.tokens_output as u64
    }
}

// ---------------------------------------------------------------------------
// Rate limit info (from provider response headers)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    pub limit_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub limit_requests: Option<u64>,
    pub remaining_requests: Option<u64>,
    pub reset_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Usage log — rolling, persisted to disk
// ---------------------------------------------------------------------------

/// How often to auto-save (every N records added).
const SAVE_INTERVAL: usize = 10;

/// Records older than this are pruned on save.
const PRUNE_AGE_HOURS: i64 = 24;

pub struct UsageLog {
    records: Vec<UsageRecord>,
    path: PathBuf,
    unsaved_count: usize,
}

impl UsageLog {
    /// Load (or create) the usage log from the default path.
    pub fn load() -> Self {
        let path = Self::default_path();
        let records: Vec<UsageRecord> = if path.exists() {
            storage::read_json(&path).unwrap_or_default()
        } else {
            Vec::new()
        };
        UsageLog {
            records,
            path,
            unsaved_count: 0,
        }
    }

    fn default_path() -> PathBuf {
        storage::jcode_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("ambient")
            .join("usage.json")
    }

    /// Add a record and periodically save.
    pub fn record(&mut self, record: UsageRecord) {
        self.records.push(record);
        self.unsaved_count += 1;
        if self.unsaved_count >= SAVE_INTERVAL {
            let _ = self.save();
        }
    }

    /// Rolling average of *user* token usage per minute over `window`.
    pub fn user_rate_per_minute(&self, window: Duration) -> f32 {
        self.rate_per_minute(UsageSource::User, window)
    }

    /// Rolling average of *ambient* token usage per minute over `window`.
    pub fn ambient_rate_per_minute(&self, window: Duration) -> f32 {
        self.rate_per_minute(UsageSource::Ambient, window)
    }

    /// Total tokens for a given source within a window.
    pub fn total_tokens_in_window(&self, source: &UsageSource, window: Duration) -> u64 {
        let cutoff = Utc::now() - ChronoDuration::from_std(window).unwrap_or_default();
        self.records
            .iter()
            .filter(|r| r.source == *source && r.timestamp >= cutoff)
            .map(|r| r.total_tokens())
            .sum()
    }

    /// Average tokens per ambient cycle (last N cycles).
    pub fn avg_tokens_per_ambient_cycle(&self, last_n: usize) -> Option<f64> {
        let ambient: Vec<u64> = self
            .records
            .iter()
            .rev()
            .filter(|r| r.source == UsageSource::Ambient)
            .take(last_n)
            .map(|r| r.total_tokens())
            .collect();
        if ambient.is_empty() {
            return None;
        }
        let sum: u64 = ambient.iter().sum();
        Some(sum as f64 / ambient.len() as f64)
    }

    /// Persist to disk, pruning old records.
    pub fn save(&mut self) -> anyhow::Result<()> {
        self.prune();
        storage::write_json(&self.path, &self.records)?;
        self.unsaved_count = 0;
        Ok(())
    }

    // -- internal helpers ---------------------------------------------------

    fn rate_per_minute(&self, source: UsageSource, window: Duration) -> f32 {
        let cutoff = Utc::now() - ChronoDuration::from_std(window).unwrap_or_default();
        let total: u64 = self
            .records
            .iter()
            .filter(|r| r.source == source && r.timestamp >= cutoff)
            .map(|r| r.total_tokens())
            .sum();
        let minutes = window.as_secs_f32() / 60.0;
        if minutes > 0.0 {
            total as f32 / minutes
        } else {
            0.0
        }
    }

    fn prune(&mut self) {
        let cutoff = Utc::now() - ChronoDuration::hours(PRUNE_AGE_HOURS);
        self.records.retain(|r| r.timestamp >= cutoff);
    }
}

// ---------------------------------------------------------------------------
// Scheduler config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AmbientSchedulerConfig {
    pub min_interval_minutes: u32,
    pub max_interval_minutes: u32,
    pub pause_on_active_session: bool,
    /// Fraction of remaining budget reserved for user. 0.8 means ambient gets
    /// at most 20% of headroom.
    pub user_budget_reserve: f32,
}

impl Default for AmbientSchedulerConfig {
    fn default() -> Self {
        AmbientSchedulerConfig {
            min_interval_minutes: 5,
            max_interval_minutes: 120,
            pause_on_active_session: true,
            user_budget_reserve: 0.8,
        }
    }
}

// ---------------------------------------------------------------------------
// Adaptive scheduler
// ---------------------------------------------------------------------------

pub struct AdaptiveScheduler {
    pub usage_log: UsageLog,
    pub config: AmbientSchedulerConfig,
    /// Exponential backoff multiplier (doubles on rate limit hits).
    backoff_multiplier: u32,
    /// Whether a user session is currently active.
    user_active: bool,
}

impl AdaptiveScheduler {
    pub fn new(config: AmbientSchedulerConfig) -> Self {
        AdaptiveScheduler {
            usage_log: UsageLog::load(),
            config,
            backoff_multiplier: 1,
            user_active: false,
        }
    }

    /// Core interval calculation following the algorithm in AMBIENT_MODE.md.
    pub fn calculate_interval(&self, rate_limit_info: Option<&RateLimitInfo>) -> Duration {
        let max = Duration::from_secs(self.config.max_interval_minutes as u64 * 60);
        let min = Duration::from_secs(self.config.min_interval_minutes as u64 * 60);

        // If no rate limit info, fall back to max interval.
        let info = match rate_limit_info {
            Some(i) => i,
            None => return self.apply_backoff(max),
        };

        // window_remaining = reset_time - now
        let window_remaining_secs = info
            .reset_at
            .map(|r| {
                let diff = r - Utc::now();
                diff.num_seconds().max(0) as f64
            })
            .unwrap_or(3600.0); // default 1 hour if unknown

        let tokens_remaining = info.remaining_tokens.unwrap_or(0) as f64;

        if tokens_remaining <= 0.0 || window_remaining_secs <= 0.0 {
            return self.apply_backoff(max);
        }

        // Estimate user consumption from rolling history (last hour).
        let user_rate = self
            .usage_log
            .user_rate_per_minute(Duration::from_secs(3600)) as f64;

        // Project user usage for rest of window.
        let window_remaining_minutes = window_remaining_secs / 60.0;
        let user_projected = user_rate * window_remaining_minutes;

        // Ambient budget = (remaining - user_projected) * (1 - reserve)
        let ambient_fraction = 1.0 - self.config.user_budget_reserve as f64;
        let ambient_budget = (tokens_remaining - user_projected) * ambient_fraction;

        if ambient_budget <= 0.0 {
            // No headroom — wait until window resets.
            return self.apply_backoff(max);
        }

        // Estimate cost per ambient cycle from recent cycles.
        let tokens_per_cycle = self
            .usage_log
            .avg_tokens_per_ambient_cycle(5)
            .unwrap_or(10_000.0); // conservative default

        let cycles_available = ambient_budget / tokens_per_cycle;

        let interval_secs = if cycles_available > 0.0 {
            window_remaining_secs / cycles_available
        } else {
            window_remaining_secs
        };

        let interval = Duration::from_secs_f64(interval_secs);
        self.apply_backoff(interval.clamp(min, max))
    }

    /// Returns `true` if the scheduler thinks ambient should pause (user active).
    pub fn should_pause(&self) -> bool {
        self.config.pause_on_active_session && self.user_active
    }

    /// Mark user session state.
    pub fn set_user_active(&mut self, active: bool) {
        self.user_active = active;
    }

    /// Called when a provider rate limit error occurs.
    pub fn on_rate_limit_hit(&mut self) {
        self.backoff_multiplier = self.backoff_multiplier.saturating_mul(2).min(64);
    }

    /// Called after a successful ambient cycle.
    pub fn on_successful_cycle(&mut self) {
        self.backoff_multiplier = 1;
    }

    // -- internal --

    fn apply_backoff(&self, interval: Duration) -> Duration {
        let min = Duration::from_secs(self.config.min_interval_minutes as u64 * 60);
        let max = Duration::from_secs(self.config.max_interval_minutes as u64 * 60);
        let adjusted = interval.saturating_mul(self.backoff_multiplier);
        adjusted.clamp(min, max)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(source: UsageSource, tokens: u32, mins_ago: i64) -> UsageRecord {
        UsageRecord {
            timestamp: Utc::now() - ChronoDuration::minutes(mins_ago),
            source,
            tokens_input: tokens / 2,
            tokens_output: tokens / 2,
            provider: "test".to_string(),
        }
    }

    #[test]
    fn test_usage_log_rate_per_minute() {
        let mut log = UsageLog {
            records: Vec::new(),
            path: PathBuf::from("/tmp/test_usage.json"),
            unsaved_count: 0,
        };

        // Add 3 user records in the last 30 minutes, 1000 tokens each.
        for i in 0..3 {
            log.records
                .push(make_record(UsageSource::User, 1000, i * 10));
        }

        let rate = log.user_rate_per_minute(Duration::from_secs(3600));
        // 3000 tokens over 60 minutes = 50 tokens/min
        assert!((rate - 50.0).abs() < 1.0, "got {}", rate);
    }

    #[test]
    fn test_total_tokens_in_window() {
        let mut log = UsageLog {
            records: Vec::new(),
            path: PathBuf::from("/tmp/test_usage2.json"),
            unsaved_count: 0,
        };

        log.records
            .push(make_record(UsageSource::User, 500, 10));
        log.records
            .push(make_record(UsageSource::Ambient, 300, 5));
        log.records
            .push(make_record(UsageSource::User, 200, 2));

        let user_total = log.total_tokens_in_window(&UsageSource::User, Duration::from_secs(3600));
        assert_eq!(user_total, 700);

        let ambient_total =
            log.total_tokens_in_window(&UsageSource::Ambient, Duration::from_secs(3600));
        assert_eq!(ambient_total, 300);
    }

    #[test]
    fn test_avg_tokens_per_ambient_cycle() {
        let mut log = UsageLog {
            records: Vec::new(),
            path: PathBuf::from("/tmp/test_usage3.json"),
            unsaved_count: 0,
        };

        // No ambient records => None.
        assert!(log.avg_tokens_per_ambient_cycle(5).is_none());

        log.records
            .push(make_record(UsageSource::Ambient, 1000, 30));
        log.records
            .push(make_record(UsageSource::Ambient, 2000, 20));
        log.records
            .push(make_record(UsageSource::Ambient, 3000, 10));

        let avg = log.avg_tokens_per_ambient_cycle(5).unwrap();
        assert!((avg - 2000.0).abs() < 1.0, "got {}", avg);
    }

    #[test]
    fn test_scheduler_no_rate_limit_returns_max() {
        let config = AmbientSchedulerConfig {
            min_interval_minutes: 5,
            max_interval_minutes: 120,
            ..Default::default()
        };
        let scheduler = AdaptiveScheduler::new(config);
        let interval = scheduler.calculate_interval(None);
        assert_eq!(interval, Duration::from_secs(120 * 60));
    }

    #[test]
    fn test_scheduler_no_remaining_tokens_returns_max() {
        let config = AmbientSchedulerConfig::default();
        let scheduler = AdaptiveScheduler::new(config);

        let info = RateLimitInfo {
            limit_tokens: Some(100_000),
            remaining_tokens: Some(0),
            limit_requests: None,
            remaining_requests: None,
            reset_at: Some(Utc::now() + ChronoDuration::hours(1)),
        };
        let interval = scheduler.calculate_interval(Some(&info));
        assert_eq!(interval, Duration::from_secs(120 * 60));
    }

    #[test]
    fn test_scheduler_plenty_of_headroom() {
        let config = AmbientSchedulerConfig {
            min_interval_minutes: 5,
            max_interval_minutes: 120,
            user_budget_reserve: 0.8,
            ..Default::default()
        };
        let scheduler = AdaptiveScheduler::new(config);

        let info = RateLimitInfo {
            limit_tokens: Some(1_000_000),
            remaining_tokens: Some(500_000),
            limit_requests: None,
            remaining_requests: None,
            reset_at: Some(Utc::now() + ChronoDuration::hours(1)),
        };

        let interval = scheduler.calculate_interval(Some(&info));
        // With 500k remaining, 0 user rate, 20% for ambient = 100k budget.
        // Default 10k per cycle => 10 cycles in 60 min => 6 min per cycle.
        let mins = interval.as_secs() as f64 / 60.0;
        assert!(
            mins >= 5.0 && mins <= 10.0,
            "expected 5-10 min, got {:.1}",
            mins
        );
    }

    #[test]
    fn test_backoff_doubles() {
        let config = AmbientSchedulerConfig {
            min_interval_minutes: 5,
            max_interval_minutes: 120,
            ..Default::default()
        };
        let mut scheduler = AdaptiveScheduler::new(config);

        let info = RateLimitInfo {
            limit_tokens: Some(1_000_000),
            remaining_tokens: Some(500_000),
            limit_requests: None,
            remaining_requests: None,
            reset_at: Some(Utc::now() + ChronoDuration::hours(1)),
        };

        let before = scheduler.calculate_interval(Some(&info));
        scheduler.on_rate_limit_hit();
        let after = scheduler.calculate_interval(Some(&info));

        // After one hit, interval should roughly double (clamped).
        assert!(
            after >= before,
            "after backoff should be >= before: {:?} vs {:?}",
            after,
            before
        );
    }

    #[test]
    fn test_backoff_resets_on_success() {
        let config = AmbientSchedulerConfig::default();
        let mut scheduler = AdaptiveScheduler::new(config);

        scheduler.on_rate_limit_hit();
        scheduler.on_rate_limit_hit();
        assert!(scheduler.backoff_multiplier > 1);

        scheduler.on_successful_cycle();
        assert_eq!(scheduler.backoff_multiplier, 1);
    }

    #[test]
    fn test_should_pause() {
        let config = AmbientSchedulerConfig {
            pause_on_active_session: true,
            ..Default::default()
        };
        let mut scheduler = AdaptiveScheduler::new(config);

        assert!(!scheduler.should_pause());
        scheduler.set_user_active(true);
        assert!(scheduler.should_pause());
        scheduler.set_user_active(false);
        assert!(!scheduler.should_pause());
    }

    #[test]
    fn test_prune_removes_old_records() {
        let mut log = UsageLog {
            records: Vec::new(),
            path: PathBuf::from("/tmp/test_prune.json"),
            unsaved_count: 0,
        };

        // Record from 25 hours ago (should be pruned).
        log.records.push(UsageRecord {
            timestamp: Utc::now() - ChronoDuration::hours(25),
            source: UsageSource::User,
            tokens_input: 100,
            tokens_output: 100,
            provider: "test".to_string(),
        });

        // Recent record (should survive).
        log.records
            .push(make_record(UsageSource::User, 200, 5));

        log.prune();
        assert_eq!(log.records.len(), 1);
        assert_eq!(log.records[0].total_tokens(), 200);
    }
}
