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

mod capabilities;
mod feedback;
mod gate;
mod json_output;
mod json_runner;
mod payload;
mod sync_runner;

pub use capabilities::hook_capabilities;
pub use feedback::run_post_tool_feedback;
#[cfg(test)]
use gate::{CommandVerdict, parse_gate_stdout};
pub use gate::{GateDecision, GateOutcome, run_pre_tool_gate, transform_tool_input};
pub use json_output::{CommandResult, Verdict, interpret};
pub use json_runner::run_json_commands;
pub use payload::{TEXT_FIELD_LIMIT, common_payload, jcode_object, truncate_marked};

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "tests.rs"]
mod tests;
