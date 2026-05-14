//! Plan Mode — Agent enters a read-only planning phase before executing.
//!
//! Architecture:
//! - `PlanModeState` tracks whether the agent is in plan mode.
//! - `EnterPlanModeTool` / `ExitPlanModeTool` toggle the state.
//! - The Agent's `build_prompt()` injects plan-mode system prompt when active.
//! - Tools are filtered to read-only when in plan mode.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Global plan-mode flag. When true, the agent outputs plans but does NOT
/// write or edit any files — it's a read-only exploration phase.
static PLAN_MODE: AtomicBool = AtomicBool::new(false);

/// Saved pre-plan permission mode for restoration on exit.
static PRE_PLAN_MODE: Mutex<Option<String>> = Mutex::new(None);

/// Check whether the agent is currently in plan mode.
pub fn is_plan_mode() -> bool {
    PLAN_MODE.load(Ordering::Relaxed)
}

/// Enter plan mode: save current mode and activate.
pub fn enter_plan_mode(current_mode: Option<&str>) {
    if let Ok(mut saved) = PRE_PLAN_MODE.lock() {
        *saved = current_mode.map(|s| s.to_string());
    }
    PLAN_MODE.store(true, Ordering::Relaxed);
}

/// Exit plan mode: restore previous mode.
pub fn exit_plan_mode() -> Option<String> {
    PLAN_MODE.store(false, Ordering::Relaxed);
    PRE_PLAN_MODE.lock().ok().and_then(|mut m| m.take())
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
5. **Ask clarifying questions** using AskUserQuestion if something is unclear.
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
