//! W3: Tour Builder — 生成引导式导览 (Guided Tour)
//! 移植自: Understand-Anything agents/tour-builder
//! 基于依赖顺序生成层次化代码导览

use std::sync::Arc;
use tokio::sync::RwLock;

use super::{KnowledgeGraph, NodeKind, RelationType};

/// 导览步骤
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TourStep {
    pub order: usize,
    pub title: String,
    pub node_id: String,
    pub file_path: String,
    pub summary: String,
    pub layer: String,
    pub domain: String,
}

/// 生成的导览
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GuidedTour {
    pub title: String,
    pub total_steps: usize,
    pub estimated_reading_time_minutes: usize,
    pub steps: Vec<TourStep>,
}

/// Agent 5: 构建导览 (基于图的拓扑排序)
pub async fn build_tour(graph: &Arc<RwLock<KnowledgeGraph>>) -> Result<GuidedTour, String> {
    let g = graph.read().await;

    if g.nodes.is_empty() {
        return Ok(GuidedTour {
            title: "Empty Project".to_string(),
            total_steps: 0,
            estimated_reading_time_minutes: 0,
            steps: vec![],
        });
    }

    // 拓扑排序: 依赖较少的文件先展示
    let mut dep_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut node_map: std::collections::HashMap<String, &super::KGNode> = std::collections::HashMap::new();

    for node in &g.nodes {
        dep_counts.insert(node.id.clone(), 0);
        node_map.insert(node.id.clone(), node);
    }

    for edge in &g.edges {
        if edge.relation == super::RelationType::Imports || edge.relation == super::RelationType::Calls {
            if let Some(count) = dep_counts.get_mut(&edge.target) {
                *count += 1;
            }
        }
    }

    // 按依赖数排序 (依赖少的先展示 = 基础模块先展示)
    let mut sorted: Vec<(&String, &usize)> = dep_counts.iter().collect();
    sorted.sort_by(|a, b| a.1.cmp(b.1));

    let mut steps = Vec::new();
    for (i, (node_id, _)) in sorted.iter().enumerate() {
        if let Some(node) = node_map.get(*node_id) {
            if node.kind == NodeKind::File {
                steps.push(TourStep {
                    order: i + 1,
                    title: format!("{} ({})", node.name, node.file_path),
                    node_id: node.id.clone(),
                    file_path: node.file_path.clone(),
                    summary: node.summary.clone(),
                    layer: node.architecture_layer.clone().unwrap_or_default(),
                    domain: node.domain.clone().unwrap_or_default(),
                });
            }
        }
    }

    // 分层标题
    let total_steps = steps.len();
    let title = format!("{} — {}/{} steps in knowledge graph",
        g.metadata.project_name, total_steps, g.nodes.len());

    Ok(GuidedTour {
        title,
        total_steps,
        estimated_reading_time_minutes: (total_steps / 10).max(1),
        steps,
    })
}
