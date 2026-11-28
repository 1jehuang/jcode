use super::provider_catalog_tests::EnvGuard;
use super::*;

#[test]
fn merged_header_overrides_fold_user_agent_and_apply_profile_precedence() {
    let mut global = crate::config::ProviderConfig {
        user_agent: Some("global-agent/1.0".to_string()),
        ..crate::config::ProviderConfig::default()
    };
    global
        .headers
        .insert("x-tenant-id".to_string(), "global-tenant".to_string());
    global
        .headers
        .insert("x-route".to_string(), "openai".to_string());

    let mut profile = crate::config::NamedProviderConfig::default();
    profile
        .headers
        .insert("x-route".to_string(), "anthropic".to_string());

    let merged = merged_provider_header_overrides(&global, Some(&profile));
    assert_eq!(
        merged.get("User-Agent").map(String::as_str),
        Some("global-agent/1.0")
    );
    assert_eq!(
        merged.get("x-tenant-id").map(String::as_str),
        Some("global-tenant")
    );
    assert_eq!(merged.get("x-route").map(String::as_str), Some("anthropic"));

    // A profile user_agent beats the global one.
    profile.user_agent = Some("profile-agent/2.0".to_string());
    let merged = merged_provider_header_overrides(&global, Some(&profile));
    assert_eq!(
        merged.get("User-Agent").map(String::as_str),
        Some("profile-agent/2.0")
    );
    // profile user_agent is applied after profile headers, so it also beats a
    // profile headers["User-Agent"] entry.
    profile
        .headers
        .insert("User-Agent".to_string(), "header-map-agent/3.0".to_string());
    let merged = merged_provider_header_overrides(&global, Some(&profile));
    assert_eq!(
        merged.get("User-Agent").map(String::as_str),
        Some("profile-agent/2.0")
    );

    // No profile: global-only resolution.
    let merged = merged_provider_header_overrides(&global, None);
    assert_eq!(
        merged.get("User-Agent").map(String::as_str),
        Some("global-agent/1.0")
    );
    assert_eq!(merged.get("x-route").map(String::as_str), Some("openai"));
}

#[test]
fn merged_header_overrides_ignore_header_name_case() {
    let mut global = crate::config::ProviderConfig {
        user_agent: Some("global-field-agent/1.0".to_string()),
        ..crate::config::ProviderConfig::default()
    };
    global
        .headers
        .insert("x-route".to_string(), "global-table".to_string());

    // Profile writes header names in different case than the global table.
    // The profile level must still win per header name, and the profile
    // user_agent field must beat the profile's own lowercase
    // headers["user-agent"] entry.
    let mut profile = crate::config::NamedProviderConfig::default();
    profile
        .headers
        .insert("X-ROUTE".to_string(), "profile-table".to_string());
    profile.headers.insert(
        "user-agent".to_string(),
        "profile-table-agent/2.0".to_string(),
    );
    profile.user_agent = Some("profile-field-agent/3.0".to_string());

    let merged = merged_provider_header_overrides(&global, Some(&profile));
    assert_eq!(
        merged.get("User-Agent").map(String::as_str),
        Some("profile-field-agent/3.0")
    );
    let x_route = merged
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("x-route"))
        .map(|(_, value)| value.as_str());
    assert_eq!(x_route, Some("profile-table"));
    // Exactly one entry survives per header name.
    let x_route_count = merged
        .keys()
        .filter(|name| name.eq_ignore_ascii_case("x-route"))
        .count();
    assert_eq!(x_route_count, 1);
    let ua_count = merged
        .keys()
        .filter(|name| name.eq_ignore_ascii_case("user-agent"))
        .count();
    assert_eq!(ua_count, 1);

    // Within the global level, the field beats the lowercase table entry too.
    global.headers.insert(
        "user-agent".to_string(),
        "global-table-agent/9.0".to_string(),
    );
    let merged = merged_provider_header_overrides(&global, None);
    assert_eq!(
        merged.get("User-Agent").map(String::as_str),
        Some("global-field-agent/1.0")
    );
    let ua_count = merged
        .keys()
        .filter(|name| name.eq_ignore_ascii_case("user-agent"))
        .count();
    assert_eq!(ua_count, 1);
}

#[test]
fn merged_header_overrides_skip_blank_values() {
    let global = crate::config::ProviderConfig {
        user_agent: Some("   ".to_string()),
        ..crate::config::ProviderConfig::default()
    };
    let merged = merged_provider_header_overrides(&global, None);
    assert!(!merged.contains_key("User-Agent"));

    let profile = crate::config::NamedProviderConfig {
        user_agent: Some(String::new()),
        ..crate::config::NamedProviderConfig::default()
    };
    let merged = merged_provider_header_overrides(&global, Some(&profile));
    assert!(!merged.contains_key("User-Agent"));
}

