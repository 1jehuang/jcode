use super::*;
use anyhow::anyhow;

async fn started(
    manager: &BackgroundTaskManager,
    adopted: bool,
    wake: bool,
) -> (BackgroundTaskInfo, tokio::sync::oneshot::Sender<()>) {
    let (finish, wait) = tokio::sync::oneshot::channel();
    let info = if adopted {
        let task = tokio::spawn(async move {
            wait.await.unwrap();
            Ok(jcode_tool_types::ToolOutput::new("adopted result"))
        });
        manager
            .adopt_with_options("test", None, "wait-owner", true, wake, task)
            .await
    } else {
        manager
            .spawn_with_notify("test", None, "wait-owner", true, wake, |_| async move {
                wait.await.unwrap();
                Ok(TaskResult::completed(Some(0)))
            })
            .await
    };
    (info, finish)
}

async fn completion(
    events: &mut tokio::sync::broadcast::Receiver<BusEvent>,
    id: &str,
) -> BackgroundTaskCompleted {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let BusEvent::BackgroundTaskCompleted(event) = events.recv().await.unwrap()
                && event.task_id == id
            {
                return event;
            }
        }
    })
    .await
    .expect("completion event")
}

#[tokio::test]
async fn waiting_owns_spawned_and_adopted_completion_until_return_or_abandonment() {
    let dir = tempfile::tempdir().unwrap();
    let manager = BackgroundTaskManager::with_output_dir(dir.path().into());
    for adopted in [false, true] {
        for abandon in [false, true] {
            let mut events = Bus::global().subscribe();
            let (info, finish) = started(&manager, adopted, true).await;
            assert!(manager.hold_delivery(&info.task_id, "wait-owner").await);
            finish.send(()).unwrap();
            let event = completion(&mut events, &info.task_id).await;
            assert!(!event.wake, "the wait reply owns model delivery");
            assert!(event.notify, "display delivery is preserved");
            let returned = manager
                .release_delivery(&info.task_id, abandon)
                .await
                .unwrap();
            assert_eq!(returned.status, BackgroundTaskStatus::Completed);
            assert!(returned.wake, "persisted preference stays enabled");
            if adopted {
                assert_eq!(event.output_preview, "adopted result");
            }
            if abandon {
                assert!(completion(&mut events, &info.task_id).await.wake);
            }
            manager.release_delivery(&info.task_id, true).await;
            while let Ok(event) = events.try_recv() {
                if let BusEvent::BackgroundTaskCompleted(event) = event {
                    assert_ne!(
                        event.task_id, info.task_id,
                        "release cannot duplicate delivery"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn hold_is_owner_only_single_claim_process_local_and_not_a_delivery_override() {
    let dir = tempfile::tempdir().unwrap();
    let manager = BackgroundTaskManager::with_output_dir(dir.path().into());
    let mut events = Bus::global().subscribe();
    let (info, finish) = started(&manager, false, true).await;
    assert!(!manager.hold_delivery("missing", "wait-owner").await);
    assert!(!manager.hold_delivery(&info.task_id, "other-session").await);
    assert!(manager.hold_delivery(&info.task_id, "wait-owner").await);
    assert!(!manager.hold_delivery(&info.task_id, "wait-owner").await);
    let persisted = manager.read_status_file(&info.status_file).await.unwrap();
    assert!(persisted.wake);
    let restarted = BackgroundTaskManager::with_output_dir(dir.path().into());
    assert!(restarted.held_deliveries.lock().unwrap().is_empty());
    assert!(restarted.status(&info.task_id).await.unwrap().wake);
    // Abandoning before completion restores ordinary delivery, without a wake now.
    assert_eq!(
        manager
            .release_delivery(&info.task_id, true)
            .await
            .unwrap()
            .status,
        BackgroundTaskStatus::Running
    );
    finish.send(()).unwrap();
    assert!(completion(&mut events, &info.task_id).await.wake);
    assert!(!manager.hold_delivery(&info.task_id, "wait-owner").await);

    let (silent, finish) = started(&manager, false, false).await;
    assert!(!manager.hold_delivery(&silent.task_id, "wait-owner").await);
    finish.send(()).unwrap();
    assert!(!completion(&mut events, &silent.task_id).await.wake);
}

#[tokio::test]
async fn held_stop_cannot_be_republished_but_unheld_cancel_keeps_existing_policy() {
    let dir = tempfile::tempdir().unwrap();
    let manager = BackgroundTaskManager::with_output_dir(dir.path().into());
    for held in [false, true] {
        let mut events = Bus::global().subscribe();
        let (info, _finish) = started(&manager, false, true).await;
        if held {
            assert!(manager.hold_delivery(&info.task_id, "wait-owner").await);
        }
        assert!(manager.cancel(&info.task_id).await.unwrap());
        let stopped = completion(&mut events, &info.task_id).await;
        assert_eq!(stopped.wake, !held);
        assert_eq!(stopped.status, BackgroundTaskStatus::Failed);
        manager.release_delivery(&info.task_id, true).await;
        while let Ok(event) = events.try_recv() {
            if let BusEvent::BackgroundTaskCompleted(event) = event {
                assert_ne!(
                    event.task_id, info.task_id,
                    "abandonment must not restart a stop"
                );
            }
        }
    }
}

#[tokio::test]
async fn reconciled_detached_and_orphaned_completions_obey_wait_delivery() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let manager = BackgroundTaskManager::with_output_dir(dir.path().into());
    for detached in [false, true] {
        let mut events = Bus::global().subscribe();
        let (info, _finish) = started(&manager, false, true).await;
        assert!(manager.hold_delivery(&info.task_id, "wait-owner").await);
        let task = manager.tasks.write().await.remove(&info.task_id).unwrap();
        task.handle.abort();
        let _ = task.handle.await;
        let mut status = manager.read_status_file(&info.status_file).await.unwrap();
        status.owner_instance = Some("previous-process-image".into());
        if detached {
            let dead_pid = 2_000_000_000;
            assert!(!crate::platform::is_process_running(dead_pid));
            status.detached = true;
            status.pid = Some(dead_pid);
            fs::write(&info.output_file, format!("done\n{EXIT_MARKER_PREFIX}0\n")).await?;
        }
        BackgroundTaskManager::write_status_file(&info.status_file, &status);
        let terminal = manager
            .status(&info.task_id)
            .await
            .ok_or_else(|| anyhow!("status exists"))?;
        assert_ne!(terminal.status, BackgroundTaskStatus::Running);
        assert!(!completion(&mut events, &info.task_id).await.wake);
        manager.release_delivery(&info.task_id, true).await;
        assert!(completion(&mut events, &info.task_id).await.wake);
    }
    Ok(())
}

#[tokio::test]
async fn terminal_snapshot_survives_failed_status_publication() {
    let dir = tempfile::tempdir().unwrap();
    let manager = BackgroundTaskManager::with_output_dir(dir.path().into());
    for adopted in [false, true] {
        for abandon in [false, true] {
            for wake in [false, true] {
                let mut events = Bus::global().subscribe();
                let (info, finish) = started(&manager, adopted, true).await;
                assert!(manager.hold_delivery(&info.task_id, "wait-owner").await);
                manager
                    .update_delivery(&info.task_id, true, wake)
                    .await
                    .unwrap();
                fs::remove_file(&info.status_file).await.unwrap();
                fs::create_dir(&info.status_file).await.unwrap();
                finish.send(()).unwrap();
                assert!(!completion(&mut events, &info.task_id).await.wake);
                let returned = manager
                    .release_delivery(&info.task_id, abandon)
                    .await
                    .expect("held terminal snapshot survives unreadable status");
                assert_eq!(returned.status, BackgroundTaskStatus::Completed);
                assert_eq!(
                    returned.wake, wake,
                    "a foreground reply retains the result even after opting out of wakes"
                );
                if abandon && wake {
                    assert_eq!(
                        completion(&mut events, &info.task_id).await.status,
                        BackgroundTaskStatus::Completed
                    );
                }
                while let Ok(event) = events.try_recv() {
                    if let BusEvent::BackgroundTaskCompleted(event) = event {
                        assert_ne!(
                            event.task_id, info.task_id,
                            "respect wake opt-out and single delivery"
                        );
                    }
                }
            }
        }
    }
}

async fn pause_reconciliation_reads() -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>)
{
    let (entered, started) = tokio::sync::oneshot::channel();
    let (resume, pause) = std::sync::mpsc::channel();
    let blocker = tokio::task::spawn_blocking(move || {
        entered.send(()).unwrap();
        pause.recv().unwrap();
    });
    started.await.unwrap();
    (resume, blocker)
}

#[test]
fn orphan_completion_keeps_suppression_after_terminal_visibility_and_wait_release() -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()?
        .block_on(async {
            for held in [false, true] {
                for abandon in [false, true] {
                    let dir = tempfile::tempdir()?;
                    let manager = BackgroundTaskManager::with_output_dir(dir.path().into());
                    let mut events = Bus::global().subscribe();
                    let (info, _finish) = started(&manager, false, true).await;
                    if held {
                        assert!(manager.hold_delivery(&info.task_id, "wait-owner").await);
                    }
                    let task = manager.tasks.write().await.remove(&info.task_id).unwrap();
                    task.handle.abort();
                    let _ = task.handle.await;
                    let mut status = manager.read_status_file(&info.status_file).await.unwrap();
                    status.owner_instance = Some("previous-process-image".into());
                    BackgroundTaskManager::write_status_file(&info.status_file, &status);
                    let mut finalizing = Box::pin(
                        manager.finalize_orphaned_status_if_needed(status, &info.status_file),
                    );

                    let mut terminal_barrier = None;
                    for _ in 0..3 {
                        let (resume, blocker) = pause_reconciliation_reads().await;
                        assert!(futures::poll!(&mut finalizing).is_pending());
                        let visible: TaskStatusFile =
                            serde_json::from_slice(&std::fs::read(&info.status_file)?)?;
                        if visible.status != BackgroundTaskStatus::Running {
                            terminal_barrier = Some((resume, blocker));
                            break;
                        }
                        resume.send(())?;
                        blocker.await?;
                        // Finish the queued read without polling the finalizer again.
                        tokio::task::spawn_blocking(|| ()).await?;
                    }
                    let (resume, blocker) =
                        terminal_barrier.expect("terminal status becomes visible");
                    let mut release = Box::pin(manager.release_delivery(&info.task_id, abandon));
                    assert!(futures::poll!(&mut release).is_pending());
                    resume.send(())?;
                    blocker.await?;
                    let returned = release.await.expect("waiter sees terminal status");
                    assert_eq!(returned.status, BackgroundTaskStatus::Failed);

                    // The wait reply/abandonment finishes before the original publication resumes.
                    assert_eq!(finalizing.await.status, BackgroundTaskStatus::Failed);
                    let mut wakes = 0;
                    let mut completions = 0;
                    while let Ok(event) = events.try_recv() {
                        if let BusEvent::BackgroundTaskCompleted(event) = event
                            && event.task_id == info.task_id
                        {
                            assert!(event.notify);
                            completions += 1;
                            wakes += usize::from(event.wake);
                        }
                    }
                    assert_eq!(wakes, usize::from(!held || abandon));
                    assert_eq!(completions, 1 + usize::from(held && abandon));
                }
            }
            Ok(())
        })
}

#[test]
fn reconciliation_preserves_a_terminal_status_written_after_its_initial_snapshot() -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()?
        .block_on(async {
            for detached in [false, true] {
                let dir = tempfile::tempdir()?;
                let manager = BackgroundTaskManager::with_output_dir(dir.path().into());
                let mut events = Bus::global().subscribe();
                let (info, _finish) = started(&manager, false, true).await;
                assert!(manager.hold_delivery(&info.task_id, "wait-owner").await);
                let task = manager.tasks.write().await.remove(&info.task_id).unwrap();
                task.handle.abort();
                let _ = task.handle.await;
                let mut status = manager.read_status_file(&info.status_file).await.unwrap();
                status.owner_instance = Some("previous-process-image".into());
                if detached {
                    status.detached = true;
                    status.pid = Some(2_000_000_000);
                    assert!(!crate::platform::is_process_running(status.pid.unwrap()));
                }
                BackgroundTaskManager::write_status_file(&info.status_file, &status);
                let mut replacement = status.clone();
                replacement.status = BackgroundTaskStatus::Superseded;
                let (resume, blocker) = pause_reconciliation_reads().await;
                let mut finalizing = Box::pin(async {
                    if detached {
                        manager
                            .finalize_detached_status_if_needed(status, &info.status_file)
                            .await
                    } else {
                        manager
                            .finalize_orphaned_status_if_needed(status, &info.status_file)
                            .await
                    }
                });
                assert!(futures::poll!(&mut finalizing).is_pending());
                BackgroundTaskManager::write_status_file(&info.status_file, &replacement);
                resume.send(())?;
                blocker.await?;
                assert_eq!(finalizing.await.status, BackgroundTaskStatus::Superseded);
                let returned = manager
                    .release_delivery(&info.task_id, false)
                    .await
                    .unwrap();
                assert_eq!(returned.status, BackgroundTaskStatus::Superseded);
                while let Ok(event) = events.try_recv() {
                    if let BusEvent::BackgroundTaskCompleted(event) = event {
                        assert_ne!(
                            event.task_id, info.task_id,
                            "stale reconciliation published"
                        );
                    }
                }
            }
            Ok(())
        })
}
