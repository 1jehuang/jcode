use super::*;

impl BackgroundTaskManager {
    /// Arm (or re-arm) a stall watchdog for a running task.
    ///
    /// The watchdog fires a [`BusEvent::BackgroundTaskStalled`] after the task
    /// produces no new output bytes and no new progress/checkpoint events for
    /// `stall_wake_seconds`. Any activity resets the countdown. It fires at
    /// most once per silence episode and re-arms itself when activity resumes,
    /// so a task that stalls, recovers, and stalls again produces one wake per
    /// stall. The watchdog exits when the task leaves `Running`.
    ///
    /// Returns the effective (clamped) window, or `None` when the task does
    /// not exist or is no longer running.
    pub async fn arm_stall_watchdog(&self, task_id: &str, stall_wake_seconds: u64) -> Option<u64> {
        let status_guard = self.progress_updates.lock().await;
        let stall_wake_seconds = stall_wake_seconds.max(Self::MIN_STALL_WAKE_SECONDS);
        let status_path = self.status_path_for(task_id);
        let mut status = self.read_status_file(&status_path).await?;
        if status.status != BackgroundTaskStatus::Running {
            return None;
        }
        status.stall_wake_seconds = Some(stall_wake_seconds);
        self.write_status_file(&status_path, &status).await;
        drop(status_guard);

        let task_id_owned = task_id.to_string();
        let output_path = self.output_path_for(task_id);
        let status_path_clone = status_path.clone();
        let started_at = status.started_at.clone();
        let watchdogs = Arc::clone(&self.stall_watchdogs);

        let handle = tokio::spawn(async move {
            let mut last_len = fs::metadata(&output_path)
                .await
                .map(|meta| meta.len())
                .unwrap_or(0);
            let mut last_event_count: Option<usize> = None;
            let mut quiet_since = TokioInstant::now();
            let mut fired_this_episode = false;
            let window = Duration::from_secs(stall_wake_seconds);
            let mut poll = tokio::time::interval(Self::STALL_POLL_INTERVAL);
            poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // interval fires immediately on first tick; consume it.
            poll.tick().await;

            loop {
                poll.tick().await;

                let Ok(content) = fs::read_to_string(&status_path_clone).await else {
                    break;
                };
                let Ok(status) = serde_json::from_str::<TaskStatusFile>(&content) else {
                    break;
                };
                if status.status != BackgroundTaskStatus::Running {
                    break;
                }
                // Disarmed externally (e.g. re-armed with a new window by a
                // newer watchdog that overwrote this one's registry entry and
                // cleared the field first): keep it simple, exit if cleared.
                if status.stall_wake_seconds != Some(stall_wake_seconds) {
                    break;
                }

                let output_len = fs::metadata(&output_path)
                    .await
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                let event_count = status.event_history.len();
                let activity = output_len != last_len
                    || last_event_count.is_some_and(|prior| prior != event_count);
                last_len = output_len;
                last_event_count = Some(event_count);

                if activity {
                    quiet_since = TokioInstant::now();
                    fired_this_episode = false;
                    continue;
                }

                if fired_this_episode || quiet_since.elapsed() < window {
                    continue;
                }
                fired_this_episode = true;

                let running_secs = DateTime::parse_from_rfc3339(&started_at)
                    .ok()
                    .and_then(|started| (Utc::now() - started.with_timezone(&Utc)).to_std().ok())
                    .map(|duration| duration.as_secs_f64())
                    .unwrap_or_default();
                let output_tail = fs::read_to_string(&output_path)
                    .await
                    .map(|output| {
                        let tail: Vec<&str> = output.lines().rev().take(20).collect();
                        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
                    })
                    .unwrap_or_default();
                let output_tail = if output_tail.len() > 2000 {
                    crate::util::truncate_str(&output_tail, 2000).to_string()
                } else {
                    output_tail
                };

                Bus::global().publish(BusEvent::BackgroundTaskStalled(
                    crate::bus::BackgroundTaskStalled {
                        task_id: status.task_id.clone(),
                        tool_name: status.tool_name.clone(),
                        display_name: status.display_name.clone(),
                        session_id: status.session_id.clone(),
                        stall_wake_seconds,
                        running_secs,
                        output_tail,
                        output_file: output_path.clone(),
                        // Arming a stall watchdog is an explicit "wake me if
                        // this goes quiet" request, independent of the task's
                        // completion-delivery flags.
                        notify: true,
                        wake: true,
                    },
                ));
            }

            watchdogs.write().await.remove(&task_id_owned);
        });

        // Replace any existing watchdog for this task.
        if let Some(prior) = self
            .stall_watchdogs
            .write()
            .await
            .insert(task_id.to_string(), handle)
        {
            prior.abort();
        }

        Some(stall_wake_seconds)
    }

    /// Disarm the stall watchdog for a task, if one is armed.
    pub async fn disarm_stall_watchdog(&self, task_id: &str) -> bool {
        let removed = self.stall_watchdogs.write().await.remove(task_id);
        let disarmed = removed.is_some();
        if let Some(handle) = removed {
            handle.abort();
        }
        let _status_guard = self.progress_updates.lock().await;
        let status_path = self.status_path_for(task_id);
        if let Some(mut status) = self.read_status_file(&status_path).await
            && status.stall_wake_seconds.is_some()
        {
            status.stall_wake_seconds = None;
            self.write_status_file(&status_path, &status).await;
            return true;
        }
        disarmed
    }
}
