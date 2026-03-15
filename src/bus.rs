#![allow(dead_code)]

use crate::message::ToolCall;
use crate::side_panel::SidePanelSnapshot;
use crate::todo::TodoItem;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Completed,
    Error,
}

impl ToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolStatus::Running => "running",
            ToolStatus::Completed => "completed",
            ToolStatus::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolEvent {
    pub session_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolStatus,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoEvent {
    pub session_id: String,
    pub todos: Vec<TodoItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSummaryState {
    pub status: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSummary {
    pub id: String,
    pub tool: String,
    pub state: ToolSummaryState,
}

/// Status update from a subagent (used by Task tool)
#[derive(Clone, Debug)]
pub struct SubagentStatus {
    pub session_id: String,
    pub status: String, // e.g., "calling API", "running grep", "streaming"
    pub model: Option<String>,
}

/// Progress update from a running batch tool call
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchSubcallState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchSubcallProgress {
    pub index: usize,
    pub tool_call: crate::message::ToolCall,
    pub state: BatchSubcallState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchProgress {
    pub session_id: String,
    /// Parent tool_call_id of the batch call
    pub tool_call_id: String,
    /// Total number of sub-calls in this batch
    pub total: usize,
    /// Number of sub-calls that have completed (success or error)
    pub completed: usize,
    /// Name of the sub-call that just completed
    pub last_completed: Option<String>,
    /// Sub-calls that are currently still running
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<ToolCall>,
    /// Ordered per-subcall progress state for richer UI rendering
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcalls: Vec<BatchSubcallProgress>,
}

/// Type of file operation for swarm awareness
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
    Edit,
}

impl FileOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileOp::Read => "read",
            FileOp::Write => "wrote",
            FileOp::Edit => "edited",
        }
    }
}

/// File touch event for swarm coordination
#[derive(Clone, Debug)]
pub struct FileTouch {
    pub session_id: String,
    pub path: PathBuf,
    pub op: FileOp,
    /// Human-readable summary like "edited lines 45-60" or "read 200 lines"
    pub summary: Option<String>,
}

/// Status of a background task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Failed,
}

/// Event sent when a background task completes
#[derive(Debug, Clone)]
pub struct BackgroundTaskCompleted {
    pub task_id: String,
    pub tool_name: String,
    pub session_id: String,
    pub status: BackgroundTaskStatus,
    pub exit_code: Option<i32>,
    pub output_preview: String,
    pub output_file: PathBuf,
    pub duration_secs: f64,
    pub notify: bool,
}

#[derive(Clone, Debug)]
pub struct LoginCompleted {
    pub provider: String,
    pub success: bool,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidePanelUpdated {
    pub session_id: String,
    pub snapshot: SidePanelSnapshot,
}

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Checking,
    Available { current: String, latest: String },
    Downloading { version: String },
    Installed { version: String },
    UpToDate,
    Error(String),
}

#[derive(Clone, Debug)]
pub enum BusEvent {
    ToolUpdated(ToolEvent),
    TodoUpdated(TodoEvent),
    SubagentStatus(SubagentStatus),
    BatchProgress(BatchProgress),
    /// File was touched by an agent (for swarm conflict detection)
    FileTouch(FileTouch),
    /// Background task completed
    BackgroundTaskCompleted(BackgroundTaskCompleted),
    /// Usage report fetched from providers
    UsageReport(Vec<crate::usage::ProviderUsage>),
    /// OAuth/login flow completed in the background
    LoginCompleted(LoginCompleted),
    /// Update check status from background thread
    UpdateStatus(UpdateStatus),
    /// External dictation command completed with transcript text
    DictationCompleted {
        text: String,
        mode: crate::protocol::TranscriptMode,
    },
    /// External dictation command failed
    DictationFailed {
        message: String,
    },
    /// Background compaction task finished (check_and_apply should be called)
    CompactionFinished,
    /// Provider's available models list may have changed
    ModelsUpdated,
    /// Side panel pages were updated for a session
    SidePanelUpdated(SidePanelUpdated),
}

pub struct Bus {
    sender: broadcast::Sender<BusEvent>,
}

impl Bus {
    pub fn global() -> &'static Bus {
        static INSTANCE: OnceLock<Bus> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let (sender, _) = broadcast::channel(256);
            Bus { sender }
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: BusEvent) {
        let _ = self.sender.send(event);
    }
}
