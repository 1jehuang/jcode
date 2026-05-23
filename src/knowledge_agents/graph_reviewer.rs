//! W3: Graph Reviewer — 校验图完整性与引用一致性
//! 移植自: Understand-Anything agents/graph-reviewer
//! 纯逻辑校验: 引用完整性、节点一致性、结构正确性

use super::{KnowledgeGraph};

/// 校验结果
#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub passed: bool,
    pub total_checks: usize,
    pub passed_checks: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Agent 6: 校验知识图谱的一致性
pub fn review_graph(graph: &KnowledgeGraph) -> Result<ReviewResult, String> {
    let mut result = ReviewResult {
        passed: true,
        total_checks: 0,
        passed_checks: 0,
        warnings: Vec::new(),
        errors: Vec::new(),
    };

    // 构建节点 ID 集合
    let node_ids: std::collections::HashSet<String> = graph.nodes.iter()
        .map(|n| n.id.clone())
        .collect();

    // Check 1: 节点 ID 唯一性
    result.total_checks += 1;
    if node_ids.len() != graph.nodes.len() {
        result.warnings.push(format!(
            "Duplicate node IDs: {} unique but {} total",
            node_ids.len(), graph.nodes.len()
        ));
    } else {
        result.passed_checks += 1;
    }

    // Check 2: 边引用的节点都存在
    result.total_checks += 1;
    for edge in &graph.edges {
        if !node_ids.contains(&edge.source) {
            result.errors.push(format!(
                "Edge references non-existent source node '{}'",
                edge.source
            ));
        }
        if !node_ids.contains(&edge.target) {
            result.errors.push(format!(
                "Edge references non-existent target node '{}'",
                edge.target
            ));
        }
    }
    if result.errors.is_empty() {
        result.passed_checks += 1;
    }

    // Check 3: 节点都有唯一路径
    result.total_checks += 1;
    let mut path_set = std::collections::HashSet::new();
    for node in &graph.nodes {
        if !path_set.insert(node.file_path.clone()) {
            result.warnings.push(format!(
                "Duplicate file path: '{}' (node: {})",
                node.file_path, node.id
            ));
        }
    }
    result.passed_checks += 1;

    // Check 4: 至少有一个节点
    result.total_checks += 1;
    if graph.nodes.is_empty() {
        result.errors.push("Knowledge graph has zero nodes".to_string());
    } else {
        result.passed_checks += 1;
    }

    // Check 5: metadata 非空
    result.total_checks += 1;
    if graph.metadata.project_name.is_empty() {
        result.warnings.push("Project name is empty".to_string());
    }
    if graph.metadata.total_files == 0 && !graph.nodes.is_empty() {
        result.warnings.push("total_files is 0 but nodes exist".to_string());
    }
    result.passed_checks += 1;

    // Check 6: 检测孤立节点 (没有边的节点)
    result.total_checks += 1;
    let connected_nodes: std::collections::HashSet<String> = graph.edges.iter()
        .flat_map(|e| vec![e.source.clone(), e.target.clone()])
        .collect();
    let orphan_count = graph.nodes.iter()
        .filter(|n| !connected_nodes.contains(&n.id))
        .count();
    if orphan_count > 0 {
        result.warnings.push(format!(
            "{} orphan nodes (no incoming or outgoing edges)",
            orphan_count
        ));
    }
    result.passed_checks += 1;

    // Check 7: 检测自环
    result.total_checks += 1;
    let self_loop_count = graph.edges.iter()
        .filter(|e| e.source == e.target)
        .count();
    if self_loop_count > 0 {
        result.warnings.push(format!(
            "{} self-loop edges (source == target)",
            self_loop_count
        ));
    }
    result.passed_checks += 1;

    // 最终判断
    result.passed = result.errors.is_empty();

    Ok(result)
}

