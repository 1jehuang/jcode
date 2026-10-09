use anyhow::Result;
use chrono::Utc;
use std::path::PathBuf;

use super::paths::{lock_path, state_path};
use super::{AmbientCycleResult, AmbientState, AmbientStatus, CycleStatus, ScheduledItem};
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

    /// Pop items whose `scheduled_for` is in the past, sorted by priority
    /// (highest first) then by time (earliest first).
    pub fn pop_ready(&mut self) -> Vec<ScheduledItem> {
        let now = Utc::now();
        let (ready, remaining): (Vec<_>, Vec<_>) =
            self.items.drain(..).partition(|i| i.scheduled_for <= now);

        let mut ready = ready;

        self.items = remaining;

        // Structural self-rearm (fork, 2026-09-30): recurring items get a clone
        // pushed back with scheduled_for = now + N minutes and the same id, so
        // the guardian survives even if the cycle is killed mid-run. The clone
        // is scheduled from NOW (not from scheduled_for) so a backlog of missed
        // intervals collapses to one pending copy instead of firing in a burst.
        // NOTE: must run AFTER `self.items = remaining` above, or the assignment
        // wipes the re-armed clones (caught by unit test 2026-09-30).
        for item in ready.iter() {
            if let Some(mins) = item.recurse_minutes {
                if mins > 0 {
                    let mut next = item.clone();
                    next.scheduled_for = now + chrono::Duration::minutes(mins);
                    self.items.push(next);
                }
            }
        }

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

    /// Remove and return ready items targeted at a specific direct-delivery session,
    /// leaving ambient-targeted queue items intact for the ambient agent to process.
    pub fn take_ready_direct_items(&mut self) -> Vec<ScheduledItem> {
        let now = Utc::now();
        let mut ready_direct = Vec::new();
        let mut remaining = Vec::with_capacity(self.items.len());

        for item in self.items.drain(..) {
            let is_ready = item.scheduled_for <= now;
            let is_direct_target = item.target.is_direct_delivery();
            if is_ready && is_direct_target {
                ready_direct.push(item.clone());
                // Structural self-rearm (fork, 2026-09-30): mirrors pop_ready.
                // Without this, a session-targeted recurring item (e.g. a
                // per-session guardian) is consumed silently on first fire -
                // live-caught 2026-09-30 with smoke item sched_d9eae4bf.
                if let Some(mins) = item.recurse_minutes {
                    if mins > 0 {
                        let mut next = item;
                        next.scheduled_for = now + chrono::Duration::minutes(mins);
                        remaining.push(next);
                    }
                }
            } else {
                remaining.push(item);
            }
        }

        self.items = remaining;

        if !ready_direct.is_empty() {
            let _ = self.save();
        }

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

#[cfg(test)]
mod fork_recurse_tests {
    use super::*;
    use crate::ambient::{Priority, ScheduleTarget};

    fn item(id: &str, due_minutes_ago: i64, recurse: Option<i64>) -> ScheduledItem {
        ScheduledItem {
            id: id.to_string(),
            scheduled_for: Utc::now() - chrono::Duration::minutes(due_minutes_ago),
            context: "guardian test context".to_string(),
            priority: Priority::Normal,
            target: ScheduleTarget::Ambient,
            created_by_session: "test-session".to_string(),
            created_at: Utc::now(),
            working_dir: None,
            task_description: None,
            relevant_files: Vec::new(),
            git_branch: None,
            additional_context: None,
            recurse_minutes: recurse,
        }
    }

    fn temp_queue() -> (ScheduledQueue, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let q = ScheduledQueue::load(dir.path().join("queue.json"));
        (q, dir)
    }

    #[test]
    fn recurring_item_is_rearmed_with_same_id() {
        let (mut q, _d) = temp_queue();
        q.push(item("g1", 5, Some(240)));

        let ready = q.pop_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "g1");

        // Re-arm copy must be pending with same id, due ~240min from now.
        let pending: Vec<_> = q.items.iter().filter(|i| i.id == "g1").collect();
        assert_eq!(pending.len(), 1);
        let delta = pending[0].scheduled_for - Utc::now();
        assert!(
            delta.num_minutes() >= 238 && delta.num_minutes() <= 240,
            "rearm should be ~240min out, got {delta}"
        );
        assert!(pending[0].scheduled_for > Utc::now());
    }

    #[test]
    fn one_shot_item_is_drained() {
        let (mut q, _d) = temp_queue();
        q.push(item("one-shot", 5, None));
        let ready = q.pop_ready();
        assert_eq!(ready.len(), 1);
        assert!(q.items.is_empty(), "one-shot must not rearm");
    }

    #[test]
    fn missed_intervals_collapse_to_one_pending_copy() {
        // Item due 3 recursions ago (3 x 10min = 30min late) fires ONCE and
        // leaves exactly ONE pending copy scheduled from now, not a burst.
        let (mut q, _d) = temp_queue();
        q.push(item("backlog", 30, Some(10)));
        let ready = q.pop_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(
            q.items.len(),
            1,
            "exactly one pending copy, no backlog burst"
        );
        assert!(q.items[0].scheduled_for > Utc::now());
    }

    #[test]
    fn recurse_zero_or_negative_is_one_shot() {
        let (mut q, _d) = temp_queue();
        q.push(item("zero", 5, Some(0)));
        q.push(item("neg", 5, Some(-3)));
        let ready = q.pop_ready();
        assert_eq!(ready.len(), 2);
        assert!(q.items.is_empty(), "non-positive recurse must not rearm");
    }

    #[test]
    fn future_item_untouched_and_not_duplicated() {
        let (mut q, _d) = temp_queue();
        q.push(item("future", -60, Some(240)));
        let ready = q.pop_ready();
        assert!(ready.is_empty());
        assert_eq!(q.items.len(), 1, "future item must not be cloned");
    }

    #[test]
    fn direct_delivery_recurring_item_is_rearmed() {
        // Session-targeted (direct delivery) recurring items must re-arm too,
        // else they die after one fire (live-caught 2026-09-30, smoke item
        // sched_d9eae4bf targeted a session and was silently consumed).
        let (mut q, _d) = temp_queue();
        let mut it = item("direct-g", 5, Some(2));
        it.target = ScheduleTarget::Session {
            session_id: "sess-x".to_string(),
        };
        q.push(it);

        let ready = q.take_ready_direct_items();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "direct-g");

        let pending: Vec<_> = q.items.iter().filter(|i| i.id == "direct-g").collect();
        assert_eq!(pending.len(), 1, "direct recurring item must re-arm once");
        let delta = pending[0].scheduled_for - Utc::now();
        assert!(delta.num_minutes() >= 1 && delta.num_minutes() <= 2);

        // Second pop delivers the re-armed copy; one-shot counterpart drains.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut it2 = item("direct-oneshot", 0, None);
        it2.target = ScheduleTarget::Session {
            session_id: "sess-x".to_string(),
        };
        // ensure it's due
        it2.scheduled_for = Utc::now();
        q.push(it2);
        let ready2 = q.take_ready_direct_items();
        assert_eq!(ready2.len(), 1, "only the one-shot should be due now");
        assert_eq!(ready2[0].id, "direct-oneshot");
    }
}
