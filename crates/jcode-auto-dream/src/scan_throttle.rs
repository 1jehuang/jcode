//! Scan throttle -- 10-minute cooldown between background memory scans.
//!
//! Prevents the scanner from spinning too frequently by enforcing a minimum
//! interval between successive scan attempts.

use std::time::{Duration, Instant};

/// A simple timer-based throttle for scan operations.
#[derive(Debug)]
pub struct ScanThrottle {
    /// Minimum interval between scans.
    cooldown: Duration,
    /// When the last scan completed.
    last_scan: Option<Instant>,
}

impl ScanThrottle {
    /// Create a throttle with the given cooldown duration.
    pub fn new(cooldown: Duration) -> Self {
        Self {
            cooldown,
            last_scan: None,
        }
    }

    /// Create a throttle with the default 10-minute cooldown.
    pub fn default_10min() -> Self {
        Self::new(Duration::from_secs(10 * 60))
    }

    /// Returns the cooldown duration.
    pub fn cooldown(&self) -> Duration {
        self.cooldown
    }

    /// Check whether a scan is allowed (cooldown has elapsed).
    pub fn is_allowed(&self) -> bool {
        match self.last_scan {
            Some(last) => last.elapsed() >= self.cooldown,
            None => true,
        }
    }

    /// Record that a scan just completed.
    pub fn record_scan(&mut self) {
        self.last_scan = Some(Instant::now());
    }

    /// How long until the next scan is allowed. Returns `Duration::ZERO` if
    /// a scan is already permitted.
    pub fn time_until_allowed(&self) -> Duration {
        match self.last_scan {
            Some(last) => {
                let elapsed = last.elapsed();
                if elapsed >= self.cooldown {
                    Duration::ZERO
                } else {
                    self.cooldown - elapsed
                }
            }
            None => Duration::ZERO,
        }
    }
}

impl Default for ScanThrottle {
    fn default() -> Self {
        Self::default_10min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_before_any_scan() {
        let throttle = ScanThrottle::new(Duration::from_secs(60));
        assert!(throttle.is_allowed());
    }

    #[test]
    fn not_allowed_immediately_after_scan() {
        let mut throttle = ScanThrottle::new(Duration::from_secs(60));
        throttle.record_scan();
        assert!(!throttle.is_allowed());
    }
}
