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
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

const MAX_OUTPUT_LEN: usize = 30000;
const DEFAULT_TIMEOUT_MS: u64 = 120000;
const STDIN_POLL_INTERVAL_MS: u64 = 500;
const STDIN_INITIAL_DELAY_MS: u64 = 300;
const PROGRESS_MARKER_PREFIX: &str = "JCODE_PROGRESS ";
const CHECKPOINT_MARKER_PREFIX: &str = "JCODE_CHECKPOINT ";
const BACKGROUND_PROGRESS_GUIDANCE: &str = "For long-running background commands, prefer scripts or commands that periodically print progress updates. Best format: print lines starting with `JCODE_PROGRESS ` followed by JSON like {\"percent\":42,\"message\":\"Running\"} or {\"current\":120,\"total\":1000,\"unit\":\"batches\",\"message\":\"Epoch 2/5\",\"eta_seconds\":30}. Supported JSON fields are `percent`, `message`, `current`, `total`, `unit`, `eta_seconds`, and optional `kind`=`indeterminate` or `kind`=`checkpoint`. For milestone-style wakeups, print `JCODE_CHECKPOINT {\"message\":\"Unit tests passed\"}`. Generic fallback output that can be parsed includes `42%`, `3/10 tests`, `3 of 10 steps`, `1.5/3.0 GiB`, or phase lines like `Compiling ...`, `Downloading ...`, `Running ...`, and `Building ...`. If you are writing the script yourself, add these progress/checkpoint lines explicitly.";
const BASH_TOOL_DESCRIPTION: &str = "Run a bash command. For long-running background commands, prefer scripts that emit progress/checkpoint lines. Print `JCODE_PROGRESS {json}` or `JCODE_CHECKPOINT {json}` lines for reliable reporting, or at least output parseable progress like `42%`, `3/10 tests`, `3 of 10 steps`, `1.5/3.0 GiB`, or `Running ...`.";
const WINDOWS_SHELL_TOOL_DESCRIPTION: &str = "Run a shell command. For long-running background commands, prefer scripts that emit progress/checkpoint lines. Print `JCODE_PROGRESS {json}` or `JCODE_CHECKPOINT {json}` lines for reliable reporting, or at least output parseable progress like `42%`, `3/10 tests`, `3 of 10 steps`, `1.5/3.0 GiB`, or `Running ...`.";

fn progress_ratio_regex() -> Result<&'static regex::Regex> {
    static REGEX: LazyLock<Result<regex::Regex, regex::Error>> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)\b(?P<current>\d{1,6})\s*/\s*(?P<total>\d{1,6})\b(?:\s*(?P<unit>tests?|steps?|files?|items?|cases?|tasks?|targets?|chunks?|batches?|examples?|crates?|modules?|packages?|workers?))?",
        )
    });
    REGEX
        .as_ref()
        .map_err(|err| anyhow::anyhow!("invalid progress ratio regex: {err}"))
}

fn progress_of_regex() -> Result<&'static regex::Regex> {
    static REGEX: LazyLock<Result<regex::Regex, regex::Error>> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)\b(?P<current>\d{1,6})\s+of\s+(?P<total>\d{1,6})\b(?:\s+(?P<unit>tests?|steps?|files?|items?|cases?|tasks?|targets?|chunks?|batches?|examples?|crates?|modules?|packages?|workers?))?",
        )
    });
    REGEX
        .as_ref()
        .map_err(|err| anyhow::anyhow!("invalid progress-of regex: {err}"))
}

fn progress_byte_ratio_regex() -> Result<&'static regex::Regex> {
    static REGEX: LazyLock<Result<regex::Regex, regex::Error>> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)\b(?P<current>\d+(?:\.\d+)?)\s*/\s*(?P<total>\d+(?:\.\d+)?)\s*(?P<unit>bytes?|[kmgt]i?b)\b",
        )
    });
    REGEX
        .as_ref()
        .map_err(|err| anyhow::anyhow!("invalid progress byte-ratio regex: {err}"))
}

