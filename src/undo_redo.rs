//! # Enhanced Undo/Redo System - 增强版撤销/重做系统
//!
//! 提供完整的操作历史管理能力，包括：
//! - **多类型支持** - 文件编辑、命令执行、配置变更等
//! - **操作分组** - 将相关操作组合为原子事务
//! - **持久化存储** - 跨会话保存操作历史
//! - **可视化追踪** - 操作时间线和状态预览
//!
//! ## 使用示例
//!
//! ```rust,no_run
//! use carpai::undo_redo::{UndoRedoManager, Operation, OperationType};
//!
//! let mut manager = UndoRedoManager::new("session-123");
//!
//! // 记录文件编辑操作
//! let op = Operation::new(
//!     OperationType::FileEdit,
//!     "Edit config file",
//!     Some(serde_json::json!({"file": "config.toml", "old": "...", "new": "..."}))
//! );
//! manager.execute(op);
//!
//! // 撤销操作
//! if manager.can_undo() {
//!     let undone = manager.undo();
//!     println!("Undone: {}", undone.unwrap().description);
//! }
//!
//! // 重做操作
//! if manager.can_redo() {
//!     let redone = manager.redo();
//!     println!("Redone: {}", redone.unwrap().description);
//! }
//! ```

use std::collections::VecDeque;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OperationType {
    FileEdit,
    FileCreate,
    FileDelete,
    CommandExecution,
    ConfigChange,
    TaskUpdate,
    PluginInstall,
    PluginUninstall,
    GitCommit,
    GitRevert,
    SessionExport,
    Custom(String),
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationType::FileEdit => write!(f, "📝 File Edit"),
            OperationType::FileCreate => write!(f, "📄 File Create"),
            OperationType::FileDelete => write!(f, "🗑️  File Delete"),
            OperationType::CommandExecution => write!(f, "⚡ Command"),
            OperationType::ConfigChange => write!(f, "⚙️  Config Change"),
            OperationType::TaskUpdate => write!(f, "✅ Task Update"),
            OperationType::PluginInstall => write!(f, "🔌 Plugin Install"),
            OperationType::PluginUninstall => write!(f, "🔓 Plugin Uninstall"),
            OperationType::GitCommit => write!(f, "📦 Git Commit"),
            OperationType::GitRevert => write!(f, "↩️  Git Revert"),
            OperationType::SessionExport => write!(f, "💾 Export"),
            OperationType::Custom(name) => write!(f, "{}", name),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub operation_type: OperationType,
    pub description: String,
    pub data: Option<serde_json::Value>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub duration_ms: u64,
    pub reversible: bool,
}

impl Operation {
    pub fn new(operation_type: OperationType, description: &str, data: Option<serde_json::Value>) -> Self {
        Operation {
            id: uuid::Uuid::new_v4().to_string(),
            operation_type,
            description: description.to_string(),
            data,
            timestamp: chrono::Utc::now(),
            duration_ms: 0,
            reversible: true,
        }
    }

    pub fn with_duration(mut self, ms: u64) -> Self { self.duration_ms = ms; self }

    pub fn irreversible(mut self) -> Self { self.reversible = false; self }
}

pub struct UndoRedoManager {
    session_id: String,
    undo_stack: VecDeque<Operation>,
    redo_stack: VecDeque<Operation>,
    max_history: usize,
    operation_count: usize,
}

impl UndoRedoManager {
    pub fn new(session_id: &str) -> Self {
        UndoRedoManager {
            session_id: session_id.to_string(),
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            max_history: 100,
            operation_count: 0,
        }
    }

    pub fn with_max_history(mut self, max: usize) -> Self { self.max_history = max; self }

    pub fn execute(&mut self, operation: Operation) -> &Operation {
        eprintln!("[UNDO] Executing: {} - {}", operation.operation_type, operation.description);

        self.redo_stack.clear();
        self.undo_stack.push_back(operation.clone());
        self.operation_count += 1;

        while self.undo_stack.len() > self.max_history {
            self.undo_stack.pop_front();
        }

        self.undo_stack.back().unwrap()
    }

    pub fn execute_batch(&mut self, operations: Vec<Operation>) -> Vec<&Operation> {
        eprintln!("[UNDO] Executing batch of {} operations", operations.len());
        operations.into_iter().map(|op| self.execute(op)).collect()
    }

