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

    /// Remove and return only ready items targeted at the ambient agent.
    pub fn take_ready_ambient_items(&mut self) -> Vec<ScheduledItem> {
        self.queue.take_ready_ambient_items()
    }

    /// Return ready ambient-targeted items without removing them. The runner
    /// removes them only after a cycle completes, so reloads/crashes do not
    /// silently drop scheduled ambient work.
    pub fn ready_ambient_items(&self) -> Vec<ScheduledItem> {
        self.queue.ready_ambient_items()
    }

    pub fn remove_items_by_id(&mut self, ids: &std::collections::HashSet<String>) -> usize {
        self.queue.remove_by_ids(ids)
    }

    /// True when at least one ambient-targeted item is due now.
    pub fn has_ready_ambient_items(&self) -> bool {
        let now = Utc::now();
        self.queue
            .items()
            .iter()
            .any(|item| item.scheduled_for <= now && !item.target.is_direct_delivery())
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
    pub fn cancel_schedule(&mut self, id: &str) -> Result<Option<ScheduledItem>> {
        self.queue.remove_by_id(id)
    }

    /// Requeue a scheduled item that was popped as ready but could not be
    /// delivered. This keeps transient delivery failures from permanently
    /// dropping scheduled session wakeups.
    pub fn requeue_after(&mut self, item: ScheduledItem, delay: chrono::Duration) -> Result<()> {
        self.queue.requeue_after(item, delay);
        Ok(())
    }

    /// Cancel a queued scheduled item by ID.
    pub fn cancel_schedule(&mut self, id: &str) -> Result<Option<ScheduledItem>> {
        self.queue.remove_by_id(id)
    }

    pub fn state(&self) -> &AmbientState {
        &self.state
    }

    pub fn queue(&self) -> &ScheduledQueue {
        &self.queue
    }
}
