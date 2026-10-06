//! Keep the browser bridge (CLI, native host, extension packages) current.
//!
//! Setup used to download the bridge only when a file was missing, so fixes
//! shipped in later bridge releases (such as multi-browser hosts, #1720) never
//! reached existing installs. jcode now records the installed release tag and,
//! at most every `CHECK_INTERVAL`, compares it with the latest GitHub release.
//! A newer release is downloaded in place, running hosts are stopped so each
//! browser's extension reconnects (after ~1.5s) to the new host, and a
//! reload is requested so unpacked Chromium extensions load the new files.
//!
//! Set `JCODE_BROWSER_AUTO_UPDATE=0` to turn the automatic check off; explicit
//! `jcode browser setup` still updates.

use super::*;

/// How often the automatic (non-setup) check may hit the GitHub API.
const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);
/// Upper bound on the release lookup, so a slow network never stalls a tool call.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

fn installed_version_path() -> PathBuf {
    browser_dir().join(".installed-version")
}

fn last_check_path() -> PathBuf {
    browser_dir().join(".last-update-check")
}

/// Release tag of the installed bridge, `None` for installs made before jcode
/// recorded it (those are treated as outdated once).
pub fn installed_bridge_version() -> Option<String> {
    let Ok(raw) = std::fs::read_to_string(installed_version_path()) else {
        return None;
    };
    let tag = raw.trim();
    (!tag.is_empty()).then(|| tag.to_string())
}

pub(super) fn record_installed_version(tag: &str) {
    if let Err(e) = std::fs::write(installed_version_path(), tag) {
        crate::logging::warn(&format!("failed to record browser bridge version: {e}"));
    }
}

/// Parse `v1.2.3` / `1.2.3` into comparable numbers. Pre-release suffixes
/// (`-rc1`) are ignored, which is fine for this repo's plain version tags.
fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let core = tag.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut nums = [0u64; 3];
    for (i, part) in core.split('.').enumerate() {
        let slot = nums.get_mut(i)?;
        let Ok(n) = part.parse::<u64>() else {
            return None;
        };
        *slot = n;
    }
    Some((nums[0], nums[1], nums[2]))
}

/// Whether `latest` should replace `installed`. Unknown installed versions
/// update; an unparsable latest tag never does.
pub(super) fn is_newer_release(installed: Option<&str>, latest: &str) -> bool {
    let Some(latest) = parse_version(latest) else {
        return false;
    };
    match installed.and_then(parse_version) {
        Some(installed) => latest > installed,
        None => true,
    }
}

/// Whether the throttled automatic check is due, given the time of the last
/// check (`None`: never checked or unreadable).
pub(super) fn check_due(
    last_check: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
) -> bool {
    match last_check {
        Some(last) => now
            .duration_since(last)
            .map_or(true, |elapsed| elapsed >= CHECK_INTERVAL),
        None => true,
    }
}

fn last_check_time() -> Option<std::time::SystemTime> {
    // A missing stamp just means no check has run yet.
    std::fs::metadata(last_check_path())
        .and_then(|m| m.modified())
        .ok()
}

fn auto_update_disabled() -> bool {
    matches!(
        std::env::var("JCODE_BROWSER_AUTO_UPDATE").as_deref(),
        Ok("0") | Ok("false") | Ok("off") | Ok("no")
    )
}

/// Fetch the latest bridge release JSON.
pub(super) async fn fetch_latest_release() -> Result<serde_json::Value> {
    let client = jcode_provider_core::shared_http_client();
    let mut request = client
        .get(GITHUB_API_LATEST)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    // Avoid the shared unauthenticated 60 req/h per-IP GitHub bucket when a
    // token is available (see crate::github).
    if let Some(token) = crate::github::github_public_api_token() {
        request = request.bearer_auth(token);
    }
    request
        .send()
        .await?
        .error_for_status()
        .context("GitHub rejected the bridge release lookup")?
        .json()
        .await
        .context("Failed to fetch latest release info")
}

/// Outcome of an update check, for setup logs and tool notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeUpdate {
    /// Installed bridge is the latest release.
    Current,
    /// Downloaded `to` (previously `from`, unknown for older installs).
    Updated { from: Option<String>, to: String },
    /// Check skipped (throttled, disabled, or bridge not installed).
    Skipped,
}

