//! # Checkpoint 检查点系统
//!
//! 从 Claude Code 移植的文件历史检查点系统

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBackup {
    pub backup_file: Option<String>,
    pub version: u32,
    pub backup_time: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub message_id: String,
    pub file_backups: HashMap<String, FileBackup>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHistoryState {
    pub snapshots: Vec<FileSnapshot>,
    pub tracked_files: Vec<String>,
    pub sequence: u32,
}

impl FileHistoryState {
    const MAX: usize = 100;
    pub fn new() -> Self {
        Self { snapshots: Vec::new(), tracked_files: Vec::new(), sequence: 0 }
    }
}

pub struct CheckpointManager {
    state: Arc<RwLock<FileHistoryState>>,
    storage_dir: PathBuf,
    enabled: bool,
}

impl CheckpointManager {
    pub fn new(session_id: impl Into<String>, enabled: bool) -> Self {
        let dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
            .join(".jcode").join("file-history").join(session_id.into());
        Self { state: Arc::new(RwLock::new(FileHistoryState::new())), storage_dir: dir, enabled }
    }

    /// 编辑前备份文件
    pub async fn track_edit(&self, file_path: &str) -> Result<(), String> {
        if !self.enabled { return Ok(()); }
        let path = Path::new(file_path);
        let abs = if path.is_absolute() { path.to_path_buf() }
            else { std::env::current_dir().map_err(|e| e.to_string())?.join(path) };
        if !abs.exists() { return Ok(()); }
        let content = std::fs::read(&abs).map_err(|e| e.to_string())?;
        let hash = format!("{:x}", Sha256::digest(&content));
        let mut state = self.state.write().await;
        state.sequence += 1;
        let name = format!("{}@v{}", &hash[..16], state.sequence);
        let bp = self.storage_dir.join(&name);
        std::fs::create_dir_all(bp.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&bp, &content).map_err(|e| e.to_string())?;
        if !state.tracked_files.contains(&file_path.to_string()) {
            state.tracked_files.push(file_path.to_string());
        }
        info!("Checkpoint: backed up {} at v{}", file_path, state.sequence);
        Ok(())
    }

    /// 为所有追踪文件创建快照
    pub async fn make_snapshot(&self, message_id: &str) -> Result<(), String> {
        if !self.enabled { return Ok(()); }
        let mut state = self.state.write().await;
        state.sequence += 1;
        let mut backups = HashMap::new();
        for tf in &state.tracked_files {
            let abs = if Path::new(tf).is_absolute() { PathBuf::from(tf) }
                else { std::env::current_dir().map_err(|e| e.to_string())?.join(tf) };
            if !abs.exists() { continue; }
            let content = std::fs::read(&abs).map_err(|e| e.to_string())?;
            let hash = format!("{:x}", Sha256::digest(&content));
            let name = format!("{}@v{}", &hash[..16], state.sequence);
            let bp = self.storage_dir.join(&name);
            std::fs::create_dir_all(bp.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&bp, &content).map_err(|e| e.to_string())?;
            backups.insert(tf.clone(), FileBackup {
                backup_file: Some(name), version: state.sequence, backup_time: Utc::now(),
            });
        }
        state.snapshots.push(FileSnapshot {
            message_id: message_id.to_string(), file_backups: backups, timestamp: Utc::now(),
        });
        while state.snapshots.len() > FileHistoryState::MAX { state.snapshots.remove(0); }
        Ok(())
    }

    /// 回滚到指定消息的快照
    pub async fn rewind(&self, message_id: &str) -> Result<Vec<String>, String> {
        let state = self.state.read().await;
        let snap = state.snapshots.iter()
            .find(|s| s.message_id == message_id)
            .ok_or_else(|| format!("No snapshot for {}", message_id))?;
        let mut restored = Vec::new();
        for (fp, bak) in &snap.file_backups {
            let name = bak.backup_file.as_ref().ok_or("no backup")?;
            let bp = self.storage_dir.join(name);
            if !bp.exists() { continue; }
            let content = std::fs::read(&bp).map_err(|e| e.to_string())?;
            std::fs::write(Path::new(fp), &content).map_err(|e| e.to_string())?;
            restored.push(fp.clone());
        }
        Ok(restored)
    }

    pub async fn summary(&self) -> CheckpointSummary {
        let state = self.state.read().await;
        CheckpointSummary {
            enabled: self.enabled, total_snapshots: state.snapshots.len(),
            tracked_files: state.tracked_files.len(), sequence: state.sequence,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointSummary {
    pub enabled: bool, pub total_snapshots: usize, pub tracked_files: usize, pub sequence: u32,
}
