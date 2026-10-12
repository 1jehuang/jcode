//! Detect which browser the user actually uses so the browser bridge can be
//! installed into it (Firefox, Chromium-family browsers, or Safari).
//!
//! Resolution order:
//! 1. `JCODE_BROWSER` environment variable
//! 2. The browser a previous `jcode browser setup` was completed for
//! 3. The operating system's default web browser, when supported
//! 4. The first supported browser that is installed
//! 5. Firefox

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserKind {
    Firefox,
    Chrome,
    Chromium,
    Edge,
    Brave,
    Safari,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserFamily {
    /// Firefox: MV2 XPI + native messaging.
    Gecko,
    /// Chrome, Chromium, Edge, Brave: MV3 unpacked extension + native messaging.
    Chromium,
    /// Safari: MV3 web extension inside a macOS app + host WebSocket relay.
    Safari,
}

/// Why a particular browser was chosen. Reported in status output so users can
/// see (and override) the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectionSource {
    Requested,
    EnvOverride,
    SavedPreference,
    SystemDefault,
    Installed,
    Fallback,
}

impl DetectionSource {
    pub fn describe(&self) -> &'static str {
        match self {
            DetectionSource::Requested => "requested explicitly",
            DetectionSource::EnvOverride => "set by JCODE_BROWSER",
            DetectionSource::SavedPreference => "configured by a previous `jcode browser setup`",
            DetectionSource::SystemDefault => "your default browser",
            DetectionSource::Installed => "installed (your default browser is not supported)",
            DetectionSource::Fallback => "fallback (no supported browser detected)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserDetection {
    pub kind: BrowserKind,
    pub source: DetectionSource,
    /// Raw identifier of the system default browser, when one was found.
    pub system_default: Option<String>,
}

pub const ALL_BROWSERS: &[BrowserKind] = &[
    BrowserKind::Firefox,
    BrowserKind::Chrome,
    BrowserKind::Edge,
    BrowserKind::Brave,
    BrowserKind::Chromium,
    BrowserKind::Safari,
];

impl BrowserKind {
    pub fn id(self) -> &'static str {
        match self {
            BrowserKind::Firefox => "firefox",
            BrowserKind::Chrome => "chrome",
            BrowserKind::Chromium => "chromium",
            BrowserKind::Edge => "edge",
            BrowserKind::Brave => "brave",
            BrowserKind::Safari => "safari",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            BrowserKind::Firefox => "Firefox",
            BrowserKind::Chrome => "Google Chrome",
            BrowserKind::Chromium => "Chromium",
            BrowserKind::Edge => "Microsoft Edge",
            BrowserKind::Brave => "Brave",
            BrowserKind::Safari => "Safari",
        }
    }

    pub fn family(self) -> BrowserFamily {
        match self {
            BrowserKind::Firefox => BrowserFamily::Gecko,
            BrowserKind::Safari => BrowserFamily::Safari,
            _ => BrowserFamily::Chromium,
        }
    }

    pub fn parse(value: &str) -> Option<BrowserKind> {
        match value.trim().to_ascii_lowercase().as_str() {
            "firefox" | "ff" | "gecko" => Some(BrowserKind::Firefox),
            "chrome" | "google-chrome" | "googlechrome" => Some(BrowserKind::Chrome),
            "chromium" => Some(BrowserKind::Chromium),
            "edge" | "msedge" | "microsoft-edge" => Some(BrowserKind::Edge),
            "brave" | "brave-browser" => Some(BrowserKind::Brave),
            "safari" => Some(BrowserKind::Safari),
            _ => None,
        }
    }

    /// Whether the bridge can support this browser on the current OS.
    pub fn supported_on_this_os(self) -> bool {
        !matches!(self, BrowserKind::Safari) || cfg!(target_os = "macos")
    }

