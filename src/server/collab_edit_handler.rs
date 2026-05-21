//! # 协作编辑处理器
//!
//! 提供了增强的编辑处理功能，整合了 CRDT、OT 和传统编辑处理。
//! 支持批量操作、冲突检测、撤销/重做等高级功能。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::{Position, SelectionRange, VectorClock, TextOperation, OpType};

/// 编辑操作类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EditOperationType {
    /// 插入
    Insert,
    /// 删除
    Delete,
    /// 替换
    Replace,
    /// 多位置编辑
    MultiEdit,
}

/// 编辑批次
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditBatch {
    pub operations: Vec<EditOperation>,
    pub base_version: VectorClock,
    pub timestamp: i64,
}

/// 单个编辑操作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditOperation {
    pub op_type: EditOperationType,
    pub position: Position,
    pub content: String,
    pub old_range: Option<SelectionRange>,
    pub timestamp: i64,
}

/// 编辑结果
#[derive(Debug, Clone)]
pub struct EnhancedEditResult {
    pub success: bool,
    pub new_version: VectorClock,
    pub applied_operations: Vec<AppliedOperation>,
    pub conflicts: Vec<EditConflict>,
    pub undo_stack_size: usize,
    pub redo_stack_size: usize,
}

/// 已应用的操作记录
#[derive(Debug, Clone)]
pub struct AppliedOperation {
    pub op_id: u64,
    pub op_type: EditOperationType,
    pub position: Position,
    pub content: String,
    pub timestamp: i64,
}

/// 编辑冲突
#[derive(Debug, Clone)]
pub struct EditConflict {
    pub conflict_id: String,
    pub local_op: EditOperation,
    pub remote_op: EditOperation,
    pub conflict_type: ConflictType,
    pub resolution: Option<ConflictResolution>,
}

/// 冲突类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictType {
    /// 重叠插入
    OverlappingInsert,
    /// 重叠删除
    OverlappingDelete,
    /// 范围冲突
    RangeConflict,
    /// 版本冲突
    VersionConflict,
}

/// 冲突解决方案
#[derive(Debug, Clone)]
pub enum ConflictResolution {
    /// 使用本地操作
    UseLocal,
    /// 使用远程操作
    UseRemote,
    /// 合并
    Merge(MergedContent),
    /// 手动解决
    Manual,
}

/// 合并后的内容
#[derive(Debug, Clone)]
pub struct MergedContent {
    pub content: String,
    pub position: Position,
}

/// 撤销/重做状态
#[derive(Debug, Clone)]
pub struct UndoRedoState {
    pub undo_stack: VecDeque<UndoEntry>,
    pub redo_stack: VecDeque<UndoEntry>,
    pub max_stack_size: usize,
}

/// 撤销条目
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub operations: Vec<AppliedOperation>,
    pub version_before: VectorClock,
    pub version_after: VectorClock,
    pub timestamp: i64,
}

impl Default for UndoRedoState {
    fn default() -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            max_stack_size: 100,
        }
    }
}

impl UndoRedoState {
    /// 添加到撤销栈
    pub fn push_undo(&mut self, entry: UndoEntry) {
        if self.undo_stack.len() >= self.max_stack_size {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(entry);
        self.redo_stack.clear(); // 新编辑清除重做栈
    }

    /// 获取撤销条目
    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo_stack.pop_back()
    }

    /// 获取重做条目
    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo_stack.pop_back()
    }

    pub fn undo_stack_size(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_stack_size(&self) -> usize {
        self.redo_stack.len()
    }
}

/// 编辑处理器配置
#[derive(Debug, Clone)]
pub struct EditHandlerConfig {
    /// 是否启用冲突检测
    pub enable_conflict_detection: bool,
    /// 是否启用 CRDT 集成
    pub enable_crdt: bool,
    /// 是否启用 OT 集成
    pub enable_ot: bool,
    /// 最大批量大小
    pub max_batch_size: usize,
    /// 启用自动撤销
    pub enable_auto_undo: bool,
    /// 撤销栈大小
    pub undo_stack_size: usize,
}

impl Default for EditHandlerConfig {
    fn default() -> Self {
        Self {
            enable_conflict_detection: true,
            enable_crdt: false, // 默认关闭，需要显式启用
            enable_ot: true,
            max_batch_size: 100,
            enable_auto_undo: true,
            undo_stack_size: 100,
        }
    }
}

/// 增强的编辑处理器
pub struct EnhancedEditHandler {
    config: EditHandlerConfig,
    undo_redo: UndoRedoState,
    pending_edits: VecDeque<EditBatch>,
    last_applied_version: VectorClock,
    operation_history: VecDeque<AppliedOperation>,
    max_history_size: usize,
}

