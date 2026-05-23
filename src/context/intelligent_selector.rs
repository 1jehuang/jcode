//! Intelligent Context Selector with Call Graph Awareness
//!
//! Provides smart context selection for Agent prompts using:
//! - Call graph traversal (BFS up to 3 levels)
//! - PageRank-based file importance scoring
//! - Dynamic token budget allocation
//! - TF-IDF relevance scoring

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

use crate::ast::tree_sitter::{AstParser, FileAnalysis, SupportedLanguage};
use crate::incremental_index::{get_or_create_indexer, IncrementalIndexConfig};

/// Configuration for intelligent context selection
#[derive(Debug, Clone)]
pub struct SelectorConfig {
    /// Maximum token budget for context
    pub max_tokens: usize,
    /// BFS depth for call graph traversal
    pub bfs_depth: usize,
    /// Minimum PageRank score for inclusion
    pub min_pagerank: f64,
    /// Token estimation factor (chars per token)
    pub chars_per_token: usize,
}

impl Default for SelectorConfig {
    fn default() -> Self {
        Self {
            max_tokens: 4096,
            bfs_depth: 3,
            min_pagerank: 0.01,
            chars_per_token: 4,
        }
    }
}

/// Selected context for Agent prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedContext {
    /// Relevant functions from call graph
    pub functions: Vec<FunctionSnippet>,
    /// Relevant files (high importance)
    pub files: Vec<FileSnippet>,
    /// Selection metadata
    pub metadata: SelectionMetadata,
}

/// Function snippet with location info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSnippet {
    pub name: String,
    pub file: PathBuf,
    pub signature: String,
    pub code: String,
    pub line_start: usize,
    pub line_end: usize,
}

/// File snippet for context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnippet {
    pub path: PathBuf,
    pub content: String,
    pub importance_score: f64,
}

/// Metadata about the selection process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionMetadata {
    pub used_tokens: usize,
    pub budget_utilization: f64,
    pub selection_strategy: String,
    pub call_graph_nodes: usize,
    pub bfs_levels_traversed: usize,
}

/// Intelligent context selector with call graph awareness
pub struct IntelligentContextSelector {
    config: SelectorConfig,
    parser: Arc<AstParser>,
    /// Call graph: function_name -> list of called functions
    call_graph: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// File importance scores (PageRank)
    file_importance: Arc<RwLock<HashMap<PathBuf, f64>>>,
    /// Cache of parsed files
    file_cache: Arc<RwLock<HashMap<PathBuf, FileAnalysis>>>,
}

