use super::*;

#[test]
fn parses_browser_names_and_aliases() {
    assert_eq!(BrowserKind::parse("Chrome"), Some(BrowserKind::Chrome));
    assert_eq!(
        BrowserKind::parse("google-chrome"),
        Some(BrowserKind::Chrome)
    );
    assert_eq!(BrowserKind::parse("msedge"), Some(BrowserKind::Edge));
    assert_eq!(BrowserKind::parse(" safari "), Some(BrowserKind::Safari));
    assert_eq!(BrowserKind::parse("netscape"), None);
    for kind in ALL_BROWSERS {
        assert_eq!(BrowserKind::parse(kind.id()), Some(*kind));
    }
}

#[test]
fn families_group_chromium_browsers() {
    assert_eq!(BrowserKind::Firefox.family(), BrowserFamily::Gecko);
    assert_eq!(BrowserKind::Safari.family(), BrowserFamily::Safari);
    for kind in [
        BrowserKind::Chrome,
        BrowserKind::Chromium,
        BrowserKind::Edge,
        BrowserKind::Brave,
    ] {
        assert_eq!(kind.family(), BrowserFamily::Chromium);
    }
}

#[test]
fn maps_linux_desktop_ids() {
    assert_eq!(
        browser_from_linux_desktop_id("firefox.desktop"),
        Some(BrowserKind::Firefox)
    );
    assert_eq!(
        browser_from_linux_desktop_id("org.mozilla.firefox.desktop"),
        Some(BrowserKind::Firefox)
    );
    assert_eq!(
        browser_from_linux_desktop_id("google-chrome.desktop"),
        Some(BrowserKind::Chrome)
    );
    assert_eq!(
        browser_from_linux_desktop_id("com.google.Chrome.desktop"),
        Some(BrowserKind::Chrome)
    );
    assert_eq!(
        browser_from_linux_desktop_id("chromium-browser.desktop"),
        Some(BrowserKind::Chromium)
    );
    assert_eq!(
        browser_from_linux_desktop_id("brave-browser.desktop"),
        Some(BrowserKind::Brave)
    );
    assert_eq!(
        browser_from_linux_desktop_id("microsoft-edge.desktop"),
        Some(BrowserKind::Edge)
    );
    assert_eq!(
        browser_from_linux_desktop_id("vivaldi-stable.desktop"),
        None
    );
    assert_eq!(browser_from_linux_desktop_id(""), None);
}

#[test]
fn maps_macos_bundle_ids() {
    assert_eq!(
        browser_from_macos_bundle_id("com.apple.Safari"),
        Some(BrowserKind::Safari)
    );
    assert_eq!(
        browser_from_macos_bundle_id("com.google.chrome"),
        Some(BrowserKind::Chrome)
    );
    assert_eq!(
        browser_from_macos_bundle_id("org.mozilla.firefox"),
        Some(BrowserKind::Firefox)
    );
    assert_eq!(
        browser_from_macos_bundle_id("com.microsoft.edgemac"),
        Some(BrowserKind::Edge)
    );
    assert_eq!(
        browser_from_macos_bundle_id("com.brave.Browser"),
        Some(BrowserKind::Brave)
    );
    assert_eq!(
        browser_from_macos_bundle_id("company.thebrowser.Browser"),
        None
    );
}

#[test]
fn maps_windows_prog_ids() {
    assert_eq!(
        browser_from_windows_prog_id("ChromeHTML"),
        Some(BrowserKind::Chrome)
    );
    assert_eq!(
        browser_from_windows_prog_id("MSEdgeHTM"),
        Some(BrowserKind::Edge)
    );
    assert_eq!(
        browser_from_windows_prog_id("FirefoxURL-308046B0AF4A39CB"),
        Some(BrowserKind::Firefox)
    );
    assert_eq!(
        browser_from_windows_prog_id("BraveHTML"),
        Some(BrowserKind::Brave)
    );
    assert_eq!(browser_from_windows_prog_id("OperaStable"), None);
}

