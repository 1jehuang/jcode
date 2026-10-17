//! # jcode-auto-dream
//!
//! Background memory consolidation gate mechanics (time + session + lock).
//!
//! This crate provides the **gate evaluation** half of auto-dream. It does
//! **not** perform the consolidation LLM call itself. The crate is the
//! standalone, side-effect-free gate library; the actual consolidation call
//! lives in `jcode-app-core::ambient::auto_dream`, which uses a real
//! `jcode_provider_core::Provider` to invoke the model.
//!
//! ## Three gates
//!
//! [`DreamCoordinator`] evaluates three independent gates. All three must pass
//! before consolidation is allowed to run:
//!
//! 1. **Time gate** - at least 24 hours have elapsed since the last consolidation.
//! 2. **Session gate** - at least 5 sessions have occurred since the last
//!    consolidation.
//! 3. **Lock gate** - a file-based lock is acquired to prevent concurrent
//!    consolidation runs.
//!
//! Additional subsystems:
//! - **Scan throttle** - limits how often memory scans fire (10-minute cooldown).
//! - **Consolidation prompt** - builds the LLM prompt payload that *would* be
//!   sent to the model if consolidation were wired up here. The real caller in
//!   `jcode-app-core` already builds its own prompt via this builder.
//!
//! # Parking status (AUDIT-2026-10-16)
//!
//! The previous `maybe_dream()` entry point in this crate was a stub: it
//! logged `"gates satisfied, consolidation would run"` and did nothing.
//! As of the A-2 audit finding (2026-10-16), that stub has been retired.
//! The crate now only provides the gate library. The live consolidation
//! path lives in `jcode-app-core/src/ambient/auto_dream.rs`, which uses
//! the real LLM provider.
//!
//! If you want to wire a new caller from this crate, see
//! [`DreamCoordinator::evaluate`] and the [`consolidation_prompt`] builder.
//! Do **not** reintroduce the old `WouldConsolidate` log line.
//!
//! Audit owner: jcode / A-2 (stubs-mocks, 2026-10-16).
//! Memory Graph Phase A design: see `docs/plans/MEMORY_GRAPH_PLAN.md`.
//!
//! # Module overview
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`time_gate`] | Tracks elapsed wall-clock time since last consolidation |
//! | [`session_gate`] | Counts sessions since last consolidation |
//! | [`consolidation_lock`] | Acquires/releases a file lock for exclusive access |
//! | [`scan_throttle`] | 10-minute cooldown between background scans |
//! | [`consolidation_prompt`] | Builds the prompt payload for the consolidation LLM call |

pub mod consolidation_lock;
pub mod consolidation_prompt;
pub mod scan_throttle;
pub mod session_gate;
pub mod time_gate;

// Re-export key public types for downstream consumers.
pub use consolidation_lock::ConsolidationLock;
pub use consolidation_prompt::ConsolidationPrompt;
pub use scan_throttle::ScanThrottle;
pub use session_gate::SessionGate;

/// Configuration constants for auto-dream thresholds.
pub const DEFAULT_TIME_GATE_HOURS: u64 = 24;
pub const DEFAULT_SESSION_GATE_COUNT: u32 = 5;
pub const DEFAULT_SCAN_THROTTLE_MINUTES: u64 = 10;

/// Outcome of evaluating all three gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamTrigger {
    /// All three gates passed -- consolidation is allowed to run.
    Ready,
    /// One or more gates have not yet been satisfied.
    NotReady { reason: &'static str },
}

/// Outcome of a [`maybe_dream`] evaluation.
///
/// `maybe_dream` is the historical, never-wired-in stub entry point that was
/// flagged in the A-2 audit (2026-10-16). It used to log
/// `"gates satisfied, consolidation would run"` and return nothing. It now
/// returns an explicit outcome so callers (and tests) can detect the parked
/// state instead of being misled by a log line.
///
/// **Parking status:** the consolidation call itself is intentionally **not**
/// implemented in this crate. The real consolidation path lives in
/// `jcode-app-core::ambient::auto_dream::maybe_dream`, which uses a real
/// provider. See `docs/plans/MEMORY_GRAPH_PLAN.md` for the design context and
/// the AUDIT-2026-10-16 comment in `lib.rs` for the audit trail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamOutcome {
    /// Gates did not all pass; consolidation was not attempted.
    Skipped { reason: &'static str },
    /// Gates all passed but the consolidation LLM call is intentionally
    /// **not** made by this crate. The previous variant
    /// (`WouldConsolidate`) implied it would run; the renamed variant
    /// honestly states that this crate does not perform the call.
    ///
    /// The live caller (`jcode-app-core::ambient::auto_dream::maybe_dream`)
    /// does perform the call, but it does so directly without going through
    /// this function.
    GatesSatisfiedNoLlmCall,
}

/// Top-level evaluator that checks all three gates and decides whether
/// consolidation should run.
#[derive(Debug)]
pub struct DreamCoordinator {
    pub time_gate_open: bool,
    pub session_gate: SessionGate,
    pub lock: ConsolidationLock,
}

