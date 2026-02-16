use crate::storage;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const GITHUB_REPO: &str = "1jehuang/jcode";
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

pub fn is_release_build() -> bool {
    option_env!("JCODE_RELEASE_BUILD").is_some()
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub name: Option<String>,
    pub html_url: String,
    pub published_at: Option<String>,
    pub assets: Vec<GitHubAsset>,
    #[serde(default)]
    pub target_commitish: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMetadata {
    pub last_check: SystemTime,
    pub installed_version: Option<String>,
    pub installed_from: Option<String>,
    pub previous_binary: Option<String>,
}

impl Default for UpdateMetadata {
    fn default() -> Self {
        Self {
            last_check: SystemTime::UNIX_EPOCH,
            installed_version: None,
            installed_from: None,
            previous_binary: None,
        }
    }
}

impl UpdateMetadata {
    pub fn load() -> Result<Self> {
        let path = metadata_path()?;
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = metadata_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn should_check(&self) -> bool {
        match self.last_check.elapsed() {
            Ok(elapsed) => elapsed > UPDATE_CHECK_INTERVAL,
            Err(_) => true,
        }
    }
}

fn metadata_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("update_metadata.json"))
}

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

pub fn should_auto_update() -> bool {
    if std::env::var("JCODE_NO_AUTO_UPDATE").is_ok() {
        return false;
    }

    if !is_release_build() {
        return false;
    }

    if let Ok(exe) = std::env::current_exe() {
        if is_inside_git_repo(&exe) {
            return false;
        }
    }

    true
}

fn is_inside_git_repo(path: &std::path::Path) -> bool {
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

pub fn fetch_latest_release_blocking() -> Result<GitHubRelease> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent("jcode-updater")
        .build()?;

    let response = client
        .get(&url)
        .send()
        .context("Failed to fetch release info")?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("No releases found");
    }

    if !response.status().is_success() {
        anyhow::bail!("GitHub API error: {}", response.status());
    }

    let release: GitHubRelease = response
        .json()
        .context("Failed to parse release info")?;

    Ok(release)
}

pub fn check_for_update_blocking() -> Result<Option<GitHubRelease>> {
    let current_version = env!("JCODE_VERSION");
    let release = fetch_latest_release_blocking()?;

    let release_version = release.tag_name.trim_start_matches('v');
    if release_version == current_version.trim_start_matches('v') {
        return Ok(None);
    }

    if version_is_newer(release_version, current_version.trim_start_matches('v')) {
        let asset_name = get_asset_name();
        let has_asset = release
            .assets
            .iter()
            .any(|a| a.name.starts_with(asset_name));

        if has_asset {
            return Ok(Some(release));
        }
    }

    Ok(None)
}

fn version_is_newer(release: &str, current: &str) -> bool {
    let parse = |v: &str| -> (u32, u32, u32) {
        let v = v.trim_start_matches('v');
        let parts: Vec<&str> = v.split('.').collect();
        let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };

    let r = parse(release);
    let c = parse(current);
    r > c
}

pub fn download_and_install_blocking(release: &GitHubRelease) -> Result<PathBuf> {
    let asset_name = get_asset_name();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.starts_with(asset_name))
        .ok_or_else(|| anyhow::anyhow!("No asset found for platform: {}", asset_name))?;

    let download_url = if asset.name.ends_with(".tar.gz") {
        asset.browser_download_url.clone()
    } else {
        asset.browser_download_url.clone()
    };

    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("jcode-update-{}", std::process::id()));

    let client = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent("jcode-updater")
        .build()?;

    let response = client
        .get(&download_url)
        .send()
        .context("Failed to download update")?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed: {}", response.status());
    }

    let bytes = response
        .bytes()
        .context("Failed to read download")?;

    if asset.name.ends_with(".tar.gz") {
        let cursor = std::io::Cursor::new(&bytes);
        let gz = flate2::read::GzDecoder::new(cursor);
        let mut archive = tar::Archive::new(gz);
        let mut extracted = false;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?.into_owned();
            let file_name = entry_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if file_name.starts_with("jcode") && !file_name.ends_with(".tar.gz") {
                entry.unpack(&temp_path)?;
                extracted = true;
                break;
            }
        }
        if !extracted {
            anyhow::bail!("Could not find jcode binary inside tar.gz archive");
        }
    } else {
        fs::write(&temp_path, &bytes).context("Failed to write temp file")?;
    }

    let mut perms = fs::metadata(&temp_path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&temp_path, perms)?;

    let install_dir = stable_install_dir()?;
    fs::create_dir_all(&install_dir)?;

    let version = release.tag_name.trim_start_matches('v');
    let versioned_path = install_dir.join(format!("jcode-{}", version));

    let current_stable = install_dir.join("jcode");
    let mut metadata = UpdateMetadata::load().unwrap_or_default();
    if current_stable.exists() {
        if let Ok(resolved) = fs::read_link(&current_stable) {
            metadata.previous_binary = Some(resolved.to_string_lossy().to_string());
        } else {
            let backup = install_dir.join(format!(
                "jcode-backup-{}",
                std::process::id()
            ));
            let _ = fs::copy(&current_stable, &backup);
            metadata.previous_binary = Some(backup.to_string_lossy().to_string());
        }
    }

    fs::rename(&temp_path, &versioned_path).or_else(|_| {
        fs::copy(&temp_path, &versioned_path)?;
        fs::remove_file(&temp_path)?;
        Ok::<_, std::io::Error>(())
    })?;

    let temp_symlink = install_dir.join(format!(".jcode-symlink-{}", std::process::id()));

    #[cfg(unix)]
    {
        let _ = fs::remove_file(&temp_symlink);
        std::os::unix::fs::symlink(&versioned_path, &temp_symlink)?;
        fs::rename(&temp_symlink, &current_stable)?;
    }

    metadata.installed_version = Some(release.tag_name.clone());
    metadata.installed_from = Some(asset.browser_download_url.clone());
    metadata.last_check = SystemTime::now();
    metadata.save()?;

    Ok(versioned_path)
}

