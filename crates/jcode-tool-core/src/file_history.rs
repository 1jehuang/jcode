//! # 文件历史与回滚
//!
//! 源自 Claude Code 的 `fileHistory.ts`，提供按消息的文件快照和回滚能力。
//!
//! ## 能力
//! - 编辑前自动备份文件
//! - 按消息 ID 创建/管理快照
//! - 回滚到任意历史快照
//! - 跨会话备份迁移

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 单个文件的快照
#[derive(Debug, Clone)]
pub struct FileSnapshot {
    /// 文件路径（相对/绝对）
    pub path: PathBuf,
    /// 文件内容
    pub content: String,
    /// 快照创建时间
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 快照消息 ID
    pub message_id: String,
}

/// 单个消息的所有文件快照
#[derive(Debug, Clone)]
pub struct MessageSnapshot {
    /// 消息 ID
    pub message_id: String,
    /// 该消息前的所有文件快照
    pub files: Vec<FileSnapshot>,
    /// 时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 文件历史管理器
/// 源自 Claude Code 的 `FileHistory` (fileHistory.ts)
pub struct FileHistory {
    /// 快照列表（按消息 ID 索引）
    snapshots: Vec<MessageSnapshot>,
    /// 最大快照数
    max_snapshots: usize,
    /// 备份根目录
    backup_dir: Option<PathBuf>,
    /// 当前追踪的文件路径
    tracked_files: Vec<PathBuf>,
}

impl FileHistory {
    pub fn new() -> Self {
        Self {
            snapshots: Vec::new(),
            max_snapshots: 100,
            backup_dir: None,
            tracked_files: Vec::new(),
        }
    }

    /// 设置备份目录
    pub fn with_backup_dir(mut self, dir: PathBuf) -> Self {
        self.backup_dir = Some(dir);
        self
    }

    /// 设置最大快照数
    pub fn with_max_snapshots(mut self, max: usize) -> Self {
        self.max_snapshots = max;
        self
    }

    /// 开始追踪一个文件
    pub fn track_file(&mut self, path: PathBuf) {
        if !self.tracked_files.contains(&path) {
            self.tracked_files.push(path);
        }
    }

    /// 停止追踪一个文件
    pub fn untrack_file(&mut self, path: &Path) {
        self.tracked_files.retain(|p| p != path);
    }

    /// 获取追踪的文件列表
    pub fn tracked_files(&self) -> &[PathBuf] {
        &self.tracked_files
    }

    /// 对将要编辑的文件进行备份（编辑前调用）
    /// 源自 Claude Code 的 `fileHistoryTrackEdit()`
    pub fn track_edit(&mut self, file_path: &Path, content: &str, message_id: &str) -> anyhow::Result<()> {
        // 确保文件被追踪
        let path = file_path.to_path_buf();
        if !self.tracked_files.contains(&path) {
            self.tracked_files.push(path.clone());
        }

        let snapshot = FileSnapshot {
            path: path.clone(),
            content: content.to_string(),
            timestamp: chrono::Utc::now(),
            message_id: message_id.to_string(),
        };

        // 写入磁盘备份
        if let Some(ref backup_dir) = self.backup_dir {
            let file_backup = backup_dir
                .join(sanitize_path(&path))
                .join(&message_id)
                .with_extension("bak");
            if let Some(parent) = file_backup.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&file_backup, content);
        }

        // 查找或创建消息快照
        if let Some(msg_snap) = self.snapshots.iter_mut().rev().find(|s| s.message_id == message_id) {
            // 替换相同文件的旧快照
            msg_snap.files.retain(|f| f.path != path);
            msg_snap.files.push(snapshot);
        } else {
            self.snapshots.push(MessageSnapshot {
                message_id: message_id.to_string(),
                files: vec![snapshot],
                timestamp: chrono::Utc::now(),
            });
        }

        // 限制快照数量
        self.enforce_limit();
        Ok(())
    }

    /// 按消息 ID 创建新快照（捕获所有追踪文件的当前状态）
    /// 源自 Claude Code 的 `fileHistoryMakeSnapshot()`
    pub fn make_snapshot(&mut self, message_id: &str) -> anyhow::Result<MessageSnapshot> {
        let mut files = Vec::new();
        for path in &self.tracked_files {
            if path.exists() {
                let content = std::fs::read_to_string(path)
                    .unwrap_or_else(|_| String::new());
                files.push(FileSnapshot {
                    path: path.clone(),
                    content,
                    timestamp: chrono::Utc::now(),
                    message_id: message_id.to_string(),
                });
            }
        }

        let snapshot = MessageSnapshot {
            message_id: message_id.to_string(),
            files,
            timestamp: chrono::Utc::now(),
        };

        self.snapshots.push(snapshot.clone());
        self.enforce_limit();
        Ok(snapshot)
    }

