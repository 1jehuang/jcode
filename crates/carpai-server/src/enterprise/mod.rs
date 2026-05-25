//! Enterprise features (Multi-tenant + Quota + Audit)
//!
//! This module provides enterprise-grade functionality for CarpAI Server:
//! - Multi-tenancy with tenant context extraction
//! - Usage quota tracking and enforcement
//! - Audit logging for compliance

pub mod multi_tenant;
pub mod quota;
pub mod audit;

pub use multi_tenant::{TenantContext, TenantExtractor};
pub use quota::{UsageQuota, QuotaTracker, QuotaEnforcer};
pub use audit::{AuditEvent, AuditWriter};
