#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::*;
use crate::bus::{BackgroundTaskProgressSource, BackgroundTaskStatus};
use crate::tool::StdinInputRequest;
use crate::tool::bash::{
    BashTool, ProgressLineUpdate, parse_heuristic_progress, parse_progress_line,
};
use serde_json::json;
use tokio::sync::mpsc;

type SessionEnvLaunchGate = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);
static SESSION_ENV_LAUNCH_GATES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, SessionEnvLaunchGate>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

pub(super) async fn wait_for_session_env_launch_gate(session_id: &str) {
    let gate = SESSION_ENV_LAUNCH_GATES.lock().unwrap().remove(session_id);
    if let Some((reached, release)) = gate {
        let _ = reached.send(());
        let _ = release.await;
    }
}

#[tokio::test]
async fn session_env_foreground_sessions_are_isolated() {
    use crate::session_env::{remove_session_env, set_session_env};
    for (sid, value) in [("env-bash-a", "proxy-a"), ("env-bash-b", "proxy-b")] {
        set_session_env(
            sid,
            std::collections::HashMap::from([("HTTPS_PROXY".into(), value.into())]),
        )
        .unwrap();
    }
    let mut a = make_ctx(None);
    a.session_id = "env-bash-a".into();
    let mut b = make_ctx(None);
    b.session_id = "env-bash-b".into();
    let tool = BashTool::new();
    let command = json!({"command": "printf '%s:%s' \"$HTTPS_PROXY\" \"$JCODE_SESSION_ID\""});
    let (a, b) = tokio::join!(tool.execute(command.clone(), a), tool.execute(command, b));
    let a = a.unwrap().output;
    let b = b.unwrap().output;
    assert!(a.contains("proxy-a:env-bash-a"), "{a}");
    assert!(!a.contains("proxy-b"));
    assert!(b.contains("proxy-b:env-bash-b"), "{b}");
    assert!(!b.contains("proxy-a"));
    remove_session_env("env-bash-a");
    remove_session_env("env-bash-b");
}

#[cfg(unix)]
#[tokio::test]
async fn session_env_reload_persistable_command_receives_overrides() {
    let mut ctx = make_agent_ctx(jcode_agent_runtime::InterruptSignal::new());
    ctx.session_id = "env-bash-detached".into();
    crate::session_env::set_session_env(
        &ctx.session_id,
        std::collections::HashMap::from([("HTTPS_PROXY".into(), "proxy-detached".into())]),
    )
    .unwrap();
    let result = BashTool::new()
        .execute(
            json!({"command": "printf '%s:%s' \"$HTTPS_PROXY\" \"$JCODE_SESSION_ID\""}),
            ctx,
        )
        .await
        .unwrap();
    assert!(
        result.output.contains("proxy-detached:env-bash-detached"),
        "{}",
        result.output
    );
    crate::session_env::remove_session_env("env-bash-detached");
}

#[tokio::test]
async fn session_env_updates_affect_only_new_launches_and_invalid_updates_keep_bash_working() {
    use crate::session_env::{remove_session_env, set_session_env};
    let sid = "env-bash-updates";
    let mut ctx = make_ctx(None);
    ctx.session_id = sid.into();
    let tool = BashTool::new();
    let input = json!({"command": "printf '%s' \"${SESSION_ENV_REGRESSION_PROBE-unset}\""});
    for value in ["first", "second"] {
        set_session_env(
            sid,
            std::collections::HashMap::from([(
                "SESSION_ENV_REGRESSION_PROBE".into(),
                value.into(),
            )]),
        )
        .unwrap();
        assert!(
            set_session_env(
                sid,
                std::collections::HashMap::from([(
                    "SESSION_ENV_REGRESSION_PROBE".into(),
                    "invalid\0value".into()
                ),])
            )
            .is_err()
        );
        let result = tool.execute(input.clone(), ctx.clone()).await.unwrap();
        assert!(result.output.contains(value), "{}", result.output);
    }
    set_session_env(sid, std::collections::HashMap::new()).unwrap();
    let result = tool.execute(input, ctx).await.unwrap();
    assert!(result.output.contains("unset"), "{}", result.output);
    remove_session_env(sid);
}

#[tokio::test]
async fn session_env_background_snapshots_before_deferred_launch() {
    use crate::session_env::{remove_session_env, set_session_env};
    for clear in [false, true] {
        let sid = format!("env-bash-race-{clear}");
        let original =
            std::collections::HashMap::from([("HTTPS_PROXY".into(), "proxy-original".into())]);
        set_session_env(&sid, original).unwrap();
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        SESSION_ENV_LAUNCH_GATES
            .lock()
            .unwrap()
            .insert(sid.clone(), (reached_tx, release_rx));
        let mut ctx = make_ctx(None);
        ctx.session_id = sid.clone();
        let result = BashTool::new()
            .execute(
                json!({
                    "command": "printf '%s:%s\\n' \"$HTTPS_PROXY\" \"$JCODE_SESSION_ID\"",
                    "run_in_background": true, "notify": false,
                }),
                ctx,
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), reached_rx)
            .await
            .unwrap()
            .unwrap();
        if clear {
            set_session_env(&sid, std::collections::HashMap::new()).unwrap();
        } else {
            set_session_env(
                &sid,
                std::collections::HashMap::from([("HTTPS_PROXY".into(), "proxy-replaced".into())]),
            )
            .unwrap();
        }
        // Teardown must not erase the snapshot of this already-accepted job.
        remove_session_env(&sid);
        release_tx.send(()).unwrap();
        let metadata = result.metadata.unwrap();
        let task_id = metadata["task_id"].as_str().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let status = crate::background::global().status(task_id).await.unwrap();
                if status.status != BackgroundTaskStatus::Running {
                    assert_eq!(status.status, BackgroundTaskStatus::Completed);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let output = crate::background::global().output(task_id).await.unwrap();
        assert!(
            output.contains(&format!("proxy-original:{sid}")),
            "{output}"
        );
        assert!(!output.contains("proxy-replaced"));
    }
}

