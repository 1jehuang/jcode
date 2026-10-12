use super::*;
use crate::browser_detect::BrowserKind;

#[test]
fn test_is_browser_command() {
    assert!(is_browser_command("browser ping"));
    assert!(is_browser_command(
        "browser navigate '{\"url\": \"https://example.com\"}'"
    ));
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
    let _guard = crate::storage::lock_test_env();

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
    let _guard = crate::storage::lock_test_env();

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

#[test]
fn test_should_prompt_extension_install_only_before_setup_complete() {
    let incomplete = BrowserStatus {
        backend: "firefox_agent_bridge",
        browser: "firefox",
        detected_via: "test",
        connected_browser: None,
        setup_complete: false,
        binary_installed: true,
        responding: false,
        compatible: false,
        missing_actions: vec![],
        ready: false,
    };
    assert!(should_prompt_extension_install(&incomplete));

    // A completed setup whose bridge is healthy stays inert.
    let complete_and_healthy = BrowserStatus {
        setup_complete: true,
        responding: true,
        ..incomplete.clone()
    };
    assert!(!should_prompt_extension_install(&complete_and_healthy));

    // But a completed setup whose extension has since vanished must be able to
    // re-prompt; previously the stale .setup-complete marker suppressed the
    // installer forever (#602).
    let complete_but_dead = BrowserStatus {
        setup_complete: true,
        responding: false,
        ..incomplete
    };
    assert!(should_prompt_extension_install(&complete_but_dead));
}

#[test]
fn test_should_attempt_firefox_launch_only_when_firefox_closed_and_bridge_silent() {
    let installed_but_silent = BrowserStatus {
        backend: "firefox_agent_bridge",
        browser: "firefox",
        detected_via: "test",
        connected_browser: None,
        setup_complete: true,
        binary_installed: true,
        responding: false,
        compatible: false,
        missing_actions: vec![],
        ready: false,
    };

    // Bridge installed and silent, Firefox closed: launch Firefox rather than
    // pushing the agent toward one-time setup/repair.
    assert!(should_attempt_firefox_launch(&installed_but_silent, false));

    // Firefox already running: launching another instance will not help.
    assert!(!should_attempt_firefox_launch(&installed_but_silent, true));

    // Binaries missing: this genuinely needs setup, not a Firefox launch.
    let not_installed = BrowserStatus {
        binary_installed: false,
        setup_complete: false,
        ..installed_but_silent.clone()
    };
    assert!(!should_attempt_firefox_launch(&not_installed, false));

    // Bridge responding (even if incompatible): Firefox is clearly up.
    let responding = BrowserStatus {
        responding: true,
        ..installed_but_silent.clone()
    };
    assert!(!should_attempt_firefox_launch(&responding, false));

    // Already ready: nothing to do.
    let ready = BrowserStatus {
        responding: true,
        compatible: true,
        ready: true,
        ..installed_but_silent
    };
    assert!(!should_attempt_firefox_launch(&ready, false));
}

#[test]
fn setup_complete_requires_native_host_binary() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("JCODE_HOME", temp.path());

    std::fs::create_dir_all(browser_dir()).expect("create browser dir");
    std::fs::write(setup_marker_path(), "test").expect("write setup marker");
    std::fs::write(browser_binary_path(), "browser").expect("write browser binary");

    assert!(browser_binary_path().exists());
    assert!(!host_binary_path().exists());
    assert!(!is_setup_complete());

    std::fs::write(host_binary_path(), "host").expect("write host binary");
    assert!(is_setup_complete());

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[tokio::test]
async fn test_inspect_browser_status_without_binary() {
    // Hold the test-env lock: this reads JCODE_HOME-derived paths, and other
    // tests mutate JCODE_HOME (and write browser fixture files) under the
    // lock. Without it, the status snapshot and the exists() check below can
    // observe different JCODE_HOME values mid-test.
    let _guard = crate::storage::lock_test_env();
    let status = inspect_browser_status().await.unwrap();
    assert_eq!(status.backend, "firefox_agent_bridge");
    assert!(BrowserKind::parse(status.browser).is_some());
    if !browser_binary_path().exists() {
        assert!(!status.binary_installed);
        assert!(!status.ready);
    }
}

#[tokio::test]
async fn test_ensure_browser_ready_noninteractive_without_binary() {
    // See test_inspect_browser_status_without_binary: serialize against tests
    // that mutate JCODE_HOME under the test-env lock.
    let _guard = crate::storage::lock_test_env();
    let status = ensure_browser_ready_noninteractive().await.unwrap();
    assert_eq!(status.backend, "firefox_agent_bridge");
    assert!(BrowserKind::parse(status.browser).is_some());
    if !browser_binary_path().exists() {
        assert!(!status.binary_installed);
        assert!(!status.ready);
        assert!(!status.setup_complete);
    }
}

#[cfg(unix)]
#[test]
fn ensure_browser_session_fails_fast_when_session_process_exits_immediately() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let browser_dir = temp.path().join("browser");
    std::fs::create_dir_all(&browser_dir).expect("create browser dir");
    let bin = browser_dir.join("browser");
    std::fs::write(&bin, "#!/bin/sh\nexit 2\n").expect("write fake browser binary");
    let mut perms = std::fs::metadata(&bin)
        .expect("stat fake browser binary")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin, perms).expect("chmod fake browser binary");

    let start = Instant::now();
    let session = ensure_browser_session("fast-fail-session");
    let elapsed = start.elapsed();

    assert!(session.is_none());
    assert!(
        elapsed < Duration::from_secs(1),
        "expected immediate failure, got {:?}",
        elapsed
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[cfg(unix)]
#[test]
fn ensure_browser_session_does_not_pass_unsupported_bind_window_flag() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let browser_dir = temp.path().join("browser");
    std::fs::create_dir_all(&browser_dir).expect("create browser dir");
    let bin = browser_dir.join("browser");
    let invocations = temp.path().join("invocations");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1 $2 $3\" = \"session start --help\" ]; then\n  echo 'Usage: browser session start [NAME]'\nfi\nexit 2\n",
            invocations.display()
        ),
    )
    .expect("write fake browser binary");
    let mut perms = std::fs::metadata(&bin)
        .expect("stat fake browser binary")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin, perms).expect("chmod fake browser binary");

    assert!(ensure_browser_session("legacy-session").is_none());
    let calls = std::fs::read_to_string(invocations).expect("read invocations");
    assert!(calls.contains("session start --help"), "{calls}");
    assert!(calls.contains("session start legacy-session"), "{calls}");
    assert!(!calls.contains("--bind-window"), "{calls}");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

