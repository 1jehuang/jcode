//! # 审计日志系统
//!
//! 记录所有Agent操作和系统事件，用于：
//! - 安全审计
//! - 行为分析
//! - 问题排查
//! - 合规性检查

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tokio::sync::RwLock;

/// 审计日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub agent_id: Option<String>,
    pub action_type: ActionType,
    pub details: serde_json::Value,
    pub ip_address: Option<String>,
    pub severity: LogSeverity,
}

/// 操作类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActionType {
    // Agent操作
    AgentStart,
    AgentStop,
    TaskCreate,
    TaskComplete,
    ToolExecute,
    FileRead,
    FileWrite,
    
    // 用户操作
    UserLogin,
    UserLogout,
    SessionStart,
    SessionEnd,
    
    // 系统操作
    SystemStart,
    SystemShutdown,
    ConfigChange,
    CacheHit,
    CacheMiss,
    
    // 安全事件
    AuthFailure,
    PermissionDenied,
    SuspiciousActivity,
}

/// 日志严重级别
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LogSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// 审计日志过滤器
#[derive(Debug, Clone, Deserialize)]
pub struct AuditFilters {
    pub agent_id: Option<String>,
    pub action_type: Option<ActionType>,
    pub severity: Option<LogSeverity>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

/// 审计日志管理器
pub struct AuditLogger {
    log_dir: PathBuf,
    entries: RwLock<Vec<AuditLogEntry>>,
    max_in_memory: usize,
}

impl AuditLogger {
    /// 创建新的审计日志器
    pub fn new(log_dir: &str) -> Self {
        Self {
            log_dir: PathBuf::from(log_dir),
            entries: RwLock::new(Vec::new()),
            max_in_memory: 10000,
        }
    }
    
    /// 初始化日志目录
    pub async fn initialize(&self) -> Result<(), String> {
        fs::create_dir_all(&self.log_dir)
            .await
            .map_err(|e| format!("Failed to create log directory: {}", e))?;
        Ok(())
    }
    
    /// 记录操作
    pub async fn log_action(&self, entry: AuditLogEntry) -> Result<(), String> {
        // 添加到内存
        let mut entries = self.entries.write().await;
        entries.push(entry.clone());

        // 限制内存中的条目数
        if entries.len() > self.max_in_memory {
            let len = entries.len();
            let to_remove = len.saturating_sub(self.max_in_memory);
            entries.drain(0..to_remove);
        }

        drop(entries);

        self.write_to_file(entry).await;

        Ok(())
    }
    
    /// 写入文件
    async fn write_to_file(&self, entry: AuditLogEntry) {
        let date = entry.timestamp.format("%Y-%m-%d");
        let filename = format!("audit-{}.jsonl", date);
        let filepath = self.log_dir.join(&filename);
        
        let line = serde_json::to_string(&entry).unwrap_or_default();
        
        // 追加到文件
        if let Err(e) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&filepath)
            .await
        {
            eprintln!("Failed to open log file: {}", e);
            return;
        }
        
        // 简化：实际应该使用BufWriter
        if let Err(e) = fs::write(&filepath, format!("{}\n", line)).await {
            eprintln!("Failed to write to log file: {}", e);
        }
    }
    
    /// 查询审计日志
    pub async fn query_logs(&self, filters: AuditFilters) -> Result<Vec<AuditLogEntry>, String> {
        let entries = self.entries.read().await;
        
        let mut filtered: Vec<&AuditLogEntry> = entries.iter().collect();
        
        // 应用过滤器
        if let Some(ref agent_id) = filters.agent_id {
            filtered.retain(|e| e.agent_id.as_ref() == Some(agent_id));
        }
        
        if let Some(action_type) = filters.action_type {
            filtered.retain(|e| e.action_type == action_type);
        }
        
        if let Some(severity) = filters.severity {
            filtered.retain(|e| e.severity == severity);
        }
        
        if let Some(start_time) = filters.start_time {
            filtered.retain(|e| e.timestamp >= start_time);
        }
        
        if let Some(end_time) = filters.end_time {
            filtered.retain(|e| e.timestamp <= end_time);
        }
        
        // 限制返回数量
        let limit = filters.limit.unwrap_or(100);
        let result: Vec<AuditLogEntry> = filtered.iter()
            .take(limit)
            .map(|e| (*e).clone())
            .collect();
        
        Ok(result)
    }
    
