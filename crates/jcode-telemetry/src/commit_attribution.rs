//! # 提交归属追踪 — Telemetry 数据源
//!
//! 源自 Claude Code `src/utils/commitAttribution.ts`
//!
//! 追踪 Claude/jcode 对项目文件的修改贡献，计算每次提交中的归因百分比。
//! 用于 Telemetry 数据上报，帮助用户了解 AI 辅助开发的实际贡献度。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// 文件变更类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Modification,
    Creation,
    Deletion,
}

/// 文件归属记录
#[derive(Debug, Clone)]
pub struct FileAttribution {
    /// 文件路径
    pub path: PathBuf,
    /// 变更类型
    pub kind: FileChangeKind,
    /// Claude 贡献的字符数
    pub claude_chars: usize,
    /// 文件总字符数
    pub total_chars: usize,
    /// 变更时间
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 提交归因结果
#[derive(Debug, Clone)]
pub struct CommitAttribution {
    /// 提交的起始 HEAD
    pub start_head: String,
    /// 当前 HEAD
    pub current_head: String,
    /// 总修改文件数
    pub total_files_changed: usize,
    /// Claude 贡献的文件数
    pub claude_files: usize,
    /// Claude 贡献的字符数
    pub claude_chars: usize,
    /// 总字符数
    pub total_chars: usize,
    /// Claude 贡献百分比
    pub claude_percentage: f64,
    /// 修改的文件列表
    pub files: Vec<FileAttribution>,
    /// 会话中的 prompt 数
    pub prompt_count: u32,
}

/// 提交归属追踪管理器
///
/// 源自 Claude Code 的 `AttributionState` + `CommitAttributionTracker`
pub struct CommitAttributionTracker {
    /// 被追踪的文件及其归属
    files: Mutex<HashMap<PathBuf, FileAttribution>>,
    /// 会话起始 HEAD
    start_head: Mutex<String>,
    /// Prompt 计数
    prompt_count: Mutex<u32>,
    /// 是否启用
    enabled: bool,
}

impl CommitAttributionTracker {
    pub fn new(enabled: bool) -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            start_head: Mutex::new(String::new()),
            prompt_count: Mutex::new(0),
            enabled,
        }
    }

    /// 设置 Git HEAD（会话开始时）
    pub fn set_start_head(&self, head: String) {
        if !self.enabled { return; }
        if let Ok(mut h) = self.start_head.lock() {
            *h = head;
        }
    }

    /// 追踪文件修改
    /// 源自 Claude Code 的 `trackFileModification()`
    pub fn track_modification(&self, path: PathBuf, new_content: &str) {
        if !self.enabled { return; }
        if let Ok(mut files) = self.files.lock() {
            files.insert(path.clone(), FileAttribution {
                path,
                kind: FileChangeKind::Modification,
                claude_chars: new_content.len(),
                total_chars: new_content.len(),
                timestamp: chrono::Utc::now(),
            });
        }
    }

    /// 追踪文件创建
    /// 源自 Claude Code 的 `trackFileCreation()`
    pub fn track_creation(&self, path: PathBuf, content: &str) {
        if !self.enabled { return; }
        if let Ok(mut files) = self.files.lock() {
            files.insert(path.clone(), FileAttribution {
                path,
                kind: FileChangeKind::Creation,
                claude_chars: content.len(),
                total_chars: content.len(),
                timestamp: chrono::Utc::now(),
            });
        }
    }

    /// 追踪文件删除
    /// 源自 Claude Code 的 `trackFileDeletion()`
    pub fn track_deletion(&self, path: PathBuf) {
        if !self.enabled { return; }
        if let Ok(mut files) = self.files.lock() {
            files.insert(path, FileAttribution {
                path: PathBuf::new(), // placeholder
                kind: FileChangeKind::Deletion,
                claude_chars: 0,
                total_chars: 0,
                timestamp: chrono::Utc::now(),
            });
        }
    }

    /// 增加 prompt 计数
    pub fn increment_prompt_count(&self) {
        if !self.enabled { return; }
        if let Ok(mut count) = self.prompt_count.lock() {
            *count += 1;
        }
    }

    /// 批量追踪文件变更（避免 O(n²) 成本）
    /// 源自 Claude Code 的 `trackBulkFileChanges()`
    pub fn track_bulk(&self, changes: Vec<(PathBuf, FileChangeKind, String)>) {
        if !self.enabled { return; }
        if let Ok(mut files) = self.files.lock() {
            for (path, kind, content) in changes {
                let chars = content.len();
                files.insert(path.clone(), FileAttribution {
                    path,
                    kind,
                    claude_chars: chars,
                    total_chars: chars,
                    timestamp: chrono::Utc::now(),
                });
            }
        }
    }

    /// 计算最终归因数据
    /// 源自 Claude Code 的 `calculateCommitAttribution()`
    pub fn calculate(&self, current_head: String) -> CommitAttribution {
        let files = self.files.lock().unwrap();
        let total: Vec<FileAttribution> = files.values().cloned().collect();
        let prompt_count = *self.prompt_count.lock().unwrap();

        let total_files = total.len();
        let claude_files = total.len(); // 所有追踪的文件都是 Claude 贡献的
        let claude_chars: usize = total.iter().map(|f| f.claude_chars).sum();
        let total_chars: usize = total.iter().map(|f| f.total_chars).sum();
        let claude_pct = if total_chars > 0 {
            (claude_chars as f64 / total_chars as f64) * 100.0
        } else {
            0.0
        };

        let start_head = self.start_head.lock().unwrap().clone();

        CommitAttribution {
            start_head,
            current_head,
            total_files_changed: total_files,
            claude_files,
            claude_chars,
            total_chars,
            claude_percentage: claude_pct,
            files: total,
            prompt_count,
        }
    }

    /// 获取追踪的文件列表
    pub fn tracked_files(&self) -> Vec<PathBuf> {
        self.files.lock().unwrap().keys().cloned().collect()
    }

    /// 清除状态（新会话时调用）
    pub fn reset(&self) {
        if !self.enabled { return; }
        if let Ok(mut files) = self.files.lock() {
            files.clear();
        }
        if let Ok(mut count) = self.prompt_count.lock() {
            *count = 0;
        }
        if let Ok(mut head) = self.start_head.lock() {
            head.clear();
        }
    }

    /// 序列化为快照消息
    /// 源自 Claude Code 的 `stateToSnapshotMessage()`
    pub fn to_snapshot(&self) -> AttributionSnapshot {
        let files = self.files.lock().unwrap().values().cloned().collect();
        let prompt_count = *self.prompt_count.lock().unwrap();
        let start_head = self.start_head.lock().unwrap().clone();

        AttributionSnapshot {
            files,
            prompt_count,
            start_head,
            timestamp: chrono::Utc::now(),
        }
    }

    /// 从快照恢复状态
    /// 源自 Claude Code 的 `restoreAttributionStateFromSnapshots()`
    pub fn restore_from_snapshot(&self, snapshot: AttributionSnapshot) {
        if !self.enabled { return; }
        if let Ok(mut files) = self.files.lock() {
            for file in snapshot.files {
                files.insert(file.path.clone(), file);
            }
        }
        if let Ok(mut count) = self.prompt_count.lock() {
            *count += snapshot.prompt_count;
        }
    }
}

