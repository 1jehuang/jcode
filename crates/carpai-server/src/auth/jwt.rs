//! JWT authentication middleware

use axum::{
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,        // User ID
    pub org_id: String,     // Organization/Tenant ID
    pub email: String,
    pub role: String,
    pub exp: u64,           // Expiration timestamp
    pub iat: u64,           // Issued at timestamp
}

#[derive(Clone)]
pub struct JwtMiddleware {
    secret: String,
    expiry_hours: u64,
}

impl JwtMiddleware {
    pub fn new(secret: String, expiry_hours: u64) -> Self {
        Self { secret, expiry_hours }
    }

    /// Generate JWT token for a user
    pub fn generate_token(&self, user_id: &str, org_id: &str, email: &str, role: &str) -> Result<String, AuthError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = JwtClaims {
            sub: user_id.to_string(),
            org_id: org_id.to_string(),
            email: email.to_string(),
            role: role.to_string(),
            exp: now + (self.expiry_hours * 3600),
            iat: now,
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        ).map_err(|e| AuthError::TokenGeneration(e.to_string()))
    }

    /// Validate and decode JWT token
    pub fn validate_token(&self, token: &str) -> Result<JwtClaims, AuthError> {
        let token_data = decode::<JwtClaims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &Validation::default(),
        ).map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        Ok(token_data.claims)
    }
}

/// Axum middleware that extracts and validates JWT
pub async fn jwt_middleware(
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Get Authorization header
    let auth_header = req.headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| {
            warn!("Missing Authorization header");
            StatusCode::UNAUTHORIZED
        })?;

    // Extract JWT token (expect "Bearer <token>")
    let token = auth_header.strip_prefix("Bearer ")
        .ok_or_else(|| {
            warn!("Invalid Authorization header format");
            StatusCode::UNAUTHORIZED
        })?;

    // TODO: Get secret from config/state - for now use env var
    let secret = std::env::var("CARPAI_JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    let middleware = JwtMiddleware::new(secret, 24);

    // Validate token
    let claims = middleware.validate_token(token).map_err(|e| {
        warn!("JWT validation failed: {}", e);
        StatusCode::UNAUTHORIZED
    })?;

    // Inject claims into request extensions
    req.extensions_mut().insert(claims);

    Ok(next.run(req).await)
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Invalid token: {0}")]
    InvalidToken(String),
    #[error("Token generation failed: {0}")]
    TokenGeneration(String),
}
