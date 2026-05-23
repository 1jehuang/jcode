//! W3: Knowledge Graph JSON 输出 + 增量更新
//! 移植自: Understand-Anything knowledge-graph.json 格式规范
//!
//! 输出格式与 Understand-Anything 兼容:
//! - knowledge-graph.json: 主图谱 (可 commit 到仓库)
//! - .understand-anything/ 目录: 增量缓存 + 变更追踪

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::{KGEdge, KGNode, KnowledgeGraph, PipelineConfig};

/// 变更追踪: 记录哪些文件已变更
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChangeTracker {
    pub file_hashes: HashMap<String, String>,  // file_path → sha256
    pub last_updated: String,
}

/// 增量更新引擎
pub struct IncrementalEngine {
    tracker_path: std::path::PathBuf,
    tracker: Arc<RwLock<ChangeTracker>>,
}

impl IncrementalEngine {
    pub fn new(root: &Path) -> Self {
        let tracker_path = root.join(".understand-anything").join("change-tracker.json");
        let tracker = if tracker_path.exists() {
            std::fs::read_to_string(&tracker_path)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or(ChangeTracker {
                    file_hashes: HashMap::new(),
                    last_updated: String::new(),
                })
        } else {
            ChangeTracker {
                file_hashes: HashMap::new(),
                last_updated: String::new(),
            }
        };

        Self {
            tracker_path,
            tracker: Arc::new(RwLock::new(tracker)),
        }
    }

    /// 检测变更的文件 (返回变更文件列表)
    pub async fn detect_changes(&self, root: &Path) -> Result<Vec<String>, String> {
        let mut changed = Vec::new();
        let mut tracker = self.tracker.write().await;

        // 扫描所有文件
        let scanner = super::project_scanner::scan_project(root, &PipelineConfig::default()).await?;

        for file in &scanner {
            let full_path = root.join(&file.path);
            let current_hash = compute_file_hash(&full_path).await;

            let old_hash = tracker.file_hashes.get(&file.path);
            match (current_hash, old_hash) {
                (Some(hash), Some(old)) if &hash == old => {} // 未变更
                (Some(hash), _) => {
                    changed.push(file.path.clone());
                    tracker.file_hashes.insert(file.path.clone(), hash);
                }
                _ => {} // 无法读取的文件跳过
            }
        }

        // 检测已删除的文件
        let existing: std::collections::HashSet<String> = scanner.iter()
            .map(|f| f.path.clone()).collect();
        tracker.file_hashes.retain(|k, _| existing.contains(k));

        Ok(changed)
    }

    /// 保存变更追踪状态
    pub async fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.tracker_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| format!("Dir creation: {}", e))?;
        }
        let data = serde_json::to_string_pretty(&*self.tracker.read().await)
            .map_err(|e| format!("Serialization: {}", e))?;
        tokio::fs::write(&self.tracker_path, &data).await
            .map_err(|e| format!("Write: {}", e))?;
        Ok(())
    }

    /// 获取统计
    pub async fn stats(&self) -> String {
        let t = self.tracker.read().await;
        format!("Tracked files: {}, Last updated: {}", t.file_hashes.len(), t.last_updated)
    }
}

/// 计算文件 SHA-256 哈希
async fn compute_file_hash(path: &Path) -> Option<String> {
    let content = tokio::fs::read(path).await.ok()?;
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let result = hasher.finalize();
    Some(format!("{:x}", result))
}

/// 导出知识图谱为多种格式
pub mod export {
    use super::*;

