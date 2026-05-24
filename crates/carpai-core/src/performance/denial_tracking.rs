// ---------------------------------------------------------------------------//
// Denial Tracking System — Ported from Claude Code's denialTracking.ts      //
// ---------------------------------------------------------------------------//!
//! Tracks consecutive/total permission denials to detect when an agent
//! is repeatedly requesting dangerous operations. When thresholds are
//! exceeded, the system automatically falls back to prompting the user
//! instead of silently denying (which could cause infinite retry loops).

use std::sync::atomic::{AtomicU32, Ordering};

/// Default limits matching Claude Code's DENIAL_LIMITS
const DEFAULT_MAX_CONSECUTIVE: u32 = 3;
const DEFAULT_MAX_TOTAL: u32 = 20;

/// Thread-safe denial tracking state using atomic operations.
pub struct DenialTrackingState {
    consecutive: AtomicU32,
    total: AtomicU32,
    max_consecutive: u32,
    max_total: u32,
}

impl DenialTrackingState {
    pub fn new() -> Self {
        Self {
            consecutive: AtomicU32::new(0),
            total: AtomicU32::new(0),
            max_consecutive: DEFAULT_MAX_CONSECUTIVE,
            max_total: DEFAULT_MAX_TOTAL,
        }
    }

    pub fn with_limits(max_consecutive: u32, max_total: u32) -> Self {
        Self {
            consecutive: AtomicU32::new(0),
            total: AtomicU32::new(0),
            max_consecutive,
            max_total,
        }
    }

    pub fn record_denial(&self) -> (u32, u32) {
        let consec = self.consecutive.fetch_add(1, Ordering::SeqCst) + 1;
        let tot = self.total.fetch_add(1, Ordering::SeqCst) + 1;
        (consec, tot)
    }

    pub fn record_success(&self) {
        if self.consecutive.load(Ordering::SeqCst) != 0 {
            self.consecutive.store(0, Ordering::SeqCst);
        }
    }

    #[inline]
    pub fn should_fallback_to_prompting(&self) -> bool {
        let consec = self.consecutive.load(Ordering::SeqCst);
        let tot = self.total.load(Ordering::SeqCst);
        consec >= self.max_consecutive || tot >= self.max_total
    }

    pub fn state(&self) -> DenialSnapshot {
        DenialSnapshot {
            consecutive: self.consecutive.load(Ordering::SeqCst),
            total: self.total.load(Ordering::SeqCst),
            max_consecutive: self.max_consecutive,
            max_total: self.max_total,
        }
    }

    pub fn reset(&self) {
        self.consecutive.store(0, Ordering::SeqCst);
        self.total.store(0, Ordering::SeqCst);
    }
}

impl Default for DenialTrackingState {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of current denial tracking state for diagnostics/logging.
#[derive(Debug, Clone)]
pub struct DenialSnapshot {
    pub consecutive: u32,
    pub total: u32,
    pub max_consecutive: u32,
    pub max_total: u32,
}

impl std::fmt::Display for DenialSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "denials={}/{} (consec/total), limits={}/{}",
            self.consecutive, self.total, self.max_consecutive, self.max_total
        )
    }
}

// ---------------------------------------------------------------------------
// Integration helper: Permission Decision Logger
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DecisionMetadata {
    pub tool_name: String,
    pub approved: bool,
    pub source: DecisionSource,
    #[allow(dead_code)]
    pub reason: Option<String>,
    #[allow(dead_code)]
    pub denial_snapshot: DenialSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    UserExplicit,
    AutoAllowed,
    ClassifierDenied,
    RuleDenied,
    FallbackPrompt,
    FallbackApproved,
    Expired,
}

#[derive(Debug, Clone)]
pub struct PermissionCheckResult {
    pub allowed: bool,
    pub source: DecisionSource,
    pub message: Option<String>,
    pub should_prompt_fallback: bool,
}

impl PermissionCheckResult {
    pub fn allowed(source: DecisionSource) -> Self {
        Self {
            allowed: true,
            source,
            message: None,
            should_prompt_fallback: false,
        }
    }

    pub fn denied(source: DecisionSource, reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            source,
            message: Some(reason.into()),
            should_prompt_fallback: false,
        }
    }

    pub fn denied_with_fallback(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            source: DecisionSource::FallbackPrompt,
            message: Some(reason.into()),
            should_prompt_fallback: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_denial_tracking() {
        let state = DenialTrackingState::new();

        assert!(!state.should_fallback_to_prompting());

        state.record_denial();
        state.record_denial();
        assert!(!state.should_fallback_to_prompting());

        state.record_denial();
        assert!(state.should_fallback_to_prompting());
    }

    #[test]
    fn test_success_resets_consecutive() {
        let state = DenialTrackingState::new();

        state.record_denial();
        state.record_denial();

        state.record_success();
        assert!(!state.should_fallback_to_prompting());

        state.record_denial();
        state.record_denial();
        state.record_denial();
        assert!(state.should_fallback_to_prompting());
    }

    #[test]
    fn test_total_limit() {
        let state = DenialTrackingState::with_limits(100, 5);

        for i in 0..4 {
            state.record_denial();
            if i % 2 == 0 {
                state.record_success();
            }
        }
        assert!(!state.should_fallback_to_prompting());

        state.record_denial();
        assert!(state.should_fallback_to_prompting());
    }

    #[test]
    fn test_reset() {
        let state = DenialTrackingState::new();

        for _ in 0..10 {
            state.record_denial();
        }
        assert!(state.should_fallback_to_prompting());

        state.reset();
        assert!(!state.should_fallback_to_prompting());
        assert_eq!(state.state().consecutive, 0);
        assert_eq!(state.state().total, 0);
    }

    #[test]
    fn test_snapshot_display() {
        let state = DenialTrackingState::new();
        state.record_denial();
        let snap = state.state();
        let display = format!("{}", snap);
        assert!(display.contains("denials=1/1"));
    }
}
