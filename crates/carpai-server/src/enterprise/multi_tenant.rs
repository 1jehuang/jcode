//! Multi-tenant support for CarpAI Server
//!
//! This module provides tenant context extraction and middleware
//! for multi-tenant deployments.

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, HeaderValue},
    RequestPartsExt,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Tenant identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantId(pub String);

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TenantId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Tenant context extracted from request headers or JWT claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    /// Tenant identifier
    pub tenant_id: TenantId,

    /// User ID within the tenant
    pub user_id: String,

    /// User role (admin, member, viewer)
    pub role: TenantRole,

    /// Whether this is a valid authenticated tenant
    pub is_authenticated: bool,
}

/// Tenant user roles
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TenantRole {
    Admin,
    Member,
    Viewer,
}

impl fmt::Display for TenantRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admin => write!(f, "admin"),
            Self::Member => write!(f, "member"),
            Self::Viewer => write!(f, "viewer"),
        }
    }
}

impl TenantContext {
    /// Create a new tenant context
    pub fn new(tenant_id: String, user_id: String, role: TenantRole) -> Self {
        Self {
            tenant_id: TenantId(tenant_id),
            user_id,
            role,
            is_authenticated: true,
        }
    }

    /// Create an unauthenticated/default tenant context
    pub fn default_context(default_tenant: &str) -> Self {
        Self {
            tenant_id: TenantId(default_tenant.to_string()),
            user_id: "anonymous".to_string(),
            role: TenantRole::Viewer,
            is_authenticated: false,
        }
    }

    /// Check if user has admin role
    pub fn is_admin(&self) -> bool {
        self.role == TenantRole::Admin
    }

    /// Check if user can execute tools (members and admins only)
    pub fn can_execute_tools(&self) -> bool {
        self.role == TenantRole::Admin || self.role == TenantRole::Member
    }
}

/// Extract tenant context from request headers
///
/// Headers checked (in order):
/// 1. `X-Tenant-ID` - Direct tenant identifier
/// 2. `Authorization: Bearer <jwt>` - JWT with tenant claim
/// 3. Default tenant from config
pub struct TenantExtractor;

impl<S> FromRequestParts<S> for TenantContext
where
    S: Send + Sync,
{
    type Rejection = axum::http::StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Try to extract from X-Tenant-ID header first
        if let Some(tenant_header) = parts.headers.get("X-Tenant-ID") {
            let tenant_id = tenant_header
                .to_str()
                .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?
                .to_string();

            // Try to get user ID from X-User-ID header
            let user_id = parts
                .headers
                .get("X-User-ID")
                .and_then(|h| h.to_str().ok())
                .unwrap_or("anonymous")
                .to_string();

            // Try to get role from X-User-Role header
            let role = parts
                .headers
                .get("X-User-Role")
                .and_then(|h| h.to_str().ok())
                .map(|r| match r {
                    "admin" => TenantRole::Admin,
                    "member" => TenantRole::Member,
                    _ => TenantRole::Viewer,
                })
                .unwrap_or(TenantRole::Viewer);

            return Ok(TenantContext::new(tenant_id, user_id, role));
        }

        // TODO: Extract from JWT in Week 8 when auth middleware is complete
        // For now, return default context
        Err(axum::http::StatusCode::UNAUTHORIZED)
    }
}

/// Axum middleware layer for tenant extraction
pub fn tenant_middleware() -> axum::middleware::NextLayeredService {
    // This would be integrated into the router in app.rs
    unimplemented!("Use TenantContext::from_request_parts directly in handlers")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_context_creation() {
        let ctx = TenantContext::new("org-acme".to_string(), "user-123".to_string(), TenantRole::Admin);
        assert_eq!(ctx.tenant_id.0, "org-acme");
        assert_eq!(ctx.user_id, "user-123");
        assert!(ctx.is_admin());
        assert!(ctx.can_execute_tools());
    }

    #[test]
    fn test_default_context() {
        let ctx = TenantContext::default_context("org-default");
        assert_eq!(ctx.tenant_id.0, "org-default");
        assert!(!ctx.is_authenticated);
        assert!(!ctx.can_execute_tools());
    }
}
