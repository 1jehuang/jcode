// TODO: This module is scaffolding — types will be aligned with carpai-internal in Phase 1C
//! Semantic Memory - Embedding-based semantic search

#[allow(dead_code)]

use crate::memory::core_types::{EnhancedMemoryEntry, EnhancedMemoryQuery, VectorSearchResult};
use std::collections::HashMap;

/// Semantic memory with vector embeddings
pub struct SemanticMemory {
    memories: HashMap<String, EnhancedMemoryEntry>,
}

impl SemanticMemory {
    pub fn new() -> Self {
        Self {
            memories: HashMap::new(),
        }
    }

    /// Store a memory with embedding
    pub fn store(&mut self, memory: EnhancedMemoryEntry) {
        self.memories.insert(memory.id.clone(), memory);
    }

    /// Semantic search using vector similarity
    pub fn semantic_search(&self, query: &EnhancedMemoryQuery) -> Vec<VectorSearchResult> {
        if query.embedding.is_none() {
            return vec![];
        }
        
        let query_embedding = query.embedding.as_ref().unwrap();
        let mut results = Vec::new();
        
        for entry in self.memories.values() {
            if let Some(ref embedding) = entry.embedding {
                let similarity = cosine_similarity(query_embedding, embedding);
                
                if similarity >= query.similarity_threshold {
                    results.push(VectorSearchResult {
                        entry_id: entry.id.clone(),
                        similarity_score: similarity,
                        content: entry.content.clone(),
                        metadata: entry.metadata.clone(),
                    });
                }
            }
        }
        
        // Sort by similarity (descending)
        results.sort_by(|a, b| b.similarity_score.partial_cmp(&a.similarity_score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(query.limit);
        
        results
    }

    /// Get all memories
    pub fn get_all(&self) -> Vec<&EnhancedMemoryEntry> {
        self.memories.values().collect()
    }
}

/// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    
    let dot_product: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    
    dot_product / (norm_a * norm_b)
}

impl Default for SemanticMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
        
        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 1e-6);
    }

    #[test]
    fn test_semantic_search() {
        let mut memory = SemanticMemory::new();
        
        let entry = EnhancedMemoryEntry {
            id: "mem1".to_string(),
            content: "Test content".to_string(),
            embedding: Some(vec![1.0, 0.5, 0.3]),
            metadata: HashMap::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            scope: crate::memory::core_types::MemoryScope::Global,
            trust_level: crate::memory::core_types::TrustLevel::Medium,
            access_count: 0,
            last_accessed: None,
        };
        
        memory.store(entry);
        
        let query = EnhancedMemoryQuery {
            content_filter: None,
            embedding: Some(vec![1.0, 0.5, 0.3]),
            similarity_threshold: 0.8,
            scope: None,
            min_trust_level: None,
            limit: 10,
            offset: 0,
        };
        
        let results = memory.semantic_search(&query);
        assert_eq!(results.len(), 1);
        assert!((results[0].similarity_score - 1.0).abs() < 1e-6);
    }
}