impl BridgeUpdate {
    pub fn describe(&self) -> Option<String> {
        match self {
            BridgeUpdate::Updated {
                from: Some(from),
                to,
            } => Some(format!("Updated the browser bridge from {from} to {to}.")),
            BridgeUpdate::Updated { from: None, to } => {
                Some(format!("Updated the browser bridge to {to}."))
            }
            _ => None,
        }
    }
}

/// Update the installed bridge when a newer release exists.
///
/// `force` (setup) skips the throttle and the opt-out. Only installed bridges
/// are updated; first-time installs stay an explicit setup step.
pub async fn update_bridge_if_newer(kind: BrowserKind, force: bool) -> Result<BridgeUpdate> {
    if !browser_binary_path().exists() || !host_binary_path().exists() {
        return Ok(BridgeUpdate::Skipped);
    }
    if !force
        && (auto_update_disabled() || !check_due(last_check_time(), std::time::SystemTime::now()))
    {
        return Ok(BridgeUpdate::Skipped);
    }
    // Stamp before the network call so a failing or slow GitHub is retried at
    // the next interval instead of on every browser action.
    if let Err(e) = std::fs::create_dir_all(browser_dir())
        .and_then(|()| std::fs::write(last_check_path(), chrono::Utc::now().to_rfc3339()))
    {
        crate::logging::warn(&format!("failed to stamp browser bridge update check: {e}"));
    }

    let release = tokio::time::timeout(CHECK_TIMEOUT, fetch_latest_release())
        .await
        .context("Timed out checking for a browser bridge update")??;
    let Some(latest) = release["tag_name"].as_str().map(str::to_string) else {
        anyhow::bail!("Latest bridge release has no tag");
    };
    let installed = installed_bridge_version();
    if !is_newer_release(installed.as_deref(), &latest) {
        return Ok(BridgeUpdate::Current);
    }

    download_bridge_release(&release, kind).await?;
    restart_running_hosts().await;
    Ok(BridgeUpdate::Updated {
        from: installed,
        to: latest,
    })
}

/// Make browsers pick up a freshly downloaded bridge. Asking the extension to
/// reload first loads new unpacked extension files (Chromium) and drops its
/// native-messaging port; stopping the old hosts covers extensions too old to
/// know `reload`. Either way the extension reconnects and the browser spawns
/// the new host binary.
async fn restart_running_hosts() {
    let bin = browser_binary_path();
    if let Ok(Some(output)) =
        run_browser_cli_capped(&bin, &["reload"], std::time::Duration::from_secs(5), None).await
        && output.status.success()
    {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    stop_hosts_running(&host_binary_path());
}

/// Terminate native host processes started from `host`. The browser restarts
/// a host on the extension's next connect attempt.
fn stop_hosts_running(host: &std::path::Path) {
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten() {
            let Some(Ok(pid)) = entry.file_name().to_str().map(str::parse::<i32>) else {
                continue;
            };
            let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else {
                continue;
            };
            // The binary was just replaced, so the kernel reports the old
            // inode as "<path> (deleted)".
            let exe = exe.to_string_lossy();
            let exe = exe.trim_end_matches(" (deleted)");
            if std::path::Path::new(exe) == host {
                // SAFETY: plain signal to a process we matched by executable.
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("pkill")
            .arg("-f")
            .arg(host)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Err(e) = status {
            crate::logging::warn(&format!("failed to stop old bridge hosts: {e}"));
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // Windows locks running executables, so the replaced host only takes
        // effect after the browser restarts it.
        let _windows_keeps_old_host = host;
    }
}

/// Throttled update before a browser action. Returns a note for the agent
/// when an update was installed, after giving the extension time to reconnect.
pub async fn auto_update_before_action(kind: BrowserKind) -> Option<String> {
    match update_bridge_if_newer(kind, false).await {
        Ok(update) => {
            let note = update.describe()?;
            wait_for_bridge(kind, 10).await;
            Some(note)
        }
        Err(e) => {
            crate::logging::warn(&format!("browser bridge update check failed: {e}"));
            None
        }
    }
}
