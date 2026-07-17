use crate::build;
use crate::storage;
use anyhow::{Context, Result};
use jcode_update_core::{
    BACKGROUND_UPDATE_THRESHOLD, estimate_release_update_duration, estimate_source_update_duration,
    format_duration_estimate, get_asset_name, summarize_git_pull_failure, update_estimate,
    verify_asset_checksum_text, version_is_newer,
};
pub use jcode_update_core::{
    DownloadProgress, GIT_PULL_DIVERGED_SUMMARY, GitHubAsset, GitHubRelease, PreparedUpdate,
    UpdateCheckResult, UpdateEstimate, format_download_progress_bar, summary_is_divergence,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const GITHUB_REPO: &str = "1jehuang/jcode";
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(60); // minimum gap between checks
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// Time allowed for the initial TCP/TLS connect to the download host.
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Total wall-clock budget for a single download *attempt*.
///
/// This is intentionally a per-attempt budget, not a budget for the whole
/// asset. The old code used a single 120s total timeout for the entire
/// transfer, so on a slow link a multi-megabyte asset could never finish: it
/// was killed mid-stream, the partial bytes were discarded, and every relaunch
/// restarted from zero. We now cap each attempt and resume via HTTP Range, so
/// a slow-but-progressing download completes across several attempts while a
/// genuinely hung connection still gets bounded.
const DOWNLOAD_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(120);
/// How many *consecutive* stalled attempts (attempts that made no forward
/// progress) to tolerate before giving up. Any attempt that downloads new
/// bytes resets this counter, so a slow-but-progressing download keeps
/// resuming via HTTP Range for as long as it needs; only a genuinely stuck
/// connection eventually fails.
const DOWNLOAD_MAX_ATTEMPTS: usize = 10;
const DOWNLOAD_PROGRESS_UPDATE_STEP: u64 = 1_048_576;

pub fn print_centered(msg: &str) {
    let width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80);
    for line in msg.lines() {
        let visible_len = unicode_display_width(line);
        if visible_len >= width {
            println!("{}", line);
        } else {
            let pad = (width - visible_len) / 2;
            println!("{:>pad$}{}", "", line, pad = pad);
        }
    }
}

fn unicode_display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut w = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' {
            in_escape = true;
            continue;
        }
        w += UnicodeWidthChar::width(c).unwrap_or(0);
    }
    w
}

pub fn is_release_build() -> bool {
    jcode_build_meta::is_release_build()
}

fn current_update_semver() -> &'static str {
    jcode_build_meta::update_semver()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMetadata {
    pub last_check: SystemTime,
    pub installed_version: Option<String>,
    pub installed_from: Option<String>,
    #[serde(default)]
    pub last_release_update_secs: Option<f64>,
    #[serde(default)]
    pub last_source_update_secs: Option<f64>,
}

impl Default for UpdateMetadata {
    fn default() -> Self {
        Self {
            last_check: SystemTime::UNIX_EPOCH,
            installed_version: None,
            installed_from: None,
            last_release_update_secs: None,
            last_source_update_secs: None,
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

fn source_build_root() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("builds").join("source"))
}

fn source_build_repo_dir() -> Result<PathBuf> {
    Ok(source_build_root()?.join("jcode"))
}

fn record_release_update_duration(duration: Duration) {
    if let Ok(mut metadata) = UpdateMetadata::load() {
        metadata.last_release_update_secs = Some(duration.as_secs_f64());
        let _ = metadata.save();
    }
}

fn record_source_update_duration(duration: Duration) {
    if let Ok(mut metadata) = UpdateMetadata::load() {
        metadata.last_source_update_secs = Some(duration.as_secs_f64());
        let _ = metadata.save();
    }
}

pub fn should_auto_update() -> bool {
    if std::env::var("JCODE_NO_AUTO_UPDATE").is_ok() {
        return false;
    }

    if !is_release_build() {
        return false;
    }

    if let Ok(exe) = std::env::current_exe()
        && is_inside_git_repo(&exe)
    {
        return false;
    }

    true
}

pub fn run_git_pull_ff_only(repo_dir: &Path, quiet: bool) -> Result<()> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("pull").arg("--ff-only");
    if quiet {
        cmd.arg("-q");
    }
    let output = cmd
        .current_dir(repo_dir)
        .output()
        .context("Failed to run git pull")?;

    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!("{}", summarize_git_pull_failure(&output.stderr));
    }
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

    let response = github_api_request(&client, &url)
        .send()
        .context("Failed to fetch release info")?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("No releases found");
    }

    if !response.status().is_success() {
        anyhow::bail!("GitHub API error: {}", response.status());
    }

    let release: GitHubRelease = response.json().context("Failed to parse release info")?;

    Ok(release)
}

fn github_api_request(
    client: &reqwest::blocking::Client,
    url: &str,
) -> reqwest::blocking::RequestBuilder {
    let request = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");

    // Authenticated requests use the user's 5000 req/h quota instead of the
    // shared unauthenticated 60 req/h per-IP bucket, which other tools on the
    // same machine or NAT can exhaust and cause spurious 403s on update
    // checks. Falls back to unauthenticated when no token is available.
    if let Some(token) = jcode_base::github::github_public_api_token() {
        request.bearer_auth(token)
    } else {
        request
    }
}

fn latest_main_sha_blocking() -> Result<String> {
    let url = format!("https://api.github.com/repos/{}/commits/main", GITHUB_REPO);
    let client = reqwest::blocking::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent("jcode-updater")
        .build()?;

    let response = github_api_request(&client, &url)
        .send()
        .context("Failed to check main branch")?;
    if !response.status().is_success() {
        anyhow::bail!("GitHub API error checking main: {}", response.status());
    }

    let commit: serde_json::Value = response.json().context("Failed to parse commit info")?;
    Ok(commit["sha"]
        .as_str()
        .unwrap_or("")
        .get(..7)
        .unwrap_or("")
        .to_string())
}

