use super::*;
use crate::browser_detect::BrowserKind;

#[test]
fn macos_browser_code_sign_clones_are_exact_and_scoped_to_current_user_root() {
    let root = std::path::Path::new("/private/var/folders/test/current-user/X");
    for (kind, nonce, executable) in [
        (BrowserKind::Chrome, "K0GLNn", "Google Chrome"),
        (BrowserKind::Opera, "kzEn2b", "Opera"),
    ] {
        let relative = format!(
            "{}.code_sign_clone/code_sign_clone.{nonce}/{}.app.bundle/Contents/MacOS/{executable}",
            kind.macos_bundle_id(),
            kind.macos_app_name()
        );
        let path = root.join(&relative);
        assert!(macos_process_is_exact_browser_with_clone_root(
            &path,
            kind,
            Some(root)
        ));
        let var_alias = PathBuf::from(path.to_str().unwrap().replacen("/private/var", "/var", 1));
        assert!(macos_process_is_exact_browser_with_clone_root(
            &var_alias,
            kind,
            Some(root)
        ));
        assert!(macos_process_is_exact_browser_with_clone_root(
            &path,
            kind,
            Some(std::path::Path::new("/var/folders/test/current-user/X"))
        ));
        assert!(!macos_process_is_exact_browser_with_clone_root(
            &path, kind, None
        ));
        assert!(!macos_process_is_exact_browser_with_clone_root(
            &path,
            BrowserKind::Brave,
            Some(root)
        ));
        let outside =
            std::path::Path::new("/private/var/folders/test/other-user/X").join(&relative);
        assert!(!macos_process_is_exact_browser_with_clone_root(
            &outside,
            kind,
            Some(root)
        ));
        for invalid in [
            relative.replace(kind.macos_bundle_id(), "com.invalid.Browser"),
            relative.replace(&format!("code_sign_clone.{nonce}"), "code_sign_clone."),
            relative.replace(
                &format!("code_sign_clone.{nonce}"),
                "code_sign_clone.invalid-nonce",
            ),
            relative.replace(".app.bundle/", ".app/"),
            relative.replace(".app.bundle/", ".app.bundle.extra/"),
            relative.replace("/Contents/", "/nested/Contents/"),
            relative.replace("/Contents/", "/./Contents/"),
            relative.replace("/Contents/", "/../Contents/"),
            format!("{relative}/extra"),
            format!("{relative}wrong-executable"),
            format!("nested/{relative}"),
        ] {
            let invalid = root.join(invalid);
            assert!(
                !macos_process_is_exact_browser_with_clone_root(&invalid, kind, Some(root)),
                "{}",
                invalid.display()
            );
        }
        assert!(!macos_process_is_exact_browser_with_clone_root(
            std::path::Path::new(&relative),
            kind,
            Some(root)
        ));
        let installed = PathBuf::from(format!(
            "/Applications/{}.app/Contents/MacOS/{executable}",
            kind.macos_app_name()
        ));
        assert!(macos_process_is_exact_browser_with_clone_root(
            &installed,
            kind,
            Some(root)
        ));
        assert!(macos_process_is_exact_browser_with_clone_root(
            &installed, kind, None
        ));
    }
}

fn firefox_endpoint_fixture() -> NativeHostEndpoint {
    NativeHostEndpoint {
        entry: RegisteredBridgeHost {
            port: 8770,
            pid: 1234,
            browser: None,
            process: Some("/Applications/Firefox.app/Contents/MacOS/firefox".into()),
        },
        host_executable: PathBuf::from("/fixture/firefox-agent-bridge-host"),
        parent_executable: PathBuf::from("/Applications/Firefox.app/Contents/MacOS/firefox"),
        parent_pid: 1233,
        host_started: (100, 200),
    }
}

#[test]
fn kernel_listener_fixture_rejects_stale_ports_wrong_pids_and_non_loopback() {
    // Even if a corrupt registry claims Opera owns 8766, the kernel mapping
    // proves that PID 222 actually owns 8769 and PID 111 owns 8766.
    let output = "p111\nn127.0.0.1:8766\np222\nn127.0.0.1:8769\np333\nn127.0.0.1:8770\nn127.0.0.1:8770\nn127.0.0.1:8767\nn0.0.0.0:8768\nn192.168.1.2:8771\nn127.0.0.1:9000\npinvalid\nn127.0.0.1:8772\n";
    let listeners = lsof_loopback_listeners(output);
    assert_eq!(
        listeners
            .iter()
            .map(|entry| (entry.pid, entry.port))
            .collect::<Vec<_>>(),
        vec![(111, 8766), (222, 8769), (333, 8770)]
    );
    assert!(lsof_proves_loopback_listener(output, 333, 8770));
    assert!(!lsof_proves_loopback_listener(output, 222, 8766));
    assert!(!lsof_proves_loopback_listener(output, 999, 8770));
    assert!(!lsof_proves_loopback_listener(output, 333, 8767));
}

