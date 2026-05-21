//! # CRDT vs OT 性能基准测试
//!
//! 比较 CRDT 和 OT 算法在不同场景下的性能表现

#[cfg(test)]
mod benchmarks {
    use super::super::*;
    use std::time::{Instant, Duration};
    use rand::{Rng, thread_rng};

    // =========================================================================
    // 基准测试结果
    // =========================================================================

    #[derive(Debug, Clone)]
    pub struct BenchmarkResult {
        pub test_name: String,
        pub crdt_time: Duration,
        pub ot_time: Duration,
        pub crdt_operations: usize,
        pub ot_operations: usize,
        pub crdt_memory: usize,
        pub ot_memory: usize,
    }

    impl BenchmarkResult {
        pub fn print_summary(&self) {
            println!("=== {} ===", self.test_name);
            println!("CRDT: {:?} ({} ops)", self.crdt_time, self.crdt_operations);
            println!("OT:   {:?} ({} ops)", self.ot_time, self.ot_operations);
            
            let ratio = if self.crdt_time.as_nanos() > 0 {
                self.ot_time.as_nanos() as f64 / self.crdt_time.as_nanos() as f64
            } else {
                f64::INFINITY
            };
            
            if ratio > 1.0 {
                println!("CRDT is {:.2}x faster", ratio);
            } else {
                println!("OT is {:.2}x faster", 1.0 / ratio);
            }
            println!();
        }
    }

    // =========================================================================
    // 基准测试工具函数
    // =========================================================================

    /// 生成随机插入操作
    fn generate_random_inserts(count: usize, max_length: usize) -> Vec<(usize, String)> {
        let mut rng = thread_rng();
        let mut ops = Vec::with_capacity(count);
        
        for _ in 0..count {
            let pos = rng.gen_range(0..max_length);
            let len = rng.gen_range(1..=20);
            let content: String = (0..len).map(|_| rng.gen_range(b'a'..=b'z') as char).collect();
            ops.push((pos, content));
        }
        
        ops
    }

    /// 生成随机删除操作
    fn generate_random_deletes(count: usize, max_length: usize) -> Vec<(usize, usize)> {
        let mut rng = thread_rng();
        let mut ops = Vec::with_capacity(count);
        
        for _ in 0..count {
            let pos = rng.gen_range(0..max_length);
            let len = rng.gen_range(1..=10);
            ops.push((pos, len));
        }
        
        ops
    }

    /// 生成混合操作
    fn generate_mixed_ops(count: usize, max_length: usize) -> Vec<OpType> {
        let mut rng = thread_rng();
        let mut ops = Vec::with_capacity(count);
        
        for _ in 0..count {
            let op_type = match rng.gen_range(0..100) {
                0..=60 => OpType::Insert,
                61..=85 => OpType::Delete,
                _ => OpType::Replace,
            };
            
            match op_type {
                OpType::Insert => {
                    let pos = rng.gen_range(0..max_length);
                    let len = rng.gen_range(1..=20);
                    let content: String = (0..len).map(|_| rng.gen_range(b'a'..=b'z') as char).collect();
                    ops.push(OpType::InsertData(pos, content));
                }
                OpType::Delete => {
                    let pos = rng.gen_range(0..max_length);
                    let len = rng.gen_range(1..=10);
                    ops.push(OpType::DeleteData(pos, len));
                }
                OpType::Replace => {
                    let pos = rng.gen_range(0..max_length);
                    let len = rng.gen_range(1..=10);
                    let content: String = (0..len).map(|_| rng.gen_range(b'a'..=b'z') as char).collect();
                    ops.push(OpType::ReplaceData(pos, len, content));
                }
            }
        }
        
        ops
    }

    #[derive(Debug, Clone)]
    enum OpType {
        InsertData(usize, String),
        DeleteData(usize, usize),
        ReplaceData(usize, usize, String),
    }

    // =========================================================================
    // 单个客户端编辑基准测试
    // =========================================================================