    /// The URL of the browser's extension management page.
    pub fn extensions_page(self) -> &'static str {
        match self {
            BrowserKind::Firefox => "about:addons",
            BrowserKind::Edge => "edge://extensions",
            BrowserKind::Brave => "brave://extensions",
            BrowserKind::Safari => "Safari > Settings > Extensions",
            BrowserKind::Chrome | BrowserKind::Chromium => "chrome://extensions",
        }
    }

    /// Process names (Linux `comm`, macOS process name, or Windows image name).
    pub fn process_names(self) -> &'static [&'static str] {
        #[cfg(target_os = "macos")]
        {
            match self {
                BrowserKind::Firefox => &["firefox"],
                BrowserKind::Chrome => &["Google Chrome"],
                BrowserKind::Chromium => &["Chromium"],
                BrowserKind::Edge => &["Microsoft Edge"],
                BrowserKind::Brave => &["Brave Browser"],
                BrowserKind::Safari => &["Safari"],
            }
        }
        #[cfg(target_os = "windows")]
        {
            match self {
                BrowserKind::Firefox => &["firefox.exe"],
                BrowserKind::Chrome | BrowserKind::Chromium => &["chrome.exe"],
                BrowserKind::Edge => &["msedge.exe"],
                BrowserKind::Brave => &["brave.exe"],
                BrowserKind::Safari => &[],
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            // Linux truncates comm to 15 characters.
            match self {
                BrowserKind::Firefox => &["firefox", "firefox-bin", "firefox-esr"],
                BrowserKind::Chrome => &["chrome", "google-chrome"],
                BrowserKind::Chromium => &["chromium", "chromium-browse"],
                BrowserKind::Edge => &["msedge", "microsoft-edge"],
                BrowserKind::Brave => &["brave", "brave-browser"],
                BrowserKind::Safari => &[],
            }
        }
    }

    /// Executables to try on Linux, in order.
    pub fn linux_commands(self) -> &'static [&'static [&'static str]] {
        match self {
            BrowserKind::Firefox => &[
                &["firefox"],
                &["firefox-esr"],
                &["flatpak", "run", "org.mozilla.firefox"],
            ],
            BrowserKind::Chrome => &[
                &["google-chrome-stable"],
                &["google-chrome"],
                &["flatpak", "run", "com.google.Chrome"],
            ],
            BrowserKind::Chromium => &[
                &["chromium"],
                &["chromium-browser"],
                &["flatpak", "run", "org.chromium.Chromium"],
            ],
            BrowserKind::Edge => &[
                &["microsoft-edge-stable"],
                &["microsoft-edge"],
                &["flatpak", "run", "com.microsoft.Edge"],
            ],
            BrowserKind::Brave => &[
                &["brave-browser"],
                &["brave"],
                &["flatpak", "run", "com.brave.Browser"],
            ],
            BrowserKind::Safari => &[],
        }
    }

    pub fn flatpak_id(self) -> Option<&'static str> {
        match self {
            BrowserKind::Firefox => Some("org.mozilla.firefox"),
            BrowserKind::Chrome => Some("com.google.Chrome"),
            BrowserKind::Chromium => Some("org.chromium.Chromium"),
            BrowserKind::Edge => Some("com.microsoft.Edge"),
            BrowserKind::Brave => Some("com.brave.Browser"),
            BrowserKind::Safari => None,
        }
    }

    /// macOS application name used with `open -a`.
    pub fn macos_app_name(self) -> &'static str {
        match self {
            BrowserKind::Firefox => "Firefox",
            BrowserKind::Chrome => "Google Chrome",
            BrowserKind::Chromium => "Chromium",
            BrowserKind::Edge => "Microsoft Edge",
            BrowserKind::Brave => "Brave Browser",
            BrowserKind::Safari => "Safari",
        }
    }

    pub fn macos_bundle_id(self) -> &'static str {
        match self {
            BrowserKind::Firefox => "org.mozilla.firefox",
            BrowserKind::Chrome => "com.google.Chrome",
            BrowserKind::Chromium => "org.chromium.Chromium",
            BrowserKind::Edge => "com.microsoft.edgemac",
            BrowserKind::Brave => "com.brave.Browser",
            BrowserKind::Safari => "com.apple.Safari",
        }
    }

    /// Windows `start` target (resolved through App Paths).
    pub fn windows_start_target(self) -> &'static str {
        match self {
            BrowserKind::Firefox => "firefox",
            BrowserKind::Chrome | BrowserKind::Chromium => "chrome",
            BrowserKind::Edge => "msedge",
            BrowserKind::Brave => "brave",
            BrowserKind::Safari => "",
        }
    }

    /// Per-user Chromium native messaging registry key (Windows).
    pub fn windows_native_host_registry_roots(self) -> &'static [&'static str] {
        match self {
            BrowserKind::Firefox => &[r"HKCU\Software\Mozilla\NativeMessagingHosts"],
            BrowserKind::Chrome => &[r"HKCU\Software\Google\Chrome\NativeMessagingHosts"],
            BrowserKind::Chromium => &[r"HKCU\Software\Chromium\NativeMessagingHosts"],
            BrowserKind::Edge => &[r"HKCU\Software\Microsoft\Edge\NativeMessagingHosts"],
            // Brave reads Chrome's key on Windows as well as its own.
            BrowserKind::Brave => &[
                r"HKCU\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
                r"HKCU\Software\Google\Chrome\NativeMessagingHosts",
            ],
            BrowserKind::Safari => &[],
        }
    }

    /// Per-user directories where this browser looks for native messaging
    /// host manifests (Linux and macOS; Windows uses the registry).
    pub fn native_messaging_dirs(self) -> Vec<PathBuf> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        #[cfg(target_os = "macos")]
        {
            let support = home.join("Library").join("Application Support");
            match self {
                BrowserKind::Firefox => vec![support.join("Mozilla").join("NativeMessagingHosts")],
                BrowserKind::Chrome => {
                    vec![
                        support
                            .join("Google")
                            .join("Chrome")
                            .join("NativeMessagingHosts"),
                    ]
                }
                BrowserKind::Chromium => {
                    vec![support.join("Chromium").join("NativeMessagingHosts")]
                }
                BrowserKind::Edge => {
                    vec![support.join("Microsoft Edge").join("NativeMessagingHosts")]
                }
                BrowserKind::Brave => vec![
                    support
                        .join("BraveSoftware")
                        .join("Brave-Browser")
                        .join("NativeMessagingHosts"),
                ],
                BrowserKind::Safari => Vec::new(),
            }
        }
        #[cfg(target_os = "windows")]
        {
            let data = dirs::data_dir().unwrap_or_else(|| home.join("AppData").join("Roaming"));
            match self {
                BrowserKind::Firefox => vec![data.join("Mozilla").join("NativeMessagingHosts")],
                BrowserKind::Safari => Vec::new(),
                other => vec![
                    data.join("jcode")
                        .join("NativeMessagingHosts")
                        .join(other.id()),
                ],
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let config = home.join(".config");
            match self {
                BrowserKind::Firefox => vec![home.join(".mozilla").join("native-messaging-hosts")],
                BrowserKind::Chrome => {
                    vec![config.join("google-chrome").join("NativeMessagingHosts")]
                }
                BrowserKind::Chromium => vec![config.join("chromium").join("NativeMessagingHosts")],
                BrowserKind::Edge => {
                    vec![config.join("microsoft-edge").join("NativeMessagingHosts")]
                }
                BrowserKind::Brave => vec![
                    config
                        .join("BraveSoftware")
                        .join("Brave-Browser")
                        .join("NativeMessagingHosts"),
                ],
                BrowserKind::Safari => Vec::new(),
            }
        }
    }

    pub fn is_installed(self) -> bool {
        if !self.supported_on_this_os() {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            if self == BrowserKind::Safari {
                return true;
            }
            let app = format!("{}.app", self.macos_app_name());
            let mut roots = vec![PathBuf::from("/Applications")];
            if let Some(home) = dirs::home_dir() {
                roots.push(home.join("Applications"));
            }
            roots.iter().any(|root| root.join(&app).exists())
        }
        #[cfg(target_os = "windows")]
        {
            let rel: &[&str] = match self {
                BrowserKind::Firefox => &[r"Mozilla Firefox\firefox.exe"],
                BrowserKind::Chrome => &[r"Google\Chrome\Application\chrome.exe"],
                BrowserKind::Chromium => &[r"Chromium\Application\chrome.exe"],
                BrowserKind::Edge => &[r"Microsoft\Edge\Application\msedge.exe"],
                BrowserKind::Brave => &[r"BraveSoftware\Brave-Browser\Application\brave.exe"],
                BrowserKind::Safari => &[],
            };
            ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
                .iter()
                .filter_map(|var| std::env::var_os(var).map(PathBuf::from))
                .any(|root| rel.iter().any(|r| root.join(r).exists()))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let on_path = self
                .linux_commands()
                .iter()
                .filter(|cmd| cmd[0] != "flatpak")
                .any(|cmd| find_in_path(cmd[0]).is_some());
            on_path || self.flatpak_id().is_some_and(flatpak_installed)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn flatpak_installed(app_id: &str) -> bool {
    let mut roots = vec![PathBuf::from("/var/lib/flatpak/app")];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".local/share/flatpak/app"));
    }
    roots.iter().any(|root| root.join(app_id).exists())
}

