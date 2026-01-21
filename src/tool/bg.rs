//! Background task management tool
//!
//! Allows the agent to list, check status, get output, and cancel background tasks.

use super::{Tool, ToolContext, ToolOutput};
use crate::background;
use crate::bus::BackgroundTaskStatus;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct BgTool;

impl BgTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct BgInput {
    /// Action to perform: "list", "status", "output", "cancel", "cleanup"
    action: String,
    /// Task ID (required for status, output, cancel)
    #[serde(default)]
    task_id: Option<String>,
    /// Max age in hours for cleanup (default: 24)
    #[serde(default)]
    max_age_hours: Option<u64>,
}

#[async_trait]
impl Tool for BgTool {
    fn name(&self) -> &str {
        "bg"
    }

    fn description(&self) -> &str {
        "Manage background tasks. Actions: 'list' shows all tasks, 'status' checks a specific task, \
         'output' retrieves full output, 'cancel' stops a running task, 'cleanup' removes old task files."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "status", "output", "cancel", "cleanup"],
                    "description": "Action to perform"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID (required for status, output, cancel)"
                },
                "max_age_hours": {
                    "type": "integer",
                    "description": "For cleanup: remove tasks older than this many hours (default: 24)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: BgInput = serde_json::from_value(input)?;
        let manager = background::global();

        match params.action.as_str() {
            "list" => {
                let tasks: Vec<background::TaskStatusFile> = manager.list().await;
                if tasks.is_empty() {
                    return Ok(ToolOutput::new("No background tasks found.").with_title("bg list"));
                }

                let mut output = String::from("Background Tasks:\n\n");
                output.push_str(&format!(
                    "{:<12} {:<10} {:<12} {:<10} {}\n",
                    "TASK_ID", "TOOL", "STATUS", "DURATION", "SESSION"
                ));
                output.push_str(&"-".repeat(60));
                output.push('\n');

                for task in tasks {
                    let duration = task
                        .duration_secs
                        .map(|d| format!("{:.1}s", d))
                        .unwrap_or_else(|| "running".to_string());
                    let status = match task.status {
                        BackgroundTaskStatus::Running => "running",
                        BackgroundTaskStatus::Completed => "completed",
                        BackgroundTaskStatus::Failed => "failed",
                    };
                    output.push_str(&format!(
                        "{:<12} {:<10} {:<12} {:<10} {}\n",
                        task.task_id,
                        task.tool_name,
                        status,
                        duration,
                        &task.session_id[..8.min(task.session_id.len())]
                    ));
                }

                Ok(ToolOutput::new(output).with_title("bg list"))
            }

            "status" => {
                let task_id = params
                    .task_id
                    .ok_or_else(|| anyhow::anyhow!("task_id is required for status action"))?;

                match manager.status(&task_id).await {
                    Some(task) => {
                        let status_str = match task.status {
                            BackgroundTaskStatus::Running => "running",
                            BackgroundTaskStatus::Completed => "completed",
                            BackgroundTaskStatus::Failed => "failed",
                        };

                        let mut output = format!(
                            "Task: {}\n\
                             Tool: {}\n\
                             Status: {}\n\
                             Session: {}\n\
                             Started: {}\n",
                            task.task_id,
                            task.tool_name,
                            status_str,
                            task.session_id,
                            task.started_at,
                        );

                        if let Some(completed) = task.completed_at {
                            output.push_str(&format!("Completed: {}\n", completed));
                        }
                        if let Some(duration) = task.duration_secs {
                            output.push_str(&format!("Duration: {:.2}s\n", duration));
                        }
                        if let Some(exit_code) = task.exit_code {
                            output.push_str(&format!("Exit code: {}\n", exit_code));
                        }
                        if let Some(error) = task.error {
                            output.push_str(&format!("Error: {}\n", error));
                        }

                        Ok(ToolOutput::new(output)
                            .with_title(format!("bg status {}", task_id))
                            .with_metadata(json!({
                                "task_id": task.task_id,
                                "status": status_str,
                                "exit_code": task.exit_code,
                            })))
                    }
                    None => Err(anyhow::anyhow!("Task not found: {}", task_id)),
                }
            }

            "output" => {
                let task_id = params
                    .task_id
                    .ok_or_else(|| anyhow::anyhow!("task_id is required for output action"))?;

                let output_result: Option<String> = manager.output(&task_id).await;
                match output_result {
                    Some(output) => {
                        let truncated: String = if output.len() > 50000 {
                            format!(
                                "{}...\n\n(Output truncated. Use `read` tool on the output file for full content)",
                                &output[..50000]
                            )
                        } else {
                            output
                        };
                        Ok(ToolOutput::new(truncated).with_title(format!("bg output {}", task_id)))
                    }
                    None => Err(anyhow::anyhow!(
                        "Output not found for task: {}. Task may not exist or output file was deleted.",
                        task_id
                    )),
                }
            }

            "cancel" => {
                let task_id = params
                    .task_id
                    .ok_or_else(|| anyhow::anyhow!("task_id is required for cancel action"))?;

                match manager.cancel(&task_id).await? {
                    true => Ok(ToolOutput::new(format!("Task {} cancelled.", task_id))
                        .with_title(format!("bg cancel {}", task_id))),
                    false => Err(anyhow::anyhow!(
                        "Task {} not found or already completed.",
                        task_id
                    )),
                }
            }

            "cleanup" => {
                let max_age = params.max_age_hours.unwrap_or(24);
                let removed = manager.cleanup(max_age).await?;
                Ok(ToolOutput::new(format!(
                    "Cleaned up {} old task files (older than {} hours).",
                    removed, max_age
                ))
                .with_title("bg cleanup"))
            }

            _ => Err(anyhow::anyhow!(
                "Unknown action: {}. Valid actions: list, status, output, cancel, cleanup",
                params.action
            )),
        }
    }
}
