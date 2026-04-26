use crate::bus::{
    BackgroundTaskProgress, BackgroundTaskProgressEvent, BackgroundTaskProgressKind,
    BackgroundTaskProgressSource, BackgroundTaskStatus,
};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Directory for background task output files
pub(super) fn task_dir() -> PathBuf {
    std::env::temp_dir().join("jcode-bg-tasks")
}

pub(super) const EXIT_MARKER_PREFIX: &str = "--- Command finished with exit code: ";
const MAX_EVENT_HISTORY: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskEventKind {
    Progress,
    Checkpoint,
    Completed,
    Failed,
    Superseded,
    Cancelled,
    DeliveryUpdated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundTaskEventRecord {
    pub kind: BackgroundTaskEventKind,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<BackgroundTaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<BackgroundTaskProgress>,
}

/// Status file format (written to disk)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusFile {
    pub task_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub session_id: String,
    pub status: BackgroundTaskStatus,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub duration_secs: Option<f64>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub detached: bool,
    #[serde(default = "default_true")]
    pub notify: bool,
    #[serde(default)]
    pub wake: bool,
    #[serde(default)]
    pub progress: Option<BackgroundTaskProgress>,
    #[serde(default)]
    pub event_history: Vec<BackgroundTaskEventRecord>,
}

fn default_true() -> bool {
    true
}

pub(super) fn normalize_delivery(notify: bool, wake: bool) -> (bool, bool) {
    (notify || wake, wake)
}

pub(super) fn push_task_event(status: &mut TaskStatusFile, event: BackgroundTaskEventRecord) {
    status.event_history.push(event);
    let overflow = status.event_history.len().saturating_sub(MAX_EVENT_HISTORY);
    if overflow > 0 {
        status.event_history.drain(0..overflow);
    }
}

pub(super) fn progress_event_record(
    kind: BackgroundTaskEventKind,
    progress: BackgroundTaskProgress,
) -> BackgroundTaskEventRecord {
    BackgroundTaskEventRecord {
        kind,
        timestamp: Utc::now().to_rfc3339(),
        message: progress.message.clone(),
        status: Some(BackgroundTaskStatus::Running),
        exit_code: None,
        progress: Some(progress),
    }
}

fn terminal_event_kind(
    status: &BackgroundTaskStatus,
    error: Option<&str>,
) -> BackgroundTaskEventKind {
    match status {
        BackgroundTaskStatus::Completed => BackgroundTaskEventKind::Completed,
        BackgroundTaskStatus::Superseded => BackgroundTaskEventKind::Superseded,
        BackgroundTaskStatus::Failed if error == Some("Cancelled by user") => {
            BackgroundTaskEventKind::Cancelled
        }
        BackgroundTaskStatus::Failed => BackgroundTaskEventKind::Failed,
        BackgroundTaskStatus::Running => BackgroundTaskEventKind::Progress,
    }
}

pub(super) fn terminal_event_record(
    status: BackgroundTaskStatus,
    exit_code: Option<i32>,
    error: Option<&str>,
) -> BackgroundTaskEventRecord {
    BackgroundTaskEventRecord {
        kind: terminal_event_kind(&status, error),
        timestamp: Utc::now().to_rfc3339(),
        message: error.map(ToString::to_string),
        status: Some(status),
        exit_code,
        progress: None,
    }
}

pub(super) fn progress_wait_reason(
    event: Option<&BackgroundTaskEventRecord>,
) -> BackgroundTaskWaitReason {
    match event.map(|event| &event.kind) {
        Some(BackgroundTaskEventKind::Checkpoint) => BackgroundTaskWaitReason::Checkpoint,
        _ => BackgroundTaskWaitReason::Progress,
    }
}

