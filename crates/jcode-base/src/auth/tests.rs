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
fn auth_status_check_fast_includes_bedrock_probe() {
    let _lock = crate::storage::lock_test_env();
    let prev_bedrock_enable = std::env::var_os("JCODE_BEDROCK_ENABLE");

    crate::env::set_var("JCODE_BEDROCK_ENABLE", "1");
    AuthStatus::invalidate_cache();

    let status = AuthStatus::check_fast();
    assert_eq!(status.bedrock, AuthState::Available);

    restore_env_var("JCODE_BEDROCK_ENABLE", prev_bedrock_enable);
    AuthStatus::invalidate_cache();
}

#[test]
fn full_and_fast_auth_status_match_for_shared_probe_fields() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let home = temp.path().join("home");
    let xdg = temp.path().join("xdg");
    std::fs::create_dir_all(&home).expect("create temp home");
    std::fs::create_dir_all(&xdg).expect("create temp xdg config");
    let saved = [
        "JCODE_HOME",
        "XDG_CONFIG_HOME",
        "HOME",
        crate::subscription_catalog::JCODE_API_KEY_ENV,
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_TRANSPORT_STATE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        crate::auth::azure::ENDPOINT_ENV,
        crate::auth::azure::API_KEY_ENV,
        crate::auth::azure::MODEL_ENV,
        crate::auth::azure::USE_ENTRA_ENV,
        "JCODE_BEDROCK_ENABLE",
        "COPILOT_GITHUB_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "CURSOR_API_KEY",
        "CURSOR_ACCESS_TOKEN",
        "CURSOR_REFRESH_TOKEN",
        "JCODE_CURSOR_CLI_PATH",
    ]
    .into_iter()
    .map(|key| (key, std::env::var_os(key)))
    .collect::<Vec<_>>();

    crate::env::set_var("JCODE_HOME", temp.path().join("jcode-home"));
    crate::env::set_var("XDG_CONFIG_HOME", &xdg);
    crate::env::set_var("HOME", &home);
    crate::env::set_var(
        crate::subscription_catalog::JCODE_API_KEY_ENV,
        "jcode-test-key",
    );
    crate::env::set_var("ANTHROPIC_API_KEY", "anthropic-test-key");
    crate::env::set_var("OPENAI_API_KEY", "openai-test-key");
    crate::env::set_var("OPENROUTER_API_KEY", "openrouter-test-key");
    for key in [
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_TRANSPORT_STATE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
    ] {
        crate::env::remove_var(key);
    }
    crate::env::set_var(
        crate::auth::azure::ENDPOINT_ENV,
        "https://example.openai.azure.com",
    );
    crate::env::set_var(crate::auth::azure::API_KEY_ENV, "azure-test-key");
    crate::env::set_var(crate::auth::azure::MODEL_ENV, "gpt-test-deployment");
    crate::env::remove_var(crate::auth::azure::USE_ENTRA_ENV);
    crate::env::set_var("JCODE_BEDROCK_ENABLE", "1");
    crate::env::set_var("COPILOT_GITHUB_TOKEN", "gho_test_token");
    crate::env::remove_var("GH_TOKEN");
    crate::env::remove_var("GITHUB_TOKEN");
    crate::env::set_var("CURSOR_API_KEY", "cursor-test-key");
    crate::env::remove_var("CURSOR_ACCESS_TOKEN");
    crate::env::remove_var("CURSOR_REFRESH_TOKEN");
    crate::env::set_var(
        "JCODE_CURSOR_CLI_PATH",
        temp.path().join("missing-cursor-agent"),
    );
    AuthStatus::invalidate_cache();

    let (full, _) = build_auth_status_uncached(AuthProbeMode::Full);
    let (fast, _) = build_auth_status_uncached(AuthProbeMode::Fast);

    assert_auth_status_shared_fields_match(&full, &fast);
    assert_eq!(full.jcode, AuthState::Available);
    assert_eq!(full.anthropic.state, AuthState::Available);
    assert_eq!(full.openai, AuthState::Available);
    assert_eq!(full.openrouter, AuthState::Available);
    assert_eq!(full.azure, AuthState::Available);
    assert_eq!(full.bedrock, AuthState::Available);
    assert_eq!(full.copilot, AuthState::Available);
    assert_eq!(full.cursor, AuthState::Available);

    for (key, value) in saved {
        restore_env_var(key, value);
    }
    AuthStatus::invalidate_cache();
}

