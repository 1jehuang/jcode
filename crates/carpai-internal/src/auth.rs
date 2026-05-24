//! Authentication & Authorization Trait - Unified security interface
//!
//! Provides:
//! - Token verification and validation
//! - User information retrieval
//! - Permission checking
//! - API key management

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Main authentication provider trait
#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// Verify and validate an authentication token
    ///
    /// # Arguments
    /// * `token` - JWT or session token to verify
    ///
    /// # Returns
    /// User information if token is valid
    async fn verify_token(&self, token: &str) -> Result<UserInfo, AuthError>;

    /// Authenticate with username/password (returns token)
    async fn authenticate(&self, username: &str, password: &str) -> Result<AuthToken, AuthError>;

    /// Check if user has required permission
    async fn check_permission(&self, user_id: &str, permission: &Permission) -> Result<bool, AuthError>;

    /// Refresh an expiring token
    async fn refresh_token(&self, refresh_token: &str) -> Result<AuthToken, AuthError>;

    /// Revoke a token (logout/blacklist)
    async fn revoke_token(&self, token: &str) -> Result<(), AuthError>;

    /// Validate API key format and prefix
    fn validate_api_key_format(&self, api_key: &str) -> bool;
}

/// User information after successful authentication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    /// Unique user identifier
    pub user_id: String,

    /// Username or email
    pub username: String,

    /// Display name
    pub display_name: Option<String>,

    /// User roles/permissions
    pub roles: Vec<String>,

    /// Account tier (free/pro/enterprise)
    pub tier: UserTier,

    /// Token expiration timestamp (Unix epoch seconds)
    pub expires_at: u64,

    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// User account tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserTier {
    Free,
    Pro,
    Enterprise,
}

/// Authentication token (JWT or opaque)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    /// Access token
    pub access_token: String,

    /// Refresh token (long-lived)
    pub refresh_token: String,

    /// Token type (Bearer, etc.)
    pub token_type: String,

    /// Expiration in seconds
    pub expires_in: u64,
}

/// Permission types for authorization
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    /// Read access to resources
    Read(String),

    /// Write/modify access
    Write(String),

    /// Admin access (full control)
    Admin(String),

    /// Execute tools or commands
    Execute(String),

    /// Access enterprise features
    EnterpriseFeature(String),
}

/// Authentication error types
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Invalid token: {0}")]
    InvalidToken(String),

    #[error("Token expired")]
    TokenExpired,

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Insufficient permissions: required {0:?}")]
    InsufficientPermissions(Permission),

    #[error("Account suspended")]
    AccountSuspended,

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

/// API Key validator with prefix checking
pub struct ApiKeyValidator {
    /// Expected prefix (e.g., "carpai_")
    pub expected_prefix: String,

    /// Minimum key length (excluding prefix)
    pub min_length: usize,
}

impl ApiKeyValidator {
    pub fn new(prefix: &str, min_length: usize) -> Self {
        Self {
            expected_prefix: prefix.to_string(),
            min_length,
        }
    }

    /// Validate API key format
    pub fn validate(&self, api_key: &str) -> bool {
        // Check prefix
        if !api_key.starts_with(&self.expected_prefix) {
            return false;
        }

        // Extract key part after prefix
        let key_part = &api_key[self.expected_prefix.len()..];

        // Check minimum length
        if key_part.len() < self.min_length {
            return false;
        }

        // Check alphanumeric (no special chars except underscore/hyphen)
        key_part.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_validator() {
        let validator = ApiKeyValidator::new("carpai_", 32);

        // Valid key
        assert!(validator.validate("carpai_abc123def456ghi789jkl012mno345pq"));

        // Invalid: wrong prefix
        assert!(!validator.validate("other_abc123def456ghi789jkl012mno345pq"));

        // Invalid: too short
        assert!(!validator.validate("carpai_short"));

        // Invalid: special characters
        assert!(!validator.validate("carpai_abc@123!def456#ghi789$jkl012"));
    }
}
