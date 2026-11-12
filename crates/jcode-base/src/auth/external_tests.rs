use super::*;
use tempfile::TempDir;

fn write_auth_file(path: &std::path::Path, value: serde_json::Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, serde_json::to_string(&value).unwrap()).unwrap();
}

#[test]
fn novita_api_key_import_requires_trust() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::OpenCode.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({ "novita": { "type": "api", "key": "novita_test_secret" } }),
    );
    assert!(load_api_key_for_env("NOVITA_API_KEY").is_none());
    trust_external_auth_source(ExternalAuthSource::OpenCode).unwrap();
    assert_eq!(
        load_api_key_for_env("NOVITA_API_KEY").as_deref(),
        Some("novita_test_secret")
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn opencode_api_key_imports_from_trusted_file() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::OpenCode.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "opencode": { "type": "api", "key": "oc_test_secret" }
        }),
    );

    assert!(load_api_key_for_env("OPENCODE_API_KEY").is_none());
    trust_external_auth_source(ExternalAuthSource::OpenCode).unwrap();
    assert_eq!(
        load_api_key_for_env("OPENCODE_API_KEY").as_deref(),
        Some("oc_test_secret")
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn pi_api_key_env_reference_uses_named_env_var() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_key = std::env::var_os("PI_OPENAI_KEY");
    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::set_var("PI_OPENAI_KEY", "sk-from-env-ref");

    let path = ExternalAuthSource::Pi.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "openai": { "type": "api_key", "key": "PI_OPENAI_KEY" }
        }),
    );

    trust_external_auth_source(ExternalAuthSource::Pi).unwrap();
    assert_eq!(
        load_api_key_for_env("OPENAI_API_KEY").as_deref(),
        Some("sk-from-env-ref")
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    if let Some(prev_key) = prev_key {
        crate::env::set_var("PI_OPENAI_KEY", prev_key);
    } else {
        crate::env::remove_var("PI_OPENAI_KEY");
    }
}