fn progress_percent_regex() -> Result<&'static regex::Regex> {
    static REGEX: LazyLock<Result<regex::Regex, regex::Error>> =
        LazyLock::new(|| regex::Regex::new(r"(?i)\b(?P<percent>100|[1-9]?\d)\s*%"));
    REGEX
        .as_ref()
        .map_err(|err| anyhow::anyhow!("invalid progress percent regex: {err}"))
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
    #[serde(default)]
    checkpoint: Option<bool>,
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

fn summarize_background_command(description: Option<&str>, command: &str) -> String {
    if let Some(description) = description
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        return truncate_str(description, 28).to_string();
    }

    let trimmed = command.trim();
    if trimmed.is_empty() {
        return "bash".to_string();
    }

    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let start = tokens
        .iter()
        .position(|token| !token.contains('='))
        .unwrap_or(0);
    let tokens = &tokens[start..];
    if tokens.is_empty() {
        return truncate_str(trimmed, 28).to_string();
    }

    let label = match tokens {
        ["python" | "python3" | "bash" | "sh" | "node", script, ..] => std::path::Path::new(script)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(script)
            .to_string(),
        ["cargo", subcommand, ..] => format!("cargo {}", subcommand),
        ["npm" | "pnpm" | "yarn", command, script, ..] if *command == "run" => {
            format!("{} {} {}", tokens[0], command, script)
        }
        [first, second, ..] => format!("{} {}", first, second),
        [first] => first.to_string(),
        [] => "bash".to_string(),
    };

    truncate_str(&label, 28).to_string()
}

fn parse_progress_marker_with_checkpoint(line: &str) -> Option<(BackgroundTaskProgress, bool)> {
    let payload = line.trim().strip_prefix(PROGRESS_MARKER_PREFIX)?.trim();
    let marker: ProgressMarker = serde_json::from_str(payload).ok()?;
    let is_checkpoint =
        marker.checkpoint.unwrap_or(false) || matches!(marker.kind.as_deref(), Some("checkpoint"));
    let kind = if marker.percent.is_some()
        || matches!((marker.current, marker.total), (_, Some(total)) if total > 0)
    {
        BackgroundTaskProgressKind::Determinate
    } else {
        parse_progress_kind(marker.kind.as_deref())
    };

    Some((
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
        is_checkpoint,
    ))
}

#[cfg(test)]
fn parse_progress_marker(line: &str) -> Option<BackgroundTaskProgress> {
    parse_progress_marker_with_checkpoint(line).map(|(progress, _)| progress)
}