// Regression coverage for #602.
//
// Bug A: the bridge ping had no timeout. The browser CLI round-trips to the
// Firefox extension over ws://127.0.0.1:8766, so when the extension is missing
// the CLI never returns and `browser status` / `browser setup` hang for
// minutes (measured: 2m14s and a full 3-minute cap).
//
// Bug B: once ~/.jcode/browser/.setup-complete existed, setup could never
// reinstall a vanished extension.

#[cfg(unix)]
fn write_executable(path: &std::path::Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, script).expect("write script");
    let mut perms = std::fs::metadata(path).expect("stat script").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod script");
}

#[cfg(unix)]
#[tokio::test]
async fn hanging_browser_cli_times_out_instead_of_blocking_forever() {
    let temp = tempfile::tempdir().expect("temp dir");
    let bin = temp.path().join("browser");
    // Stands in for a CLI waiting on a bridge that will never answer. `exec` keeps it one
    // process: kill_on_drop kills only the direct child, so a forked sleep outlived the test.
    write_executable(&bin, "#!/bin/sh\nexec sleep 600\n");

    let started = std::time::Instant::now();
    let result =
        run_browser_cli_capped(&bin, &["ping"], std::time::Duration::from_millis(300), None)
            .await
            .expect("capped call should not error");
    let elapsed = started.elapsed();

    assert!(result.is_none(), "a hanging CLI must report a timeout");
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "must fail fast, took {elapsed:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn responsive_browser_cli_still_returns_its_output() {
    let temp = tempfile::tempdir().expect("temp dir");
    let bin = temp.path().join("browser");
    write_executable(&bin, "#!/bin/sh\necho pong\n");

    let output = run_browser_cli_capped(&bin, &["ping"], std::time::Duration::from_secs(5), None)
        .await
        .expect("capped call should not error")
        .expect("a responsive CLI must not report a timeout");

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("pong"));
}

fn status_fixture(setup_complete: bool, binary_installed: bool, responding: bool) -> BrowserStatus {
    BrowserStatus {
        backend: "test",
        browser: "firefox",
        detected_via: "test",
        connected_browser: None,
        setup_complete,
        binary_installed,
        responding,
        compatible: true,
        missing_actions: Vec::new(),
        ready: setup_complete && binary_installed && responding,
    }
}

#[test]
fn setup_prompts_on_a_first_run() {
    assert!(should_prompt_extension_install(&status_fixture(
        false, false, false
    )));
}

#[test]
fn stale_marker_no_longer_blocks_reinstall_when_the_bridge_is_dead() {
    // The #602 shape: marker present from a past setup, binary installed, but
    // the extension is gone from the live profile so nothing answers.
    assert!(should_prompt_extension_install(&status_fixture(
        true, true, false
    )));
}

