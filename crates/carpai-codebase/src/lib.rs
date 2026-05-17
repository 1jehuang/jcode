//! # CarpAI Codebase Intelligence Engine
//!
//! 负责代码库的深度语义理解，包含：
//! 1. **AST Parsing**: 基于 tree-sitter 的多语言语法树解析
//! 2. **Semantic Indexing**: 基于 tantivy 的代码片段检索引擎
//! 3. **Symbol Extraction**: 自动提取类、函数、接口等关键符号

pub mod parser;
pub mod indexer;
pub mod symbols;
pub mod graph;

use anyhow::Result;
use std::path::PathBuf;

/// 代码库智能引擎主入口
pub struct CodebaseEngine {
    parser: parser::CodeParser,
    indexer: indexer::SemanticIndexer,
    graph: graph::KnowledgeGraph,
}

impl CodebaseEngine {
    pub fn new(index_path: PathBuf) -> Result<Self> {
        Ok(Self {
            parser: parser::CodeParser::new()?,
            indexer: indexer::SemanticIndexer::new(index_path)?,
            graph: graph::KnowledgeGraph::new(),
        })
    }

    /// 索引整个工作区
    pub async fn index_workspace(&mut self, workspace_path: &str) -> Result<()> {
        tracing::info!("🔍 开始索引工作区: {}", workspace_path);
        
        let mut count = 0;
        let entries: Vec<_> = walkdir::WalkDir::new(workspace_path)
            .into_iter()
            .filter_entry(|e| {
                let path = e.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.') || name == "target" || name == "node_modules" {
                        return false;
                    }
                }
                true
            })
            .filter_map(|e| e.ok())
            .collect();

        for entry in entries {
            if entry.file_type().is_file() {
                if let Some(path) = entry.path().to_str() {
                    if let Err(e) = self.index_file(path).await {
                        tracing::warn!("索引文件失败 {}: {:?}", path, e);
                    } else {
                        count += 1;
                    }
                }
            }
        }
        
        tracing::info!("✅ 工作区索引完成，共处理 {} 个文件", count);
        Ok(())
    }

    /// 索引单个文件
    pub async fn index_file(&mut self, file_path: &str) -> Result<()> {
        let content = std::fs::read_to_string(file_path)?;
        let symbols = self.parser.extract_symbols(file_path, &content)?;
        self.indexer.add_document(file_path, &symbols, &content).await?;
        Ok(())
    }

    /// 语义搜索代码
    pub async fn search_code(&self, query: &str, limit: usize) -> Result<Vec<indexer::SearchResult>> {
        self.indexer.search(query, limit).await
    }
}
