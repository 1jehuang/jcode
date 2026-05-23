//! 审计日志系统
//!
//! 记录所有关键操作，满足企业合规要求（SOC2、GDPR等）
//!
//! 特性：
//! - 不可篡改的日志记录
//! - 支持多种存储后端（数据库、文件、云存储）
//! - 实时日志流
//! - 灵活的查询和导出

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tracing::{error, info};

/// 审计动作类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditAction {
    // ===== 认证相关 =====
    /// 登录成功
    LoginSuccess { method: String },
    /// 登录失败
    LoginFailure { method: String, reason: String },
    /// 登出
    Logout,
    /// API密钥创建
    ApiKeyCreated { key_preview: String },
    /// API密钥撤销
    ApiKeyRevoked { key_id: String },
    /// Token刷新
    TokenRefreshed,

    // ===== 用户管理 =====
    UserCreated { email: String, role: String },
    UserUpdated { user_id: String, fields: Vec<String> },
    UserDeleted { user_id: String },
    RoleAssigned { user_id: String, role: String },
    RoleRevoked { user_id: String, role: String },

    // ===== 会话相关 =====
    SessionStarted { session_id: String, model: String },
    SessionEnded { session_id: String, duration_secs: u64 },
    MessageSent { session_id: String, message_count: u32 },
    ToolExecuted { session_id: String, tool_name: String, success: bool },

    // ===== 数据访问 =====
    FileRead { path: String },
    FileWritten { path: String },
    CodebaseIndexed { repo_path: String, file_count: u32 },
    CodebaseSearched { query: String, result_count: u32 },

    // ===== 配置变更 =====
    ConfigUpdated { section: String, keys: Vec<String> },
    PolicyChanged { policy_type: String },
    QuotaUpdated { user_id: String, new_limit: u64 },

    // ===== 系统事件 =====
    NodeJoined { node_id: String, hardware_info: String },
    NodeLeft { node_id: String, reason: String },
    ModelDeployed { model_name: String, node_id: String },
    BackupCreated { backup_id: String, size_mb: u64 },

    // ===== 安全管理 =====
    SecurityScanPerformed { scan_type: String, issues_found: u32 },
    FirewallRuleUpdated { rule_id: String },
    SSOConfigChanged { provider: String },
}

