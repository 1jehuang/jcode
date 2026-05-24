//! Inference Engine Trait - Unified LLM inference interface
//!
//! Abstracts over:
//! - Local model inference (llama.cpp, candle)
//! - Remote API calls (OpenAI, Anthropic, etc.)
//! - Distributed inference (multi-node clusters)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;

/// Main inference engine trait
#[async_trait]
pub trait InferenceEngine: Send + Sync {
    /// Generate text completion
    ///
    /// # Arguments
    /// * `request` - Inference request with prompt and parameters
    ///
    /// # Returns
    /// Generated text response
    async fn infer(&self, request: InferenceRequest) -> Result<InferenceResponse, InferenceError>;

    /// Stream tokens as they are generated
    async fn stream_infer(
        &self,
        request: InferenceRequest,
    ) -> Result<Pin<Box<dyn tokio_stream::Stream<Item = Result<TokenChunk, InferenceError>> + Send>>, InferenceError>;

    /// Get available models
    fn list_models(&self) -> Vec<ModelInfo>;

    /// Check engine health status
    fn health_check(&self) -> HealthStatus;

    /// Estimate token count for text
    fn estimate_tokens(&self, text: &str) -> usize;
}

/// Inference request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceRequest {
    /// Model identifier to use
    pub model: String,

    /// Input prompt/text
    pub prompt: String,

    /// Optional: system message (for chat models)
    pub system_message: Option<String>,

    /// Optional: conversation history
    pub messages: Option<Vec<Message>>,

    /// Maximum tokens to generate
    pub max_tokens: Option<usize>,

    /// Temperature (0.0 - 2.0)
    pub temperature: Option<f32>,

    /// Top-p sampling (0.0 - 1.0)
    pub top_p: Option<f32>,

    /// Stop sequences
    pub stop: Option<Vec<String>>,

    /// Optional: metadata for tracking/auditing
    pub metadata: Option<HashMap<String, String>>,
}

/// Chat message structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role: "system", "user", "assistant"
    pub role: String,

    /// Message content
    pub content: String,
}

/// Inference response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResponse {
    /// Generated text
    pub text: String,

    /// Model used
    pub model: String,

    /// Token usage statistics
    pub usage: TokenUsage,

    /// Finish reason
    pub finish_reason: FinishReason,

    /// Optional: confidence scores
    pub logprobs: Option<Vec<f32>>,
}

/// Token usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Prompt tokens
    pub prompt_tokens: usize,

    /// Completion tokens
    pub completion_tokens: usize,

    /// Total tokens
    pub total_tokens: usize,
}

/// Reason generation stopped
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    Length,
    ContentFilter,
    Error,
}

/// Single token chunk (for streaming)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenChunk {
    /// Generated text fragment
    pub text: String,

    /// Token index
    pub index: usize,

    /// Optional: log probability
    pub logprob: Option<f32>,
}

/// Model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier
    pub id: String,

    /// Model name (human-readable)
    pub name: String,

    /// Context window size
    pub context_length: usize,

    /// Supported capabilities
    pub capabilities: Vec<ModelCapability>,

    /// Is model currently loaded/available
    pub available: bool,
}

/// Model capability flags
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelCapability {
    TextGeneration,
    ChatCompletion,
    CodeCompletion,
    Embeddings,
    Vision,
    FunctionCalling,
}

/// Health check status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Overall status
    pub status: HealthState,

    /// Loaded models
    pub loaded_models: Vec<String>,

    /// Memory usage (MB)
    pub memory_usage_mb: f64,

    /// GPU utilization (%)
    pub gpu_utilization: Option<f64>,

    /// Uptime in seconds
    pub uptime_secs: u64,
}

/// Health state enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthState {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Inference error types
#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Model not loaded: {0}")]
    ModelNotLoaded(String),

    #[error("Context length exceeded: requested {requested}, max {max}")]
    ContextLengthExceeded { requested: usize, max: usize },

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("API error: {status} - {message}")]
    ApiError { status: u16, message: String },

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_usage_calculation() {
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };

        assert_eq!(usage.total_tokens, usage.prompt_tokens + usage.completion_tokens);
    }

    #[test]
    fn test_finish_reason_serialization() {
        let reason = FinishReason::Stop;
        let json = serde_json::to_string(&reason).unwrap();
        assert!(json.contains("Stop"));
    }
}
