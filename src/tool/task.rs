use super::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

const DEFAULT_TIMEOUT_MS: u64 = 300_000;
const MAX_OUTPUT_LEN: usize = 30000;

pub struct TaskTool;

impl TaskTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct TaskInput {
    description: String,
    prompt: String,
    subagent_type: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    command: Option<String>,
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Run a sub-task using a separate jcode invocation. Use for delegated work. \
         The subagent_type is informational in jcode."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["description", "prompt", "subagent_type"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short description of the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "The task for the agent to perform"
                },
                "subagent_type": {
                    "type": "string",
                    "description": "Subagent type (informational in jcode)"
                },
                "session_id": {
                    "type": "string",
                    "description": "Optional session identifier to continue"
                },
                "command": {
                    "type": "string",
                    "description": "Optional command that triggered this task"
                }
            }
        })
    }

    async fn execute(&self, input: Value) -> Result<String> {
        let params: TaskInput = serde_json::from_value(input)?;

        let provider = std::env::var("JCODE_TASK_PROVIDER")
            .or_else(|_| std::env::var("JCODE_ACTIVE_PROVIDER"))
            .unwrap_or_else(|_| "claude".to_string());

        let exe = std::env::current_exe()
            .map_err(|e| anyhow::anyhow!("Unable to locate jcode binary: {}", e))?;

        let mut child = Command::new(exe)
            .arg("--provider")
            .arg(&provider)
            .arg("run")
            .arg(&params.prompt)
            .env("JCODE_NO_UPDATE", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let timeout_duration = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let result = timeout(timeout_duration, async {
            let status = child.wait().await?;
            let mut stdout = String::new();
            let mut stderr = String::new();

            if let Some(mut out) = child.stdout.take() {
                out.read_to_string(&mut stdout).await?;
            }
            if let Some(mut err) = child.stderr.take() {
                err.read_to_string(&mut stderr).await?;
            }

            Ok::<_, anyhow::Error>((status, stdout, stderr))
        })
        .await;

        match result {
            Ok(Ok((status, stdout, stderr))) => {
                let mut output = String::new();
                output.push_str(&format!(
                    "Task: {}\nSubagent: {}\nProvider: {}\n",
                    params.description, params.subagent_type, provider
                ));
                if let Some(session_id) = params.session_id {
                    output.push_str(&format!("Session: {}\n", session_id));
                }
                if let Some(command) = params.command {
                    output.push_str(&format!("Command: {}\n", command));
                }
                output.push('\n');
                if !stdout.is_empty() {
                    output.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !output.ends_with('\n') {
                        output.push('\n');
                    }
                    output.push_str(&stderr);
                }
                if !status.success() {
                    output.push_str(&format!(
                        "\nExit code: {}\n",
                        status.code().unwrap_or(-1)
                    ));
                }
                if output.len() > MAX_OUTPUT_LEN {
                    output.truncate(MAX_OUTPUT_LEN);
                    output.push_str("\n... (output truncated)");
                }
                Ok(output)
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("Task failed: {}", e)),
            Err(_) => {
                let _ = child.kill().await;
                Err(anyhow::anyhow!(
                    "Task timed out after {}ms",
                    DEFAULT_TIMEOUT_MS
                ))
            }
        }
    }
}