#[cfg(unix)]
#[test]
fn full_and_fast_auth_status_document_cursor_cli_exception() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let home = temp.path().join("home");
    let xdg = temp.path().join("xdg");
    std::fs::create_dir_all(&home).expect("create temp home");
    std::fs::create_dir_all(&xdg).expect("create temp xdg config");
    let saved = [
        "JCODE_HOME",
        "XDG_CONFIG_HOME",
        "HOME",
        "CURSOR_API_KEY",
        "CURSOR_ACCESS_TOKEN",
        "CURSOR_REFRESH_TOKEN",
        "JCODE_CURSOR_CLI_PATH",
    ]
    .into_iter()
    .map(|key| (key, std::env::var_os(key)))
    .collect::<Vec<_>>();
    let mock_cli = write_mock_cursor_agent(
        temp.path(),
        "#!/bin/sh\nif [ \"$1\" = \"status\" ]; then\n  echo \"Authenticated\\nAccount: test@example.com\"\n  exit 0\nfi\nexit 1\n",
    );

    crate::env::set_var("JCODE_HOME", temp.path().join("jcode-home"));
    crate::env::set_var("XDG_CONFIG_HOME", &xdg);
    crate::env::set_var("HOME", &home);
    crate::env::remove_var("CURSOR_API_KEY");
    crate::env::remove_var("CURSOR_ACCESS_TOKEN");
    crate::env::remove_var("CURSOR_REFRESH_TOKEN");
    crate::env::set_var("JCODE_CURSOR_CLI_PATH", &mock_cli);
    AuthStatus::invalidate_cache();

    let (full, _) = build_auth_status_uncached(AuthProbeMode::Full);
    let (fast, _) = build_auth_status_uncached(AuthProbeMode::Fast);

    assert_eq!(full.cursor, AuthState::Available);
    assert_eq!(fast.cursor, AuthState::NotConfigured);
    assert_eq!(
        full.cursor,
        AuthState::Available,
        "Full auth probes cursor-agent status; fast auth intentionally skips CLI/vscdb probes"
    );

    for (key, value) in saved {
        restore_env_var(key, value);
    }
    AuthStatus::invalidate_cache();
}

fn assert_auth_status_shared_fields_match(full: &AuthStatus, fast: &AuthStatus) {
    assert_eq!(full.jcode, fast.jcode, "jcode");
    assert_eq!(
        full.anthropic.state, fast.anthropic.state,
        "anthropic.state"
    );
    assert_eq!(
        full.anthropic.has_oauth, fast.anthropic.has_oauth,
        "anthropic.has_oauth"
    );
    assert_eq!(
        full.anthropic.has_api_key, fast.anthropic.has_api_key,
        "anthropic.has_api_key"
    );
    assert_eq!(full.openrouter, fast.openrouter, "openrouter");
    assert_eq!(full.azure, fast.azure, "azure");
    assert_eq!(
        full.azure_has_api_key, fast.azure_has_api_key,
        "azure api key"
    );
    assert_eq!(full.azure_uses_entra, fast.azure_uses_entra, "azure entra");
    assert_eq!(full.bedrock, fast.bedrock, "bedrock");
    assert_eq!(full.openai, fast.openai, "openai");
    assert_eq!(full.openai_has_oauth, fast.openai_has_oauth, "openai oauth");
    assert_eq!(
        full.openai_has_api_key, fast.openai_has_api_key,
        "openai api key"
    );
    assert_eq!(full.copilot, fast.copilot, "copilot");
    assert_eq!(
        full.copilot_has_api_token, fast.copilot_has_api_token,
        "copilot api token"
    );
    assert_eq!(full.antigravity, fast.antigravity, "antigravity");
    assert_eq!(full.gemini, fast.gemini, "gemini");
    assert_eq!(full.cursor, fast.cursor, "cursor");
    assert_eq!(full.google, fast.google, "google");
    assert_eq!(full.google_can_send, fast.google_can_send, "google send");
}

