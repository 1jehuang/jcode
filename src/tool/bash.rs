use super::{StdinInputRequest, Tool, ToolContext, ToolOutput};
use crate::background::TaskResult;
use crate::stdin_detect::{self, StdinState};
use crate::util::truncate_str;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

const MAX_OUTPUT_LEN: usize = 30000;
const DEFAULT_TIMEOUT_MS: u64 = 120000;
const STDIN_POLL_INTERVAL_MS: u64 = 500;
const STDIN_INITIAL_DELAY_MS: u64 = 300;

fn build_shell_command(cmd_str: &str) -> TokioCommand {
    #[cfg(windows)]
    {
        let mut cmd = TokioCommand::new("cmd.exe");
        cmd.arg("/C").arg(cmd_str);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = TokioCommand::new("bash");
        cmd.arg("-c").arg(cmd_str);
        cmd
    }
}

#[cfg(unix)]
fn build_detached_shell_wrapper(command: &str) -> StdCommand {
    let mut cmd = StdCommand::new("bash");
    cmd.arg("-lc")
        .arg(
            r#"eval "$JCODE_RELOAD_DETACH_COMMAND"; status=$?; printf '\n--- Command finished with exit code: %s ---\n' "$status"; exit "$status""#,
        )
        .env("JCODE_RELOAD_DETACH_COMMAND", command);
    cmd
}

fn format_command_output(mut output: String, exit_code: Option<i32>) -> String {
    if output.len() > MAX_OUTPUT_LEN {
        output = truncate_str(&output, MAX_OUTPUT_LEN).to_string();
        output.push_str("\n... (output truncated)");
    }

    if let Some(code) = exit_code.filter(|code| *code != 0) {
        output.push_str(&format!("\n\nExit code: {}", code));
    }

    if output.trim().is_empty() {
        "Command completed successfully (no output)".to_string()
    } else {
        output
    }
}

#[cfg(test)]
mod utf8_truncation_tests {
    use super::format_command_output;

    #[test]
    fn format_command_output_truncates_on_utf8_boundary() {
        let input = format!("{}é", "a".repeat(29_999));
        let output = format_command_output(input, None);
        assert!(output.ends_with("\n... (output truncated)"));
        assert!(output.starts_with(&"a".repeat(29_999)));
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
    #[serde(default = "default_true")]
    notify: bool,
    #[serde(default)]
    wake: bool,
}

fn default_true() -> bool {
    true
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        if cfg!(windows) {
            "Run a shell command."
        } else {
            "Run a bash command."
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
                    "description": "Short command description."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in ms."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run in background."
                },
                "notify": {
                    "type": "boolean",
                    "description": "Notify on completion."
                },
                "wake": {
                    "type": "boolean",
                    "description": "Wake on completion."
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
                let setup_log = crate::browser::ensure_browser_setup()
                    .await
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
                return Ok(ToolOutput::new(output).with_title(
                    params
                        .description
                        .clone()
                        .unwrap_or_else(|| "browser".to_string()),
                ));
            }

            params.command = crate::browser::rewrite_command_with_full_path(&params.command);

            // Start/attach a browser session for this jcode session.
            // This gives each agent its own browser tab, preventing
            // multi-agent conflicts when using the browser bridge.
            if !cfg!(windows) && std::env::var("BROWSER_SESSION").is_err() {
                if let Some(session_name) = crate::browser::ensure_browser_session(&ctx.session_id)
                {
                    params.command = format!("BROWSER_SESSION={} {}", session_name, params.command);
                }
            }
        }

        // Foreground execution with stdin detection
        self.execute_foreground(&params, &ctx).await
    }
}

