use super::*;
use tempfile::TempDir;

fn write_auth_file(path: &std::path::Path, value: serde_json::Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, serde_json::to_string(&value).unwrap()).unwrap();
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
