//! Semantic Search for Code Completion using Vector Embeddings
//!
//! This module provides semantic similarity search for code completions,
//! allowing the engine to find relevant code patterns even when textual
//! matching fails.
//!
//! Features:
//! - Code snippet embedding (using sentence-transformers or similar)
//! - Vector database for fast similarity search
//! - Context-aware retrieval

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Represents a vector embedding (simplified - in production use ndarray or similar)
#[derive(Debug, Clone)]
pub struct Embedding {
    pub values: Vec<f32>,
    pub dimension: usize,
}

impl Embedding {
    pub fn new(values: Vec<f32>) -> Self {
        let dimension = values.len();
        Self { values, dimension }
    }

    /// Compute cosine similarity with another embedding
    pub fn cosine_similarity(&self, other: &Embedding) -> f32 {
        if self.dimension != other.dimension {
            return 0.0;
        }

        let dot_product: f32 = self.values.iter()
            .zip(other.values.iter())
            .map(|(a, b)| a * b)
            .sum();

        let magnitude_a: f32 = self.values.iter().map(|v| v * v).sum::<f32>().sqrt();
        let magnitude_b: f32 = other.values.iter().map(|v| v * v).sum::<f32>().sqrt();

        if magnitude_a == 0.0 || magnitude_b == 0.0 {
            return 0.0;
        }

        dot_product / (magnitude_a * magnitude_b)
    }
}

/// A code snippet with its embedding
#[derive(Debug, Clone)]
pub struct CodeSnippet {
    pub id: String,
    pub code: String,
    pub language: String,
    pub embedding: Embedding,
    pub metadata: HashMap<String, String>,
    pub usage_count: u32,
}

/// Semantic search engine for code completion
pub struct SemanticCompleter {
    /// Snippet database: id -> snippet
    snippets: Arc<RwLock<HashMap<String, CodeSnippet>>>,
    /// Index by tags/categories
    tag_index: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Configuration
    config: SemanticConfig,
}

#[derive(Debug, Clone)]
pub struct SemanticConfig {
    /// Minimum similarity threshold (0.0 - 1.0)
    pub min_similarity: f32,
    /// Maximum number of results to return
    pub max_results: usize,
    /// Embedding dimension (e.g., 384 for all-MiniLM-L6-v2)
    pub embedding_dimension: usize,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self {
            min_similarity: 0.7,
            max_results: 10,
            embedding_dimension: 384,
        }
    }
}

impl SemanticCompleter {
    pub fn new(config: SemanticConfig) -> Self {
        Self {
            snippets: Arc::new(RwLock::new(HashMap::new())),
            tag_index: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Add a code snippet to the database
    pub async fn add_snippet(&self, snippet: CodeSnippet) {
        let id = snippet.id.clone();
        self.snippets.write().insert(id.clone(), snippet);

        // Update tag index (for now, use language as tag)
        // In production, extract more meaningful tags
        let mut tag_idx = self.tag_index.write();
        tag_idx
            .entry("all".to_string())
            .or_insert_with(Vec::new)
            .push(id);
    }

    /// Find semantically similar snippets to the query
    pub async fn search_similar(&self, query_embedding: &Embedding, language: Option<&str>) -> Vec<(CodeSnippet, f32)> {
        let snippets = self.snippets.read();
        let mut results = Vec::new();

        for snippet in snippets.values() {
            // Filter by language if specified
            if let Some(lang) = language {
                if snippet.language != lang {
                    continue;
                }
            }

            let similarity = query_embedding.cosine_similarity(&snippet.embedding);

            if similarity >= self.config.min_similarity {
                results.push((snippet.clone(), similarity));
            }
        }

        // Sort by similarity (descending)
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(self.config.max_results);

        results
    }

    /// Generate embedding for code (placeholder - integrate with actual model)
    pub async fn generate_embedding(&self, code: &str) -> Embedding {
        // TODO: Integrate with actual embedding model
        // For now, return a dummy embedding
        // In production, use:
        // - candle (Hugging Face transformers in Rust)
        // - ort (ONNX Runtime)
        // - External API call to embedding service

        let mut values = vec![0.0f32; self.config.embedding_dimension];
        // Simple hash-based pseudo-embedding for demonstration
        for (i, byte) in code.bytes().enumerate() {
            let idx = i % self.config.embedding_dimension;
            values[idx] += byte as f32 / 255.0;
        }

        // Normalize
        let magnitude: f32 = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if magnitude > 0.0 {
            for v in values.iter_mut() {
                *v /= magnitude;
            }
        }

        Embedding::new(values)
    }

    /// Get statistics
    pub fn get_stats(&self) -> SemanticStats {
        let snippets = self.snippets.read();
        SemanticStats {
            total_snippets: snippets.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SemanticStats {
    pub total_snippets: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cosine_similarity() {
        let emb1 = Embedding::new(vec![1.0, 0.0, 0.0]);
        let emb2 = Embedding::new(vec![1.0, 0.0, 0.0]);
        let emb3 = Embedding::new(vec![0.0, 1.0, 0.0]);

        assert!((emb1.cosine_similarity(&emb2) - 1.0).abs() < 1e-6);
        assert!((emb1.cosine_similarity(&emb3)).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_semantic_search() {
        let completer = SemanticCompleter::new(SemanticConfig::default());

        let snippet = CodeSnippet {
            id: "test1".to_string(),
            code: "fn hello() {}".to_string(),
            language: "rust".to_string(),
            embedding: Embedding::new(vec![1.0; 384]),
            metadata: HashMap::new(),
            usage_count: 0,
        };

        completer.add_snippet(snippet).await;

        let query_emb = Embedding::new(vec![0.9; 384]);
        let results = completer.search_similar(&query_emb, Some("rust")).await;

        assert_eq!(results.len(), 1);
    }
}
