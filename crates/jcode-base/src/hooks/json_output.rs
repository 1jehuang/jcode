//! Interpreting what a synchronous hook command printed.
//!
//! Contract shared by `user_prompt_submit` and `stop` (Claude compatible):
//! exit 0 with plain stdout or a JSON object, exit 2 with stderr, anything
//! else fails open. Recognised JSON: `decision` (`block`/`deny`) with
//! `reason`, `continue` (false ends processing), `stopReason`,
//! `systemMessage`, and `hookSpecificOutput.additionalContext`. Text fields
//! are capped at [`TEXT_FIELD_LIMIT`](super::payload::TEXT_FIELD_LIMIT). JSON
//! that does not parse is treated as plain text, never half-read.

use super::payload::{TEXT_FIELD_LIMIT, truncate_marked};
use serde_json::Value;

/// One command's result, before any interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// What a command asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verdict {
    /// `decision: "block"` / exit 2: the reason to give the caller.
    pub block: Option<String>,
    /// `continue: false`: end processing, optionally with `stopReason`.
    pub halt: Option<String>,
    /// `hookSpecificOutput.additionalContext`.
    pub additional_context: Option<String>,
    /// `systemMessage`: a notice for the user, not the model.
    pub system_message: Option<String>,
    /// Stdout when it was not a JSON object (exit 0 only).
    pub plain_stdout: Option<String>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_marked(s, TEXT_FIELD_LIMIT))
}

/// Interpret a finished command. `hook_name` is used for the default reason.
pub fn interpret(result: &CommandResult, hook_name: &str) -> Verdict {
    match result.exit_code {
        Some(0) => interpret_stdout(&result.stdout, hook_name),
        Some(2) => {
            let reason = result.stderr.trim();
            Verdict {
                block: Some(if reason.is_empty() {
                    format!("blocked by {hook_name} hook")
                } else {
                    truncate_marked(reason, TEXT_FIELD_LIMIT)
                }),
                ..Verdict::default()
            }
        }
        _ => Verdict::default(),
    }
}

fn interpret_stdout(stdout: &str, hook_name: &str) -> Verdict {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Verdict::default();
    }
    let parsed = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) if trimmed.starts_with('{') && value.is_object() => Some(value),
        _ => None,
    };
    let Some(value) = parsed else {
        return Verdict {
            plain_stdout: Some(truncate_marked(trimmed, TEXT_FIELD_LIMIT)),
            ..Verdict::default()
        };
    };
    let mut verdict = Verdict::default();
    let decision = text(&value, "decision").map(|d| d.to_ascii_lowercase());
    if matches!(decision.as_deref(), Some("block" | "deny")) {
        verdict.block =
            Some(text(&value, "reason").unwrap_or_else(|| format!("blocked by {hook_name} hook")));
    }
    if value.get("continue").and_then(Value::as_bool) == Some(false) {
        verdict.halt = Some(
            text(&value, "stopReason").unwrap_or_else(|| format!("stopped by {hook_name} hook")),
        );
    }
    verdict.system_message = text(&value, "systemMessage");
    verdict.additional_context = value
        .get("hookSpecificOutput")
        .and_then(|specific| text(specific, "additionalContext"));
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(code: Option<i32>, stdout: &str, stderr: &str) -> CommandResult {
        CommandResult {
            command: "cmd".into(),
            exit_code: code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn exit_two_blocks_with_stderr() {
        let v = interpret(&result(Some(2), "", " nope \n"), "stop");
        assert_eq!(v.block.as_deref(), Some("nope"));
        let v = interpret(&result(Some(2), "", ""), "stop");
        assert_eq!(v.block.as_deref(), Some("blocked by stop hook"));
    }

    #[test]
    fn odd_exit_fails_open() {
        assert_eq!(
            interpret(&result(Some(7), "{\"decision\":\"block\"}", "x"), "stop"),
            Verdict::default()
        );
        assert_eq!(interpret(&result(None, "", ""), "stop"), Verdict::default());
    }

    #[test]
    fn plain_stdout_is_reported_as_plain() {
        let v = interpret(&result(Some(0), "hello\n", ""), "user_prompt_submit");
        assert_eq!(v.plain_stdout.as_deref(), Some("hello"));
        assert_eq!(v.block, None);
    }

    #[test]
    fn json_decisions_context_and_halt() {
        let out = r#"{"decision":"block","reason":"why","continue":false,"stopReason":"halt","systemMessage":"note","hookSpecificOutput":{"additionalContext":"ctx"}}"#;
        let v = interpret(&result(Some(0), out, ""), "stop");
        assert_eq!(v.block.as_deref(), Some("why"));
        assert_eq!(v.halt.as_deref(), Some("halt"));
        assert_eq!(v.system_message.as_deref(), Some("note"));
        assert_eq!(v.additional_context.as_deref(), Some("ctx"));
        assert_eq!(v.plain_stdout, None);
    }

    #[test]
    fn invalid_or_truncated_json_is_plain_text() {
        let v = interpret(&result(Some(0), "{\"decision\":\"blo", ""), "stop");
        assert_eq!(v.block, None);
        assert_eq!(v.plain_stdout.as_deref(), Some("{\"decision\":\"blo"));
    }

    #[test]
    fn oversized_text_is_truncated_with_marker() {
        let big = "x".repeat(TEXT_FIELD_LIMIT + 50);
        let out = format!("{{\"hookSpecificOutput\":{{\"additionalContext\":\"{big}\"}}}}");
        let v = interpret(&result(Some(0), &out, ""), "stop");
        let ctx = v.additional_context.expect("context");
        assert!(ctx.ends_with("[truncated]"));
        assert!(ctx.len() < TEXT_FIELD_LIMIT + 20);
    }
}
