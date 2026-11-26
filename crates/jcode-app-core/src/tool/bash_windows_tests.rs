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

/// Normal completion must not terminate a deliberately detached child.
#[tokio::test]
async fn normal_completion_leaves_detached_child_running() {
    let dir = tempfile::tempdir().expect("temporary command directory");
    std::fs::write(
        dir.path().join("detached.cmd"),
        "@echo off\r\nping -n 3 127.0.0.1 >NUL\r\necho done>detached-finished.txt\r\n",
    )
    .expect("write child command");
    let result = BashTool::new()
        .execute(
            json!({"command": "start \"\" /B cmd.exe /D /C detached.cmd >NUL 2>&1", "run_in_background": true}),
            test_context(dir.path()),
        )
        .await
        .expect("start background command");
    let task_id = result.metadata.expect("metadata")["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();
    let status = crate::background::global()
        .wait(&task_id, Duration::from_secs(10), false)
        .await
        .expect("task status");
    assert_eq!(
        status.task.status,
        crate::bus::BackgroundTaskStatus::Completed
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !dir.path().join("detached-finished.txt").exists() && std::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        dir.path().join("detached-finished.txt").exists(),
        "detached child was stopped"
    );
}

/// Wait for the shell handle, then cancel jobs with and without live children.
#[tokio::test]
async fn exited_shell_job_cleanup_preserves_both_exit_codes() {
    use process_wrap::tokio::{CommandWrap, JobObject, JobObjectChild, KillOnDrop};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    let dir = tempfile::tempdir().expect("temporary command directory");
    std::fs::write(
        dir.path().join("child.cmd"),
        "@echo off\r\necho ready>child-ready.txt\r\nping -n 8 127.0.0.1 >NUL\r\n",
    )
    .expect("write child command");
    for (exit_code, has_child) in [(0, true), (7, true), (0, false), (7, false)] {
        let marker = dir.path().join("child-ready.txt");
        let _ = std::fs::remove_file(&marker);
        let command_line = if has_child {
            format!("start \"\" /B cmd.exe /D /C child.cmd & exit {exit_code}")
        } else {
            format!("exit {exit_code}")
        };
        let mut command = build_shell_command(&command_line);
        command.current_dir(dir.path());
        configure_background_command_stdio(&mut command);
        let mut wrapped = CommandWrap::from(command);
        // Test-only kill-on-drop ensures cleanup even if an assertion fails.
        wrapped.wrap(JobObject).wrap(KillOnDrop);
        let mut child = wrapped.spawn().expect("spawn command");
        let job = (child.as_ref() as &dyn std::any::Any)
            .downcast_ref::<JobObjectChild>()
            .expect("job wrapper")
            .job_handle();
        // SAFETY: the job wrapper's wait is never polled while using its child.
        let native = unsafe { child.try_inner_child_mut() }.expect("native child");
        let handle = native.raw_handle().expect("process handle");
        // SAFETY: native owns this handle for the duration of the wait.
        assert_eq!(
            unsafe { WaitForSingleObject(handle, 10_000) },
            WAIT_OBJECT_0
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while has_child && !marker.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if has_child {
            assert!(marker.exists(), "descendant did not start");
            assert!(windows_job_active_processes(&job).expect("job members") > 0);
        } else {
            while windows_job_active_processes(&job).expect("job members") != 0
                && std::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(windows_job_active_processes(&job).expect("job members"), 0);
        }
        let status = cancel_windows_background_tree(native, &job)
            .await
            .expect("cleanup")
            .expect("shell already exited");
        assert_eq!(status.code(), Some(exit_code));
        assert_eq!(windows_job_active_processes(&job).expect("job members"), 0);
    }
}

/// An exited shell keeps its exit code while its still-running child is stopped.
#[tokio::test]
async fn cancel_after_shell_exit_preserves_status_and_stops_descendant() {
    let dir = tempfile::tempdir().expect("temporary command directory");
    std::fs::write(
        dir.path().join("child.cmd"),
        "@echo off\r\necho ready>child-ready.txt\r\nping -n 8 127.0.0.1 >NUL\r\necho survived>child-survived.txt\r\n",
    )
    .expect("write child command");
    for exit_code in [0, 7] {
        let ready_path = dir.path().join("child-ready.txt");
        let _ = std::fs::remove_file(&ready_path);
        let result = BashTool::new()
            .execute(
                json!({"command": format!("start \"\" /B cmd.exe /D /C child.cmd & exit {exit_code}"), "run_in_background": true}),
                test_context(dir.path()),
            )
            .await
            .expect("start background command");
        let task_id = result.metadata.expect("metadata")["task_id"]
            .as_str()
            .expect("task id")
            .to_owned();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !ready_path.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ready_path.exists(), "child did not start");
        tokio::time::sleep(Duration::from_millis(400)).await;
        let cancelled = crate::background::global()
            .cancel(&task_id)
            .await
            .expect("cleanup");
        assert!(!cancelled, "the shell has already completed");
        let status = crate::background::global()
            .status(&task_id)
            .await
            .expect("task status");
        assert_eq!(status.exit_code, Some(exit_code));
        assert_eq!(
            status.status,
            if exit_code == 0 {
                crate::bus::BackgroundTaskStatus::Completed
            } else {
                crate::bus::BackgroundTaskStatus::Failed
            }
        );
    }
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert!(
        !dir.path().join("child-survived.txt").exists(),
        "descendant survived cleanup"
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

/// A background timeout also stops every process in the Windows job.
#[tokio::test]
async fn background_timeout_stops_windows_descendants() {
    let dir = tempfile::tempdir().expect("temporary command directory");
    std::fs::write(
        dir.path().join("child.cmd"),
        "@echo off\r\necho ready>child-ready.txt\r\nping -n 8 127.0.0.1 >NUL\r\necho survived>child-survived.txt\r\n",
    )
    .expect("write child command");
    let result = BashTool::new()
        .execute(
            json!({"command": "start \"\" /B cmd.exe /D /C child.cmd & ping -n 12 127.0.0.1 >NUL", "run_in_background": true, "timeout": 3000}),
            test_context(dir.path()),
        )
        .await
        .expect("start background command");
    let task_id = result.metadata.expect("metadata")["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();
    let status = crate::background::global()
        .wait(&task_id, Duration::from_secs(15), false)
        .await
        .expect("task status");
    assert!(
        dir.path().join("child-ready.txt").exists(),
        "child did not start"
    );
    assert_eq!(status.task.status, crate::bus::BackgroundTaskStatus::Failed);
    assert_eq!(status.task.exit_code, Some(124));
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert!(
        !dir.path().join("child-survived.txt").exists(),
        "child survived timeout"
    );
}