fn stable_install_dir() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("builds").join("stable"))
}

pub enum UpdateCheckResult {
    NoUpdate,
    UpdateAvailable {
        current: String,
        latest: String,
        release: GitHubRelease,
    },
    UpdateInstalled {
        version: String,
        path: PathBuf,
    },
    Error(String),
}

pub fn check_and_maybe_update(auto_install: bool) -> UpdateCheckResult {
    if !should_auto_update() {
        return UpdateCheckResult::NoUpdate;
    }

    let metadata = UpdateMetadata::load().unwrap_or_default();
    if !metadata.should_check() {
        return UpdateCheckResult::NoUpdate;
    }

    match check_for_update_blocking() {
        Ok(Some(release)) => {
            let current = env!("JCODE_VERSION").to_string();
            let latest = release.tag_name.clone();

            if auto_install {
                eprintln!("⬇️  Downloading jcode {}...", latest);
                match download_and_install_blocking(&release) {
                    Ok(path) => UpdateCheckResult::UpdateInstalled {
                        version: latest,
                        path,
                    },
                    Err(e) => UpdateCheckResult::Error(format!("Failed to install: {}", e)),
                }
            } else {
                let mut metadata = UpdateMetadata::load().unwrap_or_default();
                metadata.last_check = SystemTime::now();
                let _ = metadata.save();
                UpdateCheckResult::UpdateAvailable {
                    current,
                    latest,
                    release,
                }
            }
        }
        Ok(None) => {
            let mut metadata = UpdateMetadata::load().unwrap_or_default();
            metadata.last_check = SystemTime::now();
            let _ = metadata.save();
            UpdateCheckResult::NoUpdate
        }
        Err(e) => UpdateCheckResult::Error(format!("Check failed: {}", e)),
    }
}

pub fn rollback() -> Result<Option<PathBuf>> {
    let metadata = UpdateMetadata::load()?;

    if let Some(ref previous) = metadata.previous_binary {
        let previous_path = PathBuf::from(previous);
        if previous_path.exists() {
            let install_dir = stable_install_dir()?;
            let current_stable = install_dir.join("jcode");
            let temp_symlink = install_dir.join(format!(".jcode-symlink-{}", std::process::id()));

            #[cfg(unix)]
            {
                let _ = fs::remove_file(&temp_symlink);
                std::os::unix::fs::symlink(&previous_path, &temp_symlink)?;
                fs::rename(&temp_symlink, &current_stable)?;
            }

            return Ok(Some(previous_path));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_is_newer() {
        assert!(version_is_newer("0.1.3", "0.1.2"));
        assert!(version_is_newer("0.2.0", "0.1.9"));
        assert!(version_is_newer("1.0.0", "0.9.9"));
        assert!(!version_is_newer("0.1.2", "0.1.2"));
        assert!(!version_is_newer("0.1.1", "0.1.2"));
        assert!(!version_is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn test_asset_name() {
        let name = get_asset_name();
        assert!(name.starts_with("jcode-"));
    }

    #[test]
    fn test_is_release_build() {
        assert!(!is_release_build());
    }

    #[test]
    fn test_should_auto_update_dev_build() {
        assert!(!should_auto_update());
    }
}
