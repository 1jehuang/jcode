use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::browser_detect::{self, BrowserDetection, BrowserFamily, BrowserKind};
use crate::{platform, storage};

const GITHUB_API_LATEST: &str =
    "https://api.github.com/repos/1jehuang/firefox-agent-bridge/releases/latest";

const NATIVE_HOST_NAME: &str = "firefox_agent_bridge";
const EXTENSION_ID_LISTED: &str = "browser-agent-bridge@1jehuang.github.io";
const EXTENSION_ID_LOCAL: &str = "firefox-agent-bridge@local";
/// Stable ID of the unpacked Chromium extension (derived from the public key
/// embedded in its manifest by the bridge's build-extensions.py).
pub const CHROMIUM_EXTENSION_ID: &str = "ijifgeepmnbalajhfjnpbnobfobflfkk";
/// TCP port of the native host's agent-facing WebSocket server.
const BRIDGE_WS_PORT: u16 = 8766;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserStatus {
    pub backend: &'static str,
    /// Browser jcode targets for setup and launch (see `browser_detect`).
    pub browser: &'static str,
    /// Why `browser` was chosen, e.g. "your default browser".
    pub detected_via: &'static str,
    /// Browser that actually answered the bridge ping, when one did.
    pub connected_browser: Option<String>,
    pub setup_complete: bool,
    pub binary_installed: bool,
    pub responding: bool,
    pub compatible: bool,
    pub missing_actions: Vec<String>,
    pub ready: bool,
}

const REQUIRED_BRIDGE_ACTION_PROBES: &[(&str, &str)] = &[
    ("evaluate", r#"{"script":"return 1"}"#),
    ("listFrames", "{}"),
    ("scroll", r#"{"position":"top"}"#),
    (
        "uploadFile",
        r#"{"selector":"input[type=file]","filePath":"/tmp/jcode-browser-capability-probe"}"#,
    ),
];

fn jcode_dir() -> PathBuf {
    storage::jcode_dir().unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".jcode")
    })
}

fn browser_dir() -> PathBuf {
    jcode_dir().join("browser")
}

pub fn browser_binary_path() -> PathBuf {
    let dir = browser_dir();
    #[cfg(windows)]
    {
        dir.join("browser.exe")
    }
    #[cfg(not(windows))]
    {
        dir.join("browser")
    }
}

fn host_binary_path() -> PathBuf {
    let dir = browser_dir();
    #[cfg(windows)]
    {
        dir.join("firefox-agent-bridge-host.exe")
    }
    #[cfg(not(windows))]
    {
        dir.join("firefox-agent-bridge-host")
    }
}

fn xpi_path() -> PathBuf {
    browser_dir().join("browser-agent-bridge.xpi")
}

fn setup_marker_path() -> PathBuf {
    browser_dir().join(".setup-complete")
}

/// Records which browser `jcode browser setup` last configured, so detection
/// stays stable even if the system default browser changes later.
fn browser_preference_path() -> PathBuf {
    browser_dir().join(".browser")
}

fn extensions_dir() -> PathBuf {
    browser_dir().join("extensions")
}

/// Unpacked Chromium extension, loaded via "Load unpacked" in the browser.
pub fn chromium_extension_dir() -> PathBuf {
    extensions_dir().join("chromium")
}

fn safari_extension_dir() -> PathBuf {
    extensions_dir().join("safari")
}

#[cfg(target_os = "macos")]
fn safari_app_project_dir() -> PathBuf {
    browser_dir().join("safari-app")
}

pub fn saved_browser_preference() -> Option<BrowserKind> {
    std::fs::read_to_string(browser_preference_path())
        .ok()
        .and_then(|s| BrowserKind::parse(&s))
}

fn save_browser_preference(kind: BrowserKind) {
    let _ = std::fs::create_dir_all(browser_dir());
    let _ = std::fs::write(browser_preference_path(), kind.id());
}

/// Which browser jcode should set up and launch.
pub fn detect_target_browser() -> BrowserDetection {
    let env = std::env::var("JCODE_BROWSER").ok();
    let env = env
        .as_deref()
        .filter(|v| !v.trim().is_empty() && *v != "auto");
    let installed: Vec<BrowserKind> = browser_detect::ALL_BROWSERS
        .iter()
        .copied()
        .filter(|k| k.is_installed())
        .collect();
    browser_detect::resolve_detection(
        env,
        saved_browser_preference(),
        browser_detect::system_default_browser_id(),
        &installed,
    )
}

/// Resolve an explicit browser request ("auto" or None means detect).
pub fn resolve_target_browser(requested: Option<&str>) -> Result<BrowserDetection> {
    match requested
        .map(str::trim)
        .filter(|r| !r.is_empty() && *r != "auto")
    {
        None => Ok(detect_target_browser()),
        Some(name) => {
            let kind = BrowserKind::parse(name).with_context(|| {
                format!(
                    "Unknown browser '{}'. Supported: auto, firefox, chrome, chromium, edge, brave, safari.",
                    name
                )
            })?;
            if !kind.supported_on_this_os() {
                anyhow::bail!(
                    "{} is not available on this operating system.",
                    kind.display_name()
                );
            }
            Ok(BrowserDetection {
                kind,
                source: browser_detect::DetectionSource::Requested,
                system_default: None,
            })
        }
    }
}

fn runtime_dir() -> PathBuf {
    storage::runtime_dir()
}

fn session_socket_path(name: &str) -> PathBuf {
    runtime_dir().join(format!("browser-session-{}.sock", name))
}

fn session_pid_path(name: &str) -> PathBuf {
    runtime_dir().join(format!("browser-session-{}.pid", name))
}

fn is_session_alive(name: &str) -> bool {
    let pid_path = session_pid_path(name);
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path)
        && let Ok(pid) = pid_str.trim().parse::<u32>()
        && platform::is_process_running(pid)
    {
        return session_socket_path(name).exists();
    }
    false
}

