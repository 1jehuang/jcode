use super::{Tool, ToolContext, ToolOutput, StdinInputRequest};
use crate::background::TaskResult;
use crate::stdin_detect::{self, StdinState};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

const MAX_OUTPUT_LEN: usize = 30000;
const DEFAULT_TIMEOUT_MS: u64 = 120000;
const STDIN_POLL_INTERVAL_MS: u64 = 500;
const STDIN_INITIAL_DELAY_MS: u64 = 300;

pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct BashInput {
    command: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    run_in_background: Option<bool>,
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command. Use for system commands, git operations, running scripts, etc. \
         Avoid using for file operations (reading, writing, editing) - use dedicated tools instead. \
         Set run_in_background=true for long-running commands - you'll get a task_id to check later."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "description": {
                    "type": "string",
                    "description": "A brief (5-10 word) description of what this command does"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (max 600000, default 120000). Ignored for background tasks."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run the command in the background. Returns immediately with task_id and output_file path. Use the bg tool or Read tool to check on progress."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: BashInput = serde_json::from_value(input)?;
        let run_in_background = params.run_in_background.unwrap_or(false);

        if run_in_background {
            return self.execute_background(params, ctx).await;
        }

        // Foreground execution with stdin detection
        let timeout_ms = params.timeout.unwrap_or(DEFAULT_TIMEOUT_MS).min(600000);
        let timeout_duration = Duration::from_millis(timeout_ms);

        let has_stdin_channel = ctx.stdin_request_tx.is_some();

        let mut command = Command::new("bash");
        command
            .arg("-c")
            .arg(&params.command)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if has_stdin_channel {
            command.stdin(Stdio::piped());
        }

        if let Some(ref dir) = ctx.working_dir {
            command.current_dir(dir);
        }
        let mut child = command.spawn()?;

        let child_pid = child.id().unwrap_or(0);
        let stdin_handle = child.stdin.take();
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let result = timeout(timeout_duration, async {
            // Collect stdout/stderr in shared buffers
            let stdout_buf = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
            let stderr_buf = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));

            let stdout_buf2 = stdout_buf.clone();
            let stdout_task = tokio::spawn(async move {
                if let Some(out) = stdout_handle {
                    let mut reader = BufReader::new(out);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line).await {
                            Ok(0) => break,
                            Ok(_) => {
                                stdout_buf2.lock().await.push_str(&line);
                            }
                            Err(_) => break,
                        }
                    }
                }
            });

            let stderr_buf2 = stderr_buf.clone();
            let stderr_task = tokio::spawn(async move {
                if let Some(err) = stderr_handle {
                    let mut reader = BufReader::new(err);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line).await {
                            Ok(0) => break,
                            Ok(_) => {
                                stderr_buf2.lock().await.push_str(&line);
                            }
                            Err(_) => break,
                        }
                    }
                }
            });

            // Stdin forwarding with detection polling
            let stdin_task = tokio::spawn({
                let stdin_tx = ctx.stdin_request_tx.clone();
                let tool_call_id = ctx.tool_call_id.clone();
                async move {
                    if let (Some(mut stdin_pipe), Some(stdin_tx)) = (stdin_handle, stdin_tx) {
                        // Wait for the process to start and potentially block
                        tokio::time::sleep(Duration::from_millis(STDIN_INITIAL_DELAY_MS)).await;

                        let mut request_counter = 0u32;
                        loop {
                            // Check if the process (or its children) is waiting for stdin
                            #[cfg(target_os = "linux")]
                            let state = stdin_detect::linux::check_process_tree(child_pid);
                            #[cfg(not(target_os = "linux"))]
                            let state = stdin_detect::is_waiting_for_stdin(child_pid);

                            if state == StdinState::Reading {
                                request_counter += 1;
                                let request_id = format!("stdin-{}-{}", tool_call_id, request_counter);
                                let (response_tx, response_rx) = tokio::sync::oneshot::channel();

                                let request = StdinInputRequest {
                                    request_id,
                                    prompt: String::new(),
                                    is_password: false,
                                    response_tx,
                                };

                                if stdin_tx.send(request).is_err() {
                                    break;
                                }

                                match response_rx.await {
                                    Ok(input) => {
                                        let line = if input.ends_with('\n') {
                                            input
                                        } else {
                                            format!("{}\n", input)
                                        };
                                        if stdin_pipe.write_all(line.as_bytes()).await.is_err() {
                                            break;
                                        }
                                        if stdin_pipe.flush().await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }

                                // Small delay before checking again
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            } else {
                                tokio::time::sleep(Duration::from_millis(STDIN_POLL_INTERVAL_MS)).await;
                            }
                        }
                    }
                }
            });

            // Wait for process exit
            let status = child.wait().await?;

            // Cancel stdin polling
            stdin_task.abort();

            // Wait for output readers to finish
            let _ = stdout_task.await;
            let _ = stderr_task.await;

            let stdout = stdout_buf.lock().await.clone();
            let stderr = stderr_buf.lock().await.clone();

            Ok::<_, anyhow::Error>((status, stdout, stderr))
        })
        .await;

        match result {
            Ok(Ok((status, stdout, stderr))) => {
                let mut output = String::new();

                if !stdout.is_empty() {
                    output.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&stderr);
                }

                // Truncate if too long
                if output.len() > MAX_OUTPUT_LEN {
                    output.truncate(MAX_OUTPUT_LEN);
                    output.push_str("\n... (output truncated)");
                }

                if !status.success() {
                    output.push_str(&format!("\n\nExit code: {}", status.code().unwrap_or(-1)));
                }

                let output = if output.is_empty() {
                    "Command completed successfully (no output)".to_string()
                } else {
                    output
                };
                Ok(ToolOutput::new(output)
                    .with_title(params.description.unwrap_or_else(|| params.command.clone())))
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("Command failed: {}", e)),
            Err(_) => {
                // Timeout - try to kill the process
                let _ = child.kill().await;
                Err(anyhow::anyhow!("Command timed out after {}ms", timeout_ms))
            }
        }
    }
}