impl EnhancedEditHandler {
    pub fn new(config: EditHandlerConfig) -> Self {
        Self {
            config,
            undo_redo: UndoRedoState {
                max_stack_size: config.undo_stack_size,
                ..Default::default()
            },
            pending_edits: VecDeque::new(),
            last_applied_version: VectorClock::default(),
            operation_history: VecDeque::new(),
            max_history_size: 1000,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(EditHandlerConfig::default())
    }

    /// 处理单个编辑操作
    pub fn handle_edit(
        &mut self,
        operation: EditOperation,
        current_version: &VectorClock,
    ) -> EnhancedEditResult {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let mut applied_ops = Vec::new();
        let mut conflicts = Vec::new();

        // 1. 冲突检测
        if self.config.enable_conflict_detection {
            conflicts = self.detect_conflicts(&operation, current_version);
            if !conflicts.is_empty() {
                warn!("Detected {} conflicts for edit at {:?}", conflicts.len(), operation.position);
            }
        }

        // 2. 版本检查
        if !self.is_version_compatible(current_version) {
            return EnhancedEditResult {
                success: false,
                new_version: self.last_applied_version.clone(),
                applied_operations: Vec::new(),
                conflicts: vec![EditConflict {
                    conflict_id: format!("version_{}", timestamp),
                    local_op: operation.clone(),
                    remote_op: operation.clone(),
                    conflict_type: ConflictType::VersionConflict,
                    resolution: None,
                }],
                undo_stack_size: self.undo_redo.undo_stack_size(),
                redo_stack_size: self.undo_redo.redo_stack_size(),
            };
        }

        // 3. 应用编辑
        let op_id = self.operation_history.len() as u64 + 1;
        let applied = AppliedOperation {
            op_id,
            op_type: operation.op_type.clone(),
            position: operation.position.clone(),
            content: operation.content.clone(),
            timestamp,
        };
        applied_ops.push(applied.clone());

        // 4. 更新版本
        let mut new_version = current_version.clone();
        new_version.increment("local"); // 使用固定节点名，实际应该是参与者ID

        // 5. 记录历史
        self.operation_history.push_back(applied);
        if self.operation_history.len() > self.max_history_size {
            self.operation_history.pop_front();
        }

        // 6. 更新撤销/重做状态
        if self.config.enable_auto_undo {
            let entry = UndoEntry {
                operations: applied_ops.clone(),
                version_before: current_version.clone(),
                version_after: new_version.clone(),
                timestamp,
            };
            self.undo_redo.push_undo(entry);
        }

        self.last_applied_version = new_version.clone();

        EnhancedEditResult {
            success: true,
            new_version,
            applied_operations: applied_ops,
            conflicts,
            undo_stack_size: self.undo_redo.undo_stack_size(),
            redo_stack_size: self.undo_redo.redo_stack_size(),
        }
    }

    /// 处理批量编辑
    pub fn handle_batch(
        &mut self,
        batch: EditBatch,
        current_version: &VectorClock,
    ) -> EnhancedEditResult {
        let mut all_applied = Vec::new();
        let mut all_conflicts = Vec::new();
        let mut version = current_version.clone();

        for op in &batch.operations {
            let result = self.handle_edit(op.clone(), &version);
            if !result.success {
                // 批量操作在第一个失败时停止
                return EnhancedEditResult {
                    success: false,
                    new_version: version,
                    applied_operations: all_applied,
                    conflicts: all_conflicts,
                    undo_stack_size: self.undo_redo.undo_stack_size(),
                    redo_stack_size: self.undo_redo.redo_stack_size(),
                };
            }
            all_applied.extend(result.applied_operations);
            all_conflicts.extend(result.conflicts);
            version = result.new_version;
        }

        EnhancedEditResult {
            success: true,
            new_version: version,
            applied_operations: all_applied,
            conflicts: all_conflicts,
            undo_stack_size: self.undo_redo.undo_stack_size(),
            redo_stack_size: self.undo_redo.redo_stack_size(),
        }
    }

    /// 撤销操作
    pub fn undo(&mut self) -> Option<UndoEntry> {
        self.undo_redo.pop_undo().map(|entry| {
            // 将条目添加到重做栈
            if self.undo_redo.redo_stack.len() >= self.undo_redo.max_stack_size {
                self.undo_redo.redo_stack.pop_front();
            }
            self.undo_redo.redo_stack.push_back(entry.clone());
            entry
        })
    }

    /// 重做操作
    pub fn redo(&mut self) -> Option<UndoEntry> {
        self.undo_redo.pop_redo().map(|entry| {
            // 将条目添加到撤销栈
            if self.undo_redo.undo_stack.len() >= self.undo_redo.max_stack_size {
                self.undo_redo.undo_stack.pop_front();
            }
            self.undo_redo.undo_stack.push_back(entry.clone());
            entry
        })
    }

    /// 检测冲突
    fn detect_conflicts(&self, _operation: &EditOperation, _version: &VectorClock) -> Vec<EditConflict> {
        // 简化实现，实际应该检查与历史操作的冲突
        Vec::new()
    }

    /// 检查版本兼容性
    fn is_version_compatible(&self, version: &VectorClock) -> bool {
        // 如果 last_applied_version 为空，任何版本都兼容
        if self.last_applied_version.is_empty() {
            return true;
        }

        // 检查版本是否兼容（简单实现）
        true
    }

    /// 获取版本历史
    pub fn get_operation_history(&self, from_id: u64, limit: usize) -> Vec<AppliedOperation> {
        self.operation_history
            .iter()
            .skip(from_id as usize)
            .take(limit)
            .cloned()
            .collect()
    }

    /// 获取待处理编辑数
    pub fn pending_edits_count(&self) -> usize {
        self.pending_edits.len()
    }

    /// 获取配置
    pub fn get_config(&self) -> &EditHandlerConfig {
        &self.config
    }

    /// 更新配置
    pub fn update_config(&mut self, config: EditHandlerConfig) {
        self.config = config;
        self.undo_redo.max_stack_size = config.undo_stack_size;
    }
}

/// CRDT 集成的编辑处理器
pub struct CrdtEditHandler {
    inner: EnhancedEditHandler,
    crdt_enabled: bool,
}

impl CrdtEditHandler {
    pub fn new(config: EditHandlerConfig) -> Self {
        Self {
            inner: EnhancedEditHandler::new(config.clone()),
            crdt_enabled: config.enable_crdt,
        }
    }