pub fn ensure_browser_session(session_id: &str) -> Option<String> {
    let session_name = sanitize_session_name(session_id);

    if is_session_alive(&session_name) {
        return Some(session_name);
    }

    let bin = browser_binary_path();
    if !bin.exists() {
        return None;
    }

    // Bind each agent session to a dedicated browser window when the installed
    // bridge supports it. Older bridge CLIs reject --bind-window, so probe the
    // command surface instead of paying for a known-failing process launch on
    // every browser action.
    if browser_supports_bind_window(&bin)
        && let Some(name) = spawn_browser_session(&bin, &session_name, true)
    {
        return Some(name);
    }
    spawn_browser_session(&bin, &session_name, false)
}

fn browser_supports_bind_window(bin: &std::path::Path) -> bool {
    std::process::Command::new(bin)
        .args(["session", "start", "--help"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout).contains("--bind-window")
                || String::from_utf8_lossy(&output.stderr).contains("--bind-window")
        })
}

fn spawn_browser_session(
    bin: &std::path::Path,
    session_name: &str,
    bind_window: bool,
) -> Option<String> {
    let mut args = vec!["session", "start", session_name];
    if bind_window {
        args.push("--bind-window");
    }
    let result = std::process::Command::new(bin)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();

    match result {
        Ok(mut child) => {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                if session_socket_path(session_name).exists() && is_session_alive(session_name) {
                    let _ = child.stdout.take();
                    return Some(session_name.to_string());
                }
                if let Ok(Some(status)) = child.try_wait() {
                    eprintln!(
                        "[browser] session '{}' exited before startup with status {}{}",
                        session_name,
                        status,
                        if bind_window {
                            " (retrying without --bind-window)"
                        } else {
                            ""
                        }
                    );
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            eprintln!(
                "[browser] session '{}' did not start within 10s",
                session_name
            );
            let _ = child.kill();
            let _ = child.wait();
            None
        }
        Err(e) => {
            eprintln!(
                "[browser] Failed to start browser session '{}': {}",
                session_name, e
            );
            None
        }
    }
}

fn sanitize_session_name(session_id: &str) -> String {
    session_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect()
}

pub fn is_browser_command(command: &str) -> bool {
    let trimmed = command.trim_start();
    trimmed.starts_with("browser ") || trimmed == "browser" || trimmed.starts_with("browser\t")
}

pub fn is_setup_complete() -> bool {
    setup_marker_path().exists() && browser_binary_path().exists() && host_binary_path().exists()
}

fn mark_setup_complete() -> Result<()> {
    let marker = setup_marker_path();
    std::fs::write(&marker, chrono::Utc::now().to_rfc3339())?;
    Ok(())
}

fn mark_setup_complete_for(kind: BrowserKind) -> Result<()> {
    save_browser_preference(kind);
    mark_setup_complete()
}

pub fn rewrite_command_with_full_path(command: &str) -> String {
    let bin = browser_binary_path();
    if !bin.exists() {
        return command.to_string();
    }
    let trimmed = command.trim_start();
    if trimmed == "browser" {
        bin.to_string_lossy().to_string()
    } else if let Some(rest) = trimmed.strip_prefix("browser ") {
        format!("{} {}", bin.to_string_lossy(), rest)
    } else if let Some(rest) = trimmed.strip_prefix("browser\t") {
        format!("{} {}", bin.to_string_lossy(), rest)
    } else {
        command.to_string()
    }
}

pub async fn ensure_browser_setup() -> Result<String> {
    ensure_browser_setup_for(detect_target_browser()).await
}

pub async fn ensure_browser_setup_for(target: BrowserDetection) -> Result<String> {
    let kind = target.kind;
    let name = kind.display_name();
    let mut log = String::new();

    std::fs::create_dir_all(browser_dir())?;
    log.push_str(&format!(
        "Target browser: {} ({}).\n",
        name,
        target.source.describe()
    ));
    if target.source != browser_detect::DetectionSource::Requested
        && target.source != browser_detect::DetectionSource::EnvOverride
    {
        log.push_str(
            "Override with `jcode browser setup <firefox|chrome|edge|brave|chromium|safari>` or JCODE_BROWSER.\n",
        );
    }

    let initial_status = inspect_browser_status_for(&target).await?;
    if initial_status.ready {
        if connected_matches(&initial_status, kind) {
            mark_setup_complete_for(kind).ok();
            log.push_str("Browser bridge is already set up and responding.\n");
            log.push_str("No setup action was needed.\n");
            return Ok(log);
        }
        log.push_str(&format!(
            "The bridge is currently answered by {}, not {}. Continuing setup for {} (only one browser can own the bridge at a time; close the other browser to switch).\n",
            initial_status.connected_browser.as_deref().unwrap_or("another browser"),
            name,
            name
        ));
    }

    // A silent bridge with installed binaries usually just means the browser is
    // closed. That is not an install problem, so launch it and re-check
    // before running any repair or reopening the extension installer.
    let initial_status = match try_launch_browser_for_bridge_with(&initial_status, kind).await? {
        Some(refreshed) if refreshed.ready => {
            log.push_str(&format!(
                "{} was not running, so it was launched and the browser bridge reconnected.\n",
                name
            ));
            log.push_str("No setup action was needed.\n");
            mark_setup_complete_for(kind).ok();
            return Ok(log);
        }
        Some(refreshed) => {
            log.push_str(&format!(
                "{} was not running; launched it, but the bridge did not reconnect yet. Continuing with setup checks...\n",
                name
            ));
            refreshed
        }
        None => initial_status,
    };

    let outdated = initial_status.responding && !initial_status.compatible;
    if outdated {
        log.push_str(&format!(
            "Browser bridge is connected, but the live {} extension is out of date for this jcode build. Attempting repair steps...\n",
            name
        ));
        if !initial_status.missing_actions.is_empty() {
            log.push_str(&format!(
                "Missing actions: {}\n",
                initial_status.missing_actions.join(", ")
            ));
        }
    } else if initial_status.binary_installed {
        log.push_str(
            "Browser bridge is installed but not fully ready. Attempting repair steps...\n",
        );
    } else {
        log.push_str("Browser bridge is not installed yet. Starting setup...\n");
    }

    // Step 1: Check/download browser bridge assets
    if !browser_binary_path().exists()
        || !host_binary_path().exists()
        || !extension_package_present(kind)
        || outdated
    {
        log.push_str("[1/3] Downloading browser bridge assets... ");
        match download_browser_binary_for(kind).await {
            Ok(()) => log.push_str("done\n"),
            Err(e) => {
                log.push_str(&format!("failed: {}\n", e));
                return Ok(log);
            }
        }
    } else {
        log.push_str("[1/3] Browser CLI... already installed\n");
    }

    // Step 2: Native messaging host (or relay host for Safari)
    if kind.family() == BrowserFamily::Safari {
        log.push_str("[2/3] Bridge relay host... ");
        match ensure_relay_host_running() {
            Ok(true) => log.push_str("started\n"),
            Ok(false) => log.push_str("already running\n"),
            Err(e) => log.push_str(&format!("failed: {}\n", e)),
        }
    } else {
        log.push_str("[2/3] Native messaging host... ");
        match install_native_host_manifest_for(kind) {
            Ok(true) => log.push_str("installed\n"),
            Ok(false) => log.push_str("already configured\n"),
            Err(e) => {
                log.push_str(&format!("failed: {}\n", e));
                log.push_str("       You may need to run setup manually.\n");
            }
        }
    }

    // Step 3: Check extension connectivity
    log.push_str(&format!("[3/3] Checking {} extension... ", name));
    let connected = bridge_ping_info()
        .await
        .ok()
        .flatten()
        .is_some_and(|info| ping_matches(&info, kind));
    if connected && !outdated {
        log.push_str("connected!\n");
        mark_setup_complete_for(kind).ok();
    } else {
        if connected {
            log.push_str("connected, but out of date\n");
        } else {
            log.push_str("not connected\n");
        }
        if outdated
            || should_prompt_extension_install(&initial_status)
            || !connected_matches(&initial_status, kind)
        {
            match install_extension_for(kind).await {
                Ok(msg) => {
                    log.push_str(&msg);
                    log.push_str("       Waiting for the extension to connect... ");
                    let wait_secs = if kind.family() == BrowserFamily::Gecko {
                        15
                    } else {
                        90
                    };
                    match wait_for_ready_for(&target, wait_secs).await {
                        Ok(true) => {
                            log.push_str("ready!\n");
                            mark_setup_complete_for(kind).ok();
                        }
                        Ok(false) => {
                            log.push_str("timed out\n");
                            log.push_str(&format!(
                                "       Finish the steps above, then re-run `jcode browser setup {}`.\n",
                                kind.id()
                            ));
                        }
                        Err(e) => log.push_str(&format!("error: {}\n", e)),
                    }
                }
                Err(e) => {
                    log.push_str(&format!(
                        "       Could not install the extension automatically: {}\n",
                        e
                    ));
                    log.push_str(&manual_install_hint(kind));
                }
            }
        } else {
            log.push_str(
                "       Existing browser setup was already completed, so setup will not reopen the extension installer.\n",
            );
            log.push_str(&format!(
                "       Make sure {} is running with the Browser Agent Bridge extension enabled, then re-run `jcode browser status`.\n",
                name
            ));
        }
    }

    let final_status = inspect_browser_status_for(&target).await?;
    if final_status.ready {
        log.push_str("\nSetup complete. Browser bridge is ready.\n");
    } else if final_status.responding && !final_status.compatible {
        log.push_str(&format!("\nSetup is not complete yet. The {} extension is connected, but it is still missing required actions for this jcode build.\n", name));
        if !final_status.missing_actions.is_empty() {
            log.push_str(&format!(
                "Missing actions: {}\n",
                final_status.missing_actions.join(", ")
            ));
        }
        log.push_str(&format!(
            "Use `jcode browser status` to verify readiness after updating the extension in {}.\n",
            name
        ));
    } else if final_status.binary_installed {
        log.push_str(&format!("\nSetup is not complete yet. Browser bridge binaries are installed, but the {} extension/bridge is not responding.\n", name));
        log.push_str(
            "Use `jcode browser status` to re-check readiness after any manual browser step.\n",
        );
    } else {
        log.push_str("\nSetup is not complete yet. Browser bridge binary is still missing.\n");
    }

    Ok(log)
}

fn manual_install_hint(kind: BrowserKind) -> String {
    match kind.family() {
        BrowserFamily::Gecko => format!(
            "       Manually install: Firefox > about:addons > Install from file > {}\n",
            xpi_path().display()
        ),
        BrowserFamily::Chromium => format!(
            "       Manually install: open {} > enable Developer mode > Load unpacked > {}\n",
            kind.extensions_page(),
            chromium_extension_dir().display()
        ),
        BrowserFamily::Safari => format!(
            "       Manually install: on a Mac with Xcode, convert {} with `xcrun safari-web-extension-converter`, build and open the app, then enable it in Safari > Settings > Extensions.\n",
            safari_extension_dir().display()
        ),
    }
}

fn extension_package_present(kind: BrowserKind) -> bool {
    match kind.family() {
        BrowserFamily::Gecko => xpi_path().exists(),
        BrowserFamily::Chromium => chromium_extension_dir().join("manifest.json").exists(),
        BrowserFamily::Safari => safari_extension_dir().join("manifest.json").exists(),
    }
}

/// Whether a bridge ping came from the requested browser family. Chromium
/// browsers are interchangeable here because they share one extension build.
fn ping_matches(info: &serde_json::Value, kind: BrowserKind) -> bool {
    match info.get("browser").and_then(|b| b.as_str()) {
        // Older extensions do not report a browser and are Firefox-only.
        None => kind.family() == BrowserFamily::Gecko,
        Some(reported) => {
            BrowserKind::parse(reported).is_some_and(|r| r == kind || r.family() == kind.family())
        }
    }
}

fn connected_matches(status: &BrowserStatus, kind: BrowserKind) -> bool {
    match status.connected_browser.as_deref() {
        None => kind.family() == BrowserFamily::Gecko,
        Some(reported) => {
            BrowserKind::parse(reported).is_some_and(|r| r == kind || r.family() == kind.family())
        }
    }
}

async fn download_browser_binary_for(kind: BrowserKind) -> Result<()> {
    let asset_name = get_platform_asset_name();
    let client = jcode_provider_core::shared_http_client();

    let mut request = client
        .get(GITHUB_API_LATEST)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    // Avoid the shared unauthenticated 60 req/h per-IP GitHub bucket when a
    // token is available (see crate::github).
    if let Some(token) = crate::github::github_public_api_token() {
        request = request.bearer_auth(token);
    }
    let release_info: serde_json::Value = request
        .send()
        .await?
        .json()
        .await
        .context("Failed to fetch latest release info")?;

    let assets = release_info["assets"]
        .as_array()
        .context("No assets in release")?;

    // Find the browser CLI binary
    let browser_asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(&asset_name))
        .context(format!("No asset found for platform: {}", asset_name))?;

    let download_url = browser_asset["browser_download_url"]
        .as_str()
        .context("No download URL")?;

    let find_asset = |pred: &dyn Fn(&str) -> bool| {
        assets
            .iter()
            .find(|a| a["name"].as_str().is_some_and(pred))
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_string)
    };
    let xpi_url = find_asset(&|n| n.ends_with(".xpi"));
    let chromium_url =
        find_asset(&|n| n.starts_with("browser-agent-bridge-chrome") && n.ends_with(".zip"));
    let safari_url =
        find_asset(&|n| n.starts_with("browser-agent-bridge-safari") && n.ends_with(".zip"));
    let needed = match kind.family() {
        BrowserFamily::Gecko => xpi_url
            .as_ref()
            .map(|_| ())
            .context("No XPI asset found in release"),
        BrowserFamily::Chromium => chromium_url
            .as_ref()
            .map(|_| ())
            .context("No Chromium extension package found in the latest bridge release"),
        BrowserFamily::Safari => safari_url
            .as_ref()
            .map(|_| ())
            .context("No Safari extension package found in the latest bridge release"),
    };
    needed?;

    // Find the host binary
    let host_asset_name = get_host_asset_name();
    let host_asset = assets
        .iter()
        .find(|a| a["name"].as_str() == Some(&host_asset_name))
        .with_context(|| {
            let available = assets
                .iter()
                .filter_map(|a| a["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "No native host asset found for platform: {}. Expected release asset '{}' alongside '{}'. Available assets: {}",
                std::env::consts::OS,
                host_asset_name,
                asset_name,
                available
            )
        })?;

    // Download browser CLI
    let browser_bytes = client
        .get(download_url)
        .send()
        .await?
        .bytes()
        .await
        .context("Failed to download browser binary")?;

    let bin_path = browser_binary_path();
    write_file_atomically(&bin_path, &browser_bytes, true)?;

    // Download the extension package(s). The XPI is always fetched when
    // available so switching back to Firefox needs no extra download.
    if let Some(url) = &xpi_url {
        let bytes = client
            .get(url)
            .send()
            .await?
            .bytes()
            .await
            .context("Failed to download XPI")?;
        write_file_atomically(&xpi_path(), &bytes, false)?;
    }
    match kind.family() {
        BrowserFamily::Chromium => {
            let url = chromium_url
                .as_deref()
                .context("No Chromium extension package")?;
            let bytes = client
                .get(url)
                .send()
                .await?
                .bytes()
                .await
                .context("Failed to download Chromium extension")?;
            replace_dir_with_zip(&chromium_extension_dir(), &bytes)?;
        }
        BrowserFamily::Safari => {
            let url = safari_url
                .as_deref()
                .context("No Safari extension package")?;
            let bytes = client
                .get(url)
                .send()
                .await?
                .bytes()
                .await
                .context("Failed to download Safari extension")?;
            replace_dir_with_zip(&safari_extension_dir(), &bytes)?;
        }
        BrowserFamily::Gecko => {}
    }

    // Download host binary
    let host_url = host_asset["browser_download_url"]
        .as_str()
        .context("No host download URL")?;
    let host_bytes = client
        .get(host_url)
        .send()
        .await?
        .bytes()
        .await
        .context("Failed to download host binary")?;

    let host_path = host_binary_path();
    write_file_atomically(&host_path, &host_bytes, true)?;

    Ok(())
}

