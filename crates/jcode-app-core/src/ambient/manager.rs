use anyhow::Result;
use chrono::Utc;

use super::paths::{ambient_dir, queue_path, transcripts_dir};
use super::{
    AmbientCycleResult, AmbientState, AmbientStatus, ScheduleRequest, ScheduledItem, ScheduledQueue,
};
use crate::config::config;

// ---------------------------------------------------------------------------
// AmbientManager
// ---------------------------------------------------------------------------

pub struct AmbientManager {
    state: AmbientState,
    queue: ScheduledQueue,
}

/// Result of [`AmbientManager::cancel_schedule`].
///
/// `NotOwned` is deliberately distinct from `NotFound`: telling a caller that a
/// schedule exists but belongs to another session is the honest answer, and it
/// is what lets the error name the owning session.
#[derive(Debug)]
pub enum CancelOutcome {
    Removed { item: ScheduledItem },
    NotOwned { created_by: String },
    NotFound,
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

    /// Remove and return all ready scheduled items.
    pub fn take_ready_items(&mut self) -> Vec<ScheduledItem> {
        self.queue.pop_ready()
    }

    /// Remove and return only ready items targeted at direct delivery into a
    /// specific resumed or spawned session.
    pub fn take_ready_direct_items(&mut self) -> Vec<ScheduledItem> {
        self.queue.take_ready_direct_items()
    }

    /// Add a schedule request to the queue. Returns the item ID.
    pub fn schedule(&mut self, request: ScheduleRequest) -> Result<String> {
        let id = format!("sched_{:08x}", rand::random::<u32>());
        let scheduled_for = request.wake_at.unwrap_or_else(|| {
            Utc::now() + chrono::Duration::minutes(request.wake_in_minutes.unwrap_or(30) as i64)
        });

        let item = ScheduledItem {
            id: id.clone(),
            scheduled_for,
            context: request.context,
            priority: request.priority,
            target: request.target,
            created_by_session: request.created_by_session,
            created_at: Utc::now(),
            working_dir: request.working_dir,
            task_description: request.task_description,
            relevant_files: request.relevant_files,
            git_branch: request.git_branch,
            additional_context: request.additional_context,
        };

        self.queue.push(item);
        Ok(id)
    }

    /// Cancel a queued scheduled item by ID.
    ///
    /// The caller is only allowed to remove items it owns, so the check lives
    /// here rather than in one caller: every path that mutates the shared queue
    /// needs it, and a check that lives in one caller is one refactor away from
    /// being gone. Ownership is per *creator* session, not per target session,
    /// because the queue is one file for the whole daemon and the creator is
    /// what ties an item to a project.
    pub fn cancel_schedule(&mut self, id: &str, created_by_session: &str) -> Result<CancelOutcome> {
        let Some(item) = self
            .queue
            .items()
            .iter()
            .find(|item| item.id == id)
            .cloned()
        else {
            return Ok(CancelOutcome::NotFound);
        };

        if item.created_by_session != created_by_session {
            return Ok(CancelOutcome::NotOwned {
                created_by: item.created_by_session,
            });
        }

        let removed = self.queue.remove_by_id(id)?;
        Ok(CancelOutcome::Removed {
            item: removed.unwrap(),
        })
    }

    /// Remove an item by ID with no ownership check.
    ///
    /// Only for a caller that has *already* established cross-session intent,
    /// such as `schedule cancel all_sessions=true`. Anything else must go
    /// through [`Self::cancel_schedule`], which refuses by default.
    pub fn force_cancel_schedule(&mut self, id: &str) -> Result<Option<ScheduledItem>> {
        self.queue.remove_by_id(id)
    }

    pub fn state(&self) -> &AmbientState {
        &self.state
    }

    pub fn queue(&self) -> &ScheduledQueue {
        &self.queue
    }
}
