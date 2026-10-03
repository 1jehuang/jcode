use super::*;
use crate::bus::{BackgroundTaskProgressKind, BackgroundTaskProgressSource};
use anyhow::anyhow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::task::Poll;
use tempfile::tempdir;
use tokio::time::Duration;

const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

struct StatusWriteTestHook {
    staged: std::sync::mpsc::SyncSender<PathBuf>,
    resume: std::sync::mpsc::Receiver<()>,
}

struct StatusWriteTestControl {
    staged: Receiver<PathBuf>,
    resume: std::sync::mpsc::SyncSender<()>,
}

fn status_write_test_hooks()
-> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, StatusWriteTestHook>> {
    static HOOKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, StatusWriteTestHook>>,
    > = std::sync::OnceLock::new();
    HOOKS.get_or_init(Default::default)
}

fn install_status_write_test_hook(path: PathBuf) -> StatusWriteTestControl {
    let (staged_tx, staged_rx) = std::sync::mpsc::sync_channel(1);
    let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
    let prior = status_write_test_hooks()
        .lock()
        .expect("status write test hooks mutex poisoned")
        .insert(
            path,
            StatusWriteTestHook {
                staged: staged_tx,
                resume: resume_rx,
            },
        );
    assert!(prior.is_none(), "status write test hook already installed");
    StatusWriteTestControl {
        staged: staged_rx,
        resume: resume_tx,
    }
}

pub(in crate::background) fn before_status_file_rename(
    path: &std::path::Path,
    staged_path: PathBuf,
) {
    let hook = status_write_test_hooks()
        .lock()
        .expect("status write test hooks mutex poisoned")
        .remove(path);
    if let Some(hook) = hook {
        hook.staged
            .send(staged_path)
            .expect("status write test observer dropped");
        hook.resume
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("status write test resume timed out");
    }
}

fn running_status_fixture(task_id: &str, session_id: &str) -> TaskStatusFile {
    TaskStatusFile {
        task_id: task_id.to_string(),
        tool_name: "swarm".to_string(),
        display_name: None,
        session_id: session_id.to_string(),
        status: BackgroundTaskStatus::Running,
        exit_code: None,
        error: None,
        started_at: Utc::now().to_rfc3339(),
        completed_at: None,
        duration_secs: None,
        pid: None,
        owner_pid: None,
        owner_instance: None,
        detached: false,
        notify: false,
        wake: false,
        progress: None,
        event_history: Vec::new(),
        stall_wake_seconds: None,
    }
}

async fn recv_staged(receiver: Receiver<PathBuf>) -> Result<PathBuf> {
    tokio::task::spawn_blocking(move || receiver.recv_timeout(HANDSHAKE_TIMEOUT))
        .await
        .map_err(|error| anyhow!("staged status observer panicked: {error}"))?
        .map_err(|error| anyhow!("status write did not reach the pre-rename hook: {error}"))
}

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    std::future::poll_fn(|context| Poll::Ready(future.as_mut().poll(context))).await
}

fn progress(percent: f32) -> BackgroundTaskProgress {
    BackgroundTaskProgress {
        kind: BackgroundTaskProgressKind::Determinate,
        percent: Some(percent),
        message: Some(format!("{percent}%")),
        current: None,
        total: None,
        unit: None,
        eta_seconds: None,
        updated_at: Utc::now().to_rfc3339(),
        source: BackgroundTaskProgressSource::Reported,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staged_atomic_publication_exposes_only_complete_old_then_new_json() -> Result<()> {
    let tmp = tempdir()?;
    let manager = Arc::new(BackgroundTaskManager::with_output_dir(
        tmp.path().to_path_buf(),
    ));
    let path = manager.status_path_for("staged-atomic");
    let old_status = running_status_fixture("staged-atomic", "old-session");
    let mut new_status = old_status.clone();
    new_status.session_id = "new-session".to_string();
    manager.write_status_file(&path, &old_status).await;

    let StatusWriteTestControl { staged, resume } = install_status_write_test_hook(path.clone());
    let writer = tokio::spawn({
        let manager = Arc::clone(&manager);
        let path = path.clone();
        let new_status = new_status.clone();
        async move { manager.write_status_file(&path, &new_status).await }
    });

    let staged_path = recv_staged(staged).await?;
    let visible: TaskStatusFile = serde_json::from_str(&tokio::fs::read_to_string(&path).await?)?;
    let staged: TaskStatusFile =
        serde_json::from_str(&tokio::fs::read_to_string(staged_path).await?)?;
    assert_eq!(visible.session_id, "old-session");
    assert_eq!(staged.session_id, "new-session");

    resume.send(()).expect("resume staged publication");
    writer.await?;
    let visible: TaskStatusFile = serde_json::from_str(&tokio::fs::read_to_string(path).await?)?;
    assert_eq!(visible.session_id, "new-session");
    Ok(())
}

/// Status files are rewritten on every progress and completion update while
/// `bg status`, `bg wait`, and other jcode processes read them. A truncating
/// write is observable as an empty file, and a reader that hits that window
/// reports the task as missing instead of returning its status. The hook holds
/// the write open after its new contents are staged, so the reader runs inside
/// the write deterministically rather than by scheduling luck.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reads_during_an_in_flight_write_see_the_previous_status() -> Result<()> {
    let tmp = tempdir()?;
    let reader = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());
    let writer = Arc::new(BackgroundTaskManager::with_output_dir(
        tmp.path().to_path_buf(),
    ));
    let path = reader.status_path_for("atomic-status");
    let old_status = running_status_fixture("atomic-status", "session-before");
    let mut new_status = old_status.clone();
    new_status.session_id = "session-after".to_string();
    writer.write_status_file(&path, &old_status).await;

    let StatusWriteTestControl { staged, resume } = install_status_write_test_hook(path.clone());
    let update = tokio::spawn({
        let writer = Arc::clone(&writer);
        let path = path.clone();
        async move { writer.write_status_file(&path, &new_status).await }
    });
    recv_staged(staged).await?;

    let during = reader.status("atomic-status").await;
    resume.send(()).expect("resume status publication");
    update.await?;
    let during = during.ok_or_else(|| anyhow!("an in-flight status write hid the task"))?;
    assert_eq!(during.session_id, "session-before");
    let after = reader
        .status("atomic-status")
        .await
        .ok_or_else(|| anyhow!("published status missing"))?;
    assert_eq!(after.session_id, "session-after");
    Ok(())
}

