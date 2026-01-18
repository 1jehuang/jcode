//! Background task execution manager
//!
//! Allows tools to run in the background and notify the agent when complete.
//! Uses file-based storage for crash resilience + event channel for real-time notifications.

use crate::bus::{BackgroundTaskCompleted, BackgroundTaskStatus, Bus, BusEvent};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// Directory for background task output files
fn task_dir() -> PathBuf {
    std::env::temp_dir().join("jcode-bg-tasks")
}

/// Status file format (written to disk)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusFile {
    pub task_id: String,
    pub tool_name: String,
    pub session_id: String,
    pub status: BackgroundTaskStatus,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub duration_secs: Option<f64>,
}

/// Information returned when a background task is started
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundTaskInfo {
    pub task_id: String,
    pub output_file: PathBuf,
    pub status_file: PathBuf,
}

/// Internal tracking for a running task
struct RunningTask {
    task_id: String,
    tool_name: String,
    session_id: String,
    output_path: PathBuf,
    status_path: PathBuf,
    started_at: Instant,
    handle: JoinHandle<Result<TaskResult>>,
}

/// Result from a background task execution
pub struct TaskResult {
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

/// Manages background task execution
pub struct BackgroundTaskManager {
    tasks: Arc<RwLock<HashMap<String, RunningTask>>>,
    output_dir: PathBuf,
}

impl BackgroundTaskManager {
    /// Create a new background task manager
    pub fn new() -> Self {
        let output_dir = task_dir();
        // Ensure directory exists (sync is fine for init)
        std::fs::create_dir_all(&output_dir).ok();

        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            output_dir,
        }
    }

