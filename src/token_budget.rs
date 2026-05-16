//! Token Budget Auto-Continue System
//!
//! Ported from Claude Code's `query/tokenBudget.ts` (v2.1.88).
//!
//! Provides automatic task continuation for long-running agentic tasks.
//! When a token budget is configured, the system tracks cumulative token usage
//! across turns and automatically injects "continue" nudges when the budget is
//! not yet exhausted, with diminishing-returns detection to stop when progress stalls.
//!
//! ## Design Principles
//!
//! - **Budget-aware**: Tracks total tokens used across all turns in a single user request
//! - **Diminishing returns detection**: Stops continuing when token output per turn drops below threshold
//! - **Completion threshold**: Only continues when under 90% of budget (configurable)
//! - **Non-invasive**: Zero overhead when no budget is configured
//!
//! ## Comparison with Claude Code Original
//!
//! | Feature | Claude Code | JCode |
//!---------|------------|-------|
//! | Budget source | API task_budget param | Config file + env override |
//! | Diminishing threshold | 500 tokens | 500 tokens (same) |
//! | Completion threshold | 90% | 90% (same) |
//! | Nudge message | "Please continue..." | Configurable via i18n |
//! | Telemetry | growthbook events | telemetry::record_* |

use std::time::Instant;

/// Completion threshold - continue if usage is below this percentage of budget.
/// Matches Claude Code's COMPLETION_THRESHOLD = 0.9
const COMPLETION_THRESHOLD: f64 = 0.9;

/// Minimum delta tokens between checks to consider "productive".
/// If both last_delta and current_delta are below this for 3+ continuations,
/// we detect diminishing returns and stop.
/// Matches Claude Code's DIMINISHING_THRESHOLD = 500
const DIMINISHING_THRESHOLD: u64 = 500;

/// Default token budget when enabled but not explicitly configured.
/// ~500K tokens matches Claude Code's default auto-continue ceiling.
pub const DEFAULT_BUDGET_TOKENS: u64 = 500_000;

/// Maximum number of automatic continuations before forcing a stop.
/// Prevents infinite loops in degenerate cases.
pub const MAX_AUTO_CONTINUATIONS: u32 = 25;

/// Result of checking whether to continue or stop.
#[derive(Debug, Clone)]
pub enum BudgetDecision {
    /// Continue with a nudge message injected into the conversation.
    Continue {
        /// Human-readable nudge message to prepend.
        nudge_message: String,
        /// Number of continuations so far (for logging/telemetry).
        continuation_count: u32,
        /// Current budget utilization as percentage (0-100).
        pct: u8,
        /// Tokens used in the most recent turn.
        turn_tokens: u64,
        /// Total budget for this task.
        budget: u64,
    },
    /// Stop the auto-continue loop.
    Stop {
        /// Some(completion_event) if we stopped after at least one continuation;
        /// None if we never started (no budget, sub-agent, etc.)
        completion_event: Option<CompletionEvent>,
    },
}

/// Recorded when auto-continue stops, for telemetry and logging.
#[derive(Debug, Clone)]
pub struct CompletionEvent {
    pub continuation_count: u32,
    pub pct: u8,
    pub turn_tokens: u64,
    pub budget: u64,
    /// True if stopped because output per turn was too low (stalled).
    pub diminishing_returns: bool,
    /// Wall-clock duration of the entire auto-continued task.
    pub duration_ms: u64,
}

/// Tracker state for a single user-request / agentic loop.
///
/// Created at the start of `run_turn()`, checked after each API response.
#[derive(Debug, Clone)]
pub struct TokenBudgetTracker {
    /// Number of times we've continued so far.
    continuation_count: u32,
    /// Token delta from the check before last (for diminishing detection).
    last_delta_tokens: u64,
    /// Cumulative global tokens seen at last check.
    last_global_turn_tokens: u64,
    /// When this tracker was created (for duration tracking).
    started_at: Instant,
    /// The configured budget (tokens). None = disabled.
    budget: Option<u64>,
}