    /// 处理编辑（CRDT 感知）
    pub fn handle_edit_crdt_aware(
        &mut self,
        operation: EditOperation,
        current_version: &VectorClock,
    ) -> EnhancedEditResult {
        if self.crdt_enabled {
            // CRDT 模式下：直接应用，允许并发编辑
            info!("Processing edit in CRDT mode");
            self.inner.handle_edit(operation, current_version)
        } else {
            // OT 模式下：需要版本检查和转换
            info!("Processing edit in OT mode");
            self.inner.handle_edit(operation, current_version)
        }
    }
}

/// 编辑处理器管理器
pub struct EditHandlerManager {
    handlers: HashMap<String, EnhancedEditHandler>,
    default_handler: EnhancedEditHandler,
}

impl EditHandlerManager {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            default_handler: EnhancedEditHandler::with_defaults(),
        }
    }

    /// 获取指定会话的处理器
    pub fn get_handler(&mut self, session_id: &str) -> &mut EnhancedEditHandler {
        self.handlers
            .entry(session_id.to_string())
            .or_insert_with(|| EnhancedEditHandler::with_defaults())
    }

    /// 移除指定会话的处理器
    pub fn remove_handler(&mut self, session_id: &str) -> bool {
        self.handlers.remove(session_id).is_some()
    }

    /// 获取默认处理器
    pub fn get_default_handler(&self) -> &EnhancedEditHandler {
        &self.default_handler
    }

    /// 获取所有活跃会话数
    pub fn active_sessions(&self) -> usize {
        self.handlers.len()
    }
}

impl Default for EditHandlerManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_edit() {
        let mut handler = EnhancedEditHandler::with_defaults();
        let version = VectorClock::default();

        let operation = EditOperation {
            op_type: EditOperationType::Insert,
            position: Position::new(0, 0),
            content: "Hello".to_string(),
            old_range: None,
            timestamp: 0,
        };

        let result = handler.handle_edit(operation, &version);

        assert!(result.success);
        assert_eq!(result.applied_operations.len(), 1);
        assert!(result.conflicts.is_empty());
    }

    #[test]
    fn test_undo_redo_state() {
        let mut state = UndoRedoState::default();

        let entry = UndoEntry {
            operations: vec![],
            version_before: VectorClock::default(),
            version_after: VectorClock::default(),
            timestamp: 0,
        };

        state.push_undo(entry.clone());
        assert_eq!(state.undo_stack_size(), 1);
        assert_eq!(state.redo_stack_size(), 0);

        let undone = state.pop_undo();
        assert!(undone.is_some());
        assert_eq!(state.undo_stack_size(), 0);
        assert_eq!(state.redo_stack_size(), 1);

        let redone = state.pop_redo();
        assert!(redone.is_some());
        assert_eq!(state.undo_stack_size(), 1);
        assert_eq!(state.redo_stack_size(), 0);
    }

    #[test]
    fn test_batch_edit() {
        let mut handler = EnhancedEditHandler::with_defaults();
        let version = VectorClock::default();

        let batch = EditBatch {
            operations: vec![
                EditOperation {
                    op_type: EditOperationType::Insert,
                    position: Position::new(0, 0),
                    content: "Hello".to_string(),
                    old_range: None,
                    timestamp: 0,
                },
                EditOperation {
                    op_type: EditOperationType::Insert,
                    position: Position::new(0, 5),
                    content: " World".to_string(),
                    old_range: None,
                    timestamp: 1,
                },
            ],
            base_version: version.clone(),
            timestamp: 0,
        };

        let result = handler.handle_batch(batch, &version);

        assert!(result.success);
        assert_eq!(result.applied_operations.len(), 2);
    }

    #[test]
    fn test_handler_manager() {
        let mut manager = EditHandlerManager::new();

        assert_eq!(manager.active_sessions(), 0);

        let handler1 = manager.get_handler("session1");
        assert_eq!(manager.active_sessions(), 1);

        let handler2 = manager.get_handler("session2");
        assert_eq!(manager.active_sessions(), 2);

        manager.remove_handler("session1");
        assert_eq!(manager.active_sessions(), 1);
    }
}
