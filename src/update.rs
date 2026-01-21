//! Auto-update functionality for jcode
//!
//! This module handles checking for updates from GitHub Releases and
//! automatically downloading/installing new versions for end users.
//!
//! Key design decisions:
//! - Only binaries built by CI (is_release_build = true) get auto-updated
//! - Local builds (cargo build) never auto-update
//! - Uses git commit hash to determine if update is needed
//! - Atomic install via temp file + rename
//! - Crash loop detection with automatic rollback

use crate::storage;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// GitHub repository for jcode
const GITHUB_REPO: &str = "1jehuang/jcode";

/// How often to check for updates (24 hours)
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Timeout for update check HTTP requests
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout for download HTTP requests
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Build information embedded at compile time
#[derive(Debug, Clone)]
pub struct BuildInfo {
    /// Git commit hash (short)
    pub git_hash: &'static str,
    /// Whether this is a release build (built by CI)
    pub is_release_build: bool,
    /// Full version string
    pub version: &'static str,
}

impl BuildInfo {
    /// Get the current build info from compile-time environment variables
    pub fn current() -> Self {
        Self {
            git_hash: env!("JCODE_GIT_HASH"),
            is_release_build: option_env!("JCODE_RELEASE_BUILD").is_some(),
            version: env!("JCODE_VERSION"),
        }
    }
}

/// Information about a GitHub release
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub html_url: String,
    pub published_at: String,
    pub assets: Vec<GitHubAsset>,
    #[serde(default)]
    pub target_commitish: String,
}

/// A release asset (downloadable file)
#[derive(Debug, Clone, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub content_type: String,
}

/// Metadata stored alongside downloaded releases
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMetadata {
    /// When we last checked for updates
    pub last_check: SystemTime,
    /// Git hash of the currently installed release (if from auto-update)
    pub installed_hash: Option<String>,
    /// Version string of currently installed release
    pub installed_version: Option<String>,
    /// Whether we're in a crash loop recovery state
    pub crash_recovery: bool,
    /// Hash of the previous working version (for rollback)
    pub previous_hash: Option<String>,
}

impl Default for UpdateMetadata {
    fn default() -> Self {
        Self {
            last_check: SystemTime::UNIX_EPOCH,
            installed_hash: None,
            installed_version: None,
            crash_recovery: false,
            previous_hash: None,
        }
    }
}

impl UpdateMetadata {
    /// Load metadata from disk
    pub fn load() -> Result<Self> {
        let path = metadata_path()?;
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    /// Save metadata to disk
    pub fn save(&self) -> Result<()> {
        let path = metadata_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// Check if enough time has passed since last update check
    pub fn should_check(&self) -> bool {
        match self.last_check.elapsed() {
            Ok(elapsed) => elapsed > UPDATE_CHECK_INTERVAL,
            Err(_) => true, // Clock went backwards, check anyway
        }
    }
}

/// Path to update metadata file
fn metadata_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("update_metadata.json"))
}

/// Path to crash marker file
fn crash_marker_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("update_crash_marker"))
}

/// Path to update lock file
fn update_lock_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("update.lock"))
}

/// Check if we should auto-update
pub fn should_auto_update() -> bool {
    // 1. Check environment variable override
    if std::env::var("JCODE_NO_AUTO_UPDATE").is_ok() {
        return false;
    }

    // 2. Only release builds get auto-updated
    let build_info = BuildInfo::current();
    if !build_info.is_release_build {
        return false;
    }

    // 3. Check if binary is inside a git repo (developer running from checkout)
    if let Ok(exe) = std::env::current_exe() {
        if is_inside_git_repo(&exe) {
            return false;
        }
    }

    true
}

/// Check if a path is inside a git repository
fn is_inside_git_repo(path: &std::path::Path) -> bool {
    // Start with the path itself if it's a directory, otherwise start with parent
    let mut dir = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };

    while let Some(d) = dir {
        if d.join(".git").exists() {
            return true;
        }
        dir = d.parent();
    }
    false
}

/// Get the appropriate asset name for the current platform
fn get_asset_name() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "jcode-linux-x86_64"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "jcode-linux-aarch64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "jcode-macos-x86_64"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "jcode-macos-aarch64"
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )))]
    {
        "jcode-unknown"
    }
}

