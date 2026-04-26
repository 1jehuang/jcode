use super::*;
use crate::bus::{BackgroundTaskProgressKind, BackgroundTaskProgressSource, BusEvent};
use anyhow::anyhow;
use tempfile::tempdir;
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn update_delivery_applies_to_running_task_completion() -> Result<()> {
    let tmp = tempdir()?;
    let manager = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());

    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "session-test",
            false,
            false,
            |output_path| async move {
                sleep(Duration::from_millis(25)).await;
                tokio::fs::write(&output_path, "hello").await?;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;

    let updated = manager
        .update_delivery(&info.task_id, true, true)
        .await
        .map_err(|err| anyhow!("update delivery should succeed: {err}"))?
        .ok_or_else(|| anyhow!("task should exist"))?;
    assert!(updated.notify);
    assert!(updated.wake);

    for _ in 0..40 {
        let status = manager
            .status(&info.task_id)
            .await
            .ok_or_else(|| anyhow!("status should exist"))?;
        if status.status != BackgroundTaskStatus::Running {
            assert!(status.notify);
            assert!(status.wake);
            assert_eq!(status.status, BackgroundTaskStatus::Completed);
            return Ok(());
        }
        sleep(Duration::from_millis(10)).await;
    }

    Err(anyhow!("background task did not complete in time"))
}

#[tokio::test]
async fn update_progress_persists_status_and_emits_bus_event() -> Result<()> {
    let tmp = tempdir()?;
    let manager = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());

    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "session-progress",
            false,
            false,
            |_output_path| async move {
                sleep(Duration::from_millis(50)).await;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;

    let progress = BackgroundTaskProgress {
        kind: BackgroundTaskProgressKind::Determinate,
        percent: Some(42.0),
        message: Some("Running checks".to_string()),
        current: Some(21),
        total: Some(50),
        unit: Some("tests".to_string()),
        eta_seconds: Some(8),
        updated_at: Utc::now().to_rfc3339(),
        source: BackgroundTaskProgressSource::Reported,
    };

    let mut bus_rx = Bus::global().subscribe();
    let updated = manager
        .update_progress(&info.task_id, progress.clone())
        .await
        .map_err(|err| anyhow!("update progress should succeed: {err}"))?
        .ok_or_else(|| anyhow!("task should exist"))?;

    assert_eq!(updated.progress, Some(progress.clone().normalize()));

    for _ in 0..20 {
        let event = tokio::time::timeout(Duration::from_millis(200), bus_rx.recv())
            .await
            .map_err(|err| anyhow!("timed out waiting for progress event: {err}"))?
            .map_err(|err| anyhow!("bus should stay open: {err}"))?;
        if let BusEvent::BackgroundTaskProgress(event) = event
            && event.task_id == info.task_id
        {
            assert_eq!(event.session_id, "session-progress");
            assert_eq!(event.progress, progress.normalize());
            return Ok(());
        }
    }

    Err(anyhow!(
        "progress event for task {} not received",
        info.task_id
    ))
}

#[tokio::test]
async fn wait_returns_when_task_finishes() -> Result<()> {
    let tmp = tempdir()?;
    let manager = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());

    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "session-wait-finish",
            false,
            false,
            |output_path| async move {
                sleep(Duration::from_millis(25)).await;
                tokio::fs::write(&output_path, "done").await?;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;

    let wait_result = manager
        .wait(&info.task_id, Duration::from_secs(2), true)
        .await
        .ok_or_else(|| anyhow!("task should exist"))?;

    assert_eq!(wait_result.reason, BackgroundTaskWaitReason::Finished);
    assert_eq!(wait_result.task.status, BackgroundTaskStatus::Completed);
    assert_eq!(wait_result.task.exit_code, Some(0));
    Ok(())
}

#[tokio::test]
async fn wait_returns_on_progress_checkpoint() -> Result<()> {
    let tmp = tempdir()?;
    let manager = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());

    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "session-wait-progress",
            false,
            false,
            |_output_path| async move {
                sleep(Duration::from_secs(2)).await;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;

    let progress = BackgroundTaskProgress {
        kind: BackgroundTaskProgressKind::Determinate,
        percent: Some(25.0),
        message: Some("checkpoint".to_string()),
        current: Some(1),
        total: Some(4),
        unit: Some("steps".to_string()),
        eta_seconds: Some(3),
        updated_at: Utc::now().to_rfc3339(),
        source: BackgroundTaskProgressSource::Reported,
    };

    let waiter = manager.wait(&info.task_id, Duration::from_secs(2), true);
    let updater = async {
        sleep(Duration::from_millis(25)).await;
        manager
            .update_progress(&info.task_id, progress.clone())
            .await
            .map_err(|err| anyhow!("progress update should succeed: {err}"))?
            .ok_or_else(|| anyhow!("task should exist"))?;
        Result::<()>::Ok(())
    };
    let (wait_result, updater_result) = tokio::join!(waiter, updater);
    updater_result?;
    let wait_result = wait_result.ok_or_else(|| anyhow!("task should exist"))?;

    assert_eq!(wait_result.reason, BackgroundTaskWaitReason::Progress);
    assert_eq!(wait_result.task.status, BackgroundTaskStatus::Running);
    assert_eq!(wait_result.task.progress, Some(progress.normalize()));
    assert!(wait_result.progress_event.is_some());
    Ok(())
}

#[tokio::test]
async fn wait_returns_on_timeout() -> Result<()> {
    let tmp = tempdir()?;
    let manager = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());

    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "session-wait-timeout",
            false,
            false,
            |_output_path| async move {
                sleep(Duration::from_millis(250)).await;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;

    let wait_result = manager
        .wait(&info.task_id, Duration::from_millis(25), true)
        .await
        .ok_or_else(|| anyhow!("task should exist"))?;

    assert_eq!(wait_result.reason, BackgroundTaskWaitReason::Timeout);
    assert_eq!(wait_result.task.status, BackgroundTaskStatus::Running);
    Ok(())
}
