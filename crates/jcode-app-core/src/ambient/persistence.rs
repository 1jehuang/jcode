use anyhow::Result;
use chrono::Utc;
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

    /// Remove a scheduled item by ID, persisting the queue when found.
    pub fn remove_by_id(&mut self, id: &str) -> Result<Option<ScheduledItem>> {
        let Some(index) = self.items.iter().position(|item| item.id == id) else {
            return Ok(None);
        };

        let item = self.items.remove(index);
        self.save()?;
        Ok(Some(item))
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
        let base = item.scheduled_for.max(now);
        let next = ScheduledItem {
            id: format!("sched_{:08x}", rand::random::<u32>()),
            scheduled_for: base + chrono::Duration::minutes(repeat.every_minutes.max(1) as i64),
            created_at: now,
            repeat: Some(RepeatState {
                every_minutes: repeat.every_minutes,
                remaining,
                recurrence_id: repeat.recurrence_id.clone(),
                // The lease belongs to the run being delivered now, never to
                // the queued next occurrence.
                active_run: None,
            }),
            ..item.clone()
        };
        self.items.push(next);
        true
    }

    /// Remove every queued item of one recurrence series. Returns the count.
    pub fn remove_by_recurrence(&mut self, recurrence_id: &str) -> Result<usize> {
        let before = self.items.len();
        self.items.retain(|item| {
            item.repeat
                .as_ref()
                .map(|repeat| repeat.recurrence_id != recurrence_id)
                .unwrap_or(true)
        });
        let removed = before - self.items.len();
        if removed > 0 {
            self.save()?;
        }
        Ok(removed)
    }

    /// True when a previous occurrence of this item's series is still running.
    ///
    /// Dead owners (session file gone, terminal status) and stale leases
    /// (older than three intervals — a hung run must not block its series
    /// forever) read as free. Only consulted for recurring items; one-shots
    /// never carry a lease.
    fn spawn_lease_held(item: &ScheduledItem) -> bool {
        let Some(repeat) = item.repeat.as_ref() else {
            return false;
        };
        let Some(run) = repeat.active_run.as_ref() else {
            return false;
        };
        let stale_after =
            run.started_at + chrono::Duration::minutes(repeat.every_minutes.max(1) as i64 * 3);
        if Utc::now() >= stale_after {
            return false;
        }
        match crate::session::Session::load(&run.owner_session) {
            Ok(session) => !matches!(
                session.status,
                crate::session::SessionStatus::Closed
                    | crate::session::SessionStatus::Crashed { .. }
                    | crate::session::SessionStatus::Error { .. }
            ),
            // Owner session file gone: the run cannot still be working.
            Err(_) => false,
        }
    }

    /// Split due items into deliverable ones, deferring lease-held recurring
    /// items in place (due time pushed out one interval, iteration count
    /// untouched). Returns the deliverable set.
    fn partition_ready(&mut self, direct_only: bool) -> Vec<ScheduledItem> {
        let now = Utc::now();
        let mut ready = Vec::new();
        let mut remaining = Vec::with_capacity(self.items.len());
        let mut touched = false;
        for mut item in self.items.drain(..) {
            let due = item.scheduled_for <= now;
            let wanted = !direct_only || item.target.is_direct_delivery();
            if due && wanted {
                if item.repeat.is_some() && Self::spawn_lease_held(&item) {
                    let every = item
                        .repeat
                        .as_ref()
                        .map(|repeat| repeat.every_minutes)
                        .unwrap_or(30)
                        .max(1);
                    item.scheduled_for = now + chrono::Duration::minutes(every as i64);
                    remaining.push(item);
                    touched = true;
                    continue;
                }
                ready.push(item);
            } else {
                remaining.push(item);
            }
        }
        self.items = remaining;
        // Popping itself mutates the queue and must persist, even when no
        // recurrence or deferral touched anything.
        if !ready.is_empty() {
            touched = true;
        }
        for item in &ready {
            if self.requeue_next(item) {
                touched = true;
            }
        }
        if touched {
            let _ = self.save();
        }
        ready
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