#[test]
fn healthy_bridge_stays_inert() {
    assert!(!should_prompt_extension_install(&status_fixture(
        true, true, true
    )));
}

#[test]
fn completed_setup_without_the_binary_does_not_prompt() {
    // Nothing to talk to yet; the binary install path handles this, so the
    // extension prompt must not fire spuriously.
    assert!(!should_prompt_extension_install(&status_fixture(
        true, false, false
    )));
}

#[test]
fn chromium_native_host_manifest_allows_the_stable_extension_origin() {
    let manifest = native_host_manifest_json(BrowserKind::Chrome, "/opt/host");
    assert_eq!(manifest["name"], "firefox_agent_bridge");
    assert_eq!(manifest["path"], "/opt/host");
    assert_eq!(
        manifest["allowed_origins"][0],
        "chrome-extension://ijifgeepmnbalajhfjnpbnobfobflfkk/"
    );
    assert!(manifest.get("allowed_extensions").is_none());

    let firefox = native_host_manifest_json(BrowserKind::Firefox, "/opt/host");
    assert!(firefox.get("allowed_origins").is_none());
    assert!(
        firefox["allowed_extensions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "browser-agent-bridge@1jehuang.github.io")
    );
}

#[test]
fn stale_firefox_manifest_is_not_valid_for_chrome() {
    let temp = tempfile::tempdir().unwrap();
    let host = temp.path().join("host");
    std::fs::write(&host, "x").unwrap();
    let firefox = native_host_manifest_json(BrowserKind::Firefox, &host.to_string_lossy());
    assert!(native_host_manifest_is_valid(
        BrowserKind::Firefox,
        &firefox
    ));
    assert!(!native_host_manifest_is_valid(
        BrowserKind::Chrome,
        &firefox
    ));
    let chrome = native_host_manifest_json(BrowserKind::Chrome, &host.to_string_lossy());
    assert!(native_host_manifest_is_valid(BrowserKind::Edge, &chrome));
    let missing_host = native_host_manifest_json(BrowserKind::Chrome, "/definitely/missing/host");
    assert!(!native_host_manifest_is_valid(
        BrowserKind::Chrome,
        &missing_host
    ));
}

#[test]
fn ping_reports_are_matched_by_browser_family() {
    let chrome = serde_json::json!({"pong": true, "browser": "chrome"});
    assert!(ping_matches(&chrome, BrowserKind::Chrome));
    assert!(ping_matches(&chrome, BrowserKind::Brave));
    assert!(!ping_matches(&chrome, BrowserKind::Firefox));
    // Pre-0.10 extensions do not report a browser and only exist for Firefox.
    let legacy = serde_json::json!({"pong": true});
    assert!(ping_matches(&legacy, BrowserKind::Firefox));
    assert!(!ping_matches(&legacy, BrowserKind::Safari));
}

#[test]
fn extract_zip_unpacks_deflate_and_rejects_traversal() {
    use std::io::Write;
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let mut enc =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(data).unwrap();
            let compressed = enc.finish().unwrap();
            let offset = out.len() as u32;
            let crc = 0u32; // extractor does not verify CRCs
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&compressed);
            central
                .extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    let temp = tempfile::tempdir().unwrap();
    let zip = build_zip(&[
        ("manifest.json", b"{\"manifest_version\":3}"),
        ("icons/a.png", b"png"),
    ]);
    extract_zip(&zip, temp.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(temp.path().join("manifest.json")).unwrap(),
        "{\"manifest_version\":3}"
    );
    assert_eq!(
        std::fs::read(temp.path().join("icons/a.png")).unwrap(),
        b"png"
    );

    let evil = build_zip(&[("../escape.txt", b"x")]);
    assert!(extract_zip(&evil, &temp.path().join("sub")).is_err());
    assert!(!temp.path().join("escape.txt").exists());
}

#[test]
fn session_daemon_name_includes_the_target_browser() {
    use super::browser_session::session_name_for;
    assert_eq!(session_name_for("session_fox_1", None), "session_fox_1");
    assert_eq!(
        session_name_for("session_fox_1", Some("chrome")),
        "session_fox_1-chrome"
    );
    assert_ne!(
        session_name_for("s", Some("chrome")),
        session_name_for("s", Some("firefox"))
    );
}

#[test]
fn extra_native_host_dirs_come_from_a_path_list() {
    assert!(extra_native_messaging_dirs(None).is_empty());
    let joined =
        std::env::join_paths(["/a/NativeMessagingHosts", "/b/NativeMessagingHosts"]).unwrap();
    assert_eq!(
        extra_native_messaging_dirs(Some(&joined)),
        vec![
            PathBuf::from("/a/NativeMessagingHosts"),
            PathBuf::from("/b/NativeMessagingHosts")
        ]
    );
}