impl Default for TokenBudgetTracker {
    fn default() -> Self {
        Self::new(None)
    }
}

impl TokenBudgetTracker {
    /// Create a new tracker with the given budget (None = disabled).
    pub fn new(budget: Option<u64>) -> Self {
        Self {
            continuation_count: 0,
            last_delta_tokens: 0,
            last_global_turn_tokens: 0,
            started_at: Instant::now(),
            budget,
        }
    }

    /// Create a tracker from environment/config.
    ///
    /// Checks `JCODE_TOKEN_BUDGET` env var, then falls back to config file.
    pub fn from_config() -> Self {
        let budget = std::env::var("JCODE_TOKEN_BUDGET")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0);

        if budget.is_none() {
            // Check config file (token_budget config not yet implemented)
            let _cfg = crate::config::config();
            // if let Some(cfg_budget) = _cfg.agent.token_budget {
            //     if cfg_budget > 0 {
            //         return Self::new(Some(cfg_budget));
            //     }
            // }
        }

        Self::new(budget)
    }

    /// Returns true if auto-continue is enabled (budget is set).
    pub fn is_enabled(&self) -> bool {
        self.budget.is_some() && self.budget.unwrap_or(0) > 0
    }

    /// Check whether to continue or stop based on current token usage.
    ///
    /// # Arguments
    /// * `global_turn_tokens` - total input+output tokens used in the most recent turn
    ///
    /// # Returns
    /// A `BudgetDecision` indicating whether to continue with a nudge message or stop.
    pub fn check(&mut self, global_turn_tokens: u64) -> BudgetDecision {
        let budget = match self.budget {
            Some(b) if b > 0 => b,
            _ => return BudgetDecision::Stop { completion_event: None },
        };

        // If we've hit max continuations, force stop
        if self.continuation_count >= MAX_AUTO_CONTINUATIONS {
            return BudgetDecision::Stop {
                completion_event: Some(CompletionEvent {
                    continuation_count: self.continuation_count,
                    pct: 100,
                    turn_tokens: global_turn_tokens,
                    budget,
                    diminishing_returns: false,
                    duration_ms: self.started_at.elapsed().as_millis() as u64,
                }),
            };
        }

        let pct = ((global_turn_tokens as f64 / budget as f64) * 100.0).min(100.0) as u8;
        let delta_since_last = global_turn_tokens.saturating_sub(self.last_global_turn_tokens);

        // Detect diminishing returns: low delta for 3+ consecutive continuations
        let is_diminishing = self.continuation_count >= 3
            && delta_since_last < DIMINISHING_THRESHOLD
            && self.last_delta_tokens < DIMINISHING_THRESHOLD;

        if !is_diminishing && global_turn_tokens < (budget as f64 * COMPLETION_THRESHOLD) as u64 {
            // Continue - under threshold and still productive
            self.continuation_count += 1;
            self.last_delta_tokens = delta_since_last;
            self.last_global_turn_tokens = global_turn_tokens;

            BudgetDecision::Continue {
                nudge_message: build_continuation_nudge(pct, global_turn_tokens, budget),
                continuation_count: self.continuation_count,
                pct,
                turn_tokens: global_turn_tokens,
                budget,
            }
        } else {
            // Stop - either over threshold or diminishing returns
            let has_continuations = self.continuation_count > 0 || is_diminishing;
            BudgetDecision::Stop {
                completion_event: if has_continuations {
                    Some(CompletionEvent {
                        continuation_count: self.continuation_count,
                        pct,
                        turn_tokens: global_turn_tokens,
                        budget,
                        diminishing_returns: is_diminishing,
                        duration_ms: self.started_at.elapsed().as_millis() as u64,
                    })
                } else {
                    None
                },
            }
        }
    }

    /// Reset the tracker (e.g., when starting a new user request).
    pub fn reset(&mut self) {
        self.continuation_count = 0;
        self.last_delta_tokens = 0;
        self.last_global_turn_tokens = 0;
        self.started_at = Instant::now();
    }

    /// Get the current continuation count (for display/logging).
    pub fn continuation_count(&self) -> u32 {
        self.continuation_count
    }
}

