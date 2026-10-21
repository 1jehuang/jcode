//! Turns ChatGPT's own sandbox shell calls into local jcode tool calls.
//!
//! ChatGPT web models sometimes run commands with their built-in
//! `container.exec` tool instead of requesting a jcode tool. That container is
//! OpenAI's remote machine, so its view of files, disks and processes is not
//! the user's computer. No request field switches the tool off, so jcode
//! stops the turn at the first such call and runs the same command locally
//! through the `bash` tool, exactly as if the model had requested it. The
//! normal tool-approval and destructive-command gates still apply.

use super::{TOOL_CALL_END, TOOL_CALL_START};
use serde_json::{Value, json};

/// ChatGPT built-ins that execute in OpenAI's remote sandbox. Their view of
/// files, disks, processes and the environment is not the user's computer.
/// Web search results are real, so browsing is not treated as a sandbox.
pub(super) fn sandbox_tools(builtin: &[String]) -> Vec<String> {
    builtin
        .iter()
        .filter(|tool| tool.starts_with("container.") || tool.starts_with("python"))
        .cloned()
        .collect()
}

/// Correction appended to the prompt when the model answered from ChatGPT's
/// sandbox instead of requesting jcode tools.
pub(super) fn with_sandbox_correction(prompt: &str, sandbox: &[String]) -> String {
    format!(
        "{prompt}\n# Correction\n\nA previous attempt at this exact turn used ChatGPT's own sandbox tool(s): {}. That sandbox is OpenAI's remote machine, not the user's computer, so its results about files, disks, processes or the environment were wrong and were discarded. Do not use ChatGPT's built-in tools at all. Request the Jcode tools you need with {TOOL_CALL_START} envelopes instead.\n",
        sandbox.join(", ")
    )
}

/// Visible note when the model kept answering from its sandbox.
pub(super) fn sandbox_warning(sandbox: &[String]) -> String {
    format!(
        "> Note: ChatGPT answered using its own remote sandbox ({}), not this computer, even after a correction. Treat any statements about local files, disks or processes below as unverified.",
        sandbox.join(", ")
    )
}

/// One `container.exec` call observed in a ChatGPT turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SandboxCall {
    /// The raw call text, e.g. `bash -lc ls -la` or `sh -c uname -a`.
    pub(super) code: String,
    /// Visible text the model wrote before the call.
    pub(super) before: Vec<String>,
}