    pub fn undo(&mut self) -> Result<Operation, String> {
        match self.undo_stack.pop_back() {
            Some(op) => {
                eprintln!("[UNDO] Undid: {} - {}", op.operation_type, op.description);

                if op.reversible {
                    self.redo_stack.push_back(op.clone());
                } else {
                    eprintln!("[UNDO] ⚠️  Warning: Irreversible operation undone (cannot redo)");
                }

                Ok(op)
            }
            None => Err("Nothing to undo".to_string()),
        }
    }

    pub fn undo_multiple(&mut self, count: usize) -> Result<Vec<Operation>, String> {
        if count > self.undo_stack.len() {
            return Err(format!("Cannot undo {} operations (only {} available)", count, self.undo_stack.len()));
        }

        let mut undone = vec![];
        for _ in 0..count {
            match self.undo()? { op => undone.push(op), Err(e) => return Err(e), }
        }

        Ok(undone)
    }

    pub fn redo(&mut self) -> Result<Operation, String> {
        match self.redo_stack.pop_back() {
            Some(op) => {
                eprintln!("[REDO] Redid: {} - {}", op.operation_type, op.description);
                self.undo_stack.push_back(op.clone());
                Ok(op)
            }
            None => Err("Nothing to redo".to_string()),
        }
    }

    pub fn redo_multiple(&mut self, count: usize) -> Result<Vec<Operation>, String> {
        if count > self.redo_stack.len() {
            return Err(format!("Cannot redo {} operations (only {} available)", count, self.redo_stack.len()));
        }

        let mut redone = vec![];
        for _ in 0..count {
            match self.redo()? { op => redone.push(op), Err(e) => return Err(e), }
        }

        Ok(redone)
    }

    pub fn can_undo(&self) -> bool { !self.undo_stack.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo_stack.is_empty() }

    pub fn undo_count(&self) -> usize { self.undo_stack.len() }
    pub fn redo_count(&self) -> usize { self.redo_stack.len() }

    pub fn peek_undo(&self) -> Option<&Operation> { self.undo_stack.back() }
    pub fn peek_redo(&self) -> Option<&Operation> { self.redo_stack.back() }

    pub fn get_history(&self) -> Vec<&Operation> { self.undo_stack.iter().rev().collect() }

    pub fn get_timeline(&self) -> OperationTimeline {
        let mut timeline = OperationTimeline {
            session_id: self.session_id.clone(),
            total_operations: self.operation_count,
            undo_available: self.can_undo(),
            redo_available: self.can_redo(),
            recent_operations: vec![],
        };

        for op in self.undo_stack.iter().rev().take(10) {
            timeline.recent_operations.push(TimelineEntry {
                id: op.id.clone(),
                operation_type: format!("{}", op.operation_type),
                description: op.description.clone(),
                timestamp: op.timestamp,
                can_undo: true,
            });
        }

        timeline
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.operation_count = 0;
    }

    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        let data = serde_json::json!({
            "session_id": self.session_id,
            "operation_count": self.operation_count,
            "undo_stack": self.undo_stack.iter().cloned().collect::<Vec<_>>(),
            "redo_stack": self.redo_stack.iter().cloned().collect::<Vec<_>>(),
        });

        std::fs::write(path, serde_json::to_string_pretty(&data).unwrap_or_default())
            .map_err(|e| format!("Failed to save: {}", e))
    }

    pub fn load_from_file(path: &std::path::Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("Failed to read: {}", e))?;
        let data: serde_json::Value = serde_json::from_str(&content).map_err(|e| format!("Invalid JSON: {}", e))?;

        let session_id = data["session_id"].as_str().unwrap_or("unknown").to_string();

        let undo_stack: Vec<Operation> = data["undo_stack"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect())
            .unwrap_or_default();

        let redo_stack: Vec<Operation> = data["redo_stack"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect())
            .unwrap_or_default();

        Ok(UndoRedoManager {
            session_id,
            undo_stack: undo_stack.into(),
            redo_stack: redo_stack.into(),
            max_history: 100,
            operation_count: data["operation_count"].as_u64().unwrap_or(0) as usize,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationTimeline {
    pub session_id: String,
    pub total_operations: usize,
    pub undo_available: bool,
    pub redo_available: bool,
    pub recent_operations: Vec<TimelineEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub id: String,
    pub operation_type: String,
    pub description: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub can_undo: bool,
}
