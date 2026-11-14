use std::time::{Duration, SystemTime};

/// Default minimum hours before auto-dream consolidation is allowed.
pub const DEFAULT_MIN_HOURS: f64 = 24.0;

/// Configuration for the time gate.
#[derive(Debug, Clone)]
pub struct TimeGateConfig {
    /// Minimum hours that must elapse before consolidation proceeds.
    pub min_hours: f64,
}

impl Default for TimeGateConfig {
    fn default() -> Self {
        Self {
            min_hours: DEFAULT_MIN_HOURS,
        }
    }
}

/// Returns the number of hours elapsed since `last`.
///
/// Clamps to zero if `last` is in the future.
pub fn hours_since(last: SystemTime) -> f64 {
    let now = SystemTime::now();
    let elapsed = now.duration_since(last).unwrap_or(Duration::ZERO);
    elapsed.as_secs_f64() / 3_600.0
}

/// Returns `true` when enough time has elapsed since `last_consolidated_at`
/// for auto-dream consolidation to proceed.
pub fn is_time_gate_open(last_consolidated_at: SystemTime) -> bool {
    let cfg = TimeGateConfig::default();
    is_time_gate_open_with_config(last_consolidated_at, &cfg)
}

/// Returns `true` when enough time has elapsed, using the supplied config.
pub fn is_time_gate_open_with_config(
    last_consolidated_at: SystemTime,
    config: &TimeGateConfig,
) -> bool {
    hours_since(last_consolidated_at) >= config.min_hours
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_24_hours() {
        let cfg = TimeGateConfig::default();
        assert!((cfg.min_hours - 24.0).abs() < f64::EPSILON);
    }

    #[test]
    fn hours_since_past() {
        let past = SystemTime::now() - Duration::from_secs(7200); // 2h ago
        let h = hours_since(past);
        assert!((h - 2.0).abs() < 0.01);
    }

    #[test]
    fn hours_since_future_clamps_to_zero() {
        let future = SystemTime::now() + Duration::from_secs(3600);
        let h = hours_since(future);
        assert!((0.0..0.001).contains(&h));
    }

    #[test]
    fn gate_closed_when_too_soon() {
        let just_now = SystemTime::now();
        assert!(!is_time_gate_open(just_now));
    }

    #[test]
    fn gate_open_after_24_hours() {
        let long_ago = SystemTime::now() - Duration::from_secs(25 * 3600);
        assert!(is_time_gate_open(long_ago));
    }

    #[test]
    fn gate_respects_custom_config() {
        let cfg = TimeGateConfig { min_hours: 1.0 };
        let one_hour_ago = SystemTime::now() - Duration::from_secs(3600);
        assert!(is_time_gate_open_with_config(one_hour_ago, &cfg));

        let just_now = SystemTime::now();
        assert!(!is_time_gate_open_with_config(just_now, &cfg));
    }
}
