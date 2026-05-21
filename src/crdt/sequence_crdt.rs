//! # Sequence CRDT - 序列 CRDT 实现
//!
//! 专门用于文本编辑的 CRDT 实现，支持高效的插入和删除操作。

use std::collections::{HashMap, VecDeque};
use serde::{Deserialize, Serialize};
use super::{CrdtNodeId, CrdtOperation, LogicalClock, Element};

/// 序列 CRDT 状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceCrdtState {
    /// 序列中的所有元素
    pub items: Vec<SequenceItem>,
    /// ID 到位置的映射
    pub id_index: HashMap<String, usize>,
    /// 最大 tombstone 数
    pub max_tombstones: usize,
    /// tombstone 队列 (用于清理)
    pub tombstone_queue: VecDeque<String>,
}

/// 序列项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceItem {
    pub id: String,
    pub value: String,
    pub left_id: Option<String>,
    pub right_id: Option<String>,
    pub clock: LogicalClock,
    pub is_deleted: bool,
    pub deleted_by: Option<CrdtNodeId>,
}

impl SequenceItem {
    pub fn new(id: String, value: String, left_id: Option<String>, right_id: Option<String>) -> Self {
        Self {
            id,
            value,
            left_id,
            right_id,
            clock: LogicalClock::new(),
            is_deleted: false,
            deleted_by: None,
        }
    }

    pub fn length(&self) -> usize {
        self.value.chars().count()
    }
}

/// 序列 CRDT 操作结果
#[derive(Debug, Clone)]
pub struct SequenceCrdtResult {
    pub success: bool,
    pub positions: Vec<usize>,
    pub tombstones_created: usize,
    pub error: Option<String>,
}

/// 序列 CRDT 编辑器
pub struct SequenceCrdtEditor {
    state: SequenceCrdtState,
    node_id: CrdtNodeId,
}

impl SequenceCrdtEditor {
    pub fn new(doc_id: String) -> Self {
        Self {
            state: SequenceCrdtState {
                items: Vec::new(),
                id_index: HashMap::new(),
                max_tombstones: 1000,
                tombstone_queue: VecDeque::new(),
            },
            node_id: CrdtNodeId::new(doc_id, 0),
        }
    }

    pub fn set_node_id(&mut self, node_id: CrdtNodeId) {
        self.node_id = node_id;
    }

    /// 插入文本
    pub fn insert(&mut self, position: usize, content: &str) -> SequenceCrdtResult {
        if position > self.len() {
            return SequenceCrdtResult {
                success: false,
                positions: vec![],
                tombstones_created: 0,
                error: Some(format!("Position {} out of bounds (length: {})", position, self.len())),
            };
        }

        let mut positions = Vec::new();
        let mut clock = LogicalClock::new();
        clock.tick(&self.node_id);

        // 找到插入位置的左边和右边的元素
        let (left_id, right_id) = if position == 0 {
            (None, self.state.items.first().map(|i| i.id.clone()))
        } else if position >= self.len() {
            (self.state.items.last().map(|i| i.id.clone()), None)
        } else {
            let left_item = &self.state.items[position - 1];
            let right_item = self.state.items.get(position);
            (
                Some(left_item.id.clone()),
                right_item.map(|i| i.id.clone()),
            )
        };

        // 创建新元素
        for (i, c) in content.chars().enumerate() {
            let id = format!("{}:{}:{}", self.node_id.node_id, self.node_id.client_id, clock.get(&self.node_id) + i as u64);
            let item = SequenceItem {
                id: id.clone(),
                value: c.to_string(),
                left_id: if i == 0 { left_id.clone() } else { Some(positions.last().map(|p| format!("{}:{}:{}", self.node_id.node_id, self.node_id.client_id, clock.get(&self.node_id) + (i-1) as u64)).unwrap()) },
                right_id: right_id.clone(),
                clock: clock.clone(),
                is_deleted: false,
                deleted_by: None,
            };

            let insert_pos = position + i;
            self.state.id_index.insert(id.clone(), insert_pos);
            self.state.items.insert(insert_pos, item);
            positions.push(insert_pos);
        }

        // 更新相邻元素的指针
        self.update_adjacent_pointers(&positions);

        SequenceCrdtResult {
            success: true,
            positions,
            tombstones_created: 0,
            error: None,
        }
    }

