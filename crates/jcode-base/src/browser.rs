use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::{platform, storage};

const GITHUB_API_LATEST: &str =
    "https://api.github.com/repos/1jehuang/firefox-agent-bridge/releases/latest";

const NATIVE_HOST_NAME: &str = "firefox_agent_bridge";
const EXTENSION_ID_LISTED: &str = "browser-agent-bridge@1jehuang.github.io";
const EXTENSION_ID_LOCAL: &str = "firefox-agent-bridge@local";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserStatus {
    pub backend: &'static str,
    pub browser: &'static str,
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
    let mut log = String::new();

    std::fs::create_dir_all(browser_dir())?;

    let initial_status = ensure_browser_ready_noninteractive().await?;
    if initial_status.ready {
        log.push_str("Browser bridge is already set up and responding.\n");
        log.push_str("No setup action was needed.\n");
        return Ok(log);
    }

    // A silent bridge with installed binaries usually just means Firefox is
    // closed. That is not an install problem, so launch Firefox and re-check
    // before running any repair or reopening the extension installer.
    let initial_status = match try_launch_firefox_for_bridge(&initial_status).await? {
        Some(refreshed) if refreshed.ready => {
            log.push_str(
                "Firefox was not running, so it was launched and the browser bridge reconnected.\n",
            );
            log.push_str("No setup action was needed.\n");
            return Ok(log);
        }
        Some(refreshed) => {
            log.push_str("Firefox was not running; launched it, but the bridge did not reconnect yet. Continuing with setup checks...\n");
            refreshed
        }
        None => initial_status,
    };

    if initial_status.responding && !initial_status.compatible {
        log.push_str("Browser bridge is connected, but the live Firefox extension is out of date for this jcode build. Attempting repair steps...\n");
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
        || !xpi_path().exists()
        || (initial_status.responding && !initial_status.compatible)
    {
        log.push_str("[1/3] Downloading browser bridge assets... ");
        match download_browser_binary().await {
            Ok(()) => log.push_str("done\n"),
            Err(e) => {
                log.push_str(&format!("failed: {}\n", e));
                return Ok(log);
            }
        }
    } else {
        log.push_str("[1/3] Browser CLI... already installed\n");
    }

    // Step 2: Install native messaging host manifest
    log.push_str("[2/3] Native messaging host... ");
    match install_native_host_manifest() {
        Ok(installed) => {
            if installed {
                log.push_str("installed\n");
            } else {
                log.push_str("already configured\n");
            }
        }
        Err(e) => {
            log.push_str(&format!("failed: {}\n", e));
            log.push_str("       You may need to run setup manually.\n");
        }
    }

    // Step 3: Check extension connectivity
    log.push_str("[3/3] Checking Firefox extension... ");
    match check_browser_ping().await {
        Ok(true) => {
            log.push_str("connected!\n");
            if initial_status.responding && !initial_status.compatible {
                log.push_str("       Existing extension is missing required actions. Opening Firefox install/update prompt...\n");
                match install_extension().await {
                    Ok(msg) => {
                        log.push_str(&msg);
                        log.push_str("       Waiting for extension update to become ready... ");
                        match wait_for_ready(15).await {
                            Ok(true) => {
                                log.push_str("ready!\n");
                                mark_setup_complete().ok();
                            }
                            Ok(false) => {
                                log.push_str("timed out\n");
                            }
                            Err(e) => {
                                log.push_str(&format!("error: {}\n", e));
                            }
                        }
                    }
                    Err(e) => {
                        log.push_str(&format!("       Could not auto-update extension: {}\n", e));
                    }
                }
            } else {
                mark_setup_complete().ok();
            }
        }
        Ok(false) => {
            log.push_str("not connected\n");
            if should_prompt_extension_install(&initial_status) {
                log.push_str("       Firefox extension needs to be installed.\n");

                match install_extension().await {
                    Ok(msg) => {
                        log.push_str(&msg);
                        // Check again after install attempt
                        log.push_str("       Waiting for extension connection... ");
                        match wait_for_ping(15).await {
                            Ok(true) => {
                                log.push_str("connected!\n");
                                mark_setup_complete().ok();
                            }
                            Ok(false) => {
                                log.push_str("timed out\n");
                                log.push_str(
                                    "       Extension not detected. You can retry with: jcode browser setup\n",
                                );
                                log.push_str(
                                    "       Or manually install: Firefox > about:addons > Install from file > ",
                                );
                                log.push_str(&xpi_path().to_string_lossy());
                                log.push('\n');
                            }
                            Err(e) => {
                                log.push_str(&format!("error: {}\n", e));
                            }
                        }
                    }
                    Err(e) => {
                        log.push_str(&format!("       Could not auto-install extension: {}\n", e));
                        log.push_str(
                            "       Manually install: Firefox > about:addons > Install from file > ",
                        );
                        log.push_str(&xpi_path().to_string_lossy());
                        log.push('\n');
                    }
                }
            } else {
                log.push_str(
                    "       Existing browser setup was already completed, so setup will not reopen the extension installer.\n",
                );
                log.push_str(
                    "       Make sure Firefox is running with the Browser Agent Bridge extension enabled, then re-run `jcode browser status`.\n",
                );
            }
        }
        Err(e) => {
            log.push_str(&format!("error: {}\n", e));
            log.push_str("       Make sure Firefox is running.\n");
        }
    }

    let final_status = ensure_browser_ready_noninteractive().await?;
    if final_status.ready {
        log.push_str("\nSetup complete. Browser bridge is ready.\n");
    } else if final_status.responding && !final_status.compatible {
        log.push_str("\nSetup is not complete yet. The Firefox extension is connected, but it is still missing required actions for this jcode build.\n");
        if !final_status.missing_actions.is_empty() {
            log.push_str(&format!(
                "Missing actions: {}\n",
                final_status.missing_actions.join(", ")
            ));
        }
        log.push_str("Use `jcode browser status` to verify readiness after updating the extension in Firefox.\n");
    } else if final_status.binary_installed {
        log.push_str("\nSetup is not complete yet. Browser bridge binaries are installed, but the Firefox extension/bridge is not responding.\n");
        log.push_str(
            "Use `jcode browser status` to re-check readiness after any manual Firefox step.\n",
        );
    } else {
        log.push_str("\nSetup is not complete yet. Browser bridge binary is still missing.\n");
    }

    Ok(log)
}

async fn download_browser_binary() -> Result<()> {
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

    // Find the XPI
    let xpi_asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .map(|n| n.ends_with(".xpi"))
                .unwrap_or(false)
        })
        .context("No XPI asset found in release")?;

    let xpi_url = xpi_asset["browser_download_url"]
        .as_str()
        .context("No XPI download URL")?;

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

    // Download XPI
    let xpi_bytes = client
        .get(xpi_url)
        .send()
        .await?
        .bytes()
        .await
        .context("Failed to download XPI")?;

    write_file_atomically(&xpi_path(), &xpi_bytes, false)?;

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

