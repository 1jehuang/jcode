//! Cron-like task scheduler
//!
//! Schedules periodic background tasks (e.g., memory cleanup, cache warming,
//! session GC). Uses tokio's interval mechanism for timing.

use std::time::Duration;
use tokio::time::interval;
use tracing::{info, error};

/// A scheduled task that runs at a fixed interval
pub trait ScheduledTask: Send + Sync + 'static {
    /// Unique name for this task (used for logging)
    fn name(&self) -> &'static str;

    /// The interval between runs
    fn interval(&self) -> Duration;

    /// Execute one iteration of this task
    async fn execute(&self);
}

/// Scheduler that runs periodic tasks
pub struct TaskScheduler {
    /// Internal cancellation token
    cancel: tokio_util::sync::CancellationToken,
}

impl TaskScheduler {
    /// Create a new task scheduler
    pub fn new() -> Self {
        Self {
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Register and start a periodic task
    pub fn register<T: ScheduledTask>(&self, task: T) {
        let name = task.name();
        let interval_dur = task.interval();
        let cancel = self.cancel.clone();

        tokio::spawn(async move {
            let mut timer = interval(interval_dur);
            // Skip the first immediate tick
            timer.tick().await;

            loop {
                tokio::select! {
                    _ = timer.tick() => {
                        info!(task = %name, "Running scheduled task");
                        task.execute().await;
                    }
                    _ = cancel.cancelled() => {
                        info!(task = %name, "Scheduled task cancelled");
                        break;
                    }
                }
            }
        });

        info!(task = %name, interval = ?interval_dur, "Scheduled task registered");
    }

    /// Cancel all scheduled tasks
    pub fn shutdown(&self) {
        info!("Shutting down task scheduler");
        self.cancel.cancel();
    }
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}