    /// 删除文本
    pub fn delete(&mut self, position: usize, length: usize) -> SequenceCrdtResult {
        let end_pos = (position + length).min(self.len());
        if position >= self.len() {
            return SequenceCrdtResult {
                success: false,
                positions: vec![],
                tombstones_created: 0,
                error: Some(format!("Position {} out of bounds", position)),
            };
        }

        let mut tombstones_created = 0;
        let mut clock = LogicalClock::new();
        clock.tick(&self.node_id);

        for i in position..end_pos {
            let item = &mut self.state.items[i];
            if !item.is_deleted {
                item.is_deleted = true;
                item.deleted_by = Some(self.node_id.clone());
                tombstones_created += 1;

                // 添加到 tombstone 队列
                self.state.tombstone_queue.push_back(item.id.clone());
                
                // 清理过期的 tombstones
                while self.state.tombstone_queue.len() > self.state.max_tombstones {
                    if let Some(old_id) = self.state.tombstone_queue.pop_front() {
                        self.remove_item(&old_id);
                    }
                }
            }
        }

        SequenceCrdtResult {
            success: true,
            positions: (position..end_pos).collect(),
            tombstones_created,
            error: None,
        }
    }

    fn update_adjacent_pointers(&mut self, positions: &[usize]) {
        // 更新插入序列的内部指针
        for (i, &pos) in positions.iter().enumerate() {
            let item = &mut self.state.items[pos];
            if i > 0 {
                item.left_id = Some(positions[i - 1].to_string());
            }
            if i < positions.len() - 1 {
                item.right_id = Some(positions[i + 1].to_string());
            }
        }
    }

    fn remove_item(&mut self, id: &str) {
        if let Some(&pos) = self.state.id_index.get(id) {
            self.state.items.remove(pos);
            self.state.id_index.remove(id);
            // 更新所有后续位置
            for (i, item) in self.state.items.iter_mut().enumerate().skip(pos) {
                self.state.id_index.insert(item.id.clone(), i);
            }
        }
    }

    /// 获取当前文本
    pub fn get_text(&self) -> String {
        self.state
            .items
            .iter()
            .filter(|item| !item.is_deleted)
            .map(|item| item.value.clone())
            .collect()
    }

    /// 获取有效长度
    pub fn len(&self) -> usize {
        self.state.items.iter().filter(|item| !item.is_deleted).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 合并另一个序列的状态
    pub fn merge(&mut self, other: &SequenceCrdtState) {
        for item in &other.items {
            if !self.state.id_index.contains_key(&item.id) {
                let pos = self.state.items.len();
                self.state.id_index.insert(item.id.clone(), pos);
                self.state.items.push(item.clone());
            }
        }
    }

    /// 序列化状态
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.state)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let state: SequenceCrdtState = serde_json::from_str(json)?;
        let mut editor = Self {
            state,
            node_id: CrdtNodeId::new(String::new(), 0),
        };
        // 重建 id_index
        editor.state.id_index.clear();
        for (i, item) in editor.state.items.iter().enumerate() {
            editor.state.id_index.insert(item.id.clone(), i);
        }
        Ok(editor)
    }
}

impl Default for SequenceCrdtState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            id_index: HashMap::new(),
            max_tombstones: 1000,
            tombstone_queue: VecDeque::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));

        let result = editor.insert(0, "Hello");
        assert!(result.success);
        assert_eq!(result.positions, vec![0, 1, 2, 3, 4]);
        assert_eq!(editor.get_text(), "Hello");
    }

    #[test]
    fn test_delete() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));

        editor.insert(0, "Hello");
        let result = editor.delete(1, 2);
        assert!(result.success);
        assert_eq!(result.tombstones_created, 2);
        assert_eq!(editor.get_text(), "Hlo");
    }

    #[test]
    fn test_insert_at_position() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));

        editor.insert(0, "Hello");
        editor.insert(5, " World");
        assert_eq!(editor.get_text(), "Hello World");
    }

    #[test]
    fn test_multiple_operations() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));

        editor.insert(0, "Hello");
        editor.insert(5, " World");
        editor.delete(5, 1); // Delete space
        assert_eq!(editor.get_text(), "HelloWorld");

        editor.insert(5, " ");
        assert_eq!(editor.get_text(), "Hello World");
    }

    #[test]
    fn test_empty_state() {
        let editor = SequenceCrdtEditor::new("doc1".to_string());
        assert_eq!(editor.get_text(), "");
        assert_eq!(editor.len(), 0);
        assert!(editor.is_empty());
    }

    #[test]
    fn test_serialization() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));

        editor.insert(0, "Hello");
        let json = editor.to_json().unwrap();

        let restored = SequenceCrdtEditor::from_json(&json).unwrap();
        assert_eq!(restored.get_text(), "Hello");
    }
}