#[test]
fn pi_shell_command_api_keys_are_not_executed() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::Pi.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "openai": { "type": "api_key", "key": "!security find-generic-password -ws openai" }
        }),
    );

    trust_external_auth_source(ExternalAuthSource::Pi).unwrap();
    assert!(load_api_key_for_env("OPENAI_API_KEY").is_none());

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn load_copilot_oauth_token_from_pi_auth() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::Pi.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "github-copilot": {
                "type": "oauth",
                "access": "ghu_pi_token",
                "refresh": "refresh",
                "expires": chrono::Utc::now().timestamp_millis() + 60_000
            }
        }),
    );

    trust_external_auth_source(ExternalAuthSource::Pi).unwrap();
    assert_eq!(load_copilot_oauth_token().as_deref(), Some("ghu_pi_token"));

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn unconsented_source_detects_supported_api_key_files() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::OpenCode.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "opencode": { "type": "api", "key": "oc_test_secret" }
        }),
    );

    assert_eq!(
        preferred_unconsented_api_key_source_for_env("OPENCODE_API_KEY"),
        Some(ExternalAuthSource::OpenCode)
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn source_provider_labels_reports_supported_oauth_and_api_key_imports() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::OpenCode.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "openai": {
                "type": "oauth",
                "access": "sk-access",
                "refresh": "refresh",
                "expires": chrono::Utc::now().timestamp_millis() + 60_000
            },
            "anthropic": {
                "type": "oauth",
                "access": "claude-access",
                "refresh": "refresh",
                "expires": chrono::Utc::now().timestamp_millis() + 60_000
            },
            "openrouter": { "type": "api", "key": "sk-or-test" }
        }),
    );

    let labels = source_provider_labels(ExternalAuthSource::OpenCode);
    assert!(labels.contains(&"OpenAI/Codex"));
    assert!(labels.contains(&"Claude"));
    assert!(labels.contains(&"OpenRouter/API-key providers"));

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn openclaw_api_key_imports_from_trusted_file() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::OpenClaw.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "anthropic": { "type": "api_key", "key": "sk-ant-openclaw" }
        }),
    );

    assert!(load_api_key_for_env("ANTHROPIC_API_KEY").is_none());
    trust_external_auth_source(ExternalAuthSource::OpenClaw).unwrap();
    assert_eq!(
        load_api_key_for_env("ANTHROPIC_API_KEY").as_deref(),
        Some("sk-ant-openclaw")
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn openclaw_oauth_tokens_import_like_pi() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::OpenClaw.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "anthropic": {
                "type": "oauth",
                "access": "claude-access",
                "refresh": "claude-refresh",
                "expires": chrono::Utc::now().timestamp_millis() + 60_000
            }
        }),
    );

    assert!(load_anthropic_oauth_tokens().is_none());
    trust_external_auth_source(ExternalAuthSource::OpenClaw).unwrap();
    let tokens = load_anthropic_oauth_tokens().expect("oauth tokens imported");
    assert_eq!(tokens.access_token, "claude-access");
    assert_eq!(tokens.refresh_token, "claude-refresh");

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn hermes_api_key_imports_from_credential_pool() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::Hermes.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "version": 1,
            "active_provider": "anthropic",
            "credential_pool": {
                "anthropic": [
                    {
                        "id": "abc123",
                        "label": "manual",
                        "auth_type": "api_key",
                        "priority": 0,
                        "source": "manual:1",
                        "access_token": "sk-ant-hermes"
                    }
                ]
            }
        }),
    );

    assert!(load_api_key_for_env("ANTHROPIC_API_KEY").is_none());
    trust_external_auth_source(ExternalAuthSource::Hermes).unwrap();
    assert_eq!(
        load_api_key_for_env("ANTHROPIC_API_KEY").as_deref(),
        Some("sk-ant-hermes")
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn hermes_oauth_tokens_import_from_credential_pool() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = ExternalAuthSource::Hermes.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "version": 1,
            "credential_pool": {
                "openai-codex": [
                    {
                        "id": "def456",
                        "label": "loopback_pkce",
                        "auth_type": "oauth_external",
                        "priority": 0,
                        "source": "loopback_pkce",
                        "access_token": "codex-access",
                        "refresh_token": "codex-refresh",
                        "expires_at_ms": chrono::Utc::now().timestamp_millis() + 60_000
                    }
                ]
            }
        }),
    );

    assert!(load_openai_oauth_tokens().is_none());
    trust_external_auth_source(ExternalAuthSource::Hermes).unwrap();
    let tokens = load_openai_oauth_tokens().expect("oauth tokens imported");
    assert_eq!(tokens.access_token, "codex-access");
    assert_eq!(tokens.refresh_token, "codex-refresh");

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn hermes_oauth_tokens_parse_rfc3339_expiry() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let future = chrono::Utc::now() + chrono::Duration::minutes(5);
    let path = ExternalAuthSource::Hermes.path().unwrap();
    write_auth_file(
        &path,
        serde_json::json!({
            "version": 1,
            "credential_pool": {
                "anthropic": [
                    {
                        "auth_type": "oauth_external",
                        "access_token": "claude-access",
                        "refresh_token": "claude-refresh",
                        "expires_at": future.to_rfc3339()
                    }
                ]
            }
        }),
    );

    trust_external_auth_source(ExternalAuthSource::Hermes).unwrap();
    let tokens = load_anthropic_oauth_tokens().expect("oauth tokens imported");
    assert_eq!(tokens.access_token, "claude-access");
    assert!(tokens.expires_at > chrono::Utc::now().timestamp_millis());

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn openclaw_auth_profiles_store_resolves_and_flattens() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    // No legacy ~/.openclaw/agent/auth.json: the current per-agent
    // auth-profiles.json store must be discovered instead.
    let profiles_path =
        crate::storage::user_home_path(".openclaw/agents/main/agent/auth-profiles.json").unwrap();
    write_auth_file(
        &profiles_path,
        serde_json::json!({
            "version": 1,
            "profiles": {
                "openai:work": {
                    "type": "oauth",
                    "provider": "openai",
                    "access": "work-access",
                    "refresh": "work-refresh",
                    "expires": chrono::Utc::now().timestamp_millis() + 60_000
                },
                "openai:default": {
                    "type": "oauth",
                    "provider": "openai",
                    "access": "openclaw-access",
                    "refresh": "openclaw-refresh",
                    "expires": chrono::Utc::now().timestamp_millis() + 60_000
                },
                "openrouter:default": {
                    "type": "api_key",
                    "provider": "openrouter",
                    "key": "sk-or-openclaw"
                }
            }
        }),
    );

    assert_eq!(ExternalAuthSource::OpenClaw.path().unwrap(), profiles_path);
    trust_external_auth_source(ExternalAuthSource::OpenClaw).unwrap();

    // The `:default` profile wins over the sibling `openai:work` profile.
    let tokens = load_openai_oauth_tokens().expect("oauth tokens imported");
    assert_eq!(tokens.access_token, "openclaw-access");
    assert_eq!(
        load_api_key_for_env("OPENROUTER_API_KEY").as_deref(),
        Some("sk-or-openclaw")
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn openclaw_legacy_flat_auth_json_still_wins_when_present() {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    // Both layouts exist: the original pi-fork path must take precedence so
    // previously-recorded trust decisions stay bound to the same file.
    let legacy_path = crate::storage::user_home_path(".openclaw/agent/auth.json").unwrap();
    write_auth_file(
        &legacy_path,
        serde_json::json!({
            "anthropic": { "type": "api_key", "key": "sk-ant-legacy" }
        }),
    );
    let profiles_path =
        crate::storage::user_home_path(".openclaw/agents/main/agent/auth-profiles.json").unwrap();
    write_auth_file(
        &profiles_path,
        serde_json::json!({ "version": 1, "profiles": {} }),
    );

    assert_eq!(ExternalAuthSource::OpenClaw.path().unwrap(), legacy_path);

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