/// Each browser's native host takes its own port (#1720); jcode must tell the
/// bridge CLI which browser's host to use.
#[cfg(unix)]
#[tokio::test]
async fn bridge_cli_is_pointed_at_the_target_browsers_host() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("FAB_BROWSER");
    crate::env::remove_var("FAB_BROWSER");
    let temp = tempfile::tempdir().expect("temp dir");
    let bin = temp.path().join("browser");
    write_executable(&bin, "#!/bin/sh\necho \"target=$FAB_BROWSER\"\n");

    let run = |kind| {
        let bin = bin.clone();
        async move {
            let out =
                run_browser_cli_capped(&bin, &["ping"], std::time::Duration::from_secs(5), kind)
                    .await
                    .unwrap()
                    .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
    };
    assert_eq!(run(Some(BrowserKind::Chrome)).await, "target=chrome");
    assert_eq!(run(Some(BrowserKind::Firefox)).await, "target=firefox");
    assert_eq!(run(None).await, "target=");

    // A user-chosen FAB_BROWSER (e.g. a fork jcode does not know) wins.
    crate::env::set_var("FAB_BROWSER", "helium");
    assert_eq!(run(Some(BrowserKind::Chrome)).await, "target=helium");
    match prev {
        Some(v) => crate::env::set_var("FAB_BROWSER", v),
        None => crate::env::remove_var("FAB_BROWSER"),
    }
}

#[test]
fn bridge_updates_only_to_strictly_newer_releases() {
    use super::browser_update::is_newer_release;
    assert!(is_newer_release(Some("v0.10.0"), "v0.10.1"));
    assert!(is_newer_release(Some("v0.9.9"), "v0.10.0"));
    assert!(!is_newer_release(Some("v0.10.1"), "v0.10.1"));
    assert!(!is_newer_release(Some("v0.10.1"), "v0.10.0"));
    // Installs from before jcode recorded a version update once.
    assert!(is_newer_release(None, "v0.10.1"));
    assert!(is_newer_release(Some("garbage"), "v0.10.1"));
    // A malformed latest tag never triggers a download.
    assert!(!is_newer_release(None, "nightly"));
}

#[test]
fn bridge_update_check_is_throttled() {
    use super::browser_update::check_due;
    let now = std::time::SystemTime::now();
    let hour = std::time::Duration::from_secs(60 * 60);
    assert!(check_due(None, now));
    assert!(!check_due(Some(now - hour), now));
    assert!(check_due(Some(now - 7 * hour), now));
    // A clock that moved backwards must not block checks forever.
    assert!(check_due(Some(now + hour), now));
}

#[test]
fn bridge_update_is_skipped_until_the_bridge_is_installed() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("JCODE_HOME", temp.path());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // No network call happens: first-time installs stay an explicit setup.
    let update = rt
        .block_on(update_bridge_if_newer(BrowserKind::Firefox, true))
        .unwrap();
    assert_eq!(update, BridgeUpdate::Skipped);
    assert_eq!(installed_bridge_version(), None);
    match prev_home {
        Some(v) => crate::env::set_var("JCODE_HOME", v),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

/// After an update only hosts still running the replaced binary are stopped;
/// a host the reloaded extension already started from the new file survives.
#[cfg(target_os = "linux")]
#[test]
fn update_stops_only_hosts_running_the_replaced_binary() {
    let temp = tempfile::tempdir().expect("temp dir");
    let host = temp.path().join("firefox-agent-bridge-host");
    let sleep = std::path::Path::new("/bin/sleep");
    std::fs::copy(sleep, &host).expect("copy sleep");
    let mut old = std::process::Command::new(&host)
        .arg("30")
        .spawn()
        .expect("old host");
    // Replace the binary the way the updater does (rename over it).
    let staged = temp.path().join(".staged");
    std::fs::copy(sleep, &staged).expect("stage");
    std::fs::rename(&staged, &host).expect("replace");
    let mut new = std::process::Command::new(&host)
        .arg("30")
        .spawn()
        .expect("new host");
    std::thread::sleep(std::time::Duration::from_millis(100));

    super::browser_update::stop_hosts_running(&host);
    std::thread::sleep(std::time::Duration::from_millis(300));

    let old_exited = old.try_wait().expect("poll old").is_some();
    let new_running = new.try_wait().expect("poll new").is_none();
    let _ = new.kill();
    let _ = new.wait();
    let _ = old.kill();
    let _ = old.wait();
    assert!(
        old_exited,
        "host running the replaced binary must be stopped"
    );
    assert!(new_running, "host running the new binary must be kept");
}
