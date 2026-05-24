//! Task Scheduler - Schedules and prioritizes tasks for execution
//!
//! Provides:
//! - Priority-based scheduling (Critical > High > Medium > Low)
//! - Dependency-aware execution ordering
//! - Concurrent task execution with controlled parallelism
//! - Progress tracking and status updates

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BinaryHeap, HashMap};
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SchedulerPriority {
    Critical = 0,
    High = 1,
    Medium = 2,
    Low = 3,
}

impl SchedulerPriority {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "critical" => Self::Critical,
            "high" => Self::High,
            "medium" => Self::Medium,
            _ => Self::Low,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub title: String,
    pub priority: SchedulerPriority,
    pub dependencies: Vec<String>,
    pub estimated_duration_secs: u64,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: TaskScheduleStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskScheduleStatus {
    Queued,
    Ready,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Priority queue entry for task scheduling
#[derive(Debug, Clone)]
struct PriorityEntry {
    task_id: String,
    priority: SchedulerPriority,
    sequence: u64, // For stable sorting
}

impl PartialEq for PriorityEntry {
    fn eq(&self, other: &Self) -> bool {
        self.task_id == other.task_id
    }
}

impl Eq for PriorityEntry {}

impl PartialOrd for PriorityEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriorityEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower priority value = higher priority (Critical = 0 is highest)
        other.priority.cmp(&self.priority)
            .then_with(|| self.sequence.cmp(&other.sequence))
    }
}

/// Task scheduler with priority queue and dependency management
pub struct TaskScheduler {
    tasks: HashMap<String, ScheduledTask>,
    ready_queue: BinaryHeap<PriorityEntry>,
    running_tasks: HashMap<String, DateTime<Utc>>,
    sequence_counter: u64,
    max_concurrent: usize,
}

