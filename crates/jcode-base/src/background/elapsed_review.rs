use super::*;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const REVIEW_AFTER: Duration = Duration::from_secs(5 * 60);

impl BackgroundTaskManager {
    pub(super) fn arm_elapsed_review(&self, tool_name: &str, task_id: &str) {
        // Waiting on a worker or another task is not itself a slow command.
        if !matches!(tool_name, "bash" | "selfdev-build") {
            return;
        }
        let status_path = self.status_path_for(task_id);
        let output_path = self.output_path_for(task_id);
        let updates = self.progress_updates.clone();
        let deadline = TokioInstant::now() + REVIEW_AFTER;
        tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            let tail = bounded_tail(&output_path).await;
            let _guard = updates.lock().await;
            let Ok(bytes) = fs::read(&status_path).await else {
                return;
            };
            let Ok(status) = serde_json::from_slice::<TaskStatusFile>(&bytes) else {
                return;
            };
            if let Some(event) = review_event(status, output_path, tail) {
                Bus::global().publish(BusEvent::BackgroundTaskStalled(event));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn elapsed_review_wakes_once_even_for_an_active_command() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let manager = BackgroundTaskManager::with_output_dir(dir.path().into());
        let mut events = Bus::global().subscribe();
        let info = manager
            .spawn("bash", "elapsed-review-test", |_| std::future::pending())
            .await;
        tokio::time::advance(Duration::from_secs(299)).await;
        tokio::time::resume();
        fs::write(&info.output_file, "still compiling").await?;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::time::resume();
        let task = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(BusEvent::BackgroundTaskStalled(task)) = events.recv().await
                    && task.task_id == info.task_id
                {
                    break task;
                }
            }
        })
        .await?;
        assert!(task.elapsed_review && task.wake);
        assert_eq!(task.output_tail, "still compiling");
        tokio::time::pause();
        tokio::time::advance(REVIEW_AFTER).await;
        while let Ok(event) = events.try_recv() {
            assert!(
                !matches!(event, BusEvent::BackgroundTaskStalled(e) if e.task_id == info.task_id)
            );
        }
        manager.cancel(&info.task_id).await?;
        Ok(())
    }
}

async fn bounded_tail(path: &std::path::Path) -> String {
    let Ok(mut file) = File::open(path).await else {
        return String::new();
    };
    let Ok(meta) = file.metadata().await else {
        return String::new();
    };
    if file
        .seek(std::io::SeekFrom::Start(meta.len().saturating_sub(2000)))
        .await
        .is_err()
    {
        return String::new();
    }
    let mut bytes = Vec::new();
    if let Err(error) = file.take(2000).read_to_end(&mut bytes).await {
        crate::logging::warn(&format!("Incomplete background output tail: {error}"));
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn review_event(
    status: TaskStatusFile,
    output_file: PathBuf,
    mut output_tail: String,
) -> Option<crate::bus::BackgroundTaskStalled> {
    if !status.notify
        || status.status != BackgroundTaskStatus::Running
        || (status.detached && !status.pid.is_some_and(crate::platform::is_process_running))
    {
        return None;
    }
    if let Some(progress) = &status.progress {
        output_tail = format!(
            "Current progress: {}\n{}",
            format_progress_summary(progress),
            output_tail
        );
    }
    Some(crate::bus::BackgroundTaskStalled {
        elapsed_review: true,
        running_secs: BackgroundTaskManager::status_duration_secs(&status.started_at, Utc::now())
            .unwrap_or(REVIEW_AFTER.as_secs_f64()),
        task_id: status.task_id,
        tool_name: status.tool_name,
        display_name: status.display_name,
        session_id: status.session_id,
        stall_wake_seconds: REVIEW_AFTER.as_secs(),
        output_tail,
        output_file,
        notify: true,
        wake: true,
    })
}
