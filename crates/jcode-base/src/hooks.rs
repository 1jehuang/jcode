//! User-configurable lifecycle hooks.
//!
//! Hooks are external commands that jcode runs at well-defined lifecycle
//! points so other programs can observe or gate agent behavior without
//! forking jcode. They are configured in `[hooks]` in config.toml (or
//! `JCODE_HOOK_*` env vars) and follow the same command-line conventions as
//! `[terminal] spawn_hook`: the command is parsed shell-style but executed
//! directly (no shell), with `JCODE_HOOK_*` metadata env vars describing the
//! event.
//!
//! Two dispatch styles:
//!
//! - **Observers** (`turn_start`, `turn_end`, `session_start`, `session_end`,
//!   `post_tool`): spawned detached, fire-and-forget. Failures are logged and
//!   never affect the agent.
//! - **Gate** (`pre_tool`): jcode waits (with a timeout) for the hook to
//!   exit. Exit 0 allows the tool call, exit 2 blocks it and the hook's
//!   stderr is fed back to the model as the tool error. Any other outcome
//!   (other exit codes, timeout, spawn failure) fails open with a warning.
//! - **Feedback** (`post_tool_feedback`): jcode waits (with a timeout) after a
//!   successful tool call. Anything the hook prints to stdout is appended to
//!   the tool result the model sees (e.g. linter findings for an edited file).
//!   Timeouts and spawn failures add nothing.
//!
//! Hook processes get `JCODE_HOOKS_DISABLED=1` in their environment so a
//! hook that itself invokes jcode does not recursively trigger hooks.

use serde_json::Value;
use std::path::PathBuf;

tokio::task_local! {
    /// Terminal identity for the client whose request is currently executing.
    /// Task-local storage keeps concurrent clients isolated without mutating
    /// the daemon's process-wide environment.
    static CLIENT_TERMINAL_ENV: Vec<(String, String)>;
}

/// Maximum bytes of JSON payload exported via `JCODE_HOOK_PAYLOAD`.
const PAYLOAD_ENV_LIMIT: usize = 16 * 1024;
/// Maximum bytes of tool input JSON exported to the pre_tool gate.
const TOOL_INPUT_ENV_LIMIT: usize = 16 * 1024;
/// Maximum chars of hook stderr used as a block reason.
const BLOCK_REASON_LIMIT: usize = 2000;
/// Maximum bytes of `post_tool_feedback` stdout appended to a tool result,
/// per hook command.
const FEEDBACK_LIMIT: usize = 8 * 1024;

/// Maximum bytes of `additionalContext` accepted from one pre_tool hook.
const GATE_CONTEXT_LIMIT: usize = 8 * 1024;

/// Decision returned by the `pre_tool` gate hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    Allow,
    Block { reason: String },
}

/// Full result of running every configured `pre_tool` gate.
#[derive(Debug, Clone, PartialEq)]
pub struct GateOutcome {
    pub decision: GateDecision,
    /// Replacement tool input from `hookSpecificOutput.updatedInput`. Later
    /// gates see the replacement and may replace it again.
    pub updated_input: Option<Value>,
    /// Context strings (`hookSpecificOutput.additionalContext` or
    /// `systemMessage`) to show the model alongside the tool result.
    pub additional_context: Vec<String>,
}

impl GateOutcome {
    fn allow() -> Self {
        Self {
            decision: GateDecision::Allow,
            updated_input: None,
            additional_context: Vec::new(),
        }
    }
}

/// What one gate command asked for.
#[derive(Debug, Default, PartialEq)]
struct CommandVerdict {
    block: Option<String>,
    updated_input: Option<Value>,
    context: Option<String>,
}

impl CommandVerdict {
    fn block(reason: String) -> Self {
        Self {
            block: Some(reason),
            ..Self::default()
        }
    }
}

