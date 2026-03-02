use super::{StdinInputRequest, Tool, ToolContext, ToolOutput};
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

fn build_shell_command(cmd_str: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg(cmd_str);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(cmd_str);
        cmd
    }
}

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
        if cfg!(windows) {
            "Execute a shell command (cmd.exe). Use for system commands, git operations, running scripts, etc. \
             Avoid using for file operations (reading, writing, editing) - use dedicated tools instead. \
             Set run_in_background=true for long-running commands - you'll get a task_id to check later."
        } else {
            "Execute a bash command. Use for system commands, git operations, running scripts, etc. \
             Avoid using for file operations (reading, writing, editing) - use dedicated tools instead. \
             Set run_in_background=true for long-running commands - you'll get a task_id to check later."
        }
    }

    fn parameters_schema(&self) -> Value {
        let cmd_desc = if cfg!(windows) {
            "The shell command to execute (via cmd.exe)"
        } else {
            "The bash command to execute"
        };
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": cmd_desc
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
        let mut params: BashInput = serde_json::from_value(input)?;
        let run_in_background = params.run_in_background.unwrap_or(false);

        if run_in_background {
            return self.execute_background(params, ctx).await;
        }

        // Auto-detect and setup browser bridge if needed
        if crate::browser::is_browser_command(&params.command) {
            if !crate::browser::is_setup_complete() {
                let setup_log = crate::browser::ensure_browser_setup().await
                    .unwrap_or_else(|e| format!("Browser setup failed: {}\n", e));

                if !crate::browser::is_setup_complete() {
                    return Ok(ToolOutput::new(setup_log)
                        .with_title("Browser bridge setup (incomplete)".to_string()));
                }

                // Rewrite command to use the installed binary path
                let rewritten = crate::browser::rewrite_command_with_full_path(&params.command);
                let mut output = setup_log;
                output.push_str(&format!("\nRetrying: {}\n\n", rewritten));
                params.command = rewritten;

                // Execute the rewritten command and append output
                let result = self.execute_foreground(&params, &ctx).await?;
                output.push_str(&result.output);
                return Ok(ToolOutput::new(output)
                    .with_title(params.description.clone().unwrap_or_else(|| "browser".to_string())));
            }

            params.command = crate::browser::rewrite_command_with_full_path(&params.command);
        }

        // Foreground execution with stdin detection
        self.execute_foreground(&params, &ctx).await
    }
}

