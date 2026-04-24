use super::*;
use std::ffi::OsString;

fn restore_env_var(key: &str, previous: Option<OsString>) {
    if let Some(previous) = previous {
        crate::env::set_var(key, previous);
    } else {
        crate::env::remove_var(key);
    }
}

#[cfg(unix)]
fn write_mock_cursor_agent(dir: &std::path::Path, script_body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join("cursor-agent-mock");
    std::fs::write(&path, script_body).expect("write mock cursor agent");
    let mut permissions = std::fs::metadata(&path)
        .expect("stat mock cursor agent")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions).expect("chmod mock cursor agent");
    path
}

#[test]
fn command_candidates_adds_extension_on_windows() {
    crate::env::set_var("PATHEXT", ".EXE;.BAT");
    let candidates = command_candidates("testcmd");
    if cfg!(windows) {
        let normalized: Vec<String> = candidates
            .iter()
            .map(|c| c.to_string_lossy().to_ascii_lowercase())
            .collect();
        assert!(normalized.iter().any(|c| c == "testcmd"));
        assert!(normalized.iter().any(|c| c == "testcmd.exe"));
        assert!(normalized.iter().any(|c| c == "testcmd.bat"));
    } else {
        assert_eq!(candidates.len(), 1);
        assert!(candidates.iter().any(|c| c == "testcmd"));
    }
}

#[test]
fn auth_state_default_is_not_configured() {
    let state = AuthState::default();
    assert_eq!(state, AuthState::NotConfigured);
}

#[test]
fn auth_status_default_all_not_configured() {
    let status = AuthStatus::default();
    assert_eq!(status.anthropic.state, AuthState::NotConfigured);
    assert_eq!(status.openrouter, AuthState::NotConfigured);
    assert_eq!(status.openai, AuthState::NotConfigured);
    assert_eq!(status.copilot, AuthState::NotConfigured);
    assert_eq!(status.cursor, AuthState::NotConfigured);
    assert_eq!(status.antigravity, AuthState::NotConfigured);
    assert!(!status.openai_has_oauth);
    assert!(!status.openai_has_api_key);
    assert!(!status.copilot_has_api_token);
    assert!(!status.anthropic.has_oauth);
    assert!(!status.anthropic.has_api_key);
}

#[test]
fn provider_auth_default() {
    let auth = ProviderAuth::default();
    assert_eq!(auth.state, AuthState::NotConfigured);
    assert!(!auth.has_oauth);
    assert!(!auth.has_api_key);
}

#[test]
fn command_exists_for_known_binary() {
    if cfg!(windows) {
        assert!(command_exists("cmd") || command_exists("cmd.exe"));
    } else {
        assert!(command_exists("ls"));
    }
}

#[test]
fn command_exists_empty_string() {
    assert!(!command_exists(""));
    assert!(!command_exists("   "));
}

#[test]
fn command_exists_nonexistent() {
    assert!(!command_exists("surely_this_binary_does_not_exist_xyz"));
}

#[test]
fn command_exists_absolute_path() {
    if cfg!(windows) {
        assert!(command_exists(r"C:\Windows\System32\cmd.exe"));
    } else {
        assert!(command_exists("/bin/ls") || command_exists("/usr/bin/ls"));
    }
}

#[test]
fn command_exists_absolute_nonexistent() {
    assert!(!command_exists("/nonexistent/path/to/binary"));
}

#[test]
fn contains_path_separator_detection() {
    assert!(contains_path_separator("/usr/bin/test"));
    assert!(contains_path_separator("./test"));
    assert!(!contains_path_separator("test"));
}

#[test]
fn has_extension_detection() {
    assert!(has_extension(std::path::Path::new("test.exe")));
    assert!(!has_extension(std::path::Path::new("test")));
    assert!(has_extension(std::path::Path::new("test.sh")));
}

#[test]
fn dedup_preserves_order() {
    let input = vec![
        std::ffi::OsString::from("a"),
        std::ffi::OsString::from("b"),
        std::ffi::OsString::from("a"),
        std::ffi::OsString::from("c"),
    ];
    let result = dedup_preserve_order(input);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], "a");
    assert_eq!(result[1], "b");
    assert_eq!(result[2], "c");
}

#[test]
fn auth_state_equality() {
    assert_eq!(AuthState::Available, AuthState::Available);
    assert_eq!(AuthState::Expired, AuthState::Expired);
    assert_eq!(AuthState::NotConfigured, AuthState::NotConfigured);
    assert_ne!(AuthState::Available, AuthState::Expired);
    assert_ne!(AuthState::Available, AuthState::NotConfigured);
}

