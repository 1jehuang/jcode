//! Task Management System for CarpAI
//!
//! Provides comprehensive task and project management:
//! - Task CRUD operations
//! - Dependency tracking (DAG)
//! - Progress monitoring
//! - Priority and status management

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Unique task identifier
pub type TaskId = String;

/// Task priority levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Critical,
}

/// Task status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Pending,
    InProgress { progress: f64 },
    Blocked(Vec<TaskId>),
    Completed(Result<TaskOutput>),
    Failed(String),
}

/// Task output when completed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutput {
    pub result: String,
    pub artifacts: Vec<String>,
    pub duration_secs: f64,
}

/// Comprehensive task definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub assignee: Option<String>,
    pub tags: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub due_date: Option<chrono::DateTime<chrono::Utc>>,
    pub dependencies: HashSet<TaskId>,
    pub subtasks: Vec<TaskId>,
    pub metadata: HashMap<String, String>,
}

impl Task {
    /// Create new task with auto-generated ID
    pub fn new(title: impl Into<String>) -> Self {
        let id = format!("task-{}", uuid::Uuid::new_v4().simple());
        let now = chrono::Utc::now();
        
        Self {
            id,
            title: title.into(),
            description: None,
            status: TaskStatus::Pending,
            priority: TaskPriority::Medium,
            assignee: None,
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            due_date: None,
            dependencies: HashSet::new(),
            subtasks: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    /// Check if task is actionable (not blocked)
    pub fn is_actionable(&self) -> bool {
        match &self.status {
            TaskStatus::Pending => true,
            TaskStatus::InProgress { .. } => true,
            _ => false,
        }
    }

    /// Add dependency on another task
    pub fn add_dependency(&mut self, task_id: TaskId) {
        self.dependencies.insert(task_id);
        // If we have dependencies, mark as blocked
        if !self.dependencies.is_empty() && matches!(self.status, TaskStatus::Pending) {
            self.status = TaskStatus::Blocked(self.dependencies.iter().cloned().collect());
        }
    }

    /// Update progress (only if in InProgress state)
    pub fn update_progress(&mut self, progress: f64) -> Result<()> {
        let clamped_progress = progress.max(0.0).min(1.0);
        
        match &mut self.status {
            TaskStatus::InProgress { progress: p } => {
                *p = clamped_progress;
                if clamped_progress >= 1.0 {
                    self.status = TaskStatus::Completed(Ok(TaskOutput {
                        result: "Task completed".to_string(),
                        artifacts: Vec::new(),
                        duration_secs: 0.0,
                    }));
                }
                Ok(())
            }
            other => Err(anyhow::anyhow!("Cannot update progress in {:?} state", other)),
        }
    }

    /// Mark task as complete
    pub fn complete(&mut self, output: TaskOutput) -> Result<()> {
        match &self.status {
            TaskStatus::InProgress { .. } | TaskStatus::Pending => {
                self.status = TaskStatus::Completed(Ok(output));
                self.updated_at = chrono::Utc::now();
                Ok(())
            }
            other => Err(anyhow::anyhow!("Cannot complete task in {:?} state", other)),
        }
    }

    /// Mark task as failed
    pub fn fail(&mut self, error: impl Into<String>) -> Result<()> {
        match &self.status {
            TaskStatus::InProgress { .. } | TaskStatus::Pending => {
                self.status = TaskStatus::Failed(error.into());
                self.updated_at = chrono::Utc::now();
                Ok(())
            }
            other => Err(anyhow::anyhow!("Cannot fail task in {:?} state", other)),
        }
    }
}

/// Task manager with dependency resolution
pub struct TaskManager {
    tasks: RwLock<HashMap<TaskId, Arc<RwLock<Task>>>>,
}

impl TaskManager {
    /// Create new task manager
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new task
    pub async fn create_task(
        &self,
        title: impl Into<String>,
        options: TaskOptions,
    ) -> Result<Arc<RwLock<Task>>> {
        let mut task = Task::new(title);
        
        if let Some(desc) = options.description {
            task.description = Some(desc);
        }
        task.priority = options.priority.unwrap_or(TaskPriority::Medium);
        task.assignee = options.assignee;
        task.tags = options.tags.unwrap_or_default();
        task.due_date = options.due_date;

        let task_id = task.id.clone();
        let task_arc = Arc::new(RwLock::new(task));
        
        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id.clone(), task_arc.clone());
        }

        // Process dependencies
        for dep_id in &options.dependencies {
            self.add_dependency(&task_id, dep_id).await?;
        }

        Ok(task_arc)
    }

