use super::{MigrationContext, binary_name};
use anyhow::Result;
use jcode_storage as storage;
use std::path::PathBuf;

/// Get path to builds directory
pub fn builds_dir() -> Result<PathBuf> {
    let base = storage::jcode_dir()?;
    let dir = base.join("builds");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

/// Get path to build manifest
pub fn manifest_path() -> Result<PathBuf> {
    Ok(builds_dir()?.join("manifest.json"))
}

/// Get path to a specific version's binary
pub fn version_binary_path(hash: &str) -> Result<PathBuf> {
    Ok(builds_dir()?
        .join("versions")
        .join(hash)
        .join(binary_name()))
}

/// Get path to stable symlink
pub fn stable_binary_path() -> Result<PathBuf> {
    Ok(builds_dir()?.join("stable").join(binary_name()))
}

/// Get path to current symlink (active local build channel)
pub fn current_binary_path() -> Result<PathBuf> {
    Ok(builds_dir()?.join("current").join(binary_name()))
}

/// Get path to the shared server symlink (approved daemon channel).
pub fn shared_server_binary_path() -> Result<PathBuf> {
    Ok(builds_dir()?.join("shared-server").join(binary_name()))
}

/// Get path to canary binary
pub fn canary_binary_path() -> Result<PathBuf> {
    Ok(builds_dir()?.join("canary").join(binary_name()))
}

/// Get path to migration context file
pub fn migration_context_path(session_id: &str) -> Result<PathBuf> {
    Ok(builds_dir()?
        .join("migrations")
        .join(format!("{}.json", session_id)))
}

/// Get path to stable version file (watched by other sessions)
pub fn stable_version_file() -> Result<PathBuf> {
    Ok(builds_dir()?.join("stable-version"))
}

/// Get path to current version file (active local build marker).
pub fn current_version_file() -> Result<PathBuf> {
    Ok(builds_dir()?.join("current-version"))
}

/// Get path to the shared server version file (approved daemon marker).
pub fn shared_server_version_file() -> Result<PathBuf> {
    Ok(builds_dir()?.join("shared-server-version"))
}

/// Save migration context before switching to canary
pub fn save_migration_context(ctx: &MigrationContext) -> Result<()> {
    let path = migration_context_path(&ctx.session_id)?;
    storage::write_json(&path, ctx)
}

/// Load migration context
pub fn load_migration_context(session_id: &str) -> Result<Option<MigrationContext>> {
    let path = migration_context_path(session_id)?;
    if path.exists() {
        Ok(Some(storage::read_json(&path)?))
    } else {
        Ok(None)
    }
}

/// Clear migration context after successful migration
pub fn clear_migration_context(session_id: &str) -> Result<()> {
    let path = migration_context_path(session_id)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Read the current stable version
pub fn read_stable_version() -> Result<Option<String>> {
    let path = stable_version_file()?;
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let hash = content.trim();
        if hash.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hash.to_string()))
        }
    } else {
        Ok(None)
    }
}

/// Read the current active version.
pub fn read_current_version() -> Result<Option<String>> {
    let path = current_version_file()?;
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let hash = content.trim();
        if hash.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hash.to_string()))
        }
    } else {
        Ok(None)
    }
}

/// Read the current shared-server version.
pub fn read_shared_server_version() -> Result<Option<String>> {
    let path = shared_server_version_file()?;
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let hash = content.trim();
        if hash.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hash.to_string()))
        }
    } else {
        Ok(None)
    }
}

/// Get path to build log file
pub fn build_log_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("build.log"))
}

/// Get path to build progress file (for TUI to watch)
pub fn build_progress_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("build-progress"))
}

/// Write current build progress (for TUI to display)
pub fn write_build_progress(status: &str) -> Result<()> {
    let path = build_progress_path()?;
    std::fs::write(&path, status)?;
    Ok(())
}

/// Read current build progress
pub fn read_build_progress() -> Option<String> {
    build_progress_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Clear build progress
pub fn clear_build_progress() -> Result<()> {
    let path = build_progress_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}