fn write_file_atomically(path: &PathBuf, bytes: &[u8], _executable: bool) -> Result<()> {
    let parent = path
        .parent()
        .context("Target file has no parent directory")?;
    std::fs::create_dir_all(parent)?;

    let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let pid = std::process::id();
    let tmp_path = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("download"),
        pid,
        ts
    ));

    std::fs::write(&tmp_path, bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if _executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode))?;
    }

    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Extract `bytes` (a zip archive) into `dir`, replacing its previous
/// contents. The directory path stays stable so a browser that loaded the
/// unpacked extension from it picks up the update on reload.
fn replace_dir_with_zip(dir: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = dir.parent().context("extension dir has no parent")?;
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(
        ".{}.staging-{}",
        dir.file_name().and_then(|n| n.to_str()).unwrap_or("ext"),
        std::process::id()
    ));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    extract_zip(bytes, &staging)?;
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::rename(&staging, dir)?;
    Ok(())
}

/// Minimal zip reader (stored and deflate entries) using the central
/// directory. Rejects absolute paths and `..` components.
pub(crate) fn extract_zip(bytes: &[u8], dest: &std::path::Path) -> Result<()> {
    use std::io::Read;
    let u16_at = |o: usize| -> Result<usize> {
        bytes
            .get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            .context("truncated zip")
    };
    let u32_at = |o: usize| -> Result<usize> {
        bytes
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .context("truncated zip")
    };
    let eocd = (0..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| bytes[i..].starts_with(&[0x50, 0x4b, 0x05, 0x06]))
        .context("not a zip archive")?;
    let entries = u16_at(eocd + 10)?;
    let mut offset = u32_at(eocd + 16)?;
    std::fs::create_dir_all(dest)?;
    for _ in 0..entries {
        anyhow::ensure!(
            bytes.get(offset..offset + 4) == Some(&[0x50, 0x4b, 0x01, 0x02][..]),
            "corrupt zip central directory"
        );
        let method = u16_at(offset + 10)?;
        let compressed = u32_at(offset + 20)?;
        let name_len = u16_at(offset + 28)?;
        let extra_len = u16_at(offset + 30)?;
        let comment_len = u16_at(offset + 32)?;
        let local = u32_at(offset + 42)?;
        let name_bytes = bytes
            .get(offset + 46..offset + 46 + name_len)
            .context("truncated zip")?;
        let name = String::from_utf8_lossy(name_bytes).replace('\\', "/");
        offset += 46 + name_len + extra_len + comment_len;

        let rel = std::path::Path::new(&name);
        anyhow::ensure!(
            !rel.is_absolute()
                && rel
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            "unsafe path in zip: {}",
            name
        );
        let out = dest.join(rel);
        if name.ends_with('/') {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        let data_start = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        let data = bytes
            .get(data_start..data_start + compressed)
            .context("truncated zip entry")?;
        let contents = match method {
            0 => data.to_vec(),
            8 => {
                let mut buf = Vec::new();
                flate2::read::DeflateDecoder::new(data).read_to_end(&mut buf)?;
                buf
            }
            other => anyhow::bail!("unsupported zip compression method {}", other),
        };
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&out, contents)?;
    }
    Ok(())
}

