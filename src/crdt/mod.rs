//! # CRDT (Conflict-free Replicated Data Types) 集成模块
//!
//! 本模块提供了对专业 CRDT 库（如 yrs、automerge）的抽象封装，
//! 增强了现有的 OT (Operational Transform) 系统，提供更强的一致性保证。
//!
//! ## 特性
//! - 序列 CRDT (Sequence CRDT) - 用于文本编辑
//! - 操作转换 (OT) 桥接
//! - 离线编辑支持
//! - 向量时钟版本控制

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::cmp::Ordering;
use serde::{Deserialize, Serialize};

pub mod sequence_crdt;
pub mod ot_bridge;
pub mod version_vector;

pub use sequence_crdt::*;
pub use ot_bridge::*;
pub use version_vector::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod benchmarks;

// ============================================================================
// 核心 CRDT 类型定义
// ============================================================================

/// CRDT 节点 ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CrdtNodeId {
    pub node_id: String,
    pub client_id: u64,
}

impl CrdtNodeId {
    pub fn new(node_id: String, client_id: u64) -> Self {
        Self { node_id, client_id }
    }
}

/// 逻辑时钟
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogicalClock(HashMap<CrdtNodeId, u64>);

impl LogicalClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tick(&mut self, node: &CrdtNodeId) -> u64 {
        let counter = self.0.entry(node.clone()).or_insert(0);
        *counter += 1;
        *counter
    }

    pub fn get(&self, node: &CrdtNodeId) -> u64 {
        self.0.get(node).copied().unwrap_or(0)
    }

    pub fn merge(&mut self, other: &LogicalClock) {
        for (node, counter) in &other.0 {
            let entry = self.0.entry(node.clone()).or_insert(0);
            *entry = (*counter).max(*entry);
        }
    }

    pub fn happened_before(&self, other: &LogicalClock) -> bool {
        // 如果 self 中的所有计数器都 <= other，且至少有一个 <，则 self happened_before other
        let all_less = true;
        let mut some_less = false;

        for (node, counter) in &self.0 {
            let other_counter = other.0.get(node).copied().unwrap_or(0);
            if *counter > other_counter {
                return false;
            }
            if *counter < other_counter {
                some_less = true;
            }
        }

        // 检查 other 中有但 self 中没有的节点
        for (node, counter) in &other.0 {
            if !self.0.contains_key(node) && *counter > 0 {
                some_less = true;
            }
        }

        all_less && some_less
    }

    pub fn concurrent_with(&self, other: &LogicalClock) -> bool {
        !self.happened_before(other) && !other.happened_before(self)
    }
}

/// Tombstone (删除标记) 用于 CRDT 中的软删除
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tombstone {
    pub id: CrdtNodeId,
    pub clock: LogicalClock,
    pub position: usize,
    pub length: usize,
}

/// CRDT 操作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CrdtOperation {
    /// 插入操作
    Insert {
        id: CrdtNodeId,
        position: usize,
        content: String,
        clock: LogicalClock,
    },
    /// 删除操作
    Delete {
        id: CrdtNodeId,
        position: usize,
        length: usize,
        clock: LogicalClock,
    },
    /// 更新属性
    Update {
        id: CrdtNodeId,
        key: String,
        value: String,
        clock: LogicalClock,
    },
}

/// CRDT 状态
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrdtState {
    pub content: Vec<Element>,
    pub tombstones: Vec<Tombstone>,
    pub clock: LogicalClock,
    pub metadata: HashMap<String, String>,
}

/// CRDT 元素
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Element {
    /// 字符元素
    Char {
        id: CrdtNodeId,
        value: char,
        clock: LogicalClock,
    },
    /// 嵌入式元素 (用于标记、评论等)
    Embedded {
        id: CrdtNodeId,
        kind: String,
        data: HashMap<String, String>,
        clock: LogicalClock,
    },
    /// 删除的元素 (墓碑)
    Deleted {
        id: CrdtNodeId,
        original: Box<Element>,
        clock: LogicalClock,
    },
}

/// CRDT 引用 (用于跨文档引用)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrdtRef {
    pub doc_id: String,
    pub position: usize,
}

/// 远程光标状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCursor {
    pub participant_id: String,
    pub position: CursorPosition,
    pub selection: Option<SelectionRange>,
    pub color: String,
    pub name: String,
    pub last_updated: u64,
}

/// 光标位置
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPosition {
    pub line: usize,
    pub column: usize,
    pub absolute_offset: usize,
}

