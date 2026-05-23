//! SOC2 Type II Continuous Compliance Monitor
//!
//! Implements immutable audit trail with cryptographic hash chaining
//! to ensure tamper-proof logging required for SOC2 Type II certification.
//!
//! Key features:
//! - Blockchain-style hash chaining for audit events
//! - SHA-256 cryptographic sealing
//! - Automated compliance checks against SOC2 controls
//! - Real-time alerting on policy violations
//! - Evidence collection and reporting

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tracing::{error, info, warn};

/// Audit event types covering all SOC2 Trust Services Criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEventType {
    // CC1: Control Environment
    UserCreated,
    UserDeleted,
    RoleAssigned,
    PolicyUpdated,

    // CC2: Communication & Information
    DataExported,
    DataImported,
    NotificationSent,

    // CC3: Risk Assessment
    RiskAssessmentPerformed,
    VulnerabilityDetected,
    ThreatMitigated,

    // CC4: Monitoring Activities
    SecurityScanCompleted,
    AnomalyDetected,
    AlertTriggered,

    // CC5: Control Activities
    AccessGranted,
    AccessRevoked,
    AuthenticationSuccess,
    AuthenticationFailure,
    AuthorizationCheck,

    // CC6: Logical & Physical Access
    LoginAttempt,
    Logout,
    SessionTimeout,
    MFAEnabled,
    MFADisabled,
    PasswordChanged,

    // CC7: System Operations
    SystemStartup,
    SystemShutdown,
    BackupCompleted,
    BackupFailed,
    RecoveryInitiated,
    ConfigurationChange,

    // CC8: Change Management
    CodeDeployed,
    DeploymentRolledBack,
    InfrastructureChange,
    SecurityPatchApplied,

    // CC9: Risk Mitigation
    IncidentDetected,
    IncidentResponded,
    DataBreachDetected,
    EmergencyAccessGranted,

    // A1: Availability
    ServiceDegradation,
    ServiceOutage,
    AutoScalingEvent,
    LoadBalancerChange,

    // C1: Confidentiality
    DataEncrypted,
    DataDecrypted,
    EncryptionKeyRotated,
    SensitiveDataAccessed,

    // P1: Privacy
    PersonalDataCollected,
    PersonalDataProcessed,
    PersonalDataDeleted,
    ConsentGiven,
    ConsentWithdrawn,
    DataSubjectRequest,
}

impl std::fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Individual audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_id: String,
    pub event_type: AuditEventType,
    pub user_id: Option<String>,
    pub tenant_id: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub action: String,
    pub status: String, // "success", "failure", "pending"
    pub details: HashMap<String, serde_json::Value>,
    pub severity: String, // "info", "warning", "error", "critical"
}

impl AuditEvent {
    pub fn new(event_type: AuditEventType, user_id: Option<String>) -> Self {
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type,
            user_id,
            tenant_id: None,
            timestamp: Utc::now(),
            source_ip: None,
            user_agent: None,
            resource_type: None,
            resource_id: None,
            action: event_type.to_string(),
            status: "success".to_string(),
            details: HashMap::new(),
            severity: "info".to_string(),
        }
    }

    pub fn with_tenant(mut self, tenant_id: String) -> Self {
        self.tenant_id = Some(tenant_id);
        self
    }

    pub fn with_source_ip(mut self, ip: String) -> Self {
        self.source_ip = Some(ip);
        self
    }

    pub fn with_details(mut self, key: &str, value: serde_json::Value) -> Self {
        self.details.insert(key.to_string(), value);
        self
    }

    pub fn with_severity(mut self, severity: &str) -> Self {
        self.severity = severity.to_string();
        self
    }

    /// Serialize event to JSON bytes for hashing
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("Failed to serialize audit event")
    }
}

/// Audit block containing multiple events with hash chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditBlock {
    pub block_id: u64,
    pub events: Vec<AuditEvent>,
    pub previous_hash: Vec<u8>,
    pub current_hash: Vec<u8>,
    pub timestamp: i64,
    pub merkle_root: Vec<u8>,
}

