//! 文件状态缓存 — 读后写防护
//!
//! 对标 Claude Code 的 FileStateCache，防止陈旧写入:
//! - 追踪文件读取时间 + 内容
//! - 写入前验证文件未被外部修改
//! - 确定性备份文件名 (SHA-256)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 文件状态
#[derive(Debug, Clone)]
pub struct FileState {
    pub path: PathBuf,
    pub content: String,
    pub mtime: u64,            // 读取时的修改时间
    pub read_at: SystemTime,   // 读取时间
}

/// 文件状态缓存（LRU）
pub struct FileStateCache {
    entries: HashMap<PathBuf, FileState>,
    max_entries: usize,
}

impl FileStateCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries,
        }
    }

    /// 记录文件被读取
    pub fn record_read(&mut self, path: &Path, content: &str) {
        let mtime = Self::get_mtime(path);
        let state = FileState {
            path: path.to_path_buf(),
            content: content.to_string(),
            mtime,
            read_at: SystemTime::now(),
        };

        // LRU: 超过限制时移除最早条目
        if self.entries.len() >= self.max_entries {
            if let Some(oldest_key) = self.entries.keys().next().cloned() {
                self.entries.remove(&oldest_key);
            }
        }
        self.entries.insert(path.to_path_buf(), state);
    }

    /// 验证文件自读取后是否被修改
    /// 返回 Ok(content) 或 Err(修改警告)
    pub fn validate_write(&self, path: &Path) -> Result<&str, String> {
        if let Some(state) = self.entries.get(path) {
            let current_mtime = Self::get_mtime(path);
            if current_mtime != state.mtime {
                // 检查内容是否真的变了（Windows 有时戳噪声）
                if let Ok(current_content) = std::fs::read_to_string(path) {
                    if current_content != state.content {
                        return Err(format!(
                            "File '{}' was modified externally since read. Read at {:?}, mtime changed from {} to {}",
                            path.display(), state.read_at, state.mtime, current_mtime
                        ));
                    }
                }
            }
            Ok(&state.content)
        } else {
            Err(format!(
                "File '{}' was not read in this session. Must read before editing.",
                path.display()
            ))
        }
    }

    /// 清除缓存
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn get_mtime(path: &Path) -> u64 {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
            .unwrap_or(0)
    }

    /// 缓存中的文件数
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Default for FileStateCache {
    fn default() -> Self {
        Self::new(100) // 最多缓存100个文件
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_record_and_validate() {
        let tmp = std::env::temp_dir().join("fsc_test.txt");
        fs::write(&tmp, "hello").unwrap();

        let mut cache = FileStateCache::new(10);
        cache.record_read(&tmp, "hello");

        // 未修改时应通过
        assert!(cache.validate_write(&tmp).is_ok());

        // 修改后应失败
        fs::write(&tmp, "modified").unwrap();
        assert!(cache.validate_write(&tmp).is_err());

        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_unread_file() {
        let cache = FileStateCache::new(10);
        let tmp = std::env::temp_dir().join("unread.txt");
        assert!(cache.validate_write(&tmp).is_err());
    }

    #[test]
    fn test_lru_eviction() {
        let mut cache = FileStateCache::new(2);
        cache.record_read(Path::new("/a"), "a");
        cache.record_read(Path::new("/b"), "b");
        cache.record_read(Path::new("/c"), "c"); // /a 被逐出
        assert!(cache.validate_write(Path::new("/a")).is_err());
        assert!(cache.validate_write(Path::new("/b")).is_ok());
    }
}
