//! Inference Backend — Enhanced inference with routing, quota, and multi-model
//!
//! This module **extends** the base `InferenceEngine` trait from `inference.rs`
//! with enterprise-grade capabilities:
//!
//! - **Model Routing**: Automatic provider selection based on cost/latency/capability
//! - **Quota Enforcement**: Per-user/per-tenant token limits
//! - **Fallback Chain**: Primary → Secondary → Tertiary model cascade
//! - **Cost Tracking**: Token usage accounting per request
//!
//! ## Relationship to base InferenceEngine
//!
//! ```
//! Base InferenceEngine (inference.rs)     InferenceBackend (this module)
//! ┌──────────────────────────┐           ┌─────────────────────────────┐
//! │ infer()                  │           │ complete_chat()             │
//! │ stream_infer()           │  embeds   │ complete_with_routing()     │
//! │ list_models()            │ ───────>  │ get_quota_usage()           │
//! │ health_check()           │           │ select_model()              │
//! │ estimate_tokens()        │           │ record_usage()              │
//! └──────────────────────────┘           └─────────────────────────────┘
//! ```
//!
//! ## Implementations
//!
//! | Product | Implementation | Behavior |
//! |---------|---------------|----------|
//! | `carpai-cli` | `SidecarInferenceBackend` | Wraps existing `src/sidecar.rs` |
//! | `carpai-server` | `RoutedInferenceBackend` | Multi-provider + auto-fallback + quota |

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// Re-export base types for convenience
pub use super::inference::{
    InferenceEngine, InferenceRequest, InferenceResponse,
    InferenceError, TokenUsage, ModelInfo, HealthStatus,
    HealthState, Message, FinishReason,
};

// ========================================================================
// Enhanced Request Types
// ========================================================================

/// Chat completion request (OpenAI-compatible format)
///
/// This is the primary request type for agent conversations.
/// It maps directly to OpenAI's `/v1/chat/completions` format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    /// Messages in the conversation
    pub messages: Vec<ChatMessage>,

    /// Model identifier (or "auto" for router selection)
    pub model: String,

    /// Max tokens to generate
    #[serde(default)]
    pub max_tokens: Option<usize>,

    /// Temperature (0.0 - 2.0)
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Top-p sampling (0.0 - 1.0)
    #[serde(default)]
    pub top_p: Option<f32>,

    /// Stop sequences
    #[serde(default)]
    pub stop: Option<Vec<String>>,

    /// Presence penalty (-2.0 to 2.0)
    #[serde(default)]
    pub presence_penalty: Option<f32>,

    /// Frequency penalty (-2.0 to 2.0)
    #[serde(default)]
    pub frequency_penalty: Option<f32>,

    /// Tool definitions for function calling
    #[serde(default)]
    pub tools: Option<Vec<ChatToolDefinition>>,

    /// Tool choice policy
    #[serde(default)]
    pub tool_choice: Option<ToolChoice>,

    /// User/tenant ID for quota tracking
    #[serde(default)]
    pub user_id: Option<String>,

    /// Session ID for conversation context
    #[serde(default)]
    pub session_id: Option<String>,

    /// Metadata for audit/routing
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Chat message (role + content + optional name)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: ChatContent,
    /// Optional name for function/results messages
    #[serde(default)]
    pub name: Option<String>,
}

/// Chat role
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatRole { System, User, Assistant, Tool, }

/// Content can be a string or array of parts (multi-modal)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl From<String> for ChatContent {
    fn from(s: String) -> Self { Self::Text(s) }
}

impl From<&str> for ChatContent {
    fn from(s: &str) -> Self { Self::Text(s.to_string()) }
}

/// A content part (for multi-part messages)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: ContentType,
    pub text: Option<String>,
}

/// Content part type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentType { Text, ImageUrl, }

/// Tool definition for function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolDefinition {
    /// Function definition
    #[serde(rename = "type")]
    pub tool_type: ToolType,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolType { #[default] Function, }

/// Function definition (JSON Schema based)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: serde_json::Value,
}

/// Tool choice policy
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    None,
    #[default]
    Auto,
    Required,
    Specific(String),
}

// ========================================================================
// Enhanced Response Types
// ========================================================================

/// Chat completion response (OpenAI-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    /// Unique response ID
    pub id: String,

    /// Object type ("chat.completion")
    pub object: String,

    /// Timestamp (Unix epoch)
    pub created: u64,

    /// Model that actually responded (may differ from requested)
    pub model: String,

    /// Response choices (usually one for non-streaming)
    pub choices: Vec<Choice>,

    /// Token usage
    pub usage: CompletionTokenUsage,

    /// Which provider was used (internal metadata)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Fallback info if the model was changed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_info: Option<FallbackInfo>,
}