#[tokio::test]
async fn terminal_publication_does_not_block_a_single_worker_runtime() -> Result<()> {
    let tmp = tempdir()?;
    let manager = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "single-worker-publication",
            false,
            false,
            move |_output_path| async move {
                finish_rx.await?;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;
    let StatusWriteTestControl { staged, resume } =
        install_status_write_test_hook(info.status_file.clone());
    let heartbeat_start = Arc::new(tokio::sync::Notify::new());
    let (heartbeat_tx, heartbeat_rx) = std::sync::mpsc::sync_channel(1);
    tokio::spawn({
        let heartbeat_start = Arc::clone(&heartbeat_start);
        async move {
            heartbeat_start.notified().await;
            heartbeat_tx.send(()).expect("heartbeat observer dropped");
        }
    });
    let observer = tokio::task::spawn_blocking(move || {
        staged
            .recv_timeout(HANDSHAKE_TIMEOUT)
            .expect("terminal publication never staged");
        heartbeat_start.notify_one();
        let heartbeat = heartbeat_rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_ok();
        resume.send(()).expect("resume terminal publication");
        heartbeat
    });

    finish_tx.send(()).expect("finish background task");
    assert!(
        observer.await?,
        "Tokio heartbeat was starved by synchronous status publication"
    );
    let status = manager
        .wait(&info.task_id, Duration::from_secs(2), false)
        .await
        .ok_or_else(|| anyhow!("completed task status missing"))?;
    assert_eq!(status.task.status, BackgroundTaskStatus::Completed);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_progress_update_keeps_publication_serialized_until_rename_finishes() -> Result<()>
{
    let tmp = tempdir()?;
    let manager = Arc::new(BackgroundTaskManager::with_output_dir(
        tmp.path().to_path_buf(),
    ));
    let (_finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "cancelled-progress-publication",
            false,
            false,
            move |_output_path| async move {
                finish_rx.await?;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;

    let first = install_status_write_test_hook(info.status_file.clone());
    let update = tokio::spawn({
        let manager = Arc::clone(&manager);
        let task_id = info.task_id.clone();
        async move { manager.update_progress(&task_id, progress(50.0)).await }
    });
    let first_staged = recv_staged(first.staged).await?;
    let staged: TaskStatusFile =
        serde_json::from_str(&tokio::fs::read_to_string(first_staged).await?)?;
    assert_eq!(staged.progress.and_then(|value| value.percent), Some(50.0));
    update.abort();
    assert!(
        update
            .await
            .expect_err("progress update should be cancelled")
            .is_cancelled()
    );
    if manager.status_updates.try_lock().is_ok() {
        first.resume.send(()).expect("resume progress publication");
        return Err(anyhow!(
            "cancelled progress caller released serialization before its blocking rename finished"
        ));
    }

    let cancel_manager = Arc::clone(&manager);
    let cancel_task_id = info.task_id.clone();
    let mut cancel = Box::pin(async move { cancel_manager.cancel(&cancel_task_id).await });
    assert!(
        poll_once(cancel.as_mut()).await.is_pending(),
        "cancellation must wait for the staged progress publication"
    );

    first.resume.send(()).expect("resume progress publication");
    assert!(cancel.await?);
    let status = manager
        .status(&info.task_id)
        .await
        .ok_or_else(|| anyhow!("cancelled task status missing"))?;
    assert_eq!(status.status, BackgroundTaskStatus::Failed);
    assert_eq!(status.error.as_deref(), Some("Cancelled by user"));
    Ok(())
}

#[tokio::test]
async fn live_task_cancellation_survives_missing_or_corrupt_status_file() -> Result<()> {
    for corrupt_contents in [None, Some("{not-json")] {
        let tmp = tempdir()?;
        let manager = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());
        let (_finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
        let info = manager
            .spawn_with_notify(
                "bash",
                None,
                "cancel-without-status",
                false,
                false,
                move |_output_path| async move {
                    finish_rx.await?;
                    Ok(TaskResult::completed(Some(0)))
                },
            )
            .await;

        if let Some(contents) = corrupt_contents {
            tokio::fs::write(&info.status_file, contents).await?;
        } else {
            tokio::fs::remove_file(&info.status_file).await?;
        }

        assert!(manager.cancel(&info.task_id).await?);
        assert!(!manager.is_live_task(&info.task_id));
        let status = manager
            .status(&info.task_id)
            .await
            .ok_or_else(|| anyhow!("cancel should recreate terminal status"))?;
        assert_eq!(status.status, BackgroundTaskStatus::Failed);
        assert_eq!(status.error.as_deref(), Some("Cancelled by user"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_cancel_caller_still_publishes_terminal_status() -> Result<()> {
    let tmp = tempdir()?;
    let manager = Arc::new(BackgroundTaskManager::with_output_dir(
        tmp.path().to_path_buf(),
    ));
    let (_finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "cancelled-cancel-caller",
            false,
            false,
            move |_output_path| async move {
                finish_rx.await?;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;
    let publication = install_status_write_test_hook(info.status_file.clone());
    let cancel = tokio::spawn({
        let manager = Arc::clone(&manager);
        let task_id = info.task_id.clone();
        async move { manager.cancel(&task_id).await }
    });

    let staged_path = recv_staged(publication.staged).await?;
    let staged: TaskStatusFile =
        serde_json::from_str(&tokio::fs::read_to_string(staged_path).await?)?;
    assert_eq!(staged.status, BackgroundTaskStatus::Failed);
    cancel.abort();
    assert!(
        cancel
            .await
            .expect_err("cancel caller should abort")
            .is_cancelled()
    );
    publication
        .resume
        .send(())
        .expect("resume cancellation publication");
    drop(manager.status_updates.clone().lock_owned().await);

    assert!(!manager.is_live_task(&info.task_id));
    let status = manager
        .status(&info.task_id)
        .await
        .ok_or_else(|| anyhow!("terminal cancellation status missing"))?;
    assert_eq!(status.status, BackgroundTaskStatus::Failed);
    assert_eq!(status.error.as_deref(), Some("Cancelled by user"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_in_publication_cannot_be_overwritten_by_cancellation() -> Result<()> {
    let tmp = tempdir()?;
    let manager = Arc::new(BackgroundTaskManager::with_output_dir(
        tmp.path().to_path_buf(),
    ));
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let info = manager
        .spawn_with_notify(
            "bash",
            None,
            "completion-cancel-race",
            false,
            false,
            move |_output_path| async move {
                finish_rx.await?;
                Ok(TaskResult::completed(Some(0)))
            },
        )
        .await;
    let publication = install_status_write_test_hook(info.status_file.clone());
    finish_tx.send(()).expect("finish background task");
    let staged_path = recv_staged(publication.staged).await?;
    let staged: TaskStatusFile =
        serde_json::from_str(&tokio::fs::read_to_string(staged_path).await?)?;
    assert_eq!(staged.status, BackgroundTaskStatus::Completed);

    let cancel_manager = Arc::clone(&manager);
    let cancel_task_id = info.task_id.clone();
    let mut cancel = Box::pin(async move { cancel_manager.cancel(&cancel_task_id).await });
    assert!(
        poll_once(cancel.as_mut()).await.is_pending(),
        "cancellation must wait while completion owns publication"
    );
    publication
        .resume
        .send(())
        .expect("resume completion publication");
    let cancelled = cancel.await?;
    assert!(
        !cancelled,
        "terminal completion must win before cancellation"
    );

    let status = manager
        .status(&info.task_id)
        .await
        .ok_or_else(|| anyhow!("completed task status missing"))?;
    assert_eq!(status.status, BackgroundTaskStatus::Completed);
    assert_eq!(status.error, None);
    Ok(())
}