#[test]
fn provider_auth_default() {
    let auth = ProviderAuth::default();
    assert_eq!(auth.state, AuthState::NotConfigured);
    assert!(!auth.has_oauth);
    assert!(!auth.has_api_key);
}

#[test]
fn provider_auth_assessment_predicates_reflect_state() {
    fn assessment_with_state(state: AuthState) -> ProviderAuthAssessment {
        ProviderAuthAssessment {
            state,
            readiness: AuthReadinessLevel::None,
            method_detail: "test".to_string(),
            credential_source: AuthCredentialSource::None,
            credential_source_detail: "not configured".to_string(),
            expiry_confidence: AuthExpiryConfidence::Unknown,
            refresh_support: AuthRefreshSupport::Unknown,
            validation_method: AuthValidationMethod::Unknown,
            last_validation: None,
            last_refresh: None,
        }
    }

    let not_configured = assessment_with_state(AuthState::NotConfigured);
    assert!(!not_configured.is_configured());
    assert!(!not_configured.is_available());

    let expired = assessment_with_state(AuthState::Expired);
    assert!(expired.is_configured());
    assert!(!expired.is_available());

    let available = assessment_with_state(AuthState::Available);
    assert!(available.is_configured());
    assert!(available.is_available());
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
fn auth_status_check_fast_ignores_expired_full_cache() {
    let _lock = crate::storage::lock_test_env();
    AuthStatus::invalidate_cache();

    let stale_status = AuthStatus {
        jcode: AuthState::Expired,
        ..Default::default()
    };
    let stale_when = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(
            AUTH_STATUS_CACHE_TTL_SECS + 1,
        ))
        .expect("stale cache timestamp");

    *AUTH_STATUS_CACHE.write().expect("auth cache lock") = Some((stale_status, stale_when));
    *AUTH_STATUS_FAST_CACHE
        .write()
        .expect("fast auth cache lock") = None;

    let status = AuthStatus::check_fast();
    assert_ne!(
        status.jcode,
        AuthState::Expired,
        "check_fast must not reuse an expired full auth cache forever"
    );

    AuthStatus::invalidate_cache();
}

#[test]
fn copilot_recent_token_exchange_failure_is_not_auto_usable() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_copilot_token = std::env::var_os("COPILOT_GITHUB_TOKEN");
    let prev_gh_token = std::env::var_os("GH_TOKEN");
    let prev_github_token = std::env::var_os("GITHUB_TOKEN");

    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::remove_var("COPILOT_GITHUB_TOKEN");
    crate::env::remove_var("GH_TOKEN");
    crate::env::remove_var("GITHUB_TOKEN");
    AuthStatus::invalidate_cache();
    crate::auth::copilot::invalidate_github_token_cache();

    crate::auth::copilot::save_github_token("gho_saved_token", "tester")
        .expect("save copilot token");
    crate::auth::validation::save(
        "copilot",
        crate::auth::validation::ProviderValidationRecord {
            checked_at_ms: chrono::Utc::now().timestamp_millis(),
            success: false,
            provider_smoke_ok: None,
            tool_smoke_ok: None,
            summary:
                "refresh_probe: Copilot token exchange failed (HTTP 403 Forbidden): feature_flag_blocked"
                    .to_string(),
        },
    )
    .expect("save validation failure");

    AuthStatus::invalidate_cache();
    crate::auth::copilot::invalidate_github_token_cache();
    let status = AuthStatus::check_fast();
    assert_eq!(status.copilot, AuthState::Expired);
    assert!(!status.copilot_has_api_token);
    assert_eq!(
        copilot_auth_state_from_credentials(),
        (AuthState::Expired, false)
    );

    crate::env::set_var("GH_TOKEN", "gho_env_override");
    AuthStatus::invalidate_cache();
    crate::auth::copilot::invalidate_github_token_cache();
    let status = AuthStatus::check_fast();
    assert_eq!(status.copilot, AuthState::Available);
    assert!(status.copilot_has_api_token);

    restore_env_var("JCODE_HOME", prev_home);
    restore_env_var("COPILOT_GITHUB_TOKEN", prev_copilot_token);
    restore_env_var("GH_TOKEN", prev_gh_token);
    restore_env_var("GITHUB_TOKEN", prev_github_token);
    AuthStatus::invalidate_cache();
    crate::auth::copilot::invalidate_github_token_cache();
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
    let chutes_assessment =
        status.assessment_for_provider(crate::provider_catalog::CHUTES_LOGIN_PROVIDER);
    let opencode_assessment =
        status.assessment_for_provider(crate::provider_catalog::OPENCODE_LOGIN_PROVIDER);
    assert!(chutes_assessment.is_available());
    assert_eq!(opencode_assessment.state, AuthState::NotConfigured);
    assert_eq!(
        chutes_assessment.method_detail,
        "API key (`CHUTES_API_KEY`)".to_string()
    );

    restore_env_var("JCODE_HOME", prev_home);
    restore_env_var("CHUTES_API_KEY", prev_chutes);
    restore_env_var("OPENCODE_API_KEY", prev_opencode);
    AuthStatus::invalidate_cache();
}

