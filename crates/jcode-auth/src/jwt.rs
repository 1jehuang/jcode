//! JWT (JSON Web Token) management for authentication and authorization
//!
//! Supports RS256, ES256, and HS256 algorithms with configurable expiration.

use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum JwtError {
    #[error("JWT error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),

    #[error("Token expired")]
    TokenExpired,

    #[error("Invalid token claims")]
    InvalidClaims(String),

    #[error("Key generation error: {0}")]
    KeyGeneration(String),
}

pub type Result<T> = std::result::Result<T, JwtError>;

/// JWT claims structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject (user ID)
    pub sub: String,

    /// Issuer
    pub iss: String,

    /// Audience
    pub aud: Option<String>,

    /// Expiration time
    pub exp: i64,

    /// Not before
    pub nbf: i64,

    /// Issued at
    pub iat: i64,

    /// JWT ID
    pub jti: Option<String>,

    /// Custom claims
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

impl JwtClaims {
    /// Create new JWT claims with default values
    pub fn new(subject: String, issuer: String, expiration_hours: i64) -> Self {
        let now = Utc::now();
        let exp = now + Duration::hours(expiration_hours);

        Self {
            sub: subject,
            iss: issuer,
            aud: None,
            exp: exp.timestamp(),
            nbf: now.timestamp(),
            iat: now.timestamp(),
            jti: Some(uuid::Uuid::new_v4().to_string()),
            custom: HashMap::new(),
        }
    }

    /// Add a custom claim
    pub fn with_claim(mut self, key: &str, value: serde_json::Value) -> Self {
        self.custom.insert(key.to_string(), value);
        self
    }

    /// Check if token is expired
    pub fn is_expired(&self) -> bool {
        Utc::now().timestamp() >= self.exp
    }
}

/// Token validation result
#[derive(Debug, Clone)]
pub struct TokenValidation {
    pub claims: JwtClaims,
    pub is_valid: bool,
    pub error: Option<String>,
}

/// JWT algorithm type
#[derive(Debug, Clone)]
pub enum JwtAlgorithm {
    HS256, // HMAC with SHA-256
    RS256, // RSA with SHA-256
    ES256, // ECDSA with SHA-256
}

/// JWT manager for token operations
pub struct JwtManager {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    algorithm: JwtAlgorithm,
    issuer: String,
    default_expiration_hours: i64,
}

impl JwtManager {
    /// Create new JWT manager with HS256 (symmetric key)
    pub fn new_hs256(secret: &[u8], issuer: String, default_expiration_hours: i64) -> Result<Self> {
        Ok(Self {
            encoding_key: EncodingKey::from_secret(secret),
            decoding_key: DecodingKey::from_secret(secret),
            algorithm: JwtAlgorithm::HS256,
            issuer,
            default_expiration_hours,
        })
    }

    /// Create new JWT manager with RS256 (asymmetric key pair)
    pub fn new_rs256(
        private_key_pem: &[u8],
        public_key_pem: &[u8],
        issuer: String,
        default_expiration_hours: i64,
    ) -> Result<Self> {
        Ok(Self {
            encoding_key: EncodingKey::from_rsa_pem(private_key_pem)
                .map_err(|e| JwtError::KeyGeneration(e.to_string()))?,
            decoding_key: DecodingKey::from_rsa_pem(public_key_pem)
                .map_err(|e| JwtError::KeyGeneration(e.to_string()))?,
            algorithm: JwtAlgorithm::RS256,
            issuer,
            default_expiration_hours,
        })
    }

    /// Generate a new JWT token
    pub fn generate_token(&self, claims: JwtClaims) -> Result<String> {
        let header = match self.algorithm {
            JwtAlgorithm::HS256 => Header::new(jsonwebtoken::Algorithm::HS256),
            JwtAlgorithm::RS256 => Header::new(jsonwebtoken::Algorithm::RS256),
            JwtAlgorithm::ES256 => Header::new(jsonwebtoken::Algorithm::ES256),
        };

        let token = encode(&header, &claims, &self.encoding_key)?;
        Ok(token)
    }

    /// Validate and decode a JWT token
    pub fn validate_token(&self, token: &str) -> Result<TokenValidation> {
        let mut validation = Validation::new(match self.algorithm {
            JwtAlgorithm::HS256 => jsonwebtoken::Algorithm::HS256,
            JwtAlgorithm::RS256 => jsonwebtoken::Algorithm::RS256,
            JwtAlgorithm::ES256 => jsonwebtoken::Algorithm::ES256,
        });

        validation.set_issuer(&[&self.issuer]);
        validation.validate_exp = true;
        validation.validate_nbf = true;

        match decode::<JwtClaims>(token, &self.decoding_key, &validation) {
            Ok(token_data) => Ok(TokenValidation {
                claims: token_data.claims,
                is_valid: true,
                error: None,
            }),
            Err(e) => {
                let error_msg = e.to_string();
                Ok(TokenValidation {
                    claims: JwtClaims::new("unknown".to_string(), self.issuer.clone(), 1),
                    is_valid: false,
                    error: Some(error_msg),
                })
            }
        }
    }

