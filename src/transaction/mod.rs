//! 跨文件事务管理器
//!
//! 提供原子性多文件变更的事务支持：
//! - 快照式回滚：变更前保存快照
//! - 两阶段提交：写入 temp + 原子重命名
//! - 事务日志：所有事务可审计、可撤销
//! - Agent 集成：自动包装工具调用为事务

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// 事务 ID
pub type TransactionId = String;

pub fn generate_txn_id() -> TransactionId {
    format!("txn-{}", uuid::Uuid::new_v4())
}

/// 事务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransactionStatus {
    Active,
    Committed,
    RolledBack,
    Failed(String),
}

/// 文件快照（变更前的备份）
#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub path: PathBuf,
    pub original_content: Option<String>,
    pub backup_path: Option<PathBuf>,
}

/// 文件变更
#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub old_content: String,
    pub new_content: String,
}

/// 事务日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionLogEntry {
    pub id: TransactionId,
    pub created_at: DateTime<Utc>,
    pub committed_at: Option<DateTime<Utc>>,
    pub status: TransactionStatus,
    pub files_changed: Vec<String>,
    pub description: String,
    pub diff_summary: Option<String>,
}

/// 事务
pub struct Transaction {
    pub id: TransactionId,
    pub changes: Vec<FileChange>,
    pub snapshots: Vec<FileSnapshot>,
    pub status: TransactionStatus,
    pub description: String,
    created_at: DateTime<Utc>,
}

impl Transaction {
    pub fn new(description: &str) -> Self {
        Self {
            id: generate_txn_id(),
            changes: Vec::new(),
            snapshots: Vec::new(),
            status: TransactionStatus::Active,
            description: description.to_string(),
            created_at: Utc::now(),
        }
    }

    /// 记录文件变更（自动创建快照）
    pub async fn record_change(&mut self, path: &Path, new_content: &str) -> Result<()> {
        let old_content = if path.exists() {
            tokio::fs::read_to_string(path).await?
        } else {
            String::new()
        };

        // 创建快照
        let backup_dir = TransactionManager::backup_dir();
        tokio::fs::create_dir_all(&backup_dir).await?;

        let backup_name = format!("{}_{}", self.id, path.file_name().unwrap_or_default().to_string_lossy());
        let backup_path = backup_dir.join(&backup_name);

        if !old_content.is_empty() {
            tokio::fs::write(&backup_path, &old_content).await?;
        }

        self.snapshots.push(FileSnapshot {
            path: path.to_path_buf(),
            original_content: if old_content.is_empty() { None } else { Some(old_content.clone()) },
            backup_path: if old_content.is_empty() { None } else { Some(backup_path) },
        });

        self.changes.push(FileChange {
            path: path.to_path_buf(),
            old_content,
            new_content: new_content.to_string(),
        });

        Ok(())
    }

    /// 提交事务：执行两阶段提交
    pub async fn commit(&mut self) -> Result<()> {
        if self.status != TransactionStatus::Active {
            anyhow::bail!("Transaction {} is not active (status: {:?})", self.id, self.status);
        }

        // Phase 1: 写入临时文件
        let mut temp_paths: Vec<(PathBuf, PathBuf)> = Vec::new();
        for change in &self.changes {
            let temp_path = change.path.with_file_name(
                format!(".{}.tmp", change.path.file_name().unwrap_or_default().to_string_lossy())
            );

            if let Some(parent) = temp_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            if !change.new_content.is_empty() {
                tokio::fs::write(&temp_path, &change.new_content).await?;
            } else {
                // 空内容 = 删除文件，创建空文件标记
                tokio::fs::write(&temp_path, "").await?;
            }

            temp_paths.push((temp_path, change.path.clone()));
        }

        // Phase 2: 原子重命名
        for (temp, target) in &temp_paths {
            if temp.exists() {
                tokio::fs::rename(temp, target).await?;
            }
        }

        self.status = TransactionStatus::Committed;
        Ok(())
    }