fn json_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// Parse the Codex/Claude-compatible JSON a gate may print on exit 0.
///
/// Recognized fields: top-level `decision: "block"` with `reason`;
/// `hookSpecificOutput.permissionDecision` (`allow`, `deny`, or `ask`) with
/// `permissionDecisionReason`; `hookSpecificOutput.updatedInput`;
/// `hookSpecificOutput.additionalContext`; and top-level `systemMessage`.
/// jcode has no interactive approval for hooks, so `ask` blocks. Stdout that
/// is not a JSON object is ignored, preserving the plain exit-code contract.
fn parse_gate_stdout(stdout: &str) -> CommandVerdict {
    let trimmed = stdout.trim();
    if !trimmed.starts_with('{') {
        return CommandVerdict::default();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        crate::logging::warn("Hook 'pre_tool' printed invalid JSON on stdout (ignored)");
        return CommandVerdict::default();
    };
    let specific = value.get("hookSpecificOutput").unwrap_or(&Value::Null);
    let mut verdict = CommandVerdict::default();

    let top_decision = json_str(&value, "decision").map(str::to_ascii_lowercase);
    if matches!(top_decision.as_deref(), Some("block" | "deny")) {
        verdict.block = Some(
            json_str(&value, "reason")
                .unwrap_or("blocked by pre_tool hook")
                .to_string(),
        );
    }
    let permission = json_str(specific, "permissionDecision").map(str::to_ascii_lowercase);
    match permission.as_deref() {
        Some("deny") => {
            verdict.block = Some(
                json_str(specific, "permissionDecisionReason")
                    .or_else(|| json_str(&value, "reason"))
                    .unwrap_or("blocked by pre_tool hook")
                    .to_string(),
            );
        }
        Some("ask") => {
            let reason = json_str(specific, "permissionDecisionReason")
                .or_else(|| json_str(&value, "reason"));
            verdict.block = Some(match reason {
                Some(reason) => format!("requires user approval: {reason}"),
                None => "requires user approval (pre_tool hook asked)".to_string(),
            });
        }
        _ => {}
    }
    if let Some(reason) = verdict.block.take() {
        verdict.block = Some(truncate_bytes(&reason, BLOCK_REASON_LIMIT).to_string());
    }

    if let Some(updated) = specific.get("updatedInput")
        && updated.is_object()
    {
        verdict.updated_input = Some(updated.clone());
    }
    let context: Vec<&str> = [
        json_str(specific, "additionalContext"),
        json_str(&value, "systemMessage"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !context.is_empty() {
        verdict.context = Some(truncate_bytes(&context.join("\n"), GATE_CONTEXT_LIMIT).to_string());
    }
    verdict
}

/// A lifecycle event to deliver to a hook.
#[derive(Debug, Clone)]
pub struct HookEvent {
    /// Event name: "turn_start", "turn_end", "session_start", "session_end",
    /// "post_tool".
    pub event: &'static str,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    /// Extra env fields. Keys are suffixes: ("STATUS", "ok") becomes
    /// `JCODE_HOOK_STATUS=ok` and `"status": "ok"` in the JSON payload.
    pub fields: Vec<(&'static str, String)>,
}

impl HookEvent {
    pub fn new(event: &'static str) -> Self {
        Self {
            event,
            session_id: None,
            cwd: None,
            fields: Vec::new(),
        }
    }

    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn field(mut self, key: &'static str, value: impl Into<String>) -> Self {
        self.fields.push((key, value.into()));
        self
    }
}

/// Run `future` with the terminal identity of the client that initiated it.
///
/// Shared-server request handlers use this to keep lifecycle hooks scoped to
/// the requesting pane instead of the environment inherited by the server.
pub async fn with_client_terminal_env<F>(env: Vec<(String, String)>, future: F) -> F::Output
where
    F: std::future::Future,
{
    CLIENT_TERMINAL_ENV.scope(env, future).await
}

/// The configured commands for `event`, in declaration order.
pub fn hook_commands(event: &str) -> Vec<String> {
    if hooks_suppressed() {
        return Vec::new();
    }
    let hooks = &crate::config::config().hooks;
    let raw = match event {
        "turn_start" => hooks.turn_start.as_ref(),
        "turn_end" => hooks.turn_end.as_ref(),
        "session_start" => hooks.session_start.as_ref(),
        "session_end" => hooks.session_end.as_ref(),
        "pre_tool" => hooks.pre_tool.as_ref(),
        "post_tool" => hooks.post_tool.as_ref(),
        "post_tool_feedback" => hooks.post_tool_feedback.as_ref(),
        _ => None,
    };
    raw.into_iter()
        .flat_map(|commands| commands.iter())
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The first configured command for `event`, retained for scalar callers.
pub fn hook_command(event: &str) -> Option<String> {
    hook_commands(event).into_iter().next()
}

/// Whether a hook is configured for `event`. Cheap; used by hot paths to
/// skip payload construction entirely when no hook is set.
pub fn hook_configured(event: &str) -> bool {
    !hook_commands(event).is_empty()
}

/// Run configured tool-input transformers in declaration order. Transformers
/// receive the current JSON tool input on stdin and may print a complete
/// replacement JSON object to stdout. Invalid output, errors, and timeouts
/// fail open and leave the current input untouched.
pub async fn transform_tool_input(
    session_id: &str,
    working_dir: Option<&str>,
    tool_name: &str,
    input: Value,
) -> Value {
    if hooks_suppressed() {
        return input;
    }
    let commands: Vec<String> = crate::config::config()
        .hooks
        .pre_tool_transform
        .as_ref()
        .into_iter()
        .flat_map(|commands| commands.iter())
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_owned)
        .collect();
    if commands.is_empty() {
        return input;
    }

    let mut event = HookEvent::new("pre_tool_transform")
        .session_id(session_id)
        .field("TOOL_NAME", tool_name);
    if let Some(cwd) = working_dir {
        event = event.cwd(cwd);
    }

    let timeout = std::time::Duration::from_millis(
        crate::config::config()
            .hooks
            .pre_tool_transform_timeout_ms
            .max(1),
    );
    let mut current = input;
    for command_line in commands {
        let serialized = current.to_string();
        let std_cmd = match build_hook_process(&command_line, &event) {
            Ok(cmd) => cmd,
            Err(error) => {
                crate::logging::warn(&format!(
                    "Tool transformer '{command_line}' is invalid: {error} (leaving input unchanged)"
                ));
                continue;
            }
        };
        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                crate::logging::warn(&format!(
                    "Tool transformer '{command_line}' failed to start: {error} (leaving input unchanged)"
                ));
                continue;
            }
        };
        // A child that never reads stdin can block `write_all` once the pipe
        // fills. Keep that write under the same deadline as process completion
        // so a transformer always fails open within its configured timeout.
        let output = match tokio::time::timeout(timeout, async {
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                stdin.write_all(serialized.as_bytes()).await?;
            }
            child.wait_with_output().await
        })
        .await
        {
            Ok(Ok(output)) if output.status.success() => output,
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => continue,
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let candidate = stdout.trim();
        if candidate.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(candidate) {
            Ok(value) if value.is_object() => current = value,
            Ok(_) | Err(_) => crate::logging::warn(&format!(
                "Tool transformer '{command_line}' returned invalid JSON object (leaving input unchanged)"
            )),
        }
    }
    current
}

/// True when running inside a hook process (recursion guard).
fn hooks_suppressed() -> bool {
    std::env::var_os("JCODE_HOOKS_DISABLED").is_some()
}

fn expand_home(program: &str) -> PathBuf {
    if let Some(rest) = program.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(program)
}

fn truncate_bytes(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// Tool input JSON truncated to the size exported in `JCODE_HOOK_TOOL_INPUT`.
pub fn truncate_tool_input(tool_input_json: &str) -> &str {
    truncate_bytes(tool_input_json, TOOL_INPUT_ENV_LIMIT)
}

/// JSON payload mirroring the env fields, exported as `JCODE_HOOK_PAYLOAD`.
fn payload_json(event: &HookEvent) -> String {
    let mut map = serde_json::Map::new();
    map.insert(
        "event".to_string(),
        serde_json::Value::String(event.event.to_string()),
    );
    if let Some(session_id) = &event.session_id {
        map.insert(
            "session_id".to_string(),
            serde_json::Value::String(session_id.clone()),
        );
    }
    if let Some(cwd) = &event.cwd {
        map.insert("cwd".to_string(), serde_json::Value::String(cwd.clone()));
    }
    for (key, value) in &event.fields {
        map.insert(
            key.to_ascii_lowercase(),
            serde_json::Value::String(value.clone()),
        );
    }
    let payload = serde_json::Value::Object(map).to_string();
    truncate_bytes(&payload, PAYLOAD_ENV_LIMIT).to_string()
}

fn apply_event_env(cmd: &mut std::process::Command, event: &HookEvent) {
    cmd.env("JCODE_HOOKS_DISABLED", "1");
    cmd.env("JCODE_HOOK_EVENT", event.event);
    if let Some(session_id) = &event.session_id {
        cmd.env("JCODE_HOOK_SESSION_ID", session_id);
    }
    if let Some(cwd) = &event.cwd {
        cmd.env("JCODE_HOOK_CWD", cwd);
    }
    for (key, value) in &event.fields {
        cmd.env(format!("JCODE_HOOK_{key}"), value);
    }
    cmd.env("JCODE_HOOK_PAYLOAD", payload_json(event));
}

fn build_hook_process(
    command_line: &str,
    event: &HookEvent,
) -> anyhow::Result<std::process::Command> {
    let parts = crate::terminal_launch::parse_hook_command(command_line)?;
    let (program, args) = parts
        .split_first()
        .expect("parse_hook_command guarantees at least one part");
    let mut cmd = std::process::Command::new(expand_home(program));
    cmd.args(args);
    if let Some(cwd) = event.cwd.as_deref().filter(|cwd| !cwd.is_empty())
        && std::path::Path::new(cwd).is_dir()
    {
        cmd.current_dir(cwd);
    }
    apply_event_env(&mut cmd, event);
    let _ = CLIENT_TERMINAL_ENV.try_with(|env| {
        crate::terminal_launch::apply_client_terminal_env(&mut cmd, env);
    });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

        // A hook is background work the user never asked to see. Without this the
        // child is a console application started by a process that may hold no
        // console, so Windows allocates a fresh one — and on a host where Windows
        // Terminal is the default terminal application that console is a new tab,
        // one per hook event. The flag is inherited, so a script that shells out to
        // `curl.exe` stays invisible too.
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd)
}

/// Fire an observer hook for `event` if one is configured.
///
/// Detached and fire-and-forget: failures are logged, never propagated, and
/// the hook process cannot block the agent.
pub fn dispatch_observer(event: HookEvent) {
    let command_lines = hook_commands(event.event);
    if command_lines.is_empty() {
        return;
    }
    let event_name = event.event;
    for command_line in command_lines {
        match build_hook_process(&command_line, &event) {
            Ok(mut cmd) => {
                cmd.stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                match crate::platform::spawn_detached_without_console_window(&mut cmd) {
                    Ok(child) => {
                        crate::platform::reap_detached(child);
                        crate::logging::debug(&format!(
                            "Hook '{event_name}' dispatched to '{command_line}' (session={:?})",
                            event.session_id
                        ));
                    }
                    Err(error) => crate::logging::warn(&format!(
                        "Hook '{event_name}' command '{command_line}' failed to start: {error}"
                    )),
                }
            }
            Err(error) => crate::logging::warn(&format!(
                "Hook '{event_name}' command '{command_line}' is invalid: {error}"
            )),
        }
    }
}

/// Run the `pre_tool` gate hook for a tool call, if configured.
///
/// The hook receives `JCODE_HOOK_TOOL_NAME` plus the full tool input JSON on
/// stdin (and truncated in `JCODE_HOOK_TOOL_INPUT`). Contract:
///
/// - exit 0: allow the tool call, optionally with a JSON object on stdout
///   (see [`parse_gate_stdout`]) that can deny, rewrite input, or add context
/// - exit 2: block it; stderr becomes the error shown to the model
/// - anything else (other exits, timeout, spawn failure): fail open
pub async fn run_pre_tool_gate(
    session_id: &str,
    working_dir: Option<&str>,
    tool_name: &str,
    tool_input_json: &str,
) -> GateOutcome {
    let command_lines = hook_commands("pre_tool");
    let mut outcome = GateOutcome::allow();
    if command_lines.is_empty() {
        return outcome;
    }

    let mut current_input = tool_input_json.to_string();
    for command_line in command_lines {
        let mut event = HookEvent::new("pre_tool")
            .session_id(session_id)
            .field("TOOL_NAME", tool_name)
            .field(
                "TOOL_INPUT",
                truncate_bytes(&current_input, TOOL_INPUT_ENV_LIMIT),
            );
        if let Some(cwd) = working_dir {
            event = event.cwd(cwd);
        }
        let verdict = run_pre_tool_command(&command_line, &event, tool_name, &current_input).await;
        if let Some(reason) = verdict.block {
            if outcome.decision == GateDecision::Allow {
                outcome.decision = GateDecision::Block { reason };
            }
        } else if outcome.decision == GateDecision::Allow
            && let Some(updated) = verdict.updated_input
        {
            current_input = updated.to_string();
            outcome.updated_input = Some(updated);
        }
        if let Some(context) = verdict.context {
            outcome.additional_context.push(context);
        }
    }
    if outcome.decision != GateDecision::Allow {
        outcome.updated_input = None;
    }
    outcome
}

async fn run_pre_tool_command(
    command_line: &str,
    event: &HookEvent,
    tool_name: &str,
    tool_input_json: &str,
) -> CommandVerdict {
    let session_id = event.session_id.as_deref().unwrap_or("unknown");
    let std_cmd = match build_hook_process(command_line, event) {
        Ok(cmd) => cmd,
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook 'pre_tool' command '{command_line}' is invalid: {error} (allowing tool call)"
            ));
            return CommandVerdict::default();
        }
    };

    let mut cmd = tokio::process::Command::from(std_cmd);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook 'pre_tool' command '{command_line}' failed to start: {error} (allowing tool call)"
            ));
            return CommandVerdict::default();
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(tool_input_json.as_bytes()).await;
        // Closing stdin signals EOF to hooks that read the whole input.
        drop(stdin);
    }

    let timeout =
        std::time::Duration::from_millis(crate::config::config().hooks.pre_tool_timeout_ms.max(1));
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            crate::logging::warn(&format!(
                "Hook 'pre_tool' command '{command_line}' failed: {error} (allowing tool call)"
            ));
            return CommandVerdict::default();
        }
        Err(_elapsed) => {
            crate::logging::warn(&format!(
                "Hook 'pre_tool' command '{command_line}' timed out after {}ms (allowing tool call)",
                timeout.as_millis()
            ));
            return CommandVerdict::default();
        }
    };

    match output.status.code() {
        Some(0) => {
            let verdict = parse_gate_stdout(&String::from_utf8_lossy(&output.stdout));
            if let Some(reason) = &verdict.block {
                crate::logging::info(&format!(
                    "Hook 'pre_tool' denied tool '{tool_name}' for session {session_id} via JSON: {reason}"
                ));
            }
            verdict
        }
        Some(2) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr.trim();
            let reason = if reason.is_empty() {
                "blocked by pre_tool hook".to_string()
            } else {
                truncate_bytes(reason, BLOCK_REASON_LIMIT).to_string()
            };
            crate::logging::info(&format!(
                "Hook 'pre_tool' blocked tool '{tool_name}' for session {session_id}: {reason}"
            ));
            CommandVerdict::block(reason)
        }
        other => {
            crate::logging::warn(&format!(
                "Hook 'pre_tool' command '{command_line}' exited with {other:?} (expected 0=allow or 2=block; allowing tool call)"
            ));
            CommandVerdict::default()
        }
    }
}

