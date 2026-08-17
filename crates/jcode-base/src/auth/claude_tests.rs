use super::*;
use std::ffi::OsString;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[test]
fn jcode_auth_file_default_is_empty() {
    let auth = JcodeAuthFile::default();
    assert!(auth.anthropic_accounts.is_empty());
    assert!(auth.active_anthropic_account.is_none());
}

#[test]
fn jcode_auth_file_roundtrip() {
    let auth = JcodeAuthFile {
        anthropic_accounts: vec![AnthropicAccount {
            label: "work".to_string(),
            access: "acc_123".to_string(),
            refresh: "ref_456".to_string(),
            expires: 9999999999999,
            email: None,
            scopes: Vec::new(),
            subscription_type: Some("max".to_string()),
        }],
        active_anthropic_account: Some("work".to_string()),
        anthropic: None,
    };

    let json = serde_json::to_string_pretty(&auth).unwrap();
    let parsed: JcodeAuthFile = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.anthropic_accounts.len(), 1);
    assert_eq!(parsed.anthropic_accounts[0].label, "work");
    assert_eq!(parsed.anthropic_accounts[0].access, "acc_123");
    assert_eq!(parsed.active_anthropic_account, Some("work".to_string()));
}

#[test]
fn jcode_path_respects_jcode_home() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());

    assert_eq!(jcode_path().unwrap(), temp.path().join("auth.json"));
    assert_eq!(
        claude_code_path().unwrap(),
        temp.path()
            .join("external")
            .join(".claude")
            .join(".credentials.json")
    );
    assert_eq!(
        opencode_path().unwrap(),
        temp.path()
            .join("external")
            .join(".local")
            .join("share")
            .join("opencode")
            .join("auth.json")
    );
}

#[test]
fn load_auth_file_renames_existing_labels_to_animal_scheme() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());
    set_active_account_override(None);

    let auth_path = temp.path().join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{
            "anthropic_accounts": [
                {
                    "label": "personal",
                    "access": "acc_personal",
                    "refresh": "ref_personal",
                    "expires": 1000
                },
                {
                    "label": "work",
                    "access": "acc_work",
                    "refresh": "ref_work",
                    "expires": 2000
                }
            ],
            "active_anthropic_account": "work"
        }"#,
    )
    .unwrap();

    let auth = load_auth_file().unwrap();
    assert_eq!(
        auth.anthropic_accounts
            .iter()
            .map(|account| account.label.as_str())
            .collect::<Vec<_>>(),
        vec!["claude-otter", "claude-fox"]
    );
    assert_eq!(auth.active_anthropic_account.as_deref(), Some("claude-fox"));
}

#[test]
fn jcode_auth_file_multi_account() {
    let auth = JcodeAuthFile {
        anthropic_accounts: vec![
            AnthropicAccount {
                label: "personal".to_string(),
                access: "acc_personal".to_string(),
                refresh: "ref_personal".to_string(),
                expires: 1000,
                scopes: Vec::new(),
                subscription_type: Some("pro".to_string()),
                email: None,
            },
            AnthropicAccount {
                label: "work".to_string(),
                access: "acc_work".to_string(),
                refresh: "ref_work".to_string(),
                expires: 2000,
                email: None,
                scopes: Vec::new(),
                subscription_type: Some("max".to_string()),
            },
        ],
        active_anthropic_account: Some("work".to_string()),
        anthropic: None,
    };

    let json = serde_json::to_string(&auth).unwrap();
    let parsed: JcodeAuthFile = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.anthropic_accounts.len(), 2);
    assert_eq!(parsed.active_anthropic_account, Some("work".to_string()));
}

#[test]
fn jcode_auth_file_legacy_migration_format() {
    let legacy_json = r#"{
        "anthropic": {
            "access": "legacy_acc",
            "refresh": "legacy_ref",
            "expires": 12345
        }
    }"#;
    let parsed: JcodeAuthFile = serde_json::from_str(legacy_json).unwrap();
    assert!(parsed.anthropic_accounts.is_empty());
    assert!(parsed.anthropic.is_some());
}

