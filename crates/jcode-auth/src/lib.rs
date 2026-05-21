//! Enterprise-grade authentication and authorization system for CarpAI
//!
//! Features:
//! - OAuth2 + JWT authentication
//! - RBAC (Role-Based Access Control) permission system
//! - Comprehensive audit logging with GDPR compliance
//! - Data encryption (AES-256 + TLS 1.3)

pub mod oauth;
pub mod jwt;
pub mod rbac;
pub mod audit;
pub mod encryption;

// Re-export main types
pub use oauth::{OAuthProvider, OAuthConfig, OAuthToken};
pub use jwt::{JwtManager, JwtClaims, TokenValidation};
pub use rbac::{RbacEngine, Role, Permission, PermissionContext};
pub use audit::{AuditLogger, AuditEvent, AuditConfig};
pub use encryption::{EncryptionManager, EncryptedData};
