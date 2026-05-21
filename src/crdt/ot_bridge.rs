//! # OT-CRDT 桥接模块
//!
//! 提供 OT (Operational Transform) 和 CRDT 之间的转换和互操作能力。
//! 允许在现有 OT 系统上添加 CRDT 功能，或者在 CRDT 上使用 OT 算法。

use std::collections::{HashMap, VecDeque};
use serde::{Deserialize, Serialize};
use super::{CrdtNodeId, LogicalClock, CrdtOperation, Position, SelectionRange};

/// OT 操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OtOpType {
    Insert,
    Delete,
    Replace,
}

/// OT 操作
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtOperation {
    pub op_type: OtOpType,
    pub position: Position,
    pub content: Option<String>,
    pub length: Option<usize>,
    pub timestamp: i64,
}

/// OT 客户端状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtClientState {
    pub revision: i64,
    pub pending_ops: Vec<OtOperation>,
    pub sent_ops: Vec<OtOperation>,
}

/// OT 服务器状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtServerState {
    pub revision: i64,
    pub history: Vec<OtOperation>,
}

/// OT 转换结果
#[derive(Debug, Clone)]
pub struct OtTransformResult {
    pub transformed_op: OtOperation,
    pub needs_ack: bool,
}

/// OT 发送结果
#[derive(Debug, Clone)]
pub struct OtSendResult {
    pub accepted: bool,
    pub revision: i64,
    pub transformed_op: Option<OtOperation>,
    pub error: Option<String>,
}

/// OT 桥接配置
#[derive(Debug, Clone)]
pub struct OtBridgeConfig {
    /// 历史保留大小
    pub history_size: usize,
    /// 等待确认超时 (毫秒)
    pub ack_timeout_ms: u64,
    /// 最大重试次数
    pub max_retries: u32,
    /// 是否启用压缩
    pub enable_compression: bool,
}

impl Default for OtBridgeConfig {
    fn default() -> Self {
        Self {
            history_size: 1000,
            ack_timeout_ms: 5000,
            max_retries: 3,
            enable_compression: true,
        }
    }
}

/// OT-CRDT 桥接器
pub struct OtCrdtBridge {
    config: OtBridgeConfig,
    client_state: OtClientState,
    server_state: OtServerState,
    node_id: CrdtNodeId,
}

impl OtCrdtBridge {
    pub fn new(node_id: CrdtNodeId) -> Self {
        Self {
            config: OtBridgeConfig::default(),
            client_state: OtClientState {
                revision: 0,
                pending_ops: Vec::new(),
                sent_ops: Vec::new(),
            },
            server_state: OtServerState {
                revision: 0,
                history: Vec::new(),
            },
            node_id,
        }
    }

    pub fn set_config(&mut self, config: OtBridgeConfig) {
        self.config = config;
    }

    // =========================================================================
    // OT 操作
    // =========================================================================

    /// 客户端发送操作
    pub fn client_send(&mut self, op: OtOperation) -> OtSendResult {
        // 添加到待发送队列
        self.client_state.pending_ops.push(op.clone());
        self.client_state.sent_ops.push(op.clone());

        // 尝试提交到服务器
        self.try_submit()
    }

    fn try_submit(&mut self) -> OtSendResult {
        if self.client_state.pending_ops.is_empty() {
            return OtSendResult {
                accepted: true,
                revision: self.client_state.revision,
                transformed_op: None,
                error: None,
            };
        }

        // 取出最早的待发送操作
        let op = self.client_state.pending_ops.remove(0);

        // 转换操作以应对服务器状态
        let transformed = self.transform_against_history(&op);
        
        // 模拟服务器接受
        self.server_state.revision += 1;
        self.server_state.history.push(transformed.clone());
        
        // 保持历史大小
        while self.server_state.history.len() > self.config.history_size {
            self.server_state.history.remove(0);
        }

        // 确认客户端
        self.client_state.revision = self.server_state.revision;
        
        // 移除已确认的 sent_ops
        if let Some(pos) = self.client_state.sent_ops.iter().position(|o| o == &op) {
            self.client_state.sent_ops.remove(pos);
        }

        OtSendResult {
            accepted: true,
            revision: self.server_state.revision,
            transformed_op: Some(transformed),
            error: None,
        }
    }

    /// 服务器接收操作
    pub fn server_receive(&mut self, op: OtOperation) -> OtTransformResult {
        // 转换操作以应对历史
        let transformed = self.transform_against_history(&op);
        
        // 应用到服务器状态
        self.server_state.revision += 1;
        self.server_state.history.push(transformed.clone());

        OtTransformResult {
            transformed_op: transformed,
            needs_ack: true,
        }
    }

