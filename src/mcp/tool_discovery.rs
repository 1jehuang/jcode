//! MCP Tool Discovery and Ranking
//!
//! Provides automatic discovery, relevance scoring, and dynamic inclusion
//! of MCP tools in Agent prompts.
//!
//! ## Architecture
//! ```text
//! Agent Request
//!     │
//!     ▼
//! ToolDiscoveryEngine
//!     ├── 1. Fetch all registered MCP tools
//!     ├── 2. Extract query keywords from user message
//!     ├── 3. Score each tool by relevance (TF-IDF + semantic)
//!     ├── 4. Rank and select top-N tools
//!     └── 5. Generate tool description for prompt
//!     │
//!     ▼
//! Agent Prompt (with relevant MCP tools)
//! ```

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::mcp::dynamic_registry::{DynamicToolRegistry, ToolCategory};

/// Configuration for tool discovery
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Maximum number of tools to include in prompt
    pub max_tools: usize,
    /// Minimum relevance score (0.0-1.0)
    pub min_score: f64,
    /// Enable semantic similarity scoring
    pub use_semantic: bool,
    /// Enable keyword-based TF-IDF scoring
    pub use_tfidf: bool,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            max_tools: 10,
            min_score: 0.1,
            use_semantic: true,
            use_tfidf: true,
        }
    }
}

/// A discovered tool with relevance score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredTool {
    /// Tool name (e.g., "github.list_pull_requests")
    pub name: String,
    /// Tool description
    pub description: String,
    /// Tool category
    pub category: ToolCategory,
    /// Relevance score (0.0-1.0)
    pub score: f64,
    /// Input schema (JSON Schema)
    pub input_schema: Option<serde_json::Value>,
}

/// Result of tool discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResult {
    /// Selected tools sorted by relevance
    pub tools: Vec<DiscoveredTool>,
    /// Total tools considered
    pub total_candidates: usize,
    /// Time taken for discovery (ms)
    pub discovery_time_ms: u64,
}

/// Engine for discovering and ranking MCP tools
pub struct ToolDiscoveryEngine {
    config: DiscoveryConfig,
    registry: std::sync::Arc<DynamicToolRegistry>,
    /// Cached TF-IDF index for tool descriptions
    tfidf_index: Option<TfIdfIndex>,
}

impl ToolDiscoveryEngine {
    /// Create a new discovery engine
    pub fn new(
        config: DiscoveryConfig,
        registry: std::sync::Arc<DynamicToolRegistry>,
    ) -> Self {
        Self {
            config,
            registry,
            tfidf_index: None,
        }
    }

    /// Build TF-IDF index from all registered tools
    pub async fn build_index(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let tools = self.registry.list_all_tools().await?;
        let documents: Vec<&str> = tools.iter()
            .map(|t| t.description.as_str())
            .collect();

        self.tfidf_index = Some(TfIdfIndex::build(&documents));
        Ok(())
    }

    /// Discover relevant tools for a given query
    pub async fn discover(
        &self,
        query: &str,
    ) -> Result<DiscoveryResult, Box<dyn std::error::Error>> {
        let start = std::time::Instant::now();

        // Fetch all registered tools
        let all_tools = self.registry.list_all_tools().await?;
        let total_candidates = all_tools.len();

        // Score each tool
        let mut scored_tools: Vec<(DiscoveredTool, f64)> = Vec::new();

        for tool in &all_tools {
            let score = self.calculate_relevance(query, tool).await;
            if score >= self.config.min_score {
                let discovered = DiscoveredTool {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    category: tool.category.clone(),
                    score,
                    input_schema: tool.input_schema.clone(),
                };
                scored_tools.push((discovered, score));
            }
        }

        // Sort by score descending
        scored_tools.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top-N
        let selected: Vec<DiscoveredTool> = scored_tools
            .into_iter()
            .take(self.config.max_tools)
            .map(|(tool, _)| tool)
            .collect();

        let discovery_time_ms = start.elapsed().as_millis() as u64;

        Ok(DiscoveryResult {
            tools: selected,
            total_candidates,
            discovery_time_ms,
        })
    }

    /// Calculate relevance score for a tool given a query
    async fn calculate_relevance(
        &self,
        query: &str,
        tool: &crate::mcp::dynamic_registry::DynamicTool,
    ) -> f64 {
        let mut score = 0.0;

        // Keyword-based TF-IDF scoring
        if self.config.use_tfidf {
            if let Some(ref index) = self.tfidf_index {
                let tfidf_score = index.score_document(query, &tool.description);
                score += tfidf_score * 0.5;
            }
        }

        // Semantic similarity scoring (placeholder - would use embeddings)
        if self.config.use_semantic {
            let semantic_score = self.semantic_similarity(query, &tool.description).await;
            score += semantic_score * 0.5;
        }

        // Boost score for exact keyword matches in tool name
        let query_lower = query.to_lowercase();
        let name_lower = tool.name.to_lowercase();
        if name_lower.contains(&query_lower) {
            score += 0.3;
        }

        // Boost for category match (if query mentions category)
        for keyword in &["github", "jira", "slack", "docker", "postgres", "redis"] {
            if query_lower.contains(keyword) && tool.category.to_string().to_lowercase().contains(keyword) {
                score += 0.2;
            }
        }

        score.min(1.0)
    }