impl AuditBlock {
    pub fn new(block_id: u64, events: Vec<AuditEvent>, previous_hash: Vec<u8>) -> Self {
        let timestamp = Utc::now().timestamp();
        let merkle_root = Self::compute_merkle_root(&events);
        let current_hash = Self::compute_block_hash(block_id, &events, &previous_hash, timestamp, &merkle_root);

        Self {
            block_id,
            events,
            previous_hash,
            current_hash,
            timestamp,
            merkle_root,
        }
    }

    fn compute_block_hash(
        block_id: u64,
        events: &[AuditEvent],
        previous_hash: &[u8],
        timestamp: i64,
        merkle_root: &[u8],
    ) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(block_id.to_le_bytes());
        hasher.update(previous_hash);
        hasher.update(timestamp.to_le_bytes());
        hasher.update(merkle_root);

        // Hash all events
        for event in events {
            hasher.update(event.to_bytes());
        }

        hasher.finalize().to_vec()
    }

    fn compute_merkle_root(events: &[AuditEvent]) -> Vec<u8> {
        if events.is_empty() {
            return Sha256::digest(b"empty").to_vec();
        }

        let mut hashes: Vec<Vec<u8>> = events
            .iter()
            .map(|e| Sha256::digest(e.to_bytes()).to_vec())
            .collect();

        // Build Merkle tree
        while hashes.len() > 1 {
            let mut new_level = Vec::new();
            for chunk in hashes.chunks(2) {
                let mut hasher = Sha256::new();
                hasher.update(&chunk[0]);
                if chunk.len() > 1 {
                    hasher.update(&chunk[1]);
                } else {
                    hasher.update(&chunk[0]); // Duplicate last if odd
                }
                new_level.push(hasher.finalize().to_vec());
            }
            hashes = new_level;
        }

        hashes.into_iter().next().unwrap_or_else(|| Sha256::digest(b"empty").to_vec())
    }

    /// Verify block integrity
    pub fn verify(&self) -> bool {
        let expected_hash = Self::compute_block_hash(
            self.block_id,
            &self.events,
            &self.previous_hash,
            self.timestamp,
            &self.merkle_root,
        );
        expected_hash == self.current_hash
    }
}

/// Immutable audit trail with blockchain-style hash chaining
pub struct ImmutableAuditTrail {
    chain: Vec<AuditBlock>,
    pending_events: Vec<AuditEvent>,
    max_block_size: usize,
    total_events: u64,
}

impl ImmutableAuditTrail {
    pub fn new(max_block_size: usize) -> Self {
        info!("Initializing immutable audit trail with block_size={}", max_block_size);
        Self {
            chain: Vec::new(),
            pending_events: Vec::new(),
            max_block_size,
            total_events: 0,
        }
    }

    /// Add event to pending queue
    pub fn add_event(&mut self, event: AuditEvent) {
        self.pending_events.push(event);
        self.total_events += 1;

        // Seal block when full
        if self.pending_events.len() >= self.max_block_size {
            self.seal_block();
        }
    }

    /// Force seal current pending events into a block
    pub fn seal_block(&mut self) {
        if self.pending_events.is_empty() {
            return;
        }

        let block_id = self.chain.len() as u64;
        let previous_hash = self
            .chain
            .last()
            .map(|b| b.current_hash.clone())
            .unwrap_or_else(|| vec![0; 32]); // Genesis block has zero hash

        let events = std::mem::take(&mut self.pending_events);
        let block = AuditBlock::new(block_id, events, previous_hash);

        info!(
            "Sealed audit block #{} with {} events (total: {})",
            block_id,
            block.events.len(),
            self.total_events
        );

        self.chain.push(block);
    }

    /// Verify entire chain integrity
    pub fn verify_chain(&self) -> bool {
        for (i, block) in self.chain.iter().enumerate() {
            if !block.verify() {
                error!("Block #{} failed verification - POSSIBLE TAMPERING DETECTED!", i);
                return false;
            }

            // Verify chain linkage
            if i > 0 {
                let prev_block = &self.chain[i - 1];
                if block.previous_hash != prev_block.current_hash {
                    error!(
                        "Chain linkage broken between blocks #{} and #{}",
                        i - 1,
                        i
                    );
                    return false;
                }
            }
        }

        info!("Audit chain verification passed: {} blocks, {} total events", self.chain.len(), self.total_events);
        true
    }