impl SandboxCall {
    /// Reads the `sandboxCall` field the page script reports, if any.
    pub(super) fn from_state(state: &Value) -> Option<Self> {
        let call = state.get("sandboxCall")?;
        let code = call.get("code")?.as_str()?.to_string();
        let before = call
            .get("before")
            .and_then(Value::as_array)
            .map(|texts| {
                texts
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_else(Vec::new);
        Some(Self { code, before })
    }
}

/// The shell script a `container.exec` call meant to run. ChatGPT shows the
/// call as its argv joined by spaces, so only `<shell> -c <script>` (or
/// `-lc`) is unambiguous: the rest of the text is the script, verbatim. Other
/// shapes lost their quoting and are not guessed at.
pub(super) fn sandbox_command(code: &str) -> Option<String> {
    let trimmed = code.trim();
    if let Ok(Value::Object(call)) = serde_json::from_str::<Value>(trimmed) {
        // Remote execution options cannot be mapped onto the user's machine.
        // Even a relative write changes meaning when workdir/env is dropped.
        if call.keys().any(|key| key != "cmd") {
            return None;
        }
        let argv: Vec<&str> = call
            .get("cmd")?
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<_>>()?;
        return match argv.as_slice() {
            [shell, "-c" | "-lc", script] if is_shell(shell) && !script.trim().is_empty() => {
                Some(script.trim().to_string())
            }
            _ => None,
        };
    }
    let (shell, rest) = trimmed.split_once(char::is_whitespace)?;
    let (flag, script) = rest.trim_start().split_once(char::is_whitespace)?;
    let script = script.trim();
    (is_shell(shell) && matches!(flag, "-c" | "-lc") && !script.is_empty())
        .then(|| script.to_string())
}

fn is_shell(program: &str) -> bool {
    matches!(program, "bash" | "/bin/bash" | "/usr/bin/bash")
}

/// The answer jcode acts on instead of the sandboxed one: the text the model
/// wrote before the call, then the same command as a local `bash` tool call.
/// None when the call holds no unambiguous shell script.
pub(super) fn redirected_response(call: &SandboxCall) -> Option<String> {
    let command = sandbox_command(&call.code)?;
    let envelope = json!({"name": "bash", "input": {
        "command": command,
        "intent": "Run the command ChatGPT tried in its remote sandbox on this computer instead",
    }});
    let before: Vec<&str> = call.before.iter().map(String::as_str).collect();
    Some(format!(
        "{}\n{TOOL_CALL_START}{envelope}{TOOL_CALL_END}",
        super::transcript_answer(&before)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_remote_execution_builtins_count_as_sandbox() {
        let used = vec![
            "container.exec".to_string(),
            "web.run".to_string(),
            "python".to_string(),
        ];
        assert_eq!(sandbox_tools(&used), vec!["container.exec", "python"]);
        assert!(sandbox_tools(&["web.run".to_string()]).is_empty());
    }

    #[test]
    fn sandbox_correction_keeps_the_prompt_and_names_the_tools() {
        let corrected = with_sandbox_correction("PROMPT", &["container.exec".to_string()]);
        assert!(corrected.starts_with("PROMPT"));
        assert!(corrected.contains("container.exec"));
        assert!(corrected.contains(TOOL_CALL_START));
        assert!(sandbox_warning(&["container.exec".to_string()]).contains("unverified"));
    }

    #[test]
    fn only_shell_scripts_are_taken_and_kept_verbatim() {
        assert_eq!(
            sandbox_command("bash -lc pwd; ls -la /tmp | head -80").as_deref(),
            Some("pwd; ls -la /tmp | head -80")
        );
        assert_eq!(
            sandbox_command("bash -c for i in 1 2; do\n  echo \"$i\"\ndone").as_deref(),
            Some("for i in 1 2; do\n  echo \"$i\"\ndone")
        );
        assert_eq!(
            sandbox_command(r#"{"cmd":["/bin/bash","-lc","git status --short"]}"#).as_deref(),
            Some("git status --short")
        );
        // Plain argv lost its quoting, so it is not guessed at.
        assert_eq!(sandbox_command("grep -r foo bar ."), None);
        assert_eq!(sandbox_command(r#"{"cmd":["ls","-la"]}"#), None);
        assert_eq!(sandbox_command("bash -lcx echo hi"), None);
        assert_eq!(sandbox_command("  bash -lc   "), None);
    }

    #[test]
    fn redirect_keeps_earlier_text_and_becomes_one_bash_call() {
        let call = SandboxCall {
            code: "bash -lc df -h | grep \"/home\"".to_string(),
            before: vec!["Checking disk space.".to_string()],
        };
        let response = redirected_response(&call).unwrap();
        let parsed = super::super::parse_response(&response).unwrap();
        assert_eq!(parsed.text, "Checking disk space.");
        assert_eq!(parsed.calls.len(), 1);
        assert_eq!(parsed.calls[0].name, "bash");
        assert_eq!(parsed.calls[0].input["command"], "df -h | grep \"/home\"");
        assert!(
            parsed.calls[0].input["intent"]
                .as_str()
                .unwrap()
                .contains("this computer")
        );
    }

    #[test]
    fn contextual_or_non_bash_sandbox_calls_are_not_redirected() {
        for code in [
            r#"{"cmd":["bash","-lc","printf new > result.txt"],"workdir":"/tmp/scratch"}"#,
            r#"{"cmd":["bash","-lc","echo $TARGET"],"env":{"TARGET":"remote"}}"#,
            r#"{"cmd":["bash","-lc","sleep 10"],"timeout":1}"#,
            "sh -c echo hi",
            "zsh -lc echo hi",
            "dash -c echo hi",
            "/untrusted/bash -lc echo hi",
        ] {
            let call = SandboxCall {
                code: code.to_string(),
                before: Vec::new(),
            };
            assert_eq!(redirected_response(&call), None, "redirected: {code}");
        }
        assert_eq!(sandbox_command("bash -lc pwd").as_deref(), Some("pwd"));
    }

    #[test]
    fn calls_without_a_shell_script_are_not_redirected() {
        let empty = SandboxCall {
            code: "bash -lc".to_string(),
            before: Vec::new(),
        };
        assert_eq!(redirected_response(&empty), None);
    }

    #[test]
    fn page_state_is_parsed_with_text_before_the_call() {
        let state = json!({"sandboxCall": {"tool": "container.exec", "code": "sh -c ls", "before": ["Checking.", 3]}});
        assert_eq!(
            SandboxCall::from_state(&state),
            Some(SandboxCall {
                code: "sh -c ls".to_string(),
                before: vec!["Checking.".to_string()],
            })
        );
        assert_eq!(SandboxCall::from_state(&json!({"sandboxCall": null})), None);
        assert_eq!(SandboxCall::from_state(&json!({})), None);
    }
}