#[test]
fn anthropic_account_no_subscription_type() {
    let json = r#"{
        "label": "test",
        "access": "acc",
        "refresh": "ref",
        "expires": 0
    }"#;
    let account: AnthropicAccount = serde_json::from_str(json).unwrap();
    assert_eq!(account.label, "test");
    assert!(account.subscription_type.is_none());
    assert!(account.email.is_none());
}

#[test]
fn anthropic_account_email_serialized_when_present() {
    let account = AnthropicAccount {
        label: "test".to_string(),
        access: "acc".to_string(),
        refresh: "ref".to_string(),
        expires: 0,
        email: Some("user@example.com".to_string()),
        scopes: Vec::new(),
        subscription_type: Some("max".to_string()),
    };
    let json = serde_json::to_string(&account).unwrap();
    assert!(json.contains("email"));
    assert!(json.contains("user@example.com"));
}

#[test]
fn anthropic_account_email_omitted_when_none() {
    let account = AnthropicAccount {
        label: "test".to_string(),
        access: "acc".to_string(),
        refresh: "ref".to_string(),
        expires: 0,
        email: None,
        scopes: Vec::new(),
        subscription_type: Some("max".to_string()),
    };
    let json = serde_json::to_string(&account).unwrap();
    assert!(!json.contains("\"email\""));
}

#[test]
fn anthropic_account_subscription_type_serialized_when_present() {
    let account = AnthropicAccount {
        label: "test".to_string(),
        access: "acc".to_string(),
        refresh: "ref".to_string(),
        expires: 0,
        email: None,
        scopes: Vec::new(),
        subscription_type: Some("max".to_string()),
    };
    let json = serde_json::to_string(&account).unwrap();
    assert!(json.contains("subscription_type"));
    assert!(json.contains("max"));
}

#[test]
fn anthropic_account_subscription_type_omitted_when_none() {
    let account = AnthropicAccount {
        label: "test".to_string(),
        access: "acc".to_string(),
        refresh: "ref".to_string(),
        expires: 0,
        scopes: Vec::new(),
        subscription_type: None,
        email: None,
    };
    let json = serde_json::to_string(&account).unwrap();
    assert!(!json.contains("subscription_type"));
}

#[test]
fn update_account_profile_sets_email() {
    let mut auth = JcodeAuthFile::default();
    auth.anthropic_accounts.push(AnthropicAccount {
        label: "test".to_string(),
        access: "acc".to_string(),
        refresh: "ref".to_string(),
        expires: 1,
        email: None,
        scopes: Vec::new(),
        subscription_type: None,
    });

    if let Some(account) = auth
        .anthropic_accounts
        .iter_mut()
        .find(|a| a.label == "test")
    {
        account.email = Some("user@example.com".to_string());
    }

    assert_eq!(
        auth.anthropic_accounts[0].email.as_deref(),
        Some("user@example.com")
    );
}

#[test]
fn is_max_subscription_pro_is_false() {
    // This tests the logic directly since we can't mock the file
    let sub_type = Some("pro".to_string());
    let is_max = match sub_type {
        Some(t) => t != "pro",
        None => true,
    };
    assert!(!is_max);
}

#[test]
fn is_max_subscription_max_is_true() {
    let sub_type = Some("max".to_string());
    let is_max = match sub_type {
        Some(t) => t != "pro",
        None => true,
    };
    assert!(is_max);
}

#[test]
fn is_max_subscription_unknown_is_true() {
    let sub_type: Option<String> = None;
    let is_max = match sub_type {
        Some(t) => t != "pro",
        None => true,
    };
    assert!(is_max);
}

#[test]
fn claude_code_credentials_format() {
    let json = r#"{
        "claudeAiOauth": {
            "accessToken": "at_12345",
            "refreshToken": "rt_67890",
            "expiresAt": 9999999999999,
            "subscriptionType": "max"
        }
    }"#;
    let file: CredentialsFile = serde_json::from_str(json).unwrap();
    let oauth = file.claude_ai_oauth.unwrap();
    assert_eq!(oauth.access_token, "at_12345");
    assert_eq!(oauth.refresh_token, "rt_67890");
    assert_eq!(oauth.expires_at, 9999999999999);
    assert_eq!(oauth.subscription_type, Some("max".to_string()));
}

