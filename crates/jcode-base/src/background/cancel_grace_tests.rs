#![cfg(unix)]

use super::*;
use crate::bus::{BackgroundTaskProgressKind, BackgroundTaskProgressSource};
use anyhow::{Context, anyhow};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::oneshot;
use tokio::time::Duration;

const TEST_TIMEOUT: Duration = Duration::from_secs(1);

struct GraceBoundaryHook {
    entered: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

struct GraceBoundaryControl {
    entered: oneshot::Receiver<()>,
    resume: oneshot::Sender<()>,
}

fn grace_boundary_hooks() -> &'static std::sync::Mutex<HashMap<String, GraceBoundaryHook>> {
    static HOOKS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, GraceBoundaryHook>>> =
        std::sync::OnceLock::new();
    HOOKS.get_or_init(Default::default)
}

fn install_grace_boundary_hook(task_id: &str) -> GraceBoundaryControl {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let prior = grace_boundary_hooks()
        .lock()
        .expect("grace boundary hooks mutex poisoned")
        .insert(
            task_id.to_string(),
            GraceBoundaryHook {
                entered: entered_tx,
                resume: resume_rx,
            },
        );
    assert!(prior.is_none(), "grace boundary hook already installed");
    GraceBoundaryControl {
        entered: entered_rx,
        resume: resume_tx,
    }
}

pub(in crate::background) async fn at_grace_boundary(task_id: &str) {
    let hook = grace_boundary_hooks()
        .lock()
        .expect("grace boundary hooks mutex poisoned")
        .remove(task_id);
    if let Some(hook) = hook {
        let _ = hook.entered.send(());
        let _ = hook.resume.await;
    }
}

struct DetachedChild(Child);

impl Drop for DetachedChild {
    fn drop(&mut self) {
        if crate::platform::is_process_running(self.0.id())
            && crate::platform::signal_detached_process_group(self.0.id(), libc::SIGKILL).is_err()
        {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

async fn spawn_term_ignoring_child(ready_path: &std::path::Path) -> Result<DetachedChild> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("trap '' TERM; : > \"$1\"; while :; do sleep 60; done")
        .arg("cancel-grace-test")
        .arg(ready_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = DetachedChild(
        crate::platform::spawn_detached(&mut command).context("spawn detached child")?,
    );
    let ready_path = ready_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !ready_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        ready_path
            .exists()
            .then_some(())
            .ok_or_else(|| anyhow!("detached child readiness handshake timed out"))
    })
    .await??;
    Ok(child)
}

fn running_status(task_id: &str, pid: Option<u32>, detached: bool) -> TaskStatusFile {
    TaskStatusFile {
        task_id: task_id.to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: "cancel-grace-tests".to_string(),
        status: BackgroundTaskStatus::Running,
        exit_code: None,
        error: None,
        started_at: Utc::now().to_rfc3339(),
        completed_at: None,
        duration_secs: None,
        pid,
        owner_pid: None,
        owner_instance: None,
        detached,
        notify: false,
        wake: false,
        progress: None,
        event_history: Vec::new(),
        stall_wake_seconds: None,
    }
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

async fn wait_for_boundary(entered: oneshot::Receiver<()>) -> Result<()> {
    tokio::time::timeout(TEST_TIMEOUT, entered)
        .await
        .context("cancellation did not reach grace boundary")?
        .context("grace boundary hook dropped before entry")
}

async fn join_cancel(cancel: tokio::task::JoinHandle<Result<bool>>) -> Result<bool> {
    tokio::time::timeout(TEST_TIMEOUT, cancel)
        .await
        .context("cancellation did not finish after grace")?
        .context("cancellation task panicked")?
}

#[tokio::test(start_paused = true)]
async fn detached_cancel_grace_allows_unrelated_progress_publication() -> Result<()> {
    let tmp = tempdir()?;
    let manager = Arc::new(BackgroundTaskManager::with_output_dir(
        tmp.path().to_path_buf(),
    ));
    let child = spawn_term_ignoring_child(&tmp.path().join("child-ready")).await?;
    let target_id = "detached-cancel-target";
    let unrelated_id = "unrelated-running-task";
    manager
        .write_status_file(
            &manager.status_path_for(target_id),
            &running_status(target_id, Some(child.0.id()), true),
        )
        .await;
    manager
        .write_status_file(
            &manager.status_path_for(unrelated_id),
            &running_status(unrelated_id, None, false),
        )
        .await;

    let boundary = install_grace_boundary_hook(target_id);
    let cancel = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move {
            manager
                .cancel_with_grace(target_id, Duration::from_secs(3_600))
                .await
        }
    });
    wait_for_boundary(boundary.entered).await?;