fn platform_asset(release: &GitHubRelease) -> Result<&GitHubAsset> {
    let asset_name = get_asset_name();
    release
        .assets
        .iter()
        .find(|a| a.name.starts_with(asset_name))
        .ok_or_else(|| anyhow::anyhow!("No asset found for platform: {}", asset_name))
}

fn checksum_asset(release: &GitHubRelease) -> Option<&GitHubAsset> {
    release.assets.iter().find(|a| a.name == "SHA256SUMS")
}

fn verify_asset_checksum_if_available(
    client: &reqwest::blocking::Client,
    release: &GitHubRelease,
    asset: &GitHubAsset,
    bytes: &[u8],
) -> Result<()> {
    let Some(checksum_asset) = checksum_asset(release) else {
        crate::logging::info(&format!(
            "Release {} does not include SHA256SUMS; skipping checksum verification",
            release.tag_name
        ));
        return Ok(());
    };

    let response = client
        .get(&checksum_asset.browser_download_url)
        .send()
        .context("Failed to download SHA256SUMS")?;
    if !response.status().is_success() {
        anyhow::bail!("SHA256SUMS download failed: {}", response.status());
    }
    let contents = response.text().context("Failed to read SHA256SUMS")?;
    verify_asset_checksum_text(&contents, &asset.name, bytes)?;
    crate::logging::info(&format!("Verified SHA256 checksum for {}", asset.name));
    Ok(())
}

fn synthetic_main_release(latest_sha: &str) -> GitHubRelease {
    GitHubRelease {
        tag_name: format!("main-{}", latest_sha),
        _name: Some(format!("Built from main ({})", latest_sha)),
        _html_url: format!("https://github.com/{}/commit/{}", GITHUB_REPO, latest_sha),
        _published_at: None,
        assets: vec![],
        _target_commitish: latest_sha.to_string(),
    }
}

fn install_main_source_update_blocking(latest_sha: &str) -> Result<PathBuf> {
    let path = build_from_source()?;
    crate::logging::info(&format!(
        "Main channel: built successfully at {}",
        path.display()
    ));

    let mut metadata = UpdateMetadata::load().unwrap_or_default();
    let channel_version = format!("main-{}", latest_sha);
    build::install_binary_at_version(&path, &channel_version)
        .context("Failed to install built binary")?;
    // Carry the long-lived daemon's reload target forward too, but only when it
    // was tracking stable. A deliberately-promoted self-dev shared-server build
    // is left untouched so the update never silently wipes it out.
    if let Err(error) = build::advance_shared_server_if_tracking_stable(&channel_version) {
        crate::logging::warn(&format!(
            "update: failed to advance shared-server channel to {}: {}",
            channel_version, error
        ));
    }
    build::update_stable_symlink(&channel_version)?;
    build::update_current_symlink(&channel_version)?;
    build::update_launcher_symlink_to_current()?;

    metadata.installed_version = Some(channel_version.clone());
    metadata.installed_from = Some("source".to_string());
    metadata.last_check = SystemTime::now();
    metadata.save()?;

    Ok(path)
}

fn prepare_stable_update_blocking() -> Result<PreparedUpdate> {
    let current_version = jcode_build_meta::version();
    let current_update_version = current_update_semver();
    let release = fetch_latest_release_blocking()?;
    let release_version = release.tag_name.trim_start_matches('v');

    if release_version == current_update_version.trim_start_matches('v')
        || !version_is_newer(
            release_version,
            current_update_version.trim_start_matches('v'),
        )
    {
        return Ok(PreparedUpdate::None {
            current: current_version.to_string(),
        });
    }

    let Ok(asset) = platform_asset(&release) else {
        return Ok(PreparedUpdate::None {
            current: current_version.to_string(),
        });
    };
    let metadata = UpdateMetadata::load().unwrap_or_default();
    let duration = estimate_release_update_duration(asset._size, metadata.last_release_update_secs);
    let size_mb = asset._size as f64 / (1024.0 * 1024.0);
    let summary = format!(
        "Prebuilt update {} → {} (~{:.0} MB, {}). {}",
        current_version,
        release.tag_name,
        size_mb,
        format_duration_estimate(duration),
        if duration >= BACKGROUND_UPDATE_THRESHOLD {
            "Running in the background and will reload when it is ready."
        } else {
            "This should be quick."
        }
    );

    Ok(PreparedUpdate::Stable {
        release,
        estimate: update_estimate(summary, duration),
    })
}

fn prepare_main_update_blocking() -> Result<PreparedUpdate> {
    let current_hash = jcode_build_meta::git_hash();
    if current_hash.is_empty() || current_hash == "unknown" {
        crate::logging::info("Main channel: no git hash in binary, skipping update check");
        return Ok(PreparedUpdate::None {
            current: jcode_build_meta::version().to_string(),
        });
    }

    let latest_sha = latest_main_sha_blocking()?;
    if latest_sha.is_empty() {
        return Ok(PreparedUpdate::None {
            current: current_hash.to_string(),
        });
    }

    let current_short = if current_hash.len() >= 7 {
        &current_hash[..7]
    } else {
        current_hash
    };

    if current_short == latest_sha {
        crate::logging::info(&format!("Main channel: up to date ({})", current_short));
        return Ok(PreparedUpdate::None {
            current: format!("main-{}", current_short),
        });
    }

    crate::logging::info(&format!(
        "Main channel: new commit {} -> {}",
        current_short, latest_sha
    ));

    if has_cargo() {
        let repo_dir = source_build_repo_dir()?;
        let repo_exists = repo_dir.join(".git").exists();
        let has_previous_build = build::release_binary_path(&repo_dir).exists();
        let metadata = UpdateMetadata::load().unwrap_or_default();
        let duration = estimate_source_update_duration(
            repo_exists,
            has_previous_build,
            metadata.last_source_update_secs,
        );
        let action = if repo_exists {
            if has_previous_build {
                "git pull + cargo build with a warm build cache"
            } else {
                "git pull + cargo build"
            }
        } else {
            "initial clone + cargo build"
        };
        let summary = format!(
            "Source update {} → main-{} requires {} ({}). Running in the background and will reload when it is ready.",
            current_short,
            latest_sha,
            action,
            format_duration_estimate(duration)
        );
        return Ok(PreparedUpdate::MainSource {
            latest_sha,
            estimate: update_estimate(summary, duration),
        });
    }

    crate::logging::info("Main channel: cargo not found, falling back to latest release");
    prepare_stable_update_blocking()
}

