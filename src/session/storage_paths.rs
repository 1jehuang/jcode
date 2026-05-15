use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

use super::PersistVectorMode;
use crate::storage;

pub(crate) fn session_path_in_dir(base: &std::path::Path, session_id: &str) -> PathBuf {
    base.join("sessions").join(format!("{}.json", session_id))
}

pub(super) fn estimate_json_bytes<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

pub(super) fn file_len_or_zero(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

pub(super) fn persist_vector_mode_label(mode: PersistVectorMode) -> &'static str {
    match mode {
        PersistVectorMode::Clean => "clean",
        PersistVectorMode::Append => "append",
        PersistVectorMode::Full => "full",
    }
}

pub fn session_path(session_id: &str) -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    Ok(session_path_in_dir(&base, session_id))
}

pub(crate) fn session_journal_path_from_snapshot(path: &Path) -> PathBuf {
    let mut name = path
        .file_stem()
        .map(|stem| stem.to_os_string())
        .unwrap_or_default();
    name.push(".journal.jsonl");
    path.with_file_name(name)
}

pub fn session_journal_path(session_id: &str) -> Result<PathBuf> {
    Ok(session_journal_path_from_snapshot(&session_path(
        session_id,
    )?))
}

pub fn session_exists(session_id: &str) -> bool {
    session_path(session_id)
        .map(|path| path.exists())
        .unwrap_or(false)
}

#[derive(Debug, Default)]
pub struct DeletedSessionArtifacts {
    pub removed: Vec<PathBuf>,
    pub missing: Vec<PathBuf>,
}

pub fn delete_session_artifacts(session_id: &str) -> Result<DeletedSessionArtifacts> {
    if session_id.trim().is_empty()
        || session_id
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch == std::path::MAIN_SEPARATOR)
    {
        anyhow::bail!("Refusing to delete invalid session id: {session_id:?}");
    }

    let base = storage::jcode_dir()?;
    let snapshot = session_path_in_dir(&base, session_id);
    let paths = [
        snapshot.clone(),
        session_journal_path_from_snapshot(&snapshot),
        snapshot.with_extension("json.bak"),
        base.join("active_pids").join(session_id),
        base.join("todos").join(format!("{session_id}.json")),
        base.join("side_panel").join(format!("{session_id}.json")),
        base.join(format!("client-input-{session_id}")),
    ];

    let mut result = DeletedSessionArtifacts::default();
    for path in paths {
        if path.exists() {
            std::fs::remove_file(&path)?;
            result.removed.push(path);
        } else {
            result.missing.push(path);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restore_env(previous: Option<std::ffi::OsString>) {
        if let Some(previous) = previous {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn delete_session_artifacts_removes_only_session_files() {
        let _guard = crate::storage::lock_test_env();
        let previous_home = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());

        let base = storage::jcode_dir().unwrap();
        let session_id = "session_delete_test";
        let paths = [
            base.join("sessions").join(format!("{session_id}.json")),
            base.join("sessions")
                .join(format!("{session_id}.journal.jsonl")),
            base.join("sessions").join(format!("{session_id}.json.bak")),
            base.join("active_pids").join(session_id),
            base.join("todos").join(format!("{session_id}.json")),
            base.join("side_panel").join(format!("{session_id}.json")),
            base.join(format!("client-input-{session_id}")),
        ];
        for path in &paths {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        let unrelated = base.join("sessions").join("session_keep.json");
        std::fs::write(&unrelated, "keep").unwrap();

        let deleted = delete_session_artifacts(session_id).unwrap();

        assert_eq!(deleted.removed.len(), paths.len());
        for path in &paths {
            assert!(!path.exists(), "{} should be removed", path.display());
        }
        assert!(unrelated.exists(), "unrelated session must not be removed");

        restore_env(previous_home);
    }

    #[test]
    fn delete_session_artifacts_rejects_path_like_ids() {
        assert!(delete_session_artifacts("../bad").is_err());
        assert!(delete_session_artifacts("bad/name").is_err());
        assert!(delete_session_artifacts("").is_err());
    }
}
