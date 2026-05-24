//! MCP Tool Orchestration Engine
//!
//! Provides workflow definition and execution for chaining multiple MCP tools.
//! Supports sequential, parallel, and conditional execution modes.
//!
//! ## Workflow Definition (YAML)
//! ```yaml
//! name: "GitHub PR to Slack Notification"
//! steps:
//!   - tool: github.get_pull_request
//!     params:
//!       repo: "owner/repo"
//!       pr_number: "{{input.pr_number}}"
//!     output: pr_data
//!
//!   - tool: jira.create_issue
//!     condition: "{{pr_data.labels | contains('bug')}}"
//!     params:
//!       project: "PROJ"
//!       summary: "Bug from PR {{pr_data.number}}"
//!     output: jira_issue
//!
//!   - tool: slack.send_message
//!     params:
//!       channel: "#dev-notifications"
//!       text: "PR #{{pr_data.number}} merged! {{jira_issue.key if jira_issue else ''}}"
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, error, warn};

/// Workflow execution mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Execute steps sequentially
    Sequential,
    /// Execute steps in parallel (where possible)
    Parallel,
    /// Execute with conditional branching
    Conditional,
}

/// A single step in a workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Unique step identifier
    pub id: String,
    /// MCP tool name to invoke (e.g., "github.list_pull_requests")
    pub tool: String,
    /// Tool parameters (supports template variables like {{input.var}})
    pub params: Option<HashMap<String, serde_json::Value>>,
    /// Condition for executing this step (template expression)
    pub condition: Option<String>,
    /// Variable name to store the result
    pub output: Option<String>,
    /// Error handling strategy
    pub on_error: Option<OnErrorStrategy>,
}

/// Error handling strategy for a step
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnErrorStrategy {
    /// Stop workflow execution
    Fail,
    /// Continue to next step
    Continue,
    /// Retry N times
    Retry { max_attempts: u32 },
}

/// Complete workflow definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Workflow name
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// Execution mode
    pub mode: ExecutionMode,
    /// Input variable definitions
    pub inputs: Option<Vec<WorkflowInput>>,
    /// Steps to execute
    pub steps: Vec<WorkflowStep>,
}

/// Workflow input definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInput {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
    #[serde(default = "default_input_type")]
    pub r#type: String,
}

fn default_input_type() -> String {
    "string".to_string()
}

/// Result of workflow execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub workflow_name: String,
    pub success: bool,
    pub outputs: HashMap<String, serde_json::Value>,
    pub step_results: Vec<StepResult>,
    pub execution_time_ms: u64,
    pub error: Option<String>,
}

/// Result of a single step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub tool: String,
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Orchestrator for executing MCP tool workflows
pub struct WorkflowOrchestrator {
    /// Registry for resolving tool names to actual implementations
    registry: std::sync::Arc<crate::mcp::DynamicToolRegistry>,
}

impl WorkflowOrchestrator {
    /// Create a new orchestrator
    pub fn new(registry: std::sync::Arc<crate::mcp::DynamicToolRegistry>) -> Self {
        Self { registry }
    }

