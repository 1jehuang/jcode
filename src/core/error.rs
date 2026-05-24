//! Unified error hierarchy for CarpAI
//!
//! Provides a single error type that can be converted to HTTP status codes,
//! gRPC statuses, and structured JSON responses.

use thiserror::Error;
use std::time::Duration;

/// Authentication failure reasons
#[derive(Debug, Clone, PartialEq)]
pub enum AuthFailureReason {
    InvalidToken,
    TokenExpired,
    InsufficientPermissions,
    TenantMismatch,
}

impl std::fmt::Display for AuthFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidToken => write!(f, "invalid token"),
            Self::TokenExpired => write!(f, "token expired"),
            Self::InsufficientPermissions => write!(f, "insufficient permissions"),
            Self::TenantMismatch => write!(f, "tenant mismatch"),
        }
    }
}

/// Unified error type for all CarpAI operations
#[derive(Debug, Error)]
pub enum CarpAiError {
    #[error("Validation error: {message}")]
    ValidationError {
        message: String,
        field: String,
    },

    #[error("Authentication failed: {reason}")]
    AuthError {
        reason: AuthFailureReason,
    },

    #[error("Resource not found: {resource_type}/{id}")]
    NotFound {
        resource_type: String,
        id: String,
    },

    #[error("Rate limit exceeded")]
    RateLimited {
        retry_after: Duration,
    },

    #[error("Tenant access denied: requested={requested_tenant}")]
    TenantAccessDenied {
        requested_tenant: String,
    },

    #[error("Quota exceeded: {quota_type}")]
    QuotaExceeded {
        quota_type: String,
        limit: u64,
        current: u64,
    },

    #[error("Tool execution failed: {tool_name} - {error}")]
    ToolExecutionFailed {
        tool_name: String,
        error: String,
    },

    #[error("LLM inference failed: {provider} - {error}")]
    InferenceFailed {
        provider: String,
        error: String,
    },

    #[error("Database error: {message}")]
    DatabaseError {
        message: String,
        source: Option<anyhow::Error>,
    },

    #[error("Internal server error (trace_id={trace_id})")]
    Internal {
        #[source]
        source: anyhow::Error,
        trace_id: String,
    },
}

impl CarpAiError {
    /// Get a stable error code for this error type
    pub fn code(&self) -> &'static str {
        match self {
            Self::ValidationError { .. } => "VALIDATION_ERROR",
            Self::AuthError { .. } => "AUTH_FAILED",
            Self::NotFound { .. } => "NOT_FOUND",
            Self::RateLimited { .. } => "RATE_LIMITED",
            Self::TenantAccessDenied { .. } => "TENANT_ACCESS_DENIED",
            Self::QuotaExceeded { .. } => "QUOTA_EXCEEDED",
            Self::ToolExecutionFailed { .. } => "TOOL_EXECUTION_FAILED",
            Self::InferenceFailed { .. } => "INFERENCE_FAILED",
            Self::DatabaseError { .. } => "DATABASE_ERROR",
            Self::Internal { .. } => "INTERNAL_ERROR",
        }
    }

    /// Get the appropriate HTTP status code
    pub fn http_status(&self) -> u16 {
        match self {
            Self::ValidationError { .. } => 400,
            Self::AuthError { .. } => 401,
            Self::NotFound { .. } => 404,
            Self::RateLimited { .. } => 429,
            Self::TenantAccessDenied { .. } => 403,
            Self::QuotaExceeded { .. } => 429,
            Self::ToolExecutionFailed { .. } => 500,
            Self::InferenceFailed { .. } => 502,
            Self::DatabaseError { .. } => 500,
            Self::Internal { .. } => 500,
        }
    }

    /// Convert to gRPC status
    pub fn grpc_status(&self) -> tonic::Status {
        match self {
            Self::ValidationError { message, .. } => {
                tonic::Status::invalid_argument(message.clone())
            }
            Self::AuthError { reason } => {
                tonic::Status::unauthenticated(reason.to_string())
            }
            Self::NotFound { resource_type, id } => {
                tonic::Status::not_found(format!("{}/{}", resource_type, id))
            }
            Self::RateLimited { retry_after } => {
                let mut status = tonic::Status::resource_exhausted("rate limit exceeded");
                // Add retry-after metadata
                status.set_metadata(tonic::metadata::MetadataMap::new());
                status
            }
            Self::TenantAccessDenied { requested_tenant } => {
                tonic::Status::permission_denied(format!("access denied to tenant {}", requested_tenant))
            }
            Self::QuotaExceeded { quota_type, .. } => {
                tonic::Status::resource_exhausted(format!("quota exceeded: {}", quota_type))
            }
            Self::ToolExecutionFailed { error, .. } => {
                tonic::Status::internal(error.clone())
            }
            Self::InferenceFailed { error, .. } => {
                tonic::Status::unavailable(error.clone())
            }
            Self::DatabaseError { message, .. } => {
                tonic::Status::internal(message.clone())
            }
            Self::Internal { source, trace_id } => {
                let mut status = tonic::Status::internal(format!("internal error (trace_id={})", trace_id));
                status.set_source(source.clone());
                status
            }
        }
    }
}

/// Result type alias using CarpAiError
pub type Result<T> = std::result::Result<T, CarpAiError>;

// Implement From for common error types
impl From<anyhow::Error> for CarpAiError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal {
            source: err,
            trace_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

impl From<std::io::Error> for CarpAiError {
    fn from(err: std::io::Error) -> Self {
        Self::Internal {
            source: anyhow::anyhow!(err),
            trace_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

impl From<serde_json::Error> for CarpAiError {
    fn from(err: serde_json::Error) -> Self {
        Self::ValidationError {
            message: err.to_string(),
            field: "json".to_string(),
        }
    }
}
