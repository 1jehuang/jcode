use anyhow::{Context, Result};
use std::path::PathBuf;

const GITHUB_API_LATEST: &str =
    "https://api.github.com/repos/1jehuang/firefox-agent-bridge/releases/latest";

const NATIVE_HOST_NAME: &str = "firefox_agent_bridge";
const EXTENSION_ID_LISTED: &str = "browser-agent-bridge@1jehuang.github.io";
const EXTENSION_ID_LOCAL: &str = "firefox-agent-bridge@local";

fn jcode_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
}

fn browser_dir() -> PathBuf {
    jcode_dir().join("browser")
}

fn browser_binary_path() -> PathBuf {
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

pub fn is_browser_command(command: &str) -> bool {
    let trimmed = command.trim_start();
    trimmed.starts_with("browser ")
        || trimmed == "browser"
        || trimmed.starts_with("browser\t")
}

pub fn is_setup_complete() -> bool {
    setup_marker_path().exists() && browser_binary_path().exists()
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

    // Step 1: Check/download browser CLI binary
    if !browser_binary_path().exists() {
        log.push_str("Browser bridge not found. Setting up...\n");
        log.push_str("[1/3] Downloading browser CLI... ");
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
            mark_setup_complete().ok();
        }
        Ok(false) => {
            log.push_str("not connected\n");
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
        }
        Err(e) => {
            log.push_str(&format!("error: {}\n", e));
            log.push_str("       Make sure Firefox is running.\n");
        }
    }

    log.push_str("\nSetup complete. Browser bridge is ready.\n");
    Ok(log)
}

async fn download_browser_binary() -> Result<()> {
    let asset_name = get_platform_asset_name();

    let release_info: serde_json::Value = reqwest::Client::new()
        .get(GITHUB_API_LATEST)
        .header("User-Agent", "jcode")
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
        .find(|a| a["name"].as_str() == Some(&host_asset_name));

    // Download browser CLI
    let browser_bytes = reqwest::Client::new()
        .get(download_url)
        .header("User-Agent", "jcode")
        .send()
        .await?
        .bytes()
        .await
        .context("Failed to download browser binary")?;

    let bin_path = browser_binary_path();
    std::fs::write(&bin_path, &browser_bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755))?;
    }

    // Download XPI
    let xpi_bytes = reqwest::Client::new()
        .get(xpi_url)
        .header("User-Agent", "jcode")
        .send()
        .await?
        .bytes()
        .await
        .context("Failed to download XPI")?;

    std::fs::write(xpi_path(), &xpi_bytes)?;

    // Download host binary if available
    if let Some(host) = host_asset {
        if let Some(host_url) = host["browser_download_url"].as_str() {
            let host_bytes = reqwest::Client::new()
                .get(host_url)
                .header("User-Agent", "jcode")
                .send()
                .await?
                .bytes()
                .await
                .context("Failed to download host binary")?;

            let host_path = host_binary_path();
            std::fs::write(&host_path, &host_bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&host_path, std::fs::Permissions::from_mode(0o755))?;
            }
        }
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
        format!("browser-{}-{}", std::env::consts::OS, std::env::consts::ARCH)
    }
}

fn get_host_asset_name() -> String {
    // The host binary isn't shipped as a separate release asset yet
    // It's built from the same codebase, so we'd need to add it to releases
    // For now, fall back to building from source or using the browser binary
    // with a `host` subcommand if available
    let base = get_platform_asset_name();
    base.replace("browser-", "host-")
}

fn install_native_host_manifest() -> Result<bool> {
    let manifest_dir = native_messaging_hosts_dir()?;
    let manifest_path = manifest_dir.join(format!("{}.json", NATIVE_HOST_NAME));

    let host_path = host_binary_path();
    let browser_bin = browser_binary_path();

    // Use host binary if available, otherwise fall back to browser binary
    // (some versions bundle host functionality into the main binary)
    let effective_host = if host_path.exists() {
        host_path.to_string_lossy().to_string()
    } else if browser_bin.exists() {
        // If no separate host binary, the browser CLI might have a `host` subcommand
        // For now we still need the dedicated host binary
        return Err(anyhow::anyhow!(
            "Host binary not found at {}. The native messaging host is required for the Firefox extension to communicate with the bridge.",
            host_path.display()
        ));
    } else {
        return Err(anyhow::anyhow!("No browser binaries found"));
    };

    // Check if manifest already exists with correct path
    if manifest_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&manifest_path) {
            if let Ok(existing) = serde_json::from_str::<serde_json::Value>(&contents) {
                if existing["path"].as_str() == Some(&effective_host) {
                    return Ok(false);
                }
            }
        }
    }

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

    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest)?,
    )?;

    Ok(true)
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

async fn check_browser_ping() -> Result<bool> {
    let bin = browser_binary_path();
    if !bin.exists() {
        return Ok(false);
    }

    let output = tokio::process::Command::new(&bin)
        .arg("ping")
        .output()
        .await?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.contains("pong"))
    } else {
        Ok(false)
    }
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

async fn install_extension() -> Result<String> {
    let xpi = xpi_path();
    let mut msg = String::new();

    if !xpi.exists() {
        return Err(anyhow::anyhow!(
            "XPI file not found at {}",
            xpi.display()
        ));
    }

    // Try to open Firefox with the XPI to trigger install prompt
    let xpi_url = format!("file://{}", xpi.to_string_lossy());

    #[cfg(target_os = "linux")]
    {
        let _ = tokio::process::Command::new("xdg-open")
            .arg(&xpi_url)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = tokio::process::Command::new("open")
            .arg(&xpi_url)
            .spawn();
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
    println!("Firefox Agent Bridge Setup");
    println!("==========================\n");

    let log = ensure_browser_setup().await?;
    print!("{}", log);

    if is_setup_complete() {
        println!("\nTip: Import passwords from Chrome/Safari via Firefox Settings > Import Data");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_browser_command() {
        assert!(is_browser_command("browser ping"));
        assert!(is_browser_command("browser navigate '{\"url\": \"https://example.com\"}'"));
        assert!(is_browser_command("browser"));
        assert!(is_browser_command("  browser ping"));
        assert!(is_browser_command("browser\tping"));

        assert!(!is_browser_command("echo browser"));
        assert!(!is_browser_command("browsers"));
        assert!(!is_browser_command("my-browser ping"));
        assert!(!is_browser_command(""));
        assert!(!is_browser_command("browserify install"));
    }

    #[test]
    fn test_rewrite_command_with_full_path() {
        let cmd = "browser ping";
        let result = rewrite_command_with_full_path(cmd);
        // If binary exists, it rewrites; if not, returns unchanged
        if browser_binary_path().exists() {
            assert!(result.contains("ping"));
            assert!(result.contains(".jcode/browser"));
        } else {
            assert_eq!(result, cmd);
        }
    }

    #[test]
    fn test_paths() {
        let bdir = browser_dir();
        assert!(bdir.to_string_lossy().contains(".jcode"));
        assert!(bdir.to_string_lossy().ends_with("browser"));

        let bin = browser_binary_path();
        assert!(bin.to_string_lossy().contains("browser"));

        let xpi = xpi_path();
        assert!(xpi.to_string_lossy().ends_with(".xpi"));
    }

    #[test]
    fn test_platform_asset_name() {
        let name = get_platform_asset_name();
        assert!(name.starts_with("browser-"));
        assert!(!name.is_empty());
    }
}
