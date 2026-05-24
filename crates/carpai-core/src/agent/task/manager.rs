//! Task Manager - Complete task lifecycle management
//!
//! Provides:
//! - CRUD operations for tasks
//! - State machine: Todo -> InProgress -> Done/Cancelled
//! - Priority levels: Low/Medium/High/Critical
//! - Tag system for flexible categorization
//! - Real-time statistics and summaries

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub type TaskId = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl TaskPriority {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "critical" => Self::Critical,
            _ => Self::Medium,
        }
    }

    pub fn display(&self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskStatus {
    Todo,
    InProgress,
    Done,
    Cancelled,
}

impl TaskStatus {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "todo" => Self::Todo,
            "inprogress" | "in_progress" => Self::InProgress,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            _ => Self::Todo,
        }
    }

    pub fn display(&self) -> &'static str {
        match self {
            Self::Todo => "Todo",
            Self::InProgress => "In Progress",
            Self::Done => "Done",
            Self::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskUpdates {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub tags: Option<Vec<String>>,
}

/// Task manager with full CRUD support
pub struct TaskManager {
    tasks: HashMap<String, Task>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Create a new task
    pub fn create(&mut self, title: &str) -> Result<Task, String> {
        let id = Uuid::new_v4().to_string();
        let task = Task {
            id: id.clone(),
            title: title.to_string(),
            description: None,
            status: TaskStatus::Todo,
            priority: TaskPriority::Medium,
            tags: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.tasks.insert(id, task.clone());
        Ok(task)
    }

    /// Get a task by ID
    pub fn get(&self, id: &str) -> Option<Task> {
        self.tasks.get(id).cloned()
    }

    /// Update task fields
    pub fn update(&mut self, id: &str, updates: TaskUpdates) -> Result<Task, String> {
        let task = self.tasks.get_mut(id).ok_or_else(|| format!("Task '{}' not found", id))?;
        
        if let Some(title) = updates.title {
            task.title = title;
        }
        if let Some(description) = updates.description {
            task.description = Some(description);
        }
        if let Some(status) = updates.status {
            task.status = status;
        }
        if let Some(priority) = updates.priority {
            task.priority = priority;
        }
        if let Some(tags) = updates.tags {
            task.tags = tags;
        }
        
        task.updated_at = Utc::now();
        Ok(task.clone())
    }

    /// Delete a task
    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        self.tasks.remove(id).map(|_| ()).ok_or_else(|| format!("Task '{}' not found", id))
    }

    /// List all tasks sorted by update time (most recent first)
    pub fn list(&self) -> Vec<Task> {
        let mut tasks: Vec<_> = self.tasks.values().cloned().collect();
        tasks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        tasks
    }

    /// Count tasks by status
    pub fn count_by_status(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for task in self.tasks.values() {
            *counts.entry(task.status.display().to_string()).or_insert(0) += 1;
        }
        counts
    }

    /// Filter tasks by tag
    pub fn filter_by_tag(&self, tag: &str) -> Vec<Task> {
        self.tasks.values()
            .filter(|task| task.tags.contains(&tag.to_string()))
            .cloned()
            .collect()
    }

    /// Get task statistics summary
    pub fn get_summary(&self) -> TaskSummary {
        let total = self.tasks.len();
        let by_status = self.count_by_status();
        
        let todo = by_status.get("Todo").copied().unwrap_or(0);
        let in_progress = by_status.get("In Progress").copied().unwrap_or(0);
        let done = by_status.get("Done").copied().unwrap_or(0);
        let cancelled = by_status.get("Cancelled").copied().unwrap_or(0);

        TaskSummary {
            total,
            todo,
            in_progress,
            done,
            cancelled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub total: usize,
    pub todo: usize,
    pub in_progress: usize,
    pub done: usize,
    pub cancelled: usize,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get_task() {
        let mut manager = TaskManager::new();
        let task = manager.create("Test task").unwrap();
        
        let retrieved = manager.get(&task.id).unwrap();
        assert_eq!(retrieved.title, "Test task");
        assert_eq!(retrieved.status, TaskStatus::Todo);
    }

    #[test]
    fn test_update_task() {
        let mut manager = TaskManager::new();
        let task = manager.create("Original title").unwrap();
        
        manager.update(&task.id, TaskUpdates {
            title: Some("Updated title".to_string()),
            status: Some(TaskStatus::InProgress),
            priority: Some(TaskPriority::High),
            ..Default::default()
        }).unwrap();
        
        let updated = manager.get(&task.id).unwrap();
        assert_eq!(updated.title, "Updated title");
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.priority, TaskPriority::High);
    }

    #[test]
    fn test_delete_task() {
        let mut manager = TaskManager::new();
        let task = manager.create("To delete").unwrap();
        
        manager.delete(&task.id).unwrap();
        assert!(manager.get(&task.id).is_none());
    }

    #[test]
    fn test_count_by_status() {
        let mut manager = TaskManager::new();
        manager.create("Task 1").unwrap();
        manager.create("Task 2").unwrap();
        
        let counts = manager.count_by_status();
        assert_eq!(*counts.get("Todo").unwrap(), 2);
    }

    #[test]
    fn test_filter_by_tag() {
        let mut manager = TaskManager::new();
        let task1 = manager.create("Backend task").unwrap();
        manager.update(&task1.id, TaskUpdates {
            tags: Some(vec!["backend".to_string()]),
            ..Default::default()
        }).unwrap();
        
        let task2 = manager.create("Frontend task").unwrap();
        manager.update(&task2.id, TaskUpdates {
            tags: Some(vec!["frontend".to_string()]),
            ..Default::default()
        }).unwrap();
        
        let backend_tasks = manager.filter_by_tag("backend");
        assert_eq!(backend_tasks.len(), 1);
    }

    #[test]
    fn test_get_summary() {
        let mut manager = TaskManager::new();
        manager.create("Task 1").unwrap();
        manager.create("Task 2").unwrap();
        
        let summary = manager.get_summary();
        assert_eq!(summary.total, 2);
        assert_eq!(summary.todo, 2);
        assert_eq!(summary.in_progress, 0);
    }
}
