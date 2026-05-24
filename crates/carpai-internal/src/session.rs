//! Session Store Trait - Unified session persistence interface
//!
//! Abstracts over:
//! - Local file-based session storage (CLI mode, JSONL journals)
//! - PostgreSQL/Redis session storage (Server mode, multi-tenant)
//! - In-memory session cache (ephemeral / testing)
//!
//! ## Design Principles
//!
//! 1. **CRUD + State Machine**: Sessions have a lifecycle (Active → Paused → Archived → Deleted).
//!    The trait exposes both data operations and state transitions.
//!
//! 2. **Message-Append-Only**: Messages are always appended (immutable audit trail).
//!    Compaction is an explicit operation that produces a new snapshot.
//!
//! 3. **Multi-product support**:
//!    - `carpai-cli` → `LocalFileSessionStore` (JSONL on disk, existing `src/session.rs`)
//!    - `carpai-server` → `PgSessionStore` / `RedisSessionStore` (shared DB)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

// ========================================================================
// Core Types
// ========================================================================

/// Unique session identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self { SessionId(s) }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self { SessionId(s.to_string()) }
}

/// Session lifecycle states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SessionState {
    /// Active session — accepting messages
    #[default]
    Active,
    /// Paused — preserved but not accepting input
    Paused,
    /// Archived — read-only, compacted
    Archived,
    /// Deleted — soft-deleted, pending purge
    Deleted,
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Archived => write!(f, "archived"),
            Self::Deleted => write!(f, "deleted"),
        }
    }
}

/// Session metadata (lightweight, queried often)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub parent_id: Option<SessionId>,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_active_at: Option<DateTime<Utc>>,
    pub state: SessionState,
    pub model: Option<String>,
    pub working_dir: Option<String>,
    pub message_count: usize,
    /// User/tenant owner
    pub owner_id: Option<String>,
    /// Custom key-value tags
    pub tags: HashMap<String, String>,
}

/// A single stored message within a session
///
/// This is the **persisted** representation — agnostic to LLM provider format.
/// The agent runtime converts between this and provider-specific formats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    /// Unique message ID (UUID or monotonic counter)
    pub id: String,

    /// Message role: system / user / assistant / tool
    pub role: MessageRole,

    /// Content blocks (text, tool_use, tool_result, etc.)
    pub content: Vec<ContentBlock>,

    /// Timestamp when this message was recorded
    pub timestamp: DateTime<Utc>,

    /// Token usage for this message (assistant only)
    pub token_usage: Option<TokenUsage>,

    /// Optional model that generated this (assistant only)
    pub model: Option<String>,
}

/// Message role
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Content block within a message (multi-modal / multi-part)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentBlock {
    Text { text: String },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Thinking/reasoning block (for models with extended thinking)
    Thinking { text: String, signature: Option<String> },
}

/// Token usage per message
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub total_tokens: usize,
}

/// Compaction snapshot — replaces old messages with a summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSnapshot {
    /// Point-in-time of compaction
    pub compacted_at: DateTime<Utc>,
    /// Number of messages before compaction
    pub original_message_count: usize,
    /// Summary text prepended to message list
    pub system_summary: String,
    /// Tail messages retained after the summary
    pub retained_message_ids: Vec<String>,
}

// ========================================================================
// Query & Filter types
// ========================================================================

/// Filter parameters for listing sessions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionFilter {
    /// Only sessions owned by this user/tenant
    pub owner_id: Option<String>,
    /// Only sessions in this state
    pub state: Option<SessionState>,
    /// Only sessions with this model
    pub model: Option<String>,
    /// Only sessions active after this time
    pub active_after: Option<DateTime<Utc>>,
    /// Only sessions active before this time
    pub active_before: Option<DateTime<Utc>>,
    /// Tag filter (key=value)
    pub tag_filter: Option<(String, String)>,
    /// Maximum results
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
    /// Sort order
    pub sort_by: SessionSortField,
    /// Ascending or descending
    pub sort_desc: bool,
}

/// Sort field for session listing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SessionSortField {
    #[default]
    UpdatedAt,
    CreatedAt,
    LastActiveAt,
    Title,
    MessageCount,
}

// ========================================================================
// The Trait
// ========================================================================

/// Unified session persistence backend
///
/// Implementations:
/// - **LocalFileSessionStore** (`carpai-cli`): JSONL files on disk, mirrors `src/session.rs`
/// - **PgSessionStore** (`carpai-server`): PostgreSQL rows, multi-tenant safe
/// - **InMemorySessionStore**: HashMap-backed, for testing and ephemeral use
#[async_trait]
pub trait SessionStore: Send + Sync {
    // --- CRUD ---

    /// Create a new session with initial metadata
    async fn create_session(
        &self,
        meta: SessionMeta,
    ) -> Result<SessionId, SessionError>;

    /// Load full session (metadata + all messages)
    async fn load_session(
        &self,
        id: &SessionId,
    ) -> Result<Option<LoadedSession>, SessionError>;

