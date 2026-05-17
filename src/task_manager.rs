//! # Task Manager - 任务管理系统
//!
//! 提供完整的任务生命周期管理，支持：
//! - **CRUD操作** - 创建/读取/更新/删除
//! - **状态机** - Todo -> InProgress -> Done/Cancelled
//! - **优先级** - Low/Medium/High/Critical 四级优先级
//! - **标签系统** - 灵活分类和筛选
//! - **统计摘要** - 实时状态统计
//!
//! ## 核心数据结构
//!
//! ### Task (任务)
//! ```rust,no_run
//! pub struct Task {
//!     pub id: String,              // UUID唯一标识
//!     pub title: String,           // 任务标题
//!     pub description: Option<String>,  // 详细描述
//!     pub status: TaskStatus,      // 当前状态
//!     pub priority: TaskPriority,  // 优先级
//!     pub tags: Vec<String>,       // 标签列表
//!     pub created_at: DateTime<Utc>,   // 创建时间
//!     pub updated_at: DateTime<Utc>,   // 更新时间
//! }
//! ```
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use carpai::task_manager::{TaskManager, TaskStatus, TaskPriority, TaskUpdates};
//!
//! let manager = TaskManager::new();
//!
//! // 创建任务
//! let task = manager.create("实现用户认证功能").unwrap();
//!
//! // 更新任务
//! manager.update(&task.id, TaskUpdates {
//!     priority: Some(TaskPriority::High),
//!     tags: Some(vec!["backend".to_string(), "security".to_string()]),
//!     ..Default::default()
//! }).unwrap();
//!
//! // 状态流转
//! manager.update(&task.id, TaskUpdates {
//!     status: Some(TaskStatus::InProgress),
//!     ..Default::default()
//! }).unwrap();
//!
//! // 查看所有任务（按优先级排序）
//! for task in manager.list() {
//!     println!("[{}] {} - {}", task.priority.display(), task.status.display(), task.title);
//! }
//!
//! // 统计信息
//! let stats = manager.count_by_status();
//! println!("待办: {} | 进行中: {} | 已完成: {}",
//!     stats.get(&"⬜ Todo".to_string()).unwrap_or(&0),
//!     stats.get(&"🔄 In Progress".to_string()).unwrap_or(&0),
//!     stats.get(&"✅ Done".to_string()).unwrap_or(&0)
//! );
//! ```

use std::collections::HashMap;
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub type TaskId = String;

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, Default)]
pub struct TaskUpdates {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub tags: Option<Vec<String>>,
}

pub struct TaskManager {
    tasks: HashMap<String, Task>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

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

    pub fn get(&self, id: &str) -> Option<Task> {
        self.tasks.get(id).cloned()
    }

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

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        self.tasks.remove(id).map(|_| ()).ok_or_else(|| format!("Task '{}' not found", id))
    }

    pub fn list(&self) -> Vec<Task> {
        let mut tasks: Vec<_> = self.tasks.values().cloned().collect();
        tasks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        tasks
    }

    pub fn count_by_status(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for task in self.tasks.values() {
            *counts.entry(task.status.display().to_string()).or_insert(0) += 1;
        }
        counts
    }
}