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

    /// Add a schedule request to the queue. Returns the item ID.
    ///
    /// Recurrence is direct-target only: ambient cycles re-read queued items
    /// every run, so a repeating ambient-target item would double-fire.
    pub fn schedule(&mut self, request: ScheduleRequest) -> Result<String> {
        if let Some(repeat) = request.repeat.as_ref() {
            if repeat.every_minutes < 1 {
                anyhow::bail!("repeat.every_minutes must be >= 1");
            }
            if repeat.max_iterations == Some(0) {
                anyhow::bail!("repeat.max_iterations must be >= 1; omit it to repeat forever");
            }
            if !request.target.is_direct_delivery() {
                anyhow::bail!(
                    "repeat requires a direct target (session or spawn); ambient cycles already re-read queued items"
                );
            }
        }
        let id = format!("sched_{:08x}", rand::random::<u32>());
        let scheduled_for = request.wake_at.unwrap_or_else(|| {
            Utc::now() + chrono::Duration::minutes(request.wake_in_minutes.unwrap_or(30) as i64)
        });

        let repeat = request.repeat.map(|repeat| super::RepeatState {
            every_minutes: repeat.every_minutes,
            remaining: repeat.max_iterations,
            recurrence_id: format!("recur_{:08x}", rand::random::<u32>()),
            skipped: 0,
        });
        let item = ScheduledItem {
            id: id.clone(),
            scheduled_for,
            repeat,
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

        self.queue.push(item)?;
        Ok(id)
    }

    /// Cancel a queued scheduled item by ID.
    pub fn cancel_schedule(&mut self, id: &str) -> Result<Option<ScheduledItem>> {
        self.queue.remove_by_id(id)
    }

    /// Cancel every queued item of one recurrence series by its `recur_*`
    /// ID. To cancel a single occurrence, pass its item ID to
    /// `cancel_schedule` instead.
    pub fn cancel_recurrence(&mut self, recurrence_id: &str) -> Result<usize> {
        self.queue.remove_by_recurrence(recurrence_id)
    }

    pub fn state(&self) -> &AmbientState {
        &self.state
    }

    pub fn queue(&self) -> &ScheduledQueue {
        &self.queue
    }
}
