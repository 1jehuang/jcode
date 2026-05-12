//! Common types used across CarpAI SDK

use serde::{Deserialize, Serialize};

/// Request ID wrapper
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub String);

impl RequestId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// Session ID for conversation continuity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// The prompt text
    pub prompt: String,

    /// Optional session ID for conversation continuity
    pub session_id: Option<SessionId>,

    /// Model to use (e.g., "gpt-4", "claude-3")
    pub model: Option<String>,

    /// Maximum tokens to generate
    pub max_tokens: Option<u32>,

    /// Temperature for randomness (0.0 - 2.0)
    pub temperature: Option<f64>,

    /// Stop sequences
    #[serde(default)]
    pub stop_sequences: Vec<String>,

    /// Top-p sampling parameter
    pub top_p: Option<f64>,

    /// Additional context or metadata
    #[serde(default)]
    pub context: CompletionContext,
}

/// Additional context for completion requests
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompletionContext {
    /// File path if completing code in a file
    pub file_path: Option<String>,

    /// Language of the code being written
    pub language: Option<String>,

    /// Cursor position (line, column)
    pub cursor_position: Option<(u32, u32)>,

    /// Surrounding code context
    pub surrounding_code: Option<String>,

    /// Project root path
    pub project_root: Option<String>,

    /// Custom metadata
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

/// Completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    /// Generated text
    pub text: String,

    /// Request ID
    pub request_id: RequestId,

    /// Session ID (for continuing conversation)
    pub session_id: Option<SessionId>,

    /// Model that generated the response
    pub model: String,

    /// Token usage information
    pub usage: TokenUsage,

    /// Latency in milliseconds
    pub latency_ms: f64,

    /// Whether the response was cached
    #[serde(default)]
    pub cached: bool,

    /// Finish reason (stop, length, etc.)
    pub finish_reason: Option<String>,
}

/// Token usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Number of prompt tokens
    pub prompt_tokens: u32,

    /// Number of completion tokens
    pub completion_tokens: u32,

    /// Total tokens used
    pub total_tokens: u32,
}

/// Chat message role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

/// Chat completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    /// Messages in the conversation
    pub messages: Vec<ChatMessage>,

    /// Model to use
    pub model: Option<String>,

    /// Maximum tokens to generate
    pub max_tokens: Option<u32>,

    /// Temperature
    pub temperature: Option<f64>,

    /// Additional parameters
    #[serde(default)]
    pub params: GenerationParams,
}

/// Generation parameters
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GenerationParams {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub presence_penalty: Option<f64>,
    pub stop_sequences: Vec<String>,
}

/// Chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// The assistant's message
    pub message: ChatMessage,

    /// Request ID
    pub request_id: RequestId,

    /// Model used
    pub model: String,

    /// Token usage
    pub usage: TokenUsage,

    /// Latency
    pub latency_ms: f64,

    /// Cached flag
    #[serde(default)]
    pub cached: bool,
}

/// Streaming chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// Content delta (partial text)
    pub content: Option<String>,

    /// Chunk index
    pub index: usize,

    /// Is this the final chunk?
    #[serde(default)]
    pub is_final: bool,

    /// Finish reason (only present in final chunk)
    pub finish_reason: Option<String>,

    /// Usage information (only present in final chunk)
    pub usage: Option<TokenUsage>,
}

/// Code action type
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CodeActionType {
    Explain,
    Refactor,
    FixBug,
    GenerateTests,
    Optimize,
    Document,
    Custom(String),
}

/// Code action request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActionRequest {
    /// Action type
    pub action_type: CodeActionType,

    /// Code to act on
    pub code: String,

    /// File path
    pub file_path: Option<String>,

    /// Language
    pub language: Option<String>,

    /// Selection range (start_line, start_col, end_line, end_col)
    pub selection: Option<(u32, u32, u32, u32)>,

    /// Additional instructions
    pub instruction: Option<String>,
}

/// Code action response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActionResponse {
    /// Result text (explanation, refactored code, etc.)
    pub result: String,

    /// Modified code (if applicable)
    pub modified_code: Option<String>,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,

    /// Request ID
    pub request_id: RequestId,
}

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    /// Service status
    pub status: HealthStatus,

    /// Server version
    pub version: Option<String>,

    /// Uptime in seconds
    pub uptime_secs: Option<u64>,

    /// Additional details
    #[serde(default)]
    pub details: std::collections::HashMap<String, String>,
}

/// Health status enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}