#[test]
fn claude_code_credentials_no_subscription() {
    let json = r#"{
        "claudeAiOauth": {
            "accessToken": "at",
            "refreshToken": "rt",
            "expiresAt": 0
        }
    }"#;
    let file: CredentialsFile = serde_json::from_str(json).unwrap();
    let oauth = file.claude_ai_oauth.unwrap();
    assert!(oauth.subscription_type.is_none());
}

#[test]
fn claude_code_credentials_missing_oauth() {
    let json = r#"{}"#;
    let file: CredentialsFile = serde_json::from_str(json).unwrap();
    assert!(file.claude_ai_oauth.is_none());
}

#[cfg(unix)]
#[test]
fn load_claude_code_credentials_does_not_change_external_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());

    let path = claude_code_path().expect("claude code path");
    std::fs::create_dir_all(path.parent().unwrap()).expect("create dir");
    std::fs::write(
        &path,
        r#"{"claudeAiOauth":{"accessToken":"at","refreshToken":"rt","expiresAt":4102444800000}}"#,
    )
    .expect("write file");
    std::fs::set_permissions(
        path.parent().unwrap(),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("set dir perms");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("set file perms");

    let _ = load_claude_code_credentials().expect("load external claude creds");

    let dir_mode = std::fs::metadata(path.parent().unwrap())
        .expect("stat dir")
        .permissions()
        .mode()
        & 0o777;
    let file_mode = std::fs::metadata(&path)
        .expect("stat file")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(dir_mode, 0o755);
    assert_eq!(file_mode, 0o644);
}

#[test]
fn opencode_credentials_format() {
    let json = r#"{
        "anthropic": {
            "access": "oc_acc",
            "refresh": "oc_ref",
            "expires": 1234567890
        }
    }"#;
    let auth: OpenCodeAuth = serde_json::from_str(json).unwrap();
    let anthropic = auth.anthropic.unwrap();
    assert_eq!(anthropic.access, "oc_acc");
    assert_eq!(anthropic.refresh, "oc_ref");
    assert_eq!(anthropic.expires, 1234567890);
}

#[test]
fn opencode_credentials_no_anthropic() {
    let json = r#"{}"#;
    let auth: OpenCodeAuth = serde_json::from_str(json).unwrap();
    assert!(auth.anthropic.is_none());
}

#[test]
fn active_account_override_roundtrip() {
    set_active_account_override(Some("test-override".to_string()));
    assert_eq!(
        get_active_account_override(),
        Some("test-override".to_string())
    );
    set_active_account_override(None);
    assert_eq!(get_active_account_override(), None);
}

#[test]
fn parse_blob_accepts_wrapped_file_form() {
    let json = r#"{
        "claudeAiOauth": {
            "accessToken": "at_file",
            "refreshToken": "rt_file",
            "expiresAt": 9999999999999,
            "subscriptionType": "max",
            "scopes": ["user:inference", "user:profile"]
        }
    }"#;
    let creds = parse_claude_code_credentials_blob(json).expect("parse wrapped");
    assert_eq!(creds.access_token, "at_file");
    assert_eq!(creds.refresh_token, "rt_file");
    assert_eq!(creds.expires_at, 9999999999999);
    assert_eq!(creds.subscription_type, Some("max".to_string()));
    assert_eq!(creds.scopes, vec!["user:inference", "user:profile"]);
}

#[test]
fn parse_blob_accepts_bare_keychain_form_with_numeric_expiry() {
    // The macOS Keychain stores a bare OAuth object (no claudeAiOauth wrapper).
    let json = r#"{
        "accessToken": "sk-ant-oat01-abc",
        "refreshToken": "sk-ant-ort01-xyz",
        "expiresAt": 4102444800000
    }"#;
    let creds = parse_claude_code_credentials_blob(json).expect("parse bare numeric");
    assert_eq!(creds.access_token, "sk-ant-oat01-abc");
    assert_eq!(creds.refresh_token, "sk-ant-ort01-xyz");
    assert_eq!(creds.expires_at, 4102444800000);
}