#[cfg(not(windows))]
#[test]
fn bash_definition_prefers_repository_search_tools() {
    let definition = BashTool::new().to_definition();
    assert_eq!(definition.name, "bash");
    assert!(definition.description.contains("Prefer agentgrep/rg"));
    assert!(definition.description.contains("hidden/ignored"));
    assert!(definition.description.contains("--hidden --no-ignore"));
}

#[test]
fn bash_optional_nulls_use_omitted_defaults() {
    let omitted: BashInput = serde_json::from_value(json!({"command": "printf ok"})).unwrap();
    let nulls: BashInput = serde_json::from_value(json!({
        "command": "printf ok",
        "intent": null,
        "accept_large_output": null,
        "timeout": null,
        "run_in_background": null,
        "notify": null,
        "wake": null,
        "stall_wake_seconds": null,
        "justification": null
    }))
    .unwrap();
    assert!(nulls.notify);
    assert!(!nulls.wake);
    assert_eq!(nulls.notify, omitted.notify);
    assert_eq!(nulls.wake, omitted.wake);
    assert_eq!(nulls.run_in_background, omitted.run_in_background);
    assert_eq!(nulls.timeout, omitted.timeout);
    assert_eq!(nulls.stall_wake_seconds, omitted.stall_wake_seconds);
    assert_eq!(nulls.justification, omitted.justification);
    assert_eq!(nulls.intent, omitted.intent);
}

#[test]
fn bash_optional_booleans_preserve_explicit_values() {
    for value in [false, true] {
        let input: BashInput = serde_json::from_value(json!({
            "command": "printf ok", "notify": value, "wake": value,
            "run_in_background": value
        }))
        .unwrap();
        assert_eq!(input.notify, value);
        assert_eq!(input.wake, value);
        assert_eq!(input.run_in_background, Some(value));
    }
}

#[test]
fn bash_optional_booleans_still_reject_invalid_types() {
    for key in ["notify", "wake", "run_in_background"] {
        for value in [json!("false"), json!(0), json!([]), json!({})] {
            let mut input = json!({"command": "printf ok"});
            input[key] = value;
            assert!(serde_json::from_value::<BashInput>(input).is_err());
        }
    }
    assert!(serde_json::from_value::<BashInput>(json!({"command": null})).is_err());
    assert!(serde_json::from_value::<BashInput>(json!({})).is_err());
}

#[tokio::test]
async fn bash_executes_with_null_optional_arguments() {
    let output = BashTool::new()
        .execute(
            json!({
                "command": "printf null-options-ok",
                "notify": null, "wake": null, "run_in_background": null,
                "timeout": null, "stall_wake_seconds": null, "justification": null,
                "intent": null, "accept_large_output": null
            }),
            make_ctx(None),
        )
        .await
        .unwrap();
    assert!(output.output.contains("null-options-ok"));
}

#[test]
fn repository_commands_export_a_logged_cargo_function() {
    let fixture = tempfile::tempdir().expect("repo fixture");
    let repo = fixture.path();
    std::fs::create_dir(repo.join(".git")).expect("git marker");
    std::fs::create_dir(repo.join("scripts")).expect("scripts directory");
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"jcode\"\n").expect("manifest");
    std::fs::write(repo.join("scripts/dev_cargo.sh"), "#!/bin/sh\n").expect("wrapper");
    let wrapped = wrap_repo_cargo_commands("cargo test -p demo && echo done", Some(repo))
        .expect("jcode repository has dev_cargo.sh");

    assert!(wrapped.contains("export JCODE_DEV_CARGO_SCRIPT="));
    assert!(wrapped.contains("JCODE_IN_DEV_CARGO=1 \"$JCODE_DEV_CARGO_SCRIPT\" \"$@\""));
    assert!(wrapped.contains("export -f cargo"));
    assert!(wrapped.ends_with("cargo test -p demo && echo done"));
}

#[test]
fn cargo_routing_is_limited_to_the_jcode_repository() {
    assert!(wrap_repo_cargo_commands("cargo test", Some(std::path::Path::new("/"))).is_none());
    assert!(wrap_repo_cargo_commands("cargo test", None).is_none());
}

#[test]
fn cargo_wrapper_path_is_shell_quoted() {
    assert_eq!(shell_single_quote("a'b"), "'a'\"'\"'b'");
}

