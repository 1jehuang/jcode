//! Layer 1: Indexing Layer - Global Symbol Index

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::{debug, error, info, warn};

use crate::{
    PhaseResult, PhaseName, PhaseOutput, SurgicalRequest, Language,
    ProjectIndexStats, IndexingLayer,
};

/// Symbol information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: String,
    pub file_path: PathBuf,
    pub definition_line: usize,
    pub is_definition: bool,
    pub language: Language,
}

/// Indexing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingConfig {
    pub project_root: PathBuf,
    pub concurrency_limit: usize,
    pub enable_lsp_indexing: bool,
    pub enable_ctags_indexing: bool,
    pub exclude_patterns: Vec<String>,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            project_root: PathBuf::from("."),
            concurrency_limit: 4,
            enable_lsp_indexing: true,
            enable_ctags_indexing: true,
            exclude_patterns: vec![
                "node_modules".to_string(),
                "target".to_string(),
                "__pycache__".to_string(),
                ".git".to_string(),
            ],
        }
    }
}

/// Global symbol indexer
pub struct GlobalSymbolIndexer {
    config: IndexingConfig,
    lsp_symbols: Arc<RwLock<HashMap<String, SymbolInfo>>>,
    ctags_index: Arc<RwLock<HashMap<String, Vec<String>>>>,
    stats: Arc<RwLock<IndexingStats>>,
}

/// Indexing statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexingStats {
    pub total_symbols: usize,
    pub total_files_indexed: usize,
}

impl GlobalSymbolIndexer {
    pub fn new(config: IndexingConfig) -> Self {
        Self {
            config,
            lsp_symbols: Arc::new(RwLock::new(HashMap::new())),
            ctags_index: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(IndexingStats::default())),
        }
    }

    /// Build full project index
    pub async fn build_full_index(&self) -> Result<ProjectIndexStats> {
        let start_time = std::time::Instant::now();

        info!(
            project_root = %self.config.project_root.display(),
            "Starting full index build"
        );

        // Scan project files (simplified)
        let files_to_index = self.scan_project_files()?;

        info!(files_found = files_to_index.len(), "Project files scanned");

        // Process files concurrently (simplified - just count)
        for _file in &files_to_index {
            // TODO: Implement actual file parsing with LSP/Tree-sitter
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        {
            let mut stats = self.stats.write();
            stats.total_symbols = 0; // Will be populated by actual implementation
            stats.total_files_indexed = files_to_index.len();
        }

        info!(
            files = files_to_index.len(),
            duration_ms = duration_ms,
            "Full index build completed"
        );

        Ok(ProjectIndexStats {
            total_symbols: self.stats.read().total_symbols,
            total_files: files_to_index.len(),
            languages_detected: vec!["Rust".to_string()],
            index_build_time: Utc::now(),
        })
    }

    fn scan_project_files(&self) -> Result<Vec<PathBuf>> {
        let root = &self.config.project_root;
        let mut files = Vec::new();

        if root.is_dir() {
            if let Ok(entries) = std::fs::read_dir(root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(ext) = path.extension() {
                            match ext.to_str() {
                                Some("rs") | Some("py") | Some("ts") => {
                                    files.push(path);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        Ok(files)
    }

    /// Find symbols by name
    pub async fn find_symbol_by_name(&self, name: &str) -> Vec<SymbolInfo> {
        let symbols = self.lsp_symbols.read();
        symbols.values()
            .filter(|s| s.name.contains(name))
            .cloned()
            .collect()
    }

    /// Fuzzy search symbols
    pub async fn fuzzy_search_symbols(&self, query: &str, limit: usize) -> Vec<SymbolInfo> {
        let query_lower = query.to_lowercase();
        let symbols = self.lsp_symbols.read();
        
        symbols.values()
            .filter(|s| s.name.to_lowercase().contains(&query_lower))
            .take(limit)
            .cloned()
            .collect()
    }

    /// Get statistics
    pub async fn get_stats(&self) -> IndexingStats {
        self.stats.read().clone()
    }
}

#[async_trait::async_trait]
impl IndexingLayer for GlobalSymbolIndexer {
    async fn build_context_index(&self, request: &SurgicalRequest) -> Result<PhaseResult> {
        let start_time = std::time::Instant::now();

        info!(
            request_id = %request.request_id,
            target = ?request.target,
            "Building context index for surgical request"
        );

        // Build index based on target scope
        match &request.target {
            crate::TargetScope::EntireProject { .. } => {
                self.build_full_index().await?;
            }
            crate::TargetScope::SingleFile { path } => {
                info!(file = %path.display(), "Indexing single file");
            }
            _ => {}
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;
        let stats = self.stats.read();

        Ok(PhaseResult {
            phase: PhaseName::Indexing,
            passed: true,
            duration_ms,
            output: PhaseOutput::IndexingOutput {
                symbols_found: stats.total_symbols,
                files_indexed: stats.total_files_indexed,
                index_duration_ms: duration_ms,
            },
            warnings: Vec::new(),
            errors: Vec::new(),
        })
    }

    async fn get_project_stats(&self) -> Result<ProjectIndexStats> {
        let stats = self.stats.read();
        Ok(ProjectIndexStats {
            total_symbols: stats.total_symbols,
            total_files: stats.total_files_indexed,
            languages_detected: vec!["Rust".to_string()],
            index_build_time: Utc::now(),
        })
    }
}