fn get_platform_asset_name() -> String {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "browser-linux-x64".to_string()
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "browser-linux-arm64".to_string()
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "browser-macos-arm64".to_string()
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "browser-macos-x64".to_string()
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "browser-windows-x64.exe".to_string()
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    {
        format!(
            "browser-{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    }
}

fn get_host_asset_name() -> String {
    let base = get_platform_asset_name();
    base.replace("browser-", "host-")
}

fn native_host_manifest_json(kind: BrowserKind, host_path: &str) -> serde_json::Value {
    let mut manifest = serde_json::json!({
        "name": NATIVE_HOST_NAME,
        "description": "Native host for Browser Agent Bridge (managed by jcode)",
        "path": host_path,
        "type": "stdio",
    });
    if kind.family() == BrowserFamily::Gecko {
        manifest["allowed_extensions"] =
            serde_json::json!([EXTENSION_ID_LOCAL, EXTENSION_ID_LISTED]);
    } else {
        manifest["allowed_origins"] =
            serde_json::json!([format!("chrome-extension://{}/", CHROMIUM_EXTENSION_ID)]);
    }
    manifest
}

/// Whether an existing manifest already points at a live host and allows the
/// extension this browser uses.
fn native_host_manifest_is_valid(kind: BrowserKind, existing: &serde_json::Value) -> bool {
    let host_ok = existing["path"]
        .as_str()
        .is_some_and(|p| std::path::Path::new(p).exists());
    let allowed_ok = if kind.family() == BrowserFamily::Gecko {
        existing["allowed_extensions"]
            .as_array()
            .is_some_and(|ids| {
                ids.iter()
                    .any(|id| id.as_str() == Some(EXTENSION_ID_LISTED))
            })
    } else {
        let origin = format!("chrome-extension://{}/", CHROMIUM_EXTENSION_ID);
        existing["allowed_origins"]
            .as_array()
            .is_some_and(|o| o.iter().any(|v| v.as_str() == Some(origin.as_str())))
    };
    host_ok && allowed_ok
}

fn install_native_host_manifest_for(kind: BrowserKind) -> Result<bool> {
    let dirs = native_messaging_hosts_dirs_for(kind)?;
    let host_path = host_binary_path();
    if !host_path.exists() {
        return Err(anyhow::anyhow!(
            "Host binary not found at {}. The native messaging host is required for the {} extension to communicate with the bridge.",
            host_path.display(),
            kind.display_name()
        ));
    }
    let manifest = native_host_manifest_json(kind, &host_path.to_string_lossy());
    let mut wrote_any = false;
    for manifest_dir in dirs {
        let manifest_path = manifest_dir.join(format!("{}.json", NATIVE_HOST_NAME));
        let valid = std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .is_some_and(|existing| native_host_manifest_is_valid(kind, &existing));
        if !valid {
            std::fs::create_dir_all(&manifest_dir)?;
            std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
            wrote_any = true;
        }
        #[cfg(target_os = "windows")]
        register_windows_native_host_manifest(kind, &manifest_path)?;
    }
    Ok(wrote_any)
}

#[cfg(target_os = "windows")]
fn register_windows_native_host_manifest(
    kind: BrowserKind,
    manifest_path: &std::path::Path,
) -> Result<()> {
    for root in kind.windows_native_host_registry_roots() {
        let key = format!(r"{}\{}", root, NATIVE_HOST_NAME);
        let output = std::process::Command::new("reg")
            .args([
                "add",
                &key,
                "/ve",
                "/t",
                "REG_SZ",
                "/d",
                &manifest_path.to_string_lossy(),
                "/f",
            ])
            .output()
            .context("Failed to register the native messaging host in the Windows registry")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let details = if stderr.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            anyhow::bail!(
                "Failed to register the {} native messaging host in the Windows registry: {}",
                kind.display_name(),
                details
            );
        }
    }
    Ok(())
}

fn native_messaging_hosts_dirs_for(kind: BrowserKind) -> Result<Vec<PathBuf>> {
    let dirs = kind.native_messaging_dirs();
    if dirs.is_empty() {
        anyhow::bail!(
            "{} does not use native messaging on this platform",
            kind.display_name()
        );
    }
    Ok(dirs)
}

/// Whether something is listening on the bridge's agent WebSocket port.
fn bridge_port_open() -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], BRIDGE_WS_PORT)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