#[tokio::test]
async fn background_command_stdin_is_null() {
    let mut command =
        build_shell_command("if IFS= read -r _; then printf inherited; else printf eof; fi");

    // Start with a readable pipe so this test does not depend on, or modify, the
    // test runner's process-wide stdin. Background configuration must replace it.
    command.stdin(Stdio::piped());
    configure_background_command_stdio(&mut command);

    let child = command.spawn().expect("background command should spawn");
    assert!(
        child.stdin.is_none(),
        "background commands must not retain a writable stdin pipe"
    );

    let output = tokio::time::timeout(Duration::from_secs(2), child.wait_with_output())
        .await
        .expect("background command should observe EOF instead of blocking")
        .expect("background command should exit cleanly");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "eof");
}

fn make_ctx(stdin_tx: Option<mpsc::UnboundedSender<StdinInputRequest>>) -> ToolContext {
    ToolContext {
        session_id: "test-session".to_string(),
        message_id: "test-msg".to_string(),
        tool_call_id: "test-call".to_string(),
        working_dir: Some(std::path::PathBuf::from("/tmp")),
        stdin_request_tx: stdin_tx,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

#[test]
fn cloud_environment_hydrates_commands_without_transcript_values() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir(repo.join("subdir")).unwrap();
    let original_home = std::env::var_os("JCODE_HOME");
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }
    let _restore = RestoreHome(original_home);
    crate::env::set_var("JCODE_HOME", &state);
    let name = "CLOUD_TEST_RESOURCE_TOKEN";
    let secret = "opaque-sentinel-do-not-return";
    let original_value = std::env::var_os(name);
    crate::cloud_environment::save_environment(&repo, name, secret).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for persistent in [false, true] {
            let mut ctx = make_ctx(None);
            ctx.working_dir = Some(repo.join("subdir"));
            if persistent {
                ctx.execution_mode = crate::tool::ToolExecutionMode::AgentTurn;
                ctx.graceful_shutdown_signal = Some(jcode_agent_runtime::InterruptSignal::new());
            }
            let result = BashTool::new().execute(json!({
                "command": "test \"${CLOUD_TEST_RESOURCE_TOKEN:+present}\" = present && printf resource-ready"
            }), ctx).await.unwrap();
            assert_eq!(result.output.lines().next(), Some("resource-ready"));
            assert!(!result.output.contains(secret));
        }
        let mut ctx = make_ctx(None);
        ctx.working_dir = Some(repo.join("subdir"));
        let result = BashTool::new().execute(json!({
            "command": "test \"${CLOUD_TEST_RESOURCE_TOKEN:+present}\" = present && printf resource-ready",
            "run_in_background": true,
            "notify": false,
            "wake": false
        }), ctx).await.unwrap();
        let id = result.metadata.as_ref().unwrap()["task_id"].as_str().unwrap();
        let mut completed = false;
        for _ in 0..100 {
            if let Some(status) = crate::background::global().status(id).await
                && status.status != BackgroundTaskStatus::Running
            {
                assert_eq!(status.status, BackgroundTaskStatus::Completed);
                completed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(completed, "background command did not finish");
        let output = crate::background::global().output(id).await.unwrap();
        assert!(output.contains("resource-ready"));
        assert!(!output.contains(secret));
    });
    assert_eq!(
        std::env::var_os(name),
        original_value,
        "daemon environment must not change"
    );
}

#[tokio::test]
async fn background_handoff_names_reason_task_and_productive_wait_without_bg_tutorial() {
    let result = BashTool::new()
        .execute(
            json!({"command": "printf done", "run_in_background": true, "wake": true}),
            make_ctx(None),
        )
        .await
        .expect("background command should start");
    assert!(
        result
            .output
            .contains("Command started in background as requested.")
    );
    assert!(result.output.contains(BACKGROUND_WAIT_GUIDANCE));
    assert!(
        result
            .output
            .contains("The agent will be woken when the task completes.")
    );
    assert!(!result.output.contains("action=\"wait\""));
    assert!(!result.output.contains("To check progress"));
    let metadata = result.metadata.expect("background metadata");
    let id = metadata["task_id"].as_str().expect("task id");
    assert!(result.output.contains(&format!("Task ID: {id}")));
    crate::background::global()
        .wait(id, Duration::from_secs(5), false)
        .await;
}

fn make_agent_ctx(signal: jcode_agent_runtime::InterruptSignal) -> ToolContext {
    ToolContext {
        session_id: "test-session".to_string(),
        message_id: "test-msg".to_string(),
        tool_call_id: "test-call-agent".to_string(),
        working_dir: Some(std::path::PathBuf::from("/tmp")),
        stdin_request_tx: None,
        graceful_shutdown_signal: Some(signal),
        execution_mode: crate::tool::ToolExecutionMode::AgentTurn,
    }
}

#[tokio::test]
async fn test_basic_command_no_stdin() {
    let tool = BashTool::new();
    let input = json!({"command": "echo hello"});
    let ctx = make_ctx(None);
    let result = tool.execute(input, ctx).await.unwrap();
    assert!(result.output.contains("hello"));
}

#[tokio::test]
async fn test_basic_command_with_unused_stdin_channel() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let tool = BashTool::new();
    let input = json!({"command": "echo world"});
    let ctx = make_ctx(Some(tx));
    let result = tool.execute(input, ctx).await.unwrap();
    assert!(result.output.contains("world"));
}

