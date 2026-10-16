//! Cancellation of a running background task, including the TERM-to-KILL grace
//! period for detached process groups.

use super::*;

/// Cancel a running task, allowing detached processes a configurable grace period
/// between TERM and KILL on Unix.
///
/// Returns `true` when a live task (in-process or detached) was cancelled, and
/// `false` when there was nothing running to cancel.
pub(super) async fn cancel_with_grace(
    manager: &BackgroundTaskManager,
    task_id: &str,
    _graceful_timeout: std::time::Duration,
) -> Result<bool> {
    let status_path = manager.status_path_for(task_id);
    let mut status_guard = manager.status_updates.clone().lock_owned().await;
    let status = manager.read_status_file(&status_path).await;

    if status
        .as_ref()
        .is_some_and(|status| status.status != BackgroundTaskStatus::Running)
    {
        let task = { manager.tasks.write().await.remove(task_id) };
        if let Some(task) = task {
            task.handle.abort();
            if let Err(error) = task.handle.await
                && !error.is_cancelled()
            {
                crate::logging::warn(&format!("background task join failed: {error}"));
            }
        }
        return Ok(false);
    }

    let task = { manager.tasks.write().await.remove(task_id) };
    if let Some(task) = task {
        let RunningTask {
            task_id,
            tool_name,
            display_name,
            session_id,
            started_at,
            started_at_rfc3339,
            delivery_flags,
            handle,
            ..
        } = task;
        handle.abort();

        let (notify_flag, wake_flag) = *delivery_flags.borrow();
        let mut status = status.unwrap_or_else(|| TaskStatusFile {
            task_id: task_id.clone(),
            tool_name: tool_name.clone(),
            display_name: display_name.clone(),
            session_id: session_id.clone(),
            status: BackgroundTaskStatus::Running,
            exit_code: None,
            error: None,
            started_at: started_at_rfc3339.clone(),
            completed_at: None,
            duration_secs: None,
            pid: None,
            owner_pid: Some(std::process::id()),
            owner_instance: Some(model::process_instance_token().to_string()),
            detached: false,
            notify: notify_flag,
            wake: wake_flag,
            progress: None,
            event_history: Vec::new(),
            stall_wake_seconds: None,
        });
        status.task_id = task_id;
        status.tool_name = tool_name;
        status.display_name = status.display_name.or(display_name);
        status.session_id = session_id;
        status.status = BackgroundTaskStatus::Failed;
        status.exit_code = None;
        status.error = Some("Cancelled by user".to_string());
        status.started_at = started_at_rfc3339;
        status.completed_at = Some(Utc::now().to_rfc3339());
        status.duration_secs = Some(started_at.elapsed().as_secs_f64());
        status.pid = None;
        status.owner_pid = Some(std::process::id());
        status.owner_instance = Some(model::process_instance_token().to_string());
        status.detached = false;
        status.notify = notify_flag;
        status.wake = wake_flag;
        status.stall_wake_seconds = None;
        push_task_event(
            &mut status,
            terminal_event_record(
                BackgroundTaskStatus::Failed,
                None,
                Some("Cancelled by user"),
            ),
        );
        status_guard = manager
            .write_status_file_locked(status_guard, &status_path, &status)
            .await;
        if let Err(error) = handle.await
            && !error.is_cancelled()
        {
            crate::logging::warn(&format!("background task join failed: {error}"));
        }
        drop(status_guard);
        return Ok(true);
    }

    let Some(mut status) = status else {
        return Ok(false);
    };
    if !status.detached {
        return Ok(false);
    }
    drop(status_guard);
    status = manager
        .finalize_detached_status_if_needed(status, &status_path)
        .await;
    if status.status != BackgroundTaskStatus::Running {
        return Ok(false);
    }

    status_guard = manager.status_updates.clone().lock_owned().await;
    let Some(refreshed) = manager.read_status_file(&status_path).await else {
        return Ok(false);
    };
    status = refreshed;
    if status.status != BackgroundTaskStatus::Running || !status.detached {
        return Ok(false);
    }
    let Some(pid) = status.pid else {
        return Ok(false);
    };

    #[cfg(unix)]
    {
        if let Err(error) = crate::platform::signal_detached_process_group(pid, libc::SIGTERM)
            && error.raw_os_error() != Some(libc::ESRCH)
        {
            crate::logging::warn(&format!("background task {pid} TERM failed: {error}"));
        }
        let grace_sleep = tokio::time::sleep(_graceful_timeout);
        drop(status_guard);
        #[cfg(test)]
        super::cancel_grace_tests::at_grace_boundary(task_id).await;
        grace_sleep.await;
        status_guard = manager.status_updates.clone().lock_owned().await;
        let Some(refreshed) = manager.read_status_file(&status_path).await else {
            return Ok(false);
        };
        if refreshed.status != BackgroundTaskStatus::Running
            || !refreshed.detached
            || refreshed.pid != Some(pid)
        {
            return Ok(false);
        }
        status = refreshed;
        if crate::platform::is_process_running(pid)
            && let Err(error) = crate::platform::signal_detached_process_group(pid, libc::SIGKILL)
            && error.raw_os_error() != Some(libc::ESRCH)
        {
            crate::logging::warn(&format!("background task {pid} KILL failed: {error}"));
        }
    }
    #[cfg(windows)]
    {
        if let Err(error) = crate::platform::signal_detached_process_group(pid, 0) {
            crate::logging::warn(&format!(
                "background task {pid} termination failed: {error}"
            ));
        }
    }

    let completed_at = Utc::now();
    status.status = BackgroundTaskStatus::Failed;
    status.exit_code = None;
    status.error = Some("Cancelled by user".to_string());
    status.completed_at = Some(completed_at.to_rfc3339());
    status.duration_secs =
        BackgroundTaskManager::status_duration_secs(&status.started_at, completed_at);
    push_task_event(
        &mut status,
        terminal_event_record(
            BackgroundTaskStatus::Failed,
            None,
            Some("Cancelled by user"),
        ),
    );
    drop(
        manager
            .write_status_file_locked(status_guard, &status_path, &status)
            .await,
    );
    Ok(true)
}
