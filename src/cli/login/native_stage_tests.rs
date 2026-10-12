use super::*;

struct NativeStageFixture {
    _temp: tempfile::TempDir,
    file: std::path::PathBuf,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    active_override: Option<String>,
}

impl NativeStageFixture {
    fn new(trusted_native: bool) -> Self {
        let temp = tempfile::TempDir::new().unwrap();
        let previous = ["JCODE_HOME", "CLAUDE_CONFIG_DIR", "CLAUDE_CODE_OAUTH_TOKEN"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        let active_override = auth::claude::get_active_account_override();
        auth::claude::set_active_account_override(None);
        crate::env::set_var("JCODE_HOME", temp.path());
        let config_dir = temp.path().join("claude-config");
        crate::env::set_var("CLAUDE_CONFIG_DIR", &config_dir);
        // Never call native acquisition: the production snapshot boundary below
        // receives synthetic credentials directly, including on macOS.
        crate::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        std::fs::create_dir_all(&config_dir).unwrap();
        let file = config_dir.join(".credentials.json");
        Self::write_file(&file, "expired-file", 1);
        crate::config::Config::allow_external_auth_source_for_path(
            auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
            &file,
        )
        .unwrap();
        if trusted_native {
            auth::claude::trust_native_source().unwrap();
        }
        Self::save_account("account-a");
        Self {
            _temp: temp,
            file,
            previous,
            active_override,
        }
    }

    fn write_file(file: &Path, token: &str, expires: i64) {
        let value = serde_json::json!({"claudeAiOauth": {
            "accessToken": token, "refreshToken": format!("{token}-refresh"),
            "expiresAt": expires
        }});
        std::fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn save_account(label: &str) {
        let saved_label = auth::claude::upsert_account(auth::claude::AnthropicAccount {
            label: label.into(),
            access: format!("{label}-access"),
            refresh: format!("{label}-refresh"),
            expires: 4102444800000,
            email: None,
            subscription_type: None,
            scopes: vec![],
        })
        .unwrap();
        auth::claude::set_active_account(&saved_label).unwrap();
    }

    fn snapshot() -> auth::claude::ClaudeCredentials {
        auth::claude::ClaudeCredentials {
            access_token: "native-b".into(),
            refresh_token: "native-b-refresh".into(),
            expires_at: 4102444800000,
            subscription_type: None,
            scopes: vec![],
        }
    }

    fn reject_after_file_change(&self) {
        let error = reuse_claude_code_native_snapshot(&self.file, Self::snapshot(), || {
            Self::write_file(&self.file, "fresh-file", 4102444800000);
        })
        .unwrap_err();
        assert!(error.to_string().contains("taking precedence"));
    }
}

impl Drop for NativeStageFixture {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
        auth::claude::set_active_account_override(self.active_override.take());
    }
}

#[test]
fn native_stage_rejected_file_race_preserves_auth_store_bytes() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(true);
    let path = auth::claude::jcode_path().unwrap();
    let before = std::fs::read(&path).unwrap();
    fixture.reject_after_file_change();
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn native_stage_rejection_does_not_grant_native_trust() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    fixture.reject_after_file_change();
    assert!(!auth::claude::native_source_allowed());
}

#[test]
fn native_stage_rejection_preserves_existing_native_trust() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(true);
    fixture.reject_after_file_change();
    assert!(auth::claude::native_source_allowed());
}

#[test]
fn native_stage_rejection_preserves_concurrent_account_choice() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(true);
    let mut after_concurrent_change = Vec::new();
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {
        NativeStageFixture::save_account("account-c");
        after_concurrent_change = std::fs::read(auth::claude::jcode_path().unwrap()).unwrap();
        NativeStageFixture::write_file(&fixture.file, "fresh-file", 4102444800000);
    })
    .unwrap_err();
    assert_eq!(
        std::fs::read(auth::claude::jcode_path().unwrap()).unwrap(),
        after_concurrent_change
    );
}

#[test]
fn native_stage_success_imports_without_overwriting_previous_account() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {})
        .unwrap();
    assert_eq!(
        auth::claude::load_credentials_for_account("claude-otter")
            .unwrap()
            .access_token,
        "account-a-access"
    );
}

#[test]
fn native_stage_success_selects_imported_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {})
        .unwrap();
    assert_eq!(
        auth::claude::load_credentials().unwrap().access_token,
        "native-b"
    );
}

#[test]
fn native_stage_success_records_native_trust() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {})
        .unwrap();
    assert!(auth::claude::native_source_allowed());
}

#[test]
fn native_stage_rejects_changed_environment_token_without_store_changes() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    let path = auth::claude::jcode_path().unwrap();
    let before = std::fs::read(&path).unwrap();
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {
        crate::env::set_var(
            "CLAUDE_CODE_OAUTH_TOKEN",
            r#"{"claudeAiOauth":{"accessToken":"different-env-token","refreshToken":"different-refresh","expiresAt":4102444800000}}"#,
        );
    })
    .unwrap_err();
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn native_stage_rejection_preserves_concurrently_granted_trust() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {
        auth::claude::trust_native_source().unwrap();
        NativeStageFixture::write_file(&fixture.file, "fresh-file", 4102444800000);
    })
    .unwrap_err();
    assert!(auth::claude::native_source_allowed());
}

#[test]
fn native_stage_rejection_preserves_concurrently_revoked_trust() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(true);
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {
        crate::config::Config::revoke_external_auth_source(
            auth::claude::CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID,
        )
        .unwrap();
        NativeStageFixture::write_file(&fixture.file, "fresh-file", 4102444800000);
    })
    .unwrap_err();
    assert!(!auth::claude::native_source_allowed());
}

#[test]
fn native_stage_repeated_import_matches_existing_snapshot() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {})
        .unwrap();
    reuse_claude_code_native_snapshot(&fixture.file, NativeStageFixture::snapshot(), || {})
        .unwrap();
    assert_eq!(auth::claude::list_accounts().unwrap().len(), 2);
}

#[test]
fn native_stage_missing_refresh_and_environment_leaves_store_unchanged() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    let path = auth::claude::jcode_path().unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut snapshot = NativeStageFixture::snapshot();
    snapshot.refresh_token.clear();
    reuse_claude_code_native_snapshot(&fixture.file, snapshot, || {}).unwrap_err();
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn native_stage_environment_only_token_does_not_import_an_account() {
    let _guard = crate::storage::lock_test_env();
    let fixture = NativeStageFixture::new(false);
    let path = auth::claude::jcode_path().unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut snapshot = NativeStageFixture::snapshot();
    snapshot.refresh_token.clear();
    crate::env::set_var(
        "CLAUDE_CODE_OAUTH_TOKEN",
        r#"{"claudeAiOauth":{"accessToken":"native-b","refreshToken":"","expiresAt":4102444800000}}"#,
    );
    reuse_claude_code_native_snapshot(&fixture.file, snapshot, || {}).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), before);
}