    /// Get events by time range
    pub fn get_events_in_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<&AuditEvent> {
        self.chain
            .iter()
            .flat_map(|block| &block.events)
            .filter(|event| event.timestamp >= start && event.timestamp <= end)
            .collect()
    }

    /// Get events by type
    pub fn get_events_by_type(&self, event_type: &AuditEventType) -> Vec<&AuditEvent> {
        self.chain
            .iter()
            .flat_map(|block| &block.events)
            .filter(|event| match_event_type(&event.event_type, event_type))
            .collect()
    }

    /// Get events by user
    pub fn get_events_by_user(&self, user_id: &str) -> Vec<&AuditEvent> {
        self.chain
            .iter()
            .flat_map(|block| &block.events)
            .filter(|event| event.user_id.as_deref() == Some(user_id))
            .collect()
    }

    /// Get chain statistics
    pub fn stats(&self) -> AuditTrailStats {
        let total_blocks = self.chain.len();
        let total_events: usize = self.chain.iter().map(|b| b.events.len()).sum();
        let avg_block_size = if total_blocks > 0 {
            total_events as f64 / total_blocks as f64
        } else {
            0.0
        };

        AuditTrailStats {
            total_blocks,
            total_events,
            pending_events: self.pending_events.len(),
            avg_block_size,
            chain_integrity: self.verify_chain(),
        }
    }

    /// Export chain for external audit (SOC2 evidence)
    pub fn export_for_audit(&self) -> AuditExport {
        AuditExport {
            exported_at: Utc::now(),
            total_blocks: self.chain.len(),
            total_events: self.total_events,
            blocks: self.chain.clone(),
            chain_verified: self.verify_chain(),
        }
    }
}

fn match_event_type(a: &AuditEventType, b: &AuditEventType) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Audit trail statistics
#[derive(Debug)]
pub struct AuditTrailStats {
    pub total_blocks: usize,
    pub total_events: usize,
    pub pending_events: usize,
    pub avg_block_size: f64,
    pub chain_integrity: bool,
}

/// Exported audit data for external auditors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditExport {
    pub exported_at: DateTime<Utc>,
    pub total_blocks: usize,
    pub total_events: u64,
    pub blocks: Vec<AuditBlock>,
    pub chain_verified: bool,
}

/// SOC2 control compliance checker
pub struct SOC2ComplianceChecker {
    audit_trail: std::sync::Arc<std::sync::Mutex<ImmutableAuditTrail>>,
}

impl SOC2ComplianceChecker {
    pub fn new(audit_trail: std::sync::Arc<std::sync::Mutex<ImmutableAuditTrail>>) -> Self {
        Self { audit_trail }
    }

    /// Check CC6: Logical and Physical Access Controls
    pub fn check_cc6_access_controls(&self) -> ComplianceReport {
        let trail = self.audit_trail.lock().unwrap();
        let now = Utc::now();
        let last_24h = now - chrono::Duration::hours(24);

        let auth_failures = trail
            .get_events_by_type(&AuditEventType::AuthenticationFailure)
            .into_iter()
            .filter(|e| e.timestamp >= last_24h)
            .count();

        let access_grants = trail
            .get_events_by_type(&AuditEventType::AccessGranted)
            .into_iter()
            .filter(|e| e.timestamp >= last_24h)
            .count();

        let mfa_changes = trail
            .get_events_by_type(&AuditEventType::MFAEnabled)
            .into_iter()
            .chain(trail.get_events_by_type(&AuditEventType::MFADisabled))
            .filter(|e| e.timestamp >= last_24h)
            .count();

        let compliant = auth_failures < 100 && access_grants < 50 && mfa_changes < 10;

        ComplianceReport {
            control_id: "CC6.1".to_string(),
            control_name: "Logical and Physical Access Controls".to_string(),
            compliant,
            findings: vec![
                format!("Authentication failures (24h): {}", auth_failures),
                format!("Access grants (24h): {}", access_grants),
                format!("MFA changes (24h): {}", mfa_changes),
            ],
            checked_at: now,
        }
    }