pub fn prepare_update_blocking() -> Result<PreparedUpdate> {
    let channel = crate::config::config().features.update_channel;
    match channel {
        crate::config::UpdateChannel::Main => prepare_main_update_blocking(),
        crate::config::UpdateChannel::Stable => prepare_stable_update_blocking(),
    }
}

pub fn spawn_background_session_update(session_id: String) {
    std::thread::spawn(move || {
        use crate::bus::{Bus, BusEvent, ClientMaintenanceAction, SessionUpdateStatus};

        let action = ClientMaintenanceAction::Update;

        let publish = |status| Bus::global().publish(BusEvent::SessionUpdateStatus(status));

        match prepare_update_blocking() {
            Ok(PreparedUpdate::None { current }) => {
                publish(SessionUpdateStatus::NoUpdate {
                    session_id,
                    current,
                });
            }
            Ok(PreparedUpdate::Stable { release, estimate }) => {
                publish(SessionUpdateStatus::Status {
                    session_id: session_id.clone(),
                    action,
                    message: estimate.summary,
                });
                publish(SessionUpdateStatus::Status {
                    session_id: session_id.clone(),
                    action,
                    message: format!(
                        "Downloading {} (estimated {})...",
                        release.tag_name,
                        format_duration_estimate(estimate.duration)
                    ),
                });
                let progress_session_id = session_id.clone();
                let progress_version = release.tag_name.clone();
                match download_and_install_blocking_with_progress(&release, |progress| {
                    publish(SessionUpdateStatus::Status {
                        session_id: progress_session_id.clone(),
                        action,
                        message: format!(
                            "{} {}",
                            progress_version,
                            format_download_progress_bar(progress)
                        ),
                    });
                }) {
                    Ok(_) => publish(SessionUpdateStatus::ReadyToReload {
                        session_id,
                        action,
                        version: release.tag_name,
                    }),
                    Err(error) => publish(SessionUpdateStatus::Error {
                        session_id,
                        action,
                        message: format!("Update failed: {}", error),
                    }),
                }
            }
            Ok(PreparedUpdate::MainSource {
                latest_sha,
                estimate,
            }) => {
                publish(SessionUpdateStatus::Status {
                    session_id: session_id.clone(),
                    action,
                    message: estimate.summary,
                });
                publish(SessionUpdateStatus::Status {
                    session_id: session_id.clone(),
                    action,
                    message: format!(
                        "Building main-{} in the background (estimated {})...",
                        latest_sha,
                        format_duration_estimate(estimate.duration)
                    ),
                });
                match install_main_source_update_blocking(&latest_sha) {
                    Ok(_) => publish(SessionUpdateStatus::ReadyToReload {
                        session_id,
                        action,
                        version: format!("main-{}", latest_sha),
                    }),
                    Err(error) => publish(SessionUpdateStatus::Error {
                        session_id,
                        action,
                        message: format!("Update failed: {}", error),
                    }),
                }
            }
            Err(error) => publish(SessionUpdateStatus::Error {
                session_id,
                action,
                message: format!("Update check failed: {}", error),
            }),
        }
    });
}

pub fn check_for_update_blocking() -> Result<Option<GitHubRelease>> {
    let channel = crate::config::config().features.update_channel;
    match channel {
        crate::config::UpdateChannel::Main => check_for_main_update_blocking(),
        crate::config::UpdateChannel::Stable => check_for_stable_update_blocking(),
    }
}