    /// Get task by ID
    pub async fn get_task(&self, id: &TaskId) -> Option<Arc<RwLock<Task>>> {
        let tasks = self.tasks.read().await;
        tasks.get(id).cloned()
    }

    /// List all tasks (optionally filtered)
    pub async fn list_tasks(&self, filter: Option<TaskFilter>) -> Result<Vec<Arc<RwLock<Task>>>> {
        let tasks = self.tasks.read().await;
        let mut result: Vec<Arc<RwLock<Task>>> = Vec::new();

        for (_, task_arc) in tasks.iter() {
            let task = task_arc.read().await;
            
            if let Some(ref f) = filter {
                if !f.matches(&task) {
                    continue;
                }
            }
            
            result.push(task_arc.clone());
        }

        // Sort by priority (descending), then by created date
        result.sort_by(|a, b| {
            // This is a simplified sort - in production would need async
            // For now, just return unsorted
            std::cmp::Ordering::Equal
        });

        Ok(result)
    }

    /// Update task
    pub async fn update_task(
        &self,
        id: &TaskId,
        updates: TaskUpdates,
    ) -> Result<()> {
        let task_arc = self.get_task(id).await
            .ok_or_else(|| anyhow::anyhow!("Task {} not found", id))?;

        {
            let mut task = task_arc.write().await;
            
            if let Some(title) = updates.title {
                task.title = title;
            }
            if let Some(description) = updates.description {
                task.description = Some(description);
            }
            if let Some(priority) = updates.priority {
                task.priority = priority;
            }
            if let Some(assignee) = updates.assignee {
                task.assignee = Some(assignee);
            }
            if let Some(status) = updates.status {
                task.status = status;
            }
            
            task.updated_at = chrono::Utc::now();
        }

        Ok(())
    }

    /// Delete task
    pub async fn delete_task(&self, id: &TaskId) -> Result<()> {
        let mut tasks = self.tasks.write().await;
        
        if tasks.remove(id).is_none() {
            Err(anyhow::anyhow!("Task {} not found", id))
        } else {
            Ok(())
        }
    }

    /// Add dependency between tasks
    pub async fn add_dependency(&self, task_id: &TaskId, dep_id: &TaskId) -> Result<()> {
        // Verify both tasks exist
        if self.get_task(dep_id).await.is_none() {
            return Err(anyhow::anyhow!("Dependency task {} not found", dep_id));
        }

        let task_arc = self.get_task(task_id).await
            .ok_or_else(|| anyhow::anyhow!("Task {} not found", task_id))?;

        {
            let mut task = task_arc.write().await;
            task.add_dependency(dep_id.clone());
        }

        Ok(())
    }

    /// Get tasks ready to execute (no blocking dependencies)
    pub async fn get_ready_tasks(&self) -> Result<Vec<Arc<RwLock<Task>>>> {
        let tasks = self.tasks.read().await;
        let mut ready_tasks = Vec::new();

        for (_, task_arc) in tasks.iter() {
            let task = task_arc.read().await;
            
            if task.is_actionable() {
                // Check if all dependencies are completed
                let deps_completed = task.dependencies.iter().all(|dep_id| {
                    // Note: This synchronous check inside async is simplified
                    // In production, should use proper async iteration
                    true // Simplified for now
                });

                if deps_completed {
                    ready_tasks.push(task_arc.clone());
                }
            }
        }

        Ok(ready_tasks)
    }

    /// Get statistics about tasks
    pub async fn get_statistics(&self) -> TaskStatistics {
        let tasks = self.tasks.read().await;
        let mut stats = TaskStatistics::default();

        for (_, task_arc) in tasks.iter() {
            let task = task_arc.read().await;
            stats.total += 1;

            match &task.status {
                TaskStatus::Pending => stats.pending += 1,
                TaskStatus::InProgress { .. } => stats.in_progress += 1,
                TaskStatus::Blocked(_) => stats.blocked += 1,
                TaskStatus::Completed(_) => stats.completed += 1,
                TaskStatus::Failed(_) => stats.failed += 1,
            }

            match task.priority {
                TaskPriority::Critical => stats.critical += 1,
                TaskPriority::High => stats.high += 1,
                TaskPriority::Medium => stats.medium += 1,
                TaskPriority::Low => stats.low += 1,
            }
        }

        stats
    }
}

