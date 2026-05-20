//! Incremental Indexing System for Fast Symbol Lookup
//!
//! This module maintains an in-memory index of symbols across the workspace,
//! updating incrementally as files change. This avoids full re-indexing on
//! every edit, reducing latency from seconds to milliseconds.
//!
//! Architecture:
//! ```text
//! File Change Event -> ChangeDetector -> IndexUpdater -> SymbolIndex
//!                                                |
//!                                                v
//!                                       Query Interface (sub-ms lookup)
//! ```

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Maximum time to wait before forcing an index rebuild (30 seconds)
const MAX_INCREMENTAL_UPDATES: u32 = 50;

/// Represents a symbol in the codebase
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SymbolEntry {
    pub name: String,
    pub kind: SymbolKind,
    pub file_path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub signature: Option<String>,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Variable,
    Constant,
    TypeAlias,
    Module,
    Field,
    Parameter,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Interface => "interface",
            Self::Variable => "variable",
            Self::Constant => "constant",
            Self::TypeAlias => "type_alias",
            Self::Module => "module",
            Self::Field => "field",
            Self::Parameter => "parameter",
        }
    }
}

/// Represents a file change event
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub file_path: PathBuf,
    pub change_type: ChangeType,
    pub timestamp: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
}

/// Incremental symbol index
pub struct IncrementalIndex {
    /// Main symbol index: symbol_name -> Vec<SymbolEntry>
    symbols: Arc<RwLock<HashMap<String, Vec<SymbolEntry>>>>,
    /// File to symbols mapping: file_path -> Set<symbol_names>
    file_symbols: Arc<RwLock<HashMap<PathBuf, HashSet<String>>>>,
    /// Index metadata
    metadata: Arc<RwLock<IndexMetadata>>,
    /// Background update channel
    update_tx: mpsc::Sender<FileChangeEvent>,
}

#[derive(Debug)]
struct IndexMetadata {
    total_symbols: usize,
    indexed_files: usize,
    last_full_index: Instant,
    incremental_updates: u32,
    avg_query_time_ms: f64,
}

impl IncrementalIndex {
    pub fn new() -> Self {
        let (update_tx, mut update_rx) = mpsc::channel::<FileChangeEvent>(100);

        let index = Self {
            symbols: Arc::new(RwLock::new(HashMap::new())),
            file_symbols: Arc::new(RwLock::new(HashMap::new())),
            metadata: Arc::new(RwLock::new(IndexMetadata {
                total_symbols: 0,
                indexed_files: 0,
                last_full_index: Instant::now(),
                incremental_updates: 0,
                avg_query_time_ms: 0.0,
            })),
            update_tx,
        };

        // Spawn background indexer
        let symbols = index.symbols.clone();
        let file_symbols = index.file_symbols.clone();
        let metadata = index.metadata.clone();
        tokio::spawn(async move {
            while let Some(event) = update_rx.recv().await {
                debug!("Processing file change: {:?}", event.file_path);
                Self::process_file_change(&symbols, &file_symbols, &metadata, event).await;
            }
        });

        index
    }

    /// Queue a file change for indexing (non-blocking)
    pub async fn queue_file_change(&self, event: FileChangeEvent) {
        if let Err(e) = self.update_tx.send(event).await {
            warn!("Failed to queue file change: {}", e);
        }
    }

    /// Query symbols by name prefix (fuzzy search)
    pub async fn query_symbols(&self, prefix: &str, limit: usize) -> Vec<SymbolEntry> {
        let start = Instant::now();
        let prefix_lower = prefix.to_lowercase();

        let symbols = self.symbols.read();
        let mut results = Vec::new();

        // Exact prefix match first
        for (name, entries) in symbols.iter() {
            if name.to_lowercase().starts_with(&prefix_lower) {
                results.extend(entries.iter().cloned());
            }
        }

        // If not enough results, try substring match
        if results.len() < limit {
            for (name, entries) in symbols.iter() {
                if name.to_lowercase().contains(&prefix_lower) && !results.iter().any(|r: &SymbolEntry| r.name == *name) {
                    results.extend(entries.iter().cloned());
                }
            }
        }

        // Sort by relevance (shorter names first, then by file proximity)
        results.sort_by(|a, b| {
            let name_len_cmp = a.name.len().cmp(&b.name.len());
            if name_len_cmp != std::cmp::Ordering::Equal {
                return name_len_cmp;
            }
            a.file_path.cmp(&b.file_path)
        });

        results.truncate(limit);

        // Update query time stats
        let query_time_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.update_query_stats(query_time_ms);

        results
    }

