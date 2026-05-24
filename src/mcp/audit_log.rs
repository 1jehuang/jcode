//! MCP Audit Logging
//!
//! Records all MCP tool invocations with immutable SHA256 hash chain.
//! Supports querying by user, time range, and tool name.
//!
//! ## Schema
//! ```sql
//! CREATE TABLE mcp_audit_logs (
//!     id SERIAL PRIMARY KEY,
//!     timestamp TIMESTAMP WITH TIME ZONE NOT NULL,
//!     user_id VARCHAR(255),
//!     session_id VARCHAR(255),
//!     tool_name VARCHAR(255) NOT NULL,
//!     params JSONB,
//!     result JSONB,
//!     success BOOLEAN NOT NULL,
//!     error_message TEXT,
//!     duration_ms INTEGER,
//!     previous_hash CHAR(64),  -- SHA256 of previous log entry
//!     current_hash CHAR(64)    -- SHA256 of this log entry
//! );
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, error};

/// A single audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    /// Unique entry ID (auto-increment in DB)
    pub id: Option<i64>,
    /// Timestamp of the tool invocation
    pub timestamp: DateTime<Utc>,
    /// User who invoked the tool
    pub user_id: Option<String>,
    /// Session ID
    pub session_id: Option<String>,
    /// Tool name (e.g., "github.list_pull_requests")
    pub tool_name: String,
    /// Tool parameters (JSON)
    pub params: Option<serde_json::Value>,
    /// Tool result (JSON)
    pub result: Option<serde_json::Value>,
    /// Whether the invocation succeeded
    pub success: bool,
    /// Error message if failed
    pub error_message: Option<String>,
    /// Execution duration in milliseconds
    pub duration_ms: Option<u64>,
    /// SHA256 hash of previous entry (for immutability chain)
    pub previous_hash: Option<String>,
    /// SHA256 hash of this entry
    pub current_hash: String,
}

