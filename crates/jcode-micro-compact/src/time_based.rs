use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Default time-to-live for cached micro-compact decisions.
pub const DEFAULT_MC_TTL: Duration = Duration::from_secs(60);

/// Time-based trigger that decides when micro-compaction should run.
///
/// Instead of running micro-compaction every turn, this trigger fires
/// only after the cached MC result has expired, avoiding redundant work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBasedTrigger {
    /// How long the cached MC path remains valid.
    pub ttl: Duration,
    /// When the last micro-compaction was performed.
    #[serde(skip)]
    last_compact_at: Option<Instant>,
}

impl TimeBasedTrigger {
    /// Create a new trigger with the given TTL.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            last_compact_at: None,
        }
    }

    /// Create a trigger with the default TTL.
    pub fn default_ttl() -> Self {
        Self::new(DEFAULT_MC_TTL)
    }

    /// Check if micro-compaction should run now.
    ///
    /// Returns `true` if the cache has expired or no prior run exists.
    pub fn should_compact(&self) -> bool {
        match self.last_compact_at {
            Some(at) => at.elapsed() >= self.ttl,
            None => true,
        }
    }

    /// Mark the current time as the last compaction point.
    pub fn record_compaction(&mut self) {
        self.last_compact_at = Some(Instant::now());
    }

    /// Reset the trigger so the next check returns `true`.
    pub fn reset(&mut self) {
        self.last_compact_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_fresh_trigger_should_compact() {
        let trigger = TimeBasedTrigger::new(Duration::from_secs(60));
        assert!(trigger.should_compact());
    }

    #[test]
    fn test_after_record_does_not_compact() {
        let mut trigger = TimeBasedTrigger::new(Duration::from_secs(3600));
        trigger.record_compaction();
        assert!(!trigger.should_compact());
    }

    #[test]
    fn test_reset_triggers_compact() {
        let mut trigger = TimeBasedTrigger::new(Duration::from_secs(3600));
        trigger.record_compaction();
        assert!(!trigger.should_compact());
        trigger.reset();
        assert!(trigger.should_compact());
    }
}
