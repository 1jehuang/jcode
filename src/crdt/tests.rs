//! # CRDT 集成测试
//!
//! 全面测试 CRDT 模块的功能和边界情况

#[cfg(test)]
mod tests {
    use super::super::*;
    use chrono::Utc;

    // =========================================================================
    // LogicalClock 测试
    // =========================================================================

    #[test]
    fn test_logical_clock_basic() {
        let node1 = CrdtNodeId::new("node1".to_string(), 1);
        let node2 = CrdtNodeId::new("node2".to_string(), 2);
        
        let mut clock1 = LogicalClock::new();
        clock1.tick(&node1);
        clock1.tick(&node1);
        clock1.tick(&node2);
        
        assert_eq!(clock1.get(&node1), 2);
        assert_eq!(clock1.get(&node2), 1);
        assert_eq!(clock1.get(&CrdtNodeId::new("node3".to_string(), 3)), 0);
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
    fn test_logical_clock_happened_before() {
        let node1 = CrdtNodeId::new("node1".to_string(), 1);
        let node2 = CrdtNodeId::new("node2".to_string(), 2);
        
        let mut base = LogicalClock::new();
        base.tick(&node1);
        
        let mut derived = base.clone();
        derived.tick(&node1);
        derived.tick(&node2);
        
        assert!(base.happened_before(&derived));
        assert!(!derived.happened_before(&base));
    }

    #[test]
    fn test_logical_clock_concurrent() {
        let node1 = CrdtNodeId::new("node1".to_string(), 1);
        let node2 = CrdtNodeId::new("node2".to_string(), 2);
        
        let mut clock1 = LogicalClock::new();
        clock1.tick(&node1);
        
        let mut clock2 = LogicalClock::new();
        clock2.tick(&node2);
        
        assert!(clock1.concurrent_with(&clock2));
        assert!(clock2.concurrent_with(&clock1));
    }

    // =========================================================================
    // CrdtDocument 测试
    // =========================================================================

    #[test]
    fn test_crdt_document_insert() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let mut doc = CrdtDocument::new("test_doc".to_string(), CrdtConfig::default());
        doc.set_node_id(node.clone());
        
        let op = CrdtOperation::Insert {
            id: node.clone(),
            position: 0,
            content: "Hello".to_string(),
            clock: LogicalClock::new(),
        };
        
        let result = doc.apply_local_op(op);
        assert_eq!(result.len(), 1);
        assert_eq!(doc.get_text(), "Hello");
        assert_eq!(doc.len(), 5);
    }

    #[test]
    fn test_crdt_document_delete() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let mut doc = CrdtDocument::new("test_doc".to_string(), CrdtConfig::default());
        doc.set_node_id(node.clone());
        
        // Insert first
        let insert_op = CrdtOperation::Insert {
            id: node.clone(),
            position: 0,
            content: "Hello World".to_string(),
            clock: LogicalClock::new(),
        };
        doc.apply_local_op(insert_op);
        
        // Then delete "World"
        let delete_op = CrdtOperation::Delete {
            id: node.clone(),
            position: 6,
            length: 5,
            clock: LogicalClock::new(),
        };
        doc.apply_remote_op(&delete_op);
        
