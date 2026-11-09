use super::*;

/// Cancelling a timeout-promoted foreground command must terminate its child process too.
#[tokio::test]
async fn promoted_command_cancellation_terminates_process_group() {
    let directory = tempfile::tempdir().expect("temporary working directory");
    let mut ctx = make_ctx(None);
    ctx.working_dir = Some(directory.path().to_path_buf());
    ctx.session_id = format!("timeout-cancel-{}", uuid::Uuid::new_v4());

    let result = BashTool::new()
        .execute(
            json!({
                "command": "sleep 20 & printf '%s %s\\n' \"$$\" \"$!\" > pids; wait",
                "timeout": 1000,
                "notify": false,
                "wake": false,
            }),
            ctx,
        )
        .await
        .expect("foreground timeout should promote the command");
    let metadata = result.metadata.expect("background task metadata");
    let task_id = metadata["task_id"].as_str().expect("task ID");
    assert_eq!(metadata["timeout_promoted"], true);

    let pids = std::fs::read_to_string(directory.path().join("pids"))
        .expect("command should record its process IDs before promotion");
    let mut pids = pids.split_whitespace();
    let parent_pid: u32 = pids
        .next()
        .expect("shell PID")
        .parse()
        .expect("numeric PID");
    let child_pid: u32 = pids
        .next()
        .expect("child PID")
        .parse()
        .expect("numeric PID");
    // Only signal the dedicated group we created, never the test runner's group.
    assert_eq!(
        unsafe { libc::getpgid(child_pid as i32) },
        parent_pid as i32
    );
    let mut cleanup = ProcessGroupKillGuard::new(Some(parent_pid));
    assert!(crate::platform::is_process_running(child_pid));
    assert!(
        crate::background::global()
            .cancel(task_id)
            .await
            .expect("cancel task")
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while crate::platform::is_process_running(child_pid) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let survived = crate::platform::is_process_running(child_pid);
    if !survived || unsafe { libc::getpgid(child_pid as i32) } != parent_pid as i32 {
        cleanup.disarm();
    }
    assert!(!survived, "descendant {child_pid} survived cancellation");
}

/// A successful foreground command may deliberately leave a detached child running.
#[tokio::test]
async fn completed_foreground_command_does_not_kill_background_child() {
    let directory = tempfile::tempdir().expect("temporary working directory");
    let mut ctx = make_ctx(None);
    ctx.working_dir = Some(directory.path().to_path_buf());
    ctx.session_id = format!("foreground-exit-{}", uuid::Uuid::new_v4());
    let result = BashTool::new()
        .execute(
            json!({
                "command": "sleep 20 >/dev/null 2>&1 & printf '%s %s\\n' \"$$\" \"$!\" > pids",
                "timeout": 5000,
            }),
            ctx,
        )
        .await
        .expect("foreground command should complete");
    assert_eq!(result.output, "Command completed successfully (no output)");

    let pids = std::fs::read_to_string(directory.path().join("pids"))
        .expect("command should record its process IDs");
    let mut pids = pids.split_whitespace();
    let parent_pid: u32 = pids
        .next()
        .expect("shell PID")
        .parse()
        .expect("numeric PID");
    let child_pid: u32 = pids
        .next()
        .expect("child PID")
        .parse()
        .expect("numeric PID");
    // The child is the only surviving member of the group created by this test.
    assert_eq!(
        unsafe { libc::getpgid(child_pid as i32) },
        parent_pid as i32
    );
    let cleanup = ProcessGroupKillGuard::new(Some(parent_pid));
    assert!(crate::platform::is_process_running(child_pid));
    drop(cleanup);
}
