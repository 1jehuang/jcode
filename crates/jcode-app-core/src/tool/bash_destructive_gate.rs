//! The destructive-command gate for the `bash` tool (issue #604).
//!
//! Kept in its own file so the policy seam is easy to find and review: this is
//! the only thing standing between a model's `rm -rf` and the user's data.

/// Apply the deterministic destructive-command gate, returning refusal text
/// when the command must not run as-issued.
///
/// Stage 1 is a pure blast-radius assessment; stage 2 turns a `Confirm` verdict
/// into a reflection prompt that a blind retry cannot satisfy. Catastrophic
/// targets (`/`, `$HOME`, credential stores, device nodes) are denied outright.
/// See issue #604.
pub(super) fn destructive_command_refusal(
    command: &str,
    justification: Option<&str>,
    working_dir: Option<std::path::PathBuf>,
) -> Option<String> {
    let mut risk_ctx = jcode_command_risk::RiskContext::from_env(working_dir);
    // Assess the same scratch path that the child shell actually receives.
    #[cfg(not(windows))]
    {
        risk_ctx.scratch_dir = super::tool_scratch_dir();
    }
    let assessment = jcode_command_risk::assess(command, &risk_ctx);
    if assessment.level.runs_immediately() {
        return None;
    }

    let justification = jcode_command_risk::Justification {
        text: justification.map(str::to_string),
    };
    match jcode_command_risk::gate(&assessment, &justification) {
        jcode_command_risk::GateOutcome::Allow => None,
        jcode_command_risk::GateOutcome::Deny { reason } => {
            crate::logging::warn(&format!("[bash] denied destructive command: {command}"));
            Some(reason)
        }
        jcode_command_risk::GateOutcome::Reflect { prompt } => {
            crate::logging::info(&format!(
                "[bash] destructive command held for justification: {command}"
            ));
            Some(prompt)
        }
    }
}

/// The `bash` tool's JSON schema, including the `justification` field the
/// destructive-command gate consumes.
///
/// Lives beside the gate so the schema and the policy that reads it stay in
/// sync, and so bash.rs stays inside the code-size budget.
pub(super) fn bash_parameters_schema() -> serde_json::Value {
    let cmd_desc = if cfg!(windows) {
        "The Windows command to execute via cmd.exe. Use cmd.exe syntax and quoting, not Bash syntax."
    } else {
        "The bash command to execute. Put large temp files under `$JCODE_SCRATCH_DIR`, not `/tmp`."
    };
    serde_json::json!({
        "type": "object",
        "required": ["command"],
        "properties": {
            "intent": crate::tool::intent_schema_property(),
            "command": {
                "type": "string",
                "description": cmd_desc
            },
            "timeout": {
                "type": "integer",
                "description": "Timeout in MILLISECONDS (not seconds), e.g. 600000 = 10min; kills with exit 124. Omit for no timeout."
            },
            "run_in_background": {
                "type": "boolean",
                "description": "Run in background. Emit `JCODE_PROGRESS {json}` lines for progress reporting."
            },
            "notify": {
                "type": "boolean",
                "description": "Notify on completion."
            },
            "wake": {
                "type": "boolean",
                "description": "Wake on completion."
            },
            "stall_wake_seconds": {
                "type": "integer",
                "description": "With run_in_background: wake the agent after this many seconds of no output/progress (min 30, resets on activity). Use for long jobs that may hang silently."
            },
            "justification": {
                "type": "string",
                "description": "Only when re-issuing a command the destructive gate refused; explain which user request it serves."
            }
        }
    })
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::destructive_command_refusal;

    #[test]
    fn scratch_log_and_backup_commands_do_not_require_justification() {
        let cwd = std::env::current_dir().ok();
        for command in [
            "cargo test --lib > \"$JCODE_SCRATCH_DIR/tests.log\" 2>&1",
            "git diff > \"${JCODE_SCRATCH_DIR}/before.patch\"",
            "env | grep JCODE",
            "command -v sudo && sudo -n true",
            "find /sys -type l -exec readlink {} \\;",
            "find /etc -type f -exec sed -n '1,10p' {} \\;",
        ] {
            assert!(
                destructive_command_refusal(command, None, cwd.clone()).is_none(),
                "{command}"
            );
        }
    }

    #[test]
    fn protected_writes_and_unknown_variables_remain_blocked() {
        for command in [
            "rm -rf /etc",
            "echo bad > /etc/passwd",
            "find /etc -type f -exec rm {} \\;",
            "echo test > \"$UNKNOWN/tests.log\"",
        ] {
            assert!(
                destructive_command_refusal(command, None, std::env::current_dir().ok()).is_some(),
                "{command}"
            );
        }
    }
}
