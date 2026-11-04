//! Run synchronous hook commands that take a JSON document on stdin.

use super::json_output::CommandResult;
use super::sync_runner::{self, RunError, StderrMode};
use super::{HookEvent, build_hook_process};
use std::time::Duration;

/// Run one command with `stdin_json`. Spawn errors, timeouts and I/O failures
/// return `None` (fail open) after a log line.
pub(super) async fn run_json_command(
    command_line: &str,
    event: &HookEvent,
    stdin_json: &str,
    timeout: Duration,
) -> Option<CommandResult> {
    let name = event.event;
    let std_cmd = match build_hook_process(command_line, event) {
        Ok(cmd) => cmd,
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook '{name}' command '{command_line}' is invalid: {error}"
            ));
            return None;
        }
    };
    match sync_runner::run_command(std_cmd, stdin_json.as_bytes(), timeout, StderrMode::Capture)
        .await
    {
        Ok(output) => Some(CommandResult {
            command: command_line.to_string(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
        Err(RunError::Timeout) => {
            crate::logging::warn(&format!(
                "Hook '{name}' command '{command_line}' timed out after {}ms (failing open)",
                timeout.as_millis()
            ));
            None
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook '{name}' command '{command_line}' {error} (failing open)"
            ));
            None
        }
    }
}

/// Run `commands` in order with the same stdin, collecting each result.
/// `stop_after` can end the run early (used for "first continue wins").
/// Failed commands fail open and contribute no result.
pub async fn run_json_commands(
    commands: &[String],
    event: &HookEvent,
    stdin_json: &str,
    timeout: Duration,
    mut stop_after: impl FnMut(&CommandResult) -> bool,
) -> Vec<CommandResult> {
    let mut results = Vec::new();
    for command in commands {
        if let Some(result) = run_json_command(command, event, stdin_json, timeout).await {
            let done = stop_after(&result);
            results.push(result);
            if done {
                break;
            }
        }
    }
    results
}
