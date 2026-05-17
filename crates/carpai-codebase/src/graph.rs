//! 全局知识图谱 (Knowledge Graph) 实现
//!
//! 用于捕捉代码库中的跨文件、跨模块依赖关系。

use std::collections::{HashMap, HashSet};
use serde::{Serialize, Deserialize};

/// 知识图谱节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KGNode {
    pub id: String,          // 唯一标识符 (e.g., "crate::module::FunctionName")
    pub name: String,        // 符号名称
    pub kind: NodeType,      // 类型 (Function, Struct, etc.)
    pub file_path: String,   // 所在文件路径
    pub summary: String,     // AI 生成的功能摘要
}

/// 知识图谱边（关系）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KGEdge {
    pub source: String,      // 源节点 ID
    pub target: String,      // 目标节点 ID
    pub relation: RelationType, // 关系类型 (Calls, Inherits, Uses)
}

/// 节点类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeType {
    Module,
    Function,
    Struct,
    Interface,
    Enum,
    Constant,
}

/// 关系类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RelationType {
    Calls,       // 函数调用
    Inherits,    // 继承/实现
    Uses,        // 使用类型/变量
    Imports,     // 模块导入
}

/// 知识图谱管理器
pub struct KnowledgeGraph {
    nodes: HashMap<String, KGNode>,
    edges: Vec<KGEdge>,
    adjacency_list: HashMap<String, HashSet<String>>, // 用于快速查询邻居
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            adjacency_list: HashMap::new(),
        }
    }

    /// 添加节点
    pub fn add_node(&mut self, node: KGNode) {
        let id = node.id.clone();
        self.nodes.insert(id.clone(), node);
        self.adjacency_list.entry(id).or_insert_with(HashSet::new);
    }

    /// 添加关系
    pub fn add_edge(&mut self, edge: KGEdge) {
        self.adjacency_list.entry(edge.source.clone())
            .or_insert_with(HashSet::new)
            .insert(edge.target.clone());
        self.edges.push(edge);
    }

    /// 查询节点的直接依赖
    pub fn get_dependencies(&self, node_id: &str) -> Vec<&KGNode> {
        if let Some(neighbors) = self.adjacency_list.get(node_id) {
            neighbors.iter()
                .filter_map(|id| self.nodes.get(id))
                .collect()
        } else {
            vec![]
        }
    }

    /// 影响范围分析：找出所有引用了该节点的节点（反向查询）
    pub fn get_impact_analysis(&self, target_id: &str) -> Vec<&KGNode> {
        self.edges.iter()
            .filter(|e| e.target == target_id)
            .filter_map(|e| self.nodes.get(&e.source))
            .collect()
    }

    /// 获取图谱统计信息
    pub fn stats(&self) -> (usize, usize) {
        (self.nodes.len(), self.edges.len())
    }
}
