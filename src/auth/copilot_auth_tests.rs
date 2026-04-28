use super::*;
use anyhow::{Result, anyhow};

use tempfile::TempDir;

#[test]
fn copilot_api_token_not_expired() {
    let future_ts = chrono::Utc::now().timestamp() + 3600;
    let token = CopilotApiToken {
        token: "test-token".to_string(),
        expires_at: future_ts,
    };
    assert!(!token.is_expired());
}

#[test]
fn copilot_api_token_expired() {
    let past_ts = chrono::Utc::now().timestamp() - 100;
    let token = CopilotApiToken {
        token: "test-token".to_string(),
        expires_at: past_ts,
    };
    assert!(token.is_expired());
}

#[test]
fn copilot_api_token_expiring_within_buffer() {
    let almost_ts = chrono::Utc::now().timestamp() + 30;
    let token = CopilotApiToken {
        token: "test-token".to_string(),
        expires_at: almost_ts,
    };
    assert!(token.is_expired());
}

#[test]
fn load_token_from_hosts_json() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let hosts_path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "github.com": {
            "oauth_token": "gho_testtoken123",
            "user": "testuser"
        }
    });
    std::fs::write(&hosts_path, serde_json::to_string(&data)?)?;

    let token = load_token_from_json(&hosts_path.to_path_buf())?;
    assert_eq!(token, "gho_testtoken123");
    Ok(())
}

#[test]
fn load_token_from_apps_json() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let apps_path = dir.path().join("apps.json");
    let data = serde_json::json!({
        "github.com": {
            "oauth_token": "ghu_vscodetoken456"
        }
    });
    std::fs::write(&apps_path, serde_json::to_string(&data)?)?;

    let token = load_token_from_json(&apps_path.to_path_buf())?;
    assert_eq!(token, "ghu_vscodetoken456");
    Ok(())
}

#[test]
fn load_token_missing_oauth_token_field() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "github.com": {
            "user": "testuser"
        }
    });
    std::fs::write(&path, serde_json::to_string(&data)?)?;

    let result = load_token_from_json(&path.to_path_buf());
    assert!(result.is_err());
    Ok(())
}

#[test]
fn load_token_empty_oauth_token() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "github.com": {
            "oauth_token": "",
            "user": "testuser"
        }
    });
    std::fs::write(&path, serde_json::to_string(&data)?)?;

    let result = load_token_from_json(&path.to_path_buf());
    assert!(result.is_err());
    Ok(())
}

#[test]
fn load_token_nonexistent_file() {
    let path = PathBuf::from("/tmp/nonexistent_auth_test_file.json");
    let result = load_token_from_json(&path);
    assert!(result.is_err());
}

#[test]
fn load_token_invalid_json() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    std::fs::write(&path, "not valid json{{{")?;

    let result = load_token_from_json(&path.to_path_buf());
    assert!(result.is_err());
    Ok(())
}

#[test]
fn load_token_from_copilot_config_json() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "auth": {
                "token": "ghu_config_token"
            }
        })
        .to_string(),
    )?;

    let token = load_token_from_config_json(&path)?;
    assert_eq!(token, "ghu_config_token");
    Ok(())
}

#[test]
fn normalize_candidate_token_rejects_empty_and_unknown_values() {
    assert_eq!(
        normalize_candidate_token("gho_valid"),
        Some("gho_valid".to_string())
    );
    assert_eq!(
        normalize_candidate_token("ghu_valid"),
        Some("ghu_valid".to_string())
    );
    assert_eq!(
        normalize_candidate_token("github_pat_valid"),
        Some("github_pat_valid".to_string())
    );
    assert_eq!(normalize_candidate_token("ghp_classic"), None);
    assert_eq!(normalize_candidate_token("   "), None);
}

#[test]
fn save_and_load_github_token() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let config_dir = dir.path().join("github-copilot");
    std::fs::create_dir_all(&config_dir)?;

    let hosts_path = config_dir.join("hosts.json");

    let mut config: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut entry = HashMap::new();
    entry.insert("user".to_string(), "testuser".to_string());
    entry.insert("oauth_token".to_string(), "gho_saved_token".to_string());
    config.insert("github.com".to_string(), entry);

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(&hosts_path, json)?;

    let loaded = load_token_from_json(&hosts_path.to_path_buf())?;
    assert_eq!(loaded, "gho_saved_token");
    Ok(())
}

#[test]
fn save_github_token_creates_config_dir() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let config_dir = dir.path().join("github-copilot");
    let prev_jcode_home = std::env::var_os("JCODE_HOME");
    let prev_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");

    crate::env::remove_var("JCODE_HOME");
    crate::env::set_var(
        "XDG_CONFIG_HOME",
        dir.path()
            .to_str()
            .ok_or_else(|| anyhow!("temp dir path should be valid UTF-8"))?,
    );

    let result = save_github_token("gho_newtoken", "testuser");
    assert!(result.is_ok());

    let hosts_path = config_dir.join("hosts.json");
    assert!(hosts_path.exists());

    let loaded = load_token_from_json(&hosts_path)?;
    assert_eq!(loaded, "gho_newtoken");

    if let Some(prev) = prev_jcode_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }

    if let Some(prev) = prev_xdg_config_home {
        crate::env::set_var("XDG_CONFIG_HOME", prev);
    } else {
        crate::env::remove_var("XDG_CONFIG_HOME");
    }
    Ok(())
}