/// Safari cannot launch native messaging hosts, so jcode runs the host in
/// relay mode and the Safari extension dials it. Returns whether a new host
/// was started.
pub fn ensure_relay_host_running() -> Result<bool> {
    if bridge_port_open() {
        return Ok(false);
    }
    let host = host_binary_path();
    anyhow::ensure!(host.exists(), "Host binary not found at {}", host.display());
    let mut cmd = std::process::Command::new(&host);
    cmd.arg("--relay")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let child = platform::spawn_detached(&mut cmd).context("Failed to start relay host")?;
    platform::reap_detached(child);
    Ok(true)
}

/// How long to wait for the browser CLI before declaring the bridge dead.
///
/// The CLI round-trips to the Firefox extension over `ws://127.0.0.1:8766`. If
/// the extension is missing, disabled, or Firefox is closed, nothing ever
/// answers and an unbounded `.output().await` hangs `browser status` and
/// `browser setup` for minutes. See #602.
const BRIDGE_PING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run the browser CLI with a hard timeout, killing the child if it overruns.
///
/// `Ok(None)` means the call timed out, which callers treat as "not
/// responding" so they fail fast instead of hanging.
async fn run_browser_cli_capped(
    bin: &std::path::Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<Option<std::process::Output>> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args).kill_on_drop(true);

    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(output) => Ok(Some(output?)),
        Err(_) => {
            crate::logging::warn(&format!(
                "browser CLI '{}' timed out after {}s; treating the bridge as not responding",
                args.first().copied().unwrap_or("(no action)"),
                timeout.as_secs()
            ));
            Ok(None)
        }
    }
}