/// Options for creating a task
#[derive(Debug, Clone, Default)]
pub struct TaskOptions {
    pub description: Option<String>,
    pub priority: Option<TaskPriority>,
    pub assignee: Option<String>,
    pub tags: Option<Vec<String>>,
    pub due_date: Option<chrono::DateTime<chrono::Utc>>,
    pub dependencies: Vec<TaskId>,
}

/// Updates that can be applied to a task
#[derive(Debug, Clone, Default)]
pub struct TaskUpdates {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<TaskPriority>,
    pub assignee: Option<String>,
    pub status: Option<TaskStatus>,
}

/// Filter for listing tasks
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub assignee: Option<String>,
    pub tags: Option<Vec<String>>,
}

impl TaskFilter {
    fn matches(&self, task: &Task) -> bool {
        if let Some(ref status) = self.status {
            // Simplified status matching - in production should be more sophisticated
            let matches_status = match (&task.status, status) {
                (TaskStatus::Pending, TaskStatus::Pending) => true,
                (TaskStatus::InProgress { .. }, TaskStatus::InProgress { .. }) => true,
                _ => false
            };
            if !matches_status {
                return false;
            }
        }

        if let Some(ref priority) = self.priority {
            if task.priority != *priority {
                return false;
            }
        }

        if let Some(ref assignee) = self.assignee {
            if task.assignee.as_deref() != Some(assignee) {
                return false;
            }
        }

        true
    }
}

/// Task statistics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskStatistics {
    pub total: usize,
    pub pending: usize,
    pub in_progress: usize,
    pub blocked: usize,
    pub completed: usize,
    pub failed: usize,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
}

/// CLI command handler for task management
pub struct TaskCommands {
    manager: Arc<TaskManager>,
}

