use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::paths::{lock_path, state_path};
use super::{
    AmbientCycleResult, AmbientState, AmbientStatus, CycleStatus, RepeatState, ScheduledItem,
};
use crate::storage;

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
                    let next = req.wake_at.unwrap_or_else(|| {
                        Utc::now()
                            + chrono::Duration::minutes(req.wake_in_minutes.unwrap_or(30) as i64)
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

/// Persisted queue envelope. The version is a cross-process compare-and-
/// swap generation: every writer checks it before replacing the file, so a
/// CLI cancel and the ambient runner can no longer silently clobber each
/// other's mutations (cancelled series resurrected by a stale snapshot,
/// dequeued occurrences redelivered after restart).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueFile {
    #[serde(default)]
    version: u64,
    items: Vec<ScheduledItem>,
}

pub struct ScheduledQueue {
    items: Vec<ScheduledItem>,
    path: PathBuf,
    version: u64,
}

impl ScheduledQueue {
    pub fn load(path: PathBuf) -> Self {
        if !path.exists() {
            return Self {
                items: Vec::new(),
                path,
                version: 0,
            };
        }
        // Current envelope first, then the legacy bare-array shape (v0).
        if let Ok(file) = storage::read_json::<QueueFile>(&path) {
            return Self {
                items: file.items,
                path,
                version: file.version,
            };
        }
        Self {
            items: storage::read_json(&path).unwrap_or_default(),
            path,
            version: 0,
        }
    }

    pub fn save(&mut self) -> Result<()> {
        if self.try_save()? {
            return Ok(());
        }
        // Conflict with no retry context: adopt the fresh state so the next
        // mutation starts from disk truth instead of wedging on a stale
        // version forever.
        self.reload();
        anyhow::bail!("schedule queue changed on disk; reloaded, retry the operation")
    }

    /// Write the queue iff the file still holds the loaded version.
    /// Returns Ok(false) on a version conflict (nothing written).
    /// Io errors propagate as Err.
    fn try_save(&mut self) -> Result<bool> {
        if Self::disk_version(&self.path) != self.version {
            return Ok(false);
        }
        let next = self.version + 1;
        storage::write_json(
            &self.path,
            &QueueFile {
                version: next,
                items: self.items.clone(),
            },
        )?;
        self.version = next;
        Ok(true)
    }

    /// Re-read disk state, adopting its items and version.
    fn reload(&mut self) {
        *self = Self::load(self.path.clone());
    }

    fn disk_version(path: &PathBuf) -> u64 {
        if !path.exists() {
            return 0;
        }
        storage::read_json::<QueueFile>(path)
            .map(|file| file.version)
            .unwrap_or(0)
    }

    /// Queue an item, persisting it. Errors when the queue cannot retain
    /// the item, so callers never report success for lost work.
    pub fn push(&mut self, item: ScheduledItem) -> Result<()> {
        // Retry on cross-process conflicts: reload disk truth, re-apply.
        for _ in 0..3 {
            self.items.push(item.clone());
            match self.try_save() {
                Ok(true) => return Ok(()),
                Ok(false) => self.reload(),
                Err(error) => {
                    crate::logging::warn(&format!("schedule queue save failed, retrying: {error}"));
                    self.reload();
                }
            }
        }
        anyhow::bail!("schedule queue push failed: item could not be persisted")
    }

    /// Remove a scheduled item by ID, persisting the queue when found.
    pub fn remove_by_id(&mut self, id: &str) -> Result<Option<ScheduledItem>> {
        for _ in 0..3 {
            let Some(index) = self.items.iter().position(|item| item.id == id) else {
                return Ok(None);
            };
            let item = self.items.remove(index);
            match self.try_save() {
                Ok(true) => return Ok(Some(item)),
                Ok(false) => self.reload(),
                Err(error) => {
                    self.reload();
                    return Err(error);
                }
            }
        }
        anyhow::bail!("schedule queue changed on disk; reload and retry the cancel")
    }

    /// Queue the next occurrence of a recurring item, preserving the series.
    ///
    /// Called while popping, before delivery runs: the chain survives delivery
    /// failures, crashes, and missed wakes, because the next occurrence already
    /// exists. Returns false when the series is exhausted.
    fn requeue_next(&mut self, item: &ScheduledItem) -> bool {
        let Some(repeat) = item.repeat.as_ref() else {
            return false;
        };
        let remaining = match repeat.remaining {
            // max_iterations counts the first occurrence: remaining hits 1 on
            // the last run, so nothing is re-queued after it.
            Some(1) | Some(0) => return false,
            Some(n) => Some(n - 1),
            None => None,
        };
        let now = Utc::now();
        let interval = chrono::Duration::minutes(repeat.every_minutes.max(1) as i64);
        let base = item.scheduled_for.max(now);
        // Missed intervals between due time and now collapse into this one
        // catch-up fire; count them so `schedule list` shows the folding.
        let skipped_now = if now > item.scheduled_for {
            ((now - item.scheduled_for).num_seconds() / interval.num_seconds().max(1)) as u64
        } else {
            0
        };
        let next = ScheduledItem {
            id: format!("sched_{:08x}", rand::random::<u32>()),
            scheduled_for: base + interval,
            created_at: now,
            repeat: Some(RepeatState {
                every_minutes: repeat.every_minutes,
                remaining,
                recurrence_id: repeat.recurrence_id.clone(),
                skipped: repeat.skipped + skipped_now,
            }),
            ..item.clone()
        };
        self.items.push(next);
        true
    }

    /// Remove every queued item of one recurrence series. Returns the count.
    ///
    /// Reloads and retries on cross-process conflicts, so a runner holding
    /// a stale snapshot cannot resurrect the series by saving after us: the
    /// loser's write is rejected by the version check instead of silently
    /// restoring cancelled items.
    pub fn remove_by_recurrence(&mut self, recurrence_id: &str) -> Result<usize> {
        for _ in 0..3 {
            let before = self.items.len();
            self.items.retain(|item| {
                item.repeat
                    .as_ref()
                    .map(|repeat| repeat.recurrence_id != recurrence_id)
                    .unwrap_or(true)
            });
            let removed = before - self.items.len();
            if removed == 0 {
                return Ok(0);
            }
            match self.try_save() {
                Ok(true) => return Ok(removed),
                Ok(false) => self.reload(),
                Err(error) => {
                    self.reload();
                    return Err(error);
                }
            }
        }
        anyhow::bail!("schedule queue changed on disk; reload and retry the cancel")
    }

    /// Split due items into deliverable ones. Delivery through the ambient
    /// runner is serial — each spawn is awaited before the next item pops —
    /// so occurrences of one series can never overlap and no lease is needed.
    /// Returns the deliverable set.
    fn partition_ready(&mut self, direct_only: bool) -> Vec<ScheduledItem> {
        // Dequeue durability: nothing is delivered until its removal is on
        // disk. A failed save rolls the popped items back and delivers
        // nothing this cycle (the runner retries on its next poll) instead
        // of delivering now and redelivering after a restart. Version
        // conflicts reload disk truth and recompute rather than wedging.
        for _ in 0..3 {
            let now = Utc::now();
            let mut ready = Vec::new();
            let mut remaining = Vec::with_capacity(self.items.len());
            for item in self.items.drain(..) {
                let due = item.scheduled_for <= now;
                let wanted = !direct_only || item.target.is_direct_delivery();
                if due && wanted {
                    ready.push(item);
                } else {
                    remaining.push(item);
                }
            }
            self.items = remaining;
            if ready.is_empty() {
                return Vec::new();
            }
            // Popping itself mutates the queue and must persist, even when
            // no recurrence touched anything.
            for item in &ready {
                self.requeue_next(item);
            }
            match self.try_save() {
                Ok(true) => {
                    return ready;
                }
                Ok(false) => {
                    self.reload();
                }
                Err(error) => {
                    crate::logging::warn(&format!(
                        "schedule queue dequeue not persisted ({error}); re-queuing this cycle"
                    ));
                    self.reload();
                    return Vec::new();
                }
            }
        }
        crate::logging::warn("schedule queue conflict retries exhausted; skipping this cycle");
        self.reload();
        Vec::new()
    }

    /// Pop items whose `scheduled_for` is in the past, sorted by priority
    /// (highest first) then by time (earliest first).
    pub fn pop_ready(&mut self) -> Vec<ScheduledItem> {
        let mut ready = self.partition_ready(false);

        // Sort: highest priority first, then earliest scheduled_for
        ready.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.scheduled_for.cmp(&b.scheduled_for))
        });

        ready
    }

    /// Remove and return ready items targeted at a specific direct-delivery session,
    /// leaving ambient-targeted queue items intact for the ambient agent to process.
    pub fn take_ready_direct_items(&mut self) -> Vec<ScheduledItem> {
        let mut ready_direct = self.partition_ready(true);

        ready_direct.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.scheduled_for.cmp(&b.scheduled_for))
        });

        ready_direct
    }

    pub fn peek_next(&self) -> Option<&ScheduledItem> {
        self.items.iter().min_by_key(|i| i.scheduled_for)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[ScheduledItem] {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut [ScheduledItem] {
        &mut self.items
    }
}

// ---------------------------------------------------------------------------
// AmbientLock  (single-instance guard)
// ---------------------------------------------------------------------------

pub struct AmbientLock {
    pub(crate) lock_path: PathBuf,
}

impl AmbientLock {
    /// Try to acquire the ambient lock.
    /// Returns `Ok(Some(lock))` if acquired, `Ok(None)` if another instance
    /// already holds it, or `Err` on I/O failure.
    pub fn try_acquire() -> Result<Option<Self>> {
        let path = lock_path()?;

        // Check existing lock
        if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path)
                && let Ok(pid) = contents.trim().parse::<u32>()
                && is_pid_alive(pid)
            {
                return Ok(None); // Another instance is running
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

fn is_pid_alive(pid: u32) -> bool {
    crate::platform::is_process_running(pid)
}
