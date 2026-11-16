use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;

use super::paths::{lock_path, state_path};
use super::{AmbientCycleResult, AmbientState, AmbientStatus, CycleStatus, ScheduledItem};
use crate::storage;

#[cfg(test)]
#[path = "queue_transaction_tests.rs"]
mod queue_transaction_tests;

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

    /// Lock a separate stable inode: atomic JSON replacement must not replace
    /// the inode carrying the lock. Reload under lock, and publish only after
    /// the entire mutation was durably saved. Never write a cached snapshot.
    pub(super) fn transact<T>(
        &mut self,
        update: impl FnOnce(&mut Vec<ScheduledItem>) -> Result<T>,
    ) -> Result<T> {
        if let Some(parent) = self.path.parent() {
            storage::ensure_dir(parent)?;
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("json.lock"))?;
        lock.lock()?;
        let mut latest = if self.path.exists() {
            storage::read_json::<Vec<ScheduledItem>>(&self.path)?
        } else {
            Vec::new()
        };
        let before = latest.clone();
        let result = update(&mut latest)?;
        if latest != before {
            storage::write_json(&self.path, &latest)?;
        }
        self.items = latest;
        Ok(result)
    }

    pub fn try_push(&mut self, item: ScheduledItem) -> Result<()> {
        self.transact(|items| {
            items.push(item);
            Ok(())
        })
    }

    pub fn push(&mut self, item: ScheduledItem) {
        if let Err(error) = self.try_push(item) {
            crate::logging::warn(&format!("Failed to persist scheduled item: {error:#}"));
        }
    }

    pub fn remove_by_id(&mut self, id: &str) -> Result<Option<ScheduledItem>> {
        self.transact(|items| {
            Ok(items
                .iter()
                .position(|item| item.id == id)
                .map(|index| items.remove(index)))
        })
    }

    fn take_ready(&mut self, direct_only: bool) -> Result<Vec<ScheduledItem>> {
        self.transact(|items| {
            let now = Utc::now();
            let (mut ready, remaining): (Vec<_>, Vec<_>) =
                std::mem::take(items).into_iter().partition(|item| {
                    item.scheduled_for <= now && (!direct_only || item.target.is_direct_delivery())
                });
            *items = remaining;
            ready.sort_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.scheduled_for.cmp(&b.scheduled_for))
            });
            Ok(ready)
        })
    }

    pub fn pop_ready(&mut self) -> Vec<ScheduledItem> {
        self.take_ready(false).unwrap_or_else(|error| {
            crate::logging::warn(&format!("Failed to claim scheduled items: {error:#}"));
            Vec::new()
        })
    }

    pub fn take_ready_direct_items(&mut self) -> Vec<ScheduledItem> {
        self.take_ready(true).unwrap_or_else(|error| {
            crate::logging::warn(&format!(
                "Failed to claim direct scheduled items: {error:#}"
            ));
            Vec::new()
        })
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
