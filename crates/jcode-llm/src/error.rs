//! Error types for LLM operations

use thiserror::Error;

/// Result type alias for LLM operations
pub type LlmResult<T> = std::result::Result<T, LlmError>;

/// Error type for LLM provider operations
#[derive(Error, Debug)]
pub enum LlmError {
    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// API key not found
    #[error("API key not found for environment variable: {0}")]
    ApiKeyNotFound(String),

    /// Network/HTTP request failed
    #[error("Request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    /// API returned an error response
    #[error("API error (status {status}): {message}")]
    ApiError {
        status: u16,
        message: String,
        code: Option<String>,
    },

    /// Invalid response from API
    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded. Retry after {retry_after_seconds}s")]
    RateLimited {
        retry_after_seconds: u64,
    },

    /// Context window exceeded
    #[error("Context window exceeded: {input_tokens} tokens exceeds limit of {max_tokens}")]
    ContextWindowExceeded {
        input_tokens: usize,
        max_tokens: usize,
    },

    /// Model not found or unavailable
    #[error("Model not available: {model_name}")]
    ModelNotFound {
        model_name: String,
    },

    /// Streaming error
    #[error("Streaming error: {0}")]
    StreamingError(String),

    /// Timeout
    #[error("Operation timed out after {timeout_secs}s")]
    Timeout {
        timeout_secs: u64,
    },

    /// Authentication failed
    #[error("Authentication failed")]
    AuthenticationFailed,

    /// Provider-specific error
    #[error("{provider} error: {message}")]
    ProviderError {
        provider: String,
        message: String,
    },

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl LlmError {
    /// Check if this is a retryable error
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } 
                | Self::Timeout { .. }
                | Self::RequestFailed(_)
        )
    }

    /// Get suggested retry delay in seconds (if applicable)
    pub fn retry_delay_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after_seconds } => Some(*retry_after_seconds),
            Self::Timeout { timeout_secs } => Some(*timeout_secs),
            _ => None,
        }
    }
}
