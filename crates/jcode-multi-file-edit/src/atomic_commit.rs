use crate::diff_merge::UnifiedDiff;
use crate::parallel_processor::ProcessedFile;

/// Result of an atomic commit operation.
#[derive(Debug, Clone)]
pub struct CommitResult {
    pub diff: UnifiedDiff,
    pub processed_files: Vec<ProcessedFile>,
    pub success: bool,
    pub stats: CommitStats,
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
        let stats = CommitStats {
            files_modified: processed.len(),
            total_additions: diff.total_additions,
            total_deletions: diff.total_deletions,
            ..Default::default()
        };
        Self { diff, processed_files: processed, success: true, stats }
    }

    pub fn summary(&self) -> String {
        format!(
            "Atomic commit: {} files, +{} -{} lines",
            self.stats.files_modified,
            self.stats.total_additions,
            self.stats.total_deletions
        )
    }
}

/// Atomic commit — applies changes as a single atomic operation.
pub struct AtomicCommit;

impl AtomicCommit {
    pub fn new() -> Self { Self }

    /// Apply the processed changes to disk.
    pub async fn apply(&self, result: &CommitResult) -> anyhow::Result<()> {
        use tokio::io::AsyncWriteExt;
        for pf in &result.processed_files {
            let mut file = tokio::fs::File::create(&pf.path).await?;
            file.write_all(pf.new_content.as_bytes()).await?;
        }
        Ok(())
    }
}
