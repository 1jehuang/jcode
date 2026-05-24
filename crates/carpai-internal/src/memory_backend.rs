//! Memory Backend — Enhanced memory with vector search, dedup, and tiered storage
//!
//! This module **extends** the base `MemoryStore` trait from `memory.rs`
//! with enterprise-grade capabilities:
//!
//! - **Vector similarity search** (embedding-based semantic retrieval)
//! - **Automatic deduplication** (near-duplicate detection before insert)
//! - **Tiered storage** (project-level vs global vs shared)
//! - **Confidence decay** (time-based relevance scoring)
//! - **Consolidation / reinforcement** (learning from corrections)
//!
//! ## Relationship to existing code
//!
//! ```
//! jcode-ui-types::MemoryEntry (existing)     MemoryBackend (this module)
//! ┌──────────────────────────────┐           ┌──────────────────────────┐
//! │ id, category, content, tags  │  maps to  │ store() / search()       │
//! │ embedding, confidence, ...   │ ───────>  │ vector_search()          │
//! │ strength, reinforcements    │           | consolidate()            │
//! └──────────────────────────────┘           └──────────────────────────┘
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

// Re-export base types
pub use super::memory::{
    MemoryStore, MemoryEntry, MemoryQuery, MemoryError,
    MemoryUpdate, MemoryStats, MemoryType,
};

// ========================================================================
// Enhanced Entry Types
// ========================================================================

/// Enhanced memory entry with enterprise features
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedMemoryEntry {
    /// Base fields from original MemoryEntry
    #[serde(flatten)]
    pub base: MemoryEntry,

    /// Confidence score (0.0-1.0), decays over time
    #[serde(default = "default_confidence")]
    pub confidence: f32,

    /// How many times this memory has been reinforced
    #[serde(default)]
    pub strength: u32,

    /// Whether this memory is active (not superseded)
    #[serde(default = "default_active")]
    pub active: bool,

    /// ID of a newer memory that supersedes this one
    #[serde(default)]
    pub superseded_by: Option<String>,

    /// Provenance breadcrumbs of when/where this was reinforced
    #[serde(default)]
    pub reinforcements: Vec<Reinforcement>,

    /// Scope of this memory
    #[serde(default)]
    pub scope: MemoryScope,

    /// Trust level
    #[serde(default)]
    pub trust: TrustLevel,

    /// Source session ID (for cross-session learning)
    #[serde(default)]
    pub source_session: Option<String>,
}

fn default_confidence() -> f32 { 1.0 }
fn default_active() -> bool { true }

/// Scope of a memory — who can see it
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum MemoryScope {
    #[default]
    Project,
    Global,
    All,
}
impl MemoryScope {
    pub fn includes_project(self) -> bool { matches!(self, Self::Project | Self::All) }
    pub fn includes_global(self) -> bool { matches!(self, Self::Global | Self::All) }
}

/// Trust level for memories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TrustLevel { High, #[default] Medium, Low, }

/// A reinforcement breadcrumb
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reinforcement {
    pub session_id: String,
    pub message_index: usize,
    pub timestamp: DateTime<Utc>,
}

// ========================================================================
// The Enhanced Trait
// ========================================================================

/// Enterprise-grade memory backend with vector search and dedup
///
/// Implementations:
/// - **LocalMemoryBackend** (`carpai-cli`): File-based JSON + local vector index
/// - **PgVectorMemoryBackend** (`carpai-server`): PostgreSQL + pgvector
/// - **RedisMemoryBackend** (`carpai-server`): Redis + RediSearch + vector
/// - **InMemoryMemoryBackend**: HashMap-backed, for testing
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    // --- Base operations ---

    /// Store a new memory entry (with automatic dedup check)
    async fn store(&self, entry: EnhancedMemoryEntry) -> Result<String, MemoryError>;

    /// Retrieve by ID
    async fn retrieve(&self, id: &str) -> Result<Option<EnhancedMemoryEntry>, MemoryError>;

    /// Search by text query and/or filters
    async fn search(&self, query: &EnhancedMemoryQuery) -> Result<Vec<EnhancedMemoryEntry>, MemoryError>;

    /// Delete by ID
    async fn delete(&self, id: &str) -> Result<(), MemoryError>;

    /// Update an existing entry
    async fn update(&self, id: &str, updates: &EnhancedMemoryUpdate) -> Result<EnhancedMemoryEntry, MemoryError>;

    // --- Vector Operations ---

    /// Vector similarity search using embeddings
    async fn vector_search(
        &self,
        embedding: &[f32],
        limit: usize,
        options: &VectorSearchOptions,
    ) -> Result<Vec<VectorSearchResult>, MemoryError>;

    /// Store or update an embedding for a memory entry
    async fn upsert_embedding(&self, memory_id: &str, embedding: Vec<f32>) -> Result<(), MemoryError>;

    // --- Dedup & Consolidation ---

    /// Check if a similar memory already exists (before storing)
    async fn find_duplicate(&self, content: &str, threshold: f32) -> Result<Option<String>, MemoryError>;

    /// Reinforce a memory (confirm it was useful in context)
    async fn reinforce(&self, id: &str, session_id: &str, message_index: usize) -> Result<(), MemoryError>;

    /// Consolidate multiple memories into one
    async fn consolidate(&self, primary_id: &str, merge_ids: &[String]) -> Result<EnhancedMemoryEntry, MemoryError>;

    // --- Scoped Access ---

    /// Get memories within a specific scope
    async fn get_by_scope(&self, scope: MemoryScope, project_id: Option<&str>, limit: usize)
        -> Result<Vec<EnhancedMemoryEntry>, MemoryError>;

    // --- Statistics ---

    /// Get enhanced statistics
    async fn stats(&self, scope: Option<MemoryScope>) -> Result<EnhancedMemoryStats, MemoryError>;

    /// Cleanup expired/low-confidence entries
    async fn cleanup(&self, options: &CleanupOptions) -> Result<CleanupResult, MemoryError>;
}