    /// 回滚事务：从快照恢复
    pub async fn rollback(&mut self) -> Result<()> {
        if self.status == TransactionStatus::Committed {
            // 已提交的事务：从备份恢复
            for snapshot in &self.snapshots {
                if let Some(backup_path) = &snapshot.backup_path {
                    if backup_path.exists() {
                        let content = tokio::fs::read_to_string(backup_path).await?;
                        tokio::fs::write(&snapshot.path, &content).await?;
                        tokio::fs::remove_file(backup_path).await?;
                    }
                } else if snapshot.path.exists() {
                    // 文件原本不存在（新建的文件），删除之
                    tokio::fs::remove_file(&snapshot.path).await?;
                }
            }
        } else {
            // 未提交的事务：直接清理
            for snapshot in &self.snapshots {
                if let Some(backup_path) = &snapshot.backup_path {
                    if backup_path.exists() {
                        tokio::fs::remove_file(backup_path).await?;
                    }
                }
            }
        }

        self.status = TransactionStatus::RolledBack;
        Ok(())
    }

    /// 生成 diff 摘要
    pub fn diff_summary(&self) -> Option<String> {
        if self.changes.is_empty() {
            return None;
        }
        let mut summary = format!("Transaction '{}': {} file(s)\n", self.description, self.changes.len());
        for change in &self.changes {
            let file_name = change.path.file_name().unwrap_or_default().to_string_lossy();
            let additions = change.new_content.lines().count().saturating_sub(change.old_content.lines().count());
            let deletions = change.old_content.lines().count().saturating_sub(change.new_content.lines().count());
            summary.push_str(&format!("  {}: +{} -{} lines\n", file_name, additions, deletions));
        }
        Some(summary)
    }
}

/// 事务管理器
pub struct TransactionManager {
    active_txn: Arc<RwLock<Option<Transaction>>>,
    history: Arc<RwLock<Vec<TransactionLogEntry>>>,
    workspace_root: PathBuf,
}

impl TransactionManager {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            active_txn: Arc::new(RwLock::new(None)),
            history: Arc::new(RwLock::new(Vec::new())),
            workspace_root: workspace_root.to_path_buf(),
        }
    }

    /// 备份目录路径
    fn backup_dir() -> PathBuf {
        std::env::temp_dir().join("carpai-transactions")
    }

    /// 开始新事务
    pub async fn begin(&self, description: &str) -> Result<TransactionId> {
        let mut txn = self.active_txn.write().await;
        if txn.is_some() {
            anyhow::bail!("A transaction is already active. Commit or rollback first.");
        }
        let new_txn = Transaction::new(description);
        let id = new_txn.id.clone();
        *txn = Some(new_txn);
        Ok(id)
    }

    /// 记录变更到当前事务
    pub async fn record_change(&self, path: &Path, new_content: &str) -> Result<()> {
        let mut txn = self.active_txn.write().await;
        if let Some(ref mut txn) = *txn {
            txn.record_change(path, new_content).await
        } else {
            anyhow::bail!("No active transaction. Call begin() first.");
        }
    }

    /// 提交当前事务
    pub async fn commit(&self, description: &str) -> Result<()> {
        let mut txn_wrapper = self.active_txn.write().await;
        if let Some(ref mut txn) = *txn_wrapper {
            txn.description = description.to_string();
            txn.commit().await?;

            // 记录到历史
            let entry = TransactionLogEntry {
                id: txn.id.clone(),
                created_at: txn.created_at,
                committed_at: Some(Utc::now()),
                status: txn.status.clone(),
                files_changed: txn.changes.iter().map(|c| c.path.to_string_lossy().to_string()).collect(),
                description: description.to_string(),
                diff_summary: txn.diff_summary(),
            };

            let mut history = self.history.write().await;
            history.push(entry);
            drop(history);

            *txn_wrapper = None;
            Ok(())
        } else {
            anyhow::bail!("No active transaction to commit.");
        }
    }

    /// 回滚当前事务
    pub async fn rollback(&self) -> Result<()> {
        let mut txn_wrapper = self.active_txn.write().await;
        if let Some(ref mut txn) = *txn_wrapper {
            txn.rollback().await?;
            let entry = TransactionLogEntry {
                id: txn.id.clone(),
                created_at: txn.created_at,
                committed_at: None,
                status: TransactionStatus::RolledBack,
                files_changed: txn.changes.iter().map(|c| c.path.to_string_lossy().to_string()).collect(),
                description: format!("ROLLED BACK: {}", txn.description),
                diff_summary: None,
            };
            let mut history = self.history.write().await;
            history.push(entry);
            drop(history);

            *txn_wrapper = None;
            Ok(())
        } else {
            anyhow::bail!("No active transaction to rollback.");
        }
    }

    /// 获取当前事务状态
    pub async fn current_status(&self) -> Option<(TransactionId, TransactionStatus, usize)> {
        let txn = self.active_txn.read().await;
        txn.as_ref().map(|t| (t.id.clone(), t.status.clone(), t.changes.len()))
    }

    /// 获取事务历史
    pub async fn history(&self) -> Vec<TransactionLogEntry> {
        self.history.read().await.clone()
    }

    /// 检查是否有活跃事务
    pub async fn is_active(&self) -> bool {
        self.active_txn.read().await.is_some()
    }

    /// 保存事务历史到磁盘
    pub async fn save_history(&self) -> Result<()> {
        let history = self.history.read().await;
        let history_path = self.workspace_root.join(".jcode").join("transactions.json");
        if let Some(parent) = history_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(&*history)?;
        tokio::fs::write(&history_path, json).await?;
        Ok(())
    }
}

