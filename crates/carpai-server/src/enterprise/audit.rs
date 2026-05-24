//! Audit logging for compliance and security
//!
//! This module provides structured audit logging for enterprise deployments:
//! - All agent interactions
//! - Tool executions
//! - File modifications
//! - Authentication events

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs::{OpenOptions, File};
use tokio::io::AsyncWriteExt;
use tracing::info;

/// Audit event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEventType {
    // Agent Events
    AgentTurnStarted,
    AgentTurnCompleted,
    AgentTurnFailed,

    // Tool Events
    ToolExecutionRequested,
    ToolExecutionCompleted,
    ToolExecutionFailed,

    // File Events
    FileRead,
    FileWrite,
    FileDelete,

    // Session Events
    SessionCreated,
    SessionDeleted,
    SessionMessagesAppended,

    // Auth Events
    AuthenticationSuccess,
    AuthenticationFailure,
    AuthorizationDenied,

    // System Events
    ConfigChanged,
    QuotaExceeded,
    RateLimitExceeded,
}

impl std::fmt::Display for AuditEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AgentTurnStarted => write!(f, "agent.turn_started"),
            Self::AgentTurnCompleted => write!(f, "agent.turn_completed"),
            Self::AgentTurnFailed => write!(f, "agent.turn_failed"),
            Self::ToolExecutionRequested => write!(f, "tool.execution_requested"),
            Self::ToolExecutionCompleted => write!(f, "tool.execution_completed"),
            Self::ToolExecutionFailed => write!(f, "tool.execution_failed"),
            Self::FileRead => write!(f, "file.read"),
            Self::FileWrite => write!(f, "file.write"),
            Self::FileDelete => write!(f, "file.delete"),
            Self::SessionCreated => write!(f, "session.created"),
            Self::SessionDeleted => write!(f, "session.deleted"),
            Self::SessionMessagesAppended => write!(f, "session.messages_appended"),
            Self::AuthenticationSuccess => write!(f, "auth.success"),
            Self::AuthenticationFailure => write!(f, "auth.failure"),
            Self::AuthorizationDenied => write!(f, "auth.denied"),
            Self::ConfigChanged => write!(f, "system.config_changed"),
            Self::QuotaExceeded => write!(f, "system.quota_exceeded"),
            Self::RateLimitExceeded => write!(f, "system.rate_limit_exceeded"),
        }
    }
}

/// A single audit event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique event ID
    pub id: String,

    /// Timestamp of the event
    pub timestamp: DateTime<Utc>,

    /// Type of event
    #[serde(rename = "type")]
    pub event_type: AuditEventType,

    /// Tenant ID (if applicable)
    pub tenant_id: Option<String>,

    /// User ID who triggered the event
    pub user_id: String,

    /// Session ID (if applicable)
    pub session_id: Option<String>,

    /// Event-specific metadata
    pub metadata: serde_json::Value,

    /// IP address of the requester (if available)
    pub source_ip: Option<String>,
}

impl AuditEvent {
    /// Create a new audit event
    pub fn new(event_type: AuditEventType, user_id: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            event_type,
            tenant_id: None,
            user_id,
            session_id: None,
            metadata: serde_json::json!({}),
            source_ip: None,
        }
    }

    /// Set tenant ID
    pub fn with_tenant(mut self, tenant_id: String) -> Self {
        self.tenant_id = Some(tenant_id);
        self
    }

    /// Set session ID
    pub fn with_session(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set source IP
    pub fn with_source_ip(mut self, ip: String) -> Self {
        self.source_ip = Some(ip);
        self
    }
}

/// Audit log writer - writes events to file asynchronously
pub struct AuditWriter {
    log_path: PathBuf,
    enabled: bool,
}

impl AuditWriter {
    /// Create a new audit writer
    pub fn new(log_path: PathBuf, enabled: bool) -> Self {
        Self { log_path, enabled }
    }

    /// Write an audit event to the log file
    pub async fn write_event(&self, event: &AuditEvent) -> Result<(), AuditError> {
        if !self.enabled {
            return Ok(());
        }

        // Ensure directory exists
        if let Some(parent) = self.log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Serialize event to JSON line
        let json_line = serde_json::to_string(event)?;

        // Append to log file
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .await?;

        file.write_all(json_line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        // Also log to tracing for observability
        info!(
            event_type = %event.event_type,
            user_id = %event.user_id,
            tenant_id = ?event.tenant_id,
            session_id = ?event.session_id,
            "Audit event"
        );

        Ok(())
    }

    /// Convenience method to log agent turn start
    pub async fn log_agent_turn(&self, user_id: &str, session_id: &str, message: &str) -> Result<(), AuditError> {
        let event = AuditEvent::new(AuditEventType::AgentTurnStarted, user_id.to_string())
            .with_session(session_id.to_string())
            .with_metadata(serde_json::json!({
                "message_preview": message.chars().take(100).collect::<String>()
            }));

        self.write_event(&event).await
    }

    /// Convenience method to log tool execution
    pub async fn log_tool_execution(
        &self,
        user_id: &str,
        tool_name: &str,
        success: bool,
        duration_ms: u64,
    ) -> Result<(), AuditError> {
        let event_type = if success {
            AuditEventType::ToolExecutionCompleted
        } else {
            AuditEventType::ToolExecutionFailed
        };

        let event = AuditEvent::new(event_type, user_id.to_string())
            .with_metadata(serde_json::json!({
                "tool_name": tool_name,
                "duration_ms": duration_ms
            }));

        self.write_event(&event).await
    }

    /// Convenience method to log authentication
    pub async fn log_auth(&self, user_id: &str, success: bool, source_ip: Option<&str>) -> Result<(), AuditError> {
        let event_type = if success {
            AuditEventType::AuthenticationSuccess
        } else {
            AuditEventType::AuthenticationFailure
        };

        let mut event = AuditEvent::new(event_type, user_id.to_string());
        if let Some(ip) = source_ip {
            event = event.with_source_ip(ip.to_string());
        }

        self.write_event(&event).await
    }
}

/// Audit error types
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_event_creation() {
        let event = AuditEvent::new(AuditEventType::AgentTurnStarted, "user-123".to_string())
            .with_tenant("org-acme".to_string())
            .with_session("sess-456".to_string());

        assert_eq!(event.user_id, "user-123");
        assert_eq!(event.tenant_id, Some("org-acme".to_string()));
        assert_eq!(event.session_id, Some("sess-456".to_string()));
    }

    #[tokio::test]
    async fn test_audit_writer_disabled() {
        let writer = AuditWriter::new(PathBuf::from("/tmp/test-audit.log"), false);
        let event = AuditEvent::new(AuditEventType::AgentTurnStarted, "user".to_string());

        // Should succeed without writing anything
        assert!(writer.write_event(&event).await.is_ok());
    }
}
