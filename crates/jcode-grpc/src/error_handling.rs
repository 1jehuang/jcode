//! Enhanced Error Handling for gRPC Service
//!
//! This module provides comprehensive error handling with rich metadata
//! for the LLM gRPC service, including error codes, retry information,
//! and detailed diagnostic data.

use tonic::{Status, Code};
use tracing::{error, warn};
use std::collections::HashMap;
use chrono::Utc;
use uuid::Uuid;

/// Error codes for LLM service
#[derive(Debug, Clone, Copy)]
pub enum LlmErrorCode {
    /// Authentication failed
    AuthenticationFailed,
    /// Rate limited
    RateLimited,
    /// Provider unavailable
    ProviderUnavailable,
    /// Model not found
    ModelNotFound,
    /// Invalid request
    InvalidRequest,
    /// Context too long
    ContextTooLong,
    /// Token limit exceeded
    TokenLimitExceeded,
    /// Streaming error
    StreamingError,
    /// Timeout
    Timeout,
    /// Internal server error
    InternalError,
}

impl LlmErrorCode {
    pub fn code(&self) -> Code {
        match self {
            Self::AuthenticationFailed => Code::Unauthenticated,
            Self::RateLimited => Code::ResourceExhausted,
            Self::ProviderUnavailable => Code::Unavailable,
            Self::ModelNotFound => Code::NotFound,
            Self::InvalidRequest => Code::InvalidArgument,
            Self::ContextTooLong => Code::InvalidArgument,
            Self::TokenLimitExceeded => Code::ResourceExhausted,
            Self::StreamingError => Code::Internal,
            Self::Timeout => Code::DeadlineExceeded,
            Self::InternalError => Code::Internal,
        }
    }
    
    pub fn name(&self) -> &'static str {
        match self {
            Self::AuthenticationFailed => "AUTHENTICATION_FAILED",
            Self::RateLimited => "RATE_LIMITED",
            Self::ProviderUnavailable => "PROVIDER_UNAVAILABLE",
            Self::ModelNotFound => "MODEL_NOT_FOUND",
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::ContextTooLong => "CONTEXT_TOO_LONG",
            Self::TokenLimitExceeded => "TOKEN_LIMIT_EXCEEDED",
            Self::StreamingError => "STREAMING_ERROR",
            Self::Timeout => "TIMEOUT",
            Self::InternalError => "INTERNAL_ERROR",
        }
    }
    
    pub fn http_status(&self) -> u16 {
        match self {
            Self::AuthenticationFailed => 401,
            Self::RateLimited => 429,
            Self::ProviderUnavailable => 503,
            Self::ModelNotFound => 404,
            Self::InvalidRequest => 400,
            Self::ContextTooLong => 413,
            Self::TokenLimitExceeded => 429,
            Self::StreamingError => 500,
            Self::Timeout => 504,
            Self::InternalError => 500,
        }
    }
    
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited | 
            Self::ProviderUnavailable | 
            Self::Timeout |
            Self::StreamingError
        )
    }
    
    pub fn suggested_retry_delay_ms(&self) -> u64 {
        match self {
            Self::RateLimited => 1000,
            Self::ProviderUnavailable => 5000,
            Self::Timeout => 2000,
            Self::StreamingError => 1000,
            _ => 0,
        }
    }
}

/// Error metadata for enhanced diagnostics
#[derive(Debug, Clone)]
pub struct ErrorMetadata {
    /// Unique error ID for tracking
    pub error_id: String,
    
    /// Error timestamp
    pub timestamp: i64,
    
    /// Error code
    pub error_code: LlmErrorCode,
    
    /// Human-readable message
    pub message: String,
    
    /// Detailed technical details (optional)
    pub details: Option<String>,
    
    /// Provider that caused the error (if applicable)
    pub provider: Option<String>,
    
    /// Model that was being used (if applicable)
    pub model: Option<String>,
    
    /// Whether this error is retryable
    pub retryable: bool,
    
    /// Suggested retry delay in milliseconds
    pub retry_after_ms: Option<u64>,
    
    /// Additional context as key-value pairs
    pub context: HashMap<String, String>,
}

impl ErrorMetadata {
    pub fn new(error_code: LlmErrorCode, message: impl Into<String>) -> Self {
        let code = error_code;
        Self {
            error_id: format!("err_{}", Uuid::new_v4()),
            timestamp: Utc::now().timestamp(),
            error_code: code,
            message: message.into(),
            details: None,
            provider: None,
            model: None,
            retryable: code.is_retryable(),
            retry_after_ms: if code.is_retryable() { 
                Some(code.suggested_retry_delay_ms()) 
            } else { 
                None 
            },
            context: HashMap::new(),
        }
    }
    
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
    
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }
    
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
    
    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }
    
    /// Convert to gRPC Status with rich metadata (metadata in message)
    pub fn to_grpc_status(&self) -> Status {
        // Build detailed error message with metadata
        let error_details = format!(
            "[{}] {} | ID: {} | Provider: {:?} | Model: {:?} | Retryable: {}",
            self.error_code.name(),
            self.message,
            self.error_id,
            self.provider,
            self.model,
            self.retryable
        );
        
        let mut status = Status::new(
            self.error_code.code(),
            error_details
        );
        
        status
    }
    
    /// Log error with full metadata
    pub fn log(&self) {
        let level = match self.error_code {
            LlmErrorCode::AuthenticationFailed | LlmErrorCode::InvalidRequest => tracing::Level::WARN,
            _ => tracing::Level::ERROR,
        };
        
        if level == tracing::Level::ERROR {
            error!(
                error_id = %self.error_id,
                error_code = %self.error_code.name(),
                provider = ?self.provider,
                model = ?self.model,
                retryable = %self.retryable,
                details = ?self.details,
                "{}",
                self.message
            );
        } else {
            warn!(
                error_id = %self.error_id,
                error_code = %self.error_code.name(),
                "{}",
                self.message
            );
        }
    }
}

