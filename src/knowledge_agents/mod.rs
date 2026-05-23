//! Understand-Anything 多Agent流水线深度移植
//!
//! 核心架构: 确定性解析器 + LLM Agent 语义层
//! 源项目: https://github.com/Lum1104/Understand-Anything (Apache-2.0)
//!
//! Agent 流水线 (7个):
//!   1. project-scanner  → 扫描项目文件，识别语言/框架
//!   2. file-analyzer    → 分析每个文件，抽取符号/依赖 (并行)
//!   3. architecture-analyzer → 识别架构层并进行着色
//!   4. domain-analyzer  → 将代码映射到业务域
//!   5. tour-builder     → 生成引导式导览
//!   6. graph-reviewer   → 校验图完整性与引用一致性
//!   7. article-analyzer → 分析知识库/文档wiki
//!
//! 增量更新: 仅重分析变更文件，避免全量重跑

pub mod project_scanner;
pub mod file_analyzer;
pub mod architecture_analyzer;
pub mod domain_analyzer;
pub mod tour_builder;
pub mod graph_reviewer;
pub mod knowledge_graph;
pub mod article_analyzer;

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// 7-Agent 流水线控制器
pub struct KnowledgePipeline {
    pub config: PipelineConfig,
    pub graph: Arc<RwLock<KnowledgeGraph>>,
    pub stats: Arc<RwLock<PipelineStats>>,
}

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub max_concurrent_files: usize,
    pub batch_size: usize,
    pub enable_incremental: bool,
    pub output_path: std::path::PathBuf,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_concurrent_files: 5,
            batch_size: 20,
            enable_incremental: true,
            output_path: Path::new(".understand-anything").join("knowledge-graph.json"),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct PipelineStats {
    pub files_scanned: u64,
    pub files_changed: u64,
    pub nodes_created: u64,
    pub edges_created: u64,
    pub total_duration_ms: u64,
    pub agents_executed: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeGraph {
    pub metadata: GraphMetadata,
    pub nodes: Vec<KGNode>,
    pub edges: Vec<KGEdge>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphMetadata {
    pub project_name: String,
    pub project_root: String,
    pub generated_at: String,
    pub total_files: usize,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub languages: Vec<String>,
    pub version: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KGNode {
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub summary: String,
    pub architecture_layer: Option<String>,
    pub domain: Option<String>,
    pub complexity: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KGEdge {
    pub source: String,
    pub target: String,
    pub relation: RelationType,
    pub weight: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum NodeKind {
    File,
    Module,
    Namespace,
    Package,
    Function,
    Method,
    Struct,
    Class,
    Interface,
    Enum,
    Trait,
    Constant,
    Type,
    Macro,
    Component,
    Service,
    Database,
    Route,
    Config,
    Documentation,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum RelationType {
    Calls,
    Imports,
    Extends,
    Implements,
    Contains,
    Defines,
    Uses,
    Inherits,
    DependsOn,
    References,
    RoutesTo,
    DeploysTo,
    Configures,
    Documents,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum ArchitectureLayer {
    Api,
    Service,
    Business,
    Data,
    Infrastructure,
    Ui,
    Utility,
    Config,
    Testing,
    Unknown,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum ComplexityLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl KnowledgePipeline {
    pub fn new(config: PipelineConfig) -> Self {
        Self {
            config,
            graph: Arc::new(RwLock::new(KnowledgeGraph {
                metadata: GraphMetadata {
                    project_name: String::new(),
                    project_root: String::new(),
                    generated_at: String::new(),
                    total_files: 0,
                    total_nodes: 0,
                    total_edges: 0,
                    languages: vec![],
                    version: "1.0".to_string(),
                },
                nodes: vec![],
                edges: vec![],
            })),
            stats: Arc::new(RwLock::new(PipelineStats::default())),
        }
    }

    /// 运行完整 7-Agent 流水线
    pub async fn run_pipeline(&self, root: &Path, project_name: &str) -> Result<KnowledgeGraph, String> {
        let start = SystemTime::now();

        // Agent 1: 扫描项目文件
        let files = project_scanner::scan_project(root, &self.config).await
            .map_err(|e| format!("Project scan failed: {}", e))?;
        self.stats.write().await.files_scanned = files.len() as u64;
        self.stats.write().await.agents_executed += 1;

        // Agent 2: 并行分析文件 (最多5并发, 每批20-30文件)
        let analysis_results = file_analyzer::analyze_files(root, &files, self.config.max_concurrent_files).await
            .map_err(|e| format!("File analysis failed: {}", e))?;
        self.stats.write().await.agents_executed += 1;

        // 构建图: 节点 + 边
        let mut graph = self.graph.write().await;
        graph.metadata.project_name = project_name.to_string();
        graph.metadata.project_root = root.to_string_lossy().to_string();
        graph.metadata.generated_at = format!("{:?}", start);

        let mut all_languages = std::collections::HashSet::new();

        for result in &analysis_results {
            all_languages.insert(result.language.clone());
            graph.nodes.push(KGNode {
                id: result.node_id.clone(),
                name: result.symbol_name.clone(),
                kind: NodeKind::File,
                file_path: result.file_path.clone(),
                line: 0, column: 0,
                summary: result.summary.clone(),
                architecture_layer: None,
                domain: None,
                complexity: Some(format!("{:?}", ComplexityLevel::Low)),
            });
            self.stats.write().await.nodes_created += 1;

            for dep in &result.dependencies {
                graph.edges.push(KGEdge {
                    source: result.node_id.clone(),
                    target: dep.clone(),
                    relation: RelationType::Imports,
                    weight: 1.0,
                });
                self.stats.write().await.edges_created += 1;
            }
        }
        graph.metadata.languages = all_languages.into_iter().collect();
        drop(graph);

        // Agent 3: 架构层分析
        architecture_analyzer::analyze_architecture(root, &analysis_results, &self.graph).await
            .map_err(|e| format!("Architecture analysis failed: {}", e))?;
        self.stats.write().await.agents_executed += 1;

        // Agent 4: 业务域分析
        domain_analyzer::analyze_domains(&analysis_results, &self.graph).await
            .map_err(|e| format!("Domain analysis failed: {}", e))?;
        self.stats.write().await.agents_executed += 1;

        // Agent 5: 生成导览
        tour_builder::build_tour(&self.graph).await
            .map_err(|e| format!("Tour building failed: {}", e))?;
        self.stats.write().await.agents_executed += 1;

        // Agent 6: 图一致性校验
        let graph_clone = self.graph.read().await.clone();
        graph_reviewer::review_graph(&graph_clone)?;
        self.stats.write().await.agents_executed += 1;

        // 输出 JSON
        let final_graph = self.graph.read().await.clone();
        let elapsed = start.elapsed().unwrap_or_default();
        self.stats.write().await.total_duration_ms = elapsed.as_millis() as u64;

        // 写入文件
        let json = serde_json::to_string_pretty(&final_graph)
            .map_err(|e| format!("JSON serialization failed: {}", e))?;
        if let Some(parent) = self.config.output_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| format!("Output dir creation failed: {}", e))?;
        }
        tokio::fs::write(&self.config.output_path, &json).await
            .map_err(|e| format!("Output write failed: {}", e))?;

        Ok(final_graph)
    }

    /// 增量更新: 仅分析变更文件
    pub async fn incremental_update(&self, root: &Path, changed_files: &[String]) -> Result<KnowledgeGraph, String> {
        let start = SystemTime::now();

        // 只分析变更文件
        let analysis_results = file_analyzer::analyze_files(root, changed_files, self.config.max_concurrent_files).await
            .map_err(|e| format!("Incremental file analysis failed: {}", e))?;

        let mut graph = self.graph.write().await;

        for result in &analysis_results {
            // 删除旧节点 (如果存在)
            graph.nodes.retain(|n| n.id != result.node_id);
            graph.edges.retain(|e| e.source != result.node_id && e.target != result.node_id);

            // 添加新节点
            graph.nodes.push(KGNode {
                id: result.node_id.clone(),
                name: result.symbol_name.clone(),
                kind: NodeKind::File,
                file_path: result.file_path.clone(),
                line: 0, column: 0,
                summary: result.summary.clone(),
                architecture_layer: None,
                domain: None,
                complexity: None,
            });

            for dep in &result.dependencies {
                graph.edges.push(KGEdge {
                    source: result.node_id.clone(),
                    target: dep.clone(),
                    relation: RelationType::Imports,
                    weight: 1.0,
                });
            }
        }

        graph.metadata.generated_at = format!("{:?}", start);
        graph.metadata.total_files = graph.nodes.len();
        graph.metadata.total_nodes = graph.nodes.len();
        graph.metadata.total_edges = graph.edges.len();
        drop(graph);

        // 写入文件
        let final_graph = self.graph.read().await.clone();
        let json = serde_json::to_string_pretty(&final_graph)
            .map_err(|e| format!("JSON serialization failed: {}", e))?;
        tokio::fs::write(&self.config.output_path, &json).await
            .map_err(|e| format!("Output write failed: {}", e))?;

        self.stats.write().await.files_changed = analysis_results.len() as u64;

        Ok(final_graph)
    }

    /// 获取流水线统计
    pub async fn stats(&self) -> String {
        let s = self.stats.read().await;
        format!(
            "━━━ Knowledge Pipeline Stats ━━━\n\
             Files scanned:    {}\n\
             Files changed:    {} (incremental)\n\
             Nodes created:    {}\n\
             Edges created:    {}\n\
             Agents executed:  {}/7\n\
             Total duration:   {}ms\n\
             Output:           {}",
            s.files_scanned, s.files_changed, s.nodes_created, s.edges_created,
            s.agents_executed, s.total_duration_ms,
            self.config.output_path.display(),
        )
    }
}
