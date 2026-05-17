//! Error types for CarpAI SDK

use thiserror::Error;

/// Result type alias for CarpAI operations
pub type Result<T> = std::result::Result<T, CarpAiError>;

/// Main error type for CarpAI SDK
#[derive(Error, Debug)]
pub enum CarpAiError {
    /// Configuration error
    #[error("Configuration error: {message}")]
    Config {
        message: String,
        #[source]
        source: Option<anyhow::Error>,
    },

    /// Connection error
    #[error("Connection failed: {message}")]
    Connection {
        message: String,
        endpoint: String,
        #[source]
        source: Option<reqwest::Error>,
    },

    /// Authentication error
    #[error("Authentication failed: {message}")]
    Auth {
        message: String,
        suggestion: Option<String>,
    },

    /// Rate limit exceeded
    #[error("Rate limit exceeded: retry after {retry_after_secs} seconds")]
    RateLimit {
        retry_after_secs: u64,
        current_limit: Option<u32>,
    },

    /// Server error
    #[error("Server error ({status}): {message}")]
    Server {
        status: u16,
        message: String,
        code: Option<String>,
        request_id: Option<String>,
    },

    /// Request timeout
    #[error("Request timed out after {timeout_secs} seconds")]
    Timeout {
        timeout_secs: u64,
        operation: String,
    },

    /// Invalid response
    #[error("Invalid response: {message}")]
    InvalidResponse {
        message: String,
        raw_response: Option<String>,
    },

    /// Streaming error
    #[error("Streaming error: {message}")]
    Streaming {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    /// Cache error
    #[error("Cache error: {message}")]
    Cache {
        message: String,
        #[source]
        source: Option<anyhow::Error>,
    },

    /// Offline mode error
    #[error("Offline mode: {message}")]
    Offline {
        message: String,
        queued: bool,
        suggestion: Option<String>,
    },

    /// Protocol error (gRPC/REST)
    #[error("Protocol error ({protocol}): {message}")]
    Protocol {
        protocol: String,
        message: String,
        #[source]
        source: Option<tonic::Status>,
    },

    /// Input validation error
    #[error("Validation error: {message}")]
    Validation {
        message: String,
        field: Option<String>,
        suggestion: Option<String>,
    },

    /// Feature not available
    #[error("Feature not available: {feature}")]
    FeatureNotAvailable {
        feature: String,
        requirement: Option<String>,
    },

    /// Internal error
    #[error("Internal error: {message}")]
    Internal {
        message: String,
        #[source]
        source: Option<anyhow::Error>,
    },
}

impl CarpAiError {
    /// Check if this error is recoverable (can be retried)
    ///
    /// Returns `true` for transient errors (network, timeout, rate limit, 5xx).
    /// Returns `false` for permanent errors (auth, validation, 4xx).
    ///
    /// # Examples
    ///
    /// ```
    /// use carpai_sdk::CarpAiError;
    ///
    /// // Transient errors are recoverable
    /// let timeout = CarpAiError::Timeout {
    ///     timeout_secs: 30,
    ///     operation: "completion".to_string(),
    /// };
    /// assert!(timeout.is_recoverable());
    ///
    /// // Auth errors need user intervention
    /// let auth = CarpAiError::Auth {
    ///     message: "Invalid API key".to_string(),
    ///     suggestion: None,
    /// };
    /// assert!(!auth.is_recoverable());
    /// ```
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::Connection { .. }
                | Self::RateLimit { .. }
                | Self::Timeout { .. }
                | Self::Server { status: 500..=599, .. }
                | Self::Offline { queued: true, .. }
        )
    }

    /// Get user-friendly recovery suggestion
    ///
    /// Provides actionable advice for resolving the error.
    ///
    /// # Examples
    ///
    /// ```
    /// use carpai_sdk::CarpAiError;
    ///
    /// let err = CarpAiError::RateLimit {
    ///     retry_after_secs: 60,
    ///     current_limit: Some(100),
    /// };
    ///
    /// let suggestion = err.recovery_suggestion();
    /// assert!(suggestion.is_some());
    /// assert!(suggestion.unwrap().contains("Wait"));
    /// ```
    pub fn recovery_suggestion(&self) -> Option<String> {
        match self {
            Self::Auth { suggestion, .. } => suggestion.clone(),
            Self::Offline { suggestion, .. } => suggestion.clone(),
            Self::Validation { suggestion, .. } => suggestion.clone(),
            Self::Connection { endpoint, .. } => Some(format!(
                "Check your network connection and ensure {} is reachable",
                endpoint
            )),
            Self::RateLimit { retry_after_secs, .. } => Some(format!(
                "Wait {} seconds before retrying",
                retry_after_secs
            )),
            Self::Timeout { operation, .. } => Some(format!(
                "Increase timeout for '{}' or check server performance",
                operation
            )),
            Self::Config { .. } => Some("Check your configuration file and environment variables".to_string()),
            _ => None,
        }
    }

    /// Get error code for programmatic handling
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Config { .. } => "CONFIG_ERROR",
            Self::Connection { .. } => "CONNECTION_ERROR",
            Self::Auth { .. } => "AUTH_ERROR",
            Self::RateLimit { .. } => "RATE_LIMIT",
            Self::Server { .. } => "SERVER_ERROR",
            Self::Timeout { .. } => "TIMEOUT",
            Self::InvalidResponse { .. } => "INVALID_RESPONSE",
            Self::Streaming { .. } => "STREAMING_ERROR",
            Self::Cache { .. } => "CACHE_ERROR",
            Self::Offline { .. } => "OFFLINE_ERROR",
            Self::Protocol { .. } => "PROTOCOL_ERROR",
            Self::Validation { .. } => "VALIDATION_ERROR",
            Self::FeatureNotAvailable { .. } => "FEATURE_NOT_AVAILABLE",
            Self::Internal { .. } => "INTERNAL_ERROR",
        }
    }

    /// Convert to a serializable error response
    pub fn to_error_response(&self) -> ErrorResponse {
        ErrorResponse {
            error_code: self.error_code().to_string(),
            message: self.to_string(),
            is_recoverable: self.is_recoverable(),
            suggestion: self.recovery_suggestion(),
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Serializable error response
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ErrorResponse {
    pub error_code: String,
    pub message: String,
    pub is_recoverable: bool,
    pub suggestion: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl From<reqwest::Error> for CarpAiError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Timeout {
                timeout_secs: 30,
                operation: "HTTP request".to_string(),
            }
        } else if err.is_connect() {
            Self::Connection {
                message: format!("Failed to connect: {}", err),
                endpoint: "unknown".to_string(),
                source: Some(err),
            }
        } else {
            Self::Internal {
                message: err.to_string(),
                source: Some(err.into()),
            }
        }
    }
}

impl From<tonic::Status> for CarpAiError {
    fn from(status: tonic::Status) -> Self {
        let code = status.code();
        match code {
            tonic::Code::Unauthenticated => Self::Auth {
                message: status.message().to_string(),
                suggestion: Some("Check your API key or authentication token".to_string()),
            },
            tonic::Code::Unavailable | tonic::Code::DeadlineExceeded => Self::Connection {
                message: status.message().to_string(),
                endpoint: "gRPC server".to_string(),
                source: None,
            },
            _ => Self::Protocol {
                protocol: "gRPC".to_string(),
                message: status.message().to_string(),
                source: Some(status),
            },
        }
    }
}

impl From<serde_json::Error> for CarpAiError {
    fn from(err: serde_json::Error) -> Self {
        Self::InvalidResponse {
            message: format!("JSON parsing error: {}", err),
            raw_response: None,
        }
    }
}
