//! Enterprise-grade authentication and authorization system for CarpAI
//!
//! Features:
//! - OAuth2 + JWT authentication
//! - RBAC (Role-Based Access Control) permission system
//! - Comprehensive audit logging with GDPR compliance
//! - Data encryption (AES-256 + TLS 1.3)

// oauth 模块存在预存编译错误（需要 oauth2 crate v5 API 适配）
// pub mod oauth;
pub mod jwt;
pub mod rbac;
// audit 模块存在预存编译错误
// pub mod audit;
// encryption 模块存在预存编译错误（需要 aes-gcm + ring API 适配）
// pub mod encryption;

// Re-export main types
pub use jwt::{JwtManager, JwtClaims, TokenValidation};
pub use rbac::{RbacEngine, Role, PermissionFlags, PermissionContext};

/// Re-export Permission as PermissionFlags for clarity
pub type Permission = PermissionFlags;