#[tokio::test]
async fn test_stdin_forwarding_single_line() {
    let (tx, mut rx) = mpsc::unbounded_channel::<StdinInputRequest>();
    let tool = BashTool::new();

    // "head -n1" reads one line from stdin and prints it
    let input = json!({"command": "head -n1", "timeout": 10000});
    let ctx = make_ctx(Some(tx));

    // Spawn the tool execution
    let tool_handle = tokio::spawn(async move { tool.execute(input, ctx).await });

    // Wait for the stdin request to arrive
    let req = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for stdin request")
        .expect("channel closed");

    assert!(req.request_id.starts_with("stdin-test-call-"));
    assert!(!req.is_password);

    // Send the response
    req.response_tx.send("test_input_line".to_string()).unwrap();

    // Wait for tool to finish
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), tool_handle)
        .await
        .expect("tool timed out")
        .expect("tool panicked")
        .expect("tool errored");

    assert!(
        result.output.contains("test_input_line"),
        "output should contain the input we sent: {}",
        result.output
    );
}

#[tokio::test]
async fn test_stdin_forwarding_multiple_lines() {
    let (tx, mut rx) = mpsc::unbounded_channel::<StdinInputRequest>();
    let tool = BashTool::new();

    // "head -n2" reads two lines
    let input = json!({"command": "head -n2", "timeout": 15000});
    let ctx = make_ctx(Some(tx));

    let tool_handle = tokio::spawn(async move { tool.execute(input, ctx).await });

    // First line
    let req1 = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for first stdin request")
        .expect("channel closed");
    assert!(
        req1.request_id.ends_with("-1"),
        "first request should end with -1: {}",
        req1.request_id
    );
    req1.response_tx.send("line_one".to_string()).unwrap();

    // Second line
    let req2 = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for second stdin request")
        .expect("channel closed");
    assert!(
        req2.request_id.ends_with("-2"),
        "second request should end with -2: {}",
        req2.request_id
    );
    req2.response_tx.send("line_two".to_string()).unwrap();

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), tool_handle)
        .await
        .expect("tool timed out")
        .expect("tool panicked")
        .expect("tool errored");

    assert!(
        result.output.contains("line_one"),
        "missing line_one in: {}",
        result.output
    );
    assert!(
        result.output.contains("line_two"),
        "missing line_two in: {}",
        result.output
    );
}

#[tokio::test]
async fn test_stdin_not_triggered_for_non_blocking_command() {
    let (tx, mut rx) = mpsc::unbounded_channel::<StdinInputRequest>();
    let tool = BashTool::new();

    // This command doesn't read stdin at all
    let input = json!({"command": "echo no_stdin_needed", "timeout": 5000});
    let ctx = make_ctx(Some(tx));

    let result = tool.execute(input, ctx).await.unwrap();
    assert!(result.output.contains("no_stdin_needed"));

    // No stdin request should have been sent
    assert!(
        rx.try_recv().is_err(),
        "no stdin request should be sent for a command that doesn't read stdin"
    );
}

#[tokio::test]
async fn test_command_timeout_with_stdin_channel() {
    let (tx, _rx) = mpsc::unbounded_channel::<StdinInputRequest>();
    let tool = BashTool::new();

    // `cat` blocks forever on stdin. With a short timeout and no stdin response,
    // the command should be promoted to the background (kept running), not killed
    // with an error.
    let input = json!({"command": "cat", "timeout": 1000});
    let ctx = make_ctx(Some(tx));

    let result = tool
        .execute(input, ctx)
        .await
        .expect("timeout should promote to background, not error");
    assert!(
        result.output.contains("continuing in background"),
        "output should explain background promotion: {}",
        result.output
    );
    let metadata = result.metadata.expect("expected background metadata");
    assert!(result.output.contains(BACKGROUND_WAIT_GUIDANCE));
    assert!(!result.output.contains("Use `bg`"));
    assert_eq!(metadata["background"], true);
    assert_eq!(metadata["timeout_promoted"], true);
    assert_eq!(metadata["foreground_timeout_ms"], 1000);

    // Clean up the still-running background task so it does not linger.
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task_id should be present");
    let _ = crate::background::global().cancel(task_id).await;
}