#[test]
fn parse_provider_header_overrides_rejects_invalid_headers() {
    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert("x-valid".to_string(), "ok".to_string());
    assert!(parse_provider_header_overrides(&overrides).is_ok());

    let mut invalid_name = overrides.clone();
    invalid_name.insert("bad header".to_string(), "x".to_string());
    assert!(parse_provider_header_overrides(&invalid_name).is_err());

    let mut invalid_value = overrides.clone();
    invalid_value.insert("x-control".to_string(), "bad\u{7}value".to_string());
    assert!(parse_provider_header_overrides(&invalid_value).is_err());
}

#[test]
fn resolved_provider_http_header_overrides_labels_errors_by_source() {
    let global = crate::config::ProviderConfig::default();
    let mut profile = crate::config::NamedProviderConfig::default();
    profile
        .headers
        .insert("bad header".to_string(), "x".to_string());

    let error = resolved_provider_http_header_overrides(&global, Some(&profile), "acme")
        .expect_err("invalid header must error");
    let message = format!("{error:#}");
    assert!(
        message.contains("acme has invalid header overrides"),
        "error must name the source: {message}"
    );

    let mut global_bad = crate::config::ProviderConfig::default();
    global_bad
        .headers
        .insert("bad header".to_string(), "x".to_string());
    let error = resolved_provider_http_header_overrides(&global_bad, None, "[provider]")
        .expect_err("invalid global header must error");
    assert!(format!("{error:#}").contains("[provider] has invalid header overrides"));
}

#[test]
fn anthropic_bridge_serializes_merged_headers_with_profile_precedence() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_HOME",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_ANTHROPIC_API_BASE",
        "JCODE_ANTHROPIC_API_KEY_NAME",
        "JCODE_ANTHROPIC_AUTH",
        "JCODE_ANTHROPIC_AUTH_HEADER",
        "JCODE_ANTHROPIC_HEADERS",
        "JCODE_ANTHROPIC_MODEL",
        "JCODE_RUNTIME_PROVIDER",
    ]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::config::Config::invalidate_cache();

    let config_path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
        [provider]
        user_agent = "global-agent/1.0"

        [provider.headers]
        x-tenant-id = "global-tenant"
        x-route = "global-route"

        [providers.corporate-claude]
        type = "anthropic-compatible"
        base_url = "https://gateway.example.com/anthropic/v1/"
        auth = "bearer"
        api_key_env = "CORPORATE_CLAUDE_TOKEN"
        default_model = "claude-custom"
        user_agent = "profile-agent/2.0"

        [providers.corporate-claude.headers]
        x-route = "profile-route"
        x-extra = "extra"
        "#,
    )
    .expect("write config");

    apply_named_provider_profile_env("corporate-claude").expect("apply Anthropic profile");

    let headers: std::collections::BTreeMap<String, String> = serde_json::from_str(
        &std::env::var("JCODE_ANTHROPIC_HEADERS").expect("custom headers env"),
    )
    .expect("headers JSON");
    assert_eq!(
        headers.get("User-Agent").map(String::as_str),
        Some("profile-agent/2.0")
    );
    assert_eq!(
        headers.get("x-route").map(String::as_str),
        Some("profile-route")
    );
    assert_eq!(
        headers.get("x-tenant-id").map(String::as_str),
        Some("global-tenant")
    );
    assert_eq!(headers.get("x-extra").map(String::as_str), Some("extra"));

    crate::config::Config::invalidate_cache();
}

#[test]
fn anthropic_bridge_removes_headers_env_when_no_overrides_apply() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvGuard::save(&[
        "JCODE_HOME",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_ANTHROPIC_API_BASE",
        "JCODE_ANTHROPIC_API_KEY_NAME",
        "JCODE_ANTHROPIC_AUTH",
        "JCODE_ANTHROPIC_AUTH_HEADER",
        "JCODE_ANTHROPIC_HEADERS",
        "JCODE_ANTHROPIC_MODEL",
        "JCODE_RUNTIME_PROVIDER",
    ]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::config::Config::invalidate_cache();

    let config_path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    std::fs::write(
        &config_path,
        r#"
        [providers.corporate-claude]
        type = "anthropic-compatible"
        base_url = "https://gateway.example.com/anthropic/v1/"
        auth = "bearer"
        api_key_env = "CORPORATE_CLAUDE_TOKEN"
        default_model = "claude-custom"
        "#,
    )
    .expect("write config");

    crate::env::set_var("JCODE_ANTHROPIC_HEADERS", r#"{"stale":"yes"}"#);
    apply_named_provider_profile_env("corporate-claude").expect("apply Anthropic profile");
    assert!(std::env::var_os("JCODE_ANTHROPIC_HEADERS").is_none());

    crate::config::Config::invalidate_cache();
}
