use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::config;
use crate::storage;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Ambient mode status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AmbientStatus {
    Idle,
    Running { detail: String },
    Scheduled { next_wake: DateTime<Utc> },
    Paused { reason: String },
    Disabled,
}

impl Default for AmbientStatus {
    fn default() -> Self {
        Self::Idle
    }
}

/// Priority for scheduled items
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Normal,
    High,
}

/// A scheduled ambient task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledItem {
    pub id: String,
    pub scheduled_for: DateTime<Utc>,
    pub context: String,
    pub priority: Priority,
    pub created_by_session: String,
    pub created_at: DateTime<Utc>,
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
}

// ---------------------------------------------------------------------------
// Storage paths
// ---------------------------------------------------------------------------

fn ambient_dir() -> Result<PathBuf> {
    let dir = storage::jcode_dir()?.join("ambient");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

fn state_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("state.json"))
}

fn queue_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("queue.json"))
}

fn lock_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("ambient.lock"))
}

fn transcripts_dir() -> Result<PathBuf> {
    let dir = ambient_dir()?.join("transcripts");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// AmbientState persistence
// ---------------------------------------------------------------------------

impl AmbientState {
    pub fn load() -> Result<Self> {
        let path = state_path()?;
        if path.exists() {
            storage::read_json(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        storage::write_json(&state_path()?, self)
    }

    pub fn record_cycle(&mut self, result: &AmbientCycleResult) {
        self.last_run = Some(result.ended_at);
        self.last_summary = Some(result.summary.clone());
        self.last_compactions = Some(result.compactions);
        self.last_memories_modified = Some(result.memories_modified);
        self.total_cycles += 1;

        match result.status {
            CycleStatus::Complete => {
                if let Some(ref req) = result.next_schedule {
                    let next = req
                        .wake_at
                        .unwrap_or_else(|| {
                            Utc::now()
                                + chrono::Duration::minutes(
                                    req.wake_in_minutes.unwrap_or(30) as i64,
                                )
                        });
                    self.status = AmbientStatus::Scheduled { next_wake: next };
                } else {
                    self.status = AmbientStatus::Idle;
                }
            }
            CycleStatus::Interrupted | CycleStatus::Incomplete => {
                self.status = AmbientStatus::Idle;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ScheduledQueue
// ---------------------------------------------------------------------------

pub struct ScheduledQueue {
    items: Vec<ScheduledItem>,
    path: PathBuf,
}

impl ScheduledQueue {
    pub fn load(path: PathBuf) -> Self {
        let items: Vec<ScheduledItem> = if path.exists() {
            storage::read_json(&path).unwrap_or_default()
        } else {
            Vec::new()
        };
        Self { items, path }
    }

    pub fn save(&self) -> Result<()> {
        storage::write_json(&self.path, &self.items)
    }

    pub fn push(&mut self, item: ScheduledItem) {
        self.items.push(item);
        let _ = self.save();
    }

    /// Pop items whose `scheduled_for` is in the past, sorted by priority
    /// (highest first) then by time (earliest first).
    pub fn pop_ready(&mut self) -> Vec<ScheduledItem> {
        let now = Utc::now();
        let (ready, remaining): (Vec<_>, Vec<_>) =
            self.items.drain(..).partition(|i| i.scheduled_for <= now);

        self.items = remaining;

        let mut ready = ready;
        // Sort: highest priority first, then earliest scheduled_for
        ready.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.scheduled_for.cmp(&b.scheduled_for))
        });

        if !ready.is_empty() {
            let _ = self.save();
        }

        ready
    }

    pub fn peek_next(&self) -> Option<&ScheduledItem> {
        self.items
            .iter()
            .min_by_key(|i| i.scheduled_for)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// ---------------------------------------------------------------------------
// AmbientLock  (single-instance guard)
// ---------------------------------------------------------------------------

pub struct AmbientLock {
    lock_path: PathBuf,
}

impl AmbientLock {
    /// Try to acquire the ambient lock.
    /// Returns `Ok(Some(lock))` if acquired, `Ok(None)` if another instance
    /// already holds it, or `Err` on I/O failure.
    pub fn try_acquire() -> Result<Option<Self>> {
        let path = lock_path()?;

        // Check existing lock
        if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                if let Ok(pid) = contents.trim().parse::<u32>() {
                    if is_pid_alive(pid) {
                        return Ok(None); // Another instance is running
                    }
                    // Stale lock from a dead process — remove it
                }
            }
            let _ = std::fs::remove_file(&path);
        }

        // Write our PID
        let pid = std::process::id();
        if let Some(parent) = path.parent() {
            storage::ensure_dir(parent)?;
        }
        std::fs::write(&path, pid.to_string())?;

        Ok(Some(Self { lock_path: path }))
    }

    pub fn release(self) -> Result<()> {
        let _ = std::fs::remove_file(&self.lock_path);
        // Drop runs, but we already cleaned up
        std::mem::forget(self);
        Ok(())
    }
}

impl Drop for AmbientLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    // kill(pid, 0) checks if the process exists without sending a signal
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn is_pid_alive(_pid: u32) -> bool {
    // Conservative: assume alive on non-Unix
    true
}

// ---------------------------------------------------------------------------
// AmbientManager
// ---------------------------------------------------------------------------

pub struct AmbientManager {
    state: AmbientState,
    queue: ScheduledQueue,
}

impl AmbientManager {
    pub fn new() -> Result<Self> {
        // Ensure storage layout exists
        let _ = ambient_dir()?;
        let _ = transcripts_dir()?;

        let state = AmbientState::load()?;
        let queue = ScheduledQueue::load(queue_path()?);

        Ok(Self { state, queue })
    }

    pub fn is_enabled() -> bool {
        config().ambient.enabled
    }

    /// Check whether it's time to run a cycle based on current state and queue.
    pub fn should_run(&self) -> bool {
        if !Self::is_enabled() {
            return false;
        }

        match &self.state.status {
            AmbientStatus::Disabled | AmbientStatus::Paused { .. } => false,
            AmbientStatus::Running { .. } => false, // already running
            AmbientStatus::Idle => true,
            AmbientStatus::Scheduled { next_wake } => Utc::now() >= *next_wake,
        }
    }

    pub fn record_cycle_result(&mut self, result: AmbientCycleResult) -> Result<()> {
        self.state.record_cycle(&result);
        self.state.save()?;

        // If the cycle produced a schedule request, enqueue it
        if let Some(ref req) = result.next_schedule {
            self.schedule(req.clone())?;
        }

        Ok(())
    }

    /// Add a schedule request to the queue. Returns the item ID.
    pub fn schedule(&mut self, request: ScheduleRequest) -> Result<String> {
        let id = format!("sched_{:08x}", rand::random::<u32>());
        let scheduled_for = request.wake_at.unwrap_or_else(|| {
            Utc::now()
                + chrono::Duration::minutes(request.wake_in_minutes.unwrap_or(30) as i64)
        });

        let item = ScheduledItem {
            id: id.clone(),
            scheduled_for,
            context: request.context,
            priority: request.priority,
            created_by_session: String::new(), // filled in by caller if needed
            created_at: Utc::now(),
        };

        self.queue.push(item);
        Ok(id)
    }

    pub fn state(&self) -> &AmbientState {
        &self.state
    }

    pub fn queue(&self) -> &ScheduledQueue {
        &self.queue
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_ambient_status_default() {
        let status = AmbientStatus::default();
        assert_eq!(status, AmbientStatus::Idle);
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
    }

    #[test]
    fn test_scheduled_queue_push_and_pop() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let mut queue = ScheduledQueue::load(path);
        assert!(queue.is_empty());

        let past = Utc::now() - Duration::minutes(5);
        let future = Utc::now() + Duration::hours(1);

        queue.push(ScheduledItem {
            id: "s1".into(),
            scheduled_for: past,
            context: "past item".into(),
            priority: Priority::Low,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        queue.push(ScheduledItem {
            id: "s2".into(),
            scheduled_for: future,
            context: "future item".into(),
            priority: Priority::High,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        assert_eq!(queue.len(), 2);

        let ready = queue.pop_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "s1");

        // Future item still in queue
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.peek_next().unwrap().id, "s2");
    }

    #[test]
    fn test_pop_ready_sorts_by_priority_then_time() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let mut queue = ScheduledQueue::load(path);
        let past1 = Utc::now() - Duration::minutes(10);
        let past2 = Utc::now() - Duration::minutes(5);

        queue.push(ScheduledItem {
            id: "low_early".into(),
            scheduled_for: past1,
            context: "low early".into(),
            priority: Priority::Low,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        queue.push(ScheduledItem {
            id: "high_late".into(),
            scheduled_for: past2,
            context: "high late".into(),
            priority: Priority::High,
            created_by_session: "test".into(),
            created_at: Utc::now(),
        });

        let ready = queue.pop_ready();
        assert_eq!(ready.len(), 2);
        // High priority should come first
        assert_eq!(ready[0].id, "high_late");
        assert_eq!(ready[1].id, "low_early");
    }

    #[test]
    fn test_ambient_state_record_cycle() {
        let mut state = AmbientState::default();
        assert_eq!(state.total_cycles, 0);

        let result = AmbientCycleResult {
            summary: "Merged 2 duplicates".into(),
            memories_modified: 3,
            compactions: 1,
            proactive_work: None,
            next_schedule: None,
            started_at: Utc::now() - Duration::seconds(30),
            ended_at: Utc::now(),
            status: CycleStatus::Complete,
        };

        state.record_cycle(&result);
        assert_eq!(state.total_cycles, 1);
        assert_eq!(state.last_summary.as_deref(), Some("Merged 2 duplicates"));
        assert_eq!(state.last_compactions, Some(1));
        assert_eq!(state.last_memories_modified, Some(3));
        assert_eq!(state.status, AmbientStatus::Idle);
    }

    #[test]
    fn test_ambient_state_record_cycle_with_schedule() {
        let mut state = AmbientState::default();

        let result = AmbientCycleResult {
            summary: "Done".into(),
            memories_modified: 0,
            compactions: 0,
            proactive_work: None,
            next_schedule: Some(ScheduleRequest {
                wake_in_minutes: Some(15),
                wake_at: None,
                context: "check CI".into(),
                priority: Priority::Normal,
            }),
            started_at: Utc::now() - Duration::seconds(10),
            ended_at: Utc::now(),
            status: CycleStatus::Complete,
        };

        state.record_cycle(&result);
        assert!(matches!(state.status, AmbientStatus::Scheduled { .. }));
    }

    #[test]
    fn test_ambient_lock_release() {
        // Use a temp dir so we don't conflict with real state
        let tmp_dir = tempfile::tempdir().unwrap();
        let lock_file = tmp_dir.path().join("test.lock");

        // Manually create a lock to test release/drop
        std::fs::write(&lock_file, std::process::id().to_string()).unwrap();
        let lock = AmbientLock {
            lock_path: lock_file.clone(),
        };
        lock.release().unwrap();
        assert!(!lock_file.exists());
    }

    #[test]
    fn test_schedule_id_format() {
        let id = format!("sched_{:08x}", rand::random::<u32>());
        assert!(id.starts_with("sched_"));
        assert_eq!(id.len(), 6 + 8); // "sched_" + 8 hex chars
    }
}
