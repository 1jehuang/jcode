//! # Task Manager - 任务管理系统
//!
//! 提供完整的任务生命周期管理，支持：
//! - **CRUD操作** - 创建/读取/更新/删除
//! - **状态机** - Todo → InProgress → Done/Cancelled
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
use std::sync::{Arc, Mutex};
use chrono::{DateTime, Utc};
use uuid::Uuid;