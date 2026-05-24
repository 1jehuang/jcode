//! Token Budget Auto-Continue System
//!
//! Ported from Claude Code's `query/tokenBudget.ts` (v2.1.88).
//!
//! Provides automatic task continuation for long-running agentic tasks.
//! When a token budget is configured, the system tracks cumulative token usage
//! across turns and automatically injects "continue" nudges when the budget is
//! not yet exhausted, with diminishing-returns detection to stop when progress stalls.

use std::time::Instant;

/// Completion threshold - continue if usage is below this percentage of budget.
/// Matches Claude Code's COMPLETION_THRESHOLD = 0.9
const COMPLETION_THRESHOLD: f64 = 0.9;

/// Minimum delta tokens between checks to consider "productive".
const DIMINISHING_THRESHOLD: u64 = 500;

/// Default token budget when enabled but not explicitly configured.
pub const DEFAULT_BUDGET_TOKENS: u64 = 500_000;

/// Maximum number of automatic continuations before forcing a stop.
pub const MAX_AUTO_CONTINUATIONS: u32 = 25;

/// Result of checking whether to continue or stop.
#[derive(Debug, Clone)]
pub enum BudgetDecision {
    Continue {
        nudge_message: String,
        continuation_count: u32,
        pct: u8,
        turn_tokens: u64,
        budget: u64,
    },
    Stop {
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
    pub diminishing_returns: bool,
    pub duration_ms: u64,
}

/// Tracker state for a single user-request / agentic loop.
#[derive(Debug, Clone)]
pub struct TokenBudgetTracker {
    continuation_count: u32,
    last_delta_tokens: u64,
    last_global_turn_tokens: u64,
    started_at: Instant,
    budget: Option<u64>,
}

impl Default for TokenBudgetTracker {
    fn default() -> Self {
        Self::new(None)
    }
}

impl TokenBudgetTracker {
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
    /// Checks `JCODE_TOKEN_BUDGET` env var for configuration.
    pub fn from_config() -> Self {
        let budget = std::env::var("JCODE_TOKEN_BUDGET")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0);

        Self::new(budget)
    }

    pub fn is_enabled(&self) -> bool {
        self.budget.is_some() && self.budget.unwrap_or(0) > 0
    }

    pub fn check(&mut self, global_turn_tokens: u64) -> BudgetDecision {
        let budget = match self.budget {
            Some(b) if b > 0 => b,
            _ => return BudgetDecision::Stop { completion_event: None },
        };

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

        let is_diminishing = self.continuation_count >= 3
            && delta_since_last < DIMINISHING_THRESHOLD
            && self.last_delta_tokens < DIMINISHING_THRESHOLD;

        if !is_diminishing && global_turn_tokens < (budget as f64 * COMPLETION_THRESHOLD) as u64 {
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

    pub fn reset(&mut self) {
        self.continuation_count = 0;
        self.last_delta_tokens = 0;
        self.last_global_turn_tokens = 0;
        self.started_at = Instant::now();
    }

    pub fn continuation_count(&self) -> u32 {
        self.continuation_count
    }
}

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
        match tracker.check(5_000) {
            BudgetDecision::Continue { pct, .. } => assert_eq!(pct, 50),
            other => panic!("Expected Continue, got {:?}", other),
        }
    }

    #[test]
    fn test_stop_at_threshold() {
        let mut tracker = TokenBudgetTracker::new(Some(10_000));
        match tracker.check(9_500) {
            BudgetDecision::Stop { .. } => {}
            other => panic!("Expected Stop, got {:?}", other),
        }
    }

    #[test]
    fn test_diminishing_returns_detection() {
        let mut tracker = TokenBudgetTracker::new(Some(100_000));

        assert!(matches!(tracker.check(30_000), BudgetDecision::Continue { .. }));
        assert!(matches!(tracker.check(30_100), BudgetDecision::Continue { .. }));
        assert!(matches!(tracker.check(30_150), BudgetDecision::Continue { .. }));

        match tracker.check(30_160) {
            BudgetDecision::Stop { completion_event: Some(ev) } => {
                assert!(ev.diminishing_returns);
            }
            other => panic!("Expected Stop(diminishing), got {:?}", other),
        }
    }

    #[test]
    fn test_max_continations_limit() {
        let mut tracker = TokenBudgetTracker::new(Some(1_000_000));

        for i in 0..MAX_AUTO_CONTINUATIONS {
            match tracker.check(1000) {
                BudgetDecision::Continue { continuation_count, .. } => {
                    assert_eq!(continuation_count, i + 1);
                }
                other @ BudgetDecision::Stop { .. } => {
                    if i < MAX_AUTO_CONTINUATIONS {
                        panic!("Unexpected stop at continuation {}", i + 1);
                    }
                    return;
                }
            }
        }

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
        tracker.check(5_000);
        tracker.check(6_000);
        assert_eq!(tracker.continuation_count(), 2);

        tracker.reset();
        assert_eq!(tracker.continuation_count(), 0);
        assert!(matches!(tracker.check(5_000), BudgetDecision::Continue { continuation_count: 1, .. }));
    }
}