impl CursorPosition {
    pub fn new(line: usize, column: usize, absolute_offset: usize) -> Self {
        Self { line, column, absolute_offset }
    }

    pub fn zero() -> Self {
        Self { line: 0, column: 0, absolute_offset: 0 }
    }
}

/// 选择范围
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRange {
    pub start: CursorPosition,
    pub end: CursorPosition,
}

impl SelectionRange {
    pub fn new(start: CursorPosition, end: CursorPosition) -> Self {
        Self { start, end }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    pub fn contains(&self, pos: CursorPosition) -> bool {
        self.start <= pos && pos <= self.end
    }
}

impl PartialOrd for CursorPosition {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CursorPosition {
    fn cmp(&self, other: &Self) -> Ordering {
        self.absolute_offset.cmp(&other.absolute_offset)
    }
}

/// CRDT 同步消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncMessage {
    /// 同步请求
    SyncRequest {
        doc_id: String,
        clock: LogicalClock,
    },
    /// 同步响应
    SyncResponse {
        doc_id: String,
        state: CrdtState,
        missing_ops: Vec<CrdtOperation>,
    },
    /// 更新操作
    Update {
        doc_id: String,
        operations: Vec<CrdtOperation>,
    },
    /// -awareness 更新
    AwarenessUpdate {
        doc_id: String,
        cursors: Vec<RemoteCursor>,
    },
}

/// CRDT 文档配置
#[derive(Debug, Clone)]
pub struct CrdtConfig {
    /// 是否启用墓碑标记 (删除记录)
    pub enable_tombstones: bool,
    /// 墓碑保留时间 (秒)
    pub tombstone_retention_secs: u64,
    /// 最大离线操作数
    pub max_offline_ops: usize,
    /// 是否启用 OT 桥接
    pub enable_ot_bridge: bool,
    /// 自动合并间隔 (毫秒)
    pub auto_merge_interval_ms: u64,
    /// 是否启用压缩
    pub enable_compression: bool,
}

impl Default for CrdtConfig {
    fn default() -> Self {
        Self {
            enable_tombstones: true,
            tombstone_retention_secs: 3600,
            max_offline_ops: 10000,
            enable_ot_bridge: true,
            auto_merge_interval_ms: 100,
            enable_compression: true,
        }
    }
}

/// CRDT 统计信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrdtStats {
    pub total_operations: u64,
    pub pending_operations: u64,
    pub tombstones_count: usize,
    pub memory_size_bytes: usize,
    pub conflicts_resolved: u64,
    pub merges_performed: u64,
    pub last_sync_time: u64,
}

// ============================================================================
// CRDT 文档管理器
// ============================================================================

/// CRDT 文档管理器
pub struct CrdtDocumentManager {
    documents: HashMap<String, CrdtDocument>,
    config: CrdtConfig,
    stats: CrdtStats,
}

impl CrdtDocumentManager {
    pub fn new(config: CrdtConfig) -> Self {
        Self {
            documents: HashMap::new(),
            config,
            stats: CrdtStats::default(),
        }
    }

    pub fn create_document(&mut self, doc_id: String) -> &mut CrdtDocument {
        let config = self.config.clone();
        self.documents.entry(doc_id.clone()).or_insert_with(|| {
            CrdtDocument::new(doc_id, config)
        })
    }

    pub fn get_document(&self, doc_id: &str) -> Option<&CrdtDocument> {
        self.documents.get(doc_id)
    }

    pub fn get_document_mut(&mut self, doc_id: &str) -> Option<&mut CrdtDocument> {
        self.documents.get_mut(doc_id)
    }

    pub fn remove_document(&mut self, doc_id: &str) -> bool {
        self.documents.remove(doc_id).is_some()
    }

    pub fn list_documents(&self) -> Vec<&str> {
        self.documents.keys().map(|s| s.as_str()).collect()
    }

    pub fn get_stats(&self) -> &CrdtStats {
        &self.stats
    }

    pub fn update_stats(&mut self, stats: CrdtStats) {
        self.stats = stats;
    }
}

/// CRDT 文档
pub struct CrdtDocument {
    pub doc_id: String,
    pub state: CrdtState,
    pub config: CrdtConfig,
    pub pending_ops: Vec<CrdtOperation>,
    pub node_id: CrdtNodeId,
}