// ========================================================================
// Query Types
// ========================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnhancedMemoryQuery {
    pub text_query: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub scope: Option<MemoryScope>,
    pub memory_type: Option<MemoryType>,
    pub min_trust: Option<TrustLevel>,
    #[serde(default = "default_true")]
    pub active_only: bool,
    pub metadata_filter: Option<HashMap<String, String>>,
    pub tags: Option<Vec<String>>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub min_similarity: Option<f32>,
    pub sort_by: MemorySortField,
    #[serde(default)]
    pub sort_desc: bool,
}
fn default_true() -> bool { true }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MemorySortField { #[default] Relevance, CreatedAt, UpdatedAt, Confidence, Strength, AccessCount, }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VectorSearchOptions {
    pub min_similarity: f32,
    pub limit: usize,
    pub scope_filter: Option<MemoryScope>,
    #[serde(default)]
    pub include_inactive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResult {
    pub memory_id: String,
    pub similarity: f64,
    pub entry: EnhancedMemoryEntry,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnhancedMemoryUpdate {
    pub content: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
    pub tags: Option<Vec<String>>,
    pub scope: Option<MemoryScope>,
    pub trust: Option<TrustLevel>,
    pub active: Option<bool>,
}

// ========================================================================
// Statistics & Cleanup
// ========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedMemoryStats {
    pub total_count: usize,
    pub count_by_scope: HashMap<MemoryScope, usize>,
    pub count_by_type: HashMap<MemoryType, usize>,
    pub count_by_trust: HashMap<TrustLevel, usize>,
    pub avg_confidence: f32,
    pub storage_size_bytes: u64,
    pub stale_count: usize,
    pub superseded_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CleanupOptions {
    /// Expire memories older than this (None = no age limit)
    pub older_than: Option<DateTime<Utc>>,
    /// Prune memories with confidence below this threshold
    pub below_confidence: Option<f32>,
    /// Maximum number of entries to clean up
    pub max_prune: Option<usize>,
    /// Whether to actually delete or just mark superseded
    pub hard_delete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupResult {
    pub pruned_count: usize,
    pub superseded_count: usize,
    pub freed_bytes: u64,
    pub errors: Vec<String>,
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enhanced_entry_serialization() {
        let entry = EnhancedMemoryEntry {
            base: MemoryEntry {
                id: "mem-1".into(),
                content: "Use Rust for performance".into(),
                embedding: None,
                memory_type: MemoryType::CodePattern,
                metadata: HashMap::new(),
                created_at: Utc::now(),
                expires_at: None,
                access_count: 5,
                last_accessed: Utc::now(),
            },
            confidence: 0.9,
            strength: 3,
            active: true,
            superseded_by: None,
            reinforcements: vec![],
            scope: MemoryScope::Global,
            trust: TrustLevel::High,
            source_session: Some("sess-1".into()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("mem-1"));
        assert!(json.contains("High"));
        assert!(json.contains("Global"));
    }

    #[test]
    fn test_memory_scope_includes() {
        assert!(MemoryScope::All.includes_project());
        assert!(MemoryScope::All.includes_global());
        assert!(MemoryScope::Project.includes_project());
        assert!(!MemoryScope::Project.includes_global());
    }

    #[test]
    fn test_cleanup_result_default() {
        let result = CleanupResult::default();
        assert_eq!(result.pruned_count, 0);
        assert!(result.errors.is_empty());
    }
}
