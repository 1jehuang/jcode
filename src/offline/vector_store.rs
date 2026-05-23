//! Local Vector Store with HNSW Index Integration
//!
//! Combines SQLite cache with HNSW index for efficient offline vector search.
//! Automatically falls back to local store when remote pgvector/Milvus is unavailable.

use crate::offline::{LocalCache, HNSWIndex, HNSWConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};

/// Vector document with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorDocument {
    pub id: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub metadata: serde_json::Value,
    pub created_at: i64,
}

/// Search result from vector store
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResult {
    pub id: String,
    pub content: String,
    pub score: f32,
    pub metadata: serde_json::Value,
}

/// Configuration for local vector store
#[derive(Debug, Clone)]
pub struct VectorStoreConfig {
    /// Dimension of embeddings
    pub dimension: usize,
    /// HNSW M parameter
    pub hnsw_m: usize,
    /// HNSW ef_construction
    pub hnsw_ef_construction: usize,
    /// HNSW ef_search
    pub hnsw_ef_search: usize,
    /// Maximum cache entries
    pub max_cache_entries: usize,
    /// Database path
    pub db_path: PathBuf,
}

impl Default for VectorStoreConfig {
    fn default() -> Self {
        Self {
            dimension: 768, // Common embedding dimension
            hnsw_m: 16,
            hnsw_ef_construction: 200,
            hnsw_ef_search: 50,
            max_cache_entries: 10000,
            db_path: PathBuf::from(".carpai/offline_vector_store.db"),
        }
    }
}

/// Local vector store combining SQLite + HNSW
pub struct LocalVectorStore {
    config: VectorStoreConfig,
    cache: Arc<LocalCache>,
    hnsw_index: Arc<HNSWIndex>,
    /// Mapping from HNSW ID to document ID
    id_mapping: Arc<RwLock<std::collections::HashMap<usize, String>>>,
    /// Reverse mapping
    reverse_mapping: Arc<RwLock<std::collections::HashMap<String, usize>>>,
    next_id: Arc<RwLock<usize>>,
}

impl LocalVectorStore {
    /// Create or open local vector store
    pub async fn new(config: VectorStoreConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Ensure parent directory exists
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let cache = Arc::new(LocalCache::new(
            config.db_path.clone(),
            config.max_cache_entries,
        )?);

        let hnsw_config = HNSWConfig {
            m: config.hnsw_m,
            ef_construction: config.hnsw_ef_construction,
            ef_search: config.hnsw_ef_search,
            max_layers: None,
            distance_metric: crate::offline::hnsw_index::DistanceMetric::Cosine,
        };

        let hnsw_index = Arc::new(HNSWIndex::new(config.dimension, hnsw_config));

        info!(
            "Local vector store initialized at {:?} (dim={})",
            config.db_path, config.dimension
        );

        Ok(Self {
            config,
            cache,
            hnsw_index,
            id_mapping: Arc::new(RwLock::new(std::collections::HashMap::new())),
            reverse_mapping: Arc::new(RwLock::new(std::collections::HashMap::new())),
            next_id: Arc::new(RwLock::new(0)),
        })
    }

    /// Add a document to the vector store
    pub async fn add(&self, doc: VectorDocument) -> Result<(), Box<dyn std::error::Error>> {
        if doc.embedding.len() != self.config.dimension {
            return Err(format!(
                "Embedding dimension mismatch: expected {}, got {}",
                self.config.dimension,
                doc.embedding.len()
            ).into());
        }

        // Get next internal ID
        let mut next_id = self.next_id.write().await;
        let internal_id = *next_id;
        *next_id += 1;
        drop(next_id);

        // Store mappings
        self.id_mapping.write().await.insert(internal_id, doc.id.clone());
        self.reverse_mapping.write().await.insert(doc.id.clone(), internal_id);

        // Insert into HNSW index
        self.hnsw_index
            .insert(internal_id, doc.embedding.clone(), Some(doc.content.clone()))
            .await?;

        // Cache the document in SQLite
        let serialized = bincode::serialize(&doc)?;
        self.cache
            .put(&doc.id, serialized, Some(doc.embedding), None)
            .await?;

        debug!("Added document {} (internal_id={})", doc.id, internal_id);
        Ok(())
    }

