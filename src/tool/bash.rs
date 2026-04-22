use super::{StdinInputRequest, Tool, ToolContext, ToolOutput};
use crate::background::TaskResult;
use crate::bus::{
    BackgroundTaskProgress, BackgroundTaskProgressKind, BackgroundTaskProgressSource,
};
use crate::stdin_detect::{self, StdinState};
use crate::util::truncate_str;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

const MAX_OUTPUT_LEN: usize = 30000;
const DEFAULT_TIMEOUT_MS: u64 = 120000;
const STDIN_POLL_INTERVAL_MS: u64 = 500;
const STDIN_INITIAL_DELAY_MS: u64 = 300;
const PROGRESS_MARKER_PREFIX: &str = "JCODE_PROGRESS ";
const BACKGROUND_PROGRESS_GUIDANCE: &str = "For long-running background commands, prefer scripts or commands that periodically print progress updates. Best format: print lines starting with `JCODE_PROGRESS ` followed by JSON like {\"percent\":42,\"message\":\"Running\"} or {\"current\":120,\"total\":1000,\"unit\":\"batches\",\"message\":\"Epoch 2/5\",\"eta_seconds\":30}. Supported JSON fields are `percent`, `message`, `current`, `total`, `unit`, `eta_seconds`, and optional `kind`=`indeterminate`. Generic fallback output that can be parsed includes `42%`, `3/10 tests`, `3 of 10 steps`, `1.5/3.0 GiB`, or phase lines like `Compiling ...`, `Downloading ...`, `Running ...`, and `Building ...`. If you are writing the script yourself, add these progress lines explicitly.";
const BASH_TOOL_DESCRIPTION: &str = "Run a bash command. For long-running background commands, prefer scripts that emit progress lines. Print `JCODE_PROGRESS {json}` lines for reliable progress reporting, or at least output parseable progress like `42%`, `3/10 tests`, `3 of 10 steps`, `1.5/3.0 GiB`, or `Running ...`.";
const WINDOWS_SHELL_TOOL_DESCRIPTION: &str = "Run a shell command. For long-running background commands, prefer scripts that emit progress lines. Print `JCODE_PROGRESS {json}` lines for reliable progress reporting, or at least output parseable progress like `42%`, `3/10 tests`, `3 of 10 steps`, `1.5/3.0 GiB`, or `Running ...`.";

fn progress_ratio_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\b(?P<current>\d{1,6})\s*/\s*(?P<total>\d{1,6})\b(?:\s*(?P<unit>tests?|steps?|files?|items?|cases?|tasks?|targets?|chunks?|batches?|examples?|crates?|modules?|packages?|workers?))?",
        )
        .expect("valid progress ratio regex")
    })
}

fn progress_of_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\b(?P<current>\d{1,6})\s+of\s+(?P<total>\d{1,6})\b(?:\s+(?P<unit>tests?|steps?|files?|items?|cases?|tasks?|targets?|chunks?|batches?|examples?|crates?|modules?|packages?|workers?))?",
        )
        .expect("valid progress of regex")
    })
}

fn progress_byte_ratio_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(
            r"(?i)\b(?P<current>\d+(?:\.\d+)?)\s*/\s*(?P<total>\d+(?:\.\d+)?)\s*(?P<unit>bytes?|[kmgt]i?b)\b",
        )
        .expect("valid progress byte ratio regex")
    })
}

fn progress_percent_regex() -> &'static regex::Regex {
    static REGEX: OnceLock<regex::Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(?P<percent>100|[1-9]?\d)\s*%")
            .expect("valid progress percent regex")
    })
}

#[derive(Deserialize)]
struct ProgressMarker {
    #[serde(default)]
    percent: Option<f32>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    current: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    eta_seconds: Option<u64>,
    #[serde(default)]
    kind: Option<String>,
}

fn task_id_from_output_path(path: &Path) -> Option<&str> {
    path.file_stem()?.to_str()
}

