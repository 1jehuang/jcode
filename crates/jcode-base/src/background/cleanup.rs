//! Cleanup of aged background task files.

use anyhow::Result;
use tokio::fs;

use crate::bus::BackgroundTaskStatus;

use super::{BackgroundCleanupResult, BackgroundTaskManager};

impl BackgroundTaskManager {
    /// Clean up old task files (older than specified hours)
    pub async fn cleanup(&self, max_age_hours: u64) -> Result<usize> {
        Ok(self
            .cleanup_filtered(max_age_hours, &std::collections::HashSet::new(), false)
            .await?
            .removed_files)
    }

    /// Clean up old task files across every session.
    ///
    /// Callers serving a single session should prefer `cleanup_filtered_for_session`;
    /// this global sweep is correct for daemon maintenance, not for a tool call.
    pub async fn cleanup_filtered(
        &self,
        max_age_hours: u64,
        status_filter: &std::collections::HashSet<&str>,
        dry_run: bool,
    ) -> Result<BackgroundCleanupResult> {
        self.cleanup_filtered_scoped(max_age_hours, status_filter, dry_run, None)
            .await
    }

    /// Clean up old task files owned by one session.
    ///
    /// Output files whose status cannot be read are left alone. Losing a status
    /// read must not turn into deleting another session's transcript.
    pub async fn cleanup_filtered_for_session(
        &self,
        max_age_hours: u64,
        status_filter: &std::collections::HashSet<&str>,
        dry_run: bool,
        session_id: &str,
    ) -> Result<BackgroundCleanupResult> {
        self.cleanup_filtered_scoped(max_age_hours, status_filter, dry_run, Some(session_id))
            .await
    }

    async fn cleanup_filtered_scoped(
        &self,
        max_age_hours: u64,
        status_filter: &std::collections::HashSet<&str>,
        dry_run: bool,
        session_scope: Option<&str>,
    ) -> Result<BackgroundCleanupResult> {
        let mut result = BackgroundCleanupResult {
            matched_files: 0,
            removed_files: 0,
            skipped_running_files: 0,
        };
        let cutoff =
            std::time::SystemTime::now() - std::time::Duration::from_secs(max_age_hours * 3600);

        if let Ok(mut entries) = fs::read_dir(&self.output_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let Ok(metadata) = fs::metadata(&path).await else {
                    continue;
                };
                let Ok(modified) = metadata.modified() else {
                    continue;
                };
                if modified >= cutoff {
                    continue;
                }

                let mut associated_status = None;
                if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                    associated_status = self.read_status_file(&path).await;
                } else if path.extension().and_then(|ext| ext.to_str()) == Some("output")
                    && let Some(task_id) = path.file_stem().and_then(|stem| stem.to_str())
                {
                    associated_status = self.status(task_id).await;
                }

                if let Some(status) = associated_status.as_ref() {
                    if status.status == BackgroundTaskStatus::Running {
                        result.skipped_running_files += 1;
                        continue;
                    }
                    let status_label = match status.status {
                        BackgroundTaskStatus::Running => "running",
                        BackgroundTaskStatus::Completed => "completed",
                        BackgroundTaskStatus::Superseded => "superseded",
                        BackgroundTaskStatus::Failed => "failed",
                    };
                    if !status_filter.is_empty() && !status_filter.contains(status_label) {
                        continue;
                    }
                } else if !status_filter.is_empty() {
                    continue;
                }

                // Session scoping happens after the status read so a file whose
                // status is missing is never attributed to the wrong session.
                if let Some(scope) = session_scope {
                    let owned = associated_status
                        .as_ref()
                        .is_some_and(|status| status.session_id == scope);
                    if !owned {
                        continue;
                    }
                }

                result.matched_files += 1;
                if !dry_run {
                    let _ = fs::remove_file(&path).await;
                    result.removed_files += 1;
                }
            }
        }

        if dry_run {
            result.removed_files = result.matched_files;
        }

        Ok(result)
    }
}