impl AuditLogEntry {
    /// Calculate SHA256 hash for this entry
    pub fn calculate_hash(&self, previous_hash: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(previous_hash.as_bytes());
        hasher.update(self.timestamp.to_rfc3339().as_bytes());
        hasher.update(self.user_id.as_deref().unwrap_or("").as_bytes());
        hasher.update(self.tool_name.as_bytes());
        hasher.update(self.success.to_string().as_bytes());
        if let Some(ref params) = self.params {
            hasher.update(serde_json::to_string(params).unwrap_or_default().as_bytes());
        }
        if let Some(ref result) = self.result {
            hasher.update(serde_json::to_string(result).unwrap_or_default().as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
}

/// Filter criteria for querying audit logs
#[derive(Debug, Clone, Default)]
pub struct AuditLogFilter {
    pub user_id: Option<String>,
    pub tool_name: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub success: Option<bool>,
    pub limit: Option<usize>,
}

/// Audit logger for MCP tool invocations
pub struct AuditLogger {
    /// In-memory log buffer (for testing; production uses PostgreSQL)
    logs: Arc<RwLock<Vec<AuditLogEntry>>>,
    /// Hash of the last entry (for chain integrity)
    last_hash: Arc<RwLock<String>>,
}

impl AuditLogger {
    /// Create a new audit logger
    pub fn new() -> Self {
        Self {
            logs: Arc::new(RwLock::new(Vec::new())),
            last_hash: Arc::new(RwLock::new("0".repeat(64))), // Genesis hash
        }
    }

    /// Log a tool invocation
    pub async fn log(&self, entry: AuditLogEntry) -> Result<(), Box<dyn std::error::Error>> {
        let mut last_hash = self.last_hash.write().await;
        let current_hash = entry.calculate_hash(&last_hash);

        let mut log_entry = entry;
        log_entry.previous_hash = Some(last_hash.clone());
        log_entry.current_hash = current_hash.clone();

        *last_hash = current_hash;

        let mut logs = self.logs.write().await;
        logs.push(log_entry);

        info!("Audit log entry recorded");
        Ok(())
    }

    /// Record a tool invocation with timing
    pub async fn record_invocation(
        &self,
        user_id: Option<String>,
        session_id: Option<String>,
        tool_name: String,
        params: Option<serde_json::Value>,
        result: Option<serde_json::Value>,
        success: bool,
        error_message: Option<String>,
        duration_ms: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let entry = AuditLogEntry {
            id: None,
            timestamp: Utc::now(),
            user_id,
            session_id,
            tool_name,
            params,
            result,
            success,
            error_message,
            duration_ms: Some(duration_ms),
            previous_hash: None,
            current_hash: String::new(), // Will be calculated in log()
        };

        self.log(entry).await
    }

    /// Query audit logs with filters
    pub async fn query(&self, filter: AuditLogFilter) -> Vec<AuditLogEntry> {
        let logs = self.logs.read().await;

        let mut filtered: Vec<&AuditLogEntry> = logs.iter().collect();

        if let Some(ref user_id) = filter.user_id {
            filtered.retain(|e| e.user_id.as_ref() == Some(user_id));
        }

        if let Some(ref tool_name) = filter.tool_name {
            filtered.retain(|e| e.tool_name.contains(tool_name));
        }

        if let Some(start_time) = filter.start_time {
            filtered.retain(|e| e.timestamp >= start_time);
        }

        if let Some(end_time) = filter.end_time {
            filtered.retain(|e| e.timestamp <= end_time);
        }

        if let Some(success) = filter.success {
            filtered.retain(|e| e.success == success);
        }

        let limit = filter.limit.unwrap_or(100);
        filtered.into_iter()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Verify hash chain integrity
    pub async fn verify_integrity(&self) -> bool {
        let logs = self.logs.read().await;

        if logs.is_empty() {
            return true;
        }

        let mut expected_hash = "0".repeat(64);

        for entry in logs.iter() {
            // Check previous hash
            if entry.previous_hash.as_ref() != Some(&expected_hash) {
                error!("Hash chain broken at entry {:?}", entry.id);
                return false;
            }

            // Recalculate current hash
            let recalculated = entry.calculate_hash(&expected_hash);
            if recalculated != entry.current_hash {
                error!("Hash mismatch at entry {:?}", entry.id);
                return false;
            }

            expected_hash = recalculated;
        }

        true
    }

    /// Get statistics
    pub async fn get_stats(&self) -> AuditLogStats {
        let logs = self.logs.read().await;

        let total = logs.len();
        let successful = logs.iter().filter(|e| e.success).count();
        let failed = total - successful;

        let avg_duration = if total > 0 {
            let sum: u64 = logs.iter().filter_map(|e| e.duration_ms).sum();
            sum / total as u64
        } else {
            0
        };

        AuditLogStats {
            total_invocations: total,
            successful_invocations: successful,
            failed_invocations: failed,
            average_duration_ms: avg_duration,
        }
    }
}

/// Statistics about audit logs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogStats {
    pub total_invocations: usize,
    pub successful_invocations: usize,
    pub failed_invocations: usize,
    pub average_duration_ms: u64,
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_log_and_query() {
        let logger = AuditLogger::new();

        logger.record_invocation(
            Some("user1".to_string()),
            Some("session1".to_string()),
            "github.list_pull_requests".to_string(),
            Some(serde_json::json!({"repo": "owner/repo"})),
            Some(serde_json::json!([])),
            true,
            None,
            50,
        ).await.unwrap();

        let results = logger.query(AuditLogFilter {
            user_id: Some("user1".to_string()),
            ..Default::default()
        }).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "github.list_pull_requests");
    }

    #[tokio::test]
    async fn test_hash_chain_integrity() {
        let logger = AuditLogger::new();

        for i in 0..5 {
            logger.record_invocation(
                Some(format!("user{}", i)),
                None,
                format!("tool{}", i),
                None,
                None,
                true,
                None,
                10,
            ).await.unwrap();
        }

        assert!(logger.verify_integrity().await);
    }
}