fn parse_progress_kind(kind: Option<&str>) -> BackgroundTaskProgressKind {
    match kind {
        Some("indeterminate") => BackgroundTaskProgressKind::Indeterminate,
        _ => BackgroundTaskProgressKind::Determinate,
    }
}

fn parse_progress_marker(line: &str) -> Option<BackgroundTaskProgress> {
    let payload = line.trim().strip_prefix(PROGRESS_MARKER_PREFIX)?.trim();
    let marker: ProgressMarker = serde_json::from_str(payload).ok()?;
    let kind = if marker.percent.is_some()
        || matches!((marker.current, marker.total), (_, Some(total)) if total > 0)
    {
        BackgroundTaskProgressKind::Determinate
    } else {
        parse_progress_kind(marker.kind.as_deref())
    };

    Some(
        BackgroundTaskProgress {
            kind,
            percent: marker.percent,
            message: marker.message,
            current: marker.current,
            total: marker.total,
            unit: marker.unit,
            eta_seconds: marker.eta_seconds,
            updated_at: Utc::now().to_rfc3339(),
            source: BackgroundTaskProgressSource::Reported,
        }
        .normalize(),
    )
}

fn progress_message_from_line(line: &str, matched_fragment: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(matched_fragment.trim()) {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn progress_from_counts(
    trimmed: &str,
    matched: &str,
    current: u64,
    total: u64,
    unit: Option<String>,
) -> Option<BackgroundTaskProgress> {
    if total < 2 || current > total {
        return None;
    }

    Some(
        BackgroundTaskProgress {
            kind: BackgroundTaskProgressKind::Determinate,
            percent: None,
            message: progress_message_from_line(trimmed, matched),
            current: Some(current),
            total: Some(total),
            unit,
            eta_seconds: None,
            updated_at: Utc::now().to_rfc3339(),
            source: BackgroundTaskProgressSource::ParsedOutput,
        }
        .normalize(),
    )
}

fn parse_heuristic_progress(line: &str) -> Option<BackgroundTaskProgress> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(captures) = progress_ratio_regex().captures(trimmed) {
        let current = captures.name("current")?.as_str().parse::<u64>().ok()?;
        let total = captures.name("total")?.as_str().parse::<u64>().ok()?;
        let matched = captures.get(0)?.as_str();
        return progress_from_counts(
            trimmed,
            matched,
            current,
            total,
            captures
                .name("unit")
                .map(|unit| unit.as_str().to_ascii_lowercase()),
        );
    }

    if let Some(captures) = progress_of_regex().captures(trimmed) {
        let current = captures.name("current")?.as_str().parse::<u64>().ok()?;
        let total = captures.name("total")?.as_str().parse::<u64>().ok()?;
        let matched = captures.get(0)?.as_str();
        return progress_from_counts(
            trimmed,
            matched,
            current,
            total,
            captures
                .name("unit")
                .map(|unit| unit.as_str().to_ascii_lowercase()),
        );
    }

    if let Some(captures) = progress_byte_ratio_regex().captures(trimmed) {
        let current = captures.name("current")?.as_str().parse::<f64>().ok()?;
        let total = captures.name("total")?.as_str().parse::<f64>().ok()?;
        if total > 0.0 && current <= total {
            let matched = captures.get(0)?.as_str();
            return Some(
                BackgroundTaskProgress {
                    kind: BackgroundTaskProgressKind::Determinate,
                    percent: Some(((current / total) * 100.0) as f32),
                    message: progress_message_from_line(trimmed, matched),
                    current: None,
                    total: None,
                    unit: captures
                        .name("unit")
                        .map(|unit| unit.as_str().to_ascii_lowercase()),
                    eta_seconds: None,
                    updated_at: Utc::now().to_rfc3339(),
                    source: BackgroundTaskProgressSource::ParsedOutput,
                }
                .normalize(),
            );
        }
    }

    if let Some(captures) = progress_percent_regex().captures(trimmed) {
        let percent = captures.name("percent")?.as_str().parse::<f32>().ok()?;
        let matched = captures.get(0)?.as_str();
        return Some(
            BackgroundTaskProgress {
                kind: BackgroundTaskProgressKind::Determinate,
                percent: Some(percent),
                message: progress_message_from_line(trimmed, matched),
                current: None,
                total: None,
                unit: None,
                eta_seconds: None,
                updated_at: Utc::now().to_rfc3339(),
                source: BackgroundTaskProgressSource::ParsedOutput,
            }
            .normalize(),
        );
    }

    const PHASE_PREFIXES: &[&str] = &[
        "Compiling ",
        "Downloading ",
        "Running ",
        "Building ",
        "Linking ",
        "Resolving ",
        "Fetching ",
        "Installing ",
    ];
    if PHASE_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return Some(
            BackgroundTaskProgress {
                kind: BackgroundTaskProgressKind::Indeterminate,
                percent: None,
                message: Some(trimmed.to_string()),
                current: None,
                total: None,
                unit: None,
                eta_seconds: None,
                updated_at: Utc::now().to_rfc3339(),
                source: BackgroundTaskProgressSource::ParsedOutput,
            }
            .normalize(),
        );
    }

    None
}