fn install_native_host_manifest() -> Result<bool> {
    let manifest_dir = native_messaging_hosts_dir()?;
    let manifest_path = manifest_dir.join(format!("{}.json", NATIVE_HOST_NAME));

    // Check if an existing manifest is already valid (from independent install or previous setup)
    if manifest_path.exists()
        && let Ok(contents) = std::fs::read_to_string(&manifest_path)
        && let Ok(existing) = serde_json::from_str::<serde_json::Value>(&contents)
        && let Some(existing_path) = existing["path"].as_str()
        && std::path::Path::new(existing_path).exists()
    {
        #[cfg(target_os = "windows")]
        register_windows_native_host_manifest(&manifest_path)?;
        return Ok(false);
    }

    let host_path = host_binary_path();
    let browser_bin = browser_binary_path();

    let effective_host = if host_path.exists() {
        host_path.to_string_lossy().to_string()
    } else if browser_bin.exists() {
        return Err(anyhow::anyhow!(
            "Host binary not found at {}. The native messaging host is required for the Firefox extension to communicate with the bridge.",
            host_path.display()
        ));
    } else {
        return Err(anyhow::anyhow!("No browser binaries found"));
    };

    std::fs::create_dir_all(&manifest_dir)?;

    let manifest = serde_json::json!({
        "name": NATIVE_HOST_NAME,
        "description": "Native host for Firefox Agent Bridge (managed by jcode)",
        "path": effective_host,
        "type": "stdio",
        "allowed_extensions": [
            EXTENSION_ID_LOCAL,
            EXTENSION_ID_LISTED,
        ]
    });

    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    #[cfg(target_os = "windows")]
    register_windows_native_host_manifest(&manifest_path)?;

    Ok(true)
}

#[cfg(target_os = "windows")]
fn register_windows_native_host_manifest(manifest_path: &std::path::Path) -> Result<()> {
    let key = format!(
        r"HKCU\Software\Mozilla\NativeMessagingHosts\{}",
        NATIVE_HOST_NAME
    );
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
        .context("Failed to register Firefox native messaging host in Windows registry")?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = stderr.trim();
        if details.is_empty() {
            anyhow::bail!(
                "Failed to register Firefox native messaging host in Windows registry: {}",
                stdout.trim()
            );
        }
        anyhow::bail!(
            "Failed to register Firefox native messaging host in Windows registry: {}",
            details
        )
    }
}