/// Build the nudge message that gets injected when auto-continuing.
fn build_continuation_nudge(pct: u8, turn_tokens: u64, budget: u64) -> String {
    format!(
        "[auto-continue] Task budget at {}% ({} / {} tokens). Please continue where you left off — do not restart or summarize.",
        pct, turn_tokens, budget
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_tracker_always_stops() {
        let mut tracker = TokenBudgetTracker::new(None);
        match tracker.check(1000) {
            BudgetDecision::Stop { completion_event: None } => {}
            other => panic!("Expected Stop(None), got {:?}", other),
        }
    }

    #[test]
    fn test_continue_under_threshold() {
        let mut tracker = TokenBudgetTracker::new(Some(10_000));
        // 50% of budget
        match tracker.check(5_000) {
            BudgetDecision::Continue { pct, .. } => assert_eq!(pct, 50),
            other => panic!("Expected Continue, got {:?}", other),
        }
    }

    #[test]
    fn test_stop_at_threshold() {
        let mut tracker = TokenBudgetTracker::new(Some(10_000));
        // 95% of budget - above 90% threshold
        match tracker.check(9_500) {
            BudgetDecision::Stop { .. } => {}
            other => panic!("Expected Stop, got {:?}", other),
        }
    }

    #[test]
    fn test_diminishing_returns_detection() {
        let mut tracker = TokenBudgetTracker::new(Some(100_000));

        // Turn 1: 30k tokens (under 90%, should continue)
        assert!(matches!(tracker.check(30_000), BudgetDecision::Continue { .. }));

        // Turn 2: 30.1k tokens (delta=100, small but <3 continuations)
        assert!(matches!(tracker.check(30_100), BudgetDecision::Continue { .. }));

        // Turn 3: 30.15k tokens (delta=50, now at 3 continuations + small delta)
        assert!(matches!(tracker.check(30_150), BudgetDecision::Continue { .. }));

        // Turn 4: 30.16k tokens (delta=10, 4 continuations + very small delta -> diminishing)
        match tracker.check(30_160) {
            BudgetDecision::Stop { completion_event: Some(ev) } => {
                assert!(ev.diminishing_returns);
            }
            other => panic!("Expected Stop(diminishing), got {:?}", other),
        }
    }

    #[test]
    fn test_max_continuations_limit() {
        let mut tracker = TokenBudgetTracker::new(Some(1_000_000));

        // Simulate MAX_AUTO_CONTINUATIONS continues
        for i in 0..MAX_AUTO_CONTINUATIONS {
            match tracker.check(1000) {
                BudgetDecision::Continue { continuation_count, .. } => {
                    assert_eq!(continuation_count, i + 1);
                }
                other @ BudgetDecision::Stop { .. } => {
                    if i < MAX_AUTO_CONTINUATIONS {
                        panic!("Unexpected stop at continuation {}", i + 1);
                    }
                    // Expected stop at limit
                    return;
                }
            }
        }

        // Next check should force stop
        match tracker.check(1000) {
            BudgetDecision::Stop { completion_event: Some(ev) } => {
                assert_eq!(ev.continuation_count, MAX_AUTO_CONTINUATIONS);
            }
            other => panic!("Expected forced stop at limit, got {:?}", other),
        }
    }

    #[test]
    fn test_reset() {
        let mut tracker = TokenBudgetTracker::new(Some(10_000));
        tracker.check(5_000); // continuation_count = 1
        tracker.check(6_000); // continuation_count = 2
        assert_eq!(tracker.continuation_count(), 2);

        tracker.reset();
        assert_eq!(tracker.continuation_count(), 0);
        // Should behave like fresh tracker
        assert!(matches!(tracker.check(5_000), BudgetDecision::Continue { continuation_count: 1, .. }));
    }
}
