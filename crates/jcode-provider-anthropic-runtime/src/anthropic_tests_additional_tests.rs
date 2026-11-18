use super::*;

#[test]
fn direct_api_url_supports_standard_and_profile_overrides() {
    let _lock = jcode_base::storage::lock_test_env();
    let _standard = EnvVarGuard::set("ANTHROPIC_BASE_URL", "https://proxy.example/v1/");
    assert_eq!(direct_api_url(), "https://proxy.example/v1/messages");

    let _profile = EnvVarGuard::set(
        "JCODE_ANTHROPIC_API_BASE",
        "https://gateway.example/anthropic/v1/messages",
    );
    assert_eq!(
        direct_api_url(),
        "https://gateway.example/anthropic/v1/messages"
    );
}

#[test]
fn configured_direct_headers_parse_and_reject_invalid_values() {
    let _lock = jcode_base::storage::lock_test_env();
    let _headers = EnvVarGuard::set(
        "JCODE_ANTHROPIC_HEADERS",
        r#"{"x-tenant":"alpha","x-route":"claude"}"#,
    );
    let parsed = configured_direct_headers().expect("valid custom headers");
    assert_eq!(parsed.get("x-tenant").unwrap(), "alpha");
    assert_eq!(parsed.get("x-route").unwrap(), "claude");

    let _invalid = EnvVarGuard::set("JCODE_ANTHROPIC_HEADERS", r#"{"bad header":"x"}"#);
    assert!(configured_direct_headers().is_err());
}

#[test]
fn anthropic_auth_token_selects_bearer_without_affecting_explicit_profile_auth() {
    let _lock = jcode_base::storage::lock_test_env();
    let _token = EnvVarGuard::set("ANTHROPIC_AUTH_TOKEN", "gateway-token");
    assert_eq!(direct_auth_mode(), "bearer");

    let _explicit = EnvVarGuard::set("JCODE_ANTHROPIC_AUTH", "header");
    assert_eq!(direct_auth_mode(), "header");
}

#[test]
fn named_profile_runtime_captures_transport_and_credential_immutably() {
    let _lock = jcode_base::storage::lock_test_env();
    let _base = EnvVarGuard::set("JCODE_ANTHROPIC_API_BASE", "https://one.example/v1");
    let _auth = EnvVarGuard::set("JCODE_ANTHROPIC_AUTH", "bearer");
    let _key_name = EnvVarGuard::set("JCODE_ANTHROPIC_API_KEY_NAME", "PROFILE_ONE_KEY");
    let _key = EnvVarGuard::set("PROFILE_ONE_KEY", "one-secret");
    let provider = AnthropicProvider::new();

    let _changed_base = EnvVarGuard::set("JCODE_ANTHROPIC_API_BASE", "https://two.example/v1");
    let _changed_key = EnvVarGuard::set("PROFILE_ONE_KEY", "two-secret");
    assert_eq!(
        provider.direct_transport.api_url,
        "https://one.example/v1/messages"
    );
    assert_eq!(provider.direct_transport.auth_mode, "bearer");
    assert_eq!(
        provider.profile_api_key.as_ref().unwrap().as_ref().unwrap(),
        "one-secret"
    );
}

#[test]
fn named_anthropic_profile_accepts_its_configured_custom_model() {
    let _lock = jcode_base::storage::lock_test_env();
    let _home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", _home.path());
    std::fs::write(
        _home.path().join("config.toml"),
        r#"
        [providers.custom]
        type = "anthropic-compatible"
        base_url = "http://localhost:12345/v1"
        default_model = "claude-private"
        "#,
    )
    .expect("write config");
    jcode_base::config::Config::invalidate_cache();
    let _profile = EnvVarGuard::set("JCODE_NAMED_PROVIDER_PROFILE", "custom");
    let models = active_anthropic_profile_models().expect("active profile models");
    assert!(models.iter().any(|model| model == "claude-private"));
    assert!(!models.iter().any(|model| model == "not-configured"));
    drop(_profile);
    drop(_home_guard);
    jcode_base::config::Config::invalidate_cache();
}
