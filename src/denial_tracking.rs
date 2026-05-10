// ---------------------------------------------------------------------------//
// Denial Tracking System — Ported from Claude Code's denialTracking.ts      //
// ---------------------------------------------------------------------------//
//! Tracks consecutive/total permission denials to detect when an agent
//! is repeatedly requesting dangerous operations. When thresholds are
//! exceeded, the system automatically falls back to prompting the user
//! instead of silently denying (which could cause infinite retry loops).
//!
//! # Architecture (from Claude Code)
//!
//! ```text
//! Classifier blocks action
//!        │
//!   recordDenial()
//!        │
//!   consecutiveDenials++ / totalDenials++
//!        │
//!   shouldFallbackToPrompting()?
//!    ┌────┴────┐
//!    YES       NO
//!    │         │
//!  Prompt     Continue silent deny
//!  user
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

/// Default limits matching Claude Code's DENIAL_LIMITS
const DEFAULT_MAX_CONSECUTIVE: u32 = 3;
const DEFAULT_MAX_TOTAL: u32 = 20;

/// Thread-safe denial tracking state using atomic operations.
/// This avoids lock contention in hot paths (tool execution).
pub struct DenialTrackingState {
    /// Number of consecutive denials since last successful approval.
    consecutive: AtomicU32,
    /// Total denials in the current session/turn window.
    total: AtomicU32,
    /// Threshold for consecutive denials before fallback to prompting.
    max_consecutive: u32,
    /// Threshold for total denials before fallback to prompting.
    max_total: u32,
}

impl DenialTrackingState {
    /// Create a new denial tracking state with default limits.
    pub fn new() -> Self {
        Self {
            consecutive: AtomicU32::new(0),
            total: AtomicU32::new(0),
            max_consecutive: DEFAULT_MAX_CONSECUTIVE,
            max_total: DEFAULT_MAX_TOTAL,
        }
    }

    /// Create with custom limits (for testing or configuration override).
    pub fn with_limits(max_consecutive: u32, max_total: u32) -> Self {
        Self {
            consecutive: AtomicU32::new(0),
            total: AtomicU32::new(0),
            max_consecutive,
            max_total,
        }
    }

    /// Record a denial event. Returns the updated counts.
    ///
    /// Called when a classifier or rule denies a tool use.
    /// Increments both consecutive and total counters.
    pub fn record_denial(&self) -> (u32, u32) {
        let consec = self.consecutive.fetch_add(1, Ordering::SeqCst) + 1;
        let tot = self.total.fetch_add(1, Ordering::SeqCst) + 1;
        (consec, tot)
    }

    /// Record a successful approval (resets consecutive counter).
    ///
    /// Called when a tool is approved after some denials — this resets
    /// the "streak" counter because the user has explicitly allowed progress.
    pub fn record_success(&self) {
        // Only reset if non-zero to avoid unnecessary atomic store
        if self.consecutive.load(Ordering::SeqCst) != 0 {
            self.consecutive.store(0, Ordering::SeqCst);
        }
    }

    /// Check if we should fall back to prompting the user.
    ///
    /// Returns `true` if either threshold is exceeded:
    /// - Consecutive denials >= max_consecutive
    /// - Total denials >= max_total
    ///
    /// This prevents infinite loops where the agent keeps trying
    /// dangerous actions and getting silently denied.
    #[inline]
    pub fn should_fallback_to_prompting(&self) -> bool {
        let consec = self.consecutive.load(Ordering::SeqCst);
        let tot = self.total.load(Ordering::SeqCst);
        consec >= self.max_consecutive || tot >= self.max_total
    }

    /// Get current state for logging/diagnostics.
    pub fn state(&self) -> DenialSnapshot {
        DenialSnapshot {
            consecutive: self.consecutive.load(Ordering::SeqCst),
            total: self.total.load(Ordering::SeqCst),
            max_consecutive: self.max_consecutive,
            max_total: self.max_total,
        }
    }

    /// Reset all counters (called at start of new user request).
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

/// Enhanced decision metadata that includes denial tracking info.
#[derive(Debug, Clone)]
pub struct DecisionMetadata {
    pub tool_name: String,
    pub approved: bool,
    pub source: DecisionSource,
    /// Reason for denial (if denied). Used for YOLO classifier integration.
    #[allow(dead_code)]
    pub reason: Option<String>,
    /// Current denial snapshot at time of decision (for telemetry).
    #[allow(dead_code)]
    pub denial_snapshot: DenialSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// User explicitly approved via TUI or CLI flag
    UserExplicit,
    /// Auto-allowed by safety tier classification
    AutoAllowed,
    /// Denied by YOLO security classifier
    ClassifierDenied,
    /// Denied by permission rule (pattern match)
    RuleDenied,
    /// Fallback to user prompting after too many denials
    FallbackPrompt,
    /// Approved after fallback prompt
    FallbackApproved,
    /// Session/expired request
    Expired,
}

/// Result of a permission check that integrates with denial tracking.
#[derive(Debug, Clone)]
pub struct PermissionCheckResult {
    /// Whether the action is allowed
    pub allowed: bool,
    /// The source/mode of this decision
    pub source: DecisionSource,
    /// Human-readable explanation (shown to user on deny)
    pub message: Option<String>,
    /// Whether the system should fall back to interactive prompting
    pub should_prompt_fallback: bool,
}

impl PermissionCheckResult {
    /// Create an allow result.
    pub fn allowed(source: DecisionSource) -> Self {
        Self {
            allowed: true,
            source,
            message: None,
            should_prompt_fallback: false,
        }
    }

    /// Create a deny result with optional reason.
    pub fn denied(source: DecisionSource, reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            source,
            message: Some(reason.into()),
            should_prompt_fallback: false,
        }
    }

    /// Create a deny result that triggers fallback to prompting.
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

        // Record 2 denials — still under limit
        state.record_denial();
        state.record_denial();
        assert!(!state.should_fallback_to_prompting());

        // 3rd denial triggers fallback (max_consecutive=3)
        state.record_denial();
        assert!(state.should_fallback_to_prompting());
    }

    #[test]
    fn test_success_resets_consecutive() {
        let state = DenialTrackingState::new();

        // 2 denials
        state.record_denial();
        state.record_denial();

        // Success resets consecutive but NOT total
        state.record_success();
        assert!(!state.should_fallback_to_prompting());

        // Need 3 more consecutive to trigger again
        state.record_denial();
        state.record_denial();
        state.record_denial();
        assert!(state.should_fallback_to_prompting());
    }

    #[test]
    fn test_total_limit() {
        let state = DenialTrackingState::with_limits(100, 5);

        // Alternate deny/success to avoid consecutive trigger
        for i in 0..4 {
            state.record_denial();
            if i % 2 == 0 {
                state.record_success();
            }
        }
        assert!(!state.should_fallback_to_prompting());

        // 5th total denial triggers
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
