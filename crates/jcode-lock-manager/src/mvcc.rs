//! MVCC — 多版本并发控制
//!
//! 为多 Agent 并发编辑提供乐观锁 + 版本冲突检测。
//! 每个文件维护一个单调递增的版本号，写操作前必须匹配目标版本。
//!
//! ## 工作流
//! ```
//! Reader Agent:  read(file) -> 获取 version=3
//! Writer Agent:  write(file, expected_version=3) -> 验证版本
//!      v version==3
//! 写入成功, version -> 4
//!      v version≠3 (另一个 Agent 已写入)
//! 冲突，返回 ConflictError，Agent 需重读
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

/// 文件版本号 — 单调递增的 u64 计数器
pub type FileVersion = u64;

/// 会话 / Agent 标识
pub type SessionId = String;

/// MVCC 冲突错误
#[derive(Debug, Clone)]
pub struct ConflictError {
    pub file: PathBuf,
    pub expected_version: FileVersion,
    pub actual_version: FileVersion,
    pub locked_by: Option<SessionId>,
}

impl std::fmt::Display for ConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MVCC conflict on {:?}: expected v{}, actual v{} (locked by {:?})",
            self.file, self.expected_version, self.actual_version, self.locked_by
        )
    }
}

impl std::error::Error for ConflictError {}

/// 文件锁状态
#[derive(Debug, Clone)]
struct FileLockState {
    /// 当前版本号（每次写成功 +1）
    version: FileVersion,
    /// 持有写锁的会话 ID
    write_locked_by: Option<SessionId>,
}

/// MVCC 管理器 — 提供文件级别的乐观锁并发控制
pub struct MvccManager {
    state: Arc<RwLock<HashMap<PathBuf, FileLockState>>>,
    stats: Arc<RwLock<MvccStats>>,
}

#[derive(Debug, Clone, Default)]
pub struct MvccStats {
    pub total_reads: u64,
    pub total_writes: u64,
    pub total_conflicts: u64,
    pub total_lock_acquires: u64,
    pub total_lock_releases: u64,
}

impl MvccManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(MvccStats::default())),
        }
    }

    /// 获取文件的当前版本号（读操作）
    pub async fn read_version(&self, file: &PathBuf) -> FileVersion {
        let state = self.state.read().await;
        let ver = state.get(file).map(|s| s.version).unwrap_or(0);
        self.stats.write().await.total_reads += 1;
        ver
    }

    /// 尝试写操作（乐观锁模式）
    /// 如果 `expected_version` 匹配当前版本，则版本号+1，返回新版本
    /// 否则返回 `ConflictError`
    pub async fn try_write(
        &self,
        file: &PathBuf,
        expected_version: FileVersion,
        session: &SessionId,
    ) -> Result<FileVersion, ConflictError> {
        let mut state = self.state.write().await;
        let entry = state.entry(file.clone()).or_insert(FileLockState {
            version: 0,
            write_locked_by: None,
        });

        if let Some(ref locker) = entry.write_locked_by {
            if locker != session {
                self.stats.write().await.total_conflicts += 1;
                return Err(ConflictError {
                    file: file.clone(),
                    expected_version,
                    actual_version: entry.version,
                    locked_by: Some(locker.clone()),
                });
            }
        }

        if entry.version != expected_version {
            self.stats.write().await.total_conflicts += 1;
            return Err(ConflictError {
                file: file.clone(),
                expected_version,
                actual_version: entry.version,
                locked_by: None,
            });
        }

        entry.version += 1;
        self.stats.write().await.total_writes += 1;
        Ok(entry.version)
    }

    /// 获取写锁（排他锁）
    pub async fn acquire_write_lock(
        &self,
        file: &PathBuf,
        session: &SessionId,
    ) -> Result<FileVersion, ConflictError> {
        let mut state = self.state.write().await;
        let entry = state.entry(file.clone()).or_insert(FileLockState {
            version: 0,
            write_locked_by: None,
        });

        if let Some(ref locker) = entry.write_locked_by {
            if locker != session {
                return Err(ConflictError {
                    file: file.clone(),
                    expected_version: entry.version,
                    actual_version: entry.version,
                    locked_by: Some(locker.clone()),
                });
            }
            // Same session: already locked, return current version
            return Ok(entry.version);
        }

        entry.write_locked_by = Some(session.clone());
        self.stats.write().await.total_lock_acquires += 1;
        debug!("Write lock acquired on {:?} by {}", file, session);
        Ok(entry.version)
    }

    /// 释放写锁
    pub async fn release_write_lock(&self, file: &PathBuf, session: &SessionId) {
        let mut state = self.state.write().await;
        if let Some(entry) = state.get_mut(file) {
            if entry.write_locked_by.as_deref() == Some(session.as_str()) {
                entry.write_locked_by = None;
                self.stats.write().await.total_lock_releases += 1;
                debug!("Write lock released on {:?} by {}", file, session);
            }
        }
    }

    /// 检查文件是否有写锁
    pub async fn is_locked(&self, file: &PathBuf) -> bool {
        let state = self.state.read().await;
        state.get(file).and_then(|s| s.write_locked_by.as_ref()).is_some()
    }

    /// 获取统计信息
    pub async fn stats(&self) -> MvccStats {
        self.stats.read().await.clone()
    }

    /// 清理不再需要的文件状态
    pub async fn cleanup(&self, max_entries: usize) {
        let mut state = self.state.write().await;
        if state.len() > max_entries {
            state.clear();
            debug!("MVCC state cleared ({} entries)", state.len());
        }
    }
}

impl Default for MvccManager {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_write_version() {
        let mvcc = MvccManager::new();
        let file = PathBuf::from("test.rs");

        let v0 = mvcc.read_version(&file).await;
        assert_eq!(v0, 0);

        let v1 = mvcc.try_write(&file, 0, "agent1").await.unwrap();
        assert_eq!(v1, 1);

        // Stale version should conflict
        let err = mvcc.try_write(&file, 0, "agent2").await.unwrap_err();
        assert_eq!(err.expected_version, 0);
        assert_eq!(err.actual_version, 1);

        // Correct version should succeed
        let v2 = mvcc.try_write(&file, 1, "agent2").await.unwrap();
        assert_eq!(v2, 2);
    }

    #[tokio::test]
    async fn test_write_lock() {
        let mvcc = MvccManager::new();
        let file = PathBuf::from("shared.rs");

        let v = mvcc.acquire_write_lock(&file, "agent1").await.unwrap();
        assert_eq!(v, 0);

        // Another agent can't acquire
        let err = mvcc.acquire_write_lock(&file, "agent2").await.unwrap_err();
        assert!(err.locked_by.as_deref() == Some("agent1"));

        // Same agent can re-acquire
        let v = mvcc.acquire_write_lock(&file, "agent1").await.unwrap();
        assert_eq!(v, 0);

        mvcc.release_write_lock(&file, "agent1").await;
        assert!(!mvcc.is_locked(&file).await);
    }
}