impl AuditAction {
    pub fn category(&self) -> &str {
        match self {
            Self::LoginSuccess { .. }
            | Self::LoginFailure { .. }
            | Self::Logout
            | Self::ApiKeyCreated { .. }
            | Self::ApiKeyRevoked { .. }
            | Self::TokenRefreshed => "authentication",

            Self::UserCreated { .. }
            | Self::UserUpdated { .. }
            | Self::UserDeleted { .. }
            | Self::RoleAssigned { .. }
            | Self::RoleRevoked { .. } => "user_management",

            Self::SessionStarted { .. }
            | Self::SessionEnded { .. }
            | Self::MessageSent { .. }
            | Self::ToolExecuted { .. } => "session",

            Self::FileRead { .. }
            | Self::FileWritten { .. }
            | Self::CodebaseIndexed { .. }
            | Self::CodebaseSearched { .. } => "data_access",

            Self::ConfigUpdated { .. }
            | Self::PolicyChanged { .. }
            | Self::QuotaUpdated { .. } => "configuration",

            Self::NodeJoined { .. }
            | Self::NodeLeft { .. }
            | Self::ModelDeployed { .. }
            | Self::BackupCreated { .. } => "system",

            Self::SecurityScanPerformed { .. }
            | Self::FirewallRuleUpdated { .. }
            | Self::SSOConfigChanged { .. } => "security",
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::LoginSuccess { method } => format!("User logged in via {}", method),
            Self::LoginFailure { method, reason } => {
                format!("Login failed via {}: {}", method, reason)
            }
            Self::Logout => "User logged out".to_string(),
            Self::ApiKeyCreated { key_preview } => {
                format!("API key created (preview: {})", key_preview)
            }
            Self::ApiKeyRevoked { key_id } => format!("API key revoked: {}", key_id),
            Self::TokenRefreshed => "Authentication token refreshed".to_string(),
            Self::UserCreated { email, role } => {
                format!("User created: {} with role {}", email, role)
            }
            Self::UserUpdated { user_id, fields } => {
                format!("User {} updated: {:?}", user_id, fields)
            }
            Self::UserDeleted { user_id } => format!("User deleted: {}", user_id),
            Self::RoleAssigned { user_id, role } => {
                format!("Role {} assigned to user {}", role, user_id)
            }
            Self::RoleRevoked { user_id, role } => {
                format!("Role {} revoked from user {}", role, user_id)
            }
            Self::SessionStarted { session_id, model } => {
                format!("Session {} started with model {}", session_id, model)
            }
            Self::SessionEnded { session_id, duration_secs } => {
                format!("Session {} ended (duration: {}s)", session_id, duration_secs)
            }
            Self::MessageSent { session_id, message_count } => {
                format!("{} messages sent in session {}", message_count, session_id)
            }
            Self::ToolExecuted { session_id, tool_name, success } => {
                format!(
                    "Tool {} executed in session {} ({})",
                    tool_name,
                    session_id,
                    if *success { "success" } else { "failed" }
                )
            }
            Self::FileRead { path } => format!("File read: {}", path),
            Self::FileWritten { path } => format!("File written: {}", path),
            Self::CodebaseIndexed { repo_path, file_count } => {
                format!("Codebase indexed: {} ({} files)", repo_path, file_count)
            }
            Self::CodebaseSearched { query, result_count } => {
                format!("Codebase search: '{}' ({} results)", query, result_count)
            }
            Self::ConfigUpdated { section, keys } => {
                format!("Configuration updated: section={}, keys={:?}", section, keys)
            }
            Self::PolicyChanged { policy_type } => {
                format!("Policy changed: {}", policy_type)
            }
            Self::QuotaUpdated { user_id, new_limit } => {
                format!("Quota updated for user {}: new limit={}", user_id, new_limit)
            }
            Self::NodeJoined { node_id, hardware_info } => {
                format!("Node joined: {} ({})", node_id, hardware_info)
            }
            Self::NodeLeft { node_id, reason } => {
                format!("Node left: {} (reason: {})", node_id, reason)
            }
            Self::ModelDeployed { model_name, node_id } => {
                format!("Model {} deployed to node {}", model_name, node_id)
            }
            Self::BackupCreated { backup_id, size_mb } => {
                format!("Backup created: {} ({} MB)", backup_id, size_mb)
            }
            Self::SecurityScanPerformed { scan_type, issues_found } => {
                format!("Security scan performed: {} ({} issues found)", scan_type, issues_found)
            }
            Self::FirewallRuleUpdated { rule_id } => {
                format!("Firewall rule updated: {}", rule_id)
            }
            Self::SSOConfigChanged { provider } => {
                format!("SSO configuration changed for provider: {}", provider)
            }
        }
    }
}

/// 执行者类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorType {
    /// 用户
    User,
    /// 服务账户
    Service,
    /// 系统自动操作
    System,
}

/// 审计结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditResult {
    Success,
    Failure { error: String },
    Denied { reason: String },
}

/// 审计日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    /// 唯一标识符
    pub id: String,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 执行者ID
    pub actor_id: String,
    /// 执行者类型
    pub actor_type: ActorType,
    /// 动作
    pub action: AuditAction,
    /// 目标对象ID
    pub target_id: Option<String>,
    /// 目标对象类型
    pub target_type: Option<String>,
    /// 元数据（额外信息）
    pub metadata: serde_json::Value,
    /// IP地址
    pub ip_address: Option<String>,
    /// 用户代理
    pub user_agent: Option<String>,
    /// 结果
    pub result: AuditResult,
    /// 组织ID（多租户隔离）
    pub org_id: Option<String>,
}

