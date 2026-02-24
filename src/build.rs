use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Get the jcode repository directory
pub fn get_repo_dir() -> Option<PathBuf> {
    // First try: compile-time directory
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = PathBuf::from(manifest_dir);
    if path.join(".git").exists() {
        return Some(path);
    }

    // Fallback: check relative to executable
    if let Ok(exe) = std::env::current_exe() {
        // Assume structure: repo/target/release/<binary> (platform-specific executable name)
        if let Some(repo) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            if repo.join(".git").exists() {
                return Some(repo.to_path_buf());
            }
        }
    }

    None
}

pub fn binary_stem() -> &'static str {
    "jcode"
}

pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "jcode.exe"
    } else {
        binary_stem()
    }
}

pub fn release_binary_path(repo_dir: &std::path::Path) -> PathBuf {
    repo_dir.join("target").join("release").join(binary_name())
}

/// Find the best development binary in the repo.
/// Checks target/release (the default build output).
pub fn find_dev_binary(repo_dir: &std::path::Path) -> Option<PathBuf> {
    let release = release_binary_path(repo_dir);
    if release.exists() {
        Some(release)
    } else {
        None
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .map_err(|_| anyhow::anyhow!("HOME/USERPROFILE not set"))
}

/// Directory for the single launcher path users execute from PATH.
///
/// Defaults to `~/.local/bin`, overridable with `JCODE_INSTALL_DIR`.
pub fn launcher_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("JCODE_INSTALL_DIR") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }

    Ok(home_dir()?.join(".local").join("bin"))
}

/// Path to the launcher binary (`~/.local/bin/jcode` by default).
pub fn launcher_binary_path() -> Result<PathBuf> {
    Ok(launcher_dir()?.join(binary_name()))
}

/// Update launcher path to point at the stable channel binary.
pub fn update_launcher_symlink_to_stable() -> Result<PathBuf> {
    let launcher = launcher_binary_path()?;
    let stable = stable_binary_path()?;

    if let Some(parent) = launcher.parent() {
        storage::ensure_dir(parent)?;
    }

    let temp = launcher
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{}-launcher-{}",
            binary_stem(),
            std::process::id()
        ));

    crate::platform::atomic_symlink_swap(&stable, &launcher, &temp)?;
    Ok(launcher)
}

/// Resolve which client binary should be considered for update checks/reload.
///
/// Order matters and should match `/reload` behavior:
/// - Self-dev sessions prefer canary, then rollback
/// - Then launcher path
/// - Then stable channel path
/// - Finally currently running executable
pub fn client_update_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    if is_selfdev_session {
        if let Ok(canary) = canary_binary_path() {
            if canary.exists() {
                return Some((canary, "canary"));
            }
        }
        if let Ok(rollback) = rollback_binary_path() {
            if rollback.exists() {
                return Some((rollback, "rollback"));
            }
        }
    }

    if let Ok(launcher) = launcher_binary_path() {
        if launcher.exists() {
            return Some((launcher, "launcher"));
        }
    }

    if let Ok(stable) = stable_binary_path() {
        if stable.exists() {
            return Some((stable, "stable"));
        }
    }

    std::env::current_exe().ok().map(|exe| (exe, "current"))
}

/// Check if a directory is the jcode repository
pub fn is_jcode_repo(dir: &std::path::Path) -> bool {
    // Check for Cargo.toml with name = "jcode"
    let cargo_toml = dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return false;
    }

    // Check for .git directory
    if !dir.join(".git").exists() {
        return false;
    }

    // Read Cargo.toml and check package name
    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
        // Simple check - look for 'name = "jcode"' in [package] section
        if content.contains("name = \"jcode\"") {
            return true;
        }
    }

    false
}

/// Status of a canary build being tested
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CanaryStatus {
    /// Build is currently being tested
    #[serde(alias = "Testing")]
    Testing,
    /// Build passed all tests and is ready for promotion
    #[serde(alias = "Passed")]
    Passed,
    /// Build failed testing
    #[serde(alias = "Failed")]
    Failed,
}

/// Information about a specific build version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    /// Git commit hash (short)
    pub hash: String,
    /// Git commit hash (full)
    pub full_hash: String,
    /// Build timestamp
    pub built_at: DateTime<Utc>,
    /// Git commit message (first line)
    pub commit_message: Option<String>,
    /// Whether build is from dirty working tree
    pub dirty: bool,
}