    /// Check CC7: System Operations
    pub fn check_cc7_system_operations(&self) -> ComplianceReport {
        let trail = self.audit_trail.lock().unwrap();
        let now = Utc::now();
        let last_7d = now - chrono::Duration::days(7);

        let backups = trail
            .get_events_by_type(&AuditEventType::BackupCompleted)
            .into_iter()
            .filter(|e| e.timestamp >= last_7d)
            .count();

        let backup_failures = trail
            .get_events_by_type(&AuditEventType::BackupFailed)
            .into_iter()
            .filter(|e| e.timestamp >= last_7d)
            .count();

        let outages = trail
            .get_events_by_type(&AuditEventType::ServiceOutage)
            .into_iter()
            .filter(|e| e.timestamp >= last_7d)
            .count();

        // Require daily backups (7 in 7 days) and < 1 hour outage
        let compliant = backups >= 7 && backup_failures == 0 && outages == 0;

        ComplianceReport {
            control_id: "CC7.2".to_string(),
            control_name: "System Operations - Backup & Availability".to_string(),
            compliant,
            findings: vec![
                format!("Successful backups (7d): {}", backups),
                format!("Backup failures (7d): {}", backup_failures),
                format!("Service outages (7d): {}", outages),
            ],
            checked_at: now,
        }
    }

    /// Check CC8: Change Management
    pub fn check_cc8_change_management(&self) -> ComplianceReport {
        let trail = self.audit_trail.lock().unwrap();
        let now = Utc::now();
        let last_30d = now - chrono::Duration::days(30);

        let deployments = trail
            .get_events_by_type(&AuditEventType::CodeDeployed)
            .into_iter()
            .filter(|e| e.timestamp >= last_30d)
            .count();

        let rollbacks = trail
            .get_events_by_type(&AuditEventType::DeploymentRolledBack)
            .into_iter()
            .filter(|e| e.timestamp >= last_30d)
            .count();

        let rollback_rate = if deployments > 0 {
            rollbacks as f64 / deployments as f64
        } else {
            0.0
        };

        // Rollback rate should be < 10%
        let compliant = rollback_rate < 0.1;

        ComplianceReport {
            control_id: "CC8.1".to_string(),
            control_name: "Change Management - Deployment Quality".to_string(),
            compliant,
            findings: vec![
                format!("Deployments (30d): {}", deployments),
                format!("Rollbacks (30d): {}", rollbacks),
                format!("Rollback rate: {:.1}%", rollback_rate * 100.0),
            ],
            checked_at: now,
        }
    }

    /// Run all SOC2 checks
    pub fn run_full_compliance_check(&self) -> Vec<ComplianceReport> {
        vec![
            self.check_cc6_access_controls(),
            self.check_cc7_system_operations(),
            self.check_cc8_change_management(),
        ]
    }
}

/// Compliance check report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceReport {
    pub control_id: String,
    pub control_name: String,
    pub compliant: bool,
    pub findings: Vec<String>,
    pub checked_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_block_creation() {
        let events = vec![
            AuditEvent::new(AuditEventType::UserCreated, Some("user1".to_string())),
            AuditEvent::new(AuditEventType::LoginAttempt, Some("user1".to_string())),
        ];

        let block = AuditBlock::new(0, events, vec![0; 32]);
        assert_eq!(block.block_id, 0);
        assert_eq!(block.events.len(), 2);
        assert!(block.verify());
    }

    #[test]
    fn test_chain_integrity() {
        let mut trail = ImmutableAuditTrail::new(2);

        trail.add_event(AuditEvent::new(AuditEventType::UserCreated, Some("user1".to_string())));
        trail.add_event(AuditEvent::new(AuditEventType::LoginAttempt, Some("user1".to_string())));
        // Block sealed here (max_block_size = 2)

        trail.add_event(AuditEvent::new(AuditEventType::AccessGranted, Some("user1".to_string())));
        trail.seal_block(); // Force seal

        assert!(trail.verify_chain());
        assert_eq!(trail.chain.len(), 2);
    }

    #[test]
    fn test_compliance_checker() {
        let trail = Arc::new(std::sync::Mutex::new(ImmutableAuditTrail::new(100)));
        let checker = SOC2ComplianceChecker::new(trail);

        let reports = checker.run_full_compliance_check();
        assert!(!reports.is_empty());
    }
}