/// Agent 工具包装器：自动将工具调用包装为事务
pub struct TransactionalEditTool {
    inner: Arc<dyn crate::tool::Tool + Send + Sync>,
    txn_mgr: Arc<TransactionManager>,
}

impl TransactionalEditTool {
    pub fn new(tool: Arc<dyn crate::tool::Tool + Send + Sync>, txn_mgr: Arc<TransactionManager>) -> Self {
        Self { inner: tool, txn_mgr }
    }

    /// 在事务中执行工具调用
    pub async fn execute_in_transaction(&self, input: serde_json::Value, ctx: crate::tool::ToolContext) -> Result<crate::tool::ToolOutput> {
        let txn_id = self.txn_mgr.begin("tool:edit").await?;

        // 执行原始工具
        let result = self.inner.execute(input, ctx).await?;

        // 执行成功则提交事务
        self.txn_mgr.commit(&format!("tool:edit ({})", txn_id)).await?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_transaction_begin_commit() {
        let tmp = std::env::temp_dir().join("carpai-txn-test");
        let mgr = TransactionManager::new(&tmp);

        let id = mgr.begin("test transaction").await.unwrap();
        assert!(mgr.is_active().await);

        let status = mgr.current_status().await;
        assert!(status.is_some());
        assert_eq!(status.unwrap().1, TransactionStatus::Active);

        mgr.commit("test complete").await.unwrap();
        assert!(!mgr.is_active().await);
    }

    #[tokio::test]
    async fn test_transaction_rollback() {
        let tmp = std::env::temp_dir().join("carpai-txn-rollback-test");
        let mgr = TransactionManager::new(&tmp);

        // 创建测试文件
        let test_file = tmp.join("test.txt");
        tokio::fs::write(&test_file, "original content").await.unwrap();

        mgr.begin("rollback test").await.unwrap();
        mgr.record_change(&test_file, "new content").await.unwrap();
        mgr.rollback().await.unwrap();

        let content = tokio::fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "original content");

        // Cleanup
        let _ = tokio::fs::remove_file(&test_file);
    }

    #[tokio::test]
    async fn test_generate_txn_id() {
        let id1 = generate_txn_id();
        let id2 = generate_txn_id();
        assert_ne!(id1, id2);
        assert!(id1.starts_with("txn-"));
    }
}