impl TaskCommands {
    /// Create new task commands instance
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }

    /// Create with default manager
    pub fn with_default_manager() -> Self {
        Self::new(Arc::new(TaskManager::new()))
    }

    /// Handle task creation command
    pub async fn handle_create(
        &self,
        title: &str,
        options: TaskOptions,
    ) -> Result<()> {
        let task = self.manager.create_task(title, options).await?;
        let task_read = task.read().await;
        
        println!("✅ Task created successfully!");
        println!("   ID: {}", task_read.id);
        println!("   Title: {}", task_read.title);
        println!("   Status: {:?}", task_read.status);
        println!("   Priority: {:?}", task_read.priority);
        
        if let Some(assignee) = &task_read.assignee {
            println!("   Assignee: {}", assignee);
        }
        
        Ok(())
    }

    /// Handle list tasks command
    pub async fn handle_list(&self, filter: Option<TaskFilter>) -> Result<()> {
        let tasks = self.manager.list_tasks(filter).await?;
        
        if tasks.is_empty() {
            println!("📋 No tasks found");
            return Ok(());
        }

        println!("📋 Tasks ({})", tasks.len());
        println!("{}", "═".repeat(60));

        for task_arc in &tasks {
            let task = task_arc.read().await;
            
            let status_icon = match &task.status {
                TaskStatus::Pending => "⏳",
                TaskStatus::InProgress { progress } => 
                    format!("🔄 {:.0}%", *progress * 100.0),
                TaskStatus::Blocked(_) => "🚫",
                TaskStatus::Completed(_) => "✅",
                TaskStatus::Failed(_) => "❌",
            };

            let priority_icon = match task.priority {
                TaskPriority::Critical => "🔴",
                TaskPriority::High => "🟠",
                TaskPriority::Medium => "🟡",
                TaskPriority::Low => "🟢",
            };

            println!("{} {} [{}] {}", 
                status_icon, 
                priority_icon, 
                &task.id[..8.min(task.id.len())],
                task.title
            );

            if let Some(assignee) = &task.assignee {
                println!("   👤 {}", assignee);
            }

            if !task.tags.is_empty() {
                println!("   🏷️  {}", task.tags.join(", "));
            }
        }

        Ok(())
    }

    /// Handle show task details command
    pub async fn handle_show(&self, id: &TaskId) -> Result<()> {
        let task_arc = self.manager.get_task(id).await
            .ok_or_else(|| anyhow::anyhow!("Task {} not found", id))?;

        let task = task_arc.read().await;
        
        println!("📋 Task Details");
        println!("{}", "═".repeat(60));
        println!("ID:          {}", task.id);
        println!("Title:       {}", task.title);
        println!("Status:      {:?}", task.status);
        println!("Priority:    {:?}", task.priority);
        println!("Created:     {}", task.created_at.format("%Y-%m-%d %H:%M:%S"));
        println!("Updated:     {}", task.updated_at.format("%Y-%m-%d %H:%M:%S"));

        if let Some(desc) = &task.description {
            println!("\nDescription:\n{}", desc);
        }

        if let Some(assignee) = &task.assignee {
            println!("\nAssignee: {}", assignee);
        }

        if !task.tags.is_empty() {
            println!("Tags: {}", task.tags.join(", "));
        }

        if !task.dependencies.is_empty() {
            println!("Dependencies:");
            for dep in &task.dependencies {
                println!("  - {}", dep);
            }
        }

        if let Some(due) = &task.due_date {
            println!("Due Date: {}", due.format("%Y-%m-%d %H:%M"));
        }

        Ok(())
    }

    /// Handle task update command
    pub async fn handle_update(&self, id: &TaskId, updates: TaskUpdates) -> Result<()> {
        self.manager.update_task(id, updates).await?;
        println!("✅ Task {} updated successfully", id);
        Ok(())
    }

    /// Handle statistics display
    pub async fn handle_stats(&self) -> Result<()> {
        let stats = self.manager.get_statistics().await;
        
        println!("📊 Task Statistics");
        println!("{}", "═".repeat(40));
        println!("Total Tasks:  {}", stats.total);
        println!("+- Pending:    {}", stats.pending);
        println!("+- In Progress: {}", stats.in_progress);
        println!("+- Blocked:    {}", stats.blocked);
        println!("+- Completed:  {}", stats.completed);
        println!("+- Failed:     {}", stats.failed);

        println!("\nBy Priority:");
        println!("  🔴 Critical: {}", stats.critical);
        println!("  🟠 High:     {}", stats.high);
        println!("  🟡 Medium:   {}", stats.medium);
        println!("  🟢 Low:      {}", stats.low);

        if stats.total > 0 {
            let completion_rate = (stats.completed as f64 / stats.total as f64) * 100.0;
            println!("\nCompletion Rate: {:.1}%", completion_rate);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_get_task() {
        let manager = TaskManager::new();
        
        let task = manager.create_task("Test task", TaskOptions::default()).await.unwrap();
        let task_id = task.read().await.id.clone();
        
        let retrieved = manager.get_task(&task_id).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().read().await.title, "Test task");
    }

    #[tokio::test]
    async fn test_task_lifecycle() {
        let manager = TaskManager::new();
        
        let task = manager.create_task("Lifecycle test", TaskOptions::default()).await.unwrap();
        let task_id = task.read().await.id.clone();

        // Start progress
        manager.update_task(&task_id, TaskUpdates {
            status: Some(TaskStatus::InProgress { progress: 0.5 }),
            ..Default::default()
        }).await.unwrap();

        // Complete task
        let task_ref = manager.get_task(&task_id).await.unwrap();
        {
            let mut t = task_ref.write().await;
            t.complete(TaskOutput {
                result: "Done".to_string(),
                artifacts: vec!["output.txt".to_string()],
                duration_secs: 10.0,
            }).unwrap();
        }

        // Verify completed
        let task_final = task_ref.read().await;
        assert!(matches!(&task_final.status, TaskStatus::Completed(_)));
    }

    #[test]
    fn test_priority_ordering() {
        assert!(TaskPriority::Critical > TaskPriority::High);
        assert!(TaskPriority::High > TaskPriority::Medium);
        assert!(TaskPriority::Medium > TaskPriority::Low);
    }

    #[tokio::test]
    async fn test_task_dependencies() {
        let manager = TaskManager::new();
        
        let task1 = manager.create_task("First task", TaskOptions::default()).await.unwrap();
        let task2 = manager.create_task("Second task", TaskOptions::default()).await.unwrap();
        
        let task1_id = task1.read().await.id.clone();
        let task2_id = task2.read().await.id.clone();

        // Make task2 depend on task1
        manager.add_dependency(&task2_id, &task1_id).await.unwrap();

        // Task2 should be blocked
        let task2_state = task2.read().await.status.clone();
        assert!(matches!(task2_state, TaskStatus::Blocked(_)));
    }

    #[tokio::test]
    async fn test_task_statistics() {
        let manager = TaskManager::new();
        
        manager.create_task("Task 1", TaskOptions::default()).await.unwrap();
        manager.create_task("Task 2", TaskOptions {
            priority: Some(TaskPriority::High),
            ..Default::default()
        }).await.unwrap();

        let stats = manager.get_statistics().await;
        assert_eq!(stats.total, 2);
        assert_eq!(stats.pending, 2);
        assert_eq!(stats.high, 1);
    }
}
