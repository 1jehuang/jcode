use super::BackgroundTaskManager;
use anyhow::Result;

impl BackgroundTaskManager {
    pub(super) fn persist_status_file(
        path: &std::path::Path,
        status: &impl serde::Serialize,
    ) -> Result<()> {
        use std::io::Write;

        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Background status path has no parent"))?;
        let mut builder = tempfile::Builder::new();
        builder.prefix(".jcode-status-").suffix(".tmp");
        #[cfg(windows)]
        let mut file = builder.make_in(parent, |path| std::fs::File::create_new(path))?;
        #[cfg(not(windows))]
        let mut file = builder.tempfile_in(parent)?;
        {
            let mut writer = std::io::BufWriter::new(file.as_file_mut());
            serde_json::to_writer_pretty(&mut writer, status)?;
            writer.flush()?;
        }
        // std's Windows rename supports replacing a destination held by readers;
        // tempfile's MoveFileExW-only persist does not. Keep normal file attributes.
        #[cfg(windows)]
        std::fs::rename(file.into_temp_path(), path)?;
        #[cfg(not(windows))]
        file.persist(path)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "status_writer_tests.rs"]
mod tests;