async fn handle_background_output_line(
    output_path: &Path,
    file: &mut tokio::fs::File,
    raw_line: &str,
    stderr: bool,
) {
    if let Some(progress) =
        parse_progress_marker(raw_line).or_else(|| parse_heuristic_progress(raw_line))
    {
        if let Some(task_id) = task_id_from_output_path(output_path) {
            let _ = crate::background::global()
                .update_progress(task_id, progress)
                .await;
        }
        return;
    }

    let rendered = if stderr {
        format!("[stderr] {}\n", raw_line)
    } else {
        format!("{}\n", raw_line)
    };
    file.write_all(rendered.as_bytes()).await.ok();
    file.flush().await.ok();
}

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
    #[cfg(windows)]
    use super::build_shell_command;
    use super::format_command_output;

    #[test]
    fn format_command_output_truncates_on_utf8_boundary() {
        let input = format!("{}é", "a".repeat(29_999));
        let output = format_command_output(input, None);
        assert!(output.ends_with("\n... (output truncated)"));
        assert!(output.starts_with(&"a".repeat(29_999)));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn build_shell_command_uses_cmd_and_executes_command() {
        let output = build_shell_command("echo hello-from-cmd")
            .output()
            .await
            .expect("run cmd command");
        assert!(output.status.success(), "cmd command should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.to_ascii_lowercase().contains("hello-from-cmd"),
            "unexpected stdout: {}",
            stdout
        );
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
            WINDOWS_SHELL_TOOL_DESCRIPTION
        } else {
            BASH_TOOL_DESCRIPTION
        }
    }

    fn parameters_schema(&self) -> Value {
        let cmd_desc = if cfg!(windows) {
            "The shell command to execute (via cmd.exe). If you write a long-running script or loop for run_in_background=true, make it print progress lines. Preferred format: `JCODE_PROGRESS {json}`."
        } else {
            "The bash command to execute. If you write a long-running script or loop for run_in_background=true, make it print progress lines. Preferred format: `JCODE_PROGRESS {json}`."
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
                    "description": format!("Run in background. {}", BACKGROUND_PROGRESS_GUIDANCE)
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

        // Auto-detect browser bridge commands and rewrite them to the installed
        // binary when available, but do not run setup automatically. Browser
        // setup should stay an explicit status/setup flow rather than a default
        // side effect of trying to use the browser.
        if crate::browser::is_browser_command(&params.command) {
            params.command = crate::browser::rewrite_command_with_full_path(&params.command);

            // Start/attach a browser session for this jcode session.
            // This gives each agent its own browser tab, preventing
            // multi-agent conflicts when using the browser bridge.
            if !cfg!(windows)
                && std::env::var("BROWSER_SESSION").is_err()
                && let Some(session_name) = crate::browser::ensure_browser_session(&ctx.session_id)
            {
                params.command = format!("BROWSER_SESSION={} {}", session_name, params.command);
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
                                        handle_background_output_line(&output_path, &mut file, &line, false).await;
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
                                        handle_background_output_line(&output_path, &mut file, &line, true).await;
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
        assert!(!req.is_password);

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

    #[test]
    fn test_parse_progress_marker_handles_percent_payloads() {
        let progress = parse_progress_marker(
            r#"JCODE_PROGRESS {"percent":25,"message":"Downloading dependencies"}"#,
        )
        .expect("marker should parse");

        assert_eq!(progress.percent, Some(25.0));
        assert_eq!(
            progress.message.as_deref(),
            Some("Downloading dependencies")
        );
        assert_eq!(progress.kind, BackgroundTaskProgressKind::Determinate);
        assert_eq!(progress.source, BackgroundTaskProgressSource::Reported);
    }

    #[test]
    fn test_parse_heuristic_progress_handles_ratio_output() {
        let progress = parse_heuristic_progress("Running test 3/10 tests")
            .expect("heuristic ratio progress should parse");

        assert_eq!(progress.current, Some(3));
        assert_eq!(progress.total, Some(10));
        assert_eq!(progress.percent, Some(30.0));
        assert_eq!(progress.unit.as_deref(), Some("tests"));
        assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
    }

    #[test]
    fn test_parse_heuristic_progress_handles_percent_output() {
        let progress = parse_heuristic_progress("download progress 42% complete")
            .expect("heuristic percent progress should parse");

        assert_eq!(progress.percent, Some(42.0));
        assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
        assert_eq!(
            progress.message.as_deref(),
            Some("download progress 42% complete")
        );
    }

    #[test]
    fn test_parse_heuristic_progress_handles_phase_output() {
        let progress = parse_heuristic_progress("Compiling jcode v0.10.2")
            .expect("phase progress should parse");

        assert_eq!(progress.kind, BackgroundTaskProgressKind::Indeterminate);
        assert_eq!(progress.percent, None);
        assert_eq!(progress.message.as_deref(), Some("Compiling jcode v0.10.2"));
        assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
    }

    #[test]
    fn test_parse_heuristic_progress_handles_of_output() {
        let progress = parse_heuristic_progress("Downloaded 3 of 12 crates")
            .expect("heuristic of progress should parse");

        assert_eq!(progress.current, Some(3));
        assert_eq!(progress.total, Some(12));
        assert_eq!(progress.percent, Some(25.0));
        assert_eq!(progress.unit.as_deref(), Some("crates"));
    }

    #[test]
    fn test_parse_heuristic_progress_handles_byte_ratio_output() {
        let progress = parse_heuristic_progress("Downloaded 1.5/3.0 GiB")
            .expect("heuristic byte ratio progress should parse");

        assert_eq!(progress.percent, Some(50.0));
        assert_eq!(progress.unit.as_deref(), Some("gib"));
        assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
    }

    #[tokio::test]
    async fn test_background_command_progress_marker_updates_status_and_stays_out_of_output() {
        let tool = BashTool::new();
        let ctx = make_ctx(None);

        let result = tool
            .execute(
                json!({
                    "command": "printf '%s\n' 'JCODE_PROGRESS {\"current\":3,\"total\":10,\"unit\":\"steps\",\"message\":\"Building\"}'; sleep 0.1; echo done",
                    "run_in_background": true,
                    "notify": false,
                    "wake": false,
                }),
                ctx,
            )
            .await
            .expect("background command should start");

        let metadata = result.metadata.expect("expected metadata");
        let task_id = metadata["task_id"]
            .as_str()
            .expect("task id should be present")
            .to_string();

        let mut saw_progress = false;
        for _ in 0..50 {
            let status = crate::background::global()
                .status(&task_id)
                .await
                .expect("status should exist");
            if let Some(progress) = status.progress {
                saw_progress = true;
                assert_eq!(progress.current, Some(3));
                assert_eq!(progress.total, Some(10));
                assert_eq!(progress.unit.as_deref(), Some("steps"));
                assert_eq!(progress.message.as_deref(), Some("Building"));
                assert_eq!(progress.percent, Some(30.0));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            saw_progress,
            "expected progress to be recorded for {task_id}"
        );

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let output = crate::background::global()
            .output(&task_id)
            .await
            .expect("output should exist");
        assert!(output.contains("done"), "output was: {output}");
        assert!(
            !output.contains("JCODE_PROGRESS"),
            "progress marker should be hidden from output: {output}"
        );
    }

    #[tokio::test]
    async fn test_background_command_ratio_output_updates_progress() {
        let tool = BashTool::new();
        let ctx = make_ctx(None);

        let result = tool
            .execute(
                json!({
                    "command": "printf '%s\n' 'Running test 4/8 tests'; sleep 0.1; echo done",
                    "run_in_background": true,
                    "notify": false,
                    "wake": false,
                }),
                ctx,
            )
            .await
            .expect("background command should start");

        let metadata = result.metadata.expect("expected metadata");
        let task_id = metadata["task_id"]
            .as_str()
            .expect("task id should be present")
            .to_string();

        let mut saw_progress = false;
        for _ in 0..50 {
            let status = crate::background::global()
                .status(&task_id)
                .await
                .expect("status should exist");
            if let Some(progress) = status.progress {
                saw_progress = true;
                assert_eq!(progress.current, Some(4));
                assert_eq!(progress.total, Some(8));
                assert_eq!(progress.percent, Some(50.0));
                assert_eq!(progress.unit.as_deref(), Some("tests"));
                assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert!(
            saw_progress,
            "expected heuristic progress to be recorded for {task_id}"
        );
    }

    #[tokio::test]
    async fn test_background_command_byte_ratio_output_updates_progress() {
        let tool = BashTool::new();
        let ctx = make_ctx(None);

        let result = tool
            .execute(
                json!({
                    "command": "printf '%s\n' 'Downloaded 1.5/3.0 GiB'; sleep 0.1; echo done",
                    "run_in_background": true,
                    "notify": false,
                    "wake": false,
                }),
                ctx,
            )
            .await
            .expect("background command should start");

        let metadata = result.metadata.expect("expected metadata");
        let task_id = metadata["task_id"]
            .as_str()
            .expect("task id should be present")
            .to_string();

        let mut saw_progress = false;
        for _ in 0..50 {
            let status = crate::background::global()
                .status(&task_id)
                .await
                .expect("status should exist");
            if let Some(progress) = status.progress {
                saw_progress = true;
                assert_eq!(progress.percent, Some(50.0));
                assert_eq!(progress.unit.as_deref(), Some("gib"));
                assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert!(
            saw_progress,
            "expected byte-ratio progress to be recorded for {task_id}"
        );
    }

    #[test]
    fn test_bash_tool_schema_advertises_background_progress_guidance() {
        let schema = BashTool::new().parameters_schema();
        let command_description = schema["properties"]["command"]["description"]
            .as_str()
            .expect("command description should be a string");
        let background_description = schema["properties"]["run_in_background"]["description"]
            .as_str()
            .expect("run_in_background description should be a string");

        assert!(
            BashTool::new().description().contains("JCODE_PROGRESS"),
            "tool description should teach cooperative progress output"
        );
        assert!(
            command_description.contains("JCODE_PROGRESS"),
            "command description should mention progress marker format"
        );
        assert!(
            background_description.contains("3/10 tests"),
            "background description should mention parseable fallback progress output"
        );
    }
}