/// Map a Linux `.desktop` id (from `xdg-settings get default-web-browser`).
pub fn browser_from_linux_desktop_id(desktop_id: &str) -> Option<BrowserKind> {
    let id = desktop_id.trim().to_ascii_lowercase();
    if id.is_empty() {
        return None;
    }
    if id.contains("firefox") {
        Some(BrowserKind::Firefox)
    } else if id.contains("google-chrome") || id.contains("com.google.chrome") {
        Some(BrowserKind::Chrome)
    } else if id.contains("microsoft-edge") || id.contains("com.microsoft.edge") {
        Some(BrowserKind::Edge)
    } else if id.contains("brave") {
        Some(BrowserKind::Brave)
    } else if id.contains("chromium") {
        Some(BrowserKind::Chromium)
    } else {
        None
    }
}

/// Map a macOS bundle id (LaunchServices https handler).
pub fn browser_from_macos_bundle_id(bundle_id: &str) -> Option<BrowserKind> {
    match bundle_id.trim().to_ascii_lowercase().as_str() {
        "org.mozilla.firefox" | "org.mozilla.firefoxdeveloperedition" | "org.mozilla.nightly" => {
            Some(BrowserKind::Firefox)
        }
        "com.google.chrome"
        | "com.google.chrome.beta"
        | "com.google.chrome.dev"
        | "com.google.chrome.canary" => Some(BrowserKind::Chrome),
        "org.chromium.chromium" => Some(BrowserKind::Chromium),
        "com.microsoft.edgemac" | "com.microsoft.edgemac.beta" | "com.microsoft.edgemac.dev" => {
            Some(BrowserKind::Edge)
        }
        "com.brave.browser" | "com.brave.browser.beta" | "com.brave.browser.nightly" => {
            Some(BrowserKind::Brave)
        }
        "com.apple.safari" | "com.apple.safaritechnologypreview" => Some(BrowserKind::Safari),
        _ => None,
    }
}

