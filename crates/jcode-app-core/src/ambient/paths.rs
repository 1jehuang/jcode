use anyhow::Result;
use std::path::PathBuf;

use crate::storage;

// ---------------------------------------------------------------------------
// Storage paths
// ---------------------------------------------------------------------------

pub(super) fn ambient_dir() -> Result<PathBuf> {
    let dir = storage::jcode_dir()?.join("ambient");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

pub(super) fn state_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("state.json"))
}

pub(super) fn queue_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("queue.json"))
}

pub(super) fn lock_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("ambient.lock"))
}

pub(super) fn transcripts_dir() -> Result<PathBuf> {
    let dir = ambient_dir()?.join("transcripts");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}