fn check_for_stable_update_blocking() -> Result<Option<GitHubRelease>> {
    let current_version = current_update_semver();
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

/// Check for updates on the main branch (cutting edge channel).
/// Compares the current binary's git hash against the latest commit on main.
/// If a new commit is found:
///   - Tries to build from source if cargo is available
///   - Falls back to latest GitHub Release if not
fn check_for_main_update_blocking() -> Result<Option<GitHubRelease>> {
    let current_hash = jcode_build_meta::git_hash();
    if current_hash.is_empty() || current_hash == "unknown" {
        crate::logging::info("Main channel: no git hash in binary, skipping update check");
        return Ok(None);
    }

    let latest_sha = latest_main_sha_blocking()?;

    if latest_sha.is_empty() {
        return Ok(None);
    }

    // Compare short hashes
    let current_short = if current_hash.len() >= 7 {
        &current_hash[..7]
    } else {
        current_hash
    };

    if current_short == latest_sha {
        crate::logging::info(&format!("Main channel: up to date ({})", current_short));
        return Ok(None);
    }

    crate::logging::info(&format!(
        "Main channel: new commit {} -> {}",
        current_short, latest_sha
    ));

    // Try to build from source
    if has_cargo() {
        crate::logging::info("Main channel: cargo found, attempting build from source");
        match install_main_source_update_blocking(&latest_sha) {
            Ok(_) => {
                return Ok(Some(synthetic_main_release(&latest_sha)));
            }
            Err(e) => {
                crate::logging::error(&format!("Main channel: build failed: {}", e));
                // Fall through to release fallback
            }
        }
    } else {
        crate::logging::info("Main channel: cargo not found, falling back to latest release");
    }

    // Fallback: use latest stable release if available
    if let Ok(release) = fetch_latest_release_blocking() {
        let asset_name = get_asset_name();
        let has_asset = release
            .assets
            .iter()
            .any(|a| a.name.starts_with(asset_name));
        if has_asset {
            let release_version = release.tag_name.trim_start_matches('v');
            let current_version = current_update_semver().trim_start_matches('v');
            if version_is_newer(release_version, current_version) {
                return Ok(Some(release));
            }
        }
    }

    Ok(None)
}

/// Check if cargo is available on the system
fn has_cargo() -> bool {
    std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build jcode from source by cloning/pulling the repo and running cargo build
fn build_from_source() -> Result<PathBuf> {
    let started = Instant::now();
    let build_dir = source_build_root()?;
    fs::create_dir_all(&build_dir)?;

    let repo_dir = build_dir.join("jcode");

    if repo_dir.join(".git").exists() {
        // Pull latest
        crate::logging::info("Main channel: pulling latest from main...");
        let output = std::process::Command::new("git")
            .args(["pull", "--ff-only", "origin", "main"])
            .current_dir(&repo_dir)
            .output()
            .context("Failed to run git pull")?;

        if !output.status.success() {
            // If pull fails (e.g. diverged), reset to origin/main
            let summary = summarize_git_pull_failure(&output.stderr);
            crate::logging::warn(&format!("{}, trying reset", summary));
            let output = std::process::Command::new("git")
                .args(["fetch", "origin", "main"])
                .current_dir(&repo_dir)
                .output()
                .context("Failed to run git fetch")?;
            if !output.status.success() {
                anyhow::bail!(
                    "git fetch failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let output = std::process::Command::new("git")
                .args(["reset", "--hard", "origin/main"])
                .current_dir(&repo_dir)
                .output()
                .context("Failed to run git reset")?;
            if !output.status.success() {
                anyhow::bail!(
                    "git reset failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    } else {
        // Clone
        crate::logging::info("Main channel: cloning repository...");
        let clone_url = format!("https://github.com/{}.git", GITHUB_REPO);
        let output = std::process::Command::new("git")
            .args([
                "clone", "--depth", "1", "--branch", "main", &clone_url, "jcode",
            ])
            .current_dir(&build_dir)
            .output()
            .context("Failed to run git clone")?;

        if !output.status.success() {
            anyhow::bail!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    // Build
    crate::logging::info("Main channel: building with cargo...");
    let output = std::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .env("JCODE_RELEASE_BUILD", "1")
        .output()
        .context("Failed to run cargo build")?;

    if !output.status.success() {
        anyhow::bail!(
            "cargo build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let binary = build::release_binary_path(&repo_dir);
    if !binary.exists() {
        anyhow::bail!("Built binary not found at {}", binary.display());
    }

    record_source_update_duration(started.elapsed());

    Ok(binary)
}

pub fn download_and_install_blocking(release: &GitHubRelease) -> Result<PathBuf> {
    download_and_install_blocking_with_progress(release, |_| {})
}

/// Download an asset into memory, retrying with HTTP Range resume so a slow or
/// flaky connection recovers instead of restarting from zero.
///
/// Returns the full asset bytes plus the best-known total size (for callers
/// that want a final size). Progress callbacks are invoked across retries using
/// the cumulative bytes already on disk, so the UI never appears to go
/// backwards when a stalled connection reconnects.
fn download_asset_with_resume(
    client: &reqwest::blocking::Client,
    download_url: &str,
    total_hint: Option<u64>,
    on_progress: &mut impl FnMut(DownloadProgress),
) -> Result<(Vec<u8>, Option<u64>)> {
    let mut bytes: Vec<u8> =
        Vec::with_capacity(total_hint.unwrap_or_default().min(usize::MAX as u64) as usize);
    let mut total = total_hint;
    let mut next_progress_at = 0_u64;
    let mut last_error: Option<anyhow::Error> = None;
    // Count only *consecutive stalls* (attempts that made no forward progress).
    // A slow-but-advancing download resets this and keeps resuming, so it can
    // take as long as it needs; only a genuinely stuck connection gives up.
    let mut stalls = 0_usize;
    let mut attempt = 0_usize;

    on_progress(DownloadProgress {
        downloaded: 0,
        total,
    });

    while stalls < DOWNLOAD_MAX_ATTEMPTS {
        attempt += 1;
        let resume_from = bytes.len() as u64;
        let mut request = client.get(download_url);
        if resume_from > 0 {
            // Ask the server to continue where we left off.
            request = request.header(reqwest::header::RANGE, format!("bytes={}-", resume_from));
        }

        let response = match request.send() {
            Ok(response) => response,
            Err(err) => {
                last_error = Some(anyhow::anyhow!("Failed to download update: {}", err));
                log_download_retry(attempt, resume_from, &err);
                stalls += 1;
                continue;
            }
        };

        let status = response.status();
        if resume_from > 0 && status == reqwest::StatusCode::OK {
            // Server ignored the Range header and is resending from the start;
            // discard the partial buffer so we don't corrupt the result.
            bytes.clear();
            next_progress_at = 0;
        } else if resume_from > 0 && status != reqwest::StatusCode::PARTIAL_CONTENT {
            last_error = Some(anyhow::anyhow!(
                "Resume request returned unexpected status {}",
                status
            ));
            log_download_retry_status(attempt, resume_from, status);
            stalls += 1;
            continue;
        } else if !status.is_success() {
            last_error = Some(anyhow::anyhow!("Download failed: {}", status));
            log_download_retry_status(attempt, resume_from, status);
            stalls += 1;
            continue;
        }

        // Establish the total size. For a 206 response, content-length is the
        // remaining bytes, so prefer the original hint / Content-Range total.
        if total.is_none() {
            total = content_range_total(&response).or_else(|| {
                if status == reqwest::StatusCode::PARTIAL_CONTENT {
                    response
                        .content_length()
                        .map(|len| len.saturating_add(bytes.len() as u64))
                } else {
                    response.content_length()
                }
            });
        }

        let before = bytes.len() as u64;
        let read_result = read_response_into(
            response,
            &mut bytes,
            &mut next_progress_at,
            total,
            on_progress,
        );
        let made_progress = bytes.len() as u64 > before;

        match read_result {
            Ok(()) => {
                let downloaded = bytes.len() as u64;
                // If we know the total and fell short, treat as a stall and retry.
                if let Some(total) = total
                    && downloaded < total
                {
                    last_error = Some(anyhow::anyhow!(
                        "Download ended early ({} of {} bytes)",
                        downloaded,
                        total
                    ));
                    log_download_retry_short(attempt, downloaded, total);
                    stalls = if made_progress { 0 } else { stalls + 1 };
                    continue;
                }
                on_progress(DownloadProgress { downloaded, total });
                return Ok((bytes, total));
            }
            Err(err) => {
                let downloaded = bytes.len() as u64;
                crate::logging::warn(&format!(
                    "Update download attempt {} stream error at {} bytes: {}; retrying with resume",
                    attempt, downloaded, err
                ));
                last_error = Some(err);
                stalls = if made_progress { 0 } else { stalls + 1 };
                continue;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!(
            "Download stalled with no progress after {} attempts",
            DOWNLOAD_MAX_ATTEMPTS
        )
    }))
}

fn read_response_into(
    mut response: reqwest::blocking::Response,
    bytes: &mut Vec<u8>,
    next_progress_at: &mut u64,
    total: Option<u64>,
    on_progress: &mut impl FnMut(DownloadProgress),
) -> Result<()> {
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .context("Failed to read download")?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        let downloaded = bytes.len() as u64;
        if downloaded >= *next_progress_at || total.is_some_and(|total| downloaded >= total) {
            on_progress(DownloadProgress { downloaded, total });
            *next_progress_at = downloaded.saturating_add(DOWNLOAD_PROGRESS_UPDATE_STEP);
        }
    }
    Ok(())
}

fn content_range_total(response: &reqwest::blocking::Response) -> Option<u64> {
    // Content-Range: bytes 200-1023/1024
    response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit('/').next())
        .and_then(|total| total.trim().parse::<u64>().ok())
}

fn log_download_retry(attempt: usize, resume_from: u64, err: &impl std::fmt::Display) {
    crate::logging::warn(&format!(
        "Update download attempt {}/{} failed at {} bytes: {}; retrying with resume",
        attempt, DOWNLOAD_MAX_ATTEMPTS, resume_from, err
    ));
}

fn log_download_retry_status(attempt: usize, resume_from: u64, status: reqwest::StatusCode) {
    crate::logging::warn(&format!(
        "Update download attempt {}/{} got status {} at {} bytes; retrying with resume",
        attempt, DOWNLOAD_MAX_ATTEMPTS, status, resume_from
    ));
}

fn log_download_retry_short(attempt: usize, downloaded: u64, total: u64) {
    crate::logging::warn(&format!(
        "Update download attempt {}/{} ended early ({} of {} bytes); retrying with resume",
        attempt, DOWNLOAD_MAX_ATTEMPTS, downloaded, total
    ));
}

pub fn download_and_install_blocking_with_progress(
    release: &GitHubRelease,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf> {
    let started = Instant::now();
    let asset_name = get_asset_name();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.starts_with(asset_name))
        .ok_or_else(|| anyhow::anyhow!("No asset found for platform: {}", asset_name))?;

    let download_url = asset.browser_download_url.clone();

    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("jcode-update-{}", std::process::id()));

    // The `timeout` here applies per request. Since each retry below is a
    // separate request, this acts as a *per-attempt* budget rather than a cap
    // on the whole asset: a slow-but-progressing download resumes via HTTP
    // Range across attempts (up to DOWNLOAD_MAX_ATTEMPTS), so it can complete
    // even when a single attempt would not finish in time. A genuinely hung
    // connection is still bounded by the per-attempt timeout.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_ATTEMPT_TIMEOUT)
        .user_agent("jcode-updater")
        .build()?;

    let total_hint = if asset._size > 0 {
        Some(asset._size)
    } else {
        None
    };
    let (bytes, _total) =
        download_asset_with_resume(&client, &download_url, total_hint, &mut on_progress)?;

    verify_asset_checksum_if_available(&client, release, asset, &bytes)?;

    let mut installed_version_dir: Option<PathBuf> = None;
    if asset.name.ends_with(".tar.gz") {
        let cursor = std::io::Cursor::new(&bytes);
        let gz = flate2::read::GzDecoder::new(cursor);
        let mut archive = tar::Archive::new(gz);
        let extract_dir = temp_path.with_extension("extract");
        if extract_dir.exists() {
            let _ = fs::remove_dir_all(&extract_dir);
        }
        fs::create_dir_all(&extract_dir).context("Failed to create archive extraction dir")?;
        let mut extracted_binary: Option<PathBuf> = None;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?.into_owned();
            if entry_path.components().count() != 1 {
                continue;
            }
            let file_name = entry_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if file_name.is_empty() || file_name.ends_with(".tar.gz") {
                continue;
            }
            let dest = extract_dir.join(&file_name);
            entry.unpack(&dest)?;
            if file_name.starts_with("jcode") && !file_name.ends_with(".bin") {
                extracted_binary = Some(dest);
            }
        }
        let Some(extracted_binary) = extracted_binary else {
            anyhow::bail!("Could not find jcode binary inside tar.gz archive");
        };
        crate::platform::set_permissions_executable(&extracted_binary)?;

        let version = release.tag_name.trim_start_matches('v');
        let dest_dir = build::builds_dir()?.join("versions").join(version);
        fs::create_dir_all(&dest_dir).context("Failed to create version install dir")?;
        let mut installed_files = Vec::new();
        for entry in fs::read_dir(&extract_dir).context("Failed to read extracted archive")? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name_string = name.to_string_lossy();
            let dest_name = if name_string == get_asset_name()
                || name_string == format!("{}.exe", get_asset_name())
            {
                build::binary_name().to_string()
            } else {
                name_string.to_string()
            };
            let dest = dest_dir.join(dest_name);
            if dest.exists() {
                fs::remove_file(&dest)?;
            }
            fs::copy(entry.path(), &dest)
                .with_context(|| format!("Failed to install {}", dest.display()))?;
            if dest
                .file_name()
                .is_some_and(|name| name == build::binary_name())
                || dest.extension().is_some_and(|ext| ext == "bin")
            {
                crate::platform::set_permissions_executable(&dest)?;
            }
            installed_files.push(dest);
        }
        // Give every installed file the same mtime. The wrapper script and the
        // `.bin` payload otherwise land with whatever sub-second skew the copy
        // loop produced, and any code comparing binary freshness by mtime then
        // sees two "different age" files for one logical install.
        let install_stamp = SystemTime::now();
        for path in &installed_files {
            if let Ok(file) = fs::File::options().write(true).open(path) {
                let _ = file.set_modified(install_stamp);
            }
        }
        let _ = fs::remove_dir_all(&extract_dir);
        installed_version_dir = Some(dest_dir.join(build::binary_name()));
    } else {
        fs::write(&temp_path, &bytes).context("Failed to write temp file")?;
    }

    let version = release.tag_name.trim_start_matches('v');
    let mut metadata = UpdateMetadata::load().unwrap_or_default();

    let versioned_path = if let Some(versioned_path) = installed_version_dir {
        versioned_path
    } else {
        crate::platform::set_permissions_executable(&temp_path)?;
        let versioned_path = build::install_binary_at_version(&temp_path, version)?;
        let _ = fs::remove_file(&temp_path);
        versioned_path
    };
    if let Err(error) = build::advance_shared_server_if_tracking_stable(version) {
        crate::logging::warn(&format!(
            "update: failed to advance shared-server channel to {}: {}",
            version, error
        ));
    }
    build::update_stable_symlink(version)?;
    build::update_current_symlink(version)?;
    build::update_launcher_symlink_to_current()?;

    metadata.installed_version = Some(release.tag_name.clone());
    metadata.installed_from = Some(asset.browser_download_url.clone());
    metadata.last_check = SystemTime::now();
    metadata.save()?;
    record_release_update_duration(started.elapsed());

    Ok(versioned_path)
}

pub fn check_and_maybe_update(auto_install: bool) -> UpdateCheckResult {
    use crate::bus::{Bus, BusEvent, UpdateStatus};

    if !should_auto_update() {
        return UpdateCheckResult::NoUpdate;
    }

    let metadata = UpdateMetadata::load().unwrap_or_default();
    if !metadata.should_check() {
        return UpdateCheckResult::NoUpdate;
    }

    Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Checking));

    match check_for_update_blocking() {
        Ok(Some(release)) => {
            let current = jcode_build_meta::version().to_string();
            let latest = release.tag_name.clone();

            Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Available {
                current: current.clone(),
                latest: latest.clone(),
            }));

            if auto_install {
                Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Downloading {
                    version: latest.clone(),
                }));
                match download_and_install_blocking(&release) {
                    Ok(path) => {
                        Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Installed {
                            version: latest.clone(),
                        }));
                        UpdateCheckResult::UpdateInstalled {
                            version: latest,
                            path,
                        }
                    }
                    Err(e) => {
                        let msg = format!("Failed to install: {}", e);
                        Bus::global()
                            .publish(BusEvent::UpdateStatus(UpdateStatus::Error(msg.clone())));
                        UpdateCheckResult::Error(msg)
                    }
                }
            } else {
                let mut metadata = UpdateMetadata::load().unwrap_or_default();
                metadata.last_check = SystemTime::now();
                let _ = metadata.save();
                UpdateCheckResult::UpdateAvailable {
                    current,
                    latest,
                    _release: release,
                }
            }
        }
        Ok(None) => {
            repair_stale_shared_server_after_no_update();
            Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::UpToDate));
            let mut metadata = UpdateMetadata::load().unwrap_or_default();
            metadata.last_check = SystemTime::now();
            let _ = metadata.save();
            UpdateCheckResult::NoUpdate
        }
        Err(e) => {
            let msg = format!("Check failed: {}", e);
            Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Error(msg.clone())));
            UpdateCheckResult::Error(msg)
        }
    }
}

