// Session CRUD API types for CarpAI SDK

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::types::{RequestId, SessionId, ChatMessage, TokenUsage};

/// Create a new session request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreateRequest {
    /// Optional session ID (auto-generated if not provided)
    pub id: Option<String>,

    /// Session title
    pub title: Option<String>,

    /// Working directory
    pub working_dir: Option<String>,

    /// Model to use for this session
    pub model: Option<String>,

    /// Initial messages
    #[serde(default)]
    pub messages: Vec<ChatMessage>,

    /// Custom metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Session response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    /// Session ID
    pub id: String,

    /// Session title
    pub title: Option<String>,

    /// Current state (active, archived, etc.)
    pub state: String,

    /// Model in use
    pub model: Option<String>,

    /// Working directory
    pub working_dir: Option<String>,

    /// Message count
    pub message_count: usize,

    /// Created timestamp (ISO 8601)
    pub created_at: String,

    /// Last updated timestamp
    pub updated_at: String,

    /// Metadata
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// List sessions request with pagination
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListRequest {
    /// Page number (starts at 1)
    pub page: Option<u32>,

    /// Page size (default 20, max 100)
    pub page_size: Option<u32>,

    /// Filter by state
    pub state: Option<String>,

    /// Search in title
    pub search: Option<String>,
}

/// List sessions response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListResponse {
    /// Sessions on this page
    pub sessions: Vec<SessionResponse>,

    /// Total number of sessions
    pub total: u32,

    /// Current page
    pub page: u32,

    /// Page size
    pub page_size: u32,

    /// Has more pages
    pub has_more: bool,
}

/// Append message to session request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageAppendRequest {
    /// Message to append
    pub message: ChatMessage,
}

/// Get session messages request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMessagesRequest {
    /// Maximum messages to return (default 50)
    pub limit: Option<u32>,

    /// Offset from most recent (default 0)
    pub offset: Option<u32>,
}

/// Get session messages response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMessagesResponse {
    /// Session ID
    pub session_id: String,

    /// Messages (most recent first)
    pub messages: Vec<ChatMessage>,

    /// Total message count
    pub total_count: usize,
}

/// Delete session response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteSessionResponse {
    /// Whether deletion was successful
    pub success: bool,

    /// Deleted session ID
    pub session_id: String,
}
