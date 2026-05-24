//! Tenant Context Middleware
//!
//! Extracts tenant information from JWT tokens and injects it into
//! request extensions for downstream handlers.

use axum::{
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{warn, info};

/// Tenant context extracted from JWT claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    pub tenant_id: String,
    pub user_id: String,
    pub org_id: String,
    pub permissions: Vec<String>,
}

impl TenantContext {
    /// Check if user has a specific permission
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission || p == "*")
    }

    /// Check if user is admin
    pub fn is_admin(&self) -> bool {
        self.permissions.iter().any(|p| p == "admin" || p == "*")
    }
}

/// Extract tenant context from JWT token in Authorization header
pub async fn extract_tenant_context(req: &Request) -> Result<TenantContext, StatusCode> {
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

    // Decode JWT (simplified - in production use jsonwebtoken crate)
    decode_jwt_claims(token).map_err(|e| {
        warn!("Failed to decode JWT: {}", e);
        StatusCode::UNAUTHORIZED
    })
}

/// Decode JWT claims into TenantContext
fn decode_jwt_claims(token: &str) -> Result<TenantContext, Box<dyn std::error::Error>> {
    // In production, use jsonwebtoken crate:
    // use jsonwebtoken::{decode, DecodingKey, Validation};
    // let key = DecodingKey::from_secret(...);
    // let claims = decode::<CustomClaims>(token, &key, &Validation::default())?;

    // For now, parse from environment or use default for testing
    // This should be replaced with actual JWT decoding
    let ctx = TenantContext {
        tenant_id: std::env::var("DEFAULT_TENANT_ID").unwrap_or_else(|_| "default".to_string()),
        user_id: std::env::var("DEFAULT_USER_ID").unwrap_or_else(|_| "user-001".to_string()),
        org_id: std::env::var("DEFAULT_ORG_ID").unwrap_or_else(|_| "org-001".to_string()),
        permissions: vec!["read".to_string(), "write".to_string()],
    };

    Ok(ctx)
}

/// Axum middleware that injects TenantContext into request extensions
pub async fn tenant_middleware(
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip tenant check for health/public endpoints
    let path = req.uri().path();
    if path.starts_with("/health") || path.starts_with("/public") {
        return Ok(next.run(req).await);
    }

    // Extract tenant context
    let ctx = extract_tenant_context(&req).await?;

    info!("Tenant context extracted: tenant={} user={}", ctx.tenant_id, ctx.user_id);

    // Inject into request extensions
    req.extensions_mut().insert(ctx);

    Ok(next.run(req).await)
}

/// Helper to get TenantContext from request extensions in handlers
pub fn get_tenant_context(req: &Request) -> Option<&TenantContext> {
    req.extensions().get::<TenantContext>()
}

/// Require specific permission, returns 403 if not authorized
pub fn require_permission(ctx: &TenantContext, permission: &str) -> Result<(), StatusCode> {
    if !ctx.has_permission(permission) {
        warn!("Permission denied: user={} required={}", ctx.user_id, permission);
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// Require admin role, returns 403 if not admin
pub fn require_admin(ctx: &TenantContext) -> Result<(), StatusCode> {
    if !ctx.is_admin() {
        warn!("Admin access denied: user={}", ctx.user_id);
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_permission() {
        let ctx = TenantContext {
            tenant_id: "t1".to_string(),
            user_id: "u1".to_string(),
            org_id: "o1".to_string(),
            permissions: vec!["read".to_string(), "write".to_string()],
        };

        assert!(ctx.has_permission("read"));
        assert!(ctx.has_permission("write"));
        assert!(!ctx.has_permission("delete"));
    }

    #[test]
    fn test_is_admin() {
        let admin_ctx = TenantContext {
            tenant_id: "t1".to_string(),
            user_id: "u1".to_string(),
            org_id: "o1".to_string(),
            permissions: vec!["admin".to_string()],
        };

        let user_ctx = TenantContext {
            tenant_id: "t1".to_string(),
            user_id: "u2".to_string(),
            org_id: "o1".to_string(),
            permissions: vec!["read".to_string()],
        };

        assert!(admin_ctx.is_admin());
        assert!(!user_ctx.is_admin());
    }
}
