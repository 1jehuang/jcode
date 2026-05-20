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

    let complete = BrowserStatus {
        setup_complete: true,
        ..incomplete
    };
    assert!(!should_prompt_extension_install(&complete));
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