async fn check_browser_ping() -> Result<bool> {
    Ok(bridge_ping_info().await?.is_some())
}

/// Ping the bridge and return the extension's reply (which names the browser
/// and transport on bridge v0.10+), or `None` if nothing answered.
async fn bridge_ping_info() -> Result<Option<serde_json::Value>> {
    let bin = browser_binary_path();
    if !bin.exists() {
        return Ok(None);
    }

    match run_browser_cli_capped(&bin, &["ping"], BRIDGE_PING_TIMEOUT).await? {
        Some(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.contains("pong") {
                return Ok(None);
            }
            Ok(Some(
                serde_json::from_str(stdout.trim())
                    .unwrap_or_else(|_| serde_json::json!({"pong": true})),
            ))
        }
        _ => Ok(None),
    }
}

async fn probe_bridge_action_support(action: &str, params_json: &str) -> Result<bool> {
    let bin = browser_binary_path();
    if !bin.exists() {
        return Ok(false);
    }

    let Some(output) =
        run_browser_cli_capped(&bin, &[action, params_json], BRIDGE_PING_TIMEOUT).await?
    else {
        // A dead bridge cannot tell us whether the action exists; report it as
        // unsupported rather than hanging the caller (#602).
        return Ok(false);
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.trim().is_empty() {
        stdout.trim().to_string()
    } else if stdout.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        format!("{}\n{}", stderr.trim(), stdout.trim())
    };

    Ok(!combined.contains(&format!("Unknown action: {}", action)))
}

async fn probe_bridge_missing_actions() -> Result<Vec<String>> {
    let mut missing = Vec::new();
    for (action, params_json) in REQUIRED_BRIDGE_ACTION_PROBES {
        if !probe_bridge_action_support(action, params_json).await? {
            missing.push((*action).to_string());
        }
    }
    Ok(missing)
}

pub async fn inspect_browser_status() -> Result<BrowserStatus> {
    inspect_browser_status_for(&detect_target_browser()).await
}