impl CrdtDocument {
    pub fn new(doc_id: String, config: CrdtConfig) -> Self {
        Self {
            doc_id,
            state: CrdtState::default(),
            config,
            pending_ops: Vec::new(),
            node_id: CrdtNodeId::new(String::new(), 0),
        }
    }

    pub fn set_node_id(&mut self, node_id: CrdtNodeId) {
        self.node_id = node_id;
    }

    /// 应用本地操作
    pub fn apply_local_op(&mut self, op: CrdtOperation) -> Vec<CrdtOperation> {
        self.state.clock.tick(&self.node_id);
        self.pending_ops.push(op.clone());
        vec![op]
    }

    /// 应用远程操作
    pub fn apply_remote_op(&mut self, op: &CrdtOperation) {
        match op {
            CrdtOperation::Insert { id, position, content, clock } => {
                self.apply_insert(id, *position, content, clock);
            }
            CrdtOperation::Delete { id, position, length, clock } => {
                self.apply_delete(id, *position, *length, clock);
            }
            CrdtOperation::Update { id, key, value, clock } => {
                self.apply_update(id, key, value, clock);
            }
        }
    }

    fn apply_insert(&mut self, id: &CrdtNodeId, position: usize, content: &str, clock: &LogicalClock) {
        for (i, c) in content.chars().enumerate() {
            let elem = Element::Char {
                id: id.clone(),
                value: c,
                clock: clock.clone(),
            };
            let insert_pos = (position + i).min(self.state.content.len());
            self.state.content.insert(insert_pos, elem);
        }
        self.state.clock.merge(clock);
    }

    fn apply_delete(&mut self, id: &CrdtNodeId, position: usize, length: usize, clock: &LogicalClock) {
        if !self.config.enable_tombstones {
            // 直接删除
            for i in 0..length {
                let remove_pos = position.saturating_sub(i);
                if remove_pos < self.state.content.len() {
                    self.state.content.remove(remove_pos);
                }
            }
        } else {
            // 标记为墓碑
            for i in 0..length {
                let tombstone_pos = position.saturating_sub(i);
                if tombstone_pos < self.state.content.len() {
                    let original = self.state.content.remove(tombstone_pos);
                    let tombstone = Tombstone {
                        id: id.clone(),
                        clock: clock.clone(),
                        position: tombstone_pos,
                        length: 1,
                    };
                    self.state.tombstones.push(tombstone);
                    self.state.content.insert(
                        tombstone_pos,
                        Element::Deleted {
                            id: id.clone(),
                            original: Box::new(original),
                            clock: clock.clone(),
                        },
                    );
                }
            }
        }
        self.state.clock.merge(clock);
    }

    fn apply_update(&mut self, _id: &CrdtNodeId, key: &str, value: &str, clock: &LogicalClock) {
        self.state.metadata.insert(key.to_string(), value.to_string());
        self.state.clock.merge(clock);
    }

    /// 获取当前文本内容
    pub fn get_text(&self) -> String {
        self.state
            .content
            .iter()
            .filter_map(|e| match e {
                Element::Char { value, .. } => Some(*value),
                _ => None,
            })
            .collect()
    }

    /// 获取有效字符数
    pub fn len(&self) -> usize {
        self.state.content.iter().filter(|e| matches!(e, Element::Char { .. })).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 合并另一个文档的状态
    pub fn merge(&mut self, other: &CrdtState) {
        // 合并内容
        for elem in &other.content {
            if !self.state.content.contains(elem) {
                self.state.content.push(elem.clone());
            }
        }

        // 合并墓碑
        for tombstone in &other.tombstones {
            if !self.state.tombstones.contains(tombstone) {
                self.state.tombstones.push(tombstone.clone());
            }
        }

        // 合并时钟
        self.state.clock.merge(&other.clock);

        // 合并元数据
        for (k, v) in &other.metadata {
            self.state.metadata.insert(k.clone(), v.clone());
        }
    }

    /// 清理过期的墓碑
    pub fn cleanup_tombstones(&mut self, _max_age_secs: u64) {
        // 简化实现，实际应该检查时间戳
        if self.state.tombstones.len() > 1000 {
            let to_remove = self.state.tombstones.len() - 500;
            self.state.tombstones.drain(0..to_remove);
        }
    }

    /// 获取待处理操作
    pub fn take_pending_ops(&mut self) -> Vec<CrdtOperation> {
        std::mem::take(&mut self.pending_ops)
    }

    /// 检查是否需要同步
    pub fn needs_sync(&self, other_clock: &LogicalClock) -> bool {
        !self.state.clock.happened_before(other_clock) && 
        !other_clock.happened_before(&self.state.clock)
    }
}

// ============================================================================
// CRDT 工厂
// ============================================================================

/// CRDT 工厂 - 用于创建不同类型的 CRDT
pub struct CrdtFactory;

impl CrdtFactory {
    /// 创建序列 CRDT
    pub fn create_sequence_crdt(doc_id: String) -> CrdtDocument {
        CrdtDocument::new(doc_id, CrdtConfig::default())
    }