#[test]
fn is_wsl2_windows_path_matches_drive_mounts() {
    assert!(is_wsl2_windows_path(std::path::Path::new("/mnt/c")));
    assert!(is_wsl2_windows_path(std::path::Path::new("/mnt/d")));
    assert!(is_wsl2_windows_path(std::path::Path::new("/mnt/z")));
    assert!(is_wsl2_windows_path(std::path::Path::new(
        "/mnt/c/Windows/System32"
    )));
}

#[test]
fn is_wsl2_windows_path_rejects_non_drives() {
    // /mnt/wsl is a WSL-internal mount, not a Windows drive
    assert!(!is_wsl2_windows_path(std::path::Path::new("/mnt/wsl")));
    // /usr/bin is a plain Linux directory
    assert!(!is_wsl2_windows_path(std::path::Path::new("/usr/bin")));
    // /mnt alone is not a drive
    assert!(!is_wsl2_windows_path(std::path::Path::new("/mnt")));
    // empty
    assert!(!is_wsl2_windows_path(std::path::Path::new("")));
}

#[test]
fn command_exists_cached_on_second_call() {
    // Clear cache first to isolate this test
    if let Ok(mut cache) = COMMAND_EXISTS_CACHE.lock() {
        cache.remove("surely_this_binary_does_not_exist_xyz_cache_test");
    }
    // First call populates the cache
    let result1 = command_exists("surely_this_binary_does_not_exist_xyz_cache_test");
    assert!(!result1);
    // Second call must return same result (from cache)
    let result2 = command_exists("surely_this_binary_does_not_exist_xyz_cache_test");
    assert_eq!(result1, result2);
}

#[test]
fn auth_status_check_returns_valid_struct() {
    let status = AuthStatus::check_fast();
    // Just verify it runs without panicking and has coherent state
    match status.anthropic.state {
        AuthState::Available | AuthState::Expired | AuthState::NotConfigured => {}
    }
    match status.openai {
        AuthState::Available | AuthState::Expired | AuthState::NotConfigured => {}
    }
    // If copilot has api token, state should be Available
    if status.copilot_has_api_token {
        assert_eq!(status.copilot, AuthState::Available);
    }
}

#[test]
fn openrouter_like_status_is_provider_specific() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_chutes = std::env::var_os("CHUTES_API_KEY");
    let prev_opencode = std::env::var_os("OPENCODE_API_KEY");

    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::set_var("CHUTES_API_KEY", "chutes-test-key");
    crate::env::remove_var("OPENCODE_API_KEY");
    AuthStatus::invalidate_cache();

    let status = AuthStatus::check_fast();
    assert_eq!(
        status.state_for_provider(crate::provider_catalog::CHUTES_LOGIN_PROVIDER),
        AuthState::Available
    );
    assert_eq!(
        status.state_for_provider(crate::provider_catalog::OPENCODE_LOGIN_PROVIDER),
        AuthState::NotConfigured
    );
    assert_eq!(
        status.method_detail_for_provider(crate::provider_catalog::CHUTES_LOGIN_PROVIDER),
        "API key (`CHUTES_API_KEY`)".to_string()
    );

    restore_env_var("JCODE_HOME", prev_home);
    restore_env_var("CHUTES_API_KEY", prev_chutes);
    restore_env_var("OPENCODE_API_KEY", prev_opencode);
    AuthStatus::invalidate_cache();
}

#[cfg(unix)]
#[test]
fn cursor_status_is_available_when_api_key_exists_without_cli() {
    let _lock = crate::storage::lock_test_env();
    let prev_access_token = std::env::var_os("CURSOR_ACCESS_TOKEN");
    let prev_refresh_token = std::env::var_os("CURSOR_REFRESH_TOKEN");
    let prev_api_key = std::env::var_os("CURSOR_API_KEY");
    let prev_cli_path = std::env::var_os("JCODE_CURSOR_CLI_PATH");
    let temp = tempfile::TempDir::new().expect("create temp dir");

    crate::env::remove_var("CURSOR_ACCESS_TOKEN");
    crate::env::remove_var("CURSOR_REFRESH_TOKEN");
    crate::env::set_var("CURSOR_API_KEY", "cursor-test-key");
    crate::env::set_var(
        "JCODE_CURSOR_CLI_PATH",
        temp.path().join("missing-cursor-agent"),
    );
    AuthStatus::invalidate_cache();

    let status = AuthStatus::check();
    assert_eq!(status.cursor, AuthState::Available);

    restore_env_var("CURSOR_ACCESS_TOKEN", prev_access_token);
    restore_env_var("CURSOR_REFRESH_TOKEN", prev_refresh_token);
    restore_env_var("CURSOR_API_KEY", prev_api_key);
    restore_env_var("JCODE_CURSOR_CLI_PATH", prev_cli_path);
    AuthStatus::invalidate_cache();
}

