//! Core trait definitions for CarpAI

use async_trait::async_trait;

/// Marker trait for tenant-scoped resources
pub trait TenantScoped {
    fn org_id(&self) -> &str;
}

/// Trait for audit-logged operations
#[async_trait]
pub trait AuditLogged {
    type Action;

    async fn record_audit(&self, action: Self::Action) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