/// Manifest tracking build versions and their status
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildManifest {
    /// Current stable build hash (known good)
    pub stable: Option<String>,
    /// Current canary build hash (being tested)
    pub canary: Option<String>,
    /// Session ID testing the canary build
    pub canary_session: Option<String>,
    /// Status of canary testing
    pub canary_status: Option<CanaryStatus>,
    /// History of recent builds
    #[serde(default)]
    pub history: Vec<BuildInfo>,
    /// Last crash information (if canary crashed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_crash: Option<CrashInfo>,
}

/// Information about a crash during canary testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashInfo {
    /// Build hash that crashed
    pub build_hash: String,
    /// Exit code
    pub exit_code: i32,
    /// Stderr output (truncated)
    pub stderr: String,
    /// Timestamp of crash
    pub crashed_at: DateTime<Utc>,
    /// Git diff that was being tested
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

/// Context saved before migrating to a canary build
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationContext {
    pub session_id: String,
    pub from_version: String,
    pub to_version: String,
    pub change_summary: Option<String>,
    pub diff: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl BuildManifest {
    /// Load manifest from disk
    pub fn load() -> Result<Self> {
        let path = manifest_path()?;
        if path.exists() {
            storage::read_json(&path)
        } else {
            Ok(Self::default())
        }
    }

    /// Save manifest to disk
    pub fn save(&self) -> Result<()> {
        let path = manifest_path()?;
        storage::write_json(&path, self)
    }

    /// Check if we should use stable or canary for a given session
    pub fn binary_for_session(&self, session_id: &str) -> BinaryChoice {
        // If this session is the canary tester, use canary
        if let Some(ref canary_session) = self.canary_session {
            if canary_session == session_id {
                if let Some(ref canary) = self.canary {
                    return BinaryChoice::Canary(canary.clone());
                }
            }
        }
        // Otherwise use stable
        if let Some(ref stable) = self.stable {
            BinaryChoice::Stable(stable.clone())
        } else {
            BinaryChoice::Current
        }
    }

    /// Start canary testing for a session
    pub fn start_canary(&mut self, hash: &str, session_id: &str) -> Result<()> {
        self.canary = Some(hash.to_string());
        self.canary_session = Some(session_id.to_string());
        self.canary_status = Some(CanaryStatus::Testing);
        self.save()
    }

    /// Mark canary as passed
    pub fn mark_canary_passed(&mut self) -> Result<()> {
        self.canary_status = Some(CanaryStatus::Passed);
        self.save()
    }

    /// Mark canary as failed
    pub fn mark_canary_failed(&mut self) -> Result<()> {
        self.canary_status = Some(CanaryStatus::Failed);
        self.save()
    }

    /// Record a crash
    pub fn record_crash(
        &mut self,
        hash: &str,
        exit_code: i32,
        stderr: &str,
        diff: Option<String>,
    ) -> Result<()> {
        self.last_crash = Some(CrashInfo {
            build_hash: hash.to_string(),
            exit_code,
            stderr: stderr.chars().take(4096).collect(), // Truncate
            crashed_at: Utc::now(),
            diff,
        });
        self.canary_status = Some(CanaryStatus::Failed);
        self.save()
    }

    /// Clear crash info after it's been handled
    pub fn clear_crash(&mut self) -> Result<()> {
        self.last_crash = None;
        self.save()
    }

    /// Add build to history
    pub fn add_to_history(&mut self, info: BuildInfo) -> Result<()> {
        // Keep last 20 builds
        self.history.insert(0, info);
        self.history.truncate(20);
        self.save()
    }
}

/// Which binary to use
#[derive(Debug, Clone)]
pub enum BinaryChoice {
    /// Use the stable version
    Stable(String),
    /// Use the canary version (for testing)
    Canary(String),
    /// Use current running binary (no versioned builds yet)
    Current,
}

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

/// Get the current git hash
pub fn current_git_hash(repo_dir: &std::path::Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_dir)
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to get git hash");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the full git hash
pub fn current_git_hash_full(repo_dir: &std::path::Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_dir)
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to get git hash");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the git diff for uncommitted changes
pub fn current_git_diff(repo_dir: &std::path::Path) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(repo_dir)
        .output()?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Check if working tree is dirty
pub fn is_working_tree_dirty(repo_dir: &std::path::Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_dir)
        .output()?;

    Ok(!output.stdout.is_empty())
}

