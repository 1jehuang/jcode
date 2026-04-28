use super::*;
use crate::bus::BackgroundTaskStatus;
use crate::tool::StdinInputRequest;
use serde_json::json;
use tokio::sync::mpsc;

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

    // cat will block forever on stdin, but we set a short timeout
    // and never respond to the stdin request
    let input = json!({"command": "cat", "timeout": 2000});
    let ctx = make_ctx(Some(tx));

    let result = tool.execute(input, ctx).await;
    assert!(result.is_err(), "should timeout");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("timed out"),
        "error should mention timeout: {}",
        err_msg
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

#[test]
fn test_parse_progress_marker_handles_percent_payloads() {
    let progress = parse_progress_marker(
        r#"JCODE_PROGRESS {"percent":25,"message":"Downloading dependencies"}"#,
    )
    .expect("marker should parse");

    assert_eq!(progress.percent, Some(25.0));
    assert_eq!(
        progress.message.as_deref(),
        Some("Downloading dependencies")
    );
    assert_eq!(progress.kind, BackgroundTaskProgressKind::Determinate);
    assert_eq!(progress.source, BackgroundTaskProgressSource::Reported);
}

#[test]
fn test_parse_heuristic_progress_handles_ratio_output() {
    let progress = parse_heuristic_progress("Running test 3/10 tests")
        .expect("heuristic parser should not fail")
        .expect("heuristic ratio progress should parse");

    assert_eq!(progress.current, Some(3));
    assert_eq!(progress.total, Some(10));
    assert_eq!(progress.percent, Some(30.0));
    assert_eq!(progress.unit.as_deref(), Some("tests"));
    assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
}

#[test]
fn test_parse_heuristic_progress_handles_percent_output() {
    let progress = parse_heuristic_progress("download progress 42% complete")
        .expect("heuristic parser should not fail")
        .expect("heuristic percent progress should parse");

    assert_eq!(progress.percent, Some(42.0));
    assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
    assert_eq!(
        progress.message.as_deref(),
        Some("download progress 42% complete")
    );
}

#[test]
fn test_parse_heuristic_progress_handles_phase_output() {
    let progress = parse_heuristic_progress("Compiling jcode v0.10.2")
        .expect("heuristic parser should not fail")
        .expect("phase progress should parse");

    assert_eq!(progress.kind, BackgroundTaskProgressKind::Indeterminate);
    assert_eq!(progress.percent, None);
    assert_eq!(progress.message.as_deref(), Some("Compiling jcode v0.10.2"));
    assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
}

#[test]
fn test_parse_heuristic_progress_handles_of_output() {
    let progress = parse_heuristic_progress("Downloaded 3 of 12 crates")
        .expect("heuristic parser should not fail")
        .expect("heuristic of progress should parse");

    assert_eq!(progress.current, Some(3));
    assert_eq!(progress.total, Some(12));
    assert_eq!(progress.percent, Some(25.0));
    assert_eq!(progress.unit.as_deref(), Some("crates"));
}

#[test]
fn test_parse_heuristic_progress_handles_byte_ratio_output() {
    let progress = parse_heuristic_progress("Downloaded 1.5/3.0 GiB")
        .expect("heuristic parser should not fail")
        .expect("heuristic byte ratio progress should parse");

    assert_eq!(progress.percent, Some(50.0));
    assert_eq!(progress.unit.as_deref(), Some("gib"));
    assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
}

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
    for _ in 0..50 {
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
    for _ in 0..50 {
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
    for _ in 0..50 {
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
    for _ in 0..50 {
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
        BashTool::new().description().contains("JCODE_PROGRESS"),
        "tool description should teach cooperative progress output"
    );
    assert!(
        command_description.contains("JCODE_PROGRESS"),
        "command description should mention progress marker format"
    );
    assert!(
        background_description.contains("3/10 tests"),
        "background description should mention parseable fallback progress output"
    );
}
