//! Memory Store Trait - Unified memory persistence interface
//!
//! Supports:
//! - Short-term working memory
//! - Long-term knowledge storage
//! - Vector similarity search
//! - Memory expiration and cleanup

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

/// Main memory store trait
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Store a memory entry
    async fn store(&self, entry: MemoryEntry) -> Result<String, MemoryError>;

    /// Retrieve a memory by ID
    async fn retrieve(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryError>;

    /// Search memories by query (semantic or keyword)
    async fn search(&self, query: MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Delete a memory entry
    async fn delete(&self, id: &str) -> Result<(), MemoryError>;

    /// Update an existing memory
    async fn update(&self, id: &str, updates: MemoryUpdate) -> Result<MemoryEntry, MemoryError>;

    /// Get memory statistics
    async fn stats(&self) -> Result<MemoryStats, MemoryError>;

    /// Cleanup expired memories
    async fn cleanup_expired(&self) -> Result<usize, MemoryError>;
}

/// Memory entry structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier
    pub id: String,

    /// Memory content/text
    pub content: String,

    /// Optional: vector embedding for semantic search
    pub embedding: Option<Vec<f32>>,

    /// Memory type/category
    pub memory_type: MemoryType,

    /// Associated metadata
    pub metadata: HashMap<String, String>,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Expiration timestamp (None = permanent)
    pub expires_at: Option<DateTime<Utc>>,

    /// Access count (for LRU/LFU caching)
    pub access_count: u64,

    /// Last accessed timestamp
    pub last_accessed: DateTime<Utc>,
}

/// Memory type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryType {
    /// Short-term working memory
    ShortTerm,

    /// Long-term persistent knowledge
    LongTerm,

    /// User preferences/settings
    Preference,

    /// Code patterns/snippets
    CodePattern,

    /// Conversation history
    Conversation,

    /// Learned behaviors
    LearnedBehavior,
}

/// Memory search query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    /// Text search query
    pub text_query: Option<String>,

    /// Optional: vector embedding for semantic search
    pub embedding: Option<Vec<f32>>,

    /// Filter by memory type
    pub memory_type: Option<MemoryType>,

    /// Filter by metadata key-value pairs
    pub metadata_filter: Option<HashMap<String, String>>,

    /// Time range filter (from)
    pub created_after: Option<DateTime<Utc>>,

    /// Time range filter (to)
    pub created_before: Option<DateTime<Utc>>,

    /// Maximum results to return
    pub limit: Option<usize>,

    /// Minimum similarity score (0.0 - 1.0)
    pub min_similarity: Option<f32>,
}

/// Memory update fields (partial update)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUpdate {
    /// New content (optional)
    pub content: Option<String>,

    /// New metadata (merged with existing)
    pub metadata: Option<HashMap<String, String>>,

    /// New expiration time
    pub expires_at: Option<Option<DateTime<Utc>>>,
}

/// Memory statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    /// Total number of memories
    pub total_count: usize,

    /// Count by type
    pub count_by_type: HashMap<MemoryType, usize>,

    /// Storage size in bytes
    pub storage_size_bytes: u64,

    /// Number of expired memories pending cleanup
    pub expired_count: usize,

    /// Cache hit rate (0.0 - 1.0)
    pub cache_hit_rate: f64,
}

/// Memory error types
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Memory not found: {0}")]
    NotFound(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Invalid embedding dimension: expected {expected}, got {actual}")]
    InvalidEmbeddingDimension { expected: usize, actual: usize },

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_entry_creation() {
        let entry = MemoryEntry {
            id: "test-1".to_string(),
            content: "Test memory content".to_string(),
            embedding: None,
            memory_type: MemoryType::ShortTerm,
            metadata: HashMap::new(),
            created_at: Utc::now(),
            expires_at: None,
            access_count: 0,
            last_accessed: Utc::now(),
        };

        assert_eq!(entry.id, "test-1");
        assert_eq!(entry.memory_type, MemoryType::ShortTerm);
    }
}