/// Run the `post_tool_feedback` hooks for a completed tool call, in
/// declaration order, and collect what they print to stdout.
///
/// Each hook receives the event env vars plus the full tool input JSON on
/// stdin. Trimmed stdout (capped at 8 KB per hook) is returned regardless of
/// exit code, because linters conventionally exit non-zero when they report
/// findings. Timeouts, spawn failures, and empty stdout contribute nothing.
pub async fn run_post_tool_feedback(event: &HookEvent, tool_input_json: &str) -> Option<String> {
    let command_lines = hook_commands("post_tool_feedback");
    if command_lines.is_empty() {
        return None;
    }
    let timeout = std::time::Duration::from_millis(
        crate::config::config()
            .hooks
            .post_tool_feedback_timeout_ms
            .max(1),
    );
    let mut sections = Vec::new();
    for command_line in command_lines {
        if let Some(text) =
            run_post_tool_feedback_command(&command_line, event, tool_input_json, timeout).await
        {
            sections.push(text);
        }
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

async fn run_post_tool_feedback_command(
    command_line: &str,
    event: &HookEvent,
    tool_input_json: &str,
    timeout: std::time::Duration,
) -> Option<String> {
    let std_cmd = match build_hook_process(command_line, event) {
        Ok(cmd) => cmd,
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook 'post_tool_feedback' command '{command_line}' is invalid: {error}"
            ));
            return None;
        }
    };
    let mut cmd = tokio::process::Command::from(std_cmd);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook 'post_tool_feedback' command '{command_line}' failed to start: {error}"
            ));
            return None;
        }
    };
    // Keep the stdin write under the deadline too: a hook that never reads
    // stdin could otherwise block once the pipe fills.
    let output = match tokio::time::timeout(timeout, async {
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            // A hook that exits without reading stdin closes the pipe; that
            // is fine, its stdout still counts.
            let _ = stdin.write_all(tool_input_json.as_bytes()).await;
        }
        child.wait_with_output().await
    })
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            crate::logging::warn(&format!(
                "Hook 'post_tool_feedback' command '{command_line}' failed: {error}"
            ));
            return None;
        }
        Err(_elapsed) => {
            crate::logging::warn(&format!(
                "Hook 'post_tool_feedback' command '{command_line}' timed out after {}ms",
                timeout.as_millis()
            ));
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = stdout.trim();
    if text.is_empty() {
        return None;
    }
    let mut text = truncate_bytes(text, FEEDBACK_LIMIT).to_string();
    if text.len() < stdout.trim().len() {
        text.push_str("\n[feedback truncated]");
    }
    Some(text)
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "hooks_tests.rs"]
mod tests;
