//! Role-Based Access Control (RBAC) middleware
//!
//! Provides permission checking based on user roles extracted from JWT tokens.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::jwt::JwtClaims;

/// Granular permissions that can be assigned to roles
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    /// Read-only access to resources
    Read,
    /// Write/create/update resources
    Write,
    /// Delete resources
    Delete,
    /// Administrative operations
    Admin,
    /// Execute tools/code
    Execute,
    /// Manage sessions
    ManageSessions,
    /// View audit logs
    ViewAuditLogs,
    /// Manage organization/tenant settings
    ManageOrg,
}

impl Permission {
    /// Convert permission to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Write => "write",
            Permission::Delete => "delete",
            Permission::Admin => "admin",
            Permission::Execute => "execute",
            Permission::ManageSessions => "manage_sessions",
            Permission::ViewAuditLogs => "view_audit_logs",
            Permission::ManageOrg => "manage_org",
        }
    }
}

/// Standard roles in the system
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Regular user with basic permissions
    User,
    /// Power user with extended permissions
    PowerUser,
    /// Administrator with full permissions
    Admin,
    /// Service account for automated tasks
    ServiceAccount,
}

impl Role {
    /// Get permissions granted to this role
    pub fn permissions(&self) -> HashSet<Permission> {
        match self {
            Role::User => {
                let mut perms = HashSet::new();
                perms.insert(Permission::Read);
                perms.insert(Permission::Write);
                perms.insert(Permission::Execute);
                perms.insert(Permission::ManageSessions);
                perms
            }
            Role::PowerUser => {
                let mut perms = HashSet::new();
                perms.insert(Permission::Read);
                perms.insert(Permission::Write);
                perms.insert(Permission::Delete);
                perms.insert(Permission::Execute);
                perms.insert(Permission::ManageSessions);
                perms.insert(Permission::ViewAuditLogs);
                perms
            }
            Role::Admin => {
                let mut perms = HashSet::new();
                perms.insert(Permission::Read);
                perms.insert(Permission::Write);
                perms.insert(Permission::Delete);
                perms.insert(Permission::Admin);
                perms.insert(Permission::Execute);
                perms.insert(Permission::ManageSessions);
                perms.insert(Permission::ViewAuditLogs);
                perms.insert(Permission::ManageOrg);
                perms
            }
            Role::ServiceAccount => {
                let mut perms = HashSet::new();
                perms.insert(Permission::Read);
                perms.insert(Permission::Execute);
                perms
            }
        }
    }

    /// Check if this role has a specific permission
    pub fn has_permission(&self, permission: &Permission) -> bool {
        self.permissions().contains(permission)
    }
}

/// RBAC checker for validating user permissions
#[derive(Debug, Clone)]
pub struct RbacChecker {
    /// Default role for users without explicit role
    default_role: Role,
}

impl RbacChecker {
    /// Create a new RBAC checker with default configuration
    pub fn new() -> Self {
        Self {
            default_role: Role::User,
        }
    }

    /// Create with custom default role
    pub fn with_default_role(default_role: Role) -> Self {
        Self { default_role }
    }

    /// Extract role from JWT claims
    pub fn extract_role(&self, claims: &JwtClaims) -> Role {
        match claims.role.as_str() {
            "admin" => Role::Admin,
            "power_user" => Role::PowerUser,
            "service_account" => Role::ServiceAccount,
            _ => self.default_role.clone(),
        }
    }

    /// Check if user has a specific permission
    pub fn check_permission(&self, claims: &JwtClaims, permission: &Permission) -> bool {
        let role = self.extract_role(claims);
        role.has_permission(permission)
    }

    /// Check if user has any of the required permissions
    pub fn check_any_permission(
        &self,
        claims: &JwtClaims,
        permissions: &[Permission],
    ) -> bool {
        let role = self.extract_role(claims);
        permissions.iter().any(|p| role.has_permission(p))
    }

    /// Check if user has all required permissions
    pub fn check_all_permissions(
        &self,
        claims: &JwtClaims,
        permissions: &[Permission],
    ) -> bool {
        let role = self.extract_role(claims);
        permissions.iter().all(|p| role.has_permission(p))
    }

    /// Check if user is admin
    pub fn is_admin(&self, claims: &JwtClaims) -> bool {
        matches!(self.extract_role(claims), Role::Admin)
    }
}

impl Default for RbacChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Middleware to check read permission
pub async fn require_read_permission(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    check_permission(req, next, &Permission::Read).await
}

/// Middleware to check write permission
pub async fn require_write_permission(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    check_permission(req, next, &Permission::Write).await
}

/// Middleware to check delete permission
pub async fn require_delete_permission(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    check_permission(req, next, &Permission::Delete).await
}

/// Middleware to check admin permission
pub async fn require_admin_permission(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    check_permission(req, next, &Permission::Admin).await
}

/// Middleware to check execute permission
pub async fn require_execute_permission(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    check_permission(req, next, &Permission::Execute).await
}

/// Generic permission checking middleware
async fn check_permission(
    mut req: Request,
    next: Next,
    required_permission: &Permission,
) -> Result<Response, StatusCode> {
    // Extract JWT claims from request extensions (set by jwt_middleware)
    let claims = req
        .extensions()
        .get::<JwtClaims>()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let rbac = RbacChecker::new();

    if !rbac.check_permission(claims, required_permission) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_role_permissions() {
        let role = Role::User;
        assert!(role.has_permission(&Permission::Read));
        assert!(role.has_permission(&Permission::Write));
        assert!(role.has_permission(&Permission::Execute));
        assert!(!role.has_permission(&Permission::Delete));
        assert!(!role.has_permission(&Permission::Admin));
    }

    #[test]
    fn test_admin_role_permissions() {
        let role = Role::Admin;
        assert!(role.has_permission(&Permission::Read));
        assert!(role.has_permission(&Permission::Write));
        assert!(role.has_permission(&Permission::Delete));
        assert!(role.has_permission(&Permission::Admin));
        assert!(role.has_permission(&Permission::Execute));
        assert!(role.has_permission(&Permission::ViewAuditLogs));
        assert!(role.has_permission(&Permission::ManageOrg));
    }

    #[test]
    fn test_rbac_checker() {
        let rbac = RbacChecker::new();
        let claims = JwtClaims {
            sub: "user123".to_string(),
            org_id: "org456".to_string(),
            email: "user@example.com".to_string(),
            role: "admin".to_string(),
            exp: 0,
            iat: 0,
        };

        assert!(rbac.is_admin(&claims));
        assert!(rbac.check_permission(&claims, &Permission::Admin));
        assert!(rbac.check_permission(&claims, &Permission::Delete));
    }

    #[test]
    fn test_default_role_extraction() {
        let rbac = RbacChecker::new();
        let claims = JwtClaims {
            sub: "user123".to_string(),
            org_id: "org456".to_string(),
            email: "user@example.com".to_string(),
            role: "unknown".to_string(),
            exp: 0,
            iat: 0,
        };

        // Unknown role should default to User
        let role = rbac.extract_role(&claims);
        assert_eq!(role, Role::User);
    }
}
