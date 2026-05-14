//! Plan Mode tools — allow the AI Agent to enter/exit "plan-first" mode.
//!
//! In plan mode the agent outputs a structured plan before executing tools.
//! A global atomic flag tracks the current mode.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// Global plan-mode flag. When `true` the agent should plan before executing.
static PLAN_MODE: AtomicBool = AtomicBool::new(false);

/// Check whether the agent is currently in plan mode.
pub fn is_plan_mode() -> bool {
    PLAN_MODE.load(Ordering::Relaxed)
}

/// Set plan mode on or off.
pub fn set_plan_mode(enabled: bool) {
    PLAN_MODE.store(enabled, Ordering::Relaxed);
}

/// Force the agent to create a plan before taking further action.
pub struct EnterPlanModeTool;

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str {
        "enter_plan_mode"
    }

    fn description(&self) -> &str {
        r#"Switch to plan-first mode. In this mode the agent will first produce a
detailed plan describing every step before executing any tool. Use this for
complex multi-step tasks where you want to review the approach first.

Call this at the START of a complex task. The agent will respond with a plan
and wait for your approval before proceeding.

To leave plan mode call `exit_plan_mode`.

Parameters:
- goal (optional): A description of what you want to accomplish, so the plan can be tailored."#
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "Optional goal description to focus the planning."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let goal = input
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("(unspecified)");

        set_plan_mode(true);

        let plan = format!(
            r#"## Plan Mode — Active

**Goal:** {}

I will now analyze the task and create a step-by-step plan before making any changes.

### Analysis

1. Understanding requirements
2. Identifying files to modify
3. Determining dependencies
4. Creating the implementation plan

### Next Step

Review the analysis above. When you're ready to proceed, I will break down each step.

> Use `/review` after implementation, or call `exit_plan_mode` to leave plan mode.
"#,
            goal
        );

        Ok(ToolOutput {
            output: plan,
            title: Some("📋 Plan Mode".into()),
            metadata: None,
            images: Vec::new(),
        })
    }
}

/// Exit plan mode — return to normal tool-execution behavior.
pub struct ExitPlanModeTool;

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        "exit_plan_mode"
    }

    fn description(&self) -> &str {
        r#"Exit plan-first mode and return to normal tool execution.
Call this once you have reviewed and approved the plan, or if you no longer
need plan-first behavior."#
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        set_plan_mode(false);

        Ok(ToolOutput {
            output: "✅ Plan mode deactivated. Proceeding with normal tool execution.\n\n"
                .to_string(),
            title: Some("✅ Exiting Plan Mode".into()),
            metadata: None,
            images: Vec::new(),
        })
    }
}