impl DreamCoordinator {
    /// Create a new coordinator with the default thresholds.
    pub fn new(data_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            time_gate_open: false,
            session_gate: SessionGate::new(DEFAULT_SESSION_GATE_COUNT),
            lock: ConsolidationLock::new(data_dir.into()),
        }
    }

    /// Test-only constructor that pre-opens all gates.
    ///
    /// Required because `SessionGate::is_satisfied()` is itself a stub
    /// (always returns `false` in production) and `ConsolidationLock`
    /// already returns `true` from `is_available()`. Without this hook,
    /// the "all gates open" branch of [`Self::run`] is unreachable in
    /// tests, so the parking contract (variant is `GatesSatisfiedNoLlmCall`
    /// and not `WouldConsolidate`) cannot be asserted from outside.
    #[cfg(test)]
    pub fn for_testing_all_gates_open(data_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            time_gate_open: true,
            session_gate: SessionGate::new(0),
            lock: ConsolidationLock::new(data_dir.into()),
        }
    }

    /// Evaluate all three gates. Returns `Ready` only if every gate passes.
    pub fn evaluate(&self) -> DreamTrigger {
        if !self.time_gate_open {
            return DreamTrigger::NotReady {
                reason: "time gate not yet satisfied",
            };
        }
        if !self.session_gate.is_satisfied() {
            return DreamTrigger::NotReady {
                reason: "session gate not yet satisfied",
            };
        }
        if !self.lock.is_available() {
            return DreamTrigger::NotReady {
                reason: "consolidation lock is held",
            };
        }
        DreamTrigger::Ready
    }
}

impl DreamCoordinator {
    /// Convenience wrapper around [`Self::evaluate`] that returns a
    /// [`DreamOutcome`].
    ///
    /// **This is a stub entry point.** It does not perform the
    /// consolidation LLM call. The live consolidation path lives in
    /// `jcode-app-core::ambient::auto_dream::maybe_dream` and uses a real
    /// provider.
    pub fn run(&self) -> DreamOutcome {
        match self.evaluate() {
            DreamTrigger::Ready => {
                // AUDIT-2026-10-16: parked pending Memory Graph Phase A design.
                // The previous log line ("gates satisfied, consolidation would run")
                // was a silent failure -- no consolidation actually happened. The
                // renamed variant `GatesSatisfiedNoLlmCall` makes the parked state
                // explicit so callers/tests cannot be misled by an "everything is
                // fine" log message.
                //
                // Audit owner: jcode / A-2 (stubs-mocks).
                // Design context: docs/plans/MEMORY_GRAPH_PLAN.md.
                tracing::warn!(
                    "auto-dream: gates satisfied but no LLM call is made by this \
                     crate; the live consolidation path is in \
                     jcode-app-core::ambient::auto_dream::maybe_dream \
                     (AUDIT-2026-10-16, parked pending Memory Graph Phase A)"
                );
                DreamOutcome::GatesSatisfiedNoLlmCall
            }
            DreamTrigger::NotReady { reason } => {
                tracing::debug!(reason, "auto-dream: gates not satisfied");
                DreamOutcome::Skipped { reason }
            }
        }
    }
}

/// Historical entry point preserved for API stability, **but unused by any
/// live caller**.
///
/// As of AUDIT-2026-10-16 this function no longer logs a misleading
/// "consolidation would run" message. It builds a [`DreamCoordinator`],
/// runs the gates via [`DreamCoordinator::run`], and returns the resulting
/// [`DreamOutcome`]. When gates pass it returns
/// [`DreamOutcome::GatesSatisfiedNoLlmCall`] and emits a `warn`-level log
/// that explicitly states the parked status.
///
/// Do not reintroduce silent behavior here. New callers should call
/// `jcode-app-core::ambient::auto_dream::maybe_dream` instead, which
/// actually performs the LLM call.
pub fn maybe_dream(data_dir: impl Into<std::path::PathBuf>, _total_cycles: u64) -> DreamOutcome {
    let coordinator = DreamCoordinator::new(data_dir);
    coordinator.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_not_ready_when_gates_unsatisfied() {
        let coord = DreamCoordinator::new("/tmp/auto-dream-test");
        let result = coord.evaluate();
        assert_ne!(result, DreamTrigger::Ready);
    }

    #[test]
    fn run_returns_skipped_when_gates_unsatisfied() {
        // Time gate starts closed -> Skipped, never GatesSatisfiedNoLlmCall.
        let coord = DreamCoordinator::new("/tmp/auto-dream-test-skipped");
        let outcome = coord.run();
        assert!(
            matches!(outcome, DreamOutcome::Skipped { .. }),
            "expected Skipped, got {:?}",
            outcome
        );
    }

    #[test]
    fn run_returns_gates_satisfied_no_llm_call_when_all_open() {
        // Force all three gates open. The renamed variant name is the
        // parking contract; this test makes sure we did not silently
        // regress to `WouldConsolidate`. The session gate is satisfied
        // via the test-only override hook because `SessionGate::is_satisfied`
        // is itself a stub (always `false` in production) and the parking
        // branch of `run` would otherwise be unreachable.
        let mut coord = DreamCoordinator::for_testing_all_gates_open("/tmp/auto-dream-test-ready");
        coord.session_gate.force_satisfied_for_tests();
        let outcome = coord.run();
        assert_eq!(
            outcome,
            DreamOutcome::GatesSatisfiedNoLlmCall,
            "AUDIT-2026-10-16: this crate must not silently perform the LLM call; \
             if this assertion fails, the parking contract has regressed"
        );
        // And the old variant must NOT exist; this would not compile if it did.
    }
}
