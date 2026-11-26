use super::*;

impl BackgroundTaskManager {
    /// Wait for a task to finish, emit progress, or reach the caller's maximum wait.
    ///
    /// This combines bus-driven wakeups with a light periodic status reconciliation so
    /// detached tasks, missed broadcast messages, or crash/reload edges still return no
    /// later than `max_wait` and can notice completion without active polling by the agent.
    pub async fn wait(
        &self,
        task_id: &str,
        max_wait: Duration,
        return_on_progress: bool,
    ) -> Option<BackgroundTaskWaitResult> {
        let mut bus_rx = Bus::global().subscribe();
        let initial = self.status(task_id).await?;
        if initial.status != BackgroundTaskStatus::Running {
            return Some(BackgroundTaskWaitResult {
                reason: BackgroundTaskWaitReason::AlreadyFinished,
                task: initial,
                progress_event: None,
                event_record: None,
            });
        }
        if max_wait.is_zero() {
            return Some(BackgroundTaskWaitResult {
                reason: BackgroundTaskWaitReason::Timeout,
                task: initial,
                progress_event: None,
                event_record: None,
            });
        }

        let mut last_progress = initial.progress.clone();
        let deadline = TokioInstant::now() + max_wait;
        let timeout = tokio::time::sleep_until(deadline);
        tokio::pin!(timeout);
        let mut poll = tokio::time::interval(Duration::from_secs(1));
        poll.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    let task = self.status(task_id).await?;
                    let reason = if task.status == BackgroundTaskStatus::Running {
                        BackgroundTaskWaitReason::Timeout
                    } else {
                        BackgroundTaskWaitReason::Finished
                    };
                    return Some(BackgroundTaskWaitResult {
                        reason,
                        task,
                        progress_event: None,
                        event_record: None,
                    });
                }
                _ = poll.tick() => {
                    let task = self.status(task_id).await?;
                    if task.status != BackgroundTaskStatus::Running {
                        return Some(BackgroundTaskWaitResult {
                            reason: BackgroundTaskWaitReason::Finished,
                            task,
                            progress_event: None,
                            event_record: None,
                        });
                    }
                    if return_on_progress && task.progress != last_progress {
                        let event_record = task.event_history.last().cloned();
                        return Some(BackgroundTaskWaitResult {
                            reason: progress_wait_reason(event_record.as_ref()),
                            progress_event: None,
                            task,
                            event_record,
                        });
                    }
                    last_progress = task.progress.clone();
                }
                event = bus_rx.recv() => {
                    match event {
                        Ok(BusEvent::BackgroundTaskCompleted(event)) if event.task_id == task_id => {
                            let task = self.status(task_id).await?;
                            return Some(BackgroundTaskWaitResult {
                                reason: BackgroundTaskWaitReason::Finished,
                                task,
                                progress_event: None,
                                event_record: None,
                            });
                        }
                        Ok(BusEvent::BackgroundTaskProgress(event)) if event.task_id == task_id => {
                            if return_on_progress {
                                let task = self.status(task_id).await?;
                                let event_record = task.event_history.last().cloned();
                                return Some(BackgroundTaskWaitResult {
                                    reason: progress_wait_reason(event_record.as_ref()),
                                    task,
                                    progress_event: Some(event),
                                    event_record,
                                });
                            }
                        }
                        Ok(BusEvent::BackgroundTaskStalled(event)) if event.task_id == task_id => {
                            // A stall is always worth returning: the caller
                            // should decide whether to keep waiting or act.
                            let task = self.status(task_id).await?;
                            return Some(BackgroundTaskWaitResult {
                                reason: BackgroundTaskWaitReason::Stalled,
                                task,
                                progress_event: None,
                                event_record: None,
                            });
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let task = self.status(task_id).await?;
                            if task.status != BackgroundTaskStatus::Running {
                                return Some(BackgroundTaskWaitResult {
                                    reason: BackgroundTaskWaitReason::Finished,
                                    task,
                                    progress_event: None,
                                    event_record: None,
                                });
                            }
                            if return_on_progress && task.progress != last_progress {
                                let event_record = task.event_history.last().cloned();
                                return Some(BackgroundTaskWaitResult {
                                    reason: progress_wait_reason(event_record.as_ref()),
                                    progress_event: None,
                                    task,
                                    event_record,
                                });
                            }
                            last_progress = task.progress.clone();
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            let task = self.status(task_id).await?;
                            let reason = if task.status == BackgroundTaskStatus::Running {
                                BackgroundTaskWaitReason::Timeout
                            } else {
                                BackgroundTaskWaitReason::Finished
                            };
                            return Some(BackgroundTaskWaitResult {
                                reason,
                                task,
                                progress_event: None,
                                event_record: None,
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[tokio::test]
async fn task_ownership_waits_for_status_writer_without_settling() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let manager = BackgroundTaskManager::with_output_dir(tmp.path().to_path_buf());
    let info = manager.reserve_task_info();
    manager
        .register_detached_task(
            &info,
            "bash",
            None,
            "owner",
            i32::MAX as u32,
            &Utc::now().to_rfc3339(),
            true,
            true,
        )
        .await;
    let original = fs::read(&info.status_file).await?;
    let writer = manager.progress_updates.lock().await;
    fs::write(&info.status_file, b"{").await?;
    let ownership = manager.task_belongs_to_session(&info.task_id, "owner");
    tokio::pin!(ownership);
    assert!(
        tokio::time::timeout(Duration::ZERO, &mut ownership)
            .await
            .is_err()
    );
    fs::write(&info.status_file, &original).await?;
    drop(writer);
    assert!(ownership.await);
    assert!(
        !manager
            .task_belongs_to_session(&info.task_id, "other")
            .await
    );
    assert_eq!(
        fs::read(&info.status_file).await?,
        original,
        "ownership reads must not reconcile the ended detached task"
    );
    Ok(())
}