#[test]
fn legacy_copilot_config_dir_uses_jcode_home_external_dir() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = legacy_copilot_config_dir();
    assert_eq!(
        path,
        dir.path()
            .join("external")
            .join(".config")
            .join("github-copilot")
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    Ok(())
}

#[test]
fn save_github_token_makes_future_loads_available() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_jcode_home = std::env::var_os("JCODE_HOME");
    let prev_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");

    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::remove_var("XDG_CONFIG_HOME");

    save_github_token("gho_persisted_token", "testuser")?;

    let hosts_path = ExternalCopilotAuthSource::HostsJson.path();
    assert!(
        crate::config::Config::external_auth_source_allowed_for_path(
            COPILOT_HOSTS_AUTH_SOURCE_ID,
            &hosts_path
        )
    );
    assert_eq!(load_github_token()?, "gho_persisted_token");

    if let Some(prev) = prev_jcode_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }

    if let Some(prev) = prev_xdg_config_home {
        crate::env::set_var("XDG_CONFIG_HOME", prev);
    } else {
        crate::env::remove_var("XDG_CONFIG_HOME");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn load_token_from_json_does_not_change_external_permissions() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    std::fs::write(
        &path,
        r#"{"github.com":{"oauth_token":"gho_test","user":"tester"}}"#,
    )?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;

    let token = load_token_from_json(&path)?;
    assert_eq!(token, "gho_test");

    let dir_mode = std::fs::metadata(dir.path())?.permissions().mode() & 0o777;
    let file_mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o755);
    assert_eq!(file_mode, 0o644);
    Ok(())
}

#[test]
fn choose_default_model_with_opus() {
    let models = vec![
        CopilotModelInfo {
            id: "claude-sonnet-4".to_string(),
            name: String::new(),
            vendor: String::new(),
            version: String::new(),
            model_picker_enabled: false,
            capabilities: Default::default(),
        },
        CopilotModelInfo {
            id: "claude-opus-4.6".to_string(),
            name: String::new(),
            vendor: String::new(),
            version: String::new(),
            model_picker_enabled: false,
            capabilities: Default::default(),
        },
    ];
    assert_eq!(choose_default_model(&models), "claude-opus-4.6");
}

#[test]
fn choose_default_model_without_opus() {
    let models = vec![CopilotModelInfo {
        id: "claude-sonnet-4.6".to_string(),
        name: String::new(),
        vendor: String::new(),
        version: String::new(),
        model_picker_enabled: false,
        capabilities: Default::default(),
    }];
    assert_eq!(choose_default_model(&models), "claude-sonnet-4.6");
}

#[test]
fn choose_default_model_with_sonnet_4_only() {
    let models = vec![CopilotModelInfo {
        id: "claude-sonnet-4".to_string(),
        name: String::new(),
        vendor: String::new(),
        version: String::new(),
        model_picker_enabled: false,
        capabilities: Default::default(),
    }];
    assert_eq!(choose_default_model(&models), "claude-sonnet-4");
}

#[test]
fn choose_default_model_empty_list() {
    let models: Vec<CopilotModelInfo> = vec![];
    assert_eq!(choose_default_model(&models), "claude-sonnet-4");
}

#[test]
fn copilot_account_type_display() {
    assert_eq!(CopilotAccountType::Individual.to_string(), "individual");
    assert_eq!(CopilotAccountType::Business.to_string(), "business");
    assert_eq!(CopilotAccountType::Enterprise.to_string(), "enterprise");
    assert_eq!(CopilotAccountType::Unknown.to_string(), "unknown");
}

#[test]
fn device_code_response_deserialize() -> Result<()> {
    let json = r#"{
            "device_code": "dc_1234",
            "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5
        }"#;
    let resp: DeviceCodeResponse = serde_json::from_str(json)?;
    assert_eq!(resp.device_code, "dc_1234");
    assert_eq!(resp.user_code, "ABCD-1234");
    assert_eq!(resp.verification_uri, "https://github.com/login/device");
    assert_eq!(resp.expires_in, 900);
    assert_eq!(resp.interval, 5);
    Ok(())
}

#[test]
fn access_token_response_success() -> Result<()> {
    let json = r#"{
            "access_token": "gho_xxx123",
            "token_type": "bearer",
            "scope": "read:user"
        }"#;
    let resp: AccessTokenResponse = serde_json::from_str(json)?;
    assert_eq!(
        resp.access_token
            .ok_or_else(|| anyhow!("missing access token"))?,
        "gho_xxx123"
    );
    assert!(resp.error.is_none());
    Ok(())
}

#[test]
fn access_token_response_pending() -> Result<()> {
    let json = r#"{
            "error": "authorization_pending",
            "error_description": "The authorization request is still pending."
        }"#;
    let resp: AccessTokenResponse = serde_json::from_str(json)?;
    assert!(resp.access_token.is_none());
    assert_eq!(
        resp.error.ok_or_else(|| anyhow!("missing error"))?,
        "authorization_pending"
    );
    Ok(())
}

