use super::*;

#[test]
fn payload_json_includes_event_and_lowercased_fields() {
    let event = HookEvent::new("turn_end")
        .session_id("ses_x")
        .cwd("/work")
        .field("STATUS", "ok")
        .field("DURATION_MS", "1200");
    let payload: serde_json::Value = serde_json::from_str(&payload_json(&event)).unwrap();
    assert_eq!(payload["event"], "turn_end");
    assert_eq!(payload["session_id"], "ses_x");
    assert_eq!(payload["cwd"], "/work");
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["duration_ms"], "1200");
}

#[test]
fn truncate_bytes_respects_char_boundaries() {
    let text = "héllo wörld";
    let truncated = truncate_bytes(text, 3);
    assert!(truncated.len() <= 3);
    assert!(text.starts_with(truncated));
    assert_eq!(truncate_bytes("short", 100), "short");
}

#[cfg(unix)]
fn write_executable_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod script");
    path
}

#[cfg(unix)]
fn gate_test_config(hook: &str, timeout_ms: u64) -> impl Drop + use<> {
    struct EnvReset(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for EnvReset {
        fn drop(&mut self) {
            for (key, previous) in self.0.drain(..) {
                match previous {
                    Some(value) => crate::env::set_var(key, value),
                    None => crate::env::remove_var(key),
                }
            }
        }
    }
    let reset = EnvReset(vec![
        (
            "JCODE_HOOK_PRE_TOOL",
            std::env::var_os("JCODE_HOOK_PRE_TOOL"),
        ),
        (
            "JCODE_HOOK_PRE_TOOL_TIMEOUT_MS",
            std::env::var_os("JCODE_HOOK_PRE_TOOL_TIMEOUT_MS"),
        ),
    ]);
    crate::env::set_var("JCODE_HOOK_PRE_TOOL", hook);
    crate::env::set_var("JCODE_HOOK_PRE_TOOL_TIMEOUT_MS", timeout_ms.to_string());
    reset
}

#[cfg(unix)]
fn transform_test_config(hook: &str, timeout_ms: u64) -> impl Drop + use<> {
    struct EnvReset(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for EnvReset {
        fn drop(&mut self) {
            for (key, previous) in self.0.drain(..) {
                match previous {
                    Some(value) => crate::env::set_var(key, value),
                    None => crate::env::remove_var(key),
                }
            }
        }
    }
    let reset = EnvReset(vec![
        (
            "JCODE_HOOK_PRE_TOOL_TRANSFORM",
            std::env::var_os("JCODE_HOOK_PRE_TOOL_TRANSFORM"),
        ),
        (
            "JCODE_HOOK_PRE_TOOL_TRANSFORM_TIMEOUT_MS",
            std::env::var_os("JCODE_HOOK_PRE_TOOL_TRANSFORM_TIMEOUT_MS"),
        ),
    ]);
    crate::env::set_var("JCODE_HOOK_PRE_TOOL_TRANSFORM", hook);
    crate::env::set_var(
        "JCODE_HOOK_PRE_TOOL_TRANSFORM_TIMEOUT_MS",
        timeout_ms.to_string(),
    );
    reset
}

#[cfg(unix)]
#[tokio::test]
async fn tool_transformers_compose_json_replacements() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let first = write_executable_script(
        temp.path(),
        "first.sh",
        "#!/bin/sh\ncat >/dev/null\nprintf '%s' '{\"command\":\"first\"}'\n",
    );
    let second = write_executable_script(
        temp.path(),
        "second.sh",
        "#!/bin/sh\ninput=$(cat)\n[ \"$input\" = '{\"command\":\"first\"}' ] || exit 1\nprintf '%s' '{\"command\":\"second\"}'\n",
    );
    let commands = serde_json::to_string(&vec![
        first.to_string_lossy().into_owned(),
        second.to_string_lossy().into_owned(),
    ])
    .expect("serialize transformer commands");
    let _env = transform_test_config(&commands, 500);

    let output = transform_tool_input(
        "ses_transform",
        Some("/work"),
        "bash",
        serde_json::json!({"command": "original"}),
    )
    .await;
    assert_eq!(output, serde_json::json!({"command": "second"}));
}

#[cfg(unix)]
#[tokio::test]
async fn tool_transformer_fails_open_for_invalid_output_and_timeout() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let invalid = write_executable_script(
        temp.path(),
        "invalid.sh",
        "#!/bin/sh\ncat >/dev/null\necho nope\n",
    );
    let hang = write_executable_script(temp.path(), "hang.sh", "#!/bin/sh\nsleep 30\n");
    let commands = serde_json::to_string(&vec![
        invalid.to_string_lossy().into_owned(),
        hang.to_string_lossy().into_owned(),
    ])
    .expect("serialize transformer commands");
    let _env = transform_test_config(&commands, 50);
    let input = serde_json::json!({"command": "original"});

    assert_eq!(
        transform_tool_input("ses_transform", None, "bash", input.clone()).await,
        input
    );
}