    /// 获取最近的日志
    pub async fn get_recent(&self, count: usize) -> Result<Vec<AuditLogEntry>, String> {
        let entries = self.entries.read().await;
        let recent: Vec<AuditLogEntry> = entries.iter()
            .rev()
            .take(count)
            .cloned()
            .collect();
        Ok(recent)
    }
    
    /// 清除旧日志（保留最近N天）
    pub async fn cleanup_old_logs(&self, retain_days: u32) -> Result<usize, String> {
        let cutoff = Utc::now() - chrono::Duration::days(retain_days as i64);
        
        let mut entries = self.entries.write().await;
        let before_count = entries.len();
        
        entries.retain(|e| e.timestamp >= cutoff);
        
        let removed = before_count - entries.len();
        Ok(removed)
    }
    
    /// 导出日志为JSON
    pub async fn export_logs(&self, output_path: &str) -> Result<(), String> {
        let entries = self.entries.read().await;
        let json = serde_json::to_string_pretty(&*entries)
            .map_err(|e| format!("Failed to serialize logs: {}", e))?;
        
        fs::write(output_path, json)
            .await
            .map_err(|e| format!("Failed to write export file: {}", e))?;
        
        Ok(())
    }
    
    /// 获取统计信息
    pub async fn get_stats(&self) -> Result<AuditStats, String> {
        let entries = self.entries.read().await;
        
        let total = entries.len();
        let by_severity = entries.iter().fold(
            std::collections::HashMap::new(),
            |mut acc, e| {
                *acc.entry(format!("{:?}", e.severity)).or_insert(0) += 1;
                acc
            }
        );
        
        let by_action = entries.iter().fold(
            std::collections::HashMap::new(),
            |mut acc, e| {
                *acc.entry(format!("{:?}", e.action_type)).or_insert(0) += 1;
                acc
            }
        );
        
        Ok(AuditStats {
            total_entries: total,
            by_severity,
            by_action,
        })
    }
}

/// 审计统计信息
#[derive(Debug, Serialize)]
pub struct AuditStats {
    pub total_entries: usize,
    pub by_severity: std::collections::HashMap<String, usize>,
    pub by_action: std::collections::HashMap<String, usize>,
}

/// 辅助函数：创建日志条目
pub fn create_log_entry(
    agent_id: Option<String>,
    action_type: ActionType,
    details: serde_json::Value,
    ip_address: Option<String>,
    severity: LogSeverity,
) -> AuditLogEntry {
    use uuid::Uuid;
    
    AuditLogEntry {
        id: Uuid::new_v4().to_string(),
        timestamp: Utc::now(),
        agent_id,
        action_type,
        details,
        ip_address,
        severity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_audit_logger_basic() {
        let logger = AuditLogger::new("/tmp/test_audit_logs");
        logger.initialize().await.unwrap();
        
        let entry = create_log_entry(
            Some("agent_1".to_string()),
            ActionType::TaskCreate,
            serde_json::json!({"task": "test"}),
            None,
            LogSeverity::Info,
        );
        
        logger.log_action(entry).await.unwrap();
        
        let recent = logger.get_recent(1).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].agent_id, Some("agent_1".to_string()));
    }
    
    #[tokio::test]
    async fn test_query_filters() {
        let logger = AuditLogger::new("/tmp/test_audit_logs2");
        logger.initialize().await.unwrap();
        
        // 添加多个条目
        for i in 0..5 {
            let entry = create_log_entry(
                Some(format!("agent_{}", i)),
                ActionType::ToolExecute,
                serde_json::json!({"tool": "test"}),
                None,
                if i % 2 == 0 { LogSeverity::Info } else { LogSeverity::Warning },
            );
            logger.log_action(entry).await.unwrap();
        }
        
        // 查询特定agent
        let filters = AuditFilters {
            agent_id: Some("agent_2".to_string()),
            action_type: None,
            severity: None,
            start_time: None,
            end_time: None,
            limit: None,
        };
        
        let results = logger.query_logs(filters).await.unwrap();
        assert_eq!(results.len(), 1);
    }
    
    #[tokio::test]
    async fn test_stats() {
        let logger = AuditLogger::new("/tmp/test_audit_logs3");
        logger.initialize().await.unwrap();
        
        // 添加条目
        for _ in 0..10 {
            let entry = create_log_entry(
                None,
                ActionType::CacheHit,
                serde_json::json!({}),
                None,
                LogSeverity::Info,
            );
            logger.log_action(entry).await.unwrap();
        }
        
        let stats = logger.get_stats().await.unwrap();
        assert_eq!(stats.total_entries, 10);
        assert!(stats.by_severity.contains_key("Info"));
    }
}