    /// Execute a workflow
    pub async fn execute(
        &self,
        workflow: &Workflow,
        inputs: HashMap<String, serde_json::Value>,
    ) -> Result<WorkflowResult, Box<dyn std::error::Error>> {
        let start = std::time::Instant::now();
        info!("Starting workflow: {}", workflow.name);

        let mut outputs = HashMap::new();
        let mut step_results = Vec::new();

        // Add inputs to outputs for template resolution
        for (k, v) in &inputs {
            outputs.insert(k.clone(), v.clone());
        }

        match workflow.mode {
            ExecutionMode::Sequential => {
                for step in &workflow.steps {
                    let result = self.execute_step(step, &outputs).await;
                    let success = result.is_ok();

                    if let Ok(ref output) = result {
                        if let Some(output_var) = &step.output {
                            outputs.insert(output_var.clone(), output.clone());
                        }
                    }

                    let step_result = StepResult {
                        step_id: step.id.clone(),
                        tool: step.tool.clone(),
                        success,
                        output: result.ok(),
                        error: result.as_ref().err().map(|e| e.to_string()),
                        duration_ms: 0, // TODO: Track timing
                    };
                    step_results.push(step_result);

                    if !success {
                        match step.on_error {
                            Some(OnErrorStrategy::Continue) => {
                                warn!("Step {} failed but continuing", step.id);
                            }
                            Some(OnErrorStrategy::Retry { max_attempts }) => {
                                warn!("Step {} failed, retrying up to {} times", step.id, max_attempts);
                                // TODO: Implement retry logic
                            }
                            _ => {
                                error!("Workflow failed at step {}", step.id);
                                return Ok(WorkflowResult {
                                    workflow_name: workflow.name.clone(),
                                    success: false,
                                    outputs,
                                    step_results,
                                    execution_time_ms: start.elapsed().as_millis() as u64,
                                    error: Some(format!("Failed at step {}", step.id)),
                                });
                            }
                        }
                    }
                }
            }
            ExecutionMode::Parallel => {
                // TODO: Implement parallel execution with tokio::join!
                warn!("Parallel execution mode not yet implemented, falling back to sequential");
                for step in &workflow.steps {
                    let result = self.execute_step(step, &outputs).await;
                    if let Ok(ref output) = result {
                        if let Some(output_var) = &step.output {
                            outputs.insert(output_var.clone(), output.clone());
                        }
                    }
                }
            }
            ExecutionMode::Conditional => {
                // TODO: Implement conditional branching
                warn!("Conditional execution mode not yet implemented, falling back to sequential");
                for step in &workflow.steps {
                    // Check condition
                    if let Some(condition) = &step.condition {
                        if !self.evaluate_condition(condition, &outputs)? {
                            info!("Skipping step {} due to condition", step.id);
                            continue;
                        }
                    }

                    let result = self.execute_step(step, &outputs).await;
                    if let Ok(ref output) = result {
                        if let Some(output_var) = &step.output {
                            outputs.insert(output_var.clone(), output.clone());
                        }
                    }
                }
            }
        }

        Ok(WorkflowResult {
            workflow_name: workflow.name.clone(),
            success: true,
            outputs,
            step_results,
            execution_time_ms: start.elapsed().as_millis() as u64,
            error: None,
        })
    }

    /// Execute a single workflow step
    async fn execute_step(
        &self,
        step: &WorkflowStep,
        context: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        info!("Executing step {}: tool={}", step.id, step.tool);

        // Resolve template variables in params
        let resolved_params = if let Some(params) = &step.params {
            let mut resolved = HashMap::new();
            for (k, v) in params {
                let resolved_value = self.resolve_template(v, context)?;
                resolved.insert(k.clone(), resolved_value);
            }
            Some(resolved)
        } else {
            None
        };

        // TODO: Actually invoke the MCP tool via the registry
        // For now, return a placeholder
        Ok(serde_json::json!({
            "tool": step.tool,
            "params": resolved_params,
            "status": "executed"
        }))
    }

    /// Resolve template variables in a value
    fn resolve_template(
        &self,
        value: &serde_json::Value,
        context: &HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        if let Some(s) = value.as_str() {
            // Simple template resolution: {{var_name}}
            let mut result = s.to_string();
            for (key, val) in context {
                let pattern = format!("{{{{{}}}}}", key);
                if result.contains(&pattern) {
                    result = result.replace(&pattern, &val.to_string());
                }
            }
            Ok(serde_json::Value::String(result))
        } else {
            Ok(value.clone())
        }
    }

    /// Evaluate a condition expression
    fn evaluate_condition(
        &self,
        condition: &str,
        context: &HashMap<String, serde_json::Value>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        // Simple condition evaluation: check if variable exists and is truthy
        // TODO: Implement full expression language
        let resolved = self.resolve_template(&serde_json::Value::String(condition.to_string()), context)?;
        Ok(!resolved.as_str().map_or(true, |s| s.is_empty() || s == "false"))
    }

    /// Parse a workflow from YAML string
    pub fn parse_yaml(yaml: &str) -> Result<Workflow, Box<dyn std::error::Error>> {
        let workflow: Workflow = serde_yaml::from_str(yaml)?;
        Ok(workflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_workflow_yaml() {
        let yaml = r#"
name: "Test Workflow"
mode: sequential
steps:
  - id: step1
    tool: github.list_pull_requests
    params:
      repo: "owner/repo"
    output: prs
"#;
        let workflow = WorkflowOrchestrator::parse_yaml(yaml).unwrap();
        assert_eq!(workflow.name, "Test Workflow");
        assert_eq!(workflow.steps.len(), 1);
        assert_eq!(workflow.steps[0].tool, "github.list_pull_requests");
    }

    #[test]
    fn test_template_resolution() {
        let orch = WorkflowOrchestrator::new(std::sync::Arc::new(
            crate::mcp::DynamicToolRegistry::new(crate::mcp::DynamicRegistryConfig::default())
        ));

        let mut context = HashMap::new();
        context.insert("name".to_string(), serde_json::json!("Alice"));

        let value = serde_json::json!("Hello {{name}}!");
        let resolved = orch.resolve_template(&value, &context).unwrap();

        assert_eq!(resolved, serde_json::json!("Hello Alice!"));
    }
}