fn repair_stale_shared_server_after_no_update() {
    match build::repair_stale_shared_server_channel() {
        Ok(build::SharedServerRepair::Repaired {
            previous,
            repaired_to,
        }) => {
            crate::logging::info(&format!(
                "update: repaired stale shared-server channel {:?} -> {} after no-op update check",
                previous, repaired_to
            ));
        }
        Ok(build::SharedServerRepair::AlreadyCurrent) => {}
        Err(error) => {
            crate::logging::warn(&format!(
                "update: failed to repair stale shared-server channel after no-op update check: {}",
                error
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_update_core::parse_sha256sums;
    use sha2::{Digest, Sha256};

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
    fn test_format_download_progress_bar_known_total() {
        let rendered = format_download_progress_bar(DownloadProgress {
            downloaded: 512,
            total: Some(1024),
        });
        assert!(rendered.contains("50%"));
        assert!(rendered.contains("512 B/1.0 KiB"));
        assert!(rendered.contains('█'));
        assert!(rendered.contains('░'));
    }

    #[test]
    fn test_format_download_progress_bar_unknown_total() {
        let rendered = format_download_progress_bar(DownloadProgress {
            downloaded: 2 * 1024 * 1024,
            total: None,
        });
        assert_eq!(rendered, "Downloading update... 2.0 MiB downloaded");
    }

    #[test]
    fn test_parse_sha256sums_accepts_standard_and_binary_lines() {
        let digest_a = "a".repeat(64);
        let digest_b = "B".repeat(64);
        let digest_b_lower = "b".repeat(64);
        let contents = format!(
            "# generated by release workflow\n{}  jcode-linux-x86_64.tar.gz\r\n{} *jcode-windows-x86_64.exe\n",
            digest_a, digest_b
        );
        let parsed = parse_sha256sums(&contents).unwrap();
        assert_eq!(
            parsed.get("jcode-linux-x86_64.tar.gz").map(String::as_str),
            Some(digest_a.as_str())
        );
        assert_eq!(
            parsed.get("jcode-windows-x86_64.exe").map(String::as_str),
            Some(digest_b_lower.as_str())
        );
    }

    #[test]
    fn test_verify_asset_checksum_text_accepts_matching_digest() {
        let bytes = b"hello update";
        let digest = format!("{:x}", Sha256::digest(bytes));
        let contents = format!("{}  jcode-linux-x86_64.tar.gz\n", digest);
        verify_asset_checksum_text(&contents, "jcode-linux-x86_64.tar.gz", bytes).unwrap();
    }

    #[test]
    fn test_verify_asset_checksum_text_rejects_mismatch() {
        let wrong = "0".repeat(64);
        let contents = format!("{}  jcode-linux-x86_64.tar.gz\n", wrong);
        let err = verify_asset_checksum_text(&contents, "jcode-linux-x86_64.tar.gz", b"actual")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Checksum mismatch"));
    }

    #[test]
    fn test_verify_asset_checksum_text_requires_asset_entry() {
        let digest = "1".repeat(64);
        let contents = format!("{}  other-asset.tar.gz\n", digest);
        let err = verify_asset_checksum_text(&contents, "jcode-linux-x86_64.tar.gz", b"actual")
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not list"));
    }

    #[test]
    fn test_parse_sha256sums_rejects_invalid_digest() {
        let err = parse_sha256sums("not-a-sha  jcode-linux-x86_64.tar.gz\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid SHA256 digest"));
    }

    #[test]
    fn test_is_release_build() {
        assert!(!is_release_build());
    }

    #[test]
    fn test_should_auto_update_dev_build() {
        assert!(!should_auto_update());
    }

    #[test]
    fn test_summarize_git_pull_failure_diverged() {
        let stderr = b"hint: You have divergent branches and need to specify how to reconcile them.\nfatal: Need to specify how to reconcile divergent branches.\n";
        assert_eq!(
            summarize_git_pull_failure(stderr),
            jcode_update_core::GIT_PULL_DIVERGED_SUMMARY
        );
        assert!(jcode_update_core::summary_is_divergence(
            &summarize_git_pull_failure(stderr)
        ));
    }

    #[test]
    fn test_summarize_git_pull_failure_no_tracking_branch() {
        let stderr = b"There is no tracking information for the current branch.\n";
        assert_eq!(
            summarize_git_pull_failure(stderr),
            "git pull failed: current branch has no upstream tracking branch"
        );
    }

    #[test]
    fn test_summarize_git_pull_failure_uses_first_non_hint_line() {
        let stderr = b"hint: test hint\nfatal: repository not found\n";
        assert_eq!(
            summarize_git_pull_failure(stderr),
            "git pull failed: repository not found"
        );
    }

    #[test]
    fn test_estimate_release_update_duration_uses_size_buckets() {
        assert_eq!(
            estimate_release_update_duration(10 * 1024 * 1024, None),
            Duration::from_secs(10)
        );
        assert_eq!(
            estimate_release_update_duration(40 * 1024 * 1024, None),
            Duration::from_secs(35)
        );
    }

    #[test]
    fn test_estimate_source_update_duration_prefers_history() {
        assert_eq!(
            estimate_source_update_duration(true, true, Some(123.4)),
            Duration::from_secs(123)
        );
    }

    #[test]
    fn test_content_range_total_parses_total() {
        use reqwest::header::{HeaderMap, HeaderValue};
        // Build a Response is awkward; test the parser via a header map directly.
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 200-1023/1024"),
        );
        let parsed = headers
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit('/').next())
            .and_then(|total| total.trim().parse::<u64>().ok());
        assert_eq!(parsed, Some(1024));
    }

    #[test]
    fn test_content_range_total_unknown_size_is_none() {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 200-1023/*"),
        );
        let parsed = headers
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit('/').next())
            .and_then(|total| total.trim().parse::<u64>().ok());
        assert_eq!(parsed, None);
    }

    /// End-to-end resume test: a tiny HTTP server serves the first half of the
    /// body then drops the connection, and on the resumed Range request serves
    /// the rest. The download must recover and return the full payload.
    #[test]
    fn test_download_asset_with_resume_recovers_from_dropped_connection() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let payload: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
        let total = payload.len();
        let split = total / 2;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let payload_for_server = payload.clone();
        let request_count = Arc::new(AtomicUsize::new(0));
        let request_count_server = Arc::clone(&request_count);

        let handle = std::thread::spawn(move || {
            // Serve exactly two connections: first truncated, second resumed.
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let n = request_count_server.fetch_add(1, Ordering::SeqCst);

                // Parse request headers; look for a Range header.
                let mut range_start = 0usize;
                {
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        let trimmed = line.trim_end();
                        if let Some(rest) =
                            trimmed.to_ascii_lowercase().strip_prefix("range: bytes=")
                            && let Some(start) = rest.split('-').next()
                        {
                            range_start = start.trim().parse().unwrap_or(0);
                        }
                        if trimmed.is_empty() {
                            break;
                        }
                    }
                }

                if n == 0 {
                    // First attempt: 200 OK, but only send the first half, then
                    // close mid-stream to simulate a stalled/dropped connection.
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\r\n",
                        total
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&payload_for_server[..split]);
                    let _ = stream.flush();
                    // Drop connection without finishing the body.
                } else {
                    // Resumed attempt: serve 206 with the remaining bytes.
                    let remaining = &payload_for_server[range_start..];
                    let header = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
                        remaining.len(),
                        range_start,
                        total - 1,
                        total
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(remaining);
                    let _ = stream.flush();
                }
            }
        });

        let client = reqwest::blocking::Client::builder()
            .build()
            .expect("client");
        let url = format!("http://{}/asset", addr);
        let (bytes, parsed_total) =
            download_asset_with_resume(&client, &url, Some(total as u64), &mut |_| {})
                .expect("download should recover");

        handle.join().ok();

        assert_eq!(bytes, payload, "resumed download must reconstruct payload");
        assert_eq!(parsed_total, Some(total as u64));
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            2,
            "should have made an initial + one resume request"
        );
    }

    /// A connection that keeps dropping but always advances a little must still
    /// complete: forward progress resets the consecutive-stall budget, so the
    /// number of resumes can exceed DOWNLOAD_MAX_ATTEMPTS as long as each one
    /// delivers new bytes.
    #[test]
    fn test_download_asset_with_resume_tolerates_many_progressing_drops() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let payload: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        let total = payload.len();
        // Each attempt delivers only this many bytes, then drops, so it takes
        // many more than DOWNLOAD_MAX_ATTEMPTS attempts to finish.
        let chunk = 100usize;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let payload_for_server = payload.clone();
        let request_count = Arc::new(AtomicUsize::new(0));
        let request_count_server = Arc::clone(&request_count);

        let handle = std::thread::spawn(move || {
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let n = request_count_server.fetch_add(1, Ordering::SeqCst);

                let mut range_start = 0usize;
                {
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        let trimmed = line.trim_end();
                        if let Some(rest) =
                            trimmed.to_ascii_lowercase().strip_prefix("range: bytes=")
                            && let Some(start) = rest.split('-').next()
                        {
                            range_start = start.trim().parse().unwrap_or(0);
                        }
                        if trimmed.is_empty() {
                            break;
                        }
                    }
                }

                let end = (range_start + chunk).min(total);
                let body = &payload_for_server[range_start..end];
                let header = if n == 0 {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\r\n",
                        total
                    )
                } else {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
                        total - range_start,
                        range_start,
                        total - 1,
                        total
                    )
                };
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
                // Drop after a partial chunk unless we've reached the end.
                if end >= total {
                    break;
                }
            }
        });

        let client = reqwest::blocking::Client::builder()
            .build()
            .expect("client");
        let url = format!("http://{}/asset", addr);
        let (bytes, _total) =
            download_asset_with_resume(&client, &url, Some(total as u64), &mut |_| {})
                .expect("progressing download should complete");

        handle.join().ok();

        assert_eq!(bytes, payload);
        assert!(
            request_count.load(Ordering::SeqCst) > DOWNLOAD_MAX_ATTEMPTS,
            "test should require more than the stall budget of resumes"
        );
    }

    /// Mirrors the real #293 shape closely: the caller has *no* size hint
    /// (None), a slow connection drops repeatedly, and the size is learned from
    /// the server (Content-Length on the initial 200, Content-Range on the 206
    /// resumes, exactly like a GitHub release asset). The download must still
    /// complete, learn the correct total, and report progress that never moves
    /// backwards across reconnects (so the UI bar can't appear to regress).
    #[test]
    fn test_download_asset_with_resume_unknown_total_and_monotonic_progress() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let payload: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let total = payload.len();
        let chunk = 400usize;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let payload_for_server = payload.clone();
        let request_count = Arc::new(AtomicUsize::new(0));
        let request_count_server = Arc::clone(&request_count);

        let handle = std::thread::spawn(move || {
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let n = request_count_server.fetch_add(1, Ordering::SeqCst);

                let mut range_start = 0usize;
                {
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        let trimmed = line.trim_end();
                        if let Some(rest) =
                            trimmed.to_ascii_lowercase().strip_prefix("range: bytes=")
                            && let Some(start) = rest.split('-').next()
                        {
                            range_start = start.trim().parse().unwrap_or(0);
                        }
                        if trimmed.is_empty() {
                            break;
                        }
                    }
                }

                let end = (range_start + chunk).min(total);
                let body = &payload_for_server[range_start..end];
                // Like a real GitHub asset: the initial 200 carries the full
                // Content-Length (so a mid-stream drop is detectable), and the
                // 206 resumes carry Content-Range. The *caller* still gets no
                // size hint, so the total must be learned from these headers.
                let header = if n == 0 {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\r\n",
                        total
                    )
                } else {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\n\r\n",
                        total - range_start,
                        range_start,
                        total - 1,
                        total
                    )
                };
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
                let _ = stream.flush();
                // Drop after a partial chunk unless we've reached the end.
                if end >= total {
                    break;
                }
            }
        });

        let client = reqwest::blocking::Client::builder()
            .build()
            .expect("client");
        let url = format!("http://{}/asset", addr);

        let mut progress_points: Vec<u64> = Vec::new();
        let mut seen_total: Option<u64> = None;
        let (bytes, parsed_total) = download_asset_with_resume(
            &client,
            &url,
            None, // no size hint, like a fresh download with no metadata
            &mut |p| {
                progress_points.push(p.downloaded);
                if p.total.is_some() {
                    seen_total = p.total;
                }
            },
        )
        .expect("download with unknown caller hint should complete");

        handle.join().ok();

        assert_eq!(bytes, payload, "payload must be fully reconstructed");
        assert_eq!(
            parsed_total,
            Some(total as u64),
            "total must be learned from the server headers"
        );
        assert_eq!(seen_total, Some(total as u64));
        assert!(
            progress_points.windows(2).all(|w| w[1] >= w[0]),
            "progress must never go backwards across reconnects: {:?}",
            progress_points
        );
        assert_eq!(
            *progress_points.last().expect("at least one progress point"),
            total as u64,
            "final progress must reach the full size"
        );
    }
}

#[cfg(test)]
mod github_auth_tests {
    use super::*;

    #[test]
    #[ignore = "live network test"]
    fn live_fetch_latest_release_uses_auth() {
        let release = fetch_latest_release_blocking().expect("release fetch should succeed");
        assert!(!release.tag_name.is_empty());
    }
}