pub async fn inspect_browser_status_for(target: &BrowserDetection) -> Result<BrowserStatus> {
    let binary_installed = browser_binary_path().exists();
    let setup_complete = is_setup_complete();
    let ping = if binary_installed {
        bridge_ping_info().await.unwrap_or(None)
    } else {
        None
    };
    let responding = ping.is_some();
    let connected_browser = ping.as_ref().map(|info| {
        info.get("browser")
            .and_then(|b| b.as_str())
            .unwrap_or("firefox")
            .to_string()
    });
    let missing_actions = if responding {
        probe_bridge_missing_actions().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let compatible = responding && missing_actions.is_empty();
    let ready = responding && compatible;

    Ok(BrowserStatus {
        backend: "firefox_agent_bridge",
        browser: target.kind.id(),
        detected_via: target.source.describe(),
        connected_browser,
        setup_complete,
        binary_installed,
        responding,
        compatible,
        missing_actions,
        ready,
    })
}

pub async fn ensure_browser_ready_noninteractive() -> Result<BrowserStatus> {
    ensure_browser_ready_noninteractive_for(&detect_target_browser()).await
}

pub async fn ensure_browser_ready_noninteractive_for(
    target: &BrowserDetection,
) -> Result<BrowserStatus> {
    let mut status = inspect_browser_status_for(target).await?;
    if status.ready && !status.setup_complete {
        mark_setup_complete().ok();
        status.setup_complete = is_setup_complete();
    }
    Ok(status)
}

async fn wait_for_ping(timeout_secs: u64) -> Result<bool> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    while start.elapsed() < timeout {
        if let Ok(true) = check_browser_ping().await {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    Ok(false)
}

async fn wait_for_ready_for(target: &BrowserDetection, timeout_secs: u64) -> Result<bool> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    while start.elapsed() < timeout {
        if let Ok(status) = ensure_browser_ready_noninteractive_for(target).await
            && status.ready
            && connected_matches(&status, target.kind)
        {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    Ok(false)
}

/// Whether `browser setup` should offer to (re)install the bridge extension.
///
/// Keying only off the persistent `.setup-complete` marker meant that once a
/// past setup succeeded, setup could never recover if the extension later
/// vanished from the live Firefox profile: it just printed "already completed".
/// Also re-prompt when the binary is installed but the bridge is not
/// responding, which is the only signal that a previously-working setup lost
/// its extension. A healthy responding bridge stays inert. See #602.
fn should_prompt_extension_install(status: &BrowserStatus) -> bool {
    if !status.setup_complete {
        return true;
    }
    status.binary_installed && !status.responding
}

/// Whether a Firefox process appears to be running on this machine.
pub fn is_firefox_running() -> bool {
    is_browser_running(BrowserKind::Firefox)
}

/// Whether the target browser (see `detect_target_browser`) is running.
pub fn is_target_browser_running() -> bool {
    is_browser_running(detect_target_browser().kind)
}

/// Whether a process of `kind` appears to be running on this machine.
///
/// A bridge that once completed setup but stopped responding usually means
/// the browser is simply closed, not that the install broke. Callers use this
/// to launch the browser instead of re-running one-time setup.
pub fn is_browser_running(kind: BrowserKind) -> bool {
    let names = kind.process_names();
    if names.is_empty() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm"))
                && names.contains(&comm.trim())
            {
                return true;
            }
        }
        false
    }
    #[cfg(target_os = "macos")]
    {
        names.iter().any(|name| {
            std::process::Command::new("pgrep")
                .args(["-ix", name])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
    }
    #[cfg(target_os = "windows")]
    {
        names.iter().any(|image| {
            std::process::Command::new("tasklist")
                .args(["/FI", &format!("IMAGENAME eq {}", image), "/NH"])
                .output()
                .map(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .to_ascii_lowercase()
                        .contains(&image.to_ascii_lowercase())
                })
                .unwrap_or(false)
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Launch a browser detached, optionally opening `url`. Returns whether a
/// launch was started (not whether the browser finished starting).
fn launch_browser_detached(kind: BrowserKind, url: Option<&str>) -> bool {
    #[cfg(target_os = "linux")]
    {
        for candidate in kind.linux_commands() {
            let mut cmd = std::process::Command::new(candidate[0]);
            cmd.args(&candidate[1..]);
            if let Some(url) = url {
                cmd.arg(url);
            }
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if let Ok(child) = crate::platform::spawn_detached(&mut cmd) {
                crate::platform::reap_detached(child);
                return true;
            }
        }
        false
    }
    #[cfg(target_os = "macos")]
    {
        for selector in [
            ["-a", kind.macos_app_name()],
            ["-b", kind.macos_bundle_id()],
        ] {
            let mut cmd = std::process::Command::new("open");
            cmd.args(selector);
            if let Some(url) = url {
                cmd.arg(url);
            }
            let launched = cmd
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if launched {
                return true;
            }
        }
        false
    }
    #[cfg(target_os = "windows")]
    {
        let target = kind.windows_start_target();
        if target.is_empty() {
            return false;
        }
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", target]);
        if let Some(url) = url {
            cmd.arg(url);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Ok(child) = crate::platform::spawn_detached(&mut cmd) {
            crate::platform::reap_detached(child);
            return true;
        }
        false
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (kind, url);
        false
    }
}

/// Whether a silent bridge should be revived by launching Firefox rather than
/// by re-running setup: the binaries are installed, the bridge is not
/// responding, and no Firefox process is running.
pub fn should_attempt_firefox_launch(status: &BrowserStatus, firefox_running: bool) -> bool {
    !status.ready && status.binary_installed && !status.responding && !firefox_running
}

/// Whether automatic browser launching is disabled via environment.
///
/// Set `JCODE_BROWSER_AUTOLAUNCH=0` to keep jcode from starting the browser on
/// its own. Tests also use this to stay hermetic.
pub fn firefox_autolaunch_disabled() -> bool {
    matches!(
        std::env::var("JCODE_BROWSER_AUTOLAUNCH").as_deref(),
        Ok("0") | Ok("false") | Ok("off") | Ok("no")
    )
}

/// If the bridge is installed but silent because Firefox is not running,
/// launch Firefox, wait briefly for the bridge to reconnect, and return the
/// refreshed status. Kept for callers that predate multi-browser support.
pub async fn try_launch_firefox_for_bridge(
    status: &BrowserStatus,
) -> Result<Option<BrowserStatus>> {
    try_launch_browser_for_bridge_with(status, BrowserKind::Firefox).await
}

/// Launch the detected target browser when the bridge is silent because it
/// is closed. Returns `Ok(None)` when no launch was attempted.
pub async fn try_launch_browser_for_bridge(
    status: &BrowserStatus,
) -> Result<Option<BrowserStatus>> {
    try_launch_browser_for_bridge_with(status, detect_target_browser().kind).await
}

pub async fn try_launch_browser_for_bridge_with(
    status: &BrowserStatus,
    kind: BrowserKind,
) -> Result<Option<BrowserStatus>> {
    if firefox_autolaunch_disabled() {
        return Ok(None);
    }
    if kind.family() == BrowserFamily::Safari && status.binary_installed && !status.responding {
        // Safari's extension connects to a relay host that jcode must run.
        let _ = ensure_relay_host_running();
    }
    if !should_attempt_firefox_launch(status, is_browser_running(kind)) {
        return Ok(None);
    }
    if !launch_browser_detached(kind, None) {
        return Ok(None);
    }
    let _ = wait_for_ping(30).await;
    let target = BrowserDetection {
        kind,
        source: browser_detect::DetectionSource::Requested,
        system_default: None,
    };
    let mut refreshed = ensure_browser_ready_noninteractive_for(&target).await?;
    refreshed.detected_via = status.detected_via;
    Ok(Some(refreshed))
}

async fn install_extension() -> Result<String> {
    let xpi = xpi_path();
    let mut msg = String::new();

    if !xpi.exists() {
        return Err(anyhow::anyhow!("XPI file not found at {}", xpi.display()));
    }

    // Try to open Firefox with the XPI to trigger install prompt
    let xpi_url = url::Url::from_file_path(&xpi)
        .map_err(|_| anyhow::anyhow!("Could not convert XPI path to file URL: {}", xpi.display()))?
        .to_string();

    #[cfg(target_os = "linux")]
    {
        let _ = tokio::process::Command::new("xdg-open")
            .arg(&xpi_url)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        // macOS has no default handler for `.xpi` files, so a plain `open <url>`
        // fails with kLSApplicationNotFoundErr. Open the XPI directly with
        // Firefox, which knows how to install extensions. Try the app name first,
        // then fall back to the bundle id (covers Firefox installed under a
        // non-default name or when it is not the default browser).
        let opened = tokio::process::Command::new("open")
            .args(["-a", "Firefox", &xpi_url])
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if !opened {
            let opened_by_id = tokio::process::Command::new("open")
                .args(["-b", "org.mozilla.firefox", &xpi_url])
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false);
            if !opened_by_id {
                // Last resort: let Launch Services pick a handler. This likely
                // fails for `.xpi`, but keeps the previous behavior as a fallback.
                let _ = tokio::process::Command::new("open").arg(&xpi_url).spawn();
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let _ = tokio::process::Command::new("cmd")
            .args(["/C", "start", "", &xpi_url])
            .spawn();
    }

    msg.push_str("       Opened Firefox with extension install prompt.\n");
    msg.push_str("       Click \"Add\" when prompted to install the extension.\n");

    Ok(msg)
}

async fn install_extension_for(kind: BrowserKind) -> Result<String> {
    match kind.family() {
        BrowserFamily::Gecko => install_extension().await,
        BrowserFamily::Chromium => install_chromium_extension(kind),
        BrowserFamily::Safari => install_safari_extension().await,
    }
}

/// Chromium browsers cannot install an extension from the command line, so
/// open the extensions page and walk the user through "Load unpacked". The
/// extension directory is stable, so later updates only need a reload.
fn install_chromium_extension(kind: BrowserKind) -> Result<String> {
    let dir = chromium_extension_dir();
    anyhow::ensure!(
        dir.join("manifest.json").exists(),
        "Chromium extension not found at {}",
        dir.display()
    );
    let opened = launch_browser_detached(kind, Some(kind.extensions_page()));
    let mut msg = String::new();
    if opened {
        msg.push_str(&format!(
            "       Opened {} at {}.\n",
            kind.display_name(),
            kind.extensions_page()
        ));
    } else {
        msg.push_str(&format!(
            "       Open {} in {}.\n",
            kind.extensions_page(),
            kind.display_name()
        ));
    }
    msg.push_str("       1. Turn on \"Developer mode\" (top right).\n");
    msg.push_str("       2. Click \"Load unpacked\" and choose this folder:\n");
    msg.push_str(&format!("          {}\n", dir.display()));
    msg.push_str(&format!(
        "       If the extension is already listed, click its reload button instead. Its ID should be {}.\n",
        CHROMIUM_EXTENSION_ID
    ));
    Ok(msg)
}

/// Safari only loads web extensions shipped inside a macOS app. Build one with
/// Xcode's converter, open it so Safari registers the extension, and start the
/// relay host the extension talks to.
async fn install_safari_extension() -> Result<String> {
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("Safari is only available on macOS")
    }
    #[cfg(target_os = "macos")]
    {
        let ext = safari_extension_dir();
        anyhow::ensure!(
            ext.join("manifest.json").exists(),
            "Safari extension not found at {}",
            ext.display()
        );
        let has_xcode = tokio::process::Command::new("xcrun")
            .args(["--find", "safari-web-extension-converter"])
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        anyhow::ensure!(
            has_xcode,
            "Xcode is required to package Safari extensions. Install Xcode from the App Store, run `xcode-select --install`, then re-run `jcode browser setup safari`."
        );
        let project = safari_app_project_dir();
        if project.exists() {
            std::fs::remove_dir_all(&project)?;
        }
        let convert = tokio::process::Command::new("xcrun")
            .arg("safari-web-extension-converter")
            .arg(&ext)
            .arg("--project-location")
            .arg(&project)
            .args([
                "--app-name",
                "Browser Agent Bridge",
                "--bundle-identifier",
                "io.github.1jehuang.browser-agent-bridge",
                "--swift",
                "--macos-only",
                "--no-open",
                "--no-prompt",
                "--force",
            ])
            .output()
            .await
            .context("Failed to run safari-web-extension-converter")?;
        anyhow::ensure!(
            convert.status.success(),
            "safari-web-extension-converter failed: {}",
            String::from_utf8_lossy(&convert.stderr).trim()
        );
        let xcodeproj = std::fs::read_dir(&project)?
            .flatten()
            .map(|e| e.path())
            .chain(
                std::fs::read_dir(project.join("Browser Agent Bridge"))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path()),
            )
            .find(|p| p.extension().is_some_and(|e| e == "xcodeproj"))
            .context("Converted Xcode project not found")?;
        let derived = project.join("build");
        let build = tokio::process::Command::new("xcodebuild")
            .arg("-project")
            .arg(&xcodeproj)
            .args(["-configuration", "Release", "-derivedDataPath"])
            .arg(&derived)
            .args(["CODE_SIGN_IDENTITY=-", "CODE_SIGNING_REQUIRED=NO", "build"])
            .output()
            .await
            .context("Failed to run xcodebuild")?;
        anyhow::ensure!(
            build.status.success(),
            "xcodebuild failed: {}",
            String::from_utf8_lossy(&build.stdout)
                .lines()
                .rev()
                .take(15)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        );
        let app = derived
            .join("Build/Products/Release")
            .join("Browser Agent Bridge.app");
        anyhow::ensure!(app.exists(), "Built app not found at {}", app.display());
        let _ = tokio::process::Command::new("open")
            .arg(&app)
            .status()
            .await;
        ensure_relay_host_running().ok();
        let mut msg = String::new();
        msg.push_str(&format!("       Built and opened {}.\n", app.display()));
        msg.push_str("       In Safari: Settings > Advanced > enable \"Show features for web developers\",\n");
        msg.push_str(
            "       then Develop > \"Allow Unsigned Extensions\" (resets when Safari quits),\n",
        );
        msg.push_str("       then Settings > Extensions > enable \"Browser Agent Bridge\" and allow it on all websites.\n");
        Ok(msg)
    }
}

pub async fn run_setup_command() -> Result<()> {
    run_setup_command_for(None).await
}

pub async fn run_setup_command_for(requested: Option<&str>) -> Result<()> {
    let target = resolve_target_browser(requested)?;
    println!("Browser Automation Setup");
    println!("========================\n");
    println!("Backend: Browser Agent Bridge\n");

    let log = ensure_browser_setup_for(target).await?;
    print!("{}", log);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "browser_tests.rs"]
mod browser_tests;