    /// 转换操作以应对历史
    fn transform_against_history(&self, op: &OtOperation) -> OtOperation {
        let mut transformed = op.clone();

        for history_op in &self.server_state.history {
            transformed = self.transform(&transformed, history_op);
        }

        transformed
    }

    /// OT 转换算法
    fn transform(&self, op1: &OtOperation, op2: &OtOperation) -> OtOperation {
        // 简化的 OT 转换实现
        match (&op1.op_type, &op2.op_type) {
            (OtOpType::Insert, OtOpType::Insert) => {
                // 两个插入操作
                if op1.position < op2.position || (op1.position == op2.position && self.tiebreaker(op1, op2)) {
                    // op1 在 op2 之前，不需要改变
                    op1.clone()
                } else {
                    // op1 在 op2 之后，需要调整位置
                    let offset = op2.content.as_ref().map(|c| c.len()).unwrap_or(0);
                    OtOperation {
                        position: Position {
                            line: op1.position.line,
                            column: op1.position.column + offset,
                        },
                        ..op1.clone()
                    }
                }
            }
            (OtOpType::Delete, OtOpType::Delete) => {
                // 两个删除操作
                if op1.position >= op2.position + op2.length.unwrap_or(0) {
                    // op1 在 op2 之后，需要调整位置
                    let offset = op2.length.unwrap_or(0);
                    OtOperation {
                        position: Position {
                            line: op1.position.line,
                            column: (op1.position.column as i64 - offset as i64).max(0) as usize,
                        },
                        ..op1.clone()
                    }
                } else if op1.position + op1.length.unwrap_or(0) <= op2.position {
                    // op1 在 op2 之前，不需要改变
                    op1.clone()
                } else {
                    // 重叠，需要特殊处理
                    op1.clone()
                }
            }
            (OtOpType::Insert, OtOpType::Delete) => {
                // op1 是插入，op2 是删除
                if op1.position <= op2.position {
                    op1.clone()
                } else {
                    let offset = op2.length.unwrap_or(0);
                    OtOperation {
                        position: Position {
                            line: op1.position.line,
                            column: (op1.position.column as i64 - offset as i64).max(0) as usize,
                        },
                        ..op1.clone()
                    }
                }
            }
            (OtOpType::Delete, OtOpType::Insert) => {
                // op1 是删除，op2 是插入
                if op1.position >= op2.position {
                    let offset = op2.content.as_ref().map(|c| c.len()).unwrap_or(0);
                    OtOperation {
                        position: Position {
                            line: op1.position.line,
                            column: op1.position.column + offset,
                        },
                        ..op1.clone()
                    }
                } else {
                    op1.clone()
                }
            }
            _ => op1.clone(),
        }
    }

    /// 平局打破 - 用于相同位置的操作
    fn tiebreaker(&self, op1: &OtOperation, op2: &OtOperation) -> bool {
        op1.timestamp < op2.timestamp || 
        (op1.timestamp == op2.timestamp && self.node_id.client_id < op2.timestamp as u64)
    }

    // =========================================================================
    // CRDT 操作转换
    // =========================================================================

    /// 将 CRDT 操作转换为 OT 操作
    pub fn crdt_to_ot(&self, crdt_op: &CrdtOperation) -> Option<OtOperation> {
        match crdt_op {
            CrdtOperation::Insert { id: _, position, content, clock: _ } => {
                Some(OtOperation {
                    op_type: OtOpType::Insert,
                    position: Position::new(0, *position),
                    content: Some(content.clone()),
                    length: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                })
            }
            CrdtOperation::Delete { id: _, position, length, clock: _ } => {
                Some(OtOperation {
                    op_type: OtOpType::Delete,
                    position: Position::new(0, *position),
                    content: None,
                    length: Some(*length),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                })
            }
            CrdtOperation::Update { .. } => None, // OT 不直接支持更新操作
        }
    }

    /// 将 OT 操作转换为 CRDT 操作
    pub fn ot_to_crdt(&self, ot_op: &OtOperation) -> CrdtOperation {
        match ot_op.op_type {
            OtOpType::Insert => CrdtOperation::Insert {
                id: self.node_id.clone(),
                position: ot_op.position.column,
                content: ot_op.content.clone().unwrap_or_default(),
                clock: LogicalClock::new(),
            },
            OtOpType::Delete | OtOpType::Replace => CrdtOperation::Delete {
                id: self.node_id.clone(),
                position: ot_op.position.column,
                length: ot_op.length.unwrap_or(0),
                clock: LogicalClock::new(),
            },
        }
    }

