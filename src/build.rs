use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
        // Assume structure: repo/target/release/jcode
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

    /// Mark current build as stable
    pub fn promote_to_stable(&mut self, hash: &str) -> Result<()> {
        self.stable = Some(hash.to_string());
        self.canary = None;
        self.canary_session = None;
        self.canary_status = None;
        self.save()
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
    Ok(builds_dir()?.join("versions").join(hash).join("jcode"))
}

/// Get path to stable symlink
pub fn stable_binary_path() -> Result<PathBuf> {
    Ok(builds_dir()?.join("stable").join("jcode"))
}

/// Get path to canary binary
pub fn canary_binary_path() -> Result<PathBuf> {
    Ok(builds_dir()?.join("canary").join("jcode"))
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

/// Install release binary into ~/.local/bin with versioned filename and symlink
pub fn install_local_release(repo_dir: &std::path::Path) -> Result<PathBuf> {
    let source = repo_dir.join("target/release/jcode");
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

    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("HOME not set"))?;
    let dest_dir = home.join(".local").join("bin");
    storage::ensure_dir(&dest_dir)?;

    let versioned = dest_dir.join(format!("jcode-{}", version));

    // Remove existing file first to avoid ETXTBSY
    if versioned.exists() {
        std::fs::remove_file(&versioned)?;
    }
    std::fs::copy(&source, &versioned)?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&versioned)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&versioned, perms)?;
    }

    let link_path = dest_dir.join("jcode");
    let _ = std::fs::remove_file(&link_path);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&versioned, &link_path)?;

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

/// Write the stable version file (triggers watchers in other sessions)
pub fn write_stable_version(hash: &str) -> Result<()> {
    let path = stable_version_file()?;
    storage::ensure_dir(path.parent().unwrap())?;
    std::fs::write(path, hash)?;
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
    let source = repo_dir.join("target/release/jcode");
    if !source.exists() {
        anyhow::bail!("Binary not found at {:?}", source);
    }

    let dest_dir = builds_dir()?.join("versions").join(hash);
    storage::ensure_dir(&dest_dir)?;

    let dest = dest_dir.join("jcode");

    // Remove existing file first to avoid "Text file busy" error (ETXTBSY)
    // when the binary is currently being executed. Unlinking is allowed
    // on running executables, but writing to them is not.
    if dest.exists() {
        std::fs::remove_file(&dest)?;
    }

    std::fs::copy(&source, &dest)?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms)?;
    }

    Ok(dest)
}

/// Update stable symlink to point to a version
pub fn update_stable_symlink(hash: &str) -> Result<()> {
    let stable_dir = builds_dir()?.join("stable");
    storage::ensure_dir(&stable_dir)?;

    let link_path = stable_dir.join("jcode");
    let target = builds_dir()?.join("versions").join(hash).join("jcode");

    // Remove existing symlink/file
    let _ = std::fs::remove_file(&link_path);

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link_path)?;

    // Also write the version file for watchers
    write_stable_version(hash)?;

    Ok(())
}

/// Update canary symlink to point to a version
pub fn update_canary_symlink(hash: &str) -> Result<()> {
    let canary_dir = builds_dir()?.join("canary");
    storage::ensure_dir(&canary_dir)?;

    let link_path = canary_dir.join("jcode");
    let target = builds_dir()?.join("versions").join(hash).join("jcode");

    // Remove existing symlink/file
    let _ = std::fs::remove_file(&link_path);

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link_path)?;

    Ok(())
}

/// Clear canary symlink (called after promotion or when canary is no longer active)
pub fn clear_canary_symlink() -> Result<()> {
    let link_path = builds_dir()?.join("canary").join("jcode");
    let _ = std::fs::remove_file(&link_path);
    Ok(())
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
