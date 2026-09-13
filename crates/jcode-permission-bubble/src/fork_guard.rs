//! Recursive fork detection and depth limiting.
//!
//! A parent spawns a child. That child must not spawn its own children (a
//! grandchild), and so on. This module enforces a maximum fork depth to prevent
//! runaway recursion that would burn tokens and create confusing permission
//! chains.

use serde::{Deserialize, Serialize};

/// Default maximum fork depth (parent = depth 0, child = depth 1).
pub const MAX_FORK_DEPTH: usize = 1;

/// The result of checking whether a fork is allowed at a given depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkDepthResult {
    /// Fork is allowed at the given depth.
    Allowed { current_depth: usize },
    /// Fork would exceed the maximum depth and is denied.
    Denied {
        current_depth: usize,
        max_depth: usize,
    },
}

/// Tracks the current fork depth and decides whether another fork is permitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkDepthGuard {
    /// Current depth (0 = root parent).
    depth: usize,
    /// Maximum allowed depth.
    max_depth: usize,
    /// Chain of fork IDs from root to current (for auditing).
    fork_chain: Vec<String>,
}

impl ForkDepthGuard {
    /// Create a guard for the root parent (depth 0).
    pub fn root() -> Self {
        Self {
            depth: 0,
            max_depth: MAX_FORK_DEPTH,
            fork_chain: Vec::new(),
        }
    }

    /// Create a guard with a custom max depth.
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            depth: 0,
            max_depth,
            fork_chain: Vec::new(),
        }
    }

    /// Advance the guard by one fork level, recording a fork ID.
    pub fn advance(&mut self, fork_id: impl Into<String>) -> ForkDepthResult {
        self.fork_chain.push(fork_id.into());
        let new_depth = self.depth + 1;

        if new_depth > self.max_depth {
            return ForkDepthResult::Denied {
                current_depth: new_depth,
                max_depth: self.max_depth,
            };
        }

        self.depth = new_depth;
        ForkDepthResult::Allowed {
            current_depth: new_depth,
        }
    }

    /// Check whether spawning a child from the current depth is permitted,
    /// without mutating state.
    pub fn can_fork(&self) -> ForkDepthResult {
        let would_be = self.depth + 1;
        if would_be > self.max_depth {
            ForkDepthResult::Denied {
                current_depth: would_be,
                max_depth: self.max_depth,
            }
        } else {
            ForkDepthResult::Allowed {
                current_depth: would_be,
            }
        }
    }

    /// Current depth.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Read-only access to the fork chain.
    pub fn fork_chain(&self) -> &[String] {
        &self.fork_chain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_can_fork() {
        let guard = ForkDepthGuard::root();
        assert_eq!(guard.can_fork(), ForkDepthResult::Allowed { current_depth: 1 });
    }

    #[test]
    fn default_depth_limits_to_one_level() {
        let mut guard = ForkDepthGuard::root();
        let r1 = guard.advance("fork-1");
        assert_eq!(r1, ForkDepthResult::Allowed { current_depth: 1 });

        let r2 = guard.advance("fork-2");
        assert!(matches!(r2, ForkDepthResult::Denied { .. }));
    }

    #[test]
    fn custom_depth_allows_two_levels() {
        let mut guard = ForkDepthGuard::with_max_depth(2);
        let _r1 = guard.advance("fork-1");
        let r2 = guard.advance("fork-2");
        assert_eq!(r2, ForkDepthResult::Allowed { current_depth: 2 });

        let r3 = guard.advance("fork-3");
        assert!(matches!(r3, ForkDepthResult::Denied { .. }));
    }

    #[test]
    fn fork_chain_recorded() {
        let mut guard = ForkDepthGuard::root();
        guard.advance("a");
        guard.advance("b");
        assert_eq!(guard.fork_chain(), &["a", "b"]);
    }
}