/// Fetch the latest release from GitHub
pub async fn fetch_latest_release() -> Result<GitHubRelease> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent("jcode-updater")
        .build()?;

    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to fetch release info")?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("No releases found");
    }

    if !response.status().is_success() {
        anyhow::bail!("GitHub API error: {}", response.status());
    }

    let release: GitHubRelease = response
        .json()
        .await
        .context("Failed to parse release info")?;

    Ok(release)
}

/// Check if an update is available
pub async fn check_for_update() -> Result<Option<GitHubRelease>> {
    let build_info = BuildInfo::current();
    let release = fetch_latest_release().await?;

    // Extract git hash from release tag (format: v0.1.0-<hash> or just use target_commitish)
    let release_hash = extract_hash_from_release(&release);

    // If we're on the same commit, no update needed
    if release_hash
        .as_ref()
        .map(|h| h == build_info.git_hash)
        .unwrap_or(false)
    {
        return Ok(None);
    }

    // Check if the asset for our platform exists
    let asset_name = get_asset_name();
    let has_asset = release.assets.iter().any(|a| a.name == asset_name);

    if !has_asset {
        // No binary for our platform
        return Ok(None);
    }

    Ok(Some(release))
}

/// Extract git hash from release info
fn extract_hash_from_release(release: &GitHubRelease) -> Option<String> {
    // First try: tag name might contain hash (e.g., v0.1.0-abc1234)
    if let Some(hash) = release.tag_name.split('-').last() {
        if hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hash.to_string());
        }
    }

    // Second try: target_commitish (branch or commit)
    if release.target_commitish.len() >= 7
        && release
            .target_commitish
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    {
        return Some(release.target_commitish[..7].to_string());
    }

    None
}

/// Download and install a release
pub async fn download_and_install(release: &GitHubRelease) -> Result<PathBuf> {
    let asset_name = get_asset_name();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| anyhow::anyhow!("No asset found for platform: {}", asset_name))?;

    // Create temp file for download
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("jcode-update-{}", std::process::id()));

    // Download the binary
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent("jcode-updater")
        .build()?;

    let response = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .context("Failed to download update")?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed: {}", response.status());
    }

    let bytes = response
        .bytes()
        .await
        .context("Failed to read download")?;

    // Write to temp file
    fs::write(&temp_path, &bytes).context("Failed to write temp file")?;

    // Make executable
    let mut perms = fs::metadata(&temp_path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&temp_path, perms)?;

    // Determine install location
    let home = std::env::var("HOME").context("HOME not set")?;
    let install_dir = PathBuf::from(&home).join(".local").join("bin");
    fs::create_dir_all(&install_dir)?;

    // Extract version for filename
    let version = release.tag_name.trim_start_matches('v');
    let versioned_path = install_dir.join(format!("jcode-{}", version));

    // Atomic move
    fs::rename(&temp_path, &versioned_path).or_else(|_| {
        // rename() doesn't work across filesystems, fall back to copy+delete
        fs::copy(&temp_path, &versioned_path)?;
        fs::remove_file(&temp_path)?;
        Ok::<_, std::io::Error>(())
    })?;

    // Update symlink atomically
    let symlink_path = install_dir.join("jcode");
    let temp_symlink = install_dir.join(format!(".jcode-symlink-{}", std::process::id()));

    // Create new symlink at temp location
    #[cfg(unix)]
    std::os::unix::fs::symlink(&versioned_path, &temp_symlink)?;

    // Atomic rename of symlink
    fs::rename(&temp_symlink, &symlink_path)?;

    // Update metadata
    let mut metadata = UpdateMetadata::load().unwrap_or_default();
    metadata.previous_hash = metadata.installed_hash.take();
    metadata.installed_hash = extract_hash_from_release(release);
    metadata.installed_version = Some(release.tag_name.clone());
    metadata.last_check = SystemTime::now();
    metadata.save()?;

    Ok(versioned_path)
}

/// Mark that we're starting up (for crash detection)
pub fn mark_startup() -> Result<()> {
    let marker = crash_marker_path()?;
    fs::write(&marker, BuildInfo::current().git_hash)?;
    Ok(())
}

/// Mark successful startup (clear crash marker)
pub fn mark_startup_success() -> Result<()> {
    let marker = crash_marker_path()?;
    if marker.exists() {
        fs::remove_file(&marker)?;
    }
    Ok(())
}

/// Check if we crashed on startup with the current version
pub fn check_crash_loop() -> Result<bool> {
    let marker = crash_marker_path()?;
    if !marker.exists() {
        return Ok(false);
    }

    let marker_hash = fs::read_to_string(&marker)?;
    let current_hash = BuildInfo::current().git_hash;

    Ok(marker_hash.trim() == current_hash)
}