impl BashTool {
    async fn execute_foreground(
        &self,
        params: &BashInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        #[cfg(unix)]
        if self.supports_reload_persistence(ctx) {
            return self
                .execute_reload_persistable_foreground(params, ctx)
                .await;
        }

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
            let stdout_task = tokio::spawn(async move {
                let mut buf = String::new();
                if let Some(mut out) = stdout_handle {
                    let _ = out.read_to_string(&mut buf).await;
                }
                buf
            });

            let stderr_task = tokio::spawn(async move {
                let mut buf = String::new();
                if let Some(mut err) = stderr_handle {
                    let _ = err.read_to_string(&mut buf).await;
                }
                buf
            });

            let stdin_task = if has_stdin_channel {
                Some(tokio::spawn({
                    let stdin_tx = ctx.stdin_request_tx.clone();
                    let tool_call_id = ctx.tool_call_id.clone();
                    async move {
                        if let (Some(mut stdin_pipe), Some(stdin_tx)) = (stdin_handle, stdin_tx) {
                            tokio::time::sleep(Duration::from_millis(STDIN_INITIAL_DELAY_MS)).await;

                            let mut request_counter = 0u32;
                            loop {
                                #[cfg(target_os = "linux")]
                                let state = stdin_detect::linux::check_process_tree(child_pid);
                                #[cfg(not(target_os = "linux"))]
                                let state = stdin_detect::is_waiting_for_stdin(child_pid);

                                if state == StdinState::Reading {
                                    request_counter += 1;
                                    let request_id =
                                        format!("stdin-{}-{}", tool_call_id, request_counter);
                                    let (response_tx, response_rx) =
                                        tokio::sync::oneshot::channel();

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
                                            if stdin_pipe.write_all(line.as_bytes()).await.is_err()
                                            {
                                                break;
                                            }
                                            if stdin_pipe.flush().await.is_err() {
                                                break;
                                            }
                                        }
                                        Err(_) => break,
                                    }

                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                } else {
                                    tokio::time::sleep(Duration::from_millis(
                                        STDIN_POLL_INTERVAL_MS,
                                    ))
                                    .await;
                                }
                            }
                        }
                    }
                }))
            } else {
                drop(stdin_handle);
                None
            };

            let status = child.wait().await?;

            if let Some(task) = stdin_task {
                task.abort();
            }

            let stdout = stdout_task.await.unwrap_or_default();
            let stderr = stderr_task.await.unwrap_or_default();

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
                let output = format_command_output(output, status.code());
                Ok(ToolOutput::new(output).with_title(
                    params
                        .description
                        .clone()
                        .unwrap_or_else(|| params.command.clone()),
                ))
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("Command failed: {}", e)),
            Err(_) => {
                // Timeout - try to kill the process
                let _ = child.kill().await;
                Err(anyhow::anyhow!("Command timed out after {}ms", timeout_ms))
            }
        }
    }

    #[cfg(unix)]
    fn supports_reload_persistence(&self, ctx: &ToolContext) -> bool {
        matches!(
            ctx.execution_mode,
            crate::tool::ToolExecutionMode::AgentTurn
        ) && ctx.stdin_request_tx.is_none()
            && ctx.graceful_shutdown_signal.is_some()
    }

    #[cfg(unix)]
    async fn execute_reload_persistable_foreground(
        &self,
        params: &BashInput,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let timeout_ms = params.timeout.unwrap_or(DEFAULT_TIMEOUT_MS).min(600000);
        let timeout_duration = Duration::from_millis(timeout_ms);
        let started_at = Utc::now().to_rfc3339();
        let started = Instant::now();
        let manager = crate::background::global();
        let info = manager.reserve_task_info();

        let mut cmd = build_detached_shell_wrapper(&params.command);
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&info.output_file)?;
        let stderr = stdout.try_clone()?;
        cmd.stdin(Stdio::null()).stdout(stdout).stderr(stderr);
        if let Some(ref dir) = ctx.working_dir {
            cmd.current_dir(dir);
        }

        let mut child = crate::platform::spawn_detached(&mut cmd)?;
        let pid = child.id();
        let shutdown_signal = ctx.graceful_shutdown_signal.clone();

        loop {
            if let Some(status) = child.try_wait()? {
                let output = tokio::fs::read_to_string(&info.output_file)
                    .await
                    .unwrap_or_default();
                let _ = tokio::fs::remove_file(&info.output_file).await;
                let _ = tokio::fs::remove_file(&info.status_file).await;
                return Ok(
                    ToolOutput::new(format_command_output(output, status.code())).with_title(
                        params
                            .description
                            .clone()
                            .unwrap_or_else(|| params.command.clone()),
                    ),
                );
            }

            if started.elapsed() >= timeout_duration {
                let _ = crate::platform::signal_detached_process_group(pid, libc::SIGKILL);
                let _ = tokio::fs::remove_file(&info.output_file).await;
                let _ = tokio::fs::remove_file(&info.status_file).await;
                return Err(anyhow::anyhow!("Command timed out after {}ms", timeout_ms));
            }

            if shutdown_signal
                .as_ref()
                .map(|signal| signal.is_set())
                .unwrap_or(false)
            {
                manager
                    .register_detached_task(
                        &info,
                        "bash",
                        &ctx.session_id,
                        pid,
                        &started_at,
                        params.notify,
                        params.wake,
                    )
                    .await;
                let output = format!(
                    "Command continued in background due to reload.\n\nTask ID: {}\nOutput file: {}\nStatus file: {}\n\nUse `bg` with action=\"status\" and task_id=\"{}\" after reload.",
                    info.task_id,
                    info.output_file.display(),
                    info.status_file.display(),
                    info.task_id,
                );
                return Ok(ToolOutput::new(output)
                    .with_title(
                        params
                            .description
                            .clone()
                            .unwrap_or_else(|| params.command.clone()),
                    )
                    .with_metadata(json!({
                        "background": true,
                        "task_id": info.task_id,
                        "output_file": info.output_file.to_string_lossy(),
                        "status_file": info.status_file.to_string_lossy(),
                        "reload_persisted": true,
                        "pid": pid,
                    })));
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Execute a command in the background
    async fn execute_background(&self, params: BashInput, ctx: ToolContext) -> Result<ToolOutput> {
        let command = params.command.clone();
        let description = params.description.clone();
        let working_dir = ctx.working_dir.clone();

        let wake = params.wake;
        let notify = params.notify || wake;
        let info = crate::background::global()
            .spawn_with_notify(
                "bash",
                &ctx.session_id,
                notify,
                wake,
                move |output_path| async move {
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
                        Ok(TaskResult::completed(exit_code))
                    } else {
                        Ok(TaskResult::failed(
                            exit_code,
                            format!("Command exited with code {}", exit_code.unwrap_or(-1)),
                        ))
                    }
                },
            )
            .await;

        let notify_msg = if wake {
            "The agent will be woken when the task completes."
        } else if notify {
            "You will be notified when the task completes."
        } else {
            "Notifications disabled. Use `bg` tool to check status."
        };
        let output = format!(
            "Command started in background.\n\n\
             Task ID: {}\n\
             Output file: {}\n\
             Status file: {}\n\n\
             {}\n\
             To check progress: use the `bg` tool with action=\"status\" and task_id=\"{}\"\n\
             To see output: use the `read` tool on the output file, or `bg` with action=\"output\"",
            info.task_id,
            info.output_file.display(),
            info.status_file.display(),
            notify_msg,
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
    use crate::bus::BackgroundTaskStatus;
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
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        }
    }

    fn make_agent_ctx(signal: jcode_agent_runtime::InterruptSignal) -> ToolContext {
        ToolContext {
            session_id: "test-session".to_string(),
            message_id: "test-msg".to_string(),
            tool_call_id: "test-call-agent".to_string(),
            working_dir: Some(std::path::PathBuf::from("/tmp")),
            stdin_request_tx: None,
            graceful_shutdown_signal: Some(signal),
            execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
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
    async fn test_reload_persistable_bash_continues_in_background() {
        let tool = BashTool::new();
        let signal = jcode_agent_runtime::InterruptSignal::new();
        let ctx = make_agent_ctx(signal.clone());

        let signal_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            signal.fire();
        });

        let result = tool
            .execute(
                json!({"command": "sleep 1; echo reload_persist_ok", "timeout": 10000}),
                ctx,
            )
            .await
            .expect("reload-persistable command should succeed");
        signal_task.await.expect("signal task should complete");

        let metadata = result.metadata.expect("expected background metadata");
        assert_eq!(metadata["background"], true);
        assert_eq!(metadata["reload_persisted"], true);
        let task_id = metadata["task_id"]
            .as_str()
            .expect("task_id should be present")
            .to_string();
        let output_file = std::path::PathBuf::from(
            metadata["output_file"]
                .as_str()
                .expect("output_file should be present"),
        );
        let status_file = std::path::PathBuf::from(
            metadata["status_file"]
                .as_str()
                .expect("status_file should be present"),
        );

        tokio::time::sleep(std::time::Duration::from_millis(1400)).await;

        let status = crate::background::global()
            .status(&task_id)
            .await
            .expect("status should exist");
        assert_eq!(status.status, BackgroundTaskStatus::Completed);
        let output = crate::background::global()
            .output(&task_id)
            .await
            .expect("output should exist");
        assert!(output.contains("reload_persist_ok"), "output was: {output}");

        let _ = tokio::fs::remove_file(output_file).await;
        let _ = tokio::fs::remove_file(status_file).await;
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