    /// Get all symbols in a specific file
    pub async fn get_file_symbols(&self, file_path: &Path) -> Vec<SymbolEntry> {
        let file_symbols = self.file_symbols.read();
        let symbols = self.symbols.read();

        if let Some(symbol_names) = file_symbols.get(file_path) {
            let mut result = Vec::new();
            for name in symbol_names {
                if let Some(entries) = symbols.get(name) {
                    result.extend(entries.iter().filter(|e| e.file_path == file_path).cloned());
                }
            }
            result
        } else {
            Vec::new()
        }
    }

    /// Get statistics about the index
    pub fn get_stats(&self) -> IndexStatistics {
        let metadata = self.metadata.read();
        IndexStatistics {
            total_symbols: metadata.total_symbols,
            indexed_files: metadata.indexed_files,
            last_full_index_age: metadata.last_full_index.elapsed(),
            incremental_updates: metadata.incremental_updates,
            avg_query_time_ms: metadata.avg_query_time_ms,
        }
    }

    /// Process a file change event (called by background worker)
    async fn process_file_change(
        symbols: &Arc<RwLock<HashMap<String, Vec<SymbolEntry>>>>,
        file_symbols: &Arc<RwLock<HashMap<PathBuf, HashSet<String>>>>,
        metadata: &Arc<RwLock<IndexMetadata>>,
        event: FileChangeEvent,
    ) {
        match event.change_type {
            ChangeType::Deleted => {
                // Remove all symbols from deleted file
                let mut file_sym = file_symbols.write();
                if let Some(symbol_names) = file_sym.remove(&event.file_path) {
                    let mut sym_map = symbols.write();
                    for name in symbol_names {
                        if let Some(entries) = sym_map.get_mut(&name) {
                            entries.retain(|e| e.file_path != event.file_path);
                            if entries.is_empty() {
                                sym_map.remove(&name);
                            }
                        }
                    }
                }
            }
            ChangeType::Modified | ChangeType::Created => {
                // In a real implementation, this would parse the file and extract symbols
                // For now, we just mark it as needing re-indexing
                // TODO: Implement actual symbol extraction using tree-sitter or LSP
                debug!("File modified/created, would extract symbols: {:?}", event.file_path);
            }
        }

        // Update metadata
        let mut meta = metadata.write();
        meta.incremental_updates += 1;

        // Force full reindex after too many incremental updates
        if meta.incremental_updates >= MAX_INCREMENTAL_UPDATES {
            info!("Reached max incremental updates, scheduling full reindex");
            meta.incremental_updates = 0;
            meta.last_full_index = Instant::now();
            // TODO: Trigger full reindex
        }
    }

    /// Update query time statistics
    fn update_query_stats(&self, query_time_ms: f64) {
        let mut metadata = self.metadata.write();
        // Exponential moving average
        metadata.avg_query_time_ms = metadata.avg_query_time_ms * 0.9 + query_time_ms * 0.1;
    }
}

#[derive(Debug, Clone)]
pub struct IndexStatistics {
    pub total_symbols: usize,
    pub indexed_files: usize,
    pub last_full_index_age: Duration,
    pub incremental_updates: u32,
    pub avg_query_time_ms: f64,
}

impl Default for IncrementalIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_incremental_index_basic() {
        let index = IncrementalIndex::new();

        // Add some test symbols manually (in real usage, this happens via file changes)
        {
            let mut symbols = index.symbols.write();
            let entry = SymbolEntry {
                name: "println".to_string(),
                kind: SymbolKind::Function,
                file_path: PathBuf::from("src/main.rs"),
                line: 10,
                column: 4,
                signature: Some("fn println(s: &str)".to_string()),
                documentation: None,
            };
            symbols.entry("println".to_string()).or_insert_with(Vec::new).push(entry);

            let mut file_sym = index.file_symbols.write();
            file_sym
                .entry(PathBuf::from("src/main.rs"))
                .or_insert_with(HashSet::new)
                .insert("println".to_string());
        }

        // Query should find the symbol
        let results = index.query_symbols("print", 10).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "println");
    }

    #[tokio::test]
    async fn test_query_returns_empty_for_no_match() {
        let index = IncrementalIndex::new();
        let results = index.query_symbols("nonexistent_xyz", 10).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_index_stats() {
        let index = IncrementalIndex::new();
        let stats = index.get_stats();
        assert_eq!(stats.total_symbols, 0);
        assert_eq!(stats.indexed_files, 0);
    }
}