#[test]
fn native_host_process_evidence_must_match_exact_browser_executable_and_user() {
    let endpoint = firefox_endpoint_fixture();
    let valid = |entry: &RegisteredBridgeHost,
                 kind,
                 host: &std::path::Path,
                 parent: &std::path::Path,
                 same_user| {
        native_host_entry_matches_process_evidence(
            entry,
            kind,
            host,
            &endpoint.host_executable,
            parent,
            same_user,
        )
    };
    assert!(valid(
        &endpoint.entry,
        BrowserKind::Firefox,
        &endpoint.host_executable,
        &endpoint.parent_executable,
        true
    ));
    assert!(!valid(
        &endpoint.entry,
        BrowserKind::Chrome,
        &endpoint.host_executable,
        &endpoint.parent_executable,
        true
    ));
    assert!(!valid(
        &endpoint.entry,
        BrowserKind::Firefox,
        &endpoint.host_executable,
        &endpoint.parent_executable,
        false
    ));
    assert!(!valid(
        &endpoint.entry,
        BrowserKind::Firefox,
        std::path::Path::new("/wrong/host"),
        &endpoint.parent_executable,
        true
    ));
    for parent in [
        "/Applications/FakeFirefox.app/Contents/MacOS/firefox",
        "/Applications/Firefox.app/Contents/MacOS/other",
        "Firefox.app/Contents/MacOS/firefox",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ] {
        assert!(!valid(
            &endpoint.entry,
            BrowserKind::Firefox,
            &endpoint.host_executable,
            std::path::Path::new(parent),
            true
        ));
    }
    for port in [0, 8767, 8776, 65535] {
        let mut entry = endpoint.entry.clone();
        entry.port = port;
        assert!(!valid(
            &entry,
            BrowserKind::Firefox,
            &endpoint.host_executable,
            &endpoint.parent_executable,
            true
        ));
    }
    let mut entry = endpoint.entry.clone();
    entry.browser = Some("brave".into());
    assert!(!valid(
        &entry,
        BrowserKind::Firefox,
        &endpoint.host_executable,
        &endpoint.parent_executable,
        true
    ));
    entry = endpoint.entry.clone();
    entry.pid = 0;
    assert!(!valid(
        &entry,
        BrowserKind::Firefox,
        &endpoint.host_executable,
        &endpoint.parent_executable,
        true
    ));
}