#[test]
fn reads_https_handler_from_launchservices() {
    let json = serde_json::json!({
        "LSHandlers": [
            {"LSHandlerContentType": "public.html", "LSHandlerRoleAll": "com.apple.safari"},
            {"LSHandlerURLScheme": "http", "LSHandlerRoleAll": "org.mozilla.firefox"},
            {"LSHandlerURLScheme": "https", "LSHandlerRoleAll": "com.google.chrome"}
        ]
    });
    assert_eq!(
        macos_default_bundle_from_launchservices(&json).as_deref(),
        Some("com.google.chrome")
    );
    let http_only = serde_json::json!({
        "LSHandlers": [{"LSHandlerURLScheme": "http", "LSHandlerRoleAll": "org.mozilla.firefox"}]
    });
    assert_eq!(
        macos_default_bundle_from_launchservices(&http_only).as_deref(),
        Some("org.mozilla.firefox")
    );
    assert_eq!(
        macos_default_bundle_from_launchservices(&serde_json::json!({})),
        None
    );
}

fn system_id_for(kind: BrowserKind) -> String {
    #[cfg(target_os = "macos")]
    {
        kind.macos_bundle_id().to_string()
    }
    #[cfg(target_os = "windows")]
    {
        match kind {
            BrowserKind::Chrome => "ChromeHTML".into(),
            BrowserKind::Edge => "MSEdgeHTM".into(),
            _ => "FirefoxURL-1".into(),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        match kind {
            BrowserKind::Chrome => "google-chrome.desktop".into(),
            BrowserKind::Edge => "microsoft-edge.desktop".into(),
            _ => "firefox.desktop".into(),
        }
    }
}

#[test]
fn resolution_prefers_env_then_saved_then_default_then_installed() {
    let default_chrome = Some(system_id_for(BrowserKind::Chrome));

    let env = resolve_detection(
        Some("edge"),
        Some(BrowserKind::Firefox),
        default_chrome.clone(),
        &[],
    );
    assert_eq!(env.kind, BrowserKind::Edge);
    assert_eq!(env.source, DetectionSource::EnvOverride);

    let saved = resolve_detection(
        None,
        Some(BrowserKind::Firefox),
        default_chrome.clone(),
        &[],
    );
    assert_eq!(saved.kind, BrowserKind::Firefox);
    assert_eq!(saved.source, DetectionSource::SavedPreference);

    let default = resolve_detection(None, None, default_chrome, &[BrowserKind::Firefox]);
    assert_eq!(default.kind, BrowserKind::Chrome);
    assert_eq!(default.source, DetectionSource::SystemDefault);

    let installed = resolve_detection(
        None,
        None,
        Some("unknown.desktop".into()),
        &[BrowserKind::Brave],
    );
    assert_eq!(installed.kind, BrowserKind::Brave);
    assert_eq!(installed.source, DetectionSource::Installed);

    let fallback = resolve_detection(Some("netscape"), None, None, &[]);
    assert_eq!(fallback.kind, BrowserKind::Firefox);
    assert_eq!(fallback.source, DetectionSource::Fallback);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn safari_is_never_selected_off_macos() {
    let detection = resolve_detection(
        Some("safari"),
        Some(BrowserKind::Safari),
        None,
        &[BrowserKind::Safari],
    );
    assert_ne!(detection.kind, BrowserKind::Safari);
    assert!(!BrowserKind::Safari.is_installed());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_chromium_native_messaging_dirs_are_per_browser() {
    let chrome = BrowserKind::Chrome.native_messaging_dirs();
    assert!(chrome[0].ends_with(".config/google-chrome/NativeMessagingHosts"));
    let firefox = BrowserKind::Firefox.native_messaging_dirs();
    assert!(firefox[0].ends_with(".mozilla/native-messaging-hosts"));
    assert!(BrowserKind::Safari.native_messaging_dirs().is_empty());
}
