use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub struct TaskManager {
    tasks: Arc<Mutex<HashMap<String, Task>>>,
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Todo,
    InProgress,
    Done,
    Cancelled,
}

impl TaskStatus {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "in-progress" | "doing" => TaskStatus::InProgress,
            "done" | "completed" => TaskStatus::Done,
            "cancelled" | "canceled" => TaskStatus::Cancelled,
            _ => TaskStatus::Todo,
        }
    }

    pub fn display(&self) -> &'static str {
        match self {
            TaskStatus::Todo => "⬜ Todo",
            TaskStatus::InProgress => "🔄 In Progress",
            TaskStatus::Done => "✅ Done",
            TaskStatus::Cancelled => "❌ Cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl TaskPriority {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "critical" | "urgent" => TaskPriority::Critical,
            "high" => TaskPriority::High,
            "low" => TaskPriority::Low,
            _ => TaskPriority::Medium,
        }
    }

    pub fn display(&self) -> &'static str {
        match self {
            TaskPriority::Critical => "🔴 Critical",
            TaskPriority::High => "🟠 High",
            TaskPriority::Medium => "🟡 Medium",
            TaskPriority::Low => "🟢 Low",
        }
    }
}

impl TaskManager {
    pub fn new() -> Self {
        TaskManager {
            tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn create(&self, title: &str) -> Result<Task, String> {
        let task = Task {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            description: None,
            status: TaskStatus::Todo,
            priority: TaskPriority::Medium,
            tags: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let mut tasks = self.tasks.lock().map_err(|e| format!("Lock error: {}", e))?;
        let id = task.id.clone();
        tasks.insert(id.clone(), task);
        Ok(tasks.get(&id).unwrap().clone())
    }

    pub fn list(&self) -> Vec<Task> {
        let tasks = self.tasks.lock().unwrap();
        let mut task_list: Vec<Task> = tasks.values().cloned().collect();
        task_list.sort_by(|a, b| {
            b.priority.cmp(&a.priority)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        task_list
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        let tasks = self.tasks.lock().ok()?;
        tasks.get(id).cloned()
    }

    pub fn update(&self, id: &str, updates: TaskUpdates) -> Result<Task, String> {
        let mut tasks = self.tasks.lock().map_err(|e| format!("Lock error: {}", e))?;
        let task = tasks.get_mut(id).ok_or_else(|| format!("Task '{}' not found", id))?;

        if let Some(title) = updates.title { task.title = title; }
        if let Some(desc) = updates.description { task.description = Some(desc); }
        if let Some(status) = updates.status { task.status = status; }
        if let Some(priority) = updates.priority { task.priority = priority; }
        if let Some(tags) = updates.tags { task.tags = tags; }
        task.updated_at = Utc::now();

        Ok(task.clone())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut tasks = self.tasks.lock().map_err(|e| format!("Lock error: {}", e))?;
        tasks.remove(id).ok_or_else(|| format!("Task '{}' not found", id)).map(drop)
    }

    pub fn count_by_status(&self) -> HashMap<String, usize> {
        let tasks = self.tasks.lock().unwrap();
        let mut counts = HashMap::new();
        for task in tasks.values() {
            *counts.entry(task.status.display().to_string()).or_insert(0) += 1;
        }
        counts
    }
}

pub struct TaskUpdates {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub tags: Option<Vec<String>>,
}
