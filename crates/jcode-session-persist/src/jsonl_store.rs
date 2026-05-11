//! JSONL Store — 会话持久化存储
//!
//! ## 核心能力
//! - JSON Lines 格式存储
//! - 增量写入支持
//! - 事务性操作

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use tracing::{debug, error, info};

/// JSONL 存储条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonlEntry {
    pub id: String,
    pub timestamp: u64,
    pub data: serde_json::Value,
}

/// JSONL 存储管理器
pub struct JsonlStore {
    path: std::path::PathBuf,
}

impl JsonlStore {
    /// 创建新的 JSONL 存储
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf() }
    }

    /// 追加一条记录
    pub fn append(&self, entry: &JsonlEntry) -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let line = serde_json::to_string(entry)?;
        writeln!(file, "{}", line)?;

        debug!("Appended entry {} to {}", entry.id, self.path.display());
        Ok(())
    }

    /// 读取所有记录
    pub fn read_all(&self) -> anyhow::Result<Vec<JsonlEntry>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = std::fs::File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            
            match serde_json::from_str::<JsonlEntry>(&line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    warn!("Failed to parse JSONL line: {}", e);
                }
            }
        }

        info!("Read {} entries from {}", entries.len(), self.path.display());
        Ok(entries)
    }

    /// 清空存储
    pub fn clear(&self) -> anyhow::Result<()> {
        if self.path.exists() {
            std::fs::write(&self.path, "")?;
            info!("Cleared {}", self.path.display());
        }
        Ok(())
    }
}
