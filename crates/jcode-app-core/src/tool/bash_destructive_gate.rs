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
    let risk_ctx = jcode_command_risk::RiskContext::from_env(working_dir);
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
        "The Windows command to execute via cmd.exe. Use cmd.exe syntax and quoting, not Bash syntax. Double-quote individual arguments containing spaces. Invoke PowerShell explicitly when PowerShell syntax is needed. If you write a long-running script or loop for run_in_background=true, make it print progress lines. Preferred format: `JCODE_PROGRESS {json}`."
    } else {
        "The bash command to execute. If you write a long-running script or loop for run_in_background=true, make it print progress lines. Preferred format: `JCODE_PROGRESS {json}`. Put large temporary files and worktrees under `$JCODE_SCRATCH_DIR`, not `/tmp`, because `/tmp` may be RAM-backed."
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
                "description": "Timeout in MILLISECONDS (not seconds). Kills the command when exceeded and reports exit 124. e.g. 1000 = 1s, 600000 = 10min. Omit to run with no timeout; do NOT pass small values like 1000 for long jobs such as builds or test suites."
            },
            "run_in_background": {
                "type": "boolean",
                "description": format!("Run in background. {}", super::BACKGROUND_PROGRESS_GUIDANCE)
            },
            "notify": {
                "type": "boolean",
                "description": "Notify on completion."
            },
            "wake": {
                "type": "boolean",
                "description": "Wake on completion."
            },
            "justification": {
                "type": "string",
                "description": "Only for re-issuing a command the destructive-command gate refused. Explain which specific user request this command serves. Do not set it preemptively."
            }
        }
    })
}
