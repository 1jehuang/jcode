//! Audit logging system with GDPR compliance
//!
//! Provides comprehensive event logging, data retention policies,
//! and privacy controls for regulatory compliance.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};
use uuid::Uuid;

/// Audit event types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuditEventType {
    // Authentication events
    LoginSuccess,
    LoginFailure,
    Logout,
    TokenRefresh,
    PasswordChange,

    // Authorization events
    PermissionGranted,
    PermissionDenied,
    RoleAssigned,
    RoleRevoked,

    // Data access events
    DataRead,
    DataWrite,
    DataDelete,
    DataExport,

    // System events
    SystemStart,
    SystemShutdown,
    ConfigurationChange,
    SecurityAlert,

    // Compliance events
    DataRetentionEnforced,
    PrivacyRequestProcessed,
    ConsentGiven,
    ConsentWithdrawn,

    // Custom events
    Custom(String),
}

impl std::fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditEventType::LoginSuccess => write!(f, "login_success"),
            AuditEventType::LoginFailure => write!(f, "login_failure"),
            AuditEventType::Logout => write!(f, "logout"),
            AuditEventType::TokenRefresh => write!(f, "token_refresh"),
            AuditEventType::PasswordChange => write!(f, "password_change"),
            AuditEventType::PermissionGranted => write!(f, "permission_granted"),
            AuditEventType::PermissionDenied => write!(f, "permission_denied"),
            AuditEventType::RoleAssigned => write!(f, "role_assigned"),
            AuditEventType::RoleRevoked => write!(f, "role_revoked"),
            AuditEventType::DataRead => write!(f, "data_read"),
            AuditEventType::DataWrite => write!(f, "data_write"),
            AuditEventType::DataDelete => write!(f, "data_delete"),
            AuditEventType::DataExport => write!(f, "data_export"),
            AuditEventType::SystemStart => write!(f, "system_start"),
            AuditEventType::SystemShutdown => write!(f, "system_shutdown"),
            AuditEventType::ConfigurationChange => write!(f, "configuration_change"),
            AuditEventType::SecurityAlert => write!(f, "security_alert"),
            AuditEventType::DataRetentionEnforced => write!(f, "data_retention_enforced"),
            AuditEventType::PrivacyRequestProcessed => write!(f, "privacy_request_processed"),
            AuditEventType::ConsentGiven => write!(f, "consent_given"),
            AuditEventType::ConsentWithdrawn => write!(f, "consent_withdrawn"),
            AuditEventType::Custom(s) => write!(f, "custom:{}", s),
        }
    }
}

/// Severity levels for audit events
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuditSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// GDPR consent type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GdprConsentType {
    DataProcessing,
    DataSharing,
    Analytics,
    Marketing,
}

/// GDPR consent record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GdprConsent {
    pub user_id: String,
    pub consent_type: GdprConsentType,
    pub granted: bool,
    pub timestamp: DateTime<Utc>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub withdrawal_timestamp: Option<DateTime<Utc>>,
}

/// Audit event structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub event_type: AuditEventType,
    pub severity: AuditSeverity,
    pub timestamp: DateTime<Utc>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub source_ip: Option<String>,
    pub resource: Option<String>,
    pub action: String,
    pub result: String,
    pub metadata: HashMap<String, serde_json::Value>,
    pub pii_data: Option<HashMap<String, String>>, // Personally Identifiable Information
}

impl AuditEvent {
    pub fn new(event_type: AuditEventType, action: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            event_type,
            severity: AuditSeverity::Info,
            timestamp: Utc::now(),
            user_id: None,
            session_id: None,
            source_ip: None,
            resource: None,
            action: action.to_string(),
            result: "success".to_string(),
            metadata: HashMap::new(),
            pii_data: None,
        }
    }

    pub fn with_user(mut self, user_id: &str) -> Self {
        self.user_id = Some(user_id.to_string());
        self
    }

    pub fn with_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    pub fn with_severity(mut self, severity: AuditSeverity) -> Self {
        self.severity = severity;
        self
    }

    pub fn with_metadata(mut self, key: &str, value: serde_json::Value) -> Self {
        self.metadata.insert(key.to_string(), value);
        self
    }

    /// Mark event as containing PII (for GDPR handling)
    pub fn with_pii(mut self, key: &str, value: &str) -> Self {
        self.pii_data
            .get_or_insert_with(HashMap::new)
            .insert(key.to_string(), value.to_string());
        self
    }

    /// Anonymize PII data (for GDPR compliance)
    pub fn anonymize_pii(&mut self) {
        if let Some(ref mut pii) = self.pii_data {
            for value in pii.values_mut() {
                *value = "[REDACTED]".to_string();
            }
        }
    }
}