        assert_eq!(doc.get_text(), "Hello ");
    }

    #[test]
    fn test_crdt_document_merge() {
        let node1 = CrdtNodeId::new("node1".to_string(), 1);
        let node2 = CrdtNodeId::new("node2".to_string(), 2);
        
        let mut doc1 = CrdtDocument::new("test_doc".to_string(), CrdtConfig::default());
        doc1.set_node_id(node1.clone());
        doc1.apply_local_op(CrdtOperation::Insert {
            id: node1.clone(),
            position: 0,
            content: "Hello".to_string(),
            clock: LogicalClock::new(),
        });
        
        let mut doc2 = CrdtDocument::new("test_doc".to_string(), CrdtConfig::default());
        doc2.set_node_id(node2.clone());
        doc2.apply_local_op(CrdtOperation::Insert {
            id: node2.clone(),
            position: 5,
            content: " World".to_string(),
            clock: LogicalClock::new(),
        });
        
        doc1.merge(&doc2.state);
        assert!(doc1.get_text().contains("Hello"));
        assert!(doc1.get_text().contains("World"));
    }

    #[test]
    fn test_crdt_document_empty() {
        let doc = CrdtDocument::new("empty".to_string(), CrdtConfig::default());
        assert!(doc.is_empty());
        assert_eq!(doc.get_text(), "");
    }

    #[test]
    fn test_crdt_document_tombstones() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let mut doc = CrdtDocument::new("test".to_string(), CrdtConfig {
            enable_tombstones: true,
            ..Default::default()
        });
        doc.set_node_id(node.clone());
        
        doc.apply_local_op(CrdtOperation::Insert {
            id: node.clone(),
            position: 0,
            content: "ABCDE".to_string(),
            clock: LogicalClock::new(),
        });
        
        let delete_op = CrdtOperation::Delete {
            id: node.clone(),
            position: 1,
            length: 2,
            clock: LogicalClock::new(),
        };
        doc.apply_remote_op(&delete_op);
        
        // With tombstones enabled, content should show deleted markers
        assert_eq!(doc.state.tombstones.len(), 2);
        assert_eq!(doc.get_text(), "ADE");
    }

    // =========================================================================
    // SequenceCrdtEditor 测试
    // =========================================================================

    #[test]
    fn test_sequence_crdt_insert() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        let result = editor.insert(0, "Hello");
        assert!(result.success);
        assert_eq!(result.positions, vec![0, 1, 2, 3, 4]);
        assert_eq!(editor.get_text(), "Hello");
        assert_eq!(editor.len(), 5);
    }

    #[test]
    fn test_sequence_crdt_delete() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        editor.insert(0, "Hello");
        let result = editor.delete(1, 2);
        assert!(result.success);
        assert_eq!(result.tombstones_created, 2);
        assert_eq!(editor.get_text(), "Hlo");
    }

    #[test]
    fn test_sequence_crdt_insert_middle() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        editor.insert(0, "Hello");
        editor.insert(5, " World");
        assert_eq!(editor.get_text(), "Hello World");
    }

    #[test]
    fn test_sequence_crdt_serialization() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        editor.insert(0, "Hello World");
        let json = editor.to_json().unwrap();
        
        let restored = SequenceCrdtEditor::from_json(&json).unwrap();
        assert_eq!(restored.get_text(), "Hello World");
    }

    #[test]
    fn test_sequence_crdt_merge() {
        let mut editor1 = SequenceCrdtEditor::new("doc1".to_string());
        editor1.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        editor1.insert(0, "Hello");
        
        let mut editor2 = SequenceCrdtEditor::new("doc2".to_string());
        editor2.set_node_id(CrdtNodeId::new("node2".to_string(), 2));
        editor2.insert(0, "Hi");
        
        editor1.merge(&editor2.state);
        assert!(editor1.get_text().len() > 0);
    }

    // =========================================================================
    // OtCrdtBridge 测试
    // =========================================================================

    #[test]
    fn test_ot_crdt_bridge_create() {
        let node = CrdtNodeId::new("node1".to_string(), 1);
        let bridge = OtCrdtBridge::new(node);
        
        assert_eq!(bridge.get_revision(), 0);
        assert_eq!(bridge.get_pending_count(), 0);
    }

    #[test]
    fn test_ot_crdt_bridge_send() {
        let mut bridge = OtCrdtBridge::new(CrdtNodeId::new("node1".to_string(), 1));
        
        let op = OtOperation {
            op_type: OtOpType::Insert,
            position: Position::new(0, 0),
            content: Some("Test".to_string()),
            length: None,
            timestamp: Utc::now().timestamp_millis(),
        };
        
        let result = bridge.client_send(op);
        assert!(result.accepted);
        assert_eq!(result.revision, 1);
    }

    #[test]
    fn test_ot_crdt_bridge_transform() {
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
            position: Position::new(0, 3),
            content: Some("B".to_string()),
            length: None,
            timestamp: 2000,
        };
        
        let result = bridge.transform(&op1, &op2);
        assert_eq!(result.position.column, 5);
    }

    #[test]
    fn test_ot_crdt_bridge_conversion() {
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
        
        let converted_back = bridge.ot_to_crdt(&ot_op);
        match converted_back {
            CrdtOperation::Insert { position, content, .. } => {
                assert_eq!(position, 10);
                assert_eq!(content, "Hello");
            }
            _ => panic!("Expected Insert operation"),
        }
    }

    // =========================================================================
    // VersionVector 测试
    // =========================================================================

    #[test]
    fn test_version_vector_increment() {
        let mut vv = VersionVector::new();
        assert_eq!(vv.increment("node1"), 1);
        assert_eq!(vv.increment("node1"), 2);
        assert_eq!(vv.increment("node2"), 1);
        
        assert_eq!(vv.get("node1"), 2);
        assert_eq!(vv.get("node2"), 1);
        assert_eq!(vv.get("node3"), 0);
    }

    #[test]
    fn test_version_vector_merge() {
        let mut vv1 = VersionVector::new();
        vv1.increment("node1");
        vv1.increment("node1");
        vv1.increment("node2");
        
        let mut vv2 = VersionVector::new();
        vv2.increment("node1");
        vv2.increment("node3");
        
        let merged = vv1.merge(&vv2);
        
        assert_eq!(merged.get("node1"), 2);
        assert_eq!(merged.get("node2"), 1);
        assert_eq!(merged.get("node3"), 1);
    }

    #[test]
    fn test_version_vector_happened_before() {
        let mut vv1 = VersionVector::new();
        vv1.increment("node1");
        vv1.increment("node1");
        
        let mut vv2 = vv1.clone();
        vv2.increment("node2");
        
        assert!(vv1.happened_before(&vv2));
        assert!(!vv2.happened_before(&vv1));
    }

    #[test]
    fn test_version_vector_concurrent() {
        let mut vv1 = VersionVector::new();
        vv1.increment("node1");
        
        let mut vv2 = VersionVector::new();
        vv2.increment("node2");
        
        assert!(vv1.is_concurrent(&vv2));
    }

    #[test]
    fn test_version_vector_compare() {
        let mut vv1 = VersionVector::new();
        vv1.increment("node1");
        
        let mut vv2 = vv1.clone();
        
        assert_eq!(vv1.compare(&vv2), VersionRelation::Equal);
        
        vv2.increment("node1");
        assert_eq!(vv1.compare(&vv2), VersionRelation::Before);
        assert_eq!(vv2.compare(&vv1), VersionRelation::After);
        
        let mut vv3 = VersionVector::new();
        vv3.increment("node2");
        assert_eq!(vv1.compare(&vv3), VersionRelation::Concurrent);
    }

    // =========================================================================
    // EnhancedEditHandler 测试
    // =========================================================================

    #[test]
    fn test_enhanced_edit_handler_basic() {
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
    fn test_enhanced_edit_handler_batch() {
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
    fn test_enhanced_edit_handler_undo_redo() {
        let mut handler = EnhancedEditHandler::with_defaults();
        let version = VectorClock::default();
        
        let op = EditOperation {
            op_type: EditOperationType::Insert,
            position: Position::new(0, 0),
            content: "Test".to_string(),
            old_range: None,
            timestamp: 0,
        };
        
        handler.handle_edit(op, &version);
        assert_eq!(handler.get_summary().pending_ops, 0);
        
        let undo_entry = handler.undo();
        assert!(undo_entry.is_some());
        
        let redo_entry = handler.redo();
        assert!(redo_entry.is_some());
    }

    // =========================================================================
    // 并发编辑测试
    // =========================================================================

    #[test]
    fn test_concurrent_edits() {
        let node1 = CrdtNodeId::new("node1".to_string(), 1);
        let node2 = CrdtNodeId::new("node2".to_string(), 2);
        
        let mut doc1 = CrdtDocument::new("shared_doc".to_string(), CrdtConfig::default());
        doc1.set_node_id(node1.clone());
        
        let mut doc2 = CrdtDocument::new("shared_doc".to_string(), CrdtConfig::default());
        doc2.set_node_id(node2.clone());
        
        // Both insert at position 0
        doc1.apply_local_op(CrdtOperation::Insert {
            id: node1.clone(),
            position: 0,
            content: "Hello".to_string(),
            clock: LogicalClock::new(),
        });
        
        doc2.apply_local_op(CrdtOperation::Insert {
            id: node2.clone(),
            position: 0,
            content: "Hi".to_string(),
            clock: LogicalClock::new(),
        });
        
        // Merge should handle concurrent edits
        doc1.merge(&doc2.state);
        assert!(doc1.get_text().len() >= 2);
    }

    #[test]
    fn test_concurrent_deletes() {
        let node1 = CrdtNodeId::new("node1".to_string(), 1);
        let node2 = CrdtNodeId::new("node2".to_string(), 2);
        
        let mut doc1 = CrdtDocument::new("shared_doc".to_string(), CrdtConfig::default());
        doc1.set_node_id(node1.clone());
        
        // First insert content
        doc1.apply_local_op(CrdtOperation::Insert {
            id: node1.clone(),
            position: 0,
            content: "ABCDEFG".to_string(),
            clock: LogicalClock::new(),
        });
        
        // Clone for second node
        let mut doc2 = CrdtDocument::new("shared_doc".to_string(), CrdtConfig::default());
        doc2.set_node_id(node2.clone());
        doc2.state = doc1.state.clone();
        
        // Concurrent deletes
        doc1.apply_local_op(CrdtOperation::Delete {
            id: node1.clone(),
            position: 1,
            length: 2,
            clock: LogicalClock::new(),
        });
        
        doc2.apply_local_op(CrdtOperation::Delete {
            id: node2.clone(),
            position: 3,
            length: 2,
            clock: LogicalClock::new(),
        });
        
        doc1.merge(&doc2.state);
        let text = doc1.get_text();
        assert_eq!(text, "AFG");
    }

    // =========================================================================
    // 边界条件测试
    // =========================================================================

    #[test]
    fn test_empty_insert() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        let result = editor.insert(0, "");
        assert!(result.success);
        assert_eq!(result.positions, vec![]);
        assert_eq!(editor.len(), 0);
    }

    #[test]
    fn test_delete_empty_document() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        let result = editor.delete(0, 5);
        assert!(result.success);
        assert_eq!(result.tombstones_created, 0);
    }

    #[test]
    fn test_delete_beyond_end() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        editor.insert(0, "Hello");
        
        let result = editor.delete(10, 100);
        assert!(result.success);
        assert_eq!(editor.get_text(), "Hello"); // Should not delete beyond bounds
    }

    #[test]
    fn test_insert_at_boundary() {
        let mut editor = SequenceCrdtEditor::new("doc1".to_string());
        editor.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        editor.insert(0, "Hello");
        editor.insert(5, " World");
        
        assert_eq!(editor.get_text(), "Hello World");
    }

    // =========================================================================
    // 序列化测试
    // =========================================================================

    #[test]
    fn test_crdt_state_serialization() {
        let state = CrdtState::default();
        let json = state.to_json().unwrap();
        let restored = CrdtState::from_json(&json).unwrap();
        
        assert_eq!(restored.content.len(), 0);
        assert_eq!(restored.tombstones.len(), 0);
    }

    #[test]
    fn test_crdt_state_with_content() {
        let mut doc = CrdtDocument::new("test".to_string(), CrdtConfig::default());
        doc.set_node_id(CrdtNodeId::new("node1".to_string(), 1));
        
        doc.apply_local_op(CrdtOperation::Insert {
            id: CrdtNodeId::new("node1".to_string(), 1),
            position: 0,
            content: "Test".to_string(),
            clock: LogicalClock::new(),
        });
        
        let json = doc.state.to_json().unwrap();
        let restored = CrdtState::from_json(&json).unwrap();
        
        assert_eq!(restored.content.len(), 4);
    }

    // =========================================================================
    // 配置测试
    // =========================================================================

    #[test]
    fn test_crdt_config_defaults() {
        let config = CrdtConfig::default();
        
        assert!(config.enable_tombstones);
        assert_eq!(config.tombstone_retention_secs, 3600);
        assert_eq!(config.max_offline_ops, 10000);
        assert!(config.enable_ot_bridge);
        assert_eq!(config.auto_merge_interval_ms, 100);
        assert!(config.enable_compression);
    }

    #[test]
    fn test_crdt_config_custom() {
        let config = CrdtConfig {
            enable_tombstones: false,
            tombstone_retention_secs: 60,
            max_offline_ops: 1000,
            enable_ot_bridge: false,
            auto_merge_interval_ms: 500,
            enable_compression: false,
        };
        
        assert!(!config.enable_tombstones);
        assert_eq!(config.max_offline_ops, 1000);
        assert!(!config.enable_ot_bridge);
    }
}