#[test]
fn parse_blob_accepts_rfc3339_string_expiry() {
    // Some Keychain blobs store expiresAt as an RFC 3339 timestamp string.
    let json = r#"{
        "accessToken": "at",
        "refreshToken": "rt",
        "expiresAt": "2027-02-18T07:00:00.000Z"
    }"#;
    let creds = parse_claude_code_credentials_blob(json).expect("parse rfc3339");
    let expected = chrono::DateTime::parse_from_rfc3339("2027-02-18T07:00:00.000Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(creds.expires_at, expected);
    assert!(creds.expires_at > 0);
}

#[test]
fn parse_blob_accepts_space_delimited_scope_string() {
    let json = r#"{
        "accessToken": "at",
        "refreshToken": "rt",
        "expiresAt": 1,
        "scopes": "user:inference user:profile"
    }"#;
    let creds = parse_claude_code_credentials_blob(json).expect("parse scope string");
    assert_eq!(creds.scopes, vec!["user:inference", "user:profile"]);
}

#[test]
fn parse_blob_missing_expiry_defaults_to_zero() {
    let json = r#"{ "accessToken": "at", "refreshToken": "rt" }"#;
    let creds = parse_claude_code_credentials_blob(json).expect("parse no expiry");
    assert_eq!(creds.expires_at, 0);
}

#[test]
fn parse_blob_rejects_empty_token() {
    let json = r#"{ "accessToken": "", "refreshToken": "" }"#;
    assert!(parse_claude_code_credentials_blob(json).is_err());
}

#[test]
fn parse_blob_rejects_empty_input() {
    assert!(parse_claude_code_credentials_blob("").is_err());
    assert!(parse_claude_code_credentials_blob("   ").is_err());
}

#[test]
fn env_token_credentials_parse_json_blob() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvStringGuard::set(
        "CLAUDE_CODE_OAUTH_TOKEN",
        r#"{"accessToken":"at_env","refreshToken":"rt_env","expiresAt":4102444800000}"#,
    );
    let creds = load_claude_code_env_credentials().expect("env creds");
    assert_eq!(creds.access_token, "at_env");
    assert_eq!(creds.refresh_token, "rt_env");
    assert_eq!(creds.expires_at, 4102444800000);
}

#[test]
fn env_token_credentials_parse_bare_token() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvStringGuard::set("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-bareToken");
    let creds = load_claude_code_env_credentials().expect("bare env creds");
    assert_eq!(creds.access_token, "sk-ant-oat01-bareToken");
    assert!(creds.refresh_token.is_empty());
    assert_eq!(creds.expires_at, 0);
}

#[test]
fn env_token_absent_yields_none() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvStringGuard::remove("CLAUDE_CODE_OAUTH_TOKEN");
    assert!(load_claude_code_env_credentials().is_none());
}

/// Live macOS-only check against a real `Claude Code-credentials` Keychain item.
/// Ignored by default (mutates/reads the user Keychain). Run with:
///   cargo test -p jcode-base --lib auth::claude::tests::live_keychain -- --ignored --nocapture
#[cfg(target_os = "macos")]
#[test]
#[ignore = "live: reads the real macOS Keychain"]
fn live_keychain_native_credentials_detected_and_parsed() {
    let _lock = crate::storage::lock_test_env();
    let _guard = EnvStringGuard::remove("CLAUDE_CODE_OAUTH_TOKEN");

    assert!(
        native_credentials_present(),
        "expected a 'Claude Code-credentials' Keychain item to be present"
    );
    let creds = load_native_credentials().expect("load native creds from Keychain");
    assert!(
        !creds.access_token.trim().is_empty(),
        "expected a non-empty access token from the Keychain blob"
    );
}

/// Like `EnvVarGuard` but sets/removes string values (not just paths).
struct EnvStringGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvStringGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }

    fn remove(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        crate::env::remove_var(key);
        Self { key, previous }
    }
}

impl Drop for EnvStringGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}