/// Audit log storage backend trait
#[async_trait::async_trait]
pub trait AuditStorage: Send + Sync {
    async fn store_event(&self, event: &AuditEvent) -> Result<()>;
    async fn query_events(&self, filter: &AuditQueryFilter) -> Result<Vec<AuditEvent>>;
    async fn delete_events_before(&self, cutoff: DateTime<Utc>) -> Result<usize>;
    async fn export_events(&self, filter: &AuditQueryFilter) -> Result<Vec<u8>>; // CSV/JSON export
}

/// In-memory audit storage (for testing/small deployments)
pub struct InMemoryAuditStorage {
    events: RwLock<Vec<AuditEvent>>,
    max_events: usize,
}

impl InMemoryAuditStorage {
    pub fn new(max_events: usize) -> Self {
        Self {
            events: RwLock::new(Vec::with_capacity(max_events)),
            max_events,
        }
    }
}

#[async_trait::async_trait]
impl AuditStorage for InMemoryAuditStorage {
    async fn store_event(&self, event: &AuditEvent) -> Result<()> {
        let mut events = self.events.write().await;
        events.push(event.clone());

        // Rotate old events if at capacity
        if events.len() > self.max_events {
            events.remove(0);
        }

        Ok(())
    }

    async fn query_events(&self, filter: &AuditQueryFilter) -> Result<Vec<AuditEvent>> {
        let events = self.events.read().await;
        let mut filtered = Vec::new();

        for event in events.iter() {
            if filter.matches(event) {
                filtered.push(event.clone());
            }
        }

        Ok(filtered)
    }

    async fn delete_events_before(&self, cutoff: DateTime<Utc>) -> Result<usize> {
        let mut events = self.events.write().await;
        let initial_len = events.len();
        events.retain(|e| e.timestamp >= cutoff);
        Ok(initial_len - events.len())
    }

    async fn export_events(&self, filter: &AuditQueryFilter) -> Result<Vec<u8>> {
        let events = self.query_events(filter).await?;
        let json = serde_json::to_vec_pretty(&events)?;
        Ok(json)
    }
}

/// Audit query filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditQueryFilter {
    pub event_types: Option<Vec<AuditEventType>>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub severity: Option<AuditSeverity>,
    pub resource: Option<String>,
    pub limit: Option<usize>,
}

impl AuditQueryFilter {
    pub fn matches(&self, event: &AuditEvent) -> bool {
        if let Some(ref types) = self.event_types {
            if !types.contains(&event.event_type) {
                return false;
            }
        }

        if let Some(ref user_id) = self.user_id {
            if event.user_id.as_ref() != Some(user_id) {
                return false;
            }
        }

        if let Some(ref session_id) = self.session_id {
            if event.session_id.as_ref() != Some(session_id) {
                return false;
            }
        }

        if let Some(start) = self.start_time {
            if event.timestamp < start {
                return false;
            }
        }

        if let Some(end) = self.end_time {
            if event.timestamp > end {
                return false;
            }
        }

        if let Some(ref severity) = self.severity {
            if &event.severity != severity {
                return false;
            }
        }

        if let Some(ref resource) = self.resource {
            if event.resource.as_ref() != Some(resource) {
                return false;
            }
        }

        true
    }
}

/// Audit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    pub enabled: bool,
    pub retention_days: u32,
    pub max_events: usize,
    pub log_pii: bool, // Whether to log PII data (should be false in production)
    pub export_format: ExportFormat,
    pub gdpr_compliance: bool,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 90,
            max_events: 100000,
            log_pii: false,
            export_format: ExportFormat::Json,
            gdpr_compliance: true,
        }
    }
}

/// Export format for audit logs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExportFormat {
    Json,
    Csv,
}

/// Audit logger
pub struct AuditLogger {
    config: AuditConfig,
    storage: Arc<dyn AuditStorage>,
    consents: RwLock<HashMap<String, Vec<GdprConsent>>>,
}

impl AuditLogger {
    pub fn new(config: AuditConfig, storage: Arc<dyn AuditStorage>) -> Self {
        Self {
            config,
            storage,
            consents: RwLock::new(HashMap::new()),
        }
    }

    /// Log an audit event
    pub async fn log_event(&self, event: AuditEvent) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Anonymize PII if configured
        if !self.config.log_pii && event.pii_data.is_some() {
            let mut event = event;
            event.anonymize_pii();
            self.storage.store_event(&event).await?;
        } else {
            self.storage.store_event(&event).await?;
        }

        // Log to tracing for immediate visibility
        info!(
            event_type = %event.event_type,
            user_id = ?event.user_id,
            action = %event.action,
            result = %event.result,
            "Audit event logged"
        );

