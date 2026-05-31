use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod directives;
mod manager;
mod paths;
mod persistence;
mod prompt;
pub mod runner;
pub mod scheduler;

pub use directives::{
    UserDirective, add_directive, has_pending_directives, load_directives, take_pending_directives,
};
pub use manager::AmbientManager;
pub use persistence::{AmbientLock, ScheduledQueue};
#[cfg(test)]
pub(crate) use prompt::format_duration_rough;
pub use prompt::{
    MemoryGraphHealth, RecentSessionInfo, ResourceBudget, build_ambient_system_prompt,
    format_minutes_human, format_scheduled_session_message, gather_feedback_memories,
    gather_memory_graph_health, gather_recent_sessions,
};

use crate::storage;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Context passed from the ambient runner to a visible TUI cycle.
/// Saved to `~/.jcode/ambient/visible_cycle.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisibleCycleContext {
    pub system_prompt: String,
    pub initial_message: String,
}

impl VisibleCycleContext {
    pub fn context_path() -> Result<PathBuf> {
        Ok(storage::jcode_dir()?
            .join("ambient")
            .join("visible_cycle.json"))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::context_path()?;
        if let Some(parent) = path.parent() {
            storage::ensure_dir(parent)?;
        }
        storage::write_json(&path, self)
    }

    pub fn load() -> Result<Self> {
        let path = Self::context_path()?;
        storage::read_json(&path)
    }

    pub fn result_path() -> Result<PathBuf> {
        Ok(storage::jcode_dir()?
            .join("ambient")
            .join("cycle_result.json"))
    }
}

/// Ambient mode status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum AmbientStatus {
    #[default]
    Idle,
    Running {
        detail: String,
    },
    Scheduled {
        next_wake: DateTime<Utc>,
    },
    Paused {
        reason: String,
    },
    Disabled,
}

/// Priority for scheduled items
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Normal,
    High,
}

/// Where a scheduled task should be delivered when it becomes due.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleTarget {
    /// Wake the ambient agent and hand it the queued task.
    #[default]
    Ambient,
    /// Deliver the reminder back into a specific interactive session.
    Session { session_id: String },
    /// Spawn a single new session derived from the originating session.
    Spawn { parent_session_id: String },
}

impl ScheduleTarget {
    pub fn is_direct_delivery(&self) -> bool {
        matches!(self, Self::Session { .. } | Self::Spawn { .. })
    }
}

/// A scheduled ambient task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledItem {
    pub id: String,
    pub scheduled_for: DateTime<Utc>,
    pub context: String,
    pub priority: Priority,
    #[serde(default)]
    pub target: ScheduleTarget,
    pub created_by_session: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relevant_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

/// Persistent ambient state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AmbientState {
    pub status: AmbientStatus,
    pub last_run: Option<DateTime<Utc>>,
    pub last_summary: Option<String>,
    pub last_compactions: Option<u32>,
    pub last_memories_modified: Option<u32>,
    pub total_cycles: u64,
}

/// Result from an ambient cycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmbientCycleResult {
    pub summary: String,
    pub memories_modified: u32,
    pub compactions: u32,
    pub proactive_work: Option<String>,
    pub next_schedule: Option<ScheduleRequest>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub status: CycleStatus,
    /// Full conversation transcript (markdown) for email notifications
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CycleStatus {
    Complete,
    Interrupted,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRequest {
    pub wake_in_minutes: Option<u32>,
    pub wake_at: Option<DateTime<Utc>>,
    pub context: String,
    pub priority: Priority,
    #[serde(default)]
    pub target: ScheduleTarget,
    #[serde(default)]
    pub created_by_session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relevant_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "ambient_tests.rs"]
mod ambient_tests;