    // =========================================================================
    // 状态查询
    // =========================================================================

    pub fn get_revision(&self) -> i64 {
        self.client_state.revision
    }

    pub fn get_pending_count(&self) -> usize {
        self.client_state.pending_ops.len()
    }

    pub fn get_history_size(&self) -> usize {
        self.server_state.history.len()
    }

    /// 获取状态摘要
    pub fn get_summary(&self) -> OtBridgeSummary {
        OtBridgeSummary {
            client_revision: self.client_state.revision,
            server_revision: self.server_state.revision,
            pending_ops: self.client_state.pending_ops.len(),
            history_size: self.server_state.history.len(),
        }
    }
}

/// OT 桥接摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtBridgeSummary {
    pub client_revision: i64,
    pub server_revision: i64,
    pub pending_ops: usize,
    pub history_size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_insert_transform() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let bridge = OtCrdtBridge::new(node);

        let op1 = OtOperation {
            op_type: OtOpType::Insert,
            position: Position::new(0, 5),
            content: Some("Hello".to_string()),
            length: None,
            timestamp: 1000,
        };

        let op2 = OtOperation {
            op_type: OtOpType::Insert,
            position: Position::new(0, 3),
            content: Some("XXX".to_string()),
            length: None,
            timestamp: 2000,
        };

        // op2 在位置 3，op1 在位置 5，所以 op1 不需要改变
        let result = bridge.transform(&op1, &op2);
        assert_eq!(result.position.column, 5);
    }

    #[test]
    fn test_insert_at_same_position() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let bridge = OtCrdtBridge::new(node);

        let op1 = OtOperation {
            op_type: OtOpType::Insert,
            position: Position::new(0, 5),
            content: Some("A".to_string()),
            length: None,
            timestamp: 1000,
        };

        let op2 = OtOperation {
            op_type: OtOpType::Insert,
            position: Position::new(0, 5),
            content: Some("B".to_string()),
            length: None,
            timestamp: 2000,
        };

        // 由于 op1.timestamp < op2.timestamp，op1 优先
        let result = bridge.transform(&op1, &op2);
        assert_eq!(result.position.column, 5);
    }

    #[test]
    fn test_delete_delete_transform() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let bridge = OtCrdtBridge::new(node);

        let op1 = OtOperation {
            op_type: OtOpType::Delete,
            position: Position::new(0, 5),
            content: None,
            length: Some(3),
            timestamp: 1000,
        };

        let op2 = OtOperation {
            op_type: OtOpType::Delete,
            position: Position::new(0, 2),
            content: None,
            length: Some(2),
            timestamp: 2000,
        };

        // op1 在位置 5 删除 3 个字符，op2 在位置 2 删除 2 个字符
        // op1 的起点在 op2 之后，不需要调整
        let result = bridge.transform(&op1, &op2);
        assert_eq!(result.position.column, 5);
    }

    #[test]
    fn test_crdt_to_ot_conversion() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let bridge = OtCrdtBridge::new(node);

        let crdt_op = CrdtOperation::Insert {
            id: CrdtNodeId::new("node1".to_string(), 1),
            position: 10,
            content: "Hello".to_string(),
            clock: LogicalClock::new(),
        };

        let ot_op = bridge.crdt_to_ot(&crdt_op).unwrap();
        assert_eq!(ot_op.op_type, OtOpType::Insert);
        assert_eq!(ot_op.position.column, 10);
        assert_eq!(ot_op.content, Some("Hello".to_string()));
    }

    #[test]
    fn test_ot_to_crdt_conversion() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let bridge = OtCrdtBridge::new(node);

        let ot_op = OtOperation {
            op_type: OtOpType::Delete,
            position: Position::new(0, 5),
            content: None,
            length: Some(3),
            timestamp: 1000,
        };

        let crdt_op = bridge.ot_to_crdt(&ot_op);
        match crdt_op {
            CrdtOperation::Delete { id: _, position, length, .. } => {
                assert_eq!(position, 5);
                assert_eq!(length, 3);
            }
            _ => panic!("Expected Delete operation"),
        }
    }

    #[test]
    fn test_client_send() {
        let mut bridge = OtCrdtBridge::new(CrdtNodeId::new("node1".to_string(), 1));

        let op = OtOperation {
            op_type: OtOpType::Insert,
            position: Position::new(0, 0),
            content: Some("Hello".to_string()),
            length: None,
            timestamp: 1000,
        };

        let result = bridge.client_send(op);
        assert!(result.accepted);
        assert_eq!(result.revision, 1);
    }
}