/// Helper functions for creating common errors
pub mod errors {
    use super::*;
    
    pub fn authentication_error(message: impl Into<String>, provider: impl Into<String>) -> Status {
        let meta = ErrorMetadata::new(LlmErrorCode::AuthenticationFailed, message)
            .with_provider(provider);
        meta.log();
        meta.to_grpc_status()
    }
    
    pub fn rate_limited_error(message: impl Into<String>, retry_after_seconds: u64, provider: impl Into<String>) -> Status {
        let meta = ErrorMetadata::new(LlmErrorCode::RateLimited, message)
            .with_provider(provider)
            .with_context("retry-after-seconds", retry_after_seconds.to_string())
            .with_context("retry-after-ms", (retry_after_seconds * 1000).to_string());
        meta.log();
        meta.to_grpc_status()
    }
    
    pub fn provider_unavailable_error(message: impl Into<String>, provider: impl Into<String>) -> Status {
        let meta = ErrorMetadata::new(LlmErrorCode::ProviderUnavailable, message)
            .with_provider(provider);
        meta.log();
        meta.to_grpc_status()
    }
    
    pub fn model_not_found_error(model: impl Into<String>, provider: impl Into<String>) -> Status {
        let model_str = model.into();
        let meta = ErrorMetadata::new(LlmErrorCode::ModelNotFound, format!("Model '{}' not found", model_str))
            .with_provider(provider)
            .with_model(&model_str);
        meta.log();
        meta.to_grpc_status()
    }
    
    pub fn invalid_request_error(message: impl Into<String>, details: Option<impl Into<String>>) -> Status {
        let mut meta = ErrorMetadata::new(LlmErrorCode::InvalidRequest, message);
        if let Some(d) = details {
            meta = meta.with_details(d);
        }
        meta.log();
        meta.to_grpc_status()
    }
    
    pub fn context_too_long_error(context_length: usize, max_length: usize, model: impl Into<String>) -> Status {
        let model_str = model.into();
        let meta = ErrorMetadata::new(LlmErrorCode::ContextTooLong, "Context length exceeds maximum")
            .with_model(&model_str)
            .with_context("context-length", context_length.to_string())
            .with_context("max-length", max_length.to_string())
            .with_details(format!("Context has {} tokens, but {} allows a maximum of {}", context_length, model_str, max_length));
        meta.log();
        meta.to_grpc_status()
    }
    
    pub fn streaming_error(message: impl Into<String>, details: Option<impl Into<String>>) -> Status {
        let mut meta = ErrorMetadata::new(LlmErrorCode::StreamingError, message);
        if let Some(d) = details {
            meta = meta.with_details(d);
        }
        meta.log();
        meta.to_grpc_status()
    }
    
    pub fn timeout_error(operation: impl Into<String>, timeout_ms: u64) -> Status {
        let op_str = operation.into();
        let meta = ErrorMetadata::new(LlmErrorCode::Timeout, format!("Operation '{}' timed out", op_str))
            .with_context("operation", &op_str)
            .with_context("timeout-ms", timeout_ms.to_string());
        meta.log();
        meta.to_grpc_status()
    }
    
    pub fn internal_error(message: impl Into<String>, details: Option<impl Into<String>>) -> Status {
        let mut meta = ErrorMetadata::new(LlmErrorCode::InternalError, message);
        if let Some(d) = details {
            meta = meta.with_details(d);
        }
        meta.log();
        meta.to_grpc_status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_error_metadata_creation() {
        let meta = ErrorMetadata::new(LlmErrorCode::RateLimited, "Too many requests")
            .with_provider("deepseek")
            .with_model("deepseek-chat")
            .with_details("Rate limit: 60 requests per minute");
        
        assert_eq!(meta.error_code.name(), "RATE_LIMITED");
        assert!(meta.retryable);
        assert_eq!(meta.provider.as_deref(), Some("deepseek"));
        assert_eq!(meta.model.as_deref(), Some("deepseek-chat"));
    }
    
    #[test]
    fn test_error_to_grpc_status() {
        let meta = ErrorMetadata::new(LlmErrorCode::ModelNotFound, "Model not found")
            .with_model("nonexistent-model")
            .with_provider("vllm");
        
        let status = meta.to_grpc_status();
        
        assert_eq!(status.code(), Code::NotFound);
        assert!(!status.message().is_empty());
    }
    
    #[test]
    fn test_helper_functions() {
        let auth_err = errors::authentication_error("Invalid API key", "deepseek");
        assert_eq!(auth_err.code(), Code::Unauthenticated);
        
        let rate_err = errors::rate_limited_error("Rate limit exceeded", 30, "openai");
        assert_eq!(rate_err.code(), Code::ResourceExhausted);
        
        let model_err = errors::model_not_found_error("gpt-5", "openai");
        assert_eq!(model_err.code(), Code::NotFound);
    }
}