/// Map a Windows https `UserChoice` ProgId.
pub fn browser_from_windows_prog_id(prog_id: &str) -> Option<BrowserKind> {
    let id = prog_id.trim().to_ascii_lowercase();
    if id.starts_with("firefoxurl") {
        Some(BrowserKind::Firefox)
    } else if id.starts_with("chromehtml") {
        Some(BrowserKind::Chrome)
    } else if id.starts_with("msedgehtm") {
        Some(BrowserKind::Edge)
    } else if id.starts_with("bravehtml") {
        Some(BrowserKind::Brave)
    } else if id.starts_with("chromiumhtm") {
        Some(BrowserKind::Chromium)
    } else {
        None
    }
}

/// Extract the https handler bundle id from LaunchServices JSON (the output
/// of `plutil -convert json` on `com.apple.launchservices.secure.plist`).
pub fn macos_default_bundle_from_launchservices(json: &serde_json::Value) -> Option<String> {
    let handlers = json.get("LSHandlers")?.as_array()?;
    for scheme in ["https", "http"] {
        if let Some(handler) = handlers.iter().find(|h| {
            h.get("LSHandlerURLScheme")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
        }) && let Some(bundle) = handler.get("LSHandlerRoleAll").and_then(|v| v.as_str())
        {
            return Some(bundle.to_string());
        }
    }
    None
}

/// Raw identifier of the OS default web browser, if it can be determined.
pub fn system_default_browser_id() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir()?;
        let plist = home.join(
            "Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist",
        );
        if !plist.exists() {
            // No handler overrides recorded: Safari is the default.
            return Some("com.apple.Safari".to_string());
        }
        let output = std::process::Command::new("plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(&plist)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        Some(
            macos_default_bundle_from_launchservices(&json)
                .unwrap_or_else(|| "com.apple.Safari".to_string()),
        )
    }
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
                "/v",
                "ProgId",
            ])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .find(|line| line.contains("ProgId"))
            .and_then(|line| line.split_whitespace().last())
            .map(str::to_string)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let output = std::process::Command::new("xdg-settings")
            .args(["get", "default-web-browser"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!id.is_empty()).then_some(id)
    }
}

pub fn browser_from_system_id(id: &str) -> Option<BrowserKind> {
    #[cfg(target_os = "macos")]
    {
        browser_from_macos_bundle_id(id)
    }
    #[cfg(target_os = "windows")]
    {
        browser_from_windows_prog_id(id)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        browser_from_linux_desktop_id(id)
    }
}

/// Pure resolution step, separated from environment probing for testing.
pub fn resolve_detection(
    env_override: Option<&str>,
    saved: Option<BrowserKind>,
    system_default: Option<String>,
    installed: &[BrowserKind],
) -> BrowserDetection {
    if let Some(kind) = env_override
        .and_then(BrowserKind::parse)
        .filter(|k| k.supported_on_this_os())
    {
        return BrowserDetection {
            kind,
            source: DetectionSource::EnvOverride,
            system_default,
        };
    }
    if let Some(kind) = saved.filter(|k| k.supported_on_this_os()) {
        return BrowserDetection {
            kind,
            source: DetectionSource::SavedPreference,
            system_default,
        };
    }
    if let Some(kind) = system_default
        .as_deref()
        .and_then(browser_from_system_id)
        .filter(|k| k.supported_on_this_os())
    {
        return BrowserDetection {
            kind,
            source: DetectionSource::SystemDefault,
            system_default,
        };
    }
    if let Some(kind) = installed.iter().copied().find(|k| k.supported_on_this_os()) {
        return BrowserDetection {
            kind,
            source: DetectionSource::Installed,
            system_default,
        };
    }
    BrowserDetection {
        kind: BrowserKind::Firefox,
        source: DetectionSource::Fallback,
        system_default,
    }
}

#[cfg(test)]
#[path = "browser_detect_tests.rs"]
mod browser_detect_tests;