/// Single choice in a response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// Index (always 0 for single-choice)
    pub index: usize,

    /// Message content
    pub message: ChatMessage,

    /// Finish reason
    pub finish_reason: FinishReason,

    /// Log probabilities (if requested)
    #[serde(default)]
    pub logprobs: Option<LogProbs>,
}

/// Log probabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogProbs {
    pub content: Vec<TokenLogProb>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenLogProb {
    pub token: String,
    pub logprob: f64,
    pub top_logprobs: Vec<TopLogProb>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopLogProb {
    pub token: String,
    pub logprob: f64,
}

/// Token usage for chat completions
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CompletionTokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    /// Cache creation tokens (for prompt caching providers like Anthropic)
    #[serde(default)]
    pub cache_creation_input_tokens: Option<usize>,
    /// Cache read tokens
    #[serde(default)]
    pub cache_read_input_tokens: Option<usize>,
}

// ========================================================================
// The Enhanced Trait
// ========================================================================

/// Enterprise-grade inference backend with routing and quota support
///
/// This trait wraps a base `InferenceEngine` and adds:
/// - Model routing / selection
/// - Quota enforcement
/// - Fallback chain management
/// - Cost tracking
#[async_trait]
pub trait InferenceBackend: Send + Sync {
    /// Complete a chat conversation (main entry point for agents)
    ///
    /// Handles:
    /// 1. Model selection (if model = "auto")
    /// 2. Quota check
    /// 3. Provider selection + fallback
    /// 4. Execution
    /// 5. Usage recording
    async fn complete_chat(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, InferenceError>;

    /// Stream a chat completion
    ///
    /// Returns a stream of `StreamChunk` events.
    async fn stream_chat(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<Box<dyn tokio_stream::Stream<Item = Result<StreamChunk, InferenceError>> + Send>, InferenceError>;

    /// Get available models with routing metadata
    async fn list_models_with_routing(&self) -> Result<Vec<RoutedModelInfo>, InferenceError>;

    /// Select best model for a given request (cost/latency optimization)
    async fn select_model(
        &self,
        constraints: &ModelSelectionConstraints,
    ) -> Result<String, InferenceError>;

    /// Check quota usage for a user/tenant
    async fn get_quota_usage(&self, user_id: &str) -> Result<QuotaUsage, InferenceError>;

    /// Record token usage after a successful completion
    async fn record_usage(
        &self,
        user_id: &str,
        usage: &CompletionTokenUsage,
        model: &str,
    ) -> Result<(), InferenceError>;

    /// Get the underlying base engine (for direct access if needed)
    fn base_engine(&self) -> Arc<dyn InferenceEngine>;
}

// ========================================================================
// Routing & Selection Types
// ========================================================================

/// Model info with routing metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutedModelInfo {
    /// Base model info
    pub model: ModelInfo,

    /// Provider backend(s) that serve this model
    pub providers: Vec<ModelProviderEntry>,

    /// Cost per 1K input tokens (USD)
    pub cost_per_1k_input: f64,

    /// Cost per 1K output tokens (USD)
    pub cost_per_1k_output: f64,

    /// Average latency in ms (rolling window)
    pub avg_latency_ms: f64,

    /// Success rate (0.0 - 1.0, rolling window)
    pub success_rate: f64,

    /// Priority for auto-selection (lower = higher priority)
    pub routing_priority: u32,

    /// Whether this model supports function calling
    pub supports_function_calling: bool,

    /// Whether this model supports extended thinking
    pub supports_thinking: bool,

    /// Context window size
    pub context_window: usize,
}

/// A provider entry for a model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProviderEntry {
    /// Provider name (e.g., "openai", "anthropic", "local")
    pub provider: String,

    /// Endpoint URL
    pub endpoint: Option<String>,

    /// Weight for load balancing
    pub weight: u32,

    /// Whether this provider is currently healthy
    pub healthy: bool,
}

/// Constraints for automatic model selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSelectionConstraints {
    /// Maximum cost in USD (None = no limit)
    pub max_cost_usd: Option<f64>,

    /// Maximum latency in ms (None = no limit)
    pub max_latency_ms: Option<u64>,

    /// Must support function calling
    pub require_function_calling: bool,

    /// Must support extended thinking
    pub require_thinking: bool,

    /// Minimum context window size
    pub min_context_window: Option<usize>,

    /// Preferred providers (empty = any)
    pub preferred_providers: Vec<String>,

    /// Exclude these models
    pub exclude_models: Vec<String>,

    /// User's tier (affects which models are available)
    pub user_tier: InferenceUserTier,
}