/// 归属快照（用于持久化/恢复）
#[derive(Debug, Clone)]
pub struct AttributionSnapshot {
    pub files: Vec<FileAttribution>,
    pub prompt_count: u32,
    pub start_head: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_track_modification() {
        let tracker = CommitAttributionTracker::new(true);
        tracker.track_modification(
            PathBuf::from("src/main.rs"),
            "fn main() { println!(\"hello\"); }",
        );
        assert_eq!(tracker.tracked_files().len(), 1);
    }

    #[test]
    fn test_bulk_tracking() {
        let tracker = CommitAttributionTracker::new(true);
        tracker.track_bulk(vec![
            (PathBuf::from("src/a.rs"), FileChangeKind::Modification, "content a".to_string()),
            (PathBuf::from("src/b.rs"), FileChangeKind::Creation, "content b".to_string()),
        ]);
        assert_eq!(tracker.tracked_files().len(), 2);
    }

    #[test]
    fn test_calculate_attribution() {
        let tracker = CommitAttributionTracker::new(true);
        tracker.set_start_head("abc123".to_string());
        tracker.track_modification(PathBuf::from("src/main.rs"), "hello world");
        tracker.track_creation(PathBuf::from("src/lib.rs"), "pub fn foo() {}");
        tracker.increment_prompt_count();

        let attribution = tracker.calculate("def456".to_string());
        assert_eq!(attribution.start_head, "abc123");
        assert_eq!(attribution.current_head, "def456");
        assert_eq!(attribution.total_files_changed, 2);
        assert_eq!(attribution.prompt_count, 1);
        assert!(attribution.claude_percentage > 0.0);
    }

    #[test]
    fn test_disabled_tracker() {
        let tracker = CommitAttributionTracker::new(false);
        tracker.track_modification(PathBuf::from("src/main.rs"), "content");
        assert!(tracker.tracked_files().is_empty());
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let tracker = CommitAttributionTracker::new(true);
        tracker.track_modification(PathBuf::from("src/main.rs"), "content");
        tracker.increment_prompt_count();

        let snapshot = tracker.to_snapshot();
        assert_eq!(snapshot.files.len(), 1);

        let tracker2 = CommitAttributionTracker::new(true);
        tracker2.restore_from_snapshot(snapshot);
        assert_eq!(tracker2.tracked_files().len(), 1);
    }

    #[test]
    fn test_reset() {
        let tracker = CommitAttributionTracker::new(true);
        tracker.track_modification(PathBuf::from("src/main.rs"), "content");
        tracker.reset();
        assert!(tracker.tracked_files().is_empty());
    }
}