/// Rollback to previous version
pub fn rollback() -> Result<Option<PathBuf>> {
    let metadata = UpdateMetadata::load()?;

    if let Some(previous_hash) = &metadata.previous_hash {
        let home = std::env::var("HOME").context("HOME not set")?;
        let install_dir = PathBuf::from(&home).join(".local").join("bin");

        // Find the previous version binary
        for entry in fs::read_dir(&install_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("jcode-") && name_str.contains(previous_hash) {
                let previous_path = entry.path();

                // Update symlink to point to previous version
                let symlink_path = install_dir.join("jcode");
                let temp_symlink =
                    install_dir.join(format!(".jcode-symlink-{}", std::process::id()));

                #[cfg(unix)]
                std::os::unix::fs::symlink(&previous_path, &temp_symlink)?;

                fs::rename(&temp_symlink, &symlink_path)?;

                // Clear crash marker
                let marker = crash_marker_path()?;
                if marker.exists() {
                    fs::remove_file(&marker)?;
                }

                // Update metadata
                let mut metadata = UpdateMetadata::load().unwrap_or_default();
                metadata.crash_recovery = true;
                metadata.save()?;

                return Ok(Some(previous_path));
            }
        }
    }

    Ok(None)
}

/// Result of update check
pub enum UpdateCheckResult {
    /// No update available or not eligible
    NoUpdate,
    /// Update available but user should be notified only
    UpdateAvailable(GitHubRelease),
    /// Update was automatically installed
    UpdateInstalled(PathBuf),
    /// Crashed on startup, rolled back
    RolledBack(PathBuf),
    /// Error during update check
    Error(String),
}

/// Perform the full update check flow
pub async fn check_and_update(auto_install: bool) -> UpdateCheckResult {
    // Check for crash loop first
    if let Ok(true) = check_crash_loop() {
        eprintln!("Detected crash loop, attempting rollback...");
        match rollback() {
            Ok(Some(path)) => {
                eprintln!("Rolled back to: {}", path.display());
                return UpdateCheckResult::RolledBack(path);
            }
            Ok(None) => {
                eprintln!("No previous version to rollback to");
            }
            Err(e) => {
                eprintln!("Rollback failed: {}", e);
            }
        }
    }

    // Check if we should auto-update at all
    if !should_auto_update() {
        return UpdateCheckResult::NoUpdate;
    }

    // Check if enough time has passed since last check
    let metadata = UpdateMetadata::load().unwrap_or_default();
    if !metadata.should_check() {
        return UpdateCheckResult::NoUpdate;
    }

    // Mark startup for crash detection
    if let Err(e) = mark_startup() {
        eprintln!("Warning: Failed to mark startup: {}", e);
    }

    // Check for update
    match check_for_update().await {
        Ok(Some(release)) => {
            if auto_install {
                match download_and_install(&release).await {
                    Ok(path) => UpdateCheckResult::UpdateInstalled(path),
                    Err(e) => UpdateCheckResult::Error(format!("Failed to install update: {}", e)),
                }
            } else {
                // Just notify, update metadata
                let mut metadata = UpdateMetadata::load().unwrap_or_default();
                metadata.last_check = SystemTime::now();
                let _ = metadata.save();
                UpdateCheckResult::UpdateAvailable(release)
            }
        }
        Ok(None) => {
            // No update, but update last check time
            let mut metadata = UpdateMetadata::load().unwrap_or_default();
            metadata.last_check = SystemTime::now();
            let _ = metadata.save();
            UpdateCheckResult::NoUpdate
        }
        Err(e) => UpdateCheckResult::Error(format!("Failed to check for updates: {}", e)),
    }
}

/// Force an update check and install
pub async fn force_update() -> Result<PathBuf> {
    let release = fetch_latest_release().await?;
    download_and_install(&release).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_info() {
        let info = BuildInfo::current();
        assert!(!info.git_hash.is_empty());
        assert!(!info.version.is_empty());
    }

    #[test]
    fn test_asset_name() {
        let name = get_asset_name();
        assert!(name.starts_with("jcode-"));
    }

    #[test]
    fn test_is_inside_git_repo() {
        // Root should not be in a git repo
        assert!(!is_inside_git_repo(std::path::Path::new("/")));

        // Test with the source file path (should be in repo)
        let source_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(is_inside_git_repo(source_file));
    }
}
