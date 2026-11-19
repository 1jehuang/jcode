//! `post_tool_feedback`: synchronous hooks whose stdout is appended to a tool result.

use super::sync_runner::{self, RunError, StderrMode};
use super::*;

/// Maximum bytes of `post_tool_feedback` stdout appended to a tool result,
/// per hook command.
const FEEDBACK_LIMIT: usize = 8 * 1024;

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
    let output = match sync_runner::run_command(
        std_cmd,
        tool_input_json.as_bytes(),
        timeout,
        StderrMode::Discard,
    )
    .await
    {
        Ok(output) => output,
        Err(RunError::Timeout) => {
            crate::logging::warn(&format!(
                "Hook 'post_tool_feedback' command '{command_line}' timed out after {}ms",
                timeout.as_millis()
            ));
            return None;
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook 'post_tool_feedback' command '{command_line}' {error}"
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