impl BashTool {
    /// Execute a command in the background
    async fn execute_background(&self, params: BashInput, ctx: ToolContext) -> Result<ToolOutput> {
        let command = params.command.clone();
        let description = params.description.clone();
        let working_dir = ctx.working_dir.clone();

        let info = crate::background::global()
            .spawn("bash", &ctx.session_id, move |output_path| async move {
                let mut cmd = Command::new("bash");
                cmd.arg("-c")
                    .arg(&command)
                    .kill_on_drop(true)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                if let Some(ref dir) = working_dir {
                    cmd.current_dir(dir);
                }
                let mut child = cmd
                    .spawn()
                    .map_err(|e| anyhow::anyhow!("Failed to spawn command: {}", e))?;

                // Stream output to file
                let mut file = tokio::fs::File::create(&output_path)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create output file: {}", e))?;

                // Read stdout and stderr truly concurrently using select!
                // Sequential reads can deadlock if the unread pipe fills up.
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();

                let mut stdout_lines = stdout.map(|s| BufReader::new(s).lines());
                let mut stderr_lines = stderr.map(|s| BufReader::new(s).lines());
                let mut stdout_done = stdout_lines.is_none();
                let mut stderr_done = stderr_lines.is_none();

                while !stdout_done || !stderr_done {
                    tokio::select! {
                        line = async {
                            match stdout_lines.as_mut() {
                                Some(r) => r.next_line().await,
                                None => std::future::pending().await,
                            }
                        }, if !stdout_done => {
                            match line {
                                Ok(Some(line)) => {
                                    let line_with_newline = format!("{}\n", line);
                                    file.write_all(line_with_newline.as_bytes()).await.ok();
                                    file.flush().await.ok();
                                }
                                _ => { stdout_done = true; }
                            }
                        }
                        line = async {
                            match stderr_lines.as_mut() {
                                Some(r) => r.next_line().await,
                                None => std::future::pending().await,
                            }
                        }, if !stderr_done => {
                            match line {
                                Ok(Some(line)) => {
                                    let line_with_newline = format!("[stderr] {}\n", line);
                                    file.write_all(line_with_newline.as_bytes()).await.ok();
                                    file.flush().await.ok();
                                }
                                _ => { stderr_done = true; }
                            }
                        }
                    }
                }

                let status = child.wait().await?;
                let exit_code = status.code();

                // Write final status line
                let status_line = format!(
                    "\n--- Command finished with exit code: {} ---\n",
                    exit_code.unwrap_or(-1)
                );
                file.write_all(status_line.as_bytes()).await.ok();

                if status.success() {
                    Ok(TaskResult {
                        exit_code,
                        error: None,
                    })
                } else {
                    Ok(TaskResult {
                        exit_code,
                        error: Some(format!(
                            "Command exited with code {}",
                            exit_code.unwrap_or(-1)
                        )),
                    })
                }
            })
            .await;

        let output = format!(
            "Command started in background.\n\n\
             Task ID: {}\n\
             Output file: {}\n\
             Status file: {}\n\n\
             You will be notified when the task completes.\n\
             To check progress: use the `bg` tool with action=\"status\" and task_id=\"{}\"\n\
             To see output: use the `read` tool on the output file, or `bg` with action=\"output\"",
            info.task_id,
            info.output_file.display(),
            info.status_file.display(),
            info.task_id,
        );

        Ok(ToolOutput::new(output)
            .with_title(description.unwrap_or_else(|| format!("Background: {}", params.command)))
            .with_metadata(json!({
                "background": true,
                "task_id": info.task_id,
                "output_file": info.output_file.to_string_lossy(),
                "status_file": info.status_file.to_string_lossy(),
            })))
    }
}