pub fn format_progress_summary(progress: &BackgroundTaskProgress) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(percent) = progress.percent {
        parts.push(format!("{:.0}%", percent));
    } else if let (Some(current), Some(total)) = (progress.current, progress.total) {
        let mut counts = format!("{}/{}", current, total);
        if let Some(unit) = progress.unit.as_deref() {
            counts.push(' ');
            counts.push_str(unit);
        }
        parts.push(counts);
    } else if let Some(unit) = progress.unit.as_deref() {
        parts.push(unit.to_string());
    }

    if let Some(message) = progress.message.as_deref() {
        parts.push(message.to_string());
    }

    if parts.is_empty() {
        match progress.kind {
            BackgroundTaskProgressKind::Determinate => "progress reported".to_string(),
            BackgroundTaskProgressKind::Indeterminate => "working".to_string(),
        }
    } else {
        parts.join(" · ")
    }
}

pub fn render_progress_bar(progress: &BackgroundTaskProgress, width: usize) -> Option<String> {
    let percent = progress.percent?;
    let width = width.max(4);
    let filled = ((percent / 100.0) * width as f32).round() as usize;
    let filled = filled.min(width);
    Some(format!(
        "[{}{}]",
        "#".repeat(filled),
        "-".repeat(width.saturating_sub(filled))
    ))
}

fn progress_source_label(source: &BackgroundTaskProgressSource) -> &'static str {
    match source {
        BackgroundTaskProgressSource::Reported => "reported",
        BackgroundTaskProgressSource::ParsedOutput => "parsed",
        BackgroundTaskProgressSource::Heuristic => "estimated",
    }
}

pub fn format_progress_display(progress: &BackgroundTaskProgress, width: usize) -> String {
    let summary = format_progress_summary(progress);
    let source = progress_source_label(&progress.source);
    match render_progress_bar(progress, width) {
        Some(bar) => format!("{} {} ({})", bar, summary, source),
        None => format!("{} ({})", summary, source),
    }
}

pub(super) fn progress_equivalent(a: &BackgroundTaskProgress, b: &BackgroundTaskProgress) -> bool {
    a.kind == b.kind
        && a.percent == b.percent
        && a.message == b.message
        && a.current == b.current
        && a.total == b.total
        && a.unit == b.unit
        && a.eta_seconds == b.eta_seconds
        && a.source == b.source
}

#[derive(Debug, Clone, Default)]
pub struct RunningBackgroundProgress {
    pub task_id: String,
    pub tool_name: String,
    pub label: String,
    pub detail: Option<String>,
}

/// Information returned when a background task is started
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundTaskInfo {
    pub task_id: String,
    pub output_file: PathBuf,
    pub status_file: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskWaitReason {
    AlreadyFinished,
    Finished,
    Progress,
    Checkpoint,
    Timeout,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundTaskWaitResult {
    pub reason: BackgroundTaskWaitReason,
    pub task: TaskStatusFile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_event: Option<BackgroundTaskProgressEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_record: Option<BackgroundTaskEventRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundCleanupResult {
    pub matched_files: usize,
    pub removed_files: usize,
    pub skipped_running_files: usize,
}

/// Internal tracking for a running task
pub(super) struct RunningTask {
    pub(super) task_id: String,
    pub(super) tool_name: String,
    pub(super) display_name: Option<String>,
    pub(super) session_id: String,
    pub(super) status_path: PathBuf,
    pub(super) started_at: Instant,
    pub(super) started_at_rfc3339: String,
    pub(super) delivery_flags: watch::Sender<(bool, bool)>,
    pub(super) handle: JoinHandle<Result<TaskResult>>,
}

/// Result from a background task execution
pub struct TaskResult {
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub status: Option<BackgroundTaskStatus>,
}

impl TaskResult {
    pub fn completed(exit_code: Option<i32>) -> Self {
        Self {
            exit_code,
            error: None,
            status: Some(BackgroundTaskStatus::Completed),
        }
    }

    pub fn failed(exit_code: Option<i32>, error: impl Into<String>) -> Self {
        Self {
            exit_code,
            error: Some(error.into()),
            status: Some(BackgroundTaskStatus::Failed),
        }
    }

    pub fn superseded(exit_code: Option<i32>, detail: impl Into<String>) -> Self {
        Self {
            exit_code,
            error: Some(detail.into()),
            status: Some(BackgroundTaskStatus::Superseded),
        }
    }
}