    #[test]
    fn benchmark_single_client_inserts() {
        let test_name = "Single Client - 1000 Inserts";
        let node = CrdtNodeId::new("node1".to_string(), 1);
        
        // CRDT 测试
        let start = Instant::now();
        let mut crdt_doc = CrdtDocument::new("test".to_string(), CrdtConfig::default());
        crdt_doc.set_node_id(node.clone());
        
        let inserts = generate_random_inserts(1000, 10000);
        for (pos, content) in inserts {
            let op = CrdtOperation::Insert {
                id: node.clone(),
                position: pos,
                content,
                clock: LogicalClock::new(),
            };
            crdt_doc.apply_local_op(op);
        }
        let crdt_time = start.elapsed();
        
        // OT 测试
        let start = Instant::now();
        let mut ot_bridge = OtCrdtBridge::new(node);
        let version = VectorClock::default();
        
        for (pos, content) in generate_random_inserts(1000, 10000) {
            let op = OtOperation {
                op_type: OtOpType::Insert,
                position: Position::new(0, pos),
                content: Some(content),
                length: None,
                timestamp: 0,
            };
            ot_bridge.client_send(op);
        }
        let ot_time = start.elapsed();
        
        let result = BenchmarkResult {
            test_name: test_name.to_string(),
            crdt_time,
            ot_time,
            crdt_operations: 1000,
            ot_operations: 1000,
            crdt_memory: 0,
            ot_memory: 0,
        };
        
        result.print_summary();
    }

    #[test]
    fn benchmark_single_client_deletes() {
        let test_name = "Single Client - 1000 Deletes";
        let node = CrdtNodeId::new("node1".to_string(), 1);
        
        // CRDT 测试
        let start = Instant::now();
        let mut crdt_doc = CrdtDocument::new("test".to_string(), CrdtConfig::default());
        crdt_doc.set_node_id(node.clone());
        
        // 先插入一些内容
        crdt_doc.apply_local_op(CrdtOperation::Insert {
            id: node.clone(),
            position: 0,
            content: "a".repeat(10000),
            clock: LogicalClock::new(),
        });
        
        let deletes = generate_random_deletes(1000, 10000);
        for (pos, len) in deletes {
            let op = CrdtOperation::Delete {
                id: node.clone(),
                position: pos,
                length: len,
                clock: LogicalClock::new(),
            };
            crdt_doc.apply_remote_op(&op);
        }
        let crdt_time = start.elapsed();
        
        // OT 测试
        let start = Instant::now();
        let mut ot_bridge = OtCrdtBridge::new(node);
        
        for (pos, len) in generate_random_deletes(1000, 10000) {
            let op = OtOperation {
                op_type: OtOpType::Delete,
                position: Position::new(0, pos),
                content: None,
                length: Some(len),
                timestamp: 0,
            };
            ot_bridge.client_send(op);
        }
        let ot_time = start.elapsed();
        
        let result = BenchmarkResult {
            test_name: test_name.to_string(),
            crdt_time,
            ot_time,
            crdt_operations: 1000,
            ot_operations: 1000,
            crdt_memory: 0,
            ot_memory: 0,
        };
        
        result.print_summary();
    }

    // =========================================================================
    // 多客户端并发基准测试
    // =========================================================================

    #[test]
    fn benchmark_multi_client_concurrent() {
        let test_name = "Multi Client - 10 Clients x 100 Ops";
        let node_count = 10;
        let ops_per_node = 100;
        
        // CRDT 测试
        let start = Instant::now();
        let mut docs = Vec::with_capacity(node_count);
        
        for i in 0..node_count {
            let node = CrdtNodeId::new(format!("node{}", i), i as u64);
            let mut doc = CrdtDocument::new("shared".to_string(), CrdtConfig::default());
            doc.set_node_id(node);
            docs.push(doc);
        }
        
        // 每个节点执行操作
        for i in 0..node_count {
            let node = CrdtNodeId::new(format!("node{}", i), i as u64);
            for j in 0..ops_per_node {
                let pos = j * 10 + i;
                let op = CrdtOperation::Insert {
                    id: node.clone(),
                    position: pos,
                    content: format!("{}", i),
                    clock: LogicalClock::new(),
                };
                docs[i].apply_local_op(op);
            }
        }
        
        // 合并所有文档
        let mut merged = docs[0].clone();
        for i in 1..node_count {
            merged.merge(&docs[i].state);
        }
        let crdt_time = start.elapsed();
        
        // OT 测试
        let start = Instant::now();
        let mut bridges = Vec::with_capacity(node_count);
        
        for i in 0..node_count {
            let node = CrdtNodeId::new(format!("node{}", i), i as u64);
            bridges.push(OtCrdtBridge::new(node));
        }
        
        for i in 0..node_count {
            for j in 0..ops_per_node {
                let pos = j * 10 + i;
                let op = OtOperation {
                    op_type: OtOpType::Insert,
                    position: Position::new(0, pos),
                    content: Some(format!("{}", i)),
                    length: None,
                    timestamp: j as i64,
                };
                bridges[i].client_send(op);
            }
        }
        let ot_time = start.elapsed();
        
        let result = BenchmarkResult {
            test_name: test_name.to_string(),
            crdt_time,
            ot_time,
            crdt_operations: node_count * ops_per_node,
            ot_operations: node_count * ops_per_node,
            crdt_memory: 0,
            ot_memory: 0,
        };
        
        result.print_summary();
    }

