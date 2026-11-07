//! Synchronous gates: `pre_tool` and `pre_tool_transform`.

use super::sync_runner::{self, RunError, StderrMode};
use super::*;

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
pub(super) struct CommandVerdict {
    pub(super) block: Option<String>,
    pub(super) updated_input: Option<Value>,
    pub(super) context: Option<String>,
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
pub(super) fn parse_gate_stdout(stdout: &str) -> CommandVerdict {
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
    let commands = hook_commands_for("pre_tool_transform", working_dir);
    if commands.is_empty() {
        return input;
    }

    let base_event = {
        let mut event = HookEvent::new("pre_tool_transform")
            .session_id(session_id)
            .field("TOOL_NAME", tool_name);
        if let Some(cwd) = working_dir {
            event = event.cwd(cwd);
        }
        event
    };

    let mut current = input;
    for scoped in commands {
        let command_line = scoped.command.clone();
        let event = base_event.clone().with_scope(&scoped.scope);
        let timeout = std::time::Duration::from_millis(
            scoped
                .effective_timeout_ms(crate::config::config().hooks.pre_tool_transform_timeout_ms)
                .max(1),
        );
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
        let output = match sync_runner::run_command(
            std_cmd,
            serialized.as_bytes(),
            timeout,
            StderrMode::Discard,
        )
        .await
        {
            Ok(output) if output.status.success() => output,
            Ok(_) | Err(_) => continue,
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
    let commands = hook_commands_for("pre_tool", working_dir);
    let mut outcome = GateOutcome::allow();
    if commands.is_empty() {
        return outcome;
    }

    let mut current_input = tool_input_json.to_string();
    for scoped in commands {
        let command_line = scoped.command.clone();
        let timeout =
            scoped.effective_timeout_ms(crate::config::config().hooks.pre_tool_timeout_ms);
        let mut event = HookEvent::new("pre_tool")
            .with_scope(&scoped.scope)
            .session_id(session_id)
            .field("TOOL_NAME", tool_name)
            .field(
                "TOOL_INPUT",
                truncate_bytes(&current_input, TOOL_INPUT_ENV_LIMIT),
            );
        if let Some(cwd) = working_dir {
            event = event.cwd(cwd);
        }
        let verdict =
            run_pre_tool_command(&command_line, &event, tool_name, &current_input, timeout).await;
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
    timeout_ms: u64,
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

    let timeout = std::time::Duration::from_millis(timeout_ms.max(1));
    let output = match sync_runner::run_command(
        std_cmd,
        tool_input_json.as_bytes(),
        timeout,
        StderrMode::Capture,
    )
    .await
    {
        Ok(output) => output,
        Err(RunError::Timeout) => {
            crate::logging::warn(&format!(
                "Hook 'pre_tool' command '{command_line}' timed out after {}ms (allowing tool call)",
                timeout.as_millis()
            ));
            return CommandVerdict::default();
        }
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook 'pre_tool' command '{command_line}' {error} (allowing tool call)"
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