impl Default for ModelSelectionConstraints {
    fn default() -> Self {
        Self {
            max_cost_usd: None,
            max_latency_ms: None,
            require_function_calling: false,
            require_thinking: false,
            min_context_window: None,
            preferred_providers: vec![],
            exclude_models: vec![],
            user_tier: InferenceUserTier::Free,
        }
    }
}

/// User tier for model access control
/// Re-exported from auth module to avoid duplication
pub use super::auth::UserTier as InferenceUserTier;

// ========================================================================
// Quota Types
// ========================================================================

/// Current quota usage for a user/tenant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaUsage {
    /// User/tenant ID
    pub user_id: String,

    /// Tokens used in current billing period
    pub tokens_used: u64,

    /// Token limit for current period
    pub token_limit: u64,

    /// Requests made in current period
    pub requests_used: u64,

    /// Request limit for current period
    pub request_limit: u64,

    /// Period start
    pub period_start: chrono::DateTime<chrono::Utc>,

    /// Period end
    pub period_end: chrono::DateTime<chrono::Utc>,

    /// Reset time remaining (seconds)
    pub reset_in_secs: u64,
}

impl QuotaUsage {
    /// Whether the user has exceeded their token quota
    pub fn is_token_exceeded(&self) -> bool {
        self.tokens_used >= self.token_limit
    }

    /// Whether the user has exceeded their request quota
    pub fn is_request_exceeded(&self) -> bool {
        self.requests_used >= self.request_limit
    }

    /// Remaining tokens
    pub fn tokens_remaining(&self) -> u64 {
        self.token_limit.saturating_sub(self.tokens_used)
    }

    /// Fraction of quota used (0.0 - 1.0+)
    pub fn token_fraction(&self) -> f64 {
        if self.token_limit == 0 { return 1.0; }
        self.tokens_used as f64 / self.token_limit as f64
    }
}

// ========================================================================
// Streaming Types
// ========================================================================

/// A single chunk in a streamed response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// Chunk type
    pub chunk_type: StreamChunkType,

    /// Index of this choice (always 0 for single)
    pub index: usize,

    /// Text delta (for content chunks)
    pub delta: Option<String>,

    /// Finish reason (for final chunk)
    pub finish_reason: Option<FinishReason>,

    /// Cumulative token usage (for final chunk)
    pub usage: Option<CompletionTokenUsage>,
}

/// Type of streaming chunk
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamChunkType {
    /// Content text delta
    ContentDelta,
    /// Reasoning/thinking content
    ReasoningDelta,
    /// Final chunk with finish reason
    Finish,
    /// Error occurred during streaming
    Error,
}

// ========================================================================
// Fallback Types
// ========================================================================

/// Information about a fallback that occurred
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackInfo {
    /// Original requested model
    pub original_model: String,

    /// Model that actually served the request
    pub actual_model: String,

    /// Reason for fallback
    pub reason: FallbackReason,

    /// Number of fallback attempts before success
    pub attempts: u32,

    /// Total time spent on fallbacks (ms)
    pub total_fallback_ms: u64,
}

/// Why a fallback was triggered
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FallbackReason {
    /// Original model was overloaded
    Overloaded,
    /// Original model returned an error
    Error(String),
    /// Original model exceeded latency threshold
    LatencyExceeded,
    /// Original model was at capacity
    CapacityReached,
    /// Original model does not support required capability
    UnsupportedCapability,
    /// Quota exhausted for original model
    QuotaExhausted,
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_completion_request_serialization() {
        let req = ChatCompletionRequest {
            messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    content: ChatContent::Text("Hello".into()),
                    name: None,
                },
            ],
            model: "auto".into(),
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            tools: None,
            tool_choice: None,
            user_id: Some("user-1".into()),
            session_id: Some("sess-1".into()),
            metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("User"));
        assert!(json.contains("auto"));
    }

    #[test]
    fn test_quota_usage_checks() {
        let usage = QuotaUsage {
            user_id: "u1".into(),
            tokens_used: 90_000,
            token_limit: 100_000,
            requests_used: 900,
            request_limit: 1000,
            period_start: Utc::now(),
            period_end: Utc::now(),
            reset_in_secs: 3600,
        };
        assert!(!usage.is_token_exceeded());
        assert_eq!(usage.tokens_remaining(), 10_000);
        assert!((usage.token_fraction() - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_stream_chunk_types() {
        let content = StreamChunk {
            chunk_type: StreamChunkType::ContentDelta,
            index: 0,
            delta: Some("hello".into()),
            finish_reason: None,
            usage: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("ContentDelta"));
    }
}