#[test]
fn access_token_response_expired() -> Result<()> {
    let json = r#"{
            "error": "expired_token",
            "error_description": "The device code has expired."
        }"#;
    let resp: AccessTokenResponse = serde_json::from_str(json)?;
    assert_eq!(
        resp.error.ok_or_else(|| anyhow!("missing error"))?,
        "expired_token"
    );
    Ok(())
}

#[test]
fn copilot_token_response_roundtrip() -> Result<()> {
    let resp = CopilotTokenResponse {
        token: "bearer_token_xxx".to_string(),
        expires_at: 1700000000,
    };
    let json = serde_json::to_string(&resp)?;
    let parsed: CopilotTokenResponse = serde_json::from_str(&json)?;
    assert_eq!(parsed.token, "bearer_token_xxx");
    assert_eq!(parsed.expires_at, 1700000000);
    Ok(())
}

#[test]
fn copilot_model_info_deserialize() -> Result<()> {
    let json = r#"{
            "id": "claude-sonnet-4",
            "name": "Claude Sonnet 4",
            "vendor": "anthropic",
            "version": "2025-01-01",
            "model_picker_enabled": true,
            "capabilities": {
                "type": "chat",
                "family": "claude-sonnet-4"
            }
        }"#;
    let model: CopilotModelInfo = serde_json::from_str(json)?;
    assert_eq!(model.id, "claude-sonnet-4");
    assert_eq!(model.vendor, "anthropic");
    assert!(model.model_picker_enabled);
    Ok(())
}

#[test]
fn copilot_model_info_minimal() -> Result<()> {
    let json = r#"{"id": "gpt-4o"}"#;
    let model: CopilotModelInfo = serde_json::from_str(json)?;
    assert_eq!(model.id, "gpt-4o");
    assert_eq!(model.name, "");
    assert!(!model.model_picker_enabled);
    Ok(())
}

#[test]
fn load_token_multiple_hosts() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "api.github.com": {
            "oauth_token": "gho_api",
            "user": "user1"
        },
        "github.com": {
            "oauth_token": "gho_primary",
            "user": "user2"
        },
        "https://github.com/extra/path": {
            "oauth_token": "gho_path",
            "user": "user3"
        }
    });
    std::fs::write(&path, serde_json::to_string(&data)?)?;

    let token = load_token_from_json(&path.to_path_buf())?;
    assert_eq!(token, "gho_primary");
    Ok(())
}

#[test]
fn normalize_github_host_key_accepts_common_forms() {
    assert_eq!(
        normalize_github_host_key("https://github.com/login"),
        Some("github.com".to_string())
    );
    assert_eq!(
        normalize_github_host_key("api.github.com"),
        Some("api.github.com".to_string())
    );
    assert_eq!(
        normalize_github_host_key("sub.github.com/path"),
        Some("sub.github.com".to_string())
    );
}

#[test]
fn normalize_github_host_key_rejects_non_github_hosts() {
    assert_eq!(normalize_github_host_key("gitlab.com"), None);
    assert_eq!(normalize_github_host_key(""), None);
}

#[test]
fn copilot_exchange_404_explains_missing_subscription() {
    let body = r#"{"message":"Not Found","documentation_url":"https://docs.github.com/rest","status":"404"}"#;
    let msg = format_copilot_exchange_failure(reqwest::StatusCode::NOT_FOUND, body);
    assert!(
        msg.contains("does not have an active Copilot subscription"),
        "expected subscription guidance, got: {msg}"
    );
    // The failover machinery scans for these substrings; keep them stable.
    assert!(
        msg.contains("Copilot token exchange failed"),
        "expected canonical failure prefix, got: {msg}"
    );
    assert!(
        msg.contains("404"),
        "expected standalone 404 marker for failover routing, got: {msg}"
    );
}

#[test]
fn copilot_exchange_404_without_not_found_body_falls_back_to_generic() {
    let msg = format_copilot_exchange_failure(reqwest::StatusCode::NOT_FOUND, "weird body");
    assert!(
        !msg.contains("does not have an active Copilot subscription"),
        "should not assert subscription cause without GitHub's not-found body: {msg}"
    );
    assert!(msg.contains("404"), "{msg}");
    assert!(msg.contains("weird body"), "{msg}");
}

#[test]
fn copilot_exchange_other_status_uses_generic_format() {
    let body = r#"{"error":"something else"}"#;
    let msg = format_copilot_exchange_failure(reqwest::StatusCode::FORBIDDEN, body);
    assert!(msg.contains("403"), "{msg}");
    assert!(msg.contains("something else"), "{msg}");
    assert!(
        !msg.contains("does not have an active Copilot subscription"),
        "{msg}"
    );
}

#[test]
fn copilot_exchange_empty_body_is_handled() {
    let msg = format_copilot_exchange_failure(reqwest::StatusCode::BAD_GATEWAY, "");
    assert!(msg.contains("502"), "{msg}");
    assert!(msg.contains("(empty body)"), "{msg}");
}