    /// Refresh a token (generate new token with extended expiration)
    pub fn refresh_token(&self, old_token: &str) -> Result<String> {
        let validation = self.validate_token(old_token)?;

        if !validation.is_valid {
            return Err(JwtError::TokenExpired);
        }

        // Create new claims based on old ones but with new expiration
        let mut new_claims = validation.claims;
        let now = Utc::now();
        let exp = now + Duration::hours(self.default_expiration_hours);

        new_claims.exp = exp.timestamp();
        new_claims.iat = now.timestamp();
        new_claims.jti = Some(uuid::Uuid::new_v4().to_string());

        self.generate_token(new_claims)
    }

    /// Get the issuer
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Get default expiration hours
    pub fn default_expiration_hours(&self) -> i64 {
        self.default_expiration_hours
    }
}

/// Helper functions for common JWT operations
pub mod helpers {
    use super::*;

    /// Generate a simple access token
    pub fn generate_access_token(
        manager: &JwtManager,
        user_id: &str,
        roles: Vec<String>,
    ) -> Result<String> {
        let claims = JwtClaims::new(user_id.to_string(), manager.issuer().to_string(), 1)
            .with_claim("roles", serde_json::json!(roles))
            .with_claim("token_type", serde_json::json!("access"));

        manager.generate_token(claims)
    }

    /// Generate a refresh token with longer expiration
    pub fn generate_refresh_token(manager: &JwtManager, user_id: &str) -> Result<String> {
        let claims = JwtClaims::new(user_id.to_string(), manager.issuer().to_string(), 168) // 7 days
            .with_claim("token_type", serde_json::json!("refresh"));

        manager.generate_token(claims)
    }

    /// Extract user ID from token
    pub fn extract_user_id(manager: &JwtManager, token: &str) -> Result<Option<String>> {
        let validation = manager.validate_token(token)?;
        if validation.is_valid {
            Ok(Some(validation.claims.sub))
        } else {
            Ok(None)
        }
    }

    /// Extract roles from token
    pub fn extract_roles(manager: &JwtManager, token: &str) -> Result<Vec<String>> {
        let validation = manager.validate_token(token)?;
        if !validation.is_valid {
            return Ok(vec![]);
        }

        if let Some(roles_value) = validation.claims.custom.get("roles") {
            if let Some(roles_array) = roles_value.as_array() {
                let roles = roles_array
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                return Ok(roles);
            }
        }

        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jwt_hs256_generation_and_validation() {
        let secret = b"super_secret_key_for_testing";
        let manager = JwtManager::new_hs256(secret, "test_issuer".to_string(), 1).unwrap();

        let claims = JwtClaims::new("user123".to_string(), "test_issuer".to_string(), 1);
        let token = manager.generate_token(claims.clone()).unwrap();

        let validation = manager.validate_token(&token).unwrap();
        assert!(validation.is_valid);
        assert_eq!(validation.claims.sub, "user123");
        assert_eq!(validation.claims.iss, "test_issuer");
    }

    #[test]
    fn test_jwt_expiration() {
        let secret = b"super_secret_key_for_testing";
        let manager = JwtManager::new_hs256(secret, "test_issuer".to_string(), 1).unwrap();

        // Create token that's already expired
        let mut claims = JwtClaims::new("user123".to_string(), "test_issuer".to_string(), 1);
        claims.exp = Utc::now().timestamp() - 3600; // Expired 1 hour ago

        let token = manager.generate_token(claims).unwrap();
        let validation = manager.validate_token(&token).unwrap();

        assert!(!validation.is_valid);
        assert!(validation.error.is_some());
    }

    #[test]
    fn test_custom_claims() {
        let secret = b"super_secret_key_for_testing";
        let manager = JwtManager::new_hs256(secret, "test_issuer".to_string(), 1).unwrap();

        let claims = JwtClaims::new("user123".to_string(), "test_issuer".to_string(), 1)
            .with_claim("role", serde_json::json!("admin"))
            .with_claim("permissions", serde_json::json!(["read", "write"]));

        let token = manager.generate_token(claims).unwrap();
        let validation = manager.validate_token(&token).unwrap();

        assert_eq!(
            validation.claims.custom.get("role").unwrap().as_str().unwrap(),
            "admin"
        );
    }

    #[test]
    fn test_helpers() {
        let secret = b"super_secret_key_for_testing";
        let manager = JwtManager::new_hs256(secret, "test_issuer".to_string(), 1).unwrap();

        let token =
            helpers::generate_access_token(&manager, "user123", vec!["admin".to_string()])
                .unwrap();

        let user_id = helpers::extract_user_id(&manager, &token).unwrap();
        assert_eq!(user_id, Some("user123".to_string()));

        let roles = helpers::extract_roles(&manager, &token).unwrap();
        assert_eq!(roles, vec!["admin"]);
    }
}
