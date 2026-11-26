use super::*;

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;

// Holds disappear with the process; persisted delivery preferences remain intact.
pub(super) type HeldDeliveries = Arc<std::sync::Mutex<HashMap<String, Option<TaskStatusFile>>>>;

pub(super) fn held_wake(held: &HeldDeliveries, status: &TaskStatusFile, wake: bool) -> bool {
    let mut held = held
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match held.get_mut(&status.task_id) {
        Some(completion) => {
            *completion = Some(status.clone());
            false
        }
        None => wake,
    }
}

// An abandoned wait may return natural completions, never a requested stop.
pub(super) fn held_stop_wake(held: &HeldDeliveries, task_id: &str, wake: bool) -> bool {
    let mut held = held
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match held.get_mut(task_id) {
        Some(suppressed) => {
            *suppressed = None;
            false
        }
        None => wake,
    }
}

impl BackgroundTaskManager {
    /// Update delivery behavior for an existing background task.
    /// This supports retroactively enabling notify/wake after the task was already started.
    pub async fn update_delivery(
        &self,
        task_id: &str,
        notify: bool,
        wake: bool,
    ) -> Result<Option<TaskStatusFile>> {
        let _status_guard = self.progress_updates.lock().await;
        let (notify, wake) = normalize_delivery(notify, wake);
        let status_path = self.status_path_for(task_id);
        let Some(mut status) = self.read_status_file(&status_path).await else {
            return Ok(None);
        };
        status.notify = notify;
        status.wake = wake;
        let event_status = status.status.clone();
        let event_exit_code = status.exit_code;
        let event_progress = status.progress.clone();
        push_task_event(
            &mut status,
            BackgroundTaskEventRecord {
                kind: BackgroundTaskEventKind::DeliveryUpdated,
                timestamp: Utc::now().to_rfc3339(),
                message: Some(format!("notify={}, wake={}", notify, wake)),
                status: Some(event_status),
                exit_code: event_exit_code,
                progress: event_progress,
            },
        );
        Self::write_status_file(&status_path, &status);

        if let Some(task) = self.tasks.read().await.get(task_id)
            && let Err(error) = task.delivery_flags.send((notify, wake))
        {
            crate::logging::debug(&format!("Background delivery subscriber closed: {error}"));
        }

        Ok(Some(status))
    }

    /// Hold model delivery of a running task's completion while a foreground
    /// `bg wait` in the task's own session owns its result. False when there
    /// is nothing to hold. The hold lives only in this process's memory.
    pub async fn hold_delivery(&self, task_id: &str, session_id: &str) -> bool {
        let _status_guard = self.progress_updates.lock().await;
        let status_path = self.status_path_for(task_id);
        let Some(status) = self.read_status_file(&status_path).await else {
            return false;
        };
        if status.status != BackgroundTaskStatus::Running
            || !status.wake
            || status.session_id != session_id
        {
            return false;
        }
        let mut held = self
            .held_deliveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.contains_key(task_id) {
            return false;
        }
        held.insert(task_id.to_string(), None);
        true
    }

    /// Snapshot a held completion without ending its delivery hold.
    pub fn held_completion(&self, task_id: &str) -> Option<TaskStatusFile> {
        self.held_deliveries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(task_id)
            .cloned()
            .flatten()
    }

    /// End a [`Self::hold_delivery`]. A completion that would have woken the
    /// model went out without waking it during the hold; `deliver` publishes
    /// it to the model now, for a waiter that ends without returning it. A
    /// requested stop during the hold stays passive.
    pub async fn release_delivery(&self, task_id: &str, deliver: bool) -> Option<TaskStatusFile> {
        let (status, suppressed) = {
            let _status_guard = self.progress_updates.lock().await;
            let status = self.read_status_file(&self.status_path_for(task_id)).await;
            let suppressed = self
                .held_deliveries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(task_id)
                .flatten();
            let status = match status {
                Some(status) if status.status != BackgroundTaskStatus::Running => status,
                status => suppressed.clone().or(status)?,
            };
            (status, suppressed.is_some())
        };
        if deliver && suppressed && status.wake {
            let output = match fs::read_to_string(self.output_path_for(&status.task_id)).await {
                Ok(output) => output,
                Err(error) => {
                    crate::logging::debug(&format!(
                        "Cannot read released background output: {error}"
                    ));
                    String::new()
                }
            };
            self.publish_completion(&status, true, output);
        }
        Some(status)
    }

    // Capture wake ownership before terminal persistence, never after a waiter can release it.
    pub(super) fn publish_completion(&self, status: &TaskStatusFile, wake: bool, output: String) {
        let output_file = self.output_path_for(&status.task_id);
        let output_preview = if output.len() > 500 {
            format!("{}...", crate::util::truncate_str(&output, 500))
        } else {
            output
        };
        Bus::global().publish(BusEvent::BackgroundTaskCompleted(BackgroundTaskCompleted {
            task_id: status.task_id.clone(),
            tool_name: status.tool_name.clone(),
            display_name: status.display_name.clone(),
            session_id: status.session_id.clone(),
            status: status.status.clone(),
            exit_code: status.exit_code,
            output_preview,
            output_file,
            duration_secs: status.duration_secs.unwrap_or(0.0),
            notify: status.notify,
            wake,
        }));
    }
}