#[cfg(unix)]
#[test]
fn cursor_status_is_available_for_native_auth_without_cli() {
    let _lock = crate::storage::lock_test_env();
    let prev_access_token = std::env::var_os("CURSOR_ACCESS_TOKEN");
    let prev_refresh_token = std::env::var_os("CURSOR_REFRESH_TOKEN");
    let prev_api_key = std::env::var_os("CURSOR_API_KEY");
    let prev_cli_path = std::env::var_os("JCODE_CURSOR_CLI_PATH");
    let temp = tempfile::TempDir::new().expect("create temp dir");

    crate::env::set_var(
        "CURSOR_ACCESS_TOKEN",
        "eyJhbGciOiJub25lIn0.eyJleHAiIjo0MTAyNDQ0ODAwfQ.",
    );
    crate::env::remove_var("CURSOR_REFRESH_TOKEN");
    crate::env::remove_var("CURSOR_API_KEY");
    crate::env::set_var(
        "JCODE_CURSOR_CLI_PATH",
        temp.path().join("missing-cursor-agent"),
    );
    AuthStatus::invalidate_cache();

    let status = AuthStatus::check();
    assert_eq!(status.cursor, AuthState::Available);

    restore_env_var("CURSOR_ACCESS_TOKEN", prev_access_token);
    restore_env_var("CURSOR_REFRESH_TOKEN", prev_refresh_token);
    restore_env_var("CURSOR_API_KEY", prev_api_key);
    restore_env_var("JCODE_CURSOR_CLI_PATH", prev_cli_path);
    AuthStatus::invalidate_cache();
}

#[cfg(unix)]
#[test]
fn cursor_status_is_available_for_authenticated_cli_session() {
    let _lock = crate::storage::lock_test_env();
    let prev_api_key = std::env::var_os("CURSOR_API_KEY");
    let prev_cli_path = std::env::var_os("JCODE_CURSOR_CLI_PATH");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let mock_cli = write_mock_cursor_agent(
        temp.path(),
        "#!/bin/sh\nif [ \"$1\" = \"status\" ]; then\n  echo \"Authenticated\\nAccount: test@example.com\"\n  exit 0\nfi\nexit 1\n",
    );

    crate::env::remove_var("CURSOR_API_KEY");
    crate::env::set_var("JCODE_CURSOR_CLI_PATH", &mock_cli);
    AuthStatus::invalidate_cache();

    let status = AuthStatus::check();
    assert_eq!(status.cursor, AuthState::Available);

    restore_env_var("CURSOR_API_KEY", prev_api_key);
    restore_env_var("JCODE_CURSOR_CLI_PATH", prev_cli_path);
    AuthStatus::invalidate_cache();
}

#[test]
fn configured_api_key_source_uses_valid_overrides() {
    let _lock = crate::storage::lock_test_env();
    let key_var = "JCODE_OPENAI_COMPAT_API_KEY_NAME";
    let file_var = "JCODE_OPENAI_COMPAT_ENV_FILE";
    let prev_key = std::env::var(key_var).ok();
    let prev_file = std::env::var(file_var).ok();

    crate::env::set_var(key_var, "GROQ_API_KEY");
    crate::env::set_var(file_var, "groq.env");

    let source = crate::provider_catalog::configured_api_key_source(
        key_var,
        file_var,
        "OPENAI_COMPAT_API_KEY",
        "compat.env",
    );
    assert_eq!(
        source,
        Some(("GROQ_API_KEY".to_string(), "groq.env".to_string()))
    );

    if let Some(v) = prev_key {
        crate::env::set_var(key_var, v);
    } else {
        crate::env::remove_var(key_var);
    }
    if let Some(v) = prev_file {
        crate::env::set_var(file_var, v);
    } else {
        crate::env::remove_var(file_var);
    }
}

#[test]
fn configured_api_key_source_rejects_invalid_values() {
    let _lock = crate::storage::lock_test_env();
    let key_var = "JCODE_OPENAI_COMPAT_API_KEY_NAME";
    let file_var = "JCODE_OPENAI_COMPAT_ENV_FILE";
    let prev_key = std::env::var(key_var).ok();
    let prev_file = std::env::var(file_var).ok();

    crate::env::set_var(key_var, "bad-key");
    crate::env::set_var(file_var, "../bad.env");

    let source = crate::provider_catalog::configured_api_key_source(
        key_var,
        file_var,
        "OPENAI_COMPAT_API_KEY",
        "compat.env",
    );
    assert!(source.is_none());

    if let Some(v) = prev_key {
        crate::env::set_var(key_var, v);
    } else {
        crate::env::remove_var(key_var);
    }
    if let Some(v) = prev_file {
        crate::env::set_var(file_var, v);
    } else {
        crate::env::remove_var(file_var);
    }
}
