//! Snapshot — 会话快照管理
//!
//! ## 核心能力
//! - 定时快照
//! - 增量快照
//! - 快照恢复

use crate::types::SessionId;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{debug, info};

/// 快照元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    pub id: String,
    pub session_id: SessionId,
    pub timestamp: u64,
    pub size_bytes: u64,
    pub message_count: usize,
}

/// 快照数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub metadata: SnapshotMetadata,
    pub data: Vec<u8>,
}

/// 快照管理器
pub struct SnapshotManager {
    snapshot_dir: std::path::PathBuf,
}

impl SnapshotManager {
    /// 创建新的快照管理器
    pub fn new(snapshot_dir: &Path) -> Self {
        Self { snapshot_dir: snapshot_dir.to_path_buf() }
    }

    /// 创建快照
    pub async fn create_snapshot(
        &self,
        session_id: &SessionId,
        data: &[u8],
    ) -> anyhow::Result<Snapshot> {
        let id = format!("snap_{}", uuid::Uuid::new_v4());
        
        let metadata = SnapshotMetadata {
            id: id.clone(),
            session_id: session_id.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            size_bytes: data.len() as u64,
            message_count: 0,
        };

        let snapshot = Snapshot {
            metadata,
            data: data.to_vec(),
        };

        // 保存到文件
        if !self.snapshot_dir.exists() {
            tokio::fs::create_dir_all(&self.snapshot_dir).await?;
        }
        
        let file_path = self.snapshot_dir.join(format!("{}.snap", id));
        tokio::fs::write(&file_path, serde_json::to_string(&snapshot)?).await?;

        info!("Created snapshot {} for session {}", id, session_id);
        Ok(snapshot)
    }

    /// 恢复快照
    pub async fn restore_snapshot(&self, snapshot_id: &str) -> anyhow::Result<Snapshot> {
        let file_path = self.snapshot_dir.join(format!("{}.snap", snapshot_id));
        
        if !file_path.exists() {
            return Err(anyhow::anyhow!("Snapshot not found: {}", snapshot_id));
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let snapshot: Snapshot = serde_json::from_str(&content)?;

        info!("Restored snapshot {}", snapshot_id);
        Ok(snapshot)
    }

    /// 列出所有快照
    pub async fn list_snapshots(
        &self,
        session_id: Option<&SessionId>,
    ) -> anyhow::Result<Vec<SnapshotMetadata>> {
        if !self.snapshot_dir.exists() {
            return Ok(Vec::new());
        }

        let mut snapshots = Vec::new();
        
        let mut entries = tokio::fs::read_dir(&self.snapshot_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            
            if path.extension().and_then(|e| e.to_str()) == Some("snap") {
                let content = tokio::fs::read_to_string(&path).await?;
                
                match serde_json::from_str::<Snapshot>(&content) {
                    Ok(snapshot) => {
                        if session_id.is_none() || Some(&snapshot.metadata.session_id) == session_id {
                            snapshots.push(snapshot.metadata);
                        }
                    }
                    Err(e) => {
                        debug!("Failed to parse snapshot {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(snapshots)
    }

    /// 删除快照
    pub async fn delete_snapshot(&self, snapshot_id: &str) -> anyhow::Result<()> {
        let file_path = self.snapshot_dir.join(format!("{}.snap", snapshot_id));
        
        if file_path.exists() {
            tokio::fs::remove_file(&file_path).await?;
            info!("Deleted snapshot {}", snapshot_id);
        }

        Ok(())
    }
}