    // =========================================================================
    // 大规模编辑基准测试
    // =========================================================================

    #[test]
    fn benchmark_large_document() {
        let test_name = "Large Document - 100K Characters";
        let node = CrdtNodeId::new("node1".to_string(), 1);
        
        // 创建大型初始文档
        let initial_content = "a".repeat(100000);
        
        // CRDT 测试
        let start = Instant::now();
        let mut crdt_doc = CrdtDocument::new("large".to_string(), CrdtConfig::default());
        crdt_doc.set_node_id(node.clone());
        
        crdt_doc.apply_local_op(CrdtOperation::Insert {
            id: node.clone(),
            position: 0,
            content: initial_content.clone(),
            clock: LogicalClock::new(),
        });
        
        // 执行 1000 次随机操作
        let ops = generate_mixed_ops(1000, 100000);
        for op in ops {
            match op {
                OpType::InsertData(pos, content) => {
                    let op = CrdtOperation::Insert {
                        id: node.clone(),
                        position: pos,
                        content,
                        clock: LogicalClock::new(),
                    };
                    crdt_doc.apply_local_op(op);
                }
                OpType::DeleteData(pos, len) => {
                    let op = CrdtOperation::Delete {
                        id: node.clone(),
                        position: pos,
                        length: len,
                        clock: LogicalClock::new(),
                    };
                    crdt_doc.apply_remote_op(&op);
                }
                OpType::ReplaceData(pos, len, content) => {
                    let delete_op = CrdtOperation::Delete {
                        id: node.clone(),
                        position: pos,
                        length: len,
                        clock: LogicalClock::new(),
                    };
                    crdt_doc.apply_remote_op(&delete_op);
                    
                    let insert_op = CrdtOperation::Insert {
                        id: node.clone(),
                        position: pos,
                        content,
                        clock: LogicalClock::new(),
                    };
                    crdt_doc.apply_local_op(insert_op);
                }
            }
        }
        let crdt_time = start.elapsed();
        
        // OT 测试
        let start = Instant::now();
        let mut ot_bridge = OtCrdtBridge::new(node);
        
        // OT 不需要初始内容，直接执行操作
        let ops = generate_mixed_ops(1000, 100000);
        for op in ops {
            match op {
                OpType::InsertData(pos, content) => {
                    let op = OtOperation {
                        op_type: OtOpType::Insert,
                        position: Position::new(0, pos),
                        content: Some(content),
                        length: None,
                        timestamp: 0,
                    };
                    ot_bridge.client_send(op);
                }
                OpType::DeleteData(pos, len) => {
                    let op = OtOperation {
                        op_type: OtOpType::Delete,
                        position: Position::new(0, pos),
                        content: None,
                        length: Some(len),
                        timestamp: 0,
                    };
                    ot_bridge.client_send(op);
                }
                OpType::ReplaceData(pos, len, content) => {
                    let delete_op = OtOperation {
                        op_type: OtOpType::Delete,
                        position: Position::new(0, pos),
                        content: None,
                        length: Some(len),
                        timestamp: 0,
                    };
                    ot_bridge.client_send(delete_op);
                    
                    let insert_op = OtOperation {
                        op_type: OtOpType::Insert,
                        position: Position::new(0, pos),
                        content: Some(content),
                        length: None,
                        timestamp: 0,
                    };
                    ot_bridge.client_send(insert_op);
                }
            }
        }
        let ot_time = start.elapsed();
        
        let result = BenchmarkResult {
            test_name: test_name.to_string(),
            crdt_time,
            ot_time,
            crdt_operations: 2000, // 替换算两次操作
            ot_operations: 2000,
            crdt_memory: 0,
            ot_memory: 0,
        };
        
        result.print_summary();
    }

    // =========================================================================
    // 冲突解决基准测试
    // =========================================================================