#[test]
fn anonymous_firefox_identity_requires_unchanged_native_listener_attestation() {
    let endpoint = firefox_endpoint_fixture();
    let anonymous = serde_json::json!({"pong":true,"time":1});
    let mut info = anonymous.clone();
    assert!(!accept_attested_legacy_firefox_ping(
        &mut info,
        BrowserKind::Firefox,
        None,
        None
    ));
    assert!(!ping_matches(&info, BrowserKind::Firefox));
    assert!(accept_attested_legacy_firefox_ping(
        &mut info,
        BrowserKind::Firefox,
        Some(&endpoint),
        Some(&endpoint)
    ));
    assert!(ping_matches(&info, BrowserKind::Firefox));
    assert_eq!(info["identity_evidence"], "native_host_process");
    assert_eq!(info["attested_port"], 8770);
    let mut changed = endpoint.clone();
    changed.host_started.0 += 1;
    let mut info = anonymous.clone();
    assert!(!accept_attested_legacy_firefox_ping(
        &mut info,
        BrowserKind::Firefox,
        Some(&endpoint),
        Some(&changed)
    ));
    assert!(!accept_attested_legacy_firefox_ping(
        &mut info,
        BrowserKind::Chrome,
        Some(&endpoint),
        Some(&endpoint)
    ));
    for browser in ["chrome", "brave", "opera", "unknown"] {
        let mut info = serde_json::json!({"pong":true,"browser":browser});
        assert!(!accept_attested_legacy_firefox_ping(
            &mut info,
            BrowserKind::Firefox,
            Some(&endpoint),
            Some(&endpoint)
        ));
        assert_eq!(info["browser"], browser);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn explicit_targets_override_inherited_websocket_urls_and_missing_targets_fail_closed() {
    let endpoint = firefox_endpoint_fixture();
    let mut command = std::process::Command::new("unused-fixture");
    command.env("FAB_WS_URL", "ws://127.0.0.1:8766");
    apply_bridge_browser_env_with_endpoint(&mut command, Some("firefox"), Some(&endpoint));
    assert!(command.get_envs().any(|(key, value)| key == "FAB_WS_URL"
        && value == Some(std::ffi::OsStr::new("ws://127.0.0.1:8770"))));
    apply_bridge_browser_env_with_endpoint(&mut command, Some("opera"), None);
    assert!(command.get_envs().any(|(key, value)| key == "FAB_WS_URL"
        && value == Some(std::ffi::OsStr::new("ws://127.0.0.1:0"))));
}

#[cfg(unix)]
#[test]
fn bounded_listener_subprocess_does_not_hang() {
    let temp = tempfile::tempdir().unwrap();
    let fixture = temp.path().join("listener-fixture");
    write_executable(&fixture, "#!/bin/sh\nexec sleep 600\n");
    let start = std::time::Instant::now();
    assert!(bounded_process_output(&fixture, &[], std::time::Duration::from_millis(100)).is_none());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn chromium_opera_identity_patch_is_checked_idempotent_and_preserves_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let background = temp.path().join("background.js");
    let manifest = temp.path().join("manifest.json");
    std::fs::write(&manifest, "{\"key\":\"stable-extension-key\"}").unwrap();
    let source = format!("// before\n{LEGACY_CHROMIUM_BROWSER_DETECTOR}\n// after\n");
    std::fs::write(&background, &source).unwrap();
    assert!(repair_chromium_browser_identity(temp.path()).unwrap());
    let repaired = std::fs::read_to_string(&background).unwrap();
    assert!(repaired.starts_with("// before\n"));
    assert!(repaired.ends_with("\n// after\n"));
    assert!(repaired.contains("if (/OPR\\//.test(ua)) return \"opera\";"));
    assert!(!repair_chromium_browser_identity(temp.path()).unwrap());
    assert_eq!(std::fs::read_to_string(&background).unwrap(), repaired);
    assert_eq!(
        std::fs::read_to_string(&manifest).unwrap(),
        "{\"key\":\"stable-extension-key\"}"
    );
    for unknown in [
        "// no detector".to_string(),
        source.replace("return \"unknown\"", "return \"different\""),
        format!("{source}{source}"),
    ] {
        std::fs::write(&background, &unknown).unwrap();
        assert!(repair_chromium_browser_identity(temp.path()).is_err());
        assert_eq!(std::fs::read_to_string(&background).unwrap(), unknown);
    }
}

#[test]
fn chromium_opera_detector_identifies_browsers_in_javascript() {
    let detector = patch_chromium_browser_identity_source(LEGACY_CHROMIUM_BROWSER_DETECTOR)
        .unwrap()
        .unwrap();
    let cases = serde_json::json!([
        {"ua":"Mozilla/5.0 Chrome/140.0 Safari/537.36 OPR/123.0", "brave":false, "gecko":false, "expected":"opera"},
        {"ua":"Mozilla/5.0 Chrome/140.0 Safari/537.36", "brave":false, "gecko":false, "expected":"chrome"},
        {"ua":"Mozilla/5.0 Chrome/140.0 Safari/537.36", "brave":true, "gecko":false, "expected":"brave"},
        {"ua":"Mozilla/5.0 Chrome/140.0 Safari/537.36 Edg/140.0", "brave":false, "gecko":false, "expected":"edge"},
        {"ua":"Mozilla/5.0 Firefox/140.0", "brave":false, "gecko":true, "expected":"firefox"},
        {"ua":"Mozilla/5.0 Safari/605.1", "brave":false, "gecko":false, "expected":"safari"}
    ]);
    let script = format!(
        "const vm=require('vm');const detector={};const cases={cases};for(const c of cases){{const actual=vm.runInNewContext(detector+'\\ndetectBrowserName()',{{IS_GECKO:c.gecko,navigator:{{userAgent:c.ua,brave:c.brave}}}});if(actual!==c.expected)throw Error(c.expected+' got '+actual);}}",
        serde_json::to_string(&detector).unwrap()
    );
    match std::process::Command::new("node")
        .args(["-e", &script])
        .output()
    {
        Ok(output) => assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Pure patch regressions still run on hosts without Node.
            assert!(detector.find("/OPR\\/").unwrap() < detector.find("/Chrome\\/").unwrap());
        }
        Err(error) => panic!("Node identity fixture failed: {error}"),
    }
}

#[test]
fn darwin_temp_directory_parser_rejects_invalid_confstr_results() {
    let path = b"/var/folders/user/T/\0";
    assert_eq!(
        parse_macos_temp_directory(path, path.len()),
        Some(PathBuf::from("/var/folders/user/T/"))
    );
    for (bytes, length) in [
        (&b"\0"[..], 1),
        (&b"relative\0"[..], 9),
        (&b"/missing-terminator"[..], 19),
        (&b"/embedded\0nul\0"[..], 14),
        (&b"/\xff\0"[..], 3),
        (&b"/ok\0"[..], 0),
        (&b"/ok\0"[..], 100),
    ] {
        assert!(parse_macos_temp_directory(bytes, length).is_none());
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_bridge_children_use_system_temp_and_preserve_explicit_xdg_override() {
    let _guard = crate::storage::lock_test_env();
    let previous_tmp = std::env::var_os("TMPDIR");
    let previous_xdg = std::env::var_os("XDG_RUNTIME_DIR");
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("browser-fixture");
    write_executable(
        &bin,
        "#!/bin/sh\nprintf '%s\\n%s\\n' \"$TMPDIR\" \"$XDG_RUNTIME_DIR\"\n",
    );
    let mut command = tokio::process::Command::new(&bin);
    command.env("TMPDIR", temp.path().join("agent-scratch"));
    command.env("XDG_RUNTIME_DIR", temp.path());
    apply_bridge_browser_env(&mut command, Some("chrome"));
    let output = command.output().await.unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], macos_native_host_temp_dir().to_str().unwrap());
    assert_ne!(PathBuf::from(lines[0]), temp.path().join("agent-scratch"));
    assert_eq!(PathBuf::from(lines[1]), temp.path());
    assert_eq!(std::env::var_os("TMPDIR"), previous_tmp);
    assert_eq!(std::env::var_os("XDG_RUNTIME_DIR"), previous_xdg);
    let mut session_command = std::process::Command::new("unused-fixture");
    apply_bridge_browser_env(&mut session_command, Some("firefox"));
    assert!(
        session_command
            .get_envs()
            .any(|(key, value)| key == "TMPDIR"
                && value == Some(macos_native_host_temp_dir().as_os_str()))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn wrong_browser_reply_is_not_ready_and_is_not_probed() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::create_dir_all(browser_dir()).unwrap();
    let calls = temp.path().join("calls");
    write_executable(
        &browser_binary_path(),
        &format!(
            "#!/bin/sh\necho \"$1\" >> '{}'\necho '{{\"pong\":true,\"browser\":\"brave\"}}'\n",
            calls.display()
        ),
    );
    for name in ["chrome", "firefox", "opera"] {
        let target = resolve_target_browser(Some(name)).unwrap();
        let status = inspect_browser_status_for(&target).await.unwrap();
        assert_eq!(status.connected_browser.as_deref(), Some("brave"));
        assert!(status.responding);
        assert!(!status.ready);
        assert!(!status.compatible);
        assert!(status.missing_actions.is_empty());
    }
    assert_eq!(
        std::fs::read_to_string(calls).unwrap(),
        "ping\nping\nping\n"
    );
    match previous_home {
        Some(home) => crate::env::set_var("JCODE_HOME", home),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[cfg(unix)]
#[test]
fn native_host_manifest_rejects_wrong_identity_path_permissions_and_allowlist() {
    let temp = tempfile::tempdir().unwrap();
    let host = temp.path().join("host");
    write_executable(&host, "#!/bin/sh\nexit 0\n");
    for kind in [
        BrowserKind::Firefox,
        BrowserKind::Chrome,
        BrowserKind::Brave,
        BrowserKind::Opera,
    ] {
        let valid = native_host_manifest_json(kind, &host.to_string_lossy());
        assert!(native_host_manifest_is_valid_for_path(kind, &valid, &host));
        for (key, value) in [
            ("name", serde_json::json!("different_host")),
            ("type", serde_json::json!("socket")),
            ("path", serde_json::json!("host")),
            ("path", serde_json::json!(temp.path().to_string_lossy())),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = value;
            assert!(
                !native_host_manifest_is_valid_for_path(kind, &invalid, &host),
                "{key}"
            );
        }
        let other = temp.path().join("other-host");
        write_executable(&other, "#!/bin/sh\nexit 0\n");
        let mut invalid = valid.clone();
        invalid["path"] = serde_json::json!(other.to_string_lossy());
        assert!(!native_host_manifest_is_valid_for_path(
            kind, &invalid, &host
        ));
        let key = if kind == BrowserKind::Firefox {
            "allowed_extensions"
        } else {
            "allowed_origins"
        };
        let mut invalid = valid.clone();
        invalid[key]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("*"));
        assert!(!native_host_manifest_is_valid_for_path(
            kind, &invalid, &host
        ));
        invalid[key] = serde_json::json!([]);
        assert!(!native_host_manifest_is_valid_for_path(
            kind, &invalid, &host
        ));
    }
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o644)).unwrap();
    let invalid = native_host_manifest_json(BrowserKind::Chrome, &host.to_string_lossy());
    assert!(!native_host_manifest_is_valid_for_path(
        BrowserKind::Chrome,
        &invalid,
        &host
    ));
}

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
    let temp = tempfile::tempdir().unwrap();
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    let target = resolve_target_browser(Some("firefox")).unwrap();
    let status = inspect_browser_status_for(&target).await.unwrap();
    match previous_home {
        Some(home) => crate::env::set_var("JCODE_HOME", home),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    assert_eq!(status.backend, "firefox_agent_bridge");
    assert!(!status.binary_installed);
    assert!(!status.ready);
}

#[tokio::test]
async fn test_ensure_browser_ready_noninteractive_without_binary() {
    // See test_inspect_browser_status_without_binary: serialize against tests
    // that mutate JCODE_HOME under the test-env lock.
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let previous_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    let target = resolve_target_browser(Some("firefox")).unwrap();
    let status = ensure_browser_ready_noninteractive_for(&target)
        .await
        .unwrap();
    match previous_home {
        Some(home) => crate::env::set_var("JCODE_HOME", home),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    assert_eq!(status.backend, "firefox_agent_bridge");
    assert!(!status.binary_installed);
    assert!(!status.ready);
    assert!(!status.setup_complete);
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let firefox = native_host_manifest_json(BrowserKind::Firefox, &host.to_string_lossy());
    assert!(native_host_manifest_is_valid_for_path(
        BrowserKind::Firefox,
        &firefox,
        &host
    ));
    assert!(!native_host_manifest_is_valid_for_path(
        BrowserKind::Chrome,
        &firefox,
        &host
    ));
    let chrome = native_host_manifest_json(BrowserKind::Chrome, &host.to_string_lossy());
    assert!(native_host_manifest_is_valid_for_path(
        BrowserKind::Edge,
        &chrome,
        &host
    ));
    let missing_host = native_host_manifest_json(BrowserKind::Chrome, "/definitely/missing/host");
    assert!(!native_host_manifest_is_valid_for_path(
        BrowserKind::Chrome,
        &missing_host,
        &host
    ));
}

#[cfg(unix)]
#[test]
fn invalid_native_manifests_are_repaired_and_valid_ones_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let host = temp.path().join("host");
    write_executable(&host, "#!/bin/sh\nexit 0\n");
    let dir = temp.path().join("NativeMessagingHosts");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{NATIVE_HOST_NAME}.json"));
    for invalid in ["not json", "{}", "{\"name\":\"wrong\",\"type\":\"stdio\"}"] {
        std::fs::write(&path, invalid).unwrap();
        assert!(write_native_host_manifests(BrowserKind::Brave, vec![dir.clone()], &host).unwrap());
        let contents = std::fs::read_to_string(&path).unwrap();
        let repaired = serde_json::from_str(&contents).unwrap();
        assert!(native_host_manifest_is_valid_for_path(
            BrowserKind::Brave,
            &repaired,
            &host
        ));
        assert!(
            !write_native_host_manifests(BrowserKind::Chrome, vec![dir.clone()], &host).unwrap()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
    }
}

#[test]
fn ping_reports_are_matched_by_exact_browser() {
    let chrome = serde_json::json!({"pong": true, "browser": "chrome"});
    assert!(ping_matches(&chrome, BrowserKind::Chrome));
    assert!(!ping_matches(&chrome, BrowserKind::Brave));
    assert!(!ping_matches(&chrome, BrowserKind::Firefox));
    // Legacy identity-less replies cannot safely identify a requested browser.
    let legacy = serde_json::json!({"pong": true});
    assert!(!ping_matches(&legacy, BrowserKind::Firefox));
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

    let extension = temp.path().join("chromium-staged");
    let zip = build_zip(&[
        ("manifest.json", b"{\"key\":\"stable\"}"),
        ("background.js", LEGACY_CHROMIUM_BROWSER_DETECTOR.as_bytes()),
    ]);
    replace_dir_with_zip(&extension, &zip, true).unwrap();
    let repaired = std::fs::read_to_string(extension.join("background.js")).unwrap();
    assert_eq!(repaired, opera_aware_chromium_browser_detector());
    assert_eq!(
        std::fs::read_to_string(extension.join("manifest.json")).unwrap(),
        "{\"key\":\"stable\"}"
    );
    let unknown = build_zip(&[("background.js", b"unknown detector")]);
    assert!(replace_dir_with_zip(&extension, &unknown, true).is_err());
    assert_eq!(
        std::fs::read_to_string(extension.join("background.js")).unwrap(),
        repaired
    );
}

#[test]
fn session_daemon_name_includes_the_target_browser() {
    let _guard = crate::storage::lock_test_env();
    use super::browser_session::session_name_for;
    assert_eq!(session_name_for("session_fox_1", None), "session_fox_1");
    assert_eq!(
        session_name_for("session_fox_1", Some("chrome")),
        "session_fox_1-chrome-target-v4"
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

    // An explicit target wins without mutating the parent environment.
    crate::env::set_var("FAB_BROWSER", "helium");
    let (chrome, firefox) = tokio::join!(
        run(Some(BrowserKind::Chrome)),
        run(Some(BrowserKind::Firefox))
    );
    assert_eq!(chrome, "target=chrome");
    assert_eq!(firefox, "target=firefox");
    assert_eq!(run(None).await, "target=helium");
    assert_eq!(std::env::var("FAB_BROWSER").unwrap(), "helium");
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
#[test]
#[cfg(unix)]
fn listener_budget_accepts_measured_slow_enumeration_without_retry() {
    let output = bounded_process_output(
        std::path::Path::new("/bin/sh"),
        &["-c", "sleep 0.7; printf 'p42\\nn127.0.0.1:8770\\n'"],
        LISTENER_DISCOVERY_TIMEOUT,
    )
    .unwrap();
    assert!(captured_listener_entry(&output, 42, 8770).is_some());
}

#[test]
fn captured_listener_does_not_reselect_lower_port() {
    let output = "p99\nn127.0.0.1:8766\np42\nn127.0.0.1:8770\n";
    assert_eq!(captured_listener_entry(output, 42, 8770).unwrap().pid, 42);
}

#[test]
fn captured_listener_rejects_reused_port_owned_by_another_pid() {
    assert!(captured_listener_entry("p99\nn127.0.0.1:8770\n", 42, 8770).is_none());
}

#[test]
fn captured_listener_rejects_same_pid_on_different_port() {
    assert!(captured_listener_entry("p42\nn127.0.0.1:8768\n", 42, 8770).is_none());
}

#[test]
#[cfg(unix)]
fn probe_diagnostics_do_not_log_secret_response_bodies() {
    use std::os::unix::process::ExitStatusExt;
    let output = std::process::Output {
        status: std::process::ExitStatus::from_raw(256),
        stdout: b"{\"password\":\"stdout-secret\"}".to_vec(),
        stderr: b"stderr-secret".to_vec(),
    };
    let diagnostic = safe_probe_output_summary(&output);
    assert!(!diagnostic.contains("secret"));
    assert!(diagnostic.contains("stdout_bytes="));
}

#[test]
fn unavailable_attestation_does_not_claim_evaluate_is_unsupported() {
    assert!(
        bridge_capabilities_from_probes(&[("evaluate", BridgeActionSupport::Unavailable)])
            .is_none()
    );
}

#[test]
fn genuinely_unsupported_action_remains_reported() {
    assert_eq!(
        bridge_capabilities_from_probes(&[("evaluate", BridgeActionSupport::Unsupported)]),
        Some(vec!["evaluate".to_string()])
    );
}

#[test]
fn supported_probe_has_no_missing_actions() {
    assert_eq!(
        bridge_capabilities_from_probes(&[("evaluate", BridgeActionSupport::Supported)]),
        Some(Vec::new())
    );
}