impl TaskScheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            tasks: HashMap::new(),
            ready_queue: BinaryHeap::new(),
            running_tasks: HashMap::new(),
            sequence_counter: 0,
            max_concurrent,
        }
    }

    /// Add a task to the scheduler
    pub fn add_task(&mut self, task: ScheduledTask) {
        let id = task.id.clone();
        self.tasks.insert(id.clone(), task);
        
        // If no dependencies, mark as ready immediately
        if self.can_run_now(&id) {
            self.mark_ready(&id);
        }
    }

    /// Check if a task can run (all dependencies completed)
    fn can_run_now(&self, task_id: &str) -> bool {
        if let Some(task) = self.tasks.get(task_id) {
            task.dependencies.iter().all(|dep_id| {
                self.tasks.get(dep_id)
                    .map(|dep| dep.status == TaskScheduleStatus::Completed)
                    .unwrap_or(false)
            })
        } else {
            false
        }
    }

    /// Mark a task as ready for execution
    fn mark_ready(&mut self, task_id: &str) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = TaskScheduleStatus::Ready;
            task.scheduled_at = Some(Utc::now());
            
            self.ready_queue.push(PriorityEntry {
                task_id: task_id.to_string(),
                priority: task.priority.clone(),
                sequence: self.sequence_counter,
            });
            self.sequence_counter += 1;
        }
    }

    /// Get the next task to execute (highest priority ready task)
    pub fn next_task(&mut self) -> Option<ScheduledTask> {
        // Check if we've reached max concurrent tasks
        if self.running_tasks.len() >= self.max_concurrent {
            return None;
        }

        while let Some(entry) = self.ready_queue.pop() {
            if let Some(task) = self.tasks.get(&entry.task_id) {
                if task.status == TaskScheduleStatus::Ready {
                    // Mark as running
                    let mut task_clone = task.clone();
                    task_clone.status = TaskScheduleStatus::Running;
                    task_clone.started_at = Some(Utc::now());
                    
                    self.tasks.insert(entry.task_id.clone(), task_clone.clone());
                    self.running_tasks.insert(entry.task_id, Utc::now());
                    
                    return Some(task_clone);
                }
            }
        }
        
        None
    }

    /// Mark a task as completed
    pub fn complete_task(&mut self, task_id: &str, success: bool) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = if success {
                TaskScheduleStatus::Completed
            } else {
                TaskScheduleStatus::Failed
            };
            task.completed_at = Some(Utc::now());
        }
        
        self.running_tasks.remove(task_id);
        
        // Check if any dependent tasks are now ready
        self.check_dependent_tasks(task_id);
    }

    /// Check and mark dependent tasks as ready
    fn check_dependent_tasks(&mut self, completed_task_id: &str) {
        let ready_tasks: Vec<String> = self.tasks.iter()
            .filter(|(_, task)| {
                task.status == TaskScheduleStatus::Queued &&
                task.dependencies.contains(&completed_task_id.to_string()) &&
                self.can_run_now(&task.id)
            })
            .map(|(id, _)| id.clone())
            .collect();

        for task_id in ready_tasks {
            self.mark_ready(&task_id);
        }
    }

    /// Cancel a task
    pub fn cancel_task(&mut self, task_id: &str) -> Result<(), String> {
        let task = self.tasks.get_mut(task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;
        
        if task.status == TaskScheduleStatus::Running {
            self.running_tasks.remove(task_id);
        }
        
        task.status = TaskScheduleStatus::Cancelled;
        task.completed_at = Some(Utc::now());
        
        Ok(())
    }

    /// Get task status
    pub fn get_task_status(&self, task_id: &str) -> Option<TaskScheduleStatus> {
        self.tasks.get(task_id).map(|t| t.status.clone())
    }

    /// Get all tasks
    pub fn get_all_tasks(&self) -> Vec<&ScheduledTask> {
        self.tasks.values().collect()
    }

    /// Get scheduler statistics
    pub fn get_stats(&self) -> SchedulerStats {
        let mut queued = 0;
        let mut ready = 0;
        let mut running = 0;
        let mut completed = 0;
        let mut failed = 0;
        let mut cancelled = 0;

        for task in self.tasks.values() {
            match task.status {
                TaskScheduleStatus::Queued => queued += 1,
                TaskScheduleStatus::Ready => ready += 1,
                TaskScheduleStatus::Running => running += 1,
                TaskScheduleStatus::Completed => completed += 1,
                TaskScheduleStatus::Failed => failed += 1,
                TaskScheduleStatus::Cancelled => cancelled += 1,
            }
        }

        SchedulerStats {
            total: self.tasks.len(),
            queued,
            ready,
            running,
            completed,
            failed,
            cancelled,
            max_concurrent: self.max_concurrent,
        }
    }

    /// Check if all tasks are completed
    pub fn is_done(&self) -> bool {
        self.tasks.values().all(|t| {
            matches!(
                t.status,
                TaskScheduleStatus::Completed |
                TaskScheduleStatus::Failed |
                TaskScheduleStatus::Cancelled
            )
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerStats {
    pub total: usize,
    pub queued: usize,
    pub ready: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub max_concurrent: usize,
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new(4) // Default to 4 concurrent tasks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(id: &str, priority: SchedulerPriority) -> ScheduledTask {
        ScheduledTask {
            id: id.to_string(),
            title: format!("Task {}", id),
            priority,
            dependencies: vec![],
            estimated_duration_secs: 60,
            created_at: Utc::now(),
            scheduled_at: None,
            started_at: None,
            completed_at: None,
            status: TaskScheduleStatus::Queued,
        }
    }

    #[test]
    fn test_priority_ordering() {
        let mut scheduler = TaskScheduler::new(10);
        
        scheduler.add_task(make_task("low", SchedulerPriority::Low));
        scheduler.add_task(make_task("critical", SchedulerPriority::Critical));
        scheduler.add_task(make_task("high", SchedulerPriority::High));
        
        // Should get critical first
        let task = scheduler.next_task().unwrap();
        assert_eq!(task.id, "critical");
        assert_eq!(task.priority, SchedulerPriority::Critical);
        
        // Then high
        let task = scheduler.next_task().unwrap();
        assert_eq!(task.id, "high");
        
        // Then low
        let task = scheduler.next_task().unwrap();
        assert_eq!(task.id, "low");
    }

    #[test]
    fn test_dependency_handling() {
        let mut scheduler = TaskScheduler::new(10);
        
        let mut task_a = make_task("A", SchedulerPriority::High);
        scheduler.add_task(task_a);
        
        let mut task_b = make_task("B", SchedulerPriority::High);
        task_b.dependencies.push("A".to_string());
        scheduler.add_task(task_b);
        
        // B should not be ready yet
        let next = scheduler.next_task();
        assert_eq!(next.unwrap().id, "A");
        
        // Complete A
        scheduler.complete_task("A", true);
        
        // Now B should be ready
        let next = scheduler.next_task();
        assert_eq!(next.unwrap().id, "B");
    }

    #[test]
    fn test_max_concurrent_limit() {
        let mut scheduler = TaskScheduler::new(2);
        
        scheduler.add_task(make_task("1", SchedulerPriority::High));
        scheduler.add_task(make_task("2", SchedulerPriority::High));
        scheduler.add_task(make_task("3", SchedulerPriority::High));
        
        // Should only get 2 tasks
        assert!(scheduler.next_task().is_some());
        assert!(scheduler.next_task().is_some());
        assert!(scheduler.next_task().is_none()); // Third task blocked
        
        // Complete one task
        scheduler.complete_task("1", true);
        
        // Now third task should be available
        assert!(scheduler.next_task().is_some());
    }

    #[test]
    fn test_scheduler_stats() {
        let mut scheduler = TaskScheduler::new(10);
        
        scheduler.add_task(make_task("1", SchedulerPriority::High));
        scheduler.add_task(make_task("2", SchedulerPriority::Medium));
        
        let stats = scheduler.get_stats();
        assert_eq!(stats.total, 2);
        assert_eq!(stats.queued, 2);
        
        scheduler.next_task();
        let stats = scheduler.get_stats();
        assert_eq!(stats.running, 1);
        assert_eq!(stats.queued, 1);
    }
}
