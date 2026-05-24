//! Background task executor
//!
//! Manages long-running background tasks like model loading, indexing,
//! memory compaction, and periodic health checks.

use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::task::JoinHandle;
use tracing::{info, warn, error};

/// Maximum number of concurrent background tasks
const MAX_CONCURRENT_TASKS: usize = 4;

/// A background task that can be started and stopped
pub trait BackgroundTask: Send + Sync + 'static {
    /// Unique name for this task (used for logging and dedup)
    fn name(&self) -> &'static str;

    /// Run the task - this should loop until cancelled
    async fn run(self: Box<Self>, cancel: tokio_util::sync::CancellationToken);
}

/// Manages background task execution with concurrency limits
pub struct BackgroundRunner {
    /// Semaphore to limit concurrent background tasks
    semaphore: Arc<Semaphore>,
    /// Currently running tasks
    handles: Arc<RwLock<Vec<JoinHandle<()>>>>,
    /// Global cancellation token
    cancel: tokio_util::sync::CancellationToken,
}

impl BackgroundRunner {
    /// Create a new background runner
    pub fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS)),
            handles: Arc::new(RwLock::new(Vec::new())),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Spawn a background task with concurrency limiting
    pub async fn spawn<T: BackgroundTask>(&self, task: T) {
        let permit = self.semaphore.clone().acquire_owned().await;
        match permit {
            Ok(p) => {
                let name = task.name();
                let cancel = self.cancel.clone();
                let handles = self.handles.clone();

                let handle = tokio::spawn(async move {
                    info!(task = %name, "Background task started");
                    Box::new(task).run(cancel).await;
                    info!(task = %name, "Background task finished");
                    drop(p);
                });

                handles.write().await.push(handle);
                info!(task = %name, "Background task spawned");
            }
            Err(_) => {
                warn!("Background task queue full, task rejected");
            }
        }
    }

    /// Cancel all running tasks and wait for them to finish
    pub async fn shutdown(&self) {
        info!("Shutting down background runner");
        self.cancel.cancel();

        // Take ownership of all handles (drain from write lock)
        let handles = self.handles.write().await.drain(..).collect::<Vec<_>>();
        for handle in handles {
            if let Err(e) = handle.await {
                error!(error = %e, "Background task panicked during shutdown");
            }
        }
    }

    /// Check if the runner has been cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl Default for BackgroundRunner {
    fn default() -> Self {
        Self::new()
    }
}
