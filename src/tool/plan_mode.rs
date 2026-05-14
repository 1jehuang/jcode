//! Plan Mode tools — allow the AI Agent to enter/exit "plan-first" mode.
//!
//! When in plan mode:
//! - The system prompt includes read-only instructions
//! - The agent explores the codebase but does NOT write files
//! - On exit, the agent presents a structured plan for review

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

pub struct EnterPlanModeTool;

impl EnterPlanModeTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str { "enter_plan_mode" }

    fn description(&self) -> &str {
        r#"Switch to plan-first mode. In this mode the agent will first produce a
detailed plan describing every step before executing any tool.

The agent will explore the codebase thoroughly (read-only) and produce a plan.
Call exit_plan_mode when ready to present the plan for approval.

Parameters:
- goal (optional): A description of what you want to accomplish."#
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
        let goal = input.get("goal").and_then(|v| v.as_str()).unwrap_or("(unspecified)");

        // Activate plan mode
        crate::plan_mode::enter_plan_mode(None);

        let plan = format!(
            r#"## Plan Mode — Active

**Goal:** {}

I am now in **read-only plan mode**. I will explore the codebase, understand the
architecture, and create a step-by-step plan before making any changes.

### My approach:

1. **Explore** — I'll read relevant files to understand the current code.
2. **Analyze** — I'll identify what needs to change and how.
3. **Design** — I'll create a structured implementation plan.
4. **Present** — When ready, call `exit_plan_mode` to show my plan for approval.

> **Note:** I will NOT write or edit any files while in plan mode.
> Use `/review` after implementation, or call `exit_plan_mode` to leave plan mode.
"#, goal);

        Ok(ToolOutput {
            output: plan,
            title: Some("📋 Plan Mode".into()),
            metadata: None,
            images: Vec::new(),
        })
    }
}

pub struct ExitPlanModeTool;

impl ExitPlanModeTool {
    pub fn new() -> Self { Self }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str { "exit_plan_mode" }

    fn description(&self) -> &str {
        r#"Exit plan-first mode and present the plan for approval.
Call this once you have explored the codebase and have a concrete plan ready.

The plan will include specific file paths, changes needed, and the implementation order."#
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Summary of what was learned and the recommended approach."
                },
                "steps": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Ordered list of implementation steps."
                },
                "files_to_modify": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of files that need to be modified."
                }
            },
            "required": ["summary", "steps", "files_to_modify"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let summary = input.get("summary").and_then(|v| v.as_str()).unwrap_or("Plan completed");
        let steps: Vec<String> = input.get("steps")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let files: Vec<String> = input.get("files_to_modify")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default();

        // Deactivate plan mode
        let _prev = crate::plan_mode::exit_plan_mode();

        let mut output = String::from("## ✅ Plan Complete\n\n");
        output.push_str(&format!("**Summary:** {}\n\n", summary));

        if !steps.is_empty() {
            output.push_str("### Implementation Steps\n\n");
            for (i, step) in steps.iter().enumerate() {
                output.push_str(&format!("{}. {}\n", i + 1, step));
            }
            output.push('\n');
        }

        if !files.is_empty() {
            output.push_str("### Files to Modify\n\n");
            for f in &files {
                output.push_str(&format!("- `{}`\n", f));
            }
            output.push('\n');
        }

        output.push_str("Plan mode deactivated. Proceeding with implementation.\n");

        Ok(ToolOutput {
            output,
            title: Some("✅ Plan Ready".into()),
            metadata: None,
            images: Vec::new(),
        })
    }
}