    /// Add multiple documents in batch
    pub async fn add_batch(
        &self,
        docs: Vec<VectorDocument>,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let mut count = 0;
        for doc in docs {
            if let Err(e) = self.add(doc).await {
                warn!("Failed to add document: {}", e);
            } else {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Search for similar documents
    pub async fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        min_score: f32,
    ) -> Result<Vec<VectorSearchResult>, Box<dyn std::error::Error>> {
        if query_embedding.len() != self.config.dimension {
            return Err(format!(
                "Query dimension mismatch: expected {}, got {}",
                self.config.dimension,
                query_embedding.len()
            ).into());
        }

        // Search HNSW index
        let hnsw_results = self.hnsw_index.search(query_embedding, top_k).await?;

        // Convert to VectorSearchResult
        let mut results = Vec::new();
        let id_mapping = self.id_mapping.read().await;

        for hnsw_result in hnsw_results {
            if let Some(doc_id) = id_mapping.get(&hnsw_result.id) {
                // Retrieve full document from cache
                if let Some(cached) = self.cache.get(doc_id).await? {
                    if let Ok(doc) = bincode::deserialize::<VectorDocument>(&cached.value) {
                        // Convert distance to similarity score (1 - distance for cosine)
                        let score = 1.0 - hnsw_result.distance;

                        if score >= min_score {
                            results.push(VectorSearchResult {
                                id: doc.id,
                                content: doc.content,
                                score,
                                metadata: doc.metadata,
                            });
                        }
                    }
                }
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        debug!("Vector search returned {} results", results.len());
        Ok(results)
    }

    /// Delete a document
    pub async fn delete(&self, doc_id: &str) -> Result<bool, Box<dyn std::error::Error>> {
        // Get internal ID
        let internal_id = self.reverse_mapping.read().await.get(doc_id).copied();

        if let Some(id) = internal_id {
            // Remove from mappings
            self.id_mapping.write().await.remove(&id);
            self.reverse_mapping.write().await.remove(doc_id);

            // Remove from cache
            self.cache.delete(doc_id).await?;

            // Note: HNSW doesn't support deletion, mark as deleted in metadata
            // For production, consider rebuilding index periodically

            debug!("Deleted document {}", doc_id);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get document count
    pub async fn len(&self) -> usize {
        self.id_mapping.read().await.len()
    }

    /// Check if empty
    pub async fn is_empty(&self) -> bool {
        self.id_mapping.read().await.is_empty()
    }

    /// Clear all documents
    pub async fn clear(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.id_mapping.write().await.clear();
        self.reverse_mapping.write().await.clear();
        self.hnsw_index.clear().await;
        *self.next_id.write().await = 0;

        info!("Cleared local vector store");
        Ok(())
    }

    /// Get statistics
    pub async fn stats(&self) -> Result<VectorStoreStats, Box<dyn std::error::Error>> {
        let cache_stats = self.cache.stats().await?;

        Ok(VectorStoreStats {
            document_count: self.len().await,
            cache_entries: cache_stats.total_entries,
            cache_size_bytes: cache_stats.total_size_bytes,
            vectors_with_embeddings: cache_stats.entries_with_embeddings,
        })
    }
}

/// Vector store statistics
#[derive(Debug, Clone)]
pub struct VectorStoreStats {
    pub document_count: usize,
    pub cache_entries: usize,
    pub cache_size_bytes: usize,
    pub vectors_with_embeddings: usize,
}

/// Offline mode manager with vector store integration
pub struct OfflineVectorManager {
    store: Arc<LocalVectorStore>,
    is_online: Arc<RwLock<bool>>,
}

impl OfflineVectorManager {
    /// Create offline vector manager
    pub async fn new(config: VectorStoreConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let store = Arc::new(LocalVectorStore::new(config).await?);

        Ok(Self {
            store,
            is_online: Arc::new(RwLock::new(true)),
        })
    }

    /// Check if online
    pub async fn is_online(&self) -> bool {
        *self.is_online.read().await
    }

    /// Set online/offline status
    pub async fn set_online(&self, online: bool) {
        let mut status = self.is_online.write().await;
        *status = online;

        if online {
            info!("Switched to online mode - using remote vector store");
        } else {
            warn!("Switched to offline mode - using local vector store");
        }
    }

    /// Auto-detect and switch mode
    pub async fn auto_detect_mode(&self) {
        let online = Self::check_connectivity().await;
        self.set_online(online).await;
    }

    /// Check remote connectivity
    async fn check_connectivity() -> bool {
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reqwest::get("https://www.google.com/generate_204"),
        ).await {
            Ok(Ok(resp)) => resp.status().is_success(),
            _ => false,
        }
    }

    /// Get vector store reference
    pub fn store(&self) -> Arc<LocalVectorStore> {
        self.store.clone()
    }

    /// Hybrid search: try remote first, fallback to local
    pub async fn hybrid_search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        min_score: f32,
    ) -> Result<Vec<VectorSearchResult>, Box<dyn std::error::Error>> {
        if self.is_online().await {
            // Try remote search (placeholder - would call pgvector/Milvus)
            match self.remote_search(query_embedding, top_k, min_score).await {
                Ok(results) => {
                    debug!("Remote search succeeded: {} results", results.len());
                    return Ok(results);
                }
                Err(e) => {
                    warn!("Remote search failed, falling back to local: {}", e);
                    self.set_online(false).await;
                }
            }
        }

        // Fallback to local search
        debug!("Using local vector store");
        self.store.search(query_embedding, top_k, min_score).await
    }

    /// Placeholder for remote search
    async fn remote_search(
        &self,
        _query_embedding: &[f32],
        _top_k: usize,
        _min_score: f32,
    ) -> Result<Vec<VectorSearchResult>, Box<dyn std::error::Error>> {
        // TODO: Implement actual remote search via pgvector/Milvus
        Err("Remote search not implemented".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_vector_store_add_search() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("vector_store.db");

        let config = VectorStoreConfig {
            dimension: 3,
            db_path,
            ..Default::default()
        };

        let store = LocalVectorStore::new(config).await.unwrap();

        // Add documents
        let doc1 = VectorDocument {
            id: "doc1".to_string(),
            content: "Hello world".to_string(),
            embedding: vec![1.0, 0.0, 0.0],
            metadata: serde_json::json!({"source": "test"}),
            created_at: chrono::Utc::now().timestamp(),
        };

        let doc2 = VectorDocument {
            id: "doc2".to_string(),
            content: "Goodbye world".to_string(),
            embedding: vec![0.0, 1.0, 0.0],
            metadata: serde_json::json!({"source": "test"}),
            created_at: chrono::Utc::now().timestamp(),
        };

        store.add(doc1).await.unwrap();
        store.add(doc2).await.unwrap();

        assert_eq!(store.len().await, 2);

        // Search
        let query = vec![1.0, 0.1, 0.0];
        let results = store.search(&query, 2, 0.5).await.unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "doc1");
        assert!(results[0].score > results[1].score);
    }
}