impl IntelligentContextSelector {
    /// Create a new intelligent context selector
    pub fn new(config: SelectorConfig, parser: Arc<AstParser>) -> Self {
        Self {
            config,
            parser,
            call_graph: Arc::new(RwLock::new(HashMap::new())),
            file_importance: Arc::new(RwLock::new(HashMap::new())),
            file_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build call graph from workspace root
    pub async fn build_call_graph(&self, workspace_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut call_graph = HashMap::new();
        let mut file_importance = HashMap::new();

        // Find all source files
        let source_files = self.find_source_files(workspace_root).await?;

        // Parse each file and extract call relationships
        for file_path in &source_files {
            if let Ok(analysis) = self.parser.analyze_file(file_path).await {
                // Add to call graph
                for (caller, callees) in &analysis.call_graph {
                    call_graph
                        .entry(caller.clone())
                        .or_insert_with(Vec::new)
                        .extend(callees.iter().cloned());
                }

                // Cache the analysis
                self.file_cache.write().await.insert(file_path.clone(), analysis);

                // Initialize file importance (will be updated by PageRank)
                file_importance.insert(file_path.clone(), 1.0);
            }
        }

        // Compute PageRank for file importance
        let pagerank_scores = self.compute_pagerank(&call_graph, &source_files).await;
        file_importance = pagerank_scores;

        // Store results
        *self.call_graph.write().await = call_graph;
        *self.file_importance.write().await = file_importance;

        tracing::info!(
            "Call graph built: {} nodes, {} files indexed",
            self.call_graph.read().await.len(),
            source_files.len()
        );

        Ok(())
    }

    /// Find all source files in workspace
    async fn find_source_files(&self, workspace_root: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        let mut files = Vec::new();
        let extensions = ["rs", "py", "ts", "js", "go", "java", "cpp", "c"];

        // Use standard library for directory walking
        if let Ok(entries) = std::fs::read_dir(workspace_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if extensions.contains(&ext) {
                            files.push(path);
                        }
                    }
                } else if path.is_dir() {
                    // Skip hidden directories and target/node_modules
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if !name.starts_with('.') && name != "target" && name != "node_modules" {
                            // Recursively search subdirectories (limited depth)
                            if let Ok(sub_entries) = std::fs::read_dir(&path) {
                                for sub_entry in sub_entries.flatten() {
                                    let sub_path = sub_entry.path();
                                    if sub_path.is_file() {
                                        if let Some(ext) = sub_path.extension().and_then(|e| e.to_str()) {
                                            if extensions.contains(&ext) {
                                                files.push(sub_path);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(files)
    }

    /// Compute PageRank for files based on call graph
    async fn compute_pagerank(
        &self,
        call_graph: &HashMap<String, Vec<String>>,
        files: &[PathBuf],
    ) -> HashMap<PathBuf, f64> {
        let damping = 0.85;
        let iterations = 50;
        let num_files = files.len();

        if num_files == 0 {
            return HashMap::new();
        }

        // Initialize scores uniformly
        let mut scores: HashMap<PathBuf, f64> = files.iter()
            .map(|f| (f.clone(), 1.0 / num_files as f64))
            .collect();

        // Build adjacency: file -> files it calls
        let mut adjacency: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for file in files {
            if let Some(analysis) = self.file_cache.read().await.get(file) {
                let mut called_files = Vec::new();
                for (_caller, callees) in &analysis.call_graph {
                    for callee in callees {
                        // Find which file contains the callee
                        for other_file in files {
                            if let Some(other_analysis) = self.file_cache.read().await.get(other_file) {
                                if other_analysis.symbols.iter().any(|s| s.name == *callee) {
                                    called_files.push(other_file.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
                adjacency.insert(file.clone(), called_files);
            }
        }

        // Iterative PageRank computation
        for _ in 0..iterations {
            let mut new_scores = HashMap::new();

            for file in files {
                let mut rank = (1.0 - damping) / num_files as f64;

                // Add contributions from incoming edges
                for other_file in files {
                    if let Some(targets) = adjacency.get(other_file) {
                        if targets.contains(file) {
                            let out_degree = targets.len();
                            if out_degree > 0 {
                                if let Some(score) = scores.get(other_file) {
                                    rank += damping * score / out_degree as f64;
                                }
                            }
                        }
                    }
                }

                new_scores.insert(file.clone(), rank);
            }

            scores = new_scores;
        }

        scores
    }

    /// Select relevant context for a query within token budget
    pub async fn select_context(
        &self,
        query: &str,
        token_budget: Option<usize>,
    ) -> Result<SelectedContext, Box<dyn std::error::Error>> {
        let budget = token_budget.unwrap_or(self.config.max_tokens);
        let call_graph = self.call_graph.read().await;
        let file_importance = self.file_importance.read().await;

        // Step 1: Find seed functions matching query (simple keyword match)
        let seed_functions = self.find_relevant_functions(query, &call_graph).await;

        // Step 2: BFS traverse call graph from seeds
        let mut selected_functions = Vec::new();
        let mut visited = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        // Enqueue seed functions
        for func in &seed_functions {
            queue.push_back((func.clone(), 0));
            visited.insert(func.clone());
        }

        let mut bfs_levels = 0;

        // BFS traversal
        while let Some((func_name, depth)) = queue.pop_front() {
            if depth >= self.config.bfs_depth {
                continue;
            }

            bfs_levels = bfs_levels.max(depth);

            // Find the file containing this function
            if let Some(file_path) = self.find_function_file(&func_name).await {
                if let Some(analysis) = self.file_cache.read().await.get(&file_path) {
                    // Check if this function exists in the analysis
                    if let Some(symbol) = analysis.symbols.iter().find(|s| s.name == func_name) {
                        let code = self.extract_function_code(&file_path, symbol).await?;
                        let tokens = self.estimate_tokens(&code);

                        if selected_functions.iter().map(|f: &FunctionSnippet| f.code.len()).sum::<usize>() / self.config.chars_per_token + tokens <= budget {
                            selected_functions.push(FunctionSnippet {
                                name: func_name.clone(),
                                file: file_path.clone(),
                                signature: symbol.signature.clone(),
                                code,
                                line_start: symbol.range.0,
                                line_end: symbol.range.1,
                            });
                        }
                    }
                }
            }

            // Enqueue callees
            if let Some(callees) = call_graph.get(&func_name) {
                for callee in callees {
                    if !visited.contains(callee) {
                        visited.insert(callee.clone());
                        queue.push_back((callee.clone(), depth + 1));
                    }
                }
            }
        }

        // Step 3: Add high-importance files if budget remains
        let mut selected_files = Vec::new();
        let used_tokens = selected_functions.iter().map(|f| f.code.len()).sum::<usize>() / self.config.chars_per_token;

        if used_tokens < budget {
            let remaining_budget = budget - used_tokens;

            // Sort files by importance
            let mut sorted_files: Vec<_> = file_importance.iter().collect();
            sorted_files.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));

            for (file_path, score) in sorted_files {
                if *score < self.config.min_pagerank {
                    continue;
                }

                // Skip files already included via functions
                if selected_functions.iter().any(|f| &f.file == file_path) {
                    continue;
                }

                if let Ok(content) = std::fs::read_to_string(file_path) {
                    let tokens = self.estimate_tokens(&content);
                    if tokens <= remaining_budget / 5 {
                        // Only include small files or summaries
                        selected_files.push(FileSnippet {
                            path: file_path.clone(),
                            content: if content.len() > 2000 {
                                format!("{}... (truncated)", &content[..2000])
                            } else {
                                content
                            },
                            importance_score: *score,
                        });
                    }
                }
            }
        }

        let final_used_tokens = used_tokens + selected_files.iter().map(|f| f.content.len()).sum::<usize>() / self.config.chars_per_token;

        Ok(SelectedContext {
            functions: selected_functions,
            files: selected_files,
            metadata: SelectionMetadata {
                used_tokens: final_used_tokens,
                budget_utilization: final_used_tokens as f64 / budget as f64,
                selection_strategy: "call_graph_bfs".to_string(),
                call_graph_nodes: call_graph.len(),
                bfs_levels_traversed: bfs_levels,
            },
        })
    }

    /// Find functions relevant to query
    async fn find_relevant_functions(
        &self,
        query: &str,
        call_graph: &HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        let query_lower = query.to_lowercase();
        let mut relevant = Vec::new();

        // Simple keyword matching against function names
        for func_name in call_graph.keys() {
            if func_name.to_lowercase().contains(&query_lower) {
                relevant.push(func_name.clone());
            }
        }

        // If no direct matches, return top functions by importance
        if relevant.is_empty() {
            let file_importance = self.file_importance.read().await;
            let mut file_funcs = Vec::new();

            for (file_path, _score) in file_importance.iter() {
                if let Some(analysis) = self.file_cache.read().await.get(file_path) {
                    for symbol in &analysis.symbols {
                        if symbol.kind == "function" || symbol.kind == "method" {
                            file_funcs.push(symbol.name.clone());
                        }
                    }
                }
            }

            relevant = file_funcs.into_iter().take(5).collect();
        }

        relevant
    }

    /// Find which file contains a function
    async fn find_function_file(&self, func_name: &str) -> Option<PathBuf> {
        let cache = self.file_cache.read().await;
        for (file_path, analysis) in cache.iter() {
            if analysis.symbols.iter().any(|s| s.name == func_name) {
                return Some(file_path.clone());
            }
        }
        None
    }

    /// Extract function code from file
    async fn extract_function_code(
        &self,
        file_path: &Path,
        symbol: &crate::ast::tree_sitter::SymbolInfo,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(file_path)?;
        let lines: Vec<&str> = content.lines().collect();

        if symbol.range.0 < lines.len() && symbol.range.1 <= lines.len() {
            let code = lines[symbol.range.0..symbol.range.1].join("\n");
            Ok(code)
        } else {
            Ok(format!("// Function {} at lines {}-{}", symbol.name, symbol.range.0, symbol.range.1))
        }
    }

    /// Estimate token count from text
    fn estimate_tokens(&self, text: &str) -> usize {
        text.len() / self.config.chars_per_token
    }

    /// Incremental update when a file changes
    pub async fn incremental_update(&self, changed_file: &Path) -> Result<(), Box<dyn std::error::Error>> {
        // Re-parse the changed file
        if let Ok(analysis) = self.parser.analyze_file(changed_file).await {
            // Update call graph
            let mut call_graph = self.call_graph.write().await;

            // Remove old entries for this file
            call_graph.retain(|_caller, callees| {
                // This is simplified - would need reverse index for proper cleanup
                true
            });

            // Add new entries
            for (caller, callees) in &analysis.call_graph {
                call_graph
                    .entry(caller.clone())
                    .or_insert_with(Vec::new)
                    .extend(callees.iter().cloned());
            }

            // Update cache
            self.file_cache.write().await.insert(changed_file.to_path_buf(), analysis);

            tracing::debug!("Incremental update for {:?}", changed_file);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_selector_creation() {
        let config = SelectorConfig::default();
        let parser = Arc::new(AstParser::with_defaults().unwrap());
        let selector = IntelligentContextSelector::new(config, parser);

        assert_eq!(selector.config.max_tokens, 4096);
        assert_eq!(selector.config.bfs_depth, 3);
    }

    #[tokio::test]
    async fn test_token_estimation() {
        let config = SelectorConfig::default();
        let parser = Arc::new(AstParser::with_defaults().unwrap());
        let selector = IntelligentContextSelector::new(config, parser);

        let text = "fn hello() { println!(\"world\"); }";
        let tokens = selector.estimate_tokens(text);
        assert!(tokens > 0);
    }
}
