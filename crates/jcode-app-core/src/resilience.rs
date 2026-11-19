//! Run brake and named recovery reasons (after paperclip's
//! `recovery/automatic-run-brake.ts` and continuation causes).
//!
//! Pure state, caller supplies `Instant`, so it is testable without sleeping.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const FAILURE_WINDOW: Duration = Duration::from_secs(600);
pub const OPEN_THRESHOLD: usize = 12;
pub const COOLDOWN: Duration = Duration::from_secs(60);
pub const PROBE_SUCCESSES_TO_CLOSE: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Run normally.
    Allow,
    /// Run as the single half-open probe; report its outcome.
    Probe,
    /// Dropped: an equivalent retry is already pending or the brake is open.
    Coalesced,
}

#[derive(Debug)]
pub struct CircuitBreaker {
    state: BreakerState,
    failures: VecDeque<Instant>,
    opened_at: Option<Instant>,
    probe_in_flight: bool,
    probe_successes: u32,
    coalesced: u64,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self {
            state: BreakerState::Closed,
            failures: VecDeque::new(),
            opened_at: None,
            probe_in_flight: false,
            probe_successes: 0,
            coalesced: 0,
        }
    }
}

impl CircuitBreaker {
    pub fn state(&self, now: Instant) -> BreakerState {
        match (self.state, self.opened_at) {
            (BreakerState::Open, Some(t)) if now.duration_since(t) >= COOLDOWN => {
                BreakerState::HalfOpen
            }
            (s, _) => s,
        }
    }

    pub fn coalesced_count(&self) -> u64 {
        self.coalesced
    }

    /// Ask before starting a provider/tool/sub-agent retry.
    pub fn admit(&mut self, now: Instant) -> Admission {
        self.state = self.state(now);
        match self.state {
            BreakerState::Closed => Admission::Allow,
            BreakerState::HalfOpen if !self.probe_in_flight => {
                self.probe_in_flight = true;
                Admission::Probe
            }
            _ => {
                self.coalesced += 1;
                Admission::Coalesced
            }
        }
    }