/// Get commit message for a hash
pub fn get_commit_message(repo_dir: &std::path::Path, hash: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%s", hash])
        .current_dir(repo_dir)
        .output()?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Build info for current state
pub fn current_build_info(repo_dir: &std::path::Path) -> Result<BuildInfo> {
    let hash = current_git_hash(repo_dir)?;
    let full_hash = current_git_hash_full(repo_dir)?;
    let dirty = is_working_tree_dirty(repo_dir)?;
    let commit_message = get_commit_message(repo_dir, &hash).ok();

    Ok(BuildInfo {
        hash,
        full_hash,
        built_at: Utc::now(),
        commit_message,
        dirty,
    })
}

/// Install a binary at a specific immutable version path.
pub fn install_binary_at_version(source: &std::path::Path, version: &str) -> Result<PathBuf> {
    if !source.exists() {
        anyhow::bail!("Binary not found at {:?}", source);
    }

    let dest_dir = builds_dir()?.join("versions").join(version);
    storage::ensure_dir(&dest_dir)?;

    let dest = dest_dir.join(binary_name());

    // Remove existing file first to avoid ETXTBSY when replacing a running binary.
    if dest.exists() {
        std::fs::remove_file(&dest)?;
    }

    std::fs::copy(source, &dest)?;
    crate::platform::set_permissions_executable(&dest)?;

    Ok(dest)
}

fn update_channel_symlink(channel: &str, version: &str) -> Result<PathBuf> {
    let channel_dir = builds_dir()?.join(channel);
    storage::ensure_dir(&channel_dir)?;

    let link_path = channel_dir.join(binary_name());
    let target = version_binary_path(version)?;
    if !target.exists() {
        anyhow::bail!("Version binary not found at {:?}", target);
    }

    let temp = channel_dir.join(format!(
        ".{}-{}-{}",
        binary_stem(),
        channel,
        std::process::id()
    ));
    crate::platform::atomic_symlink_swap(&target, &link_path, &temp)?;

    Ok(link_path)
}

/// Update stable symlink to point to a version and publish stable-version marker.
pub fn update_stable_symlink(version: &str) -> Result<PathBuf> {
    let stable_link = update_channel_symlink("stable", version)?;
    std::fs::write(stable_version_file()?, version)?;
    Ok(stable_link)
}

/// Install release binary into immutable versions and point stable+launcher to it.
pub fn install_local_release(repo_dir: &std::path::Path) -> Result<PathBuf> {
    let source = release_binary_path(repo_dir);
    if !source.exists() {
        anyhow::bail!("Binary not found at {:?}", source);
    }

    let hash = current_git_hash(repo_dir)?;
    let dirty = is_working_tree_dirty(repo_dir)?;
    let version = if dirty {
        format!("{}-dirty", hash)
    } else {
        hash
    };

    let versioned = install_binary_at_version(&source, &version)?;
    update_stable_symlink(&version)?;
    update_launcher_symlink_to_stable()?;

    Ok(versioned)
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

/// Copy binary to versioned location
pub fn install_version(repo_dir: &std::path::Path, hash: &str) -> Result<PathBuf> {
    let source = release_binary_path(repo_dir);
    install_binary_at_version(&source, hash)
}

/// Update canary symlink to point to a version
pub fn update_canary_symlink(hash: &str) -> Result<()> {
    let _ = update_channel_symlink("canary", hash)?;
    Ok(())
}

/// Update rollback symlink (safety net for self-dev, separate from stable/release)
pub fn update_rollback_symlink(hash: &str) -> Result<()> {
    let _ = update_channel_symlink("rollback", hash)?;
    Ok(())
}

/// Get path to rollback binary
pub fn rollback_binary_path() -> Result<PathBuf> {
    Ok(builds_dir()?.join("rollback").join(binary_name()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_manifest_default() {
        let manifest = BuildManifest::default();
        assert!(manifest.stable.is_none());
        assert!(manifest.canary.is_none());
        assert!(manifest.history.is_empty());
    }

    #[test]
    fn test_binary_choice_for_canary_session() {
        let mut manifest = BuildManifest::default();
        manifest.canary = Some("abc123".to_string());
        manifest.canary_session = Some("session_test".to_string());

        // Canary session should get canary binary
        match manifest.binary_for_session("session_test") {
            BinaryChoice::Canary(hash) => assert_eq!(hash, "abc123"),
            _ => panic!("Expected canary binary"),
        }

        // Other sessions should get stable (or current if no stable)
        match manifest.binary_for_session("other_session") {
            BinaryChoice::Current => {}
            _ => panic!("Expected current binary"),
        }
    }

    #[test]
    fn test_canary_status_serialization() {
        assert_eq!(
            serde_json::to_string(&CanaryStatus::Testing).unwrap(),
            "\"testing\""
        );
        assert_eq!(
            serde_json::to_string(&CanaryStatus::Passed).unwrap(),
            "\"passed\""
        );
    }
}
