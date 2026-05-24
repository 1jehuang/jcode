//! API-Key validation

use axum::{
    extract::Request,
    http::{header::HeaderName, StatusCode},
    middleware::Next,
    response::Response,
};
use tracing::warn;

const API_KEY_HEADER: &str = "X-API-Key";

#[derive(Clone)]
pub struct ApiKeyValidator {
    valid_keys: Vec<String>,
}

impl ApiKeyValidator {
    pub fn new(valid_keys: Vec<String>) -> Self {
        Self { valid_keys }
    }

    pub fn validate(&self, api_key: &str) -> bool {
        self.valid_keys.iter().any(|k| k == api_key)
    }
}

/// Axum middleware that validates API-Key header
pub async fn api_key_middleware(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Get API-Key header
    let api_key = req.headers()
        .get(API_KEY_HEADER)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| {
            warn!("Missing X-API-Key header");
            StatusCode::UNAUTHORIZED
        })?;

    // TODO: Load valid keys from config/database - for now accept any non-empty key
    if api_key.is_empty() {
        warn!("Empty API key");
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}