    /// Generate a short, unique task ID
    fn generate_task_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        // Use last 6 digits of timestamp + 4 random chars
        let rand_part: String = (0..4)
            .map(|_| {
                let idx = (rand::random::<u8>() % 36) as usize;
                "abcdefghijklmnopqrstuvwxyz0123456789"
                    .chars()
                    .nth(idx)
                    .unwrap()
            })
            .collect();
        format!("{}{}", &timestamp.to_string()[timestamp.to_string().len().saturating_sub(6)..], rand_part)
    }

    /// Spawn a background task
    ///
    /// The `execute_fn` receives the output file path and should write output there.
    /// It returns a TaskResult with exit code and optional error.
    pub async fn spawn<F, Fut>(
        &self,
        tool_name: &str,
        session_id: &str,
        execute_fn: F,
    ) -> BackgroundTaskInfo
    where
        F: FnOnce(PathBuf) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<TaskResult>> + Send,
    {
        let task_id = Self::generate_task_id();
        let output_path = self.output_dir.join(format!("{}.output", task_id));
        let status_path = self.output_dir.join(format!("{}.status.json", task_id));

        // Write initial status file
        let initial_status = TaskStatusFile {
            task_id: task_id.clone(),
            tool_name: tool_name.to_string(),
            session_id: session_id.to_string(),
            status: BackgroundTaskStatus::Running,
            exit_code: None,
            error: None,
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            duration_secs: None,
        };
        if let Ok(json) = serde_json::to_string_pretty(&initial_status) {
            let _ = std::fs::write(&status_path, json);
        }

        let output_path_clone = output_path.clone();
        let status_path_clone = status_path.clone();
        let task_id_clone = task_id.clone();
        let tool_name_owned = tool_name.to_string();
        let session_id_owned = session_id.to_string();
        let started_at = Instant::now();

        // Spawn the background task
        let handle = tokio::spawn(async move {
            let result = execute_fn(output_path_clone.clone()).await;

            let duration_secs = started_at.elapsed().as_secs_f64();
            let (status, exit_code, error) = match &result {
                Ok(task_result) => {
                    if task_result.error.is_some() {
                        (BackgroundTaskStatus::Failed, task_result.exit_code, task_result.error.clone())
                    } else {
                        (BackgroundTaskStatus::Completed, task_result.exit_code, None)
                    }
                }
                Err(e) => (BackgroundTaskStatus::Failed, None, Some(e.to_string())),
            };

            // Update status file
            let final_status = TaskStatusFile {
                task_id: task_id_clone.clone(),
                tool_name: tool_name_owned.clone(),
                session_id: session_id_owned.clone(),
                status: status.clone(),
                exit_code,
                error: error.clone(),
                started_at: chrono::Utc::now().to_rfc3339(), // Not accurate but close enough
                completed_at: Some(chrono::Utc::now().to_rfc3339()),
                duration_secs: Some(duration_secs),
            };
            if let Ok(json) = serde_json::to_string_pretty(&final_status) {
                let _ = tokio::fs::write(&status_path_clone, json).await;
            }

            // Read output preview for notification
            let output_preview = tokio::fs::read_to_string(&output_path_clone)
                .await
                .map(|s| {
                    if s.len() > 500 {
                        format!("{}...", &s[..500])
                    } else {
                        s
                    }
                })
                .unwrap_or_default();

            // Publish completion event to the bus
            Bus::global().publish(BusEvent::BackgroundTaskCompleted(BackgroundTaskCompleted {
                task_id: task_id_clone,
                tool_name: tool_name_owned,
                session_id: session_id_owned,
                status,
                exit_code,
                output_preview,
                output_file: output_path_clone,
                duration_secs,
            }));

            result
        });

        // Track the running task
        let running_task = RunningTask {
            task_id: task_id.clone(),
            tool_name: tool_name.to_string(),
            session_id: session_id.to_string(),
            output_path: output_path.clone(),
            status_path: status_path.clone(),
            started_at,
            handle,
        };

        self.tasks.write().await.insert(task_id.clone(), running_task);

        BackgroundTaskInfo {
            task_id,
            output_file: output_path,
            status_file: status_path,
        }
    }

    /// List all tasks (both running and completed from disk)
    pub async fn list(&self) -> Vec<TaskStatusFile> {
        let mut results = Vec::new();

        // Read all status files from disk
        if let Ok(mut entries) = fs::read_dir(&self.output_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false) {
                    if let Ok(content) = fs::read_to_string(&path).await {
                        if let Ok(status) = serde_json::from_str::<TaskStatusFile>(&content) {
                            results.push(status);
                        }
                    }
                }
            }
        }

        // Sort by task_id (which includes timestamp)
        results.sort_by(|a, b| b.task_id.cmp(&a.task_id));
        results
    }

    /// Get status of a specific task
    pub async fn status(&self, task_id: &str) -> Option<TaskStatusFile> {
        let status_path = self.output_dir.join(format!("{}.status.json", task_id));
        if let Ok(content) = fs::read_to_string(&status_path).await {
            serde_json::from_str(&content).ok()
        } else {
            None
        }
    }

    /// Get full output of a task
    pub async fn output(&self, task_id: &str) -> Option<String> {
        let output_path = self.output_dir.join(format!("{}.output", task_id));
        fs::read_to_string(&output_path).await.ok()
    }

    /// Cancel a running task
    pub async fn cancel(&self, task_id: &str) -> Result<bool> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.remove(task_id) {
            task.handle.abort();

            // Update status file
            let final_status = TaskStatusFile {
                task_id: task.task_id,
                tool_name: task.tool_name,
                session_id: task.session_id,
                status: BackgroundTaskStatus::Failed,
                exit_code: None,
                error: Some("Cancelled by user".to_string()),
                started_at: chrono::Utc::now().to_rfc3339(),
                completed_at: Some(chrono::Utc::now().to_rfc3339()),
                duration_secs: Some(task.started_at.elapsed().as_secs_f64()),
            };
            if let Ok(json) = serde_json::to_string_pretty(&final_status) {
                let _ = fs::write(&task.status_path, json).await;
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Clean up old task files (older than specified hours)
    pub async fn cleanup(&self, max_age_hours: u64) -> Result<usize> {
        let mut removed = 0;
        let cutoff = std::time::SystemTime::now()
            - std::time::Duration::from_secs(max_age_hours * 3600);

        if let Ok(mut entries) = fs::read_dir(&self.output_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if let Ok(metadata) = fs::metadata(&path).await {
                    if let Ok(modified) = metadata.modified() {
                        if modified < cutoff {
                            let _ = fs::remove_file(&path).await;
                            removed += 1;
                        }
                    }
                }
            }
        }

        Ok(removed)
    }
}

impl Default for BackgroundTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Global singleton for background task manager
static BACKGROUND_MANAGER: std::sync::OnceLock<BackgroundTaskManager> = std::sync::OnceLock::new();

/// Get the global background task manager
pub fn global() -> &'static BackgroundTaskManager {
    BACKGROUND_MANAGER.get_or_init(BackgroundTaskManager::new)
}

/// Helper to write output to file with streaming support
pub async fn write_output(path: &PathBuf, content: &str) -> Result<()> {
    let mut file = File::create(path).await?;
    file.write_all(content.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

/// Helper to append output to file (for streaming)
pub async fn append_output(path: &PathBuf, content: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(content.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}
