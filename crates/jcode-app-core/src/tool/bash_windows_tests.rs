use super::*;

/// Build a direct tool context rooted at the test's temporary directory.
fn test_context(dir: &Path) -> ToolContext {
    ToolContext {
        session_id: "test-session".to_string(),
        message_id: "test-message".to_string(),
        tool_call_id: "test-call".to_string(),
        working_dir: Some(dir.to_owned()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

/// Ordinary Windows background commands still finish without cancellation.
#[tokio::test]
async fn background_command_completes_without_cancellation() {
    let dir = tempfile::tempdir().expect("temporary command directory");
    let ctx = test_context(dir.path());
    let result = BashTool::new()
        .execute(
            json!({"command": "echo done", "run_in_background": true}),
            ctx,
        )
        .await
        .expect("start background command");
    let metadata = result.metadata.expect("background task metadata");
    let task_id = metadata["task_id"].as_str().expect("task id");
    let result = crate::background::global()
        .wait(task_id, Duration::from_secs(10), false)
        .await
        .expect("task status");
    assert_eq!(
        result.task.status,
        crate::bus::BackgroundTaskStatus::Completed
    );
}

/// Cancelling a live background command must stop its Windows descendants too.
#[tokio::test]
async fn cancel_background_command_stops_windows_descendants() {
    let dir = tempfile::tempdir().expect("temporary command directory");
    std::fs::write(
        dir.path().join("child.cmd"),
        "@echo off\r\necho ready>child-ready.txt\r\nping -n 5 127.0.0.1 >NUL\r\necho survived>child-survived.txt\r\n",
    )
    .expect("write child command");
    std::fs::write(
        dir.path().join("parent.cmd"),
        "@echo off\r\nstart \"\" /B cmd.exe /D /C child.cmd\r\nping -n 12 127.0.0.1 >NUL\r\n",
    )
    .expect("write parent command");
    let ctx = test_context(dir.path());
    let result = BashTool::new()
        .execute(
            json!({"command": "parent.cmd", "run_in_background": true}),
            ctx,
        )
        .await
        .expect("start background command");
    let metadata = result.metadata.expect("background task metadata");
    let task_id = metadata["task_id"].as_str().expect("task id");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !dir.path().join("child-ready.txt").exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ready = dir.path().join("child-ready.txt").exists();
    let cancelled = crate::background::global().cancel(task_id).await;
    assert!(ready, "descendant did not start");
    assert!(cancelled.expect("cancel task"));
    // The child writes this marker if it outlives cancellation.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(
        !dir.path().join("child-survived.txt").exists(),
        "descendant survived cancellation"
    );
}