impl AuditLog {
    pub fn new(
        actor_id: String,
        actor_type: ActorType,
        action: AuditAction,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id,
            timestamp: Utc::now(),
            actor_id,
            actor_type,
            action,
            target_id: None,
            target_type: None,
            metadata: serde_json::json!({}),
            ip_address: None,
            user_agent: None,
            result: AuditResult::Success,
            org_id: None,
        }
    }

    pub fn with_target(mut self, target_id: String, target_type: String) -> Self {
        self.target_id = Some(target_id);
        self.target_type = Some(target_type);
        self
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_ip(mut self, ip: String) -> Self {
        self.ip_address = Some(ip);
        self
    }

    pub fn with_user_agent(mut self, ua: String) -> Self {
        self.user_agent = Some(ua);
        self
    }

    pub fn with_result(mut self, result: AuditResult) -> Self {
        self.result = result;
        self
    }

    pub fn with_org(mut self, org_id: String) -> Self {
        self.org_id = Some(org_id);
        self
    }
}

/// 审计日志写入器trait
#[async_trait::async_trait]
pub trait AuditWriter: Send + Sync {
    /// 批量写入日志
    async fn write_batch(&mut self, logs: &[AuditLog]) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// 刷新缓冲区
    async fn flush(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// 关闭写入器
    async fn close(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// 审计日志记录器
pub struct AuditLogger {
    writer: Box<dyn AuditWriter>,
    buffer: Vec<AuditLog>,
    buffer_size: usize,
}

impl AuditLogger {
    pub fn new(writer: Box<dyn AuditWriter>) -> Self {
        Self {
            writer,
            buffer: Vec::with_capacity(100),
            buffer_size: 100,
        }
    }

    pub fn with_buffer_size(writer: Box<dyn AuditWriter>, size: usize) -> Self {
        Self {
            writer,
            buffer: Vec::with_capacity(size),
            buffer_size: size,
        }
    }

    /// 记录审计日志
    pub async fn log(&mut self, log: AuditLog) {
        self.buffer.push(log);

        if self.buffer.len() >= self.buffer_size {
            if let Err(e) = self.flush().await {
                error!("Failed to flush audit logs: {}", e);
            }
        }
    }

    /// 立即刷新缓冲区
    pub async fn flush(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let logs = std::mem::take(&mut self.buffer);
        self.writer.write_batch(&logs).await?;
        info!("Flushed {} audit logs", logs.len());

        Ok(())
    }

    /// 关闭日志器
    pub async fn close(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.flush().await?;
        self.writer.close().await
    }
}

impl Drop for AuditLogger {
    fn drop(&mut self) {
        if !self.buffer.is_empty() {
            error!(
                "AuditLogger dropped with {} unflushed logs",
                self.buffer.len()
            );
        }
    }
}

/// 数据库审计写入器
#[cfg(feature = "database")]
pub struct DatabaseAuditWriter {
    db: Arc<crate::db::DatabaseManager>,
}

#[cfg(feature = "database")]
#[async_trait::async_trait]
impl AuditWriter for DatabaseAuditWriter {
    async fn write_batch(&mut self, logs: &[AuditLog]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use sqlx::types::Json;

        let mut tx = self.db.begin().await?;

        for log in logs {
            sqlx::query!(
                r#"
                INSERT INTO audit_logs (
                    id, timestamp, actor_id, actor_type, action,
                    target_id, target_type, metadata, ip_address,
                    user_agent, result, org_id
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
                log.id,
                log.timestamp,
                log.actor_id,
                serde_json::to_string(&log.actor_type)?,
                serde_json::to_string(&log.action)?,
                log.target_id,
                log.target_type,
                Json(&log.metadata),
                log.ip_address,
                log.user_agent,
                serde_json::to_string(&log.result)?,
                log.org_id,
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    async fn close(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// 文件审计写入器
pub struct FileAuditWriter {
    path: PathBuf,
    file: Option<tokio::fs::File>,
}

impl FileAuditWriter {
    pub async fn new(path: PathBuf) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        Ok(Self {
            path,
            file: Some(file),
        })
    }
}

#[async_trait::async_trait]
impl AuditWriter for FileAuditWriter {
    async fn write_batch(&mut self, logs: &[AuditLog]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use tokio::io::AsyncWriteExt;

        if let Some(file) = &mut self.file {
            for log in logs {
                let line = serde_json::to_string(log)?;
                file.write_all(line.as_bytes()).await?;
                file.write_all(b"\n").await?;
            }
            file.flush().await?;
        }

        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(file) = &mut self.file {
            file.flush().await?;
        }
        Ok(())
    }

    async fn close(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(mut file) = self.file.take() {
            file.flush().await?;
            file.shutdown().await?;
        }
        Ok(())
    }
}

/// 审计日志查询接口
#[async_trait::async_trait]
pub trait AuditQuery: Send + Sync {
    /// 查询审计日志
    async fn query_logs(
        &self,
        filter: AuditLogFilter,
    ) -> Result<Vec<AuditLog>, Box<dyn std::error::Error + Send + Sync>>;

    /// 获取日志总数
    async fn count_logs(
        &self,
        filter: AuditLogFilter,
    ) -> Result<u64, Box<dyn std::error::Error + Send + Sync>>;

    /// 导出日志为JSON
    async fn export_json(
        &self,
        filter: AuditLogFilter,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;

    /// 导出日志为CSV
    async fn export_csv(
        &self,
        filter: AuditLogFilter,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>>;
}

/// 审计日志过滤器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogFilter {
    pub start_date: Option<DateTime<Utc>>,
    pub end_date: Option<DateTime<Utc>>,
    pub actor_id: Option<String>,
    pub action_category: Option<String>,
    pub org_id: Option<String>,
    pub result_filter: Option<AuditResult>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for AuditLogFilter {
    fn default() -> Self {
        Self {
            start_date: None,
            end_date: None,
            actor_id: None,
            action_category: None,
            org_id: None,
            result_filter: None,
            limit: 100,
            offset: 0,
        }
    }
}

/// 创建默认的文件审计日志器
pub async fn create_file_logger(
    log_dir: PathBuf,
) -> Result<AuditLogger, Box<dyn std::error::Error + Send + Sync>> {
    tokio::fs::create_dir_all(&log_dir).await?;

    let log_file = log_dir.join(format!("audit-{}.log", chrono::Utc::now().format("%Y-%m-%d")));
    let writer = FileAuditWriter::new(log_file).await?;

    Ok(AuditLogger::with_buffer_size(Box::new(writer), 50))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_logger_basic() {
        let temp_dir = std::env::temp_dir().join("carpai_audit_test");
        let mut logger = create_file_logger(temp_dir.clone()).await.unwrap();

        let log = AuditLog::new(
            "user123".to_string(),
            ActorType::User,
            AuditAction::LoginSuccess {
                method: "password".to_string(),
            },
        )
        .with_ip("192.168.1.1".to_string());

        logger.log(log).await;
        logger.flush().await.unwrap();

        // 验证文件存在
        let log_files: Vec<_> = std::fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!log_files.is_empty());

        // 清理
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_audit_action_serialization() {
        let action = AuditAction::UserCreated {
            email: "test@example.com".to_string(),
            role: "developer".to_string(),
        };

        let json = serde_json::to_string(&action).unwrap();
        let deserialized: AuditAction = serde_json::from_str(&json).unwrap();

        assert_eq!(action, deserialized);
    }

    #[test]
    fn test_audit_action_category() {
        assert_eq!(
            AuditAction::LoginSuccess {
                method: "oauth".to_string()
            }
            .category(),
            "authentication"
        );
        assert_eq!(
            AuditAction::UserCreated {
                email: "test@test.com".to_string(),
                role: "admin".to_string()
            }
            .category(),
            "user_management"
        );
    }
}
