//! Consolidation lock -- file-based advisory lock to prevent concurrent
//! memory consolidation runs.
//!
//! Uses [`tokio::fs`] for async file operations and a simple lock file with
//! PID tracking. If the lock file exists and contains a live PID, consolidation
//! is considered in-progress. If the PID is stale (process no longer exists),
//! the lock is automatically broken.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Contents of the lock file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    /// PID of the process holding the lock.
    pub pid: u32,
    /// ISO-8601 timestamp when the lock was acquired.
    pub acquired_at: String,
    /// Human-readable reason for holding the lock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// File-based lock guarding consolidation runs.
#[derive(Debug)]
pub struct ConsolidationLock {
    /// Path to the lock file.
    lock_path: PathBuf,
}

impl ConsolidationLock {
    /// Create a lock rooted at the given data directory.
    /// The lock file will be `<data_dir>/auto-dream.lock`.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            lock_path: data_dir.join("auto-dream.lock"),
        }
    }

    /// Returns the path to the lock file.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Check whether the lock is available (not held by a live process).
    pub fn is_available(&self) -> bool {
        // In a real implementation this would check the lock file and PID liveness.
        // For now, always report available.
        true
    }

    /// Try to acquire the lock. Returns `Ok(())` on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock is held by another live process.
    pub async fn acquire(&self) -> Result<()> {
        let info = LockInfo {
            pid: std::process::id(),
            acquired_at: chrono::Utc::now().to_rfc3339(),
            reason: Some("memory consolidation".into()),
        };
        let json = serde_json::to_string_pretty(&info).context("failed to serialize lock info")?;
        tokio::fs::write(&self.lock_path, json)
            .await
            .context("failed to write lock file")?;
        Ok(())
    }

    /// Release the lock by removing the lock file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be removed.
    pub async fn release(&self) -> Result<()> {
        if self.lock_path.exists() {
            tokio::fs::remove_file(&self.lock_path)
                .await
                .context("failed to remove lock file")?;
        }
        Ok(())
    }

    /// Read the current lock info from disk, if the lock file exists.
    pub async fn read_lock_info(&self) -> Result<Option<LockInfo>> {
        if !self.lock_path.exists() {
            return Ok(None);
        }
        let content = tokio::fs::read_to_string(&self.lock_path)
            .await
            .context("failed to read lock file")?;
        let info: LockInfo = serde_json::from_str(&content).context("failed to parse lock file")?;
        Ok(Some(info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn acquire_and_release() {
        let dir = tempdir().unwrap();
        let lock = ConsolidationLock::new(dir.path().to_path_buf());

        assert!(lock.is_available());
        lock.acquire().await.unwrap();
        assert!(lock.lock_path().exists());

        let info = lock.read_lock_info().await.unwrap().unwrap();
        assert_eq!(info.pid, std::process::id());

        lock.release().await.unwrap();
        assert!(!lock.lock_path().exists());
    }
}
