use super::*;

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
    assert_eq!(status.browser, "firefox");
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
    assert_eq!(status.browser, "firefox");
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
    // Stands in for a CLI waiting on a bridge that will never answer.
    write_executable(&bin, "#!/bin/sh\nsleep 600\n");

    let started = std::time::Instant::now();
    let result = run_browser_cli_capped(&bin, &["ping"], std::time::Duration::from_millis(300))
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

    let output = run_browser_cli_capped(&bin, &["ping"], std::time::Duration::from_secs(5))
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