fn parse_checkpoint_marker(line: &str) -> Option<BackgroundTaskProgress> {
    let payload = line.trim().strip_prefix(CHECKPOINT_MARKER_PREFIX)?.trim();
    let marker: ProgressMarker = serde_json::from_str(payload).unwrap_or_else(|_| ProgressMarker {
        percent: None,
        message: Some(payload.to_string()),
        current: None,
        total: None,
        unit: None,
        eta_seconds: None,
        kind: Some("checkpoint".to_string()),
        checkpoint: Some(true),
    });

    Some(
        BackgroundTaskProgress {
            kind: BackgroundTaskProgressKind::Indeterminate,
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

fn parse_heuristic_progress(line: &str) -> Result<Option<BackgroundTaskProgress>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if let Some(captures) = progress_byte_ratio_regex()?.captures(trimmed) {
        let current = captures
            .name("current")
            .and_then(|m| m.as_str().parse::<f64>().ok());
        let total = captures
            .name("total")
            .and_then(|m| m.as_str().parse::<f64>().ok());
        if let (Some(current), Some(total), Some(matched)) = (current, total, captures.get(0))
            && total > 0.0
            && current <= total
        {
            return Ok(Some(
                BackgroundTaskProgress {
                    kind: BackgroundTaskProgressKind::Determinate,
                    percent: Some(((current / total) * 100.0) as f32),
                    message: progress_message_from_line(trimmed, matched.as_str()),
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
            ));
        }
    }

    if let Some(captures) = progress_ratio_regex()?.captures(trimmed) {
        let current = captures
            .name("current")
            .and_then(|m| m.as_str().parse::<u64>().ok());
        let total = captures
            .name("total")
            .and_then(|m| m.as_str().parse::<u64>().ok());
        if let (Some(current), Some(total), Some(matched)) = (current, total, captures.get(0)) {
            return Ok(progress_from_counts(
                trimmed,
                matched.as_str(),
                current,
                total,
                captures
                    .name("unit")
                    .map(|unit| unit.as_str().to_ascii_lowercase()),
            ));
        }
    }

    if let Some(captures) = progress_of_regex()?.captures(trimmed) {
        let current = captures
            .name("current")
            .and_then(|m| m.as_str().parse::<u64>().ok());
        let total = captures
            .name("total")
            .and_then(|m| m.as_str().parse::<u64>().ok());
        if let (Some(current), Some(total), Some(matched)) = (current, total, captures.get(0)) {
            return Ok(progress_from_counts(
                trimmed,
                matched.as_str(),
                current,
                total,
                captures
                    .name("unit")
                    .map(|unit| unit.as_str().to_ascii_lowercase()),
            ));
        }
    }

    if let Some(captures) = progress_percent_regex()?.captures(trimmed)
        && let (Some(percent), Some(matched)) = (
            captures
                .name("percent")
                .and_then(|m| m.as_str().parse::<f32>().ok()),
            captures.get(0),
        )
    {
        return Ok(Some(
            BackgroundTaskProgress {
                kind: BackgroundTaskProgressKind::Determinate,
                percent: Some(percent),
                message: progress_message_from_line(trimmed, matched.as_str()),
                current: None,
                total: None,
                unit: None,
                eta_seconds: None,
                updated_at: Utc::now().to_rfc3339(),
                source: BackgroundTaskProgressSource::ParsedOutput,
            }
            .normalize(),
        ));
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
        return Ok(Some(
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
        ));
    }

    Ok(None)
}

async fn handle_background_output_line(
    output_path: &Path,
    file: &mut tokio::fs::File,
    raw_line: &str,
    stderr: bool,
) {
    if let Some(progress) = parse_checkpoint_marker(raw_line) {
        if let Some(task_id) = task_id_from_output_path(output_path) {
            let _ = crate::background::global()
                .update_checkpoint(task_id, progress)
                .await;
        }
        return;
    }

    if let Some((progress, is_checkpoint)) = parse_progress_marker_with_checkpoint(raw_line) {
        if let Some(task_id) = task_id_from_output_path(output_path) {
            let manager = crate::background::global();
            let _ = if is_checkpoint {
                manager.update_checkpoint(task_id, progress).await
            } else {
                manager.update_progress(task_id, progress).await
            };
        }
        return;
    }

    match parse_heuristic_progress(raw_line) {
        Ok(Some(progress)) => {
            if let Some(task_id) = task_id_from_output_path(output_path) {
                let _ = crate::background::global()
                    .update_progress(task_id, progress)
                    .await;
            }
            return;
        }
        Ok(None) => {}
        Err(err) => {
            let warning = format!("[jcode warning] failed to parse background progress: {err}\n");
            file.write_all(warning.as_bytes()).await.ok();
            file.flush().await.ok();
        }
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
    intent: Option<String>,
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
                "intent": super::intent_schema_property(),
                "command": {
                    "type": "string",
                    "description": cmd_desc
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

    async fn execute(&self, input: Value, mut ctx: ToolContext) -> Result<ToolOutput> {
        let mut params: BashInput = serde_json::from_value(input)?;
        let run_in_background = params.run_in_background.unwrap_or(false);
        // When a sandbox root is configured, force the spawned shell's working
        // directory to that root. We cannot stop a shell command from reading
        // arbitrary absolute paths (e.g. `cat /etc/passwd`) without OS-level
        // sandboxing, but we at least anchor relative-path commands inside the
        // sandbox so the agent's default behavior stays contained.
        if let Some(root) = ctx.sandbox_root.clone() {
            ctx.working_dir = Some(root);
        }

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
                        .intent
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
        let display_name = summarize_background_command(params.intent.as_deref(), &params.command);

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
                            .intent
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
                        Some(display_name.clone()),
                        &ctx.session_id,
                        pid,
                        &started_at,
                        params.notify,
                        params.wake,
                    )
                    .await;
                let output = format!(
                    "Command continued in background due to reload.\n\nTask ID: {}\nOutput file: {}\nStatus file: {}\n\nUse `bg` with action=\"wait\" and task_id=\"{}\" after reload to wait for completion or the next progress checkpoint.",
                    info.task_id,
                    info.output_file.display(),
                    info.status_file.display(),
                    info.task_id,
                );
                return Ok(ToolOutput::new(output)
                    .with_title(
                        params
                            .intent
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
        let description = params.intent.clone();
        let display_name = summarize_background_command(description.as_deref(), &command);
        let working_dir = ctx.working_dir.clone();
        let timeout_ms = params.timeout.unwrap_or(DEFAULT_TIMEOUT_MS).min(600000);
        let timeout_duration = Duration::from_millis(timeout_ms);

        let wake = params.wake;
        let notify = params.notify || wake;
        let info = crate::background::global()
            .spawn_with_notify(
                "bash",
                Some(display_name.clone()),
                &ctx.session_id,
                notify,
                wake,
				move |output_path| async move {
					let mut cmd = build_shell_command(&command);
					#[cfg(unix)]
					unsafe {
						cmd.pre_exec(|| {
							if libc::setpgid(0, 0) == -1 {
								return Err(std::io::Error::last_os_error());
							}
							Ok(())
						});
					}
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
                    let timeout_sleep = tokio::time::sleep(timeout_duration);
                    tokio::pin!(timeout_sleep);
                    let mut timed_out = false;

                    while !stdout_done || !stderr_done {
                        tokio::select! {
							_ = &mut timeout_sleep => {
								timed_out = true;
								#[cfg(unix)]
								{
									if let Some(pid) = child.id() {
										let _ = crate::platform::signal_detached_process_group(pid, libc::SIGKILL);
									} else {
										let _ = child.start_kill();
									}
								}
								#[cfg(not(unix))]
								{
									let _ = child.start_kill();
								}
								break;
							}
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

                    if timed_out {
                        let _ = child.wait().await;
                        let timeout_line = format!(
                            "\n--- Command timed out after {}ms ---\n",
                            timeout_ms
                        );
                        file.write_all(timeout_line.as_bytes()).await.ok();
                        return Ok(TaskResult::failed(
                            Some(124),
                            format!("Command timed out after {}ms", timeout_ms),
                        ));
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
             Name: {}\n\
             Output file: {}\n\
             Status file: {}\n\n\
             {}\n\
             To wait for completion/checkpoints: use the `bg` tool with action=\"wait\" and task_id=\"{}\"\n\
             To check progress immediately: use the `bg` tool with action=\"status\" and task_id=\"{}\"\n\
             To see output: use the `read` tool on the output file, or `bg` with action=\"output\"",
            info.task_id,
            display_name,
            info.output_file.display(),
            info.status_file.display(),
            notify_msg,
            info.task_id,
            info.task_id,
        );

        Ok(ToolOutput::new(output)
            .with_title(description.unwrap_or_else(|| format!("Background: {}", params.command)))
            .with_metadata(json!({
                "background": true,
                "task_id": info.task_id,
                "display_name": display_name,
                "output_file": info.output_file.to_string_lossy(),
                "status_file": info.status_file.to_string_lossy(),
            })))
    }
}

#[cfg(all(test, not(windows)))]
#[path = "bash_tests.rs"]
mod tests;