    let updated = tokio::time::timeout(
        TEST_TIMEOUT,
        manager.update_progress(unrelated_id, progress(42.0)),
    )
    .await
    .context("unrelated progress was blocked by detached cancellation grace")??
    .context("unrelated running status disappeared")?;
    assert_eq!(updated.progress.and_then(|value| value.percent), Some(42.0));

    cancel.abort();
    assert!(
        tokio::time::timeout(TEST_TIMEOUT, cancel)
            .await
            .context("aborted cancellation did not finish")?
            .expect_err("cancel task should abort")
            .is_cancelled()
    );
    drop(boundary.resume);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn detached_cancel_preserves_terminal_status_published_during_grace() -> Result<()> {
    let tmp = tempdir()?;
    let manager = Arc::new(BackgroundTaskManager::with_output_dir(
        tmp.path().to_path_buf(),
    ));
    let child = spawn_term_ignoring_child(&tmp.path().join("child-ready")).await?;
    let target_id = "detached-terminal-race";
    let status_path = manager.status_path_for(target_id);
    manager
        .write_status_file(
            &status_path,
            &running_status(target_id, Some(child.0.id()), true),
        )
        .await;

    let boundary = install_grace_boundary_hook(target_id);
    let grace = Duration::from_secs(3_600);
    let cancel = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.cancel_with_grace(target_id, grace).await }
    });
    wait_for_boundary(boundary.entered).await?;

    let mut completed = running_status(target_id, Some(child.0.id()), true);
    completed.status = BackgroundTaskStatus::Completed;
    completed.exit_code = Some(0);
    completed.completed_at = Some(Utc::now().to_rfc3339());
    completed.notify = true;
    completed.wake = true;
    completed.progress = Some(progress(73.0));
    tokio::time::timeout(
        TEST_TIMEOUT,
        manager.write_status_file(&status_path, &completed),
    )
    .await
    .context("terminal publication was blocked by detached cancellation grace")?;

    boundary.resume.send(()).expect("resume cancellation grace");
    tokio::time::advance(grace).await;
    assert!(
        !join_cancel(cancel).await?,
        "newer terminal status should win"
    );

    let preserved = manager
        .status(target_id)
        .await
        .context("terminal status should remain persisted")?;
    assert_eq!(preserved.status, BackgroundTaskStatus::Completed);
    assert_eq!(preserved.exit_code, Some(0));
    assert_eq!(preserved.error, None);
    assert!(preserved.notify);
    assert!(preserved.wake);
    assert_eq!(
        preserved.progress.and_then(|value| value.percent),
        Some(73.0)
    );
    assert!(crate::platform::is_process_running(child.0.id()));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn detached_cancel_uses_progress_and_delivery_refreshed_during_grace() -> Result<()> {
    let tmp = tempdir()?;
    let manager = Arc::new(BackgroundTaskManager::with_output_dir(
        tmp.path().to_path_buf(),
    ));
    let child = spawn_term_ignoring_child(&tmp.path().join("child-ready")).await?;
    let target_id = "detached-refreshed-metadata";
    let status_path = manager.status_path_for(target_id);
    manager
        .write_status_file(
            &status_path,
            &running_status(target_id, Some(child.0.id()), true),
        )
        .await;

    let boundary = install_grace_boundary_hook(target_id);
    let grace = Duration::from_secs(3_600);
    let cancel = tokio::spawn({
        let manager = Arc::clone(&manager);
        async move { manager.cancel_with_grace(target_id, grace).await }
    });
    wait_for_boundary(boundary.entered).await?;

    tokio::time::timeout(
        TEST_TIMEOUT,
        manager.update_progress(target_id, progress(64.0)),
    )
    .await
    .context("progress update blocked during cancellation grace")??
    .context("running target disappeared during progress update")?;
    tokio::time::timeout(TEST_TIMEOUT, manager.update_delivery(target_id, true, true))
        .await
        .context("delivery update blocked during cancellation grace")??
        .context("running target disappeared during delivery update")?;

    boundary.resume.send(()).expect("resume cancellation grace");
    tokio::time::advance(grace).await;
    assert!(
        join_cancel(cancel).await?,
        "running target should be cancelled"
    );

    let cancelled = manager
        .status(target_id)
        .await
        .context("cancelled status should remain persisted")?;
    assert_eq!(cancelled.status, BackgroundTaskStatus::Failed);
    assert_eq!(cancelled.error.as_deref(), Some("Cancelled by user"));
    assert!(cancelled.notify);
    assert!(cancelled.wake);
    assert_eq!(
        cancelled.progress.and_then(|value| value.percent),
        Some(64.0)
    );
    Ok(())
}