fn native_messaging_hosts_dir() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir().context("No home directory")?;
        Ok(home.join(".mozilla").join("native-messaging-hosts"))
    }
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().context("No home directory")?;
        Ok(home
            .join("Library")
            .join("Application Support")
            .join("Mozilla")
            .join("NativeMessagingHosts"))
    }
    #[cfg(target_os = "windows")]
    {
        // On Windows, native messaging hosts are registered via the Windows Registry
        // We'll write the manifest file to a known location and handle registry separately
        let appdata = dirs::data_dir().context("No app data directory")?;
        Ok(appdata.join("Mozilla").join("NativeMessagingHosts"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(anyhow::anyhow!("Unsupported platform for native messaging"))
    }
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
    let bin = browser_binary_path();
    if !bin.exists() {
        return Ok(false);
    }

    match run_browser_cli_capped(&bin, &["ping"], BRIDGE_PING_TIMEOUT).await? {
        Some(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).contains("pong"))
        }
        _ => Ok(false),
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
    let binary_installed = browser_binary_path().exists();
    let setup_complete = is_setup_complete();
    let responding = if binary_installed {
        check_browser_ping().await.unwrap_or(false)
    } else {
        false
    };
    let missing_actions = if responding {
        probe_bridge_missing_actions().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let compatible = responding && missing_actions.is_empty();
    let ready = responding && compatible;

    Ok(BrowserStatus {
        backend: "firefox_agent_bridge",
        browser: "firefox",
        setup_complete,
        binary_installed,
        responding,
        compatible,
        missing_actions,
        ready,
    })
}

pub async fn ensure_browser_ready_noninteractive() -> Result<BrowserStatus> {
    let mut status = inspect_browser_status().await?;
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

async fn wait_for_ready(timeout_secs: u64) -> Result<bool> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    while start.elapsed() < timeout {
        if let Ok(status) = ensure_browser_ready_noninteractive().await
            && status.ready
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
///
/// A bridge that once completed setup but stopped responding usually means
/// Firefox is simply closed, not that the install broke. Callers use this to
/// launch Firefox instead of re-running one-time setup.
pub fn is_firefox_running() -> bool {
    #[cfg(target_os = "linux")]
    {
        const FIREFOX_PROCESS_NAMES: &[&str] = &["firefox", "firefox-bin", "firefox-esr"];
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
                && FIREFOX_PROCESS_NAMES.contains(&comm.trim())
            {
                return true;
            }
        }
        false
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("pgrep")
            .args(["-ix", "firefox"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq firefox.exe", "/NH"])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .to_ascii_lowercase()
                    .contains("firefox.exe")
            })
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Launch Firefox detached, without any install prompt. Returns whether a
/// launch was started (not whether Firefox finished starting).
fn launch_firefox_detached() -> bool {
    #[cfg(target_os = "linux")]
    {
        let candidates: &[&[&str]] = &[
            &["firefox"],
            &["firefox-esr"],
            &["flatpak", "run", "org.mozilla.firefox"],
        ];
        for candidate in candidates {
            let mut cmd = std::process::Command::new(candidate[0]);
            cmd.args(&candidate[1..])
                .stdin(std::process::Stdio::null())
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
        for args in [
            ["-a", "Firefox"].as_slice(),
            ["-b", "org.mozilla.firefox"].as_slice(),
        ] {
            let launched = std::process::Command::new("open")
                .args(args)
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
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", "firefox"])
            .stdin(std::process::Stdio::null())
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
        false
    }
}

/// Whether a silent bridge should be revived by launching Firefox rather than
/// by re-running setup: the binaries are installed, the bridge is not
/// responding, and no Firefox process is running.
pub fn should_attempt_firefox_launch(status: &BrowserStatus, firefox_running: bool) -> bool {
    !status.ready && status.binary_installed && !status.responding && !firefox_running
}

/// Whether automatic Firefox launching is disabled via environment.
///
/// Set `JCODE_BROWSER_AUTOLAUNCH=0` to keep jcode from starting Firefox on its
/// own. Tests also use this to stay hermetic.
pub fn firefox_autolaunch_disabled() -> bool {
    matches!(
        std::env::var("JCODE_BROWSER_AUTOLAUNCH").as_deref(),
        Ok("0") | Ok("false") | Ok("off") | Ok("no")
    )
}

/// If the bridge is installed but silent because Firefox is not running,
/// launch Firefox, wait briefly for the bridge to reconnect, and return the
/// refreshed status. Returns `Ok(None)` when no launch was attempted (bridge
/// healthy, binaries missing, Firefox already running, or launch failed).
pub async fn try_launch_firefox_for_bridge(status: &BrowserStatus) -> Result<Option<BrowserStatus>> {
    if firefox_autolaunch_disabled() {
        return Ok(None);
    }
    if !should_attempt_firefox_launch(status, is_firefox_running()) {
        return Ok(None);
    }
    if !launch_firefox_detached() {
        return Ok(None);
    }
    let _ = wait_for_ping(30).await;
    Ok(Some(ensure_browser_ready_noninteractive().await?))
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

pub async fn run_setup_command() -> Result<()> {
    println!("Browser Automation Setup");
    println!("========================\n");
    println!("Backend: Firefox Agent Bridge\n");

    let log = ensure_browser_setup().await?;
    print!("{}", log);

    if is_setup_complete() {
        println!("\nTip: Import passwords from Chrome/Safari via Firefox Settings > Import Data");
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
#[path = "browser_tests.rs"]
mod browser_tests;