    /// 创建启用 OT 桥接的 CRDT
    pub fn create_ot_bridged_crdt(doc_id: String) -> CrdtDocument {
        CrdtDocument::new(doc_id, CrdtConfig {
            enable_ot_bridge: true,
            ..Default::default()
        })
    }
}

// ============================================================================
// 序列化支持
// ============================================================================

impl CrdtState {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

impl std::fmt::Display for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Element::Char { value, .. } => write!(f, "{}", value),
            Element::Embedded { kind, .. } => write!(f, "[{}]", kind),
            Element::Deleted { .. } => write!(f, "█"), // Unicode block character
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logical_clock_tick() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let mut clock = LogicalClock::new();
        
        assert_eq!(clock.tick(&node), 1);
        assert_eq!(clock.tick(&node), 2);
        assert_eq!(clock.get(&node), 2);
    }

    #[test]
    fn test_logical_clock_merge() {
        let node1 = CrdtNodeId::new("node1".to_string(), 1);
        let node2 = CrdtNodeId::new("node2".to_string(), 2);
        
        let mut clock1 = LogicalClock::new();
        clock1.tick(&node1);
        clock1.tick(&node1);
        
        let mut clock2 = LogicalClock::new();
        clock2.tick(&node2);
        clock2.tick(&node2);
        clock2.tick(&node2);
        
        clock1.merge(&clock2);
        
        assert_eq!(clock1.get(&node1), 2);
        assert_eq!(clock1.get(&node2), 3);
    }

    #[test]
    fn test_crdt_document_insert() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let mut doc = CrdtDocument::new("test".to_string(), CrdtConfig::default());
        doc.set_node_id(node);
        
        let op = CrdtOperation::Insert {
            id: CrdtNodeId::new("node1".to_string(), 1),
            position: 0,
            content: "Hello".to_string(),
            clock: LogicalClock::new(),
        };
        
        doc.apply_local_op(op);
        assert_eq!(doc.get_text(), "Hello");
        assert_eq!(doc.len(), 5);
    }

    #[test]
    fn test_crdt_document_delete() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let mut doc = CrdtDocument::new("test".to_string(), CrdtConfig::default());
        doc.set_node_id(node);
        
        // Insert first
        let insert_op = CrdtOperation::Insert {
            id: CrdtNodeId::new("node1".to_string(), 1),
            position: 0,
            content: "Hello".to_string(),
            clock: LogicalClock::new(),
        };
        doc.apply_local_op(insert_op);
        
        // Then delete
        let delete_op = CrdtOperation::Delete {
            id: CrdtNodeId::new("node1".to_string(), 1),
            position: 0,
            length: 2,
            clock: LogicalClock::new(),
        };
        doc.apply_remote_op(&delete_op);
        
        assert_eq!(doc.len(), 3);
    }

    #[test]
    fn test_cursor_position_ordering() {
        let pos1 = CursorPosition::new(0, 0, 0);
        let pos2 = CursorPosition::new(1, 0, 10);
        let pos3 = CursorPosition::new(1, 5, 15);
        
        assert!(pos1 < pos2);
        assert!(pos2 < pos3);
        
        let mut positions = vec![pos3, pos1, pos2];
        positions.sort();
        assert_eq!(positions, vec![pos1, pos2, pos3]);
    }

    #[test]
    fn test_selection_range() {
        let start = CursorPosition::new(0, 0, 0);
        let middle = CursorPosition::new(0, 5, 5);
        let end = CursorPosition::new(1, 0, 10);
        
        let range = SelectionRange::new(start, end);
        
        assert!(!range.contains(start));
        assert!(range.contains(middle));
        assert!(range.contains(end));
    }

    #[test]
    fn test_crdt_state_serialization() {
        let state = CrdtState::default();
        let json = state.to_json().unwrap();
        let restored = CrdtState::from_json(&json).unwrap();
        
        assert_eq!(restored.content.len(), 0);
    }
}
