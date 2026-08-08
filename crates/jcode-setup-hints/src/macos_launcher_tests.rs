use super::*;

#[test]
fn macos_launcher_script_shows_alerts_and_uses_terminal_launcher() {
    let script = macos_launcher_script(
        MacTerminalKind::Ghostty,
        "/tmp/jcode",
        Path::new("/Users/test/Applications/Jcode.app"),
    );
    assert!(script.contains("display alert \"Jcode launch failed\""));
    assert!(script.contains("jcode setup-launcher"));
    assert!(script.contains("/usr/bin/open -na Ghostty"));
    assert!(script.contains("macos-launcher.log"));
}

#[test]
fn macos_launcher_icon_asset_is_valid_icns_container() {
    assert!(MACOS_APP_ICON_BYTES.starts_with(b"icns"));
    assert!(MACOS_APP_ICON_BYTES.len() > 1024);
}

#[test]
fn macos_launcher_refreshes_when_new_bundle_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("Jcode.app");
    let legacy_app_dir = temp.path().join("jcode.app");
    let state = SetupHintsState {
        desktop_shortcut_created: true,
        ..SetupHintsState::default()
    };

    assert!(should_refresh_macos_app_launcher_paths(
        &state,
        &app_dir,
        &legacy_app_dir,
    ));
}

#[test]
fn macos_launcher_refreshes_when_legacy_bundle_exists() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("Jcode.app");
    let legacy_app_dir = temp.path().join("jcode.app");
    std::fs::create_dir_all(&app_dir).expect("create new app dir");
    std::fs::create_dir_all(&legacy_app_dir).expect("create legacy app dir");
    let state = SetupHintsState {
        desktop_shortcut_created: true,
        ..SetupHintsState::default()
    };

    assert!(should_refresh_macos_app_launcher_paths(
        &state,
        &app_dir,
        &legacy_app_dir,
    ));
}

#[test]
fn macos_launcher_refreshes_when_new_bundle_is_plain_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("Jcode.app");
    let legacy_app_dir = temp.path().join("jcode.app");
    std::fs::write(&app_dir, "broken").expect("write broken launcher file");
    let state = SetupHintsState {
        desktop_shortcut_created: true,
        ..SetupHintsState::default()
    };

    assert!(should_refresh_macos_app_launcher_paths(
        &state,
        &app_dir,
        &legacy_app_dir,
    ));
}

#[test]
fn macos_launcher_refreshes_when_bundle_is_incomplete() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("Jcode.app");
    let legacy_app_dir = temp.path().join("jcode.app");
    std::fs::create_dir_all(app_dir.join("Contents")).expect("create incomplete bundle");
    std::fs::write(macos_app_launcher_info_plist_path(&app_dir), "plist").expect("write plist");
    let state = SetupHintsState {
        desktop_shortcut_created: true,
        ..SetupHintsState::default()
    };

    assert!(!macos_app_launcher_is_valid(&app_dir));
    assert!(should_refresh_macos_app_launcher_paths(
        &state,
        &app_dir,
        &legacy_app_dir,
    ));
}

#[test]
fn macos_launcher_does_not_refresh_when_new_bundle_exists() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("Jcode.app");
    let legacy_app_dir = temp.path().join("jcode.app");
    std::fs::create_dir_all(app_dir.join("Contents").join("MacOS")).expect("create new app dir");
    std::fs::create_dir_all(app_dir.join("Contents").join("Resources"))
        .expect("create resources dir");
    std::fs::write(macos_app_launcher_info_plist_path(&app_dir), "plist").expect("write plist");
    std::fs::write(macos_app_launcher_executable_path(&app_dir), "#!/bin/sh\n")
        .expect("write launcher executable");
    std::fs::write(macos_app_launcher_icon_path(&app_dir), MACOS_APP_ICON_BYTES)
        .expect("write launcher icon");
    let state = SetupHintsState {
        desktop_shortcut_created: true,
        ..SetupHintsState::default()
    };

    assert!(macos_app_launcher_is_valid(&app_dir));
    assert!(!should_refresh_macos_app_launcher_paths(
        &state,
        &app_dir,
        &legacy_app_dir,
    ));
}

#[test]
fn macos_launcher_refreshes_when_icon_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app_dir = temp.path().join("Jcode.app");
    let legacy_app_dir = temp.path().join("jcode.app");
    std::fs::create_dir_all(app_dir.join("Contents").join("MacOS")).expect("create new app dir");
    std::fs::write(macos_app_launcher_info_plist_path(&app_dir), "plist").expect("write plist");
    std::fs::write(macos_app_launcher_executable_path(&app_dir), "#!/bin/sh\n")
        .expect("write launcher executable");
    let state = SetupHintsState {
        desktop_shortcut_created: true,
        ..SetupHintsState::default()
    };

    assert!(!macos_app_launcher_is_valid(&app_dir));
    assert!(should_refresh_macos_app_launcher_paths(
        &state,
        &app_dir,
        &legacy_app_dir,
    ));
}

#[test]
fn macos_notification_bundle_is_faceless_and_uses_multicall_binary() {
    let plist = macos_notification_info_plist();
    assert!(plist.contains("<key>LSUIElement</key>\n    <true/>"));
    assert!(plist.contains("<string>com.jcode.notifications</string>"));
    assert!(plist.contains("<string>jcode-notification-broker</string>"));
    assert!(plist.contains(jcode_build_meta::version()));
}

#[test]
fn macos_notification_bundle_validity_is_version_gated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = temp.path().join(MACOS_NOTIFICATION_APP_NAME);
    std::fs::create_dir_all(app.join("Contents/MacOS")).expect("create MacOS");
    std::fs::create_dir_all(app.join("Contents/Resources")).expect("create Resources");
    std::fs::write(
        app.join("Contents/Info.plist"),
        macos_notification_info_plist(),
    )
    .expect("write plist");
    std::fs::write(macos_notification_broker_executable_path(&app), "binary")
        .expect("write executable");
    std::fs::write(
        macos_notification_broker_icon_path(&app),
        MACOS_APP_ICON_BYTES,
    )
    .expect("write icon");
    std::fs::write(
        macos_notification_broker_marker_path(&app),
        jcode_build_meta::version(),
    )
    .expect("write version marker");
    assert!(macos_notification_broker_is_valid(&app));

    std::fs::write(macos_notification_broker_marker_path(&app), "0.0.0")
        .expect("write stale marker");
    assert!(!macos_notification_broker_is_valid(&app));
}
