//! # Knowledge Graph — 代码知识图谱（借鉴 Understand-Anything 多智能体管道）
//!
//! 将代码库解析为交互式知识图谱，支持：
//! - 文件/函数/类节点 + 导入/调用/继承边
//! - 架构分层标注 (API/Service/Data/UI/Utility)
//! - 影响链追踪
//! - JSON 图谱可提交到 Git 团队共享
//!
//! 与 CarpAI 现有 AST/parser/completion_engine 配合使用。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 图谱节点类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeType {
    File,
    Function,
    Struct,
    Enum,
    Trait,
    Module,
    Directory,
}

/// 图谱边类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EdgeType {
    Imports,
    Calls,
    Defines,
    Implements,
    Extends,
    Contains,
}

/// 架构层级
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArchitectureLayer {
    Api,
    Service,
    Data,
    Ui,
    Utility,
    Config,
    Unknown,
}

/// 图谱节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub node_type: NodeType,
    pub file_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub layer: ArchitectureLayer,
    pub summary: Option<String>,
    pub doc_comment: Option<String>,
}

/// 图谱边
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub edge_type: EdgeType,
    pub weight: usize,
}

/// 知识图谱
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeGraph {
    pub project_name: String,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub layers: HashMap<String, Vec<String>>,
}

impl KnowledgeGraph {
    pub fn new(project_name: &str) -> Self {
        Self {
            project_name: project_name.to_string(),
            nodes: Vec::new(),
            edges: Vec::new(),
            layers: HashMap::new(),
        }
    }

    /// 添加节点
    pub fn add_node(&mut self, node: GraphNode) {
        let layer_name = format!("{:?}", node.layer);
        self.layers.entry(layer_name).or_default().push(node.id.clone());
        self.nodes.push(node);
    }

    /// 添加边
    pub fn add_edge(&mut self, source: &str, target: &str, edge_type: EdgeType) {
        self.edges.push(GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            edge_type,
            weight: 1,
        });
    }

    /// 查找受某个节点影响的所有下游节点
    pub fn find_downstream(&self, node_id: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = vec![node_id.to_string()];
        while let Some(current) = queue.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            for edge in &self.edges {
                if edge.source == current {
                    result.push(edge.target.clone());
                    queue.push(edge.target.clone());
                }
            }
        }
        result
    }

    /// 查找某个节点的上游依赖
    pub fn find_upstream(&self, node_id: &str) -> Vec<String> {
        let mut result = Vec::new();
        for edge in &self.edges {
            if edge.target == node_id {
                result.push(edge.source.clone());
            }
        }
        result
    }

    /// 导出为 JSON（可提交到 Git 团队共享）
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// 从 JSON 加载
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// 生成 Mermaid 图（供 LLM/TUI 展示）
    pub fn to_mermaid(&self) -> String {
        let mut mermaid = String::from("graph TD\n");
        for node in &self.nodes {
            let layer_style = match node.layer {
                ArchitectureLayer::Api => ":::api",
                ArchitectureLayer::Service => ":::service",
                ArchitectureLayer::Data => ":::data",
                ArchitectureLayer::Ui => ":::ui",
                ArchitectureLayer::Utility => ":::util",
                _ => "",
            };
            mermaid.push_str(&format!("    {}[\"{}\"]{}\n", node.id, node.name, layer_style));
        }
        mermaid.push_str("\n");
        for edge in &self.edges {
            let style = match edge.edge_type {
                EdgeType::Imports => "-..->",
                EdgeType::Calls => "-->",
                EdgeType::Defines => "==>",
                _ => "-->",
            };
            mermaid.push_str(&format!("    {} {} {}\n", edge.source, style, edge.target));
        }
        mermaid
    }
}

/// 架构层检测
pub fn detect_layer(file_path: &str) -> ArchitectureLayer {
    let lower = file_path.to_lowercase();
    if lower.contains("/api/") || lower.contains("/routes/") || lower.contains("/handler") {
        ArchitectureLayer::Api
    } else if lower.contains("/service") || lower.contains("/domain/") || lower.contains("/core/") {
        ArchitectureLayer::Service
    } else if lower.contains("/model") || lower.contains("/entity") || lower.contains("/db/") || lower.contains("/repository") {
        ArchitectureLayer::Data
    } else if lower.contains("/ui/") || lower.contains("/view/") || lower.contains("/component") || lower.contains("/page") {
        ArchitectureLayer::Ui
    } else if lower.contains("/util") || lower.contains("/helper") || lower.contains("/common") || lower.contains("/lib/") {
        ArchitectureLayer::Utility
    } else if lower.contains("config") || lower.contains("setting") {
        ArchitectureLayer::Config
    } else {
        ArchitectureLayer::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_basics() {
        let mut g = KnowledgeGraph::new("test-project");
        g.add_node(GraphNode {
            id: "auth.rs".into(), name: "auth".into(), node_type: NodeType::File,
            file_path: "src/auth.rs".into(), line_start: 1, line_end: 100,
            layer: ArchitectureLayer::Api, summary: None, doc_comment: None,
        });
        g.add_node(GraphNode {
            id: "user.rs".into(), name: "user".into(), node_type: NodeType::File,
            file_path: "src/user.rs".into(), line_start: 1, line_end: 80,
            layer: ArchitectureLayer::Data, summary: None, doc_comment: None,
        });
        g.add_edge("auth.rs", "user.rs", EdgeType::Calls);
        assert_eq!(g.find_downstream("auth.rs").len(), 1);
        assert_eq!(g.find_upstream("user.rs").len(), 1);
    }

    #[test]
    fn test_layer_detection() {
        assert_eq!(detect_layer("src/api/user_handler.rs"), ArchitectureLayer::Api);
        assert_eq!(detect_layer("src/service/auth_service.rs"), ArchitectureLayer::Service);
        assert_eq!(detect_layer("src/model/user.rs"), ArchitectureLayer::Data);
        assert_eq!(detect_layer("src/ui/components/Button.tsx"), ArchitectureLayer::Ui);
        assert_eq!(detect_layer("src/util/helper.rs"), ArchitectureLayer::Utility);
    }

    #[test]
    fn test_mermaid_output() {
        let mut g = KnowledgeGraph::new("test");
        g.add_node(GraphNode {
            id: "a".into(), name: "A".into(), node_type: NodeType::File,
            file_path: "a.rs".into(), line_start: 1, line_end: 10,
            layer: ArchitectureLayer::Api, summary: None, doc_comment: None,
        });
        g.add_edge("a", "b", EdgeType::Calls);
        let mermaid = g.to_mermaid();
        assert!(mermaid.contains("graph TD"));
        assert!(mermaid.contains("a --> b"));
    }
}