/// 格式化校验结果为可读字符串
pub fn format_review(result: &ReviewResult) -> String {
    let status = if result.passed { "✅ PASSED" } else { "❌ FAILED" };
    let mut out = format!(
        "━━━ Graph Review: {} ━━━\n\
         Checks: {}/{} passed\n",
        status, result.passed_checks, result.total_checks
    );

    if !result.errors.is_empty() {
        out.push_str("\nErrors:\n");
        for e in &result.errors {
            out.push_str(&format!("  ❌ {}\n", e));
        }
    }

    if !result.warnings.is_empty() {
        out.push_str("\nWarnings:\n");
        for w in &result.warnings {
            out.push_str(&format!("  ⚠️  {}\n", w));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_agents::{KGNode, KGEdge, KnowledgeGraph, GraphMetadata, NodeKind, RelationType};

    fn make_valid_graph() -> KnowledgeGraph {
        KnowledgeGraph {
            metadata: GraphMetadata {
                project_name: "test".to_string(),
                project_root: "/test".to_string(),
                generated_at: "now".to_string(),
                total_files: 2,
                total_nodes: 2,
                total_edges: 1,
                languages: vec!["Rust".to_string()],
                version: "1.0".to_string(),
            },
            nodes: vec![
                KGNode {
                    id: "n1".to_string(), name: "main".to_string(),
                    kind: NodeKind::File, file_path: "src/main.rs".to_string(),
                    line: 0, column: 0, summary: "".to_string(),
                    architecture_layer: None, domain: None, complexity: None,
                },
                KGNode {
                    id: "n2".to_string(), name: "lib".to_string(),
                    kind: NodeKind::File, file_path: "src/lib.rs".to_string(),
                    line: 0, column: 0, summary: "".to_string(),
                    architecture_layer: None, domain: None, complexity: None,
                },
            ],
            edges: vec![
                KGEdge { source: "n1".to_string(), target: "n2".to_string(),
                    relation: RelationType::Imports, weight: 1.0 },
            ],
        }
    }

    #[test]
    fn test_valid_graph_passes() {
        let graph = make_valid_graph();
        let result = review_graph(&graph).unwrap();
        assert!(result.passed);
        assert_eq!(result.errors.len(), 0);
    }

    #[test]
    fn test_orphan_nodes_detected() {
        let mut graph = make_valid_graph();
        graph.nodes.push(KGNode {
            id: "orphan".to_string(), name: "orphan".to_string(),
            kind: NodeKind::File, file_path: "orphan.rs".to_string(),
            line: 0, column: 0, summary: "".to_string(),
            architecture_layer: None, domain: None, complexity: None,
        });
        let result = review_graph(&graph).unwrap();
        assert!(result.warnings.iter().any(|w| w.contains("orphan")));
    }

    #[test]
    fn test_broken_edge_detected() {
        let mut graph = make_valid_graph();
        graph.edges.push(KGEdge {
            source: "ghost".to_string(), target: "n1".to_string(),
            relation: RelationType::Calls, weight: 1.0,
        });
        let result = review_graph(&graph).unwrap();
        assert!(!result.passed);
        assert!(result.errors.iter().any(|e| e.contains("ghost")));
    }

    #[test]
    fn test_self_loop_detected() {
        let mut graph = make_valid_graph();
        graph.edges.push(KGEdge {
            source: "n1".to_string(), target: "n1".to_string(),
            relation: RelationType::References, weight: 1.0,
        });
        let result = review_graph(&graph).unwrap();
        assert!(result.warnings.iter().any(|w| w.contains("self-loop")));
    }

    #[test]
    fn test_empty_graph() {
        let graph = KnowledgeGraph {
            metadata: GraphMetadata {
                project_name: "".to_string(), project_root: "".to_string(),
                generated_at: "".to_string(), total_files: 0, total_nodes: 0,
                total_edges: 0, languages: vec![], version: "".to_string(),
            },
            nodes: vec![], edges: vec![],
        };
        let result = review_graph(&graph).unwrap();
        assert!(result.errors.iter().any(|e| e.contains("zero nodes")));
    }
}