#[tokio::test]
async fn test_foreground_timeout_promotes_and_command_keeps_running() {
    let tool = BashTool::new();
    // No stdin channel and Direct mode -> plain foreground path. The command runs
    // longer than the timeout, so it should be promoted to background and keep
    // running to completion rather than being killed at the timeout.
    let input = json!({"command": "sleep 0.5; echo fg_promote_ok", "timeout": 100});
    let ctx = make_ctx(None);

    let result = tool
        .execute(input, ctx)
        .await
        .expect("timeout should promote the still-running command to background");
    assert!(
        result.output.contains("continuing in background"),
        "output should explain background promotion: {}",
        result.output
    );
    let metadata = result.metadata.expect("expected background metadata");
    assert_eq!(metadata["background"], true);
    assert_eq!(metadata["timeout_promoted"], true);
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task_id should be present")
        .to_string();

    // Promotion must retain an output artifact before returning, even though
    // this in-process path only fills it once the command finishes.
    assert!(
        crate::background::global().output(&task_id).await.is_some(),
        "output artifact should exist immediately after promotion"
    );

    // Wait for the promoted command to finish on its own.
    let mut final_status = None;
    for _ in 0..40 {
        if let Some(status) = crate::background::global().status(&task_id).await
            && status.status != BackgroundTaskStatus::Running
        {
            final_status = Some(status);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let status = final_status.expect("promoted background task should finish");
    assert_eq!(status.status, BackgroundTaskStatus::Completed);

    let output = crate::background::global()
        .output(&task_id)
        .await
        .expect("output should exist");
    assert!(
        output.contains("fg_promote_ok"),
        "command should have continued after foreground timeout: {output}"
    );
}

#[tokio::test]
async fn test_reload_persistable_bash_continues_in_background() {
    let tool = BashTool::new();
    let signal = jcode_agent_runtime::InterruptSignal::new();
    let ctx = make_agent_ctx(signal.clone());

    let signal_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        signal.fire();
    });

    let result = tool
        .execute(
            json!({"command": "sleep 1; echo reload_persist_ok", "timeout": 10000}),
            ctx,
        )
        .await
        .expect("reload-persistable command should succeed");
    signal_task.await.expect("signal task should complete");

    let metadata = result.metadata.expect("expected background metadata");
    assert_eq!(metadata["background"], true);
    assert_eq!(metadata["reload_persisted"], true);
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task_id should be present")
        .to_string();
    let output_file = std::path::PathBuf::from(
        metadata["output_file"]
            .as_str()
            .expect("output_file should be present"),
    );
    let status_file = std::path::PathBuf::from(
        metadata["status_file"]
            .as_str()
            .expect("status_file should be present"),
    );

    tokio::time::sleep(std::time::Duration::from_millis(1400)).await;

    let status = crate::background::global()
        .status(&task_id)
        .await
        .expect("status should exist");
    assert_eq!(status.status, BackgroundTaskStatus::Completed);
    let output = crate::background::global()
        .output(&task_id)
        .await
        .expect("output should exist");
    assert!(output.contains("reload_persist_ok"), "output was: {output}");

    let _ = tokio::fs::remove_file(output_file).await;
    let _ = tokio::fs::remove_file(status_file).await;
}