    /// Update session metadata (title, state, working_dir, etc.)
    async fn update_meta(
        &self,
        id: &SessionId,
        updates: SessionMetaUpdate,
    ) -> Result<(), SessionError>;

    /// Delete a session (soft-delete by default; hard-delete via flag)
    async fn delete_session(
        &self,
        id: &SessionId,
        hard: bool,
    ) -> Result<(), SessionError>;

    // --- Messages ---

    /// Append one or more messages to a session (atomic)
    ///
    /// Implementations MUST:
    /// 1. Validate session exists and is not archived/deleted
    /// 2. Assign IDs if not provided
    /// 3. Set timestamps
    /// 4. Persist atomically (all or nothing)
    /// 5. Update `updated_at` / `last_active_at`
    async fn append_messages(
        &self,
        session_id: &SessionId,
        messages: Vec<StoredMessage>,
    ) -> Result<Vec<String>, SessionError>;

    /// Get messages in range [offset .. offset+limit)
    async fn get_messages(
        &self,
        session_id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredMessage>, SessionError>;

    /// Get total message count for a session
    async fn message_count(&self, session_id: &SessionId) -> Result<usize, SessionError>;

    // --- State Transitions ---

    /// Transition session state (validates transition legality)
    async fn set_state(
        &self,
        id: &SessionId,
        new_state: SessionState,
    ) -> Result<(), SessionError>;

    // --- Compaction ---

    /// Store a compaction snapshot (does NOT delete messages;
    /// caller decides what to prune)
    async fn save_compaction(
        &self,
        session_id: &SessionId,
        snapshot: CompactionSnapshot,
    ) -> Result<(), SessionError>;

    /// Load latest compaction snapshot if any
    async fn load_compaction(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<CompactionSnapshot>, SessionError>;

    // --- Listing / Search ---

    /// List sessions matching filter
    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, SessionError>;

    /// Count sessions matching filter (for pagination)
    async fn count_sessions(
        &self,
        filter: &SessionFilter,
    ) -> Result<usize, SessionError>;
}

// ========================================================================
// Supporting Types
// ========================================================================

/// A fully loaded session (metadata + messages + optional compaction)
#[derive(Debug, Clone)]
pub struct LoadedSession {
    pub meta: SessionMeta,
    pub messages: Vec<StoredMessage>,
    pub compaction: Option<CompactionSnapshot>,
}

/// Partial update for session metadata
///
/// Fields set to `Some(..)` will be updated; `None` means "keep as-is".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMetaUpdate {
    pub title: Option<String>,
    pub state: Option<SessionState>,
    pub model: Option<String>,
    pub working_dir: Option<String>,
    pub last_active_at: Option<DateTime<Utc>>,
    pub tags: Option<HashMap<String, String>>,
}

/// Session store error types
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found: {0}")]
    NotFound(String),

    #[error("Invalid state transition: {from} → {to}")]
    InvalidTransition { from: SessionState, to: SessionState },

    #[error("Session conflict: concurrent modification detected")]
    Conflict,

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Quota exceeded for owner {owner}: limit={limit}, current={current}")]
    QuotaExceeded { owner: String, limit: usize, current: usize },

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

// ========================================================================
// Valid state transitions
// ========================================================================

impl SessionState {
    /// Check if this transition is legal
    pub fn can_transition_to(&self, target: &SessionState) -> bool {
        matches!(
            (self, target),
            // Normal lifecycle
            (Self::Active, Self::Paused)
            | (Self::Active, Self::Archived)
            | (Self::Active, Self::Deleted)
            | (Self::Paused, Self::Active)
            | (Self::Paused, Self::Archived)
            | (Self::Paused, Self::Deleted)
            | (Self::Archived, Self::Deleted)
            // Recovery (admin-only in practice)
            | (Self::Deleted, Self::Active)
        )
    }

    /// Whether the session can accept new messages
    pub fn is_writable(&self) -> bool {
        matches!(self, Self::Active)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let valid = [
            (SessionState::Active, SessionState::Paused),
            (SessionState::Active, SessionState::Archived),
            (SessionState::Paused, SessionState::Active),
            (SessionState::Archived, SessionState::Deleted),
        ];
        for (from, to) in valid.iter() {
            assert!(from.can_transition_to(to), "{:?} → {:?} should be valid", from, to);
        }

        let invalid = [
            (SessionState::Archived, SessionState::Active),   // cannot un-archive normally
            (SessionState::Deleted, SessionState::Paused),    // deleted → paused illegal
        ];
        for (from, to) in invalid.iter() {
            assert!(!from.can_transition_to(to), "{:?} → {:?} should be invalid", from, to);
        }
    }

    #[test]
    fn test_session_id_display() {
        let id = SessionId("sess-abc123".into());
        assert_eq!(id.to_string(), "sess-abc123");
    }

    #[test]
    fn test_content_block_serialization() {
        let block = ContentBlock::Text { text: "hello".into() };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("text"));
    }
}
