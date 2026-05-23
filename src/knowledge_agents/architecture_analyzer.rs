//! W2: Architecture Analyzer — 识别架构层并着色
//! 移植自: Understand-Anything agents/architecture-analyzer
//! 基于目录结构和文件命名约定识别架构层

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::file_analyzer::FileAnalysis;
use super::{ArchitectureLayer, KGNode, KnowledgeGraph, NodeKind};

/// 架构层识别规则
struct LayerRule {
    /// 匹配的目录名
    dirs: &'static [&'static str],
    /// 匹配的文件名模式
    file_patterns: &'static [&'static str],
    /// 对应的架构层
    layer: ArchitectureLayer,
}

static LAYER_RULES: [LayerRule; 9] = [
    LayerRule { dirs: &["api", "routes", "endpoints", "controllers", "handlers"],
        file_patterns: &["api", "route", "endpoint", "controller", "handler"], layer: ArchitectureLayer::Api },
    LayerRule { dirs: &["service", "services", "use-cases", "usecases"],
        file_patterns: &["service", "use_case", "usecase"], layer: ArchitectureLayer::Service },
    LayerRule { dirs: &["business", "domain", "model", "models", "entity", "entities"],
        file_patterns: &["domain", "model", "entity", "business"], layer: ArchitectureLayer::Business },
    LayerRule { dirs: &["data", "repository", "repositories", "dao", "persistence", "db", "database", "sql"],
        file_patterns: &["repo", "repository", "dao", "data", "db", "database", "sql"], layer: ArchitectureLayer::Data },
    LayerRule { dirs: &["infra", "infrastructure", "config", "configuration", "deploy", "deployment", "k8s", "docker"],
        file_patterns: &["infra", "config", "deploy", "k8s"], layer: ArchitectureLayer::Infrastructure },
    LayerRule { dirs: &["ui", "components", "pages", "views", "screens", "widgets", "templates"],
        file_patterns: &["ui", "component", "page", "view", "screen", "widget"], layer: ArchitectureLayer::Ui },
    LayerRule { dirs: &["utils", "util", "helpers", "helper", "common", "shared", "lib"],
        file_patterns: &["util", "helper", "common", "shared"], layer: ArchitectureLayer::Utility },
    LayerRule { dirs: &["test", "tests", "spec", "specs", "__tests__", "__test__"],
        file_patterns: &["test", "spec", "mock", "stub", "fixture"], layer: ArchitectureLayer::Testing },
    LayerRule { dirs: &["config", "configuration", "settings", "env"],
        file_patterns: &["config", "setting", "env"], layer: ArchitectureLayer::Config },
];

/// 识别文件的架构层
pub fn detect_layer(file_path: &str) -> ArchitectureLayer {
    let path = file_path.to_lowercase();
    let path_parts: Vec<&str> = path.split(&['/', '\\'][..]).collect();
    let filename = path_parts.last().unwrap_or(&"");

    for rule in LAYER_RULES {
        // 检查目录
        for dir in &rule.dirs {
            if path_parts.iter().any(|p| *p == *dir) {
                return rule.layer.clone();
            }
        }
        // 检查文件名模式
        for pat in &rule.file_patterns {
            if filename.contains(pat) {
                return rule.layer.clone();
            }
        }
    }

    ArchitectureLayer::Unknown
}

/// Agent 3: 分析架构层 (对知识图谱中所有节点进行着色)
pub async fn analyze_architecture(
    _root: &Path,
    _analysis: &[FileAnalysis],
    graph: &Arc<RwLock<KnowledgeGraph>>,
) -> Result<(), String> {
    let mut g = graph.write().await;

    for node in &mut g.nodes {
        let layer = detect_layer(&node.file_path);
        node.architecture_layer = Some(format!("{:?}", layer));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_layer() {
        assert_eq!(detect_layer("src/api/users.rs"), ArchitectureLayer::Api);
        assert_eq!(detect_layer("src/services/billing.rs"), ArchitectureLayer::Service);
        assert_eq!(detect_layer("src/models/user.rs"), ArchitectureLayer::Business);
        assert_eq!(detect_layer("src/db/repository.rs"), ArchitectureLayer::Data);
        assert_eq!(detect_layer("k8s/deployment.yaml"), ArchitectureLayer::Infrastructure);
        assert_eq!(detect_layer("src/components/Button.tsx"), ArchitectureLayer::Ui);
        assert_eq!(detect_layer("src/utils/helpers.ts"), ArchitectureLayer::Utility);
        assert_eq!(detect_layer("tests/integration_test.rs"), ArchitectureLayer::Testing);
        assert_eq!(detect_layer("config/settings.toml"), ArchitectureLayer::Config);
        assert_eq!(detect_layer("src/unknown_module/lib.rs"), ArchitectureLayer::Unknown);
    }
}