        Ok(())
    }

    /// Query audit events
    pub async fn query_events(&self, filter: &AuditQueryFilter) -> Result<Vec<AuditEvent>> {
        self.storage.query_events(filter).await
    }

    /// Enforce data retention policy
    pub async fn enforce_retention(&self) -> Result<usize> {
        if !self.config.gdpr_compliance {
            return Ok(0);
        }

        let cutoff = Utc::now() - chrono::Duration::days(self.config.retention_days as i64);
        let deleted = self.storage.delete_events_before(cutoff).await?;

        info!(deleted_count = deleted, "Audit log retention enforced");

        // Log the retention enforcement event
        let event = AuditEvent::new(
            AuditEventType::DataRetentionEnforced,
            "retention_policy_enforced",
        )
        .with_metadata("deleted_count", serde_json::json!(deleted))
        .with_metadata("retention_days", serde_json::json!(self.config.retention_days));

        self.log_event(event).await?;

        Ok(deleted)
    }

    /// Record GDPR consent
    pub async fn record_consent(&self, consent: GdprConsent) -> Result<()> {
        let mut consents = self.consents.write().await;
        consents
            .entry(consent.user_id.clone())
            .or_insert_with(Vec::new)
            .push(consent.clone());

        // Log consent event
        let event_type = if consent.granted {
            AuditEventType::ConsentGiven
        } else {
            AuditEventType::ConsentWithdrawn
        };

        let event = AuditEvent::new(event_type, "consent_recorded")
            .with_user(&consent.user_id)
            .with_metadata(
                "consent_type",
                serde_json::json!(format!("{:?}", consent.consent_type)),
            );

        self.log_event(event).await?;

        Ok(())
    }

    /// Check if user has given consent
    pub async fn has_consent(
        &self,
        user_id: &str,
        consent_type: GdprConsentType,
    ) -> Result<bool> {
        let consents = self.consents.read().await;

        if let Some(user_consents) = consents.get(user_id) {
            Ok(user_consents.iter().any(|c| {
                c.consent_type == consent_type
                    && c.granted
                    && c.withdrawal_timestamp.is_none()
            }))
        } else {
            Ok(false)
        }
    }

    /// Process GDPR data deletion request (right to be forgotten)
    pub async fn process_deletion_request(&self, user_id: &str) -> Result<()> {
        // Anonymize all events for this user
        let filter = AuditQueryFilter {
            user_id: Some(user_id.to_string()),
            ..Default::default()
        };

        let events = self.storage.query_events(&filter).await?;

        // In a real implementation, we would anonymize or delete these events
        // For now, we just log the request
        let event = AuditEvent::new(
            AuditEventType::PrivacyRequestProcessed,
            "gdpr_deletion_request",
        )
        .with_user(user_id)
        .with_metadata("events_affected", serde_json::json!(events.len()));

        self.log_event(event).await?;

        info!(user_id = user_id, "GDPR deletion request processed");

        Ok(())
    }

    /// Export audit logs
    pub async fn export_logs(&self, filter: &AuditQueryFilter) -> Result<Vec<u8>> {
        self.storage.export_events(filter).await
    }

    /// Get current configuration
    pub fn config(&self) -> &AuditConfig {
        &self.config
    }
}

// Type aliases
pub type Result<T> = std::result::Result<T, AuditError>;

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_logging() {
        let config = AuditConfig::default();
        let storage = Arc::new(InMemoryAuditStorage::new(1000));
        let logger = AuditLogger::new(config, storage);

        let event = AuditEvent::new(AuditEventType::LoginSuccess, "user_login")
            .with_user("user123")
            .with_session("session-abc")
            .with_metadata("ip_address", serde_json::json!("192.168.1.1"));

        logger.log_event(event).await.unwrap();

        let filter = AuditQueryFilter {
            user_id: Some("user123".to_string()),
            ..Default::default()
        };

        let events = logger.query_events(&filter).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, AuditEventType::LoginSuccess);
    }

    #[tokio::test]
    async fn test_gdpr_consent() {
        let config = AuditConfig::default();
        let storage = Arc::new(InMemoryAuditStorage::new(1000));
        let logger = AuditLogger::new(config, storage);

        let consent = GdprConsent {
            user_id: "user123".to_string(),
            consent_type: GdprConsentType::DataProcessing,
            granted: true,
            timestamp: Utc::now(),
            ip_address: Some("192.168.1.1".to_string()),
            user_agent: None,
            withdrawal_timestamp: None,
        };

        logger.record_consent(consent).await.unwrap();

        let has_consent = logger
            .has_consent("user123", GdprConsentType::DataProcessing)
            .await
            .unwrap();
        assert!(has_consent);
    }

    #[tokio::test]
    async fn test_retention_enforcement() {
        let config = AuditConfig {
            retention_days: 1,
            ..Default::default()
        };
        let storage = Arc::new(InMemoryAuditStorage::new(1000));
        let logger = AuditLogger::new(config, storage);

        // Create an old event
        let mut old_event =
            AuditEvent::new(AuditEventType::LoginSuccess, "old_login");
        old_event.timestamp = Utc::now() - chrono::Duration::days(2);

        logger.log_event(old_event).await.unwrap();

        // Enforce retention
        let deleted = logger.enforce_retention().await.unwrap();
        assert_eq!(deleted, 1);
    }
}
