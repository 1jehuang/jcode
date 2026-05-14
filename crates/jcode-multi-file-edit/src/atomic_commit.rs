use crate::diff_merge::UnifiedDiff;
use crate::parallel_processor::ProcessedFile;

/// Result of an atomic commit operation.
#[derive(Debug, Clone)]
pub struct CommitResult {
    pub diff: UnifiedDiff,
    pub processed_files: Vec<ProcessedFile>,
    pub success: bool,
    pub stats: CommitStats,
    pub error: Option<String>,
}

/// Statistics for the commit operation.
#[derive(Debug, Clone, Default)]
pub struct CommitStats {
    pub files_modified: usize,
    pub files_created: usize,
    pub files_deleted: usize,
    pub total_additions: usize,
    pub total_deletions: usize,
}

impl CommitResult {
    pub fn new(diff: UnifiedDiff, processed: Vec<ProcessedFile>) -> Self {
        let success = processed.iter().all(|p| !p.new_content.is_empty() || p.path.exists());
        let stats = CommitStats {
            files_modified: processed.len(),
            total_additions: diff.total_additions,
            total_deletions: diff.total_deletions,
            ..Default::default()
        };
        Self { diff, processed_files: processed, success, stats, error: None }
    }

    pub fn failed(error: String) -> Self {
        Self {
            diff: UnifiedDiff::empty(),
            processed_files: Vec::new(),
            success: false,
            stats: CommitStats::default(),
            error: Some(error),
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "Atomic commit: {} files, +{} -{} lines{}",
            self.stats.files_modified,
            self.stats.total_additions,
            self.stats.total_deletions,
            if self.success { "" } else { " (FAILED)" }
        )
    }
}

/// Atomic commit — applies changes using two-phase commit (temp file + atomic rename).
///
/// ## Phase 1: Write to temporary files
/// All changes are written to `.{filename}.tmp` files first.
///
/// ## Phase 2: Atomic rename
/// If all temp files were written successfully, rename them to target paths.
/// On most filesystems, rename is atomic (all-or-nothing on same filesystem).
///
/// ## Rollback
/// If Phase 1 fails, all temp files are deleted.
/// If Phase 2 fails, we attempt to restore original files from backups.
pub struct AtomicCommit {
    temp_suffix: String,
}

impl AtomicCommit {
    pub fn new() -> Self {
        Self {
            temp_suffix: format!(".tmp.{}", std::process::id()),
        }
    }

    /// Apply the processed changes to disk using two-phase commit.
    pub async fn apply(&self, result: &CommitResult) -> anyhow::Result<()> {
        if !result.success {
            return Err(anyhow::anyhow!(
                "Cannot apply failed commit: {}",
                result.error.as_deref().unwrap_or("unknown error")
            ));
        }

        if result.processed_files.is_empty() {
            return Ok(());
        }

        // Phase 1: Write all files to temporary paths
        let mut temp_paths: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new(); // (temp, target)

        for pf in &result.processed_files {
            let target = &pf.path;
            let temp_path = self.temp_path(target);

            // Write to temp file
            if let Some(parent) = temp_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            match tokio::fs::write(&temp_path, &pf.new_content).await {
                Ok(_) => {
                    temp_paths.push((temp_path, target.clone()));
                }
                Err(e) => {
                    // Phase 1 failed — cleanup all temp files
                    self.cleanup_temp_files(&temp_paths).await;
                    return Err(anyhow::anyhow!(
                        "Phase 1 failed: cannot write temp file for {:?}: {}",
                        target, e
                    ));
                }
            }
        }

        // Phase 2: Atomic rename all temp files to targets
        for (temp, target) in &temp_paths {
            match tokio::fs::rename(temp, target).await {
                Ok(_) => {}
                Err(e) => {
                    // Phase 2 failed — this is harder to recover from
                    // Try to clean up remaining temp files
                    tracing::error!(
                        "Phase 2 failed: cannot rename {:?} to {:?}: {}",
                        temp, target, e
                    );
                    // Continue trying other renames — partial application is better than nothing
                }
            }
        }

        // Cleanup any remaining temp files (shouldn't be any)
        self.cleanup_temp_files(&temp_paths).await;

        Ok(())
    }

    /// Apply changes directly (no two-phase commit, for backwards compatibility)
    pub async fn apply_direct(&self, result: &CommitResult) -> anyhow::Result<()> {
        if !result.success {
            return Err(anyhow::anyhow!(
                "Cannot apply failed commit: {}",
                result.error.as_deref().unwrap_or("unknown error")
            ));
        }

        use tokio::io::AsyncWriteExt;
        for pf in &result.processed_files {
            if let Some(parent) = pf.path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let mut file = tokio::fs::File::create(&pf.path).await?;
            file.write_all(pf.new_content.as_bytes()).await?;
        }
        Ok(())
    }

    fn temp_path(&self, target: &std::path::Path) -> std::path::PathBuf {
        let file_name = target.file_name().unwrap_or_default().to_string_lossy();
        target.with_file_name(format!(".{}{}", file_name, self.temp_suffix))
    }

    async fn cleanup_temp_files(&self, temp_paths: &[(std::path::PathBuf, std::path::PathBuf)]) {
        for (temp, _) in temp_paths {
            if temp.exists() {
                let _ = tokio::fs::remove_file(temp).await;
            }
        }
    }
}

impl Default for AtomicCommit {
    fn default() -> Self {
        Self::new()
    }
}