    /// Placeholder for semantic similarity using embeddings
    async fn semantic_similarity(&self, _query: &str, _description: &str) -> f64 {
        // TODO: Integrate with embedding model (e.g., sentence-transformers)
        // For now, return a neutral score
        0.5
    }

    /// Format discovered tools for inclusion in Agent prompt
    pub fn format_for_prompt(result: &DiscoveryResult) -> String {
        if result.tools.is_empty() {
            return "No relevant MCP tools found for this query.".to_string();
        }

        let mut output = String::from("\n## Available MCP Tools (ranked by relevance)\n\n");

        for (i, tool) in result.tools.iter().enumerate() {
            output.push_str(&format!(
                "{}. **{}** (score: {:.2}, category: {})\n",
                i + 1,
                tool.name,
                tool.score,
                tool.category
            ));
            output.push_str(&format!("   {}\n", tool.description));

            if let Some(ref schema) = tool.input_schema {
                if let Some(props) = schema.get("properties") {
                    if let Some(obj) = props.as_object() {
                        if !obj.is_empty() {
                            output.push_str("   Parameters:\n");
                            for (param_name, param_def) in obj {
                                let param_type = param_def.get("type")
                                    .and_then(|t| t.as_str())
                                    .unwrap_or("any");
                                output.push_str(&format!("     - `{}` ({})\n", param_name, param_type));
                            }
                        }
                    }
                }
            }
            output.push('\n');
        }

        output.push_str(&format!(
            "_Selected {} of {} candidate tools ({}ms)_\n",
            result.tools.len(),
            result.total_candidates,
            result.discovery_time_ms
        ));

        output
    }
}

/// Simple TF-IDF index for text scoring
struct TfIdfIndex {
    /// Document frequency for each term
    df: HashMap<String, usize>,
    /// Total number of documents
    num_docs: usize,
    /// Terms in each document
    doc_terms: Vec<Vec<String>>,
}

impl TfIdfIndex {
    /// Build TF-IDF index from a list of documents
    fn build(documents: &[&str]) -> Self {
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut doc_terms: Vec<Vec<String>> = Vec::new();

        for doc in documents {
            let terms = Self::tokenize(doc);
            let unique_terms: std::collections::HashSet<_> = terms.iter().cloned().collect();

            for term in unique_terms {
                *df.entry(term).or_insert(0) += 1;
            }

            doc_terms.push(terms);
        }

        Self {
            df,
            num_docs: documents.len(),
            doc_terms,
        }
    }

    /// Tokenize text into lowercase terms
    fn tokenize(text: &str) -> Vec<String> {
        text.split_whitespace()
            .map(|t| t.to_lowercase())
            .filter(|t| t.len() > 2) // Skip very short words
            .collect()
    }

    /// Score a query against a document
    fn score_document(&self, query: &str, document: &str) -> f64 {
        let query_terms = Self::tokenize(query);
        let doc_terms = Self::tokenize(document);

        if query_terms.is_empty() || doc_terms.is_empty() {
            return 0.0;
        }

        // Calculate TF-IDF score
        let mut score = 0.0;
        for query_term in &query_terms {
            // Term frequency in document
            let tf = doc_terms.iter().filter(|t| t == &query_term).count() as f64
                / doc_terms.len() as f64;

            // Inverse document frequency
            let doc_freq = self.df.get(query_term).copied().unwrap_or(0);
            let idf = ((self.num_docs as f64) / (1.0 + doc_freq as f64)).ln();

            score += tf * idf;
        }

        // Normalize by query length
        score / query_terms.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize() {
        let terms = TfIdfIndex::tokenize("Hello World Testing");
        assert_eq!(terms, vec!["hello", "world", "testing"]);
    }

    #[test]
    fn test_tfidf_scoring() {
        let docs = vec![
            "List pull requests in GitHub repository",
            "Create a new Jira issue",
            "Send message to Slack channel",
        ];

        let index = TfIdfIndex::build(&docs.iter().map(|s| s.as_str()).collect::<Vec<_>>());

        // Query for GitHub should score first doc highest
        let score_0 = index.score_document("GitHub pull requests", docs[0]);
        let score_1 = index.score_document("GitHub pull requests", docs[1]);
        let score_2 = index.score_document("GitHub pull requests", docs[2]);

        assert!(score_0 > score_1);
        assert!(score_0 > score_2);
    }
}