#[tokio::test]
async fn test_reload_persistable_bash_timeout_promotes_to_background() {
    let tool = BashTool::new();
    let signal = jcode_agent_runtime::InterruptSignal::new();
    let ctx = make_agent_ctx(signal);

    let result = tool
        .execute(
            json!({"command": "sleep 0.4; echo timeout_promote_ok", "timeout": 100}),
            ctx,
        )
        .await
        .expect("timeout should promote the still-running command to background");

    assert!(
        result.output.contains("continuing in background"),
        "output should explain background promotion: {}",
        result.output
    );
    assert!(
        result.output.contains("do not rerun"),
        "output should tell the agent not to rerun duplicate work: {}",
        result.output
    );

    let metadata = result.metadata.expect("expected background metadata");
    assert_eq!(metadata["background"], true);
    assert_eq!(metadata["timeout_promoted"], true);
    assert_eq!(metadata["foreground_timeout_ms"], 100);
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task_id should be present")
        .to_string();
    let output_file = std::path::PathBuf::from(
        metadata["output_file"]
            .as_str()
            .expect("output_file should be present"),
    );
    let status_file = std::path::PathBuf::from(
        metadata["status_file"]
            .as_str()
            .expect("status_file should be present"),
    );

    let initial_status = crate::background::global()
        .status(&task_id)
        .await
        .expect("status should exist");
    assert_eq!(initial_status.status, BackgroundTaskStatus::Running);

    let mut final_status = None;
    for _ in 0..40 {
        let status = crate::background::global()
            .status(&task_id)
            .await
            .expect("status should exist");
        if status.status != BackgroundTaskStatus::Running {
            final_status = Some(status);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let status = final_status.expect("promoted background task should finish");
    assert_eq!(status.status, BackgroundTaskStatus::Completed);
    assert_eq!(status.exit_code, Some(0));

    let output = crate::background::global()
        .output(&task_id)
        .await
        .expect("output should exist");
    assert!(
        output.contains("timeout_promote_ok"),
        "command should have continued after foreground timeout: {output}"
    );

    let _ = tokio::fs::remove_file(output_file).await;
    let _ = tokio::fs::remove_file(status_file).await;
}

#[tokio::test]
async fn test_stderr_captured_with_stdin() {
    let (tx, _rx) = mpsc::unbounded_channel::<StdinInputRequest>();
    let tool = BashTool::new();

    let input = json!({"command": "echo stderr_msg >&2", "timeout": 5000});
    let ctx = make_ctx(Some(tx));

    let result = tool.execute(input, ctx).await.unwrap();
    assert!(
        result.output.contains("stderr_msg"),
        "stderr should be captured: {}",
        result.output
    );
}

#[tokio::test]
async fn unanswered_stdin_request_closes_stdin_instead_of_hanging() {
    // Client receives the request but never answers, like the TUI does.
    let (tx, mut rx) = mpsc::unbounded_channel::<StdinInputRequest>();
    let held = tokio::spawn(async move {
        let mut pending = Vec::new();
        while let Some(req) = rx.recv().await {
            pending.push(req);
        }
    });
    let tool = BashTool::new();
    let input = json!({"command": "cat; echo got_eof", "timeout": 20000});
    let started = std::time::Instant::now();
    let result = tool.execute(input, make_ctx(Some(tx))).await.unwrap();
    assert!(result.output.contains("got_eof"), "{}", result.output);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    held.abort();
}

#[path = "bash_tests/progress_parsing.rs"]
mod progress_parsing;

#[tokio::test]
async fn test_background_command_progress_marker_updates_status_and_stays_out_of_output() {
    let tool = BashTool::new();
    let ctx = make_ctx(None);

    let result = tool
            .execute(
                json!({
                    "command": "printf '%s\n' 'JCODE_PROGRESS {\"current\":3,\"total\":10,\"unit\":\"steps\",\"message\":\"Building\"}'; sleep 0.1; echo done",
                    "run_in_background": true,
                    "notify": false,
                    "wake": false,
                }),
                ctx,
            )
            .await
            .expect("background command should start");

    let metadata = result.metadata.expect("expected metadata");
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task id should be present")
        .to_string();

    let mut saw_progress = false;
    // Wall-clock deadline: observing emitted progress depends on scheduler
    // latency, so a fixed 50-iteration budget starved under parallel load
    // (issue #593). The assertions inside stay exact.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let status = crate::background::global()
            .status(&task_id)
            .await
            .expect("status should exist");
        if let Some(progress) = status.progress {
            saw_progress = true;
            assert_eq!(progress.current, Some(3));
            assert_eq!(progress.total, Some(10));
            assert_eq!(progress.unit.as_deref(), Some("steps"));
            assert_eq!(progress.message.as_deref(), Some("Building"));
            assert_eq!(progress.percent, Some(30.0));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        saw_progress,
        "expected progress to be recorded for {task_id}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let output = crate::background::global()
        .output(&task_id)
        .await
        .expect("output should exist");
    assert!(output.contains("done"), "output was: {output}");
    assert!(
        !output.contains("JCODE_PROGRESS"),
        "progress marker should be hidden from output: {output}"
    );
}

#[tokio::test]
async fn test_background_command_ratio_output_updates_progress() {
    let tool = BashTool::new();
    let ctx = make_ctx(None);

    let result = tool
        .execute(
            json!({
                "command": "printf '%s\n' 'Running test 4/8 tests'; sleep 0.1; echo done",
                "run_in_background": true,
                "notify": false,
                "wake": false,
            }),
            ctx,
        )
        .await
        .expect("background command should start");

    let metadata = result.metadata.expect("expected metadata");
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task id should be present")
        .to_string();

    let mut saw_progress = false;
    // Wall-clock deadline: observing emitted progress depends on scheduler
    // latency, so a fixed 50-iteration budget starved under parallel load
    // (issue #593). The assertions inside stay exact.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let status = crate::background::global()
            .status(&task_id)
            .await
            .expect("status should exist");
        if let Some(progress) = status.progress {
            saw_progress = true;
            assert_eq!(progress.current, Some(4));
            assert_eq!(progress.total, Some(8));
            assert_eq!(progress.percent, Some(50.0));
            assert_eq!(progress.unit.as_deref(), Some("tests"));
            assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert!(
        saw_progress,
        "expected heuristic progress to be recorded for {task_id}"
    );
}

#[tokio::test]
async fn test_background_command_byte_ratio_output_updates_progress() {
    let tool = BashTool::new();
    let ctx = make_ctx(None);

    let result = tool
        .execute(
            json!({
                "command": "printf '%s\n' 'Downloaded 1.5/3.0 GiB'; sleep 0.1; echo done",
                "run_in_background": true,
                "notify": false,
                "wake": false,
            }),
            ctx,
        )
        .await
        .expect("background command should start");

    let metadata = result.metadata.expect("expected metadata");
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task id should be present")
        .to_string();

    let mut saw_progress = false;
    // Wall-clock deadline: observing emitted progress depends on scheduler
    // latency, so a fixed 50-iteration budget starved under parallel load
    // (issue #593). The assertions inside stay exact.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let status = crate::background::global()
            .status(&task_id)
            .await
            .expect("status should exist");
        if let Some(progress) = status.progress {
            saw_progress = true;
            assert_eq!(progress.percent, Some(50.0));
            assert_eq!(progress.unit.as_deref(), Some("gib"));
            assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert!(
        saw_progress,
        "expected byte-ratio progress to be recorded for {task_id}"
    );
}

#[tokio::test]
async fn test_background_command_respects_timeout() {
    let tool = BashTool::new();
    let ctx = make_ctx(None);

    let result = tool
        .execute(
            json!({
                "command": "sleep 5; echo should_not_print",
                "run_in_background": true,
                "timeout": 100,
                "notify": false,
                "wake": false,
            }),
            ctx,
        )
        .await
        .expect("background command should start");

    let metadata = result.metadata.expect("expected metadata");
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task id should be present")
        .to_string();

    let mut final_status = None;
    // Wall-clock deadline rather than a fixed iteration count. The command's own
    // timeout is 100ms, but the *observation* of the resulting Failed status
    // depends on scheduler latency, and a 50 x 50ms budget starved when the full
    // suite runs in parallel on a loaded machine (issue #593). A generous
    // deadline keeps the assertion strict while removing the timing race: a real
    // regression still fails, it just is not reported as a flake.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let status = crate::background::global()
            .status(&task_id)
            .await
            .expect("status should exist");
        if status.status == BackgroundTaskStatus::Failed {
            final_status = Some(status);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let status = final_status.expect("background task should fail after timeout");
    assert_eq!(status.exit_code, Some(124));
    assert!(
        status
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("timed out"),
        "timeout failure should be recorded: {status:?}"
    );

    let output = crate::background::global()
        .output(&task_id)
        .await
        .expect("output should exist");
    assert!(
        output.contains("timed out after 100ms"),
        "output was: {output}"
    );
    assert!(
        !output.contains("should_not_print"),
        "timed-out command should not complete normally: {output}"
    );
}

#[tokio::test]
async fn test_background_command_without_timeout_keeps_running_past_default_foreground_timeout() {
    let tool = BashTool::new();
    let ctx = make_ctx(None);

    let result = tool
        .execute(
            json!({
                "command": "sleep 0.25; echo background_no_implicit_timeout_ok",
                "run_in_background": true,
                "notify": false,
                "wake": false,
            }),
            ctx,
        )
        .await
        .expect("background command should start");

    let metadata = result.metadata.expect("expected metadata");
    let task_id = metadata["task_id"]
        .as_str()
        .expect("task id should be present")
        .to_string();
    let output_file = std::path::PathBuf::from(
        metadata["output_file"]
            .as_str()
            .expect("output_file should be present"),
    );
    let status_file = std::path::PathBuf::from(
        metadata["status_file"]
            .as_str()
            .expect("status_file should be present"),
    );

    let mut final_status = None;
    for _ in 0..30 {
        let status = crate::background::global()
            .status(&task_id)
            .await
            .expect("status should exist");
        if status.status != BackgroundTaskStatus::Running {
            final_status = Some(status);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    let status = final_status.expect("background task should finish normally");
    assert_eq!(status.status, BackgroundTaskStatus::Completed);
    assert_eq!(status.exit_code, Some(0));

    let output = crate::background::global()
        .output(&task_id)
        .await
        .expect("output should exist");
    assert!(
        output.contains("background_no_implicit_timeout_ok"),
        "output was: {output}"
    );

    let _ = tokio::fs::remove_file(output_file).await;
    let _ = tokio::fs::remove_file(status_file).await;
}

#[cfg(unix)]
#[tokio::test]
async fn process_group_kill_guard_terminates_descendants() {
    let mut cmd = build_shell_command("sleep 60 & echo $!; wait");
    unsafe {
        cmd.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.kill_on_drop(true).stdout(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn process group probe");
    let mut lines = BufReader::new(child.stdout.take().expect("probe stdout")).lines();
    let descendant_pid = lines
        .next_line()
        .await
        .expect("read descendant pid")
        .expect("descendant pid line")
        .parse::<u32>()
        .expect("numeric descendant pid");

    let guard = ProcessGroupKillGuard::new(child.id());
    drop(guard);
    tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("shell should exit after process-group kill")
        .expect("wait for shell");

    // Wall-clock deadline: process teardown is asynchronous and 100 x 10ms was
    // too tight under parallel load (issue #593).
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if !crate::platform::is_process_running(descendant_pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("descendant process {descendant_pid} survived process-group cleanup");
}

#[test]
fn test_bash_tool_schema_advertises_background_progress_guidance() {
    let schema = BashTool::new().parameters_schema();
    let command_description = schema["properties"]["command"]["description"]
        .as_str()
        .expect("command description should be a string");
    let background_description = schema["properties"]["run_in_background"]["description"]
        .as_str()
        .expect("run_in_background description should be a string");

    assert!(
        command_description.contains("JCODE_SCRATCH_DIR"),
        "command description should keep the scratch-dir guidance"
    );
    assert!(
        background_description.contains("JCODE_PROGRESS"),
        "background description should mention the progress marker format"
    );
}

// Destructive-command gate integration (#604).
//
// The unit-level policy is covered in jcode-command-risk. These tests pin the
// wiring: that the gate actually sits in the bash tool's execute path, that it
// refuses before spawning a process, and that it does not disturb normal work.

fn gate_ctx(working_dir: &str) -> ToolContext {
    ToolContext {
        session_id: "gate-test".to_string(),
        message_id: "m".to_string(),
        tool_call_id: "c".to_string(),
        working_dir: Some(std::path::PathBuf::from(working_dir)),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

#[tokio::test]
async fn bash_refuses_to_delete_the_home_directory() {
    // The #604 incident, at the real tool boundary.
    let _lock = crate::storage::lock_test_env();
    let _gate = super::destructive_gate::RiskGateEnvGuard::set("1");
    let temp = tempfile::tempdir().expect("temp home");
    let home = temp.path().to_string_lossy().to_string();
    let previous = std::env::var("HOME").ok();
    // SAFETY: guarded by lock_test_env; restored below.
    unsafe { std::env::set_var("HOME", &home) };

    let canary = temp.path().join("precious.txt");
    std::fs::write(&canary, "user data").expect("write canary");

    let result = BashTool::new()
        .execute(
            serde_json::json!({ "command": format!("rm -rf {home}") }),
            gate_ctx("/tmp"),
        )
        .await;

    match previous {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }

    let error = result.expect_err("deleting HOME must be refused");
    assert!(
        error.to_string().contains("blocked"),
        "expected an outright block, got: {error}"
    );
    assert!(
        canary.exists(),
        "the gate must refuse before the process runs; the file was deleted"
    );
}

#[tokio::test]
async fn bash_holds_a_risky_delete_until_justified_then_runs_it() {
    // The gate must fire for this test to mean anything, and the env-semantics
    // tests can flip JCODE_RISK_GATE_ENABLED concurrently, so take the env lock
    // and pin the gate on. Restored on drop.
    let _lock = crate::storage::lock_test_env();
    let _gate = super::destructive_gate::RiskGateEnvGuard::set("1");
    let temp = tempfile::tempdir().expect("temp dir");
    let workdir = temp.path().join("work");
    let target = temp.path().join("outside");
    std::fs::create_dir_all(&workdir).expect("workdir");
    std::fs::create_dir_all(&target).expect("target");
    std::fs::write(target.join("f.txt"), "x").expect("file");

    // Temp paths are deliberately low risk. Use a shell-expanded target to
    // exercise Confirm for a target the static assessor cannot resolve.
    let command = format!(
        "fixture_target=\"{}\"; rm -rf \"$fixture_target\"/* && rmdir \"$fixture_target\"",
        target.display()
    );
    let tool = BashTool::new();

    // First attempt: no justification, so it is held.
    let held = tool
        .execute(
            serde_json::json!({ "command": command }),
            gate_ctx(workdir.to_str().expect("utf8")),
        )
        .await
        .expect_err("first attempt should be held");
    assert!(held.to_string().contains("justification"), "{held}");
    assert!(target.exists(), "nothing should have been deleted yet");

    // A blind retry is held identically: repetition is not consent.
    let retried = tool
        .execute(
            serde_json::json!({ "command": command }),
            gate_ctx(workdir.to_str().expect("utf8")),
        )
        .await
        .expect_err("a blind retry should still be held");
    assert!(retried.to_string().contains("justification"));
    assert!(target.exists());

    // With a real justification it proceeds.
    tool.execute(
        serde_json::json!({
            "command": command,
            "justification": "The user asked me to remove the outside/ fixture \
                              directory they created earlier in this session.",
        }),
        gate_ctx(workdir.to_str().expect("utf8")),
    )
    .await
    .expect("a justified command should run");
    assert!(!target.exists(), "the justified delete should have run");
}

#[tokio::test]
async fn bash_does_not_interfere_with_ordinary_commands() {
    // If the gate fires on routine work it will be worked around, so this is a
    // load-bearing test, not a formality.
    let temp = tempfile::tempdir().expect("temp dir");
    let workdir = temp.path().to_str().expect("utf8");
    std::fs::create_dir_all(temp.path().join("build")).expect("build dir");

    for command in ["echo hello", "rm -rf build", "ls -la"] {
        BashTool::new()
            .execute(serde_json::json!({ "command": command }), gate_ctx(workdir))
            .await
            .unwrap_or_else(|e| panic!("{command:?} should run untouched: {e}"));
    }
}

#[tokio::test]
async fn indirect_dispatch_paths_cannot_bypass_the_gate() {
    // batch, and every other caller, dispatch through Tool::execute rather than
    // reimplementing it, so the gate lives at the only chokepoint. Assert that
    // directly: calling execute for a background job (the one path that returns
    // early) is still gated.
    let _lock = crate::storage::lock_test_env();
    let _gate = super::destructive_gate::RiskGateEnvGuard::set("1");
    let temp = tempfile::tempdir().expect("temp home");
    let home = temp.path().to_string_lossy().to_string();
    let previous = std::env::var("HOME").ok();
    // SAFETY: guarded by lock_test_env; restored below.
    unsafe { std::env::set_var("HOME", &home) };
    let canary = temp.path().join("precious.txt");
    std::fs::write(&canary, "user data").expect("canary");

    let result = BashTool::new()
        .execute(
            serde_json::json!({
                "command": format!("rm -rf {home}"),
                "run_in_background": true,
            }),
            gate_ctx("/tmp"),
        )
        .await;

    match previous {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }

    assert!(
        result.is_err(),
        "background dispatch must be gated too, not just foreground"
    );
    assert!(canary.exists(), "the file must survive a backgrounded call");
}

#[test]
fn parse_progress_line_classifies_markers_checkpoints_and_heuristics() {
    let update = parse_progress_line(r#"JCODE_PROGRESS {"percent":40,"message":"Working"}"#)
        .expect("parser should not fail")
        .expect("progress marker should parse");
    match update {
        ProgressLineUpdate::Progress(progress) => assert_eq!(progress.percent, Some(40.0)),
        other => panic!("expected a progress update, got {other:?}"),
    }

    let update = parse_progress_line(r#"JCODE_CHECKPOINT {"message":"Tests passed"}"#)
        .expect("parser should not fail")
        .expect("checkpoint marker should parse");
    match update {
        ProgressLineUpdate::Checkpoint(progress) => {
            assert_eq!(progress.message.as_deref(), Some("Tests passed"))
        }
        other => panic!("expected a checkpoint update, got {other:?}"),
    }

    let update = parse_progress_line("Copied 7/10 files")
        .expect("parser should not fail")
        .expect("heuristic ratio should parse");
    match update {
        ProgressLineUpdate::Progress(progress) => {
            assert_eq!(progress.percent, Some(70.0));
            assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
        }
        other => panic!("expected a progress update, got {other:?}"),
    }

    assert!(
        parse_progress_line("plain log line with no progress")
            .expect("parser should not fail")
            .is_none(),
        "non-progress output must not produce updates"
    );
}

#[path = "bash_promoted_tests.rs"]
mod promoted;