    /// 回滚到指定消息的快照
    /// 源自 Claude Code 的 `fileHistoryRewind()`
    pub fn rewind_to(&self, message_id: &str) -> anyhow::Result<RewindResult> {
        // 找到目标消息及之前的所有快照
        let index = self.snapshots.iter().position(|s| s.message_id == message_id);
        let index = match index {
            Some(i) => i,
            None => anyhow::bail!("Snapshot not found for message: {}", message_id),
        };

        // 收集恢复到该消息时的文件状态
        let mut restored_files = Vec::new();
        let mut latest_state: HashMap<PathBuf, &FileSnapshot> = HashMap::new();

        for snap in &self.snapshots[..=index] {
            for file in &snap.files {
                latest_state.insert(file.path.clone(), file);
            }
        }

        // 执行文件恢复
        for (path, snapshot) in &latest_state {
            // 查找该文件的最新备份
            if let Some(ref backup_dir) = self.backup_dir {
                // 优先使用目标消息的备份
                let target_backup = backup_dir
                    .join(sanitize_path(path))
                    .join(message_id)
                    .with_extension("bak");
                if target_backup.exists() {
                    if let Ok(content) = std::fs::read_to_string(&target_backup) {
                        std::fs::write(path, &content)?;
                        restored_files.push(RestoredFile {
                            path: path.clone(),
                            content,
                            from_message: message_id.to_string(),
                        });
                        continue;
                    }
                }
            }

            // 回退到内存中的快照内容
            std::fs::write(path, &snapshot.content)?;
            restored_files.push(RestoredFile {
                path: path.clone(),
                content: snapshot.content.clone(),
                from_message: message_id.to_string(),
            });
        }

        let total_restored = restored_files.len();
        Ok(RewindResult {
            target_message: message_id.to_string(),
            restored_files,
            total_restored,
        })
    }

    /// 获取快照列表
    pub fn snapshots(&self) -> &[MessageSnapshot] {
        &self.snapshots
    }

    /// 获取指定消息的快照
    pub fn get_snapshot(&self, message_id: &str) -> Option<&MessageSnapshot> {
        self.snapshots.iter().find(|s| s.message_id == message_id)
    }

    /// 跨会话复制备份
    /// 源自 Claude Code 的 `copyFileHistoryForResume()`
    pub fn copy_backups_for_resume(&self, target_dir: &Path) -> anyhow::Result<()> {
        if let Some(ref source_dir) = self.backup_dir {
            if source_dir.exists() {
                let _ = std::fs::create_dir_all(target_dir);
                // 递归复制备份
                copy_dir_recursive(source_dir, target_dir)?;
            }
        }
        Ok(())
    }

    /// 限制快照数量
    fn enforce_limit(&mut self) {
        while self.snapshots.len() > self.max_snapshots {
            let removed = self.snapshots.remove(0);
            // 清理磁盘备份
            if let Some(ref backup_dir) = self.backup_dir {
                for file in &removed.files {
                    let backup = backup_dir
                        .join(sanitize_path(&file.path))
                        .join(&file.message_id)
                        .with_extension("bak");
                    let _ = std::fs::remove_file(&backup);
                }
            }
        }
    }
}

impl Default for FileHistory {
    fn default() -> Self { Self::new() }
}

/// 回滚结果
#[derive(Debug, Clone)]
pub struct RewindResult {
    /// 目标消息 ID
    pub target_message: String,
    /// 已恢复的文件
    pub restored_files: Vec<RestoredFile>,
    /// 恢复的文件总数
    pub total_restored: usize,
}

/// 单个恢复的文件
#[derive(Debug, Clone)]
pub struct RestoredFile {
    pub path: PathBuf,
    pub content: String,
    pub from_message: String,
}

/// 清理路径（移除不安全字符）
fn sanitize_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy().replace(['/', '\\', ':'], "_");
    PathBuf::from(s)
}

/// 递归复制目录
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let entry_type = entry.file_type()?;
            let src_path = entry.path();
            let rel = src_path.strip_prefix(src).unwrap_or(&src_path);
            let dst_path = dst.join(rel);

            if entry_type.is_dir() {
                std::fs::create_dir_all(&dst_path)?;
                copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                let _ = std::fs::copy(&src_path, &dst_path);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_track_and_rewind() {
        let dir = std::env::temp_dir().join("jcode-test-fh");
        let _ = fs::create_dir_all(&dir);

        let file_path = dir.join("test.txt");
        fs::write(&file_path, "original content").unwrap();

        let mut fh = FileHistory::new();
        fh.track_file(file_path.clone());

        // 编辑后创建快照
        fs::write(&file_path, "modified content").unwrap();
        fh.track_edit(&file_path, "original content", "msg-1").unwrap();

        // 再次编辑
        fs::write(&file_path, "final content").unwrap();
        fh.track_edit(&file_path, "modified content", "msg-2").unwrap();

        // 回滚到 msg-1
        let result = fh.rewind_to("msg-1").unwrap();
        assert_eq!(result.total_restored, 1);

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "original content");

        // 清理
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_snapshot_limit() {
        let mut fh = FileHistory::new();
        fh.max_snapshots = 3;

        for i in 0..5 {
            fh.make_snapshot(&format!("msg-{}", i)).unwrap();
        }

        assert_eq!(fh.snapshots().len(), 3);
        assert_eq!(fh.snapshots()[0].message_id, "msg-2");
    }

    #[test]
    fn test_rewind_nonexistent() {
        let fh = FileHistory::new();
        let result = fh.rewind_to("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_copy_backups() {
        let src = std::env::temp_dir().join("jcode-test-fh-src");
        let dst = std::env::temp_dir().join("jcode-test-fh-dst");
        let _ = fs::create_dir_all(&src);

        let mut fh = FileHistory::new().with_backup_dir(src.clone());
        fh.track_file(PathBuf::from("test.txt"));

        // Track an edit (no disk backup since backup_dir exists but no actual file)
        fh.track_edit(Path::new("test.txt"), "content", "msg-1").unwrap();

        // Copy backups
        let result = fh.copy_backups_for_resume(&dst);
        assert!(result.is_ok());

        // Cleanup
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }
}
