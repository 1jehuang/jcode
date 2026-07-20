use super::{MigrationContext, binary_name};
use anyhow::Result;
use jcode_storage as storage;
use std::path::PathBuf;

/// Get path to builds directory
pub fn builds_dir() -> Result<PathBuf> {
    let dir = resolve_builds_dir(
        std::env::var_os("JCODE_HOME").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        storage::jcode_dir()?,
        cfg!(windows),
    );
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

fn resolve_builds_dir(
    jcode_home: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    default_jcode_dir: PathBuf,
    is_windows: bool,
) -> PathBuf {
    if let Some(jcode_home) = jcode_home {
        return jcode_home.join("builds");
    }

    if is_windows && let Some(local_app_data) = local_app_data {
        // Keep runtime channel discovery aligned with scripts/install.ps1 and
        // the supported Windows layout under %LOCALAPPDATA%\jcode\builds.
        // Durable user state and logs still live under ~/.jcode.
        return local_app_data.join("jcode").join("builds");
    }

    default_jcode_dir.join("builds")
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

#[cfg(test)]
mod tests {
    use super::resolve_builds_dir;
    use std::path::PathBuf;

    #[test]
    fn windows_builds_use_local_app_data() {
        let resolved = resolve_builds_dir(
            None,
            Some(PathBuf::from("/local-app-data")),
            PathBuf::from("/home/test/.jcode"),
            true,
        );

        assert_eq!(resolved, PathBuf::from("/local-app-data/jcode/builds"));
    }

    #[test]
    fn jcode_home_override_wins_on_windows() {
        let resolved = resolve_builds_dir(
            Some(PathBuf::from("/isolated-jcode")),
            Some(PathBuf::from("/local-app-data")),
            PathBuf::from("/home/test/.jcode"),
            true,
        );

        assert_eq!(resolved, PathBuf::from("/isolated-jcode/builds"));
    }

    #[test]
    fn non_windows_builds_stay_under_jcode_home() {
        let resolved = resolve_builds_dir(
            None,
            Some(PathBuf::from("/ignored/local-app-data")),
            PathBuf::from("/home/test/.jcode"),
            false,
        );

        assert_eq!(resolved, PathBuf::from("/home/test/.jcode/builds"));
    }
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
    invalidate_build_progress_cache();
    Ok(())
}

/// Process-local cache for `read_build_progress`. Stores the last-read value
/// alongside the time it was read so per-frame TUI calls can be served without
/// a disk hit.
static BUILD_PROGRESS_CACHE: std::sync::Mutex<Option<(std::time::Instant, Option<String>)>> =
    std::sync::Mutex::new(None);

const BUILD_PROGRESS_TTL: std::time::Duration = std::time::Duration::from_millis(100);

fn invalidate_build_progress_cache() {
    if let Ok(mut guard) = BUILD_PROGRESS_CACHE.lock() {
        *guard = None;
    }
}

/// Read current build progress.
///
/// The TUI calls this from its per-frame redraw scheduler (several times per
/// frame, across every connected client), so a naive implementation performs a
/// synchronous disk read on every render tick even when no build is running.
/// Build progress is a purely cosmetic status string, so we cache the result
/// for a short window. The cache is invalidated immediately on
/// `write_build_progress`/`clear_build_progress` so progress still updates
/// promptly when a build is driven from the same process; cross-process updates
/// become visible within the TTL.
pub fn read_build_progress() -> Option<String> {
    if let Ok(guard) = BUILD_PROGRESS_CACHE.lock()
        && let Some((at, ref value)) = *guard
        && at.elapsed() < BUILD_PROGRESS_TTL
    {
        return value.clone();
    }

    let value = read_build_progress_uncached();

    if let Ok(mut guard) = BUILD_PROGRESS_CACHE.lock() {
        *guard = Some((std::time::Instant::now(), value.clone()));
    }

    value
}

fn read_build_progress_uncached() -> Option<String> {
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
    invalidate_build_progress_cache();
    Ok(())
}