    #[test]
    fn benchmark_conflict_resolution() {
        let test_name = "Conflict Resolution - 100 Concurrent Conflicts";
        let node1 = CrdtNodeId::new("node1".to_string(), 1);
        let node2 = CrdtNodeId::new("node2".to_string(), 2);
        
        // CRDT 测试
        let start = Instant::now();
        let mut doc1 = CrdtDocument::new("conflict".to_string(), CrdtConfig::default());
        doc1.set_node_id(node1.clone());
        
        let mut doc2 = CrdtDocument::new("conflict".to_string(), CrdtConfig::default());
        doc2.set_node_id(node2.clone());
        
        // 并发编辑相同位置
        for i in 0..100 {
            let pos = i * 5;
            
            doc1.apply_local_op(CrdtOperation::Insert {
                id: node1.clone(),
                position: pos,
                content: format!("A{}", i),
                clock: LogicalClock::new(),
            });
            
            doc2.apply_local_op(CrdtOperation::Insert {
                id: node2.clone(),
                position: pos,
                content: format!("B{}", i),
                clock: LogicalClock::new(),
            });
        }
        
        // 合并
        doc1.merge(&doc2.state);
        let crdt_time = start.elapsed();
        
        // OT 测试
        let start = Instant::now();
        let mut bridge1 = OtCrdtBridge::new(node1);
        let mut bridge2 = OtCrdtBridge::new(node2);
        
        for i in 0..100 {
            let pos = i * 5;
            
            let op1 = OtOperation {
                op_type: OtOpType::Insert,
                position: Position::new(0, pos),
                content: Some(format!("A{}", i)),
                length: None,
                timestamp: i as i64,
            };
            bridge1.client_send(op1);
            
            let op2 = OtOperation {
                op_type: OtOpType::Insert,
                position: Position::new(0, pos),
                content: Some(format!("B{}", i)),
                length: None,
                timestamp: i as i64 + 1000,
            };
            bridge2.client_send(op2);
        }
        let ot_time = start.elapsed();
        
        let result = BenchmarkResult {
            test_name: test_name.to_string(),
            crdt_time,
            ot_time,
            crdt_operations: 200,
            ot_operations: 200,
            crdt_memory: 0,
            ot_memory: 0,
        };
        
        result.print_summary();
    }

    // =========================================================================
    // 序列 CRDT 特定基准测试
    // =========================================================================

    #[test]
    fn benchmark_sequence_crdt_editor() {
        let test_name = "Sequence CRDT Editor - 10K Operations";
        let node = CrdtNodeId::new("node1".to_string(), 1);
        
        let start = Instant::now();
        let mut editor = SequenceCrdtEditor::new("test".to_string());
        editor.set_node_id(node);
        
        // 执行 10000 次操作
        for i in 0..10000 {
            if i % 3 == 0 {
                // 插入
                editor.insert(i * 2, format!("{}", i));
            } else if i % 3 == 1 {
                // 删除
                if editor.len() > 0 {
                    editor.delete(0, 1);
                }
            } else {
                // 继续插入
                editor.insert(editor.len(), "x");
            }
        }
        let elapsed = start.elapsed();
        
        println!("=== {} ===", test_name);
        println!("Time: {:?}", elapsed);
        println!("Final length: {}", editor.len());
        println!();
    }

    // =========================================================================
    // 版本向量基准测试
    // =========================================================================

    #[test]
    fn benchmark_version_vector() {
        let test_name = "Version Vector - 100 Nodes x 1000 Increments";
        
        let start = Instant::now();
        let mut vv = VersionVector::new();
        
        // 模拟 100 个节点各做 1000 次操作
        for i in 0..100 {
            for _ in 0..1000 {
                vv.increment(&format!("node{}", i));
            }
        }
        let increment_time = start.elapsed();
        
        // 测试合并
        let mut vv2 = VersionVector::new();
        for i in 50..150 {
            for _ in 0..500 {
                vv2.increment(&format!("node{}", i));
            }
        }
        
        let start = Instant::now();
        let merged = vv.merge(&vv2);
        let merge_time = start.elapsed();
        
        println!("=== {} ===", test_name);
        println!("Increment time: {:?}", increment_time);
        println!("Merge time: {:?}", merge_time);
        println!("Final size: {} nodes", merged.len());
        println!();
    }

    // =========================================================================
    // 综合基准测试报告
    // =========================================================================

    #[test]
    fn run_all_benchmarks() {
        println!("\n\n");
        println!("========================================");
        println!("CRDT vs OT Performance Benchmark Report");
        println!("========================================");
        println!();
        
        // 运行所有基准测试
        benchmark_single_client_inserts();
        benchmark_single_client_deletes();
        benchmark_multi_client_concurrent();
        benchmark_large_document();
        benchmark_conflict_resolution();
        benchmark_sequence_crdt_editor();
        benchmark_version_vector();
        
        println!("========================================");
        println!("Benchmark Report Complete");
        println!("========================================");
        println!();
    }
}