    /// 导出为 JSON (Understand-Anything 兼容格式)
    pub async fn to_json(graph: &KnowledgeGraph, output_path: &Path) -> Result<(), String> {
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| format!("Dir creation: {}", e))?;
        }
        let json = serde_json::to_string_pretty(graph)
            .map_err(|e| format!("JSON serialization: {}", e))?;
        tokio::fs::write(output_path, &json).await
            .map_err(|e| format!("Write: {}", e))?;
        Ok(())
    }

    /// 导出为 Markdown 格式 (人类可读)
    pub fn to_markdown(graph: &KnowledgeGraph) -> String {
        let mut md = String::new();
        md.push_str(&format!("# {} Knowledge Graph\n\n", graph.metadata.project_name));
        md.push_str(&format!("> Generated at: {}\n", graph.metadata.generated_at));
        md.push_str(&format!("> Files: {} | Nodes: {} | Edges: {}\n\n",
            graph.metadata.total_files, graph.metadata.total_nodes, graph.metadata.total_edges));

        md.push_str("## Architecture Layers\n\n");
        md.push_str("| Layer | Files |\n|-------|-------|\n");
        let mut layers: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for node in &graph.nodes {
            let layer = node.architecture_layer.as_deref().unwrap_or("Unknown");
            *layers.entry(layer.to_string()).or_insert(0) += 1;
        }
        let mut layers_sorted: Vec<_> = layers.into_iter().collect();
        layers_sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (layer, count) in &layers_sorted {
            md.push_str(&format!("| {} | {} |\n", layer, count));
        }

        md.push_str("\n## Domains\n\n");
        md.push_str("| Domain | Files |\n|--------|-------|\n");
        let mut domains: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for node in &graph.nodes {
            if let Some(ref domain) = node.domain {
                *domains.entry(domain.clone()).or_insert(0) += 1;
            }
        }
        let mut domains_sorted: Vec<_> = domains.into_iter().collect();
        domains_sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (domain, count) in &domains_sorted {
            md.push_str(&format!("| {} | {} |\n", domain, count));
        }

        md.push_str("\n## Files\n\n");
        md.push_str("| # | File | Layer | Domain |\n|---|------|-------|--------|\n");
        for (i, node) in graph.nodes.iter().enumerate() {
            let layer = node.architecture_layer.as_deref().unwrap_or("-");
            let domain = node.domain.as_deref().unwrap_or("-");
            md.push_str(&format!("| {} | `{}` | {} | {} |\n", i + 1, node.file_path, layer, domain));
        }

        md.push_str("\n## Dependencies\n\n");
        for edge in &graph.edges {
            md.push_str(&format!("- `{}` → `{}` ({:?})\n",
                edge.source, edge.target, edge.relation));
        }

        md
    }

    /// 导出为 Mermaid 图
    pub fn to_mermaid(graph: &KnowledgeGraph) -> String {
        let mut mermaid = String::new();
        mermaid.push_str("```mermaid\n");
        mermaid.push_str("graph LR\n");
        mermaid.push_str(&format!("    %% Knowledge Graph: {}\n", graph.metadata.project_name));

        // 按架构层分组着色
        for node in &graph.nodes {
            let layer = node.architecture_layer.as_deref().unwrap_or("Unknown");
            let label = node.name.chars().take(20).collect::<String>();
            mermaid.push_str(&format!("    {}[\"{}\"]:::{}Style\n", node.id, label, layer));
        }

        mermaid.push('\n');
        for edge in &graph.edges {
            mermaid.push_str(&format!("    {} -->|{:?}| {}\n", edge.source, edge.relation, edge.target));
        }

        mermaid.push('\n');
        for node in &graph.nodes {
            let layer = node.architecture_layer.as_deref().unwrap_or("Unknown");
            mermaid.push_str(&format!("    classDef {}Style {}\n", layer, style_for_layer(layer)));
        }

        mermaid.push_str("```\n");
        mermaid
    }

    fn style_for_layer(layer: &str) -> &'static str {
        match layer {
            "Api" => "fill:#e1f5fe,stroke:#0288d1",
            "Service" => "fill:#e8f5e9,stroke:#388e3c",
            "Business" => "fill:#fff3e0,stroke:#f57c00",
            "Data" => "fill:#fce4ec,stroke:#c62828",
            "Infrastructure" => "fill:#f3e5f5,stroke:#7b1fa2",
            "Ui" => "fill:#e0f7fa,stroke:#00838f",
            _ => "fill:#f5f5f5,stroke:#616161",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_agents::{KnowledgeGraph, GraphMetadata, KGNode, KGEdge, NodeKind, RelationType};

    fn make_test_graph() -> KnowledgeGraph {
        KnowledgeGraph {
            metadata: GraphMetadata {
                project_name: "test".to_string(),
                project_root: "/test".to_string(),
                generated_at: "now".to_string(),
                total_files: 2, total_nodes: 2, total_edges: 1,
                languages: vec!["Rust".to_string()],
                version: "1.0".to_string(),
            },
            nodes: vec![
                KGNode {
                    id: "file::src/main.rs".to_string(), name: "main".to_string(),
                    kind: NodeKind::File, file_path: "src/main.rs".to_string(),
                    line: 0, column: 0, summary: "Entry point".to_string(),
                    architecture_layer: Some("Api".to_string()),
                    domain: Some("Admin".to_string()),
                    complexity: None,
                },
                KGNode {
                    id: "file::src/lib.rs".to_string(), name: "lib".to_string(),
                    kind: NodeKind::File, file_path: "src/lib.rs".to_string(),
                    line: 0, column: 0, summary: "Library".to_string(),
                    architecture_layer: Some("Service".to_string()),
                    domain: None,
                    complexity: None,
                },
            ],
            edges: vec![
                KGEdge { source: "file::src/main.rs".to_string(), target: "file::src/lib.rs".to_string(),
                    relation: RelationType::Imports, weight: 1.0 },
            ],
        }
    }

    #[test]
    fn test_to_markdown() {
        let graph = make_test_graph();
        let md = export::to_markdown(&graph);
        assert!(md.contains("Knowledge Graph"));
        assert!(md.contains("main"));
        assert!(md.contains("lib"));
    }

    #[test]
    fn test_to_mermaid() {
        let graph = make_test_graph();
        let mermaid = export::to_mermaid(&graph);
        assert!(mermaid.contains("mermaid"));
        assert!(mermaid.contains("graph LR"));
    }
}
