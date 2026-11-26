use super::*;

impl BackgroundTaskManager {
    /// Clean up old task files (older than specified hours).
    pub async fn cleanup(&self, max_age_hours: u64) -> Result<usize> {
        Ok(self
            .cleanup_filtered(max_age_hours, &std::collections::HashSet::new(), false)
            .await?
            .removed_files)
    }

    /// Clean up old task files, skipping running tasks and optionally filtering by status.
    pub async fn cleanup_filtered(
        &self,
        max_age_hours: u64,
        status_filter: &std::collections::HashSet<&str>,
        dry_run: bool,
    ) -> Result<BackgroundCleanupResult> {
        let mut result = BackgroundCleanupResult {
            matched_files: 0,
            removed_files: 0,
            skipped_running_files: 0,
        };
        let now = std::time::SystemTime::now();
        let cutoff = now - std::time::Duration::from_secs(max_age_hours * 3600);
        // shortcut: writer temps may be active for one hour; use leases if writes can exceed it.
        let writer_cutoff = now - std::time::Duration::from_secs(3600);

        if let Ok(mut entries) = fs::read_dir(&self.output_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let Ok(metadata) = fs::metadata(&path).await else {
                    continue;
                };
                let Ok(modified) = metadata.modified() else {
                    continue;
                };
                let writer_temp =
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with(".jcode-status-") && name.ends_with(".tmp")
                        });
                if modified >= cutoff || writer_temp && modified >= writer_cutoff {
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

                result.matched_files += 1;
                if !dry_run {
                    fs::remove_file(&path).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cleanup_reports_removal_failure_instead_of_counting_it() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let manager = BackgroundTaskManager::with_output_dir(dir.path().to_path_buf());
        let unremovable = dir.path().join("nested.output");
        std::fs::create_dir(&unremovable)?;
        assert!(manager.cleanup(0).await.is_err());
        assert!(unremovable.is_dir());
        Ok(())
    }

    #[test]
    fn cleanup_keeps_a_writer_created_replacement_while_serializing() -> Result<()> {
        struct CleanupDuringWrite<'a> {
            manager: &'a BackgroundTaskManager,
            runtime: &'a tokio::runtime::Runtime,
        }
        impl serde::Serialize for CleanupDuringWrite<'_> {
            fn serialize<S: serde::Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                // The stale orphan and the real writer's temporary file both exist now.
                assert_eq!(
                    std::fs::read_dir(&self.manager.output_dir).unwrap().count(),
                    2
                );
                assert_eq!(self.runtime.block_on(self.manager.cleanup(0)).unwrap(), 1);
                serde::Serialize::serialize("replacement status", serializer)
            }
        }

        let dir = tempfile::tempdir()?;
        let manager = BackgroundTaskManager::with_output_dir(dir.path().to_path_buf());
        let stale = dir.path().join(".jcode-status-abandoned.tmp");
        std::fs::write(&stale, "incomplete")?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&stale)?
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3601))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let path = manager.status_path_for("cleanup-during-write");
        BackgroundTaskManager::persist_status_file(
            &path,
            &CleanupDuringWrite {
                manager: &manager,
                runtime: &runtime,
            },
        )?;
        assert_eq!(std::fs::read_to_string(path)?, "\"replacement status\"");
        assert!(!stale.exists());
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
        Ok(())
    }
}
