use super::*;

fn write_bundle(app: &Path, bundle_id: &str) {
    std::fs::create_dir_all(app.join("Contents/MacOS")).expect("create bundle");
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            "<plist><dict>\n    <key>CFBundleName</key>\n    <string>Jcode</string>\n    \
             <key>CFBundleIdentifier</key>\n    <string>{bundle_id}</string>\n</dict></plist>\n"
        ),
    )
    .expect("write plist");
}

#[test]
fn macos_launcher_icon_asset_is_valid_icns_container() {
    assert!(MACOS_APP_ICON_BYTES.starts_with(b"icns"));
    assert!(MACOS_APP_ICON_BYTES.len() > 1024);
}

#[test]
fn legacy_cli_launchers_and_old_broker_location_are_found() {
    let temp = tempfile::tempdir().expect("tempdir");
    let apps = temp.path();
    write_bundle(&apps.join("Jcode.app"), "com.jcode.launcher");
    write_bundle(
        &apps.join(MACOS_NOTIFICATION_APP_NAME),
        "com.jcode.notifications",
    );

    let found = legacy_macos_bundles(apps);
    assert_eq!(
        found,
        vec![apps.join("Jcode Notifications.app"), apps.join("Jcode.app")]
    );
}

#[test]
fn original_lowercase_launcher_is_found() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_bundle(&temp.path().join("jcode.app"), "com.jcode.app");
    assert_eq!(
        legacy_macos_bundles(temp.path()),
        vec![temp.path().join("jcode.app")]
    );
}

#[test]
fn desktop_app_in_user_applications_is_never_matched() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_bundle(
        &temp.path().join("Jcode.app"),
        "dev.solosystems.jcode.desktop",
    );
    assert!(legacy_macos_bundles(temp.path()).is_empty());
}

#[test]
fn bundles_without_a_readable_identifier_are_left_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = temp.path().join("Jcode.app");
    std::fs::create_dir_all(app.join("Contents")).expect("create bundle");
    // Signed apps may ship a binary plist. Unknown means not ours.
    std::fs::write(app.join("Contents/Info.plist"), b"bplist00\x01\x02").expect("write plist");
    std::fs::create_dir_all(temp.path().join("Other.app")).expect("unrelated app");
    assert!(legacy_macos_bundles(temp.path()).is_empty());
}

#[test]
fn a_foreign_bundle_using_the_broker_name_is_left_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_bundle(
        &temp.path().join(MACOS_NOTIFICATION_APP_NAME),
        "com.example.other",
    );
    assert!(legacy_macos_bundles(temp.path()).is_empty());
}

#[test]
fn missing_applications_directory_has_no_legacy_bundles() {
    let temp = tempfile::tempdir().expect("tempdir");
    assert!(legacy_macos_bundles(&temp.path().join("missing")).is_empty());
}

#[test]
fn broker_lives_in_a_hidden_directory_outside_applications() {
    let home = Path::new("/Users/test");
    let dir = macos_notification_broker_dir_in(home);
    assert_eq!(
        dir,
        Path::new("/Users/test/.jcode/notifications/macos/Jcode Notifications.app")
    );
    assert!(!dir.starts_with(home.join("Applications")));
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
fn generated_broker_plist_is_parsed_by_the_identifier_reader() {
    let temp = tempfile::tempdir().expect("tempdir");
    let app = temp.path().join(MACOS_NOTIFICATION_APP_NAME);
    std::fs::create_dir_all(app.join("Contents")).expect("create bundle");
    std::fs::write(
        app.join("Contents/Info.plist"),
        macos_notification_info_plist(),
    )
    .expect("write plist");
    assert_eq!(
        bundle_identifier(&app).as_deref(),
        Some(MACOS_NOTIFICATION_BUNDLE_ID)
    );
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
