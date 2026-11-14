//! Agent lifecycle state for HERDR reporting.
//!
//! HERDR classifies each agent pane as one of:
//! - **Working** — agent is actively processing
//! - **Idle** — agent is ready for input
//! - **Blocked** — agent needs user decision (permission prompt, question)
//!
//! State transitions drive HERDR's sidebar rollup, wait notifications,
//! and `agent wait` completion.

use std::fmt;

/// Semantic agent state reported to HERDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentState {
    /// Agent is actively processing (streaming, running tools, etc.).
    Working,
    /// Agent is ready for user input.
    Idle,
    /// Agent needs user decision (permission prompt, approval, question).
    Blocked,
}

impl AgentState {
    /// String representation for HERDR's `pane.report_agent` API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Blocked => "blocked",
        }
    }
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_strings() {
        assert_eq!(AgentState::Working.as_str(), "working");
        assert_eq!(AgentState::Idle.as_str(), "idle");
        assert_eq!(AgentState::Blocked.as_str(), "blocked");
    }

    #[test]
    fn state_display() {
        assert_eq!(format!("{}", AgentState::Working), "working");
    }
}