#[cfg(unix)]
#[tokio::test]
async fn tool_transformer_timeout_includes_stdin_delivery() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    // Keep stdin open but never consume it, causing an oversized write to
    // block once the pipe fills.
    let hang = write_executable_script(temp.path(), "hang.sh", "#!/bin/sh\nsleep 5\n");
    let _env = transform_test_config(&hang.to_string_lossy(), 100);
    let input = serde_json::json!({"command": "x".repeat(1024 * 1024)});

    let started = std::time::Instant::now();
    assert_eq!(
        transform_tool_input("ses_transform", None, "bash", input.clone()).await,
        input
    );
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "stdin delivery exceeded the transformer timeout: {:?}",
        started.elapsed()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn pre_tool_gate_allows_on_exit_zero_and_blocks_on_exit_two() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");

    // Blocking hook: reads stdin, writes a reason to stderr, exits 2.
    let block = write_executable_script(
        temp.path(),
        "block.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"dangerous tool: $JCODE_HOOK_TOOL_NAME\" >&2\nexit 2\n",
    );
    {
        let _env = gate_test_config(&block.to_string_lossy(), 5000);
        let decision = run_pre_tool_gate("ses_g", None, "bash", r#"{"command":"rm -rf /"}"#)
            .await
            .decision;
        assert_eq!(
            decision,
            GateDecision::Block {
                reason: "dangerous tool: bash".to_string()
            }
        );
    }

    // Allowing hook: exit 0.
    let allow = write_executable_script(temp.path(), "allow.sh", "#!/bin/sh\nexit 0\n");
    {
        let _env = gate_test_config(&allow.to_string_lossy(), 5000);
        let decision = run_pre_tool_gate("ses_g", None, "read", "{}")
            .await
            .decision;
        assert_eq!(decision, GateDecision::Allow);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn pre_tool_gate_runs_every_configured_command_and_preserves_first_block() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let first_marker = temp.path().join("first-ran.txt");
    let final_marker = temp.path().join("final-ran.txt");
    let allow = write_executable_script(
        temp.path(),
        "first-allow.sh",
        &format!(
            "#!/bin/sh\nprintf ran > {}\nexit 0\n",
            crate::terminal_launch::sh_escape(&first_marker.to_string_lossy())
        ),
    );
    let block = write_executable_script(
        temp.path(),
        "second-block.sh",
        "#!/bin/sh\necho 'blocked by second policy' >&2\nexit 2\n",
    );
    let final_allow = write_executable_script(
        temp.path(),
        "third-allow.sh",
        &format!(
            "#!/bin/sh\nprintf ran > {}\nexit 0\n",
            crate::terminal_launch::sh_escape(&final_marker.to_string_lossy())
        ),
    );
    let commands = serde_json::to_string(&vec![
        allow.to_string_lossy().into_owned(),
        block.to_string_lossy().into_owned(),
        final_allow.to_string_lossy().into_owned(),
    ])
    .expect("serialize hook command array");
    let _env = gate_test_config(&commands, 5000);

    let decision = run_pre_tool_gate("ses_multi", None, "bash", "{}")
        .await
        .decision;

    assert_eq!(
        decision,
        GateDecision::Block {
            reason: "blocked by second policy".to_string()
        }
    );
    assert_eq!(
        std::fs::read_to_string(first_marker).expect("first policy should execute"),
        "ran"
    );
    assert_eq!(
        std::fs::read_to_string(final_marker).expect("later policies should still execute"),
        "ran"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn pre_tool_gate_fails_open_on_timeout_and_odd_exits() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");

    // Hook that hangs: must fail open after the timeout.
    let hang = write_executable_script(temp.path(), "hang.sh", "#!/bin/sh\nsleep 30\n");
    {
        let _env = gate_test_config(&hang.to_string_lossy(), 200);
        let decision = run_pre_tool_gate("ses_g", None, "bash", "{}")
            .await
            .decision;
        assert_eq!(decision, GateDecision::Allow);
    }

    // Hook with an unexpected exit code: fail open.
    let odd = write_executable_script(temp.path(), "odd.sh", "#!/bin/sh\nexit 7\n");
    {
        let _env = gate_test_config(&odd.to_string_lossy(), 5000);
        let decision = run_pre_tool_gate("ses_g", None, "bash", "{}")
            .await
            .decision;
        assert_eq!(decision, GateDecision::Allow);
    }

    // Missing hook binary: fail open.
    {
        let _env = gate_test_config("/nonexistent/hook-binary", 5000);
        let decision = run_pre_tool_gate("ses_g", None, "bash", "{}")
            .await
            .decision;
        assert_eq!(decision, GateDecision::Allow);
    }
}

#[test]
fn gate_stdout_json_decisions_parse_codex_shapes() {
    assert_eq!(parse_gate_stdout(""), CommandVerdict::default());
    assert_eq!(parse_gate_stdout("looks fine\n"), CommandVerdict::default());
    assert_eq!(parse_gate_stdout("{not json"), CommandVerdict::default());

    let deny = parse_gate_stdout(
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"no rm"}}"#,
    );
    assert_eq!(deny.block.as_deref(), Some("no rm"));

    let legacy = parse_gate_stdout(r#"{"decision":"block","reason":"legacy no"}"#);
    assert_eq!(legacy.block.as_deref(), Some("legacy no"));

    let ask = parse_gate_stdout(r#"{"hookSpecificOutput":{"permissionDecision":"ask"}}"#);
    assert_eq!(
        ask.block.as_deref(),
        Some("requires user approval (pre_tool hook asked)")
    );

    let rewrite = parse_gate_stdout(
        r#"{"systemMessage":"note","hookSpecificOutput":{"permissionDecision":"allow","updatedInput":{"command":"ls"},"additionalContext":"rewrote"}}"#,
    );
    assert_eq!(rewrite.block, None);
    assert_eq!(
        rewrite.updated_input,
        Some(serde_json::json!({"command":"ls"}))
    );
    assert_eq!(rewrite.context.as_deref(), Some("rewrote\nnote"));

    // Non-object updatedInput is ignored.
    let bad = parse_gate_stdout(r#"{"hookSpecificOutput":{"updatedInput":"ls"}}"#);
    assert_eq!(bad.updated_input, None);
}

#[cfg(unix)]
#[tokio::test]
async fn pre_tool_gate_json_rewrites_chain_and_deny_drops_rewrite() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let rewrite = write_executable_script(
        temp.path(),
        "rewrite.sh",
        "#!/bin/sh\ncat > /dev/null\nprintf '%s' '{\"hookSpecificOutput\":{\"updatedInput\":{\"command\":\"safe\"},\"additionalContext\":\"rewritten\"}}'\n",
    );
    let check = write_executable_script(
        temp.path(),
        "check.sh",
        "#!/bin/sh\ninput=$(cat)\n[ \"$input\" = '{\"command\":\"safe\"}' ] || exit 2\nexit 0\n",
    );
    let commands = serde_json::to_string(&vec![
        rewrite.to_string_lossy().into_owned(),
        check.to_string_lossy().into_owned(),
    ])
    .expect("serialize");
    {
        let _env = gate_test_config(&commands, 5000);
        let outcome = run_pre_tool_gate("ses_j", None, "bash", r#"{"command":"rm -rf /"}"#).await;
        assert_eq!(outcome.decision, GateDecision::Allow);
        assert_eq!(
            outcome.updated_input,
            Some(serde_json::json!({"command":"safe"}))
        );
        assert_eq!(outcome.additional_context, vec!["rewritten".to_string()]);
    }

    let deny = write_executable_script(
        temp.path(),
        "deny.sh",
        "#!/bin/sh\ncat > /dev/null\nprintf '%s' '{\"hookSpecificOutput\":{\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"json says no\"}}'\n",
    );
    let commands = serde_json::to_string(&vec![
        rewrite.to_string_lossy().into_owned(),
        deny.to_string_lossy().into_owned(),
    ])
    .expect("serialize");
    let _env = gate_test_config(&commands, 5000);
    let outcome = run_pre_tool_gate("ses_j", None, "bash", "{}").await;
    assert_eq!(
        outcome.decision,
        GateDecision::Block {
            reason: "json says no".to_string()
        }
    );
    assert_eq!(outcome.updated_input, None);
}

#[cfg(unix)]
#[tokio::test]
async fn pre_tool_gate_receives_input_on_stdin() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let record = temp.path().join("stdin.txt");
    let script = write_executable_script(
        temp.path(),
        "record.sh",
        &format!(
            "#!/bin/sh\ncat > {}\nexit 0\n",
            crate::terminal_launch::sh_escape(&record.to_string_lossy())
        ),
    );
    let _env = gate_test_config(&script.to_string_lossy(), 5000);
    let input = r#"{"file_path":"/tmp/x","content":"hello"}"#;
    let decision = run_pre_tool_gate("ses_g", None, "write", input)
        .await
        .decision;
    assert_eq!(decision, GateDecision::Allow);
    let recorded = std::fs::read_to_string(&record).expect("stdin should be recorded");
    assert_eq!(recorded, input);
}

#[cfg(unix)]
#[test]
fn observer_dispatch_runs_hook_with_event_env() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let record = temp.path().join("event.txt");
    let script = write_executable_script(
        temp.path(),
        "observe.sh",
        &format!(
            "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$JCODE_HOOK_EVENT\" \"$JCODE_HOOK_SESSION_ID\" \"$JCODE_HOOK_STATUS\" \"$JCODE_HOOKS_DISABLED\" > {}\n",
            crate::terminal_launch::sh_escape(&record.to_string_lossy())
        ),
    );

    let prev = std::env::var_os("JCODE_HOOK_TURN_END");
    crate::env::set_var("JCODE_HOOK_TURN_END", script.to_string_lossy().to_string());

    dispatch_observer(
        HookEvent::new("turn_end")
            .session_id("ses_obs")
            .field("STATUS", "ok"),
    );

    let mut recorded = String::new();
    for _ in 0..100 {
        if let Ok(data) = std::fs::read_to_string(&record)
            && !data.is_empty()
        {
            recorded = data;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    match prev {
        Some(value) => crate::env::set_var("JCODE_HOOK_TURN_END", value),
        None => crate::env::remove_var("JCODE_HOOK_TURN_END"),
    }
    assert_eq!(recorded, "turn_end|ses_obs|ok|1");
}

#[cfg(target_os = "linux")]
#[test]
fn observer_dispatch_reaps_completed_hook() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let record = temp.path().join("pid.txt");
    let script = write_executable_script(
        temp.path(),
        "record-pid.sh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > {}\n",
            crate::terminal_launch::sh_escape(&record.to_string_lossy())
        ),
    );

    let previous = std::env::var_os("JCODE_HOOK_TURN_END");
    crate::env::set_var("JCODE_HOOK_TURN_END", script.to_string_lossy().to_string());
    dispatch_observer(HookEvent::new("turn_end").session_id("ses_reap"));

    let mut pid: Option<u32> = None;
    for _ in 0..100 {
        pid = std::fs::read_to_string(&record)
            .ok()
            .and_then(|value| value.strip_suffix('\n').and_then(|pid| pid.parse().ok()));
        if pid.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOOK_TURN_END", value),
        None => crate::env::remove_var("JCODE_HOOK_TURN_END"),
    }

    let pid = pid.expect("hook should record its pid");
    let process = std::path::PathBuf::from(format!("/proc/{pid}"));
    for _ in 0..100 {
        if !process.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("completed hook process {pid} was not reaped");
}

#[cfg(unix)]
#[test]
fn observer_dispatch_runs_each_configured_command() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let first_record = temp.path().join("first.txt");
    let second_record = temp.path().join("second.txt");
    let first = write_executable_script(
        temp.path(),
        "first.sh",
        &format!(
            "#!/bin/sh\nprintf first > {}\n",
            crate::terminal_launch::sh_escape(&first_record.to_string_lossy())
        ),
    );
    let second = write_executable_script(
        temp.path(),
        "second.sh",
        &format!(
            "#!/bin/sh\nprintf second > {}\n",
            crate::terminal_launch::sh_escape(&second_record.to_string_lossy())
        ),
    );
    let previous = std::env::var_os("JCODE_HOOK_SESSION_START");
    crate::env::set_var(
        "JCODE_HOOK_SESSION_START",
        format!(
            "[{:?}, {:?}]",
            first.to_string_lossy(),
            second.to_string_lossy()
        ),
    );

    dispatch_observer(HookEvent::new("session_start").session_id("ses_multi"));
    for _ in 0..100 {
        if first_record.exists() && second_record.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    match previous {
        Some(value) => crate::env::set_var("JCODE_HOOK_SESSION_START", value),
        None => crate::env::remove_var("JCODE_HOOK_SESSION_START"),
    }
    assert_eq!(std::fs::read_to_string(first_record).unwrap(), "first");
    assert_eq!(std::fs::read_to_string(second_record).unwrap(), "second");
}

#[tokio::test]
async fn concurrent_client_terminal_environments_remain_isolated() {
    async fn pane_id(env: Vec<(String, String)>) -> Option<String> {
        with_client_terminal_env(env, async {
            let event = HookEvent::new("session_start");
            let command = build_hook_process("hook", &event).unwrap();
            command.get_envs().find_map(|(key, value)| {
                (key == "HERDR_PANE_ID")
                    .then(|| value.map(|value| value.to_string_lossy().into_owned()))
                    .flatten()
            })
        })
        .await
    }

    let (left, right) = tokio::join!(
        pane_id(vec![("HERDR_PANE_ID".to_string(), "pane-left".to_string())]),
        pane_id(vec![(
            "HERDR_PANE_ID".to_string(),
            "pane-right".to_string()
        )]),
    );
    assert_eq!(left.as_deref(), Some("pane-left"));
    assert_eq!(right.as_deref(), Some("pane-right"));
}

#[cfg(unix)]
#[tokio::test]
async fn hook_process_replaces_daemon_terminal_env_with_client_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let script = write_executable_script(
        temp.path(),
        "env.sh",
        "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$TMUX_PANE\" \"$HERDR_PANE_ID\" \"$JCODE_CLIENT_TMUX_PANE\" \"$JCODE_CLIENT_HERDR_PANE_ID\"\n",
    );
    let previous_tmux = std::env::var_os("TMUX_PANE");
    let previous_herdr = std::env::var_os("HERDR_PANE_ID");
    crate::env::set_var("TMUX_PANE", "daemon-pane");
    crate::env::set_var("HERDR_PANE_ID", "daemon-herdr");

    let run_for_pane = |tmux: &'static str, herdr: &'static str| {
        let script = script.clone();
        with_client_terminal_env(
            vec![
                ("TMUX_PANE".to_string(), tmux.to_string()),
                ("HERDR_PANE_ID".to_string(), herdr.to_string()),
            ],
            async move {
                tokio::task::yield_now().await;
                build_hook_process(&script.to_string_lossy(), &HookEvent::new("turn_start"))
                    .expect("hook command")
                    .output()
                    .expect("run hook")
            },
        )
    };
    let (first_output, second_output) = tokio::join!(
        run_for_pane("client-pane-a", "herdr-pane-a"),
        run_for_pane("client-pane-b", "herdr-pane-b")
    );

    match previous_tmux {
        Some(value) => crate::env::set_var("TMUX_PANE", value),
        None => crate::env::remove_var("TMUX_PANE"),
    }
    match previous_herdr {
        Some(value) => crate::env::set_var("HERDR_PANE_ID", value),
        None => crate::env::remove_var("HERDR_PANE_ID"),
    }
    assert!(first_output.status.success());
    assert!(second_output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&first_output.stdout),
        "client-pane-a|herdr-pane-a|client-pane-a|herdr-pane-a"
    );
    assert_eq!(
        String::from_utf8_lossy(&second_output.stdout),
        "client-pane-b|herdr-pane-b|client-pane-b|herdr-pane-b"
    );
}
