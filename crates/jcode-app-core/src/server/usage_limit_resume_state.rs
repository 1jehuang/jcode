//! Durable provider waits. Dropping an in-memory task must not consume a record.
use super::ResumeKind;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static IO: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeState {
    pub session_id: String,
    pub generation: String,
    pub resume_at: chrono::DateTime<chrono::Utc>,
    pub quota_resumes_used: u32,
    pub overload_resumes_used: u32,
    pub kind: ResumeKind,
    pub system_reminder: Option<String>,
    #[serde(default)]
    pub in_flight: bool,
    /// Committed before unlinking. A failed unlink must never resurrect work.
    #[serde(default)]
    pub terminal: bool,
}

fn path(session_id: &str) -> Result<PathBuf> {
    // Hex encoding is injective, unlike replacing punctuation with underscores.
    let name: String = session_id
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(crate::storage::jcode_dir()?
        .join("provider-waits")
        .join(format!("{name}.json")))
}

fn read_at(path: &Path) -> Result<Option<ResumeState>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes).context("Invalid provider-wait record")?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_at(path: &Path, state: &ResumeState) -> Result<()> {
    let parent = path.parent().context("Provider-wait path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(state)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        sync_parent(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn sync_parent(parent: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

pub fn load(session_id: &str) -> Result<Option<ResumeState>> {
    let _io = IO.lock().unwrap_or_else(|p| p.into_inner());
    let state = read_pending_at(&path(session_id)?)?;
    anyhow::ensure!(
        state.as_ref().is_none_or(|s| s.session_id == session_id),
        "Provider-wait session mismatch"
    );
    Ok(state)
}

fn read_pending_at(path: &Path) -> Result<Option<ResumeState>> {
    Ok(read_at(path)?.filter(|state| !state.terminal))
}

/// `expected` makes updates conditional; a cancellation or newer turn wins.
pub fn save(state: &ResumeState, expected: Option<&str>) -> Result<bool> {
    let _io = IO.lock().unwrap_or_else(|p| p.into_inner());
    let path = path(&state.session_id)?;
    if let Some(expected) = expected
        && !read_at(&path)?.is_some_and(|s| !s.terminal && s.generation == expected)
    {
        return Ok(false);
    }
    write_at(&path, state)?;
    Ok(true)
}

pub fn clear(session_id: &str, expected: Option<&str>) -> Result<bool> {
    let _io = IO.lock().unwrap_or_else(|p| p.into_inner());
    clear_at(&path(session_id)?, expected)
}

fn clear_at(path: &Path, expected: Option<&str>) -> Result<bool> {
    clear_at_with_remover(path, expected, |path| std::fs::remove_file(path))
}

fn clear_at_with_remover(
    path: &Path,
    expected: Option<&str>,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<bool> {
    let Some(mut state) = read_at(path)? else {
        return Ok(false);
    };
    if expected.is_some_and(|generation| state.generation != generation) {
        return Ok(false);
    }
    // Persist terminal intent first. Only this commit is required for success:
    // unlink is housekeeping and cannot turn terminal work back into a wait.
    state.terminal = true;
    write_at(path, &state)?;
    if let Err(error) = remove(path) {
        crate::logging::warn(&format!(
            "Could not remove terminal provider-wait record: {error}"
        ));
        return Ok(true);
    }
    if let Err(error) = sync_parent(path.parent().context("Provider-wait path has no parent")?) {
        crate::logging::warn(&format!(
            "Could not sync terminal provider-wait removal: {error}"
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(generation: &str) -> ResumeState {
        ResumeState {
            session_id: "worker".into(),
            generation: generation.into(),
            resume_at: chrono::Utc::now() + chrono::Duration::hours(2),
            quota_resumes_used: 2,
            overload_resumes_used: 1,
            kind: ResumeKind::UsageLimit,
            system_reminder: Some("continue original turn".into()),
            in_flight: false,
            terminal: false,
        }
    }

    #[test]
    fn durable_wait_roundtrip_preserves_deadline_budget_and_reminder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wait.json");
        let original = state("first");
        write_at(&path, &original).unwrap();
        let restored = read_at(&path).unwrap().unwrap();
        assert_eq!(restored.resume_at, original.resume_at);
        assert_eq!(restored.quota_resumes_used, 2);
        assert_eq!(restored.overload_resumes_used, 1);
        assert_eq!(restored.system_reminder, original.system_reminder);
    }

    #[test]
    fn stale_generation_cannot_clear_successor_and_explicit_cancel_clears_disk_only_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wait.json");
        write_at(&path, &state("successor")).unwrap();
        assert!(!clear_at(&path, Some("old")).unwrap());
        assert!(read_at(&path).unwrap().is_some());
        assert!(clear_at(&path, None).unwrap());
        assert!(read_at(&path).unwrap().is_none());
    }

    #[test]
    fn malformed_record_and_failed_write_are_errors_not_missing_waits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wait.json");
        std::fs::write(&path, b"not valid json").unwrap();
        assert!(read_at(&path).is_err());
        assert!(clear_at(&path, Some("first")).is_err());
        // A regular file cannot be the parent directory of a wait record.
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"block").unwrap();
        assert!(write_at(&blocker.join("wait.json"), &state("first")).is_err());
    }

    #[test]
    fn unlink_failure_leaves_terminal_tombstone_not_resumable_wait() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wait.json");
        write_at(&path, &state("first")).unwrap();
        assert!(
            clear_at_with_remover(&path, Some("first"), |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected unlink failure",
                ))
            })
            .unwrap()
        );
        let restored = read_at(&path).unwrap().unwrap();
        assert!(restored.terminal, "completion must commit before unlink");
        assert!(
            read_pending_at(&path).unwrap().is_none(),
            "restart loader must not return a completed turn"
        );
    }
}