    pub fn record_failure(&mut self, now: Instant) {
        self.state = self.state(now);
        match self.state {
            BreakerState::HalfOpen => self.trip(now),
            BreakerState::Open => {}
            BreakerState::Closed => {
                self.failures.push_back(now);
                while self
                    .failures
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > FAILURE_WINDOW)
                {
                    self.failures.pop_front();
                }
                if self.failures.len() >= OPEN_THRESHOLD {
                    self.trip(now);
                }
            }
        }
    }

    pub fn record_success(&mut self, now: Instant) {
        self.state = self.state(now);
        if self.state == BreakerState::HalfOpen {
            self.probe_in_flight = false;
            self.probe_successes += 1;
            if self.probe_successes >= PROBE_SUCCESSES_TO_CLOSE {
                *self = Self {
                    coalesced: self.coalesced,
                    ..Self::default()
                };
            }
        }
    }

    fn trip(&mut self, now: Instant) {
        self.state = BreakerState::Open;
        self.opened_at = Some(now);
        self.failures.clear();
        self.probe_in_flight = false;
        self.probe_successes = 0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryReason {
    /// Transient or restart-class: auto-resume, no human needed.
    Interrupted,
    /// Credentials, approvals or policy: escalate once to the user.
    UserActionRequired,
}

const USER_ACTION_MARKERS: &[&str] = &[
    "missing credential",
    "missing api key",
    "api key not",
    "unauthorized",
    "401",
    "403",
    "forbidden",
    "invalid_grant",
    "not logged in",
    "login required",
    "requires approval",
    "approval required",
    "blast-radius",
    "blast radius",
    "policy block",
    "blocked by policy",
    "permission denied",
    "requires_user_auth",
];

/// User-action markers win; everything else (network drops, restarts,
/// timeouts, unknown) is `Interrupted` so it auto-resumes.
pub fn classify_interruption(message: &str) -> RecoveryReason {
    let text = message.to_ascii_lowercase();
    if USER_ACTION_MARKERS.iter().any(|m| text.contains(m)) {
        RecoveryReason::UserActionRequired
    } else {
        RecoveryReason::Interrupted
    }
}

/// One escalation line for all user-action items, `None` if nothing needs a human.
pub fn escalation_summary<'a>(messages: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut items: Vec<&str> = messages
        .into_iter()
        .filter(|m| classify_interruption(m) == RecoveryReason::UserActionRequired)
        .collect();
    items.dedup();
    (!items.is_empty()).then(|| format!("Action required ({}): {}", items.len(), items.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trip(b: &mut CircuitBreaker, t0: Instant) {
        for i in 0..OPEN_THRESHOLD as u64 {
            b.record_failure(t0 + Duration::from_secs(i));
        }
    }

    #[test]
    fn full_cycle_closed_open_half_open_closed() {
        let t0 = Instant::now();
        let mut b = CircuitBreaker::default();
        for i in 0..(OPEN_THRESHOLD as u64 - 1) {
            b.record_failure(t0 + Duration::from_secs(i));
        }
        assert_eq!(b.state(t0), BreakerState::Closed);
        b.record_failure(t0 + Duration::from_secs(11));
        let opened = t0 + Duration::from_secs(11);
        assert_eq!(b.state(opened), BreakerState::Open);
        assert_eq!(b.admit(opened), Admission::Coalesced);

        let half = opened + COOLDOWN;
        assert_eq!(b.state(half), BreakerState::HalfOpen);
        assert_eq!(b.admit(half), Admission::Probe);
        b.record_success(half);
        assert_eq!(b.state(half), BreakerState::HalfOpen);
        assert_eq!(b.admit(half), Admission::Probe);
        b.record_success(half);
        assert_eq!(b.state(half), BreakerState::Closed);
        assert_eq!(b.admit(half), Admission::Allow);
    }

    #[test]
    fn failures_outside_window_do_not_trip() {
        let t0 = Instant::now();
        let mut b = CircuitBreaker::default();
        for i in 0..20u64 {
            b.record_failure(t0 + Duration::from_secs(i * 120));
        }
        assert_eq!(
            b.state(t0 + Duration::from_secs(2400)),
            BreakerState::Closed
        );
    }

    #[test]
    fn duplicate_triggers_coalesce_while_open_and_during_probe() {
        let t0 = Instant::now();
        let mut b = CircuitBreaker::default();
        trip(&mut b, t0);
        let t = t0 + Duration::from_secs(20);
        for _ in 0..5 {
            assert_eq!(b.admit(t), Admission::Coalesced);
        }
        let half = t0 + Duration::from_secs(11) + COOLDOWN;
        assert_eq!(b.admit(half), Admission::Probe);
        assert_eq!(b.admit(half), Admission::Coalesced);
        assert_eq!(b.coalesced_count(), 6);
    }

    #[test]
    fn probe_failure_reopens() {
        let t0 = Instant::now();
        let mut b = CircuitBreaker::default();
        trip(&mut b, t0);
        let half = t0 + Duration::from_secs(11) + COOLDOWN;
        assert_eq!(b.admit(half), Admission::Probe);
        b.record_failure(half);
        assert_eq!(b.state(half), BreakerState::Open);
        assert_eq!(b.state(half + COOLDOWN), BreakerState::HalfOpen);
    }

    #[test]
    fn classifies_interrupted_vs_user_action() {
        for m in [
            "connection reset by peer",
            "server restart during reload",
            "operation timed out",
            "stream ended unexpectedly",
        ] {
            assert_eq!(classify_interruption(m), RecoveryReason::Interrupted, "{m}");
        }
        for m in [
            "Missing API key for provider",
            "HTTP 401 Unauthorized",
            "BLAST-RADIUS GATE: requires approval",
            "blocked by policy: anchor-write",
        ] {
            assert_eq!(
                classify_interruption(m),
                RecoveryReason::UserActionRequired,
                "{m}"
            );
        }
    }

    #[test]
    fn escalation_summary_is_single_and_skips_transient() {
        assert_eq!(escalation_summary(["connection reset", "timed out"]), None);
        let s = escalation_summary(["timed out", "HTTP 401 Unauthorized", "approval required"])
            .unwrap();
        assert_eq!(
            s,
            "Action required (2): HTTP 401 Unauthorized; approval required"
        );
    }
}
