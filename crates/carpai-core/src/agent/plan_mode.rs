//! Plan Mode - Agent enters a read-only planning phase before executing.
//!
//! Architecture:
//! - `PlanModeState` tracks whether the agent is in plan mode.
//! - The Agent's prompt builder injects plan-mode system prompt when active.
//! - Tools are filtered to read-only when in plan mode.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// Global plan-mode flag. When true, the agent outputs plans but does NOT
/// write or edit any files — it's a read-only exploration phase.
static PLAN_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Check whether the agent is currently in plan mode.
pub fn is_plan_mode() -> bool {
    PLAN_MODE_ACTIVE.load(Ordering::Relaxed)
}

/// Enter plan mode.
pub fn enter_plan_mode() {
    PLAN_MODE_ACTIVE.store(true, Ordering::Relaxed);
}

/// Exit plan mode.
pub fn exit_plan_mode() {
    PLAN_MODE_ACTIVE.store(false, Ordering::Relaxed);
}

/// The system prompt fragment injected when in plan mode.
/// Mirrors Claude Code's plan mode instructions.
pub const PLAN_MODE_SYSTEM_PROMPT: &str = r#"
## Plan Mode

You are in PLAN MODE. Follow these rules strictly:

1. **Read-only exploration** — DO NOT write, edit, or create any files.
2. **Thoroughly explore** the codebase to understand existing patterns, architectures, and conventions.
3. **Identify** similar features and approaches already present.
4. **Consider** multiple approaches and their trade-offs.
5. **Ask clarifying questions** if something is unclear.
6. **Design** a concrete implementation strategy with specific file paths and changes.
7. When you have a complete plan ready, call **exit_plan_mode** to present your plan for approval.

Remember: No code changes until plan mode is exited.
"#;

/// A single step in a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: u32,
    pub description: String,
    pub status: StepStatus,
    pub file_paths: Vec<String>,
}

/// Status of a plan step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Approved,
    Rejected,
    Completed,
    Skipped,
}

impl std::fmt::Display for StepStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            Self::Pending => "⏳ Pending",
            Self::Approved => "✅ Approved",
            Self::Rejected => "❌ Rejected",
            Self::Completed => "✅ Completed",
            Self::Skipped => "⏭️ Skipped",
        })
    }
}

/// A complete plan produced by the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub goal: String,
    pub steps: Vec<PlanStep>,
    pub created_at: String,
}

impl Plan {
    pub fn new(goal: &str) -> Self {
        Self {
            goal: goal.to_string(),
            steps: Vec::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut md = format!("# Plan: {}\n\n**Created:** {}\n\n## Steps\n\n", self.goal, self.created_at);
        for step in &self.steps {
            md.push_str(&format!("- [{}] **{}:** {}\n", match step.status {
                StepStatus::Completed => "x",
                _ => " ",
            }, step.id, step.description));
            if !step.file_paths.is_empty() {
                md.push_str(&format!("  - Files: {}\n", step.file_paths.join(", ")));
            }
        }
        md
    }
}

/// Plan mode state tracker (for session persistence)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanModeState {
    pub is_active: bool,
    pub entered_at: Option<String>,
    pub current_goal: Option<String>,
}

impl Default for PlanModeState {
    fn default() -> Self {
        Self {
            is_active: false,
            entered_at: None,
            current_goal: None,
        }
    }
}

impl PlanModeState {
    pub fn activate(&mut self, goal: Option<String>) {
        self.is_active = true;
        self.entered_at = Some(chrono::Utc::now().to_rfc3339());
        self.current_goal = goal;
        enter_plan_mode();
    }

    pub fn deactivate(&mut self) {
        self.is_active = false;
        self.current_goal = None;
        exit_plan_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_mode_toggle() {
        assert!(!is_plan_mode());
        enter_plan_mode();
        assert!(is_plan_mode());
        exit_plan_mode();
        assert!(!is_plan_mode());
    }

    #[test]
    fn test_plan_creation() {
        let plan = Plan::new("Test goal");
        assert_eq!(plan.goal, "Test goal");
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn test_plan_to_markdown() {
        let mut plan = Plan::new("Add feature X");
        plan.steps.push(PlanStep {
            id: 1,
            description: "Implement core logic".to_string(),
            status: StepStatus::Pending,
            file_paths: vec!["src/lib.rs".to_string()],
        });
        let md = plan.to_markdown();
        assert!(md.contains("Add feature X"));
        assert!(md.contains("Implement core logic"));
    }
}
