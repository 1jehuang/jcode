use super::*;

struct AccountSandbox {
    _home: tempfile::TempDir,
    previous_home: Option<std::ffi::OsString>,
    previous_runtime: Option<std::ffi::OsString>,
    previous_account: Option<String>,
}

impl AccountSandbox {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("JCODE_HOME");
        let previous_runtime = std::env::var_os("JCODE_RUNTIME_PROVIDER");
        let previous_account = auth::claude::get_active_account_override();
        jcode_base::env::set_var("JCODE_HOME", home.path());
        jcode_base::env::set_var("JCODE_RUNTIME_PROVIDER", "claude-oauth");
        auth::claude::set_active_account_override(None);
        Self {
            _home: home,
            previous_home,
            previous_runtime,
            previous_account,
        }
    }
}

impl Drop for AccountSandbox {
    fn drop(&mut self) {
        auth::claude::set_active_account_override(self.previous_account.take());
        for (key, value) in [
            ("JCODE_HOME", self.previous_home.take()),
            ("JCODE_RUNTIME_PROVIDER", self.previous_runtime.take()),
        ] {
            match value {
                Some(value) => jcode_base::env::set_var(key, value),
                None => jcode_base::env::remove_var(key),
            }
        }
    }
}

fn save_account(label: &str, token: &str, refresh: &str) -> String {
    auth::claude::upsert_account(auth::claude::AnthropicAccount {
        label: label.into(),
        access: token.into(),
        refresh: refresh.into(),
        expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
        email: None,
        subscription_type: Some("max".into()),
        scopes: vec!["user:inference".into()],
    })
    .unwrap()
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn oauth_account_binding_survives_global_switch_refresh_and_fork() {
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let personal = save_account("new-personal", "personal-access", "personal-refresh-old");
    let work = save_account("new-work", "work-access", "work-refresh");
    auth::claude::set_active_account(&personal).unwrap();
    let provider = AnthropicProvider::new();
    provider.set_oauth_account(Some(&personal)).unwrap();
    provider
        .set_credential_mode(AnthropicCredentialMode::OAuth)
        .unwrap();
    assert_eq!(
        provider.get_access_token().await.unwrap(),
        ("personal-access".into(), true)
    );

    auth::claude::set_active_account(&work).unwrap();
    provider.invalidate_credentials().await;
    assert_eq!(
        provider.get_access_token().await.unwrap(),
        ("personal-access".into(), true)
    );
    assert_eq!(
        provider.catalog_scope_for_route(true),
        format!("oauth::{personal}")
    );
    assert_eq!(
        provider.oauth_refresh_label().as_deref(),
        Some(personal.as_str())
    );

    // Another request already rotated personal's refresh token. Forced refresh
    // must join that account's transaction, not the globally active work login.
    save_account(&personal, "personal-access-rotated", "personal-refresh-new");
    let token = force_refresh_oauth_token(
        Arc::clone(&provider.credentials),
        &provider.oauth_refresh_label(),
    )
    .await
    .unwrap();
    assert_eq!(token, "personal-access-rotated");
    assert_eq!(
        auth::claude::load_credentials_for_account(&work)
            .unwrap()
            .access_token,
        "work-access"
    );
    assert_eq!(
        auth::claude::active_account_label().as_deref(),
        Some(work.as_str())
    );

    let fork = provider.fork();
    assert_eq!(
        fork.oauth_account_label().as_deref(),
        Some(personal.as_str())
    );
    fork.set_oauth_account(Some(&work)).unwrap();
    assert_eq!(
        provider.oauth_account_label().as_deref(),
        Some(personal.as_str())
    );
    assert_eq!(fork.oauth_account_label().as_deref(), Some(work.as_str()));
    assert!(fork.set_oauth_account(Some("missing-account")).is_err());
    assert_eq!(fork.oauth_account_label().as_deref(), Some(work.as_str()));

    provider.invalidate_credentials().await;
    assert_eq!(
        provider.get_access_token().await.unwrap(),
        ("personal-access-rotated".into(), true)
    );
    provider.set_oauth_account(Some(&work)).unwrap();
    assert_eq!(
        provider.get_access_token().await.unwrap(),
        ("work-access".into(), true)
    );
    auth::claude::remove_account(&work).unwrap();
    assert!(
        provider.get_access_token().await.is_err(),
        "removed pinned account must not use cached bearer"
    );
}
