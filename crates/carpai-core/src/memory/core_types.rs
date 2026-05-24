//! Core Memory Types
//!
//! Defines the fundamental data structures for the memory system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Basic memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub metadata: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub scope: MemoryScope,
    pub trust_level: TrustLevel,
}

/// Enhanced memory entry with vector embeddings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedMemoryEntry {
    pub id: String,
    pub content: String,
    pub embedding: Option<Vec<f64>>,
    pub metadata: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub scope: MemoryScope,
    pub trust_level: TrustLevel,
    pub access_count: u64,
    pub last_accessed: Option<DateTime<Utc>>,
}

/// Memory query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub content_filter: Option<String>,
    pub scope: Option<MemoryScope>,
    pub min_trust_level: Option<TrustLevel>,
    pub limit: usize,
    pub offset: usize,
}

/// Enhanced memory query with vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedMemoryQuery {
    pub content_filter: Option<String>,
    pub embedding: Option<Vec<f64>>,
    pub similarity_threshold: f64,
    pub scope: Option<MemoryScope>,
    pub min_trust_level: Option<TrustLevel>,
    pub limit: usize,
    pub offset: usize,
}

/// Vector search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResult {
    pub entry_id: String,
    pub similarity_score: f64,
    pub content: String,
    pub metadata: HashMap<String, String>,
}

/// Memory type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryType {
    Episodic,   // Event-based memories
    Semantic,   // Fact-based knowledge
    Procedural, // How-to knowledge
    Working,    // Temporary working memory
}

/// Memory scope (visibility/lifetime)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryScope {
    Global,     // Available to all sessions/users
    User,       // Specific to a user
    Session,    // Specific to a session
    Temporary,  // Short-lived, cleared after use
}

/// Trust level for memory validation
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TrustLevel {
    Unverified = 0,
    Low = 1,
    Medium = 2,
    High = 3,
    Verified = 4,
}

impl Default for TrustLevel {
    fn default() -> Self {
        Self::Unverified
    }
}

/// Memory update operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryUpdate {
    UpdateContent { new_content: String },
    UpdateMetadata { key: String, value: String },
    UpdateTrustLevel { new_level: TrustLevel },
    IncrementAccess,
}

/// Reinforcement feedback for memory learning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reinforcement {
    pub memory_id: String,
    pub feedback_type: FeedbackType,
    pub strength: f64, // -1.0 to 1.0
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedbackType {
    Positive,
    Negative,
    Neutral,
}

/// Memory statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStats {
    pub total_entries: usize,
    pub by_type: HashMap<String, usize>,
    pub by_scope: HashMap<String, usize>,
    pub average_trust_level: f64,
    pub total_accesses: u64,
}

/// Cleanup options for memory management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupOptions {
    pub older_than_days: Option<u64>,
    pub min_access_count: Option<u64>,
    pub min_trust_level: Option<TrustLevel>,
    pub scopes_to_clean: Vec<MemoryScope>,
}

/// Cleanup result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupResult {
    pub entries_removed: usize,
    pub entries_archived: usize,
    pub space_freed_bytes: u64,
}