impl BashTool {
    async fn execute_foreground(&self, params: &BashInput, ctx: &ToolContext) -> Result<ToolOutput> {
        let timeout_ms = params.timeout.unwrap_or(DEFAULT_TIMEOUT_MS).min(600000);
        let timeout_duration = Duration::from_millis(timeout_ms);

        let has_stdin_channel = ctx.stdin_request_tx.is_some();

        let mut command = build_shell_command(&params.command);
        command
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
                                let request_id =
                                    format!("stdin-{}-{}", tool_call_id, request_counter);
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
                                tokio::time::sleep(Duration::from_millis(STDIN_POLL_INTERVAL_MS))
                                    .await;
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
                    .with_title(params.description.clone().unwrap_or_else(|| params.command.clone())))
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("Command failed: {}", e)),
            Err(_) => {
                // Timeout - try to kill the process
                let _ = child.kill().await;
                Err(anyhow::anyhow!("Command timed out after {}ms", timeout_ms))
            }
        }
    }

    /// Execute a command in the background
    async fn execute_background(&self, params: BashInput, ctx: ToolContext) -> Result<ToolOutput> {
        let command = params.command.clone();
        let description = params.description.clone();
        let working_dir = ctx.working_dir.clone();

        let info = crate::background::global()
            .spawn("bash", &ctx.session_id, move |output_path| async move {
                let mut cmd = build_shell_command(&command);
                cmd.kill_on_drop(true)
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

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use crate::tool::StdinInputRequest;
    use serde_json::json;
    use tokio::sync::mpsc;

    fn make_ctx(stdin_tx: Option<mpsc::UnboundedSender<StdinInputRequest>>) -> ToolContext {
        ToolContext {
            session_id: "test-session".to_string(),
            message_id: "test-msg".to_string(),
            tool_call_id: "test-call".to_string(),
            working_dir: Some(std::path::PathBuf::from("/tmp")),
            stdin_request_tx: stdin_tx,
        }
    }

    #[tokio::test]
    async fn test_basic_command_no_stdin() {
        let tool = BashTool::new();
        let input = json!({"command": "echo hello"});
        let ctx = make_ctx(None);
        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_basic_command_with_unused_stdin_channel() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tool = BashTool::new();
        let input = json!({"command": "echo world"});
        let ctx = make_ctx(Some(tx));
        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("world"));
    }

    #[tokio::test]
    async fn test_stdin_forwarding_single_line() {
        let (tx, mut rx) = mpsc::unbounded_channel::<StdinInputRequest>();
        let tool = BashTool::new();

        // "head -n1" reads one line from stdin and prints it
        let input = json!({"command": "head -n1", "timeout": 10000});
        let ctx = make_ctx(Some(tx));

        // Spawn the tool execution
        let tool_handle = tokio::spawn(async move { tool.execute(input, ctx).await });

        // Wait for the stdin request to arrive
        let req = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for stdin request")
            .expect("channel closed");

        assert!(req.request_id.starts_with("stdin-test-call-"));
        assert_eq!(req.is_password, false);

        // Send the response
        req.response_tx.send("test_input_line".to_string()).unwrap();

        // Wait for tool to finish
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), tool_handle)
            .await
            .expect("tool timed out")
            .expect("tool panicked")
            .expect("tool errored");

        assert!(
            result.output.contains("test_input_line"),
            "output should contain the input we sent: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_stdin_forwarding_multiple_lines() {
        let (tx, mut rx) = mpsc::unbounded_channel::<StdinInputRequest>();
        let tool = BashTool::new();

        // "head -n2" reads two lines
        let input = json!({"command": "head -n2", "timeout": 15000});
        let ctx = make_ctx(Some(tx));

        let tool_handle = tokio::spawn(async move { tool.execute(input, ctx).await });

        // First line
        let req1 = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for first stdin request")
            .expect("channel closed");
        assert!(
            req1.request_id.ends_with("-1"),
            "first request should end with -1: {}",
            req1.request_id
        );
        req1.response_tx.send("line_one".to_string()).unwrap();

        // Second line
        let req2 = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for second stdin request")
            .expect("channel closed");
        assert!(
            req2.request_id.ends_with("-2"),
            "second request should end with -2: {}",
            req2.request_id
        );
        req2.response_tx.send("line_two".to_string()).unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), tool_handle)
            .await
            .expect("tool timed out")
            .expect("tool panicked")
            .expect("tool errored");

        assert!(
            result.output.contains("line_one"),
            "missing line_one in: {}",
            result.output
        );
        assert!(
            result.output.contains("line_two"),
            "missing line_two in: {}",
            result.output
        );
    }

    #[tokio::test]
    async fn test_stdin_not_triggered_for_non_blocking_command() {
        let (tx, mut rx) = mpsc::unbounded_channel::<StdinInputRequest>();
        let tool = BashTool::new();

        // This command doesn't read stdin at all
        let input = json!({"command": "echo no_stdin_needed", "timeout": 5000});
        let ctx = make_ctx(Some(tx));

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("no_stdin_needed"));

        // No stdin request should have been sent
        assert!(
            rx.try_recv().is_err(),
            "no stdin request should be sent for a command that doesn't read stdin"
        );
    }

    #[tokio::test]
    async fn test_command_timeout_with_stdin_channel() {
        let (tx, _rx) = mpsc::unbounded_channel::<StdinInputRequest>();
        let tool = BashTool::new();

        // cat will block forever on stdin, but we set a short timeout
        // and never respond to the stdin request
        let input = json!({"command": "cat", "timeout": 2000});
        let ctx = make_ctx(Some(tx));

        let result = tool.execute(input, ctx).await;
        assert!(result.is_err(), "should timeout");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("timed out"),
            "error should mention timeout: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_stderr_captured_with_stdin() {
        let (tx, _rx) = mpsc::unbounded_channel::<StdinInputRequest>();
        let tool = BashTool::new();

        let input = json!({"command": "echo stderr_msg >&2", "timeout": 5000});
        let ctx = make_ctx(Some(tx));

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(
            result.output.contains("stderr_msg"),
            "stderr should be captured: {}",
            result.output
        );
    }
}