#[test]
fn azure_readiness_distinguishes_credentials_from_deployment_validation() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let saved = [
        "JCODE_HOME",
        crate::auth::azure::ENDPOINT_ENV,
        crate::auth::azure::API_KEY_ENV,
        crate::auth::azure::MODEL_ENV,
        crate::auth::azure::USE_ENTRA_ENV,
    ]
    .into_iter()
    .map(|key| (key, std::env::var_os(key)))
    .collect::<Vec<_>>();

    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::set_var(
        crate::auth::azure::ENDPOINT_ENV,
        "https://example.openai.azure.com",
    );
    crate::env::set_var(crate::auth::azure::API_KEY_ENV, "azure-test-key");
    crate::env::set_var(crate::auth::azure::MODEL_ENV, "gpt-test-deployment");
    crate::env::remove_var(crate::auth::azure::USE_ENTRA_ENV);
    AuthStatus::invalidate_cache();

    let status = AuthStatus::check_fast();
    let assessment = status.assessment_for_provider(crate::provider_catalog::AZURE_LOGIN_PROVIDER);
    assert_eq!(assessment.state, AuthState::Available);
    assert_eq!(assessment.readiness, AuthReadinessLevel::CredentialPresent);
    assert!(
        assessment
            .health_summary()
            .contains("readiness: credential present")
    );

    crate::auth::validation::save(
        "azure",
        crate::auth::validation::ProviderValidationRecord {
            checked_at_ms: chrono::Utc::now().timestamp_millis(),
            success: false,
            provider_smoke_ok: Some(false),
            tool_smoke_ok: None,
            summary: "provider_smoke: deployment not found".to_string(),
        },
    )
    .expect("save failed validation");
    let assessment = status.assessment_for_provider(crate::provider_catalog::AZURE_LOGIN_PROVIDER);
    assert_eq!(assessment.readiness, AuthReadinessLevel::CredentialPresent);

    crate::auth::validation::save(
        "azure",
        crate::auth::validation::ProviderValidationRecord {
            checked_at_ms: chrono::Utc::now().timestamp_millis(),
            success: true,
            provider_smoke_ok: Some(true),
            tool_smoke_ok: None,
            summary: "provider_smoke: ok".to_string(),
        },
    )
    .expect("save successful validation");
    let assessment = status.assessment_for_provider(crate::provider_catalog::AZURE_LOGIN_PROVIDER);
    assert_eq!(assessment.readiness, AuthReadinessLevel::DeploymentValid);
    assert!(
        assessment
            .health_summary()
            .contains("readiness: deployment valid")
    );

    for (key, value) in saved {
        restore_env_var(key, value);
    }
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
