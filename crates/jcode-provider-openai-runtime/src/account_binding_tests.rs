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
        let previous_account = jcode_base::auth::codex::get_active_account_override();
        jcode_base::env::set_var("JCODE_HOME", home.path());
        jcode_base::env::set_var("JCODE_RUNTIME_PROVIDER", "openai-oauth");
        jcode_base::auth::codex::set_active_account_override(None);
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
        jcode_base::auth::codex::set_active_account_override(self.previous_account.take());
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

fn save_account(label: &str, token: &str, refresh: &str, identity: &str) -> String {
    jcode_base::auth::codex::upsert_account(jcode_base::auth::codex::OpenAiAccount {
        label: label.into(),
        access_token: token.into(),
        refresh_token: refresh.into(),
        expires_at: Some(chrono::Utc::now().timestamp_millis() + 3_600_000),
        id_token: None,
        account_id: Some(identity.into()),
        email: None,
    })
    .unwrap()
}

#[tokio::test]
async fn oauth_account_binding_survives_global_switch_refresh_and_fork() {
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let personal = save_account(
        "new-personal",
        "personal-access",
        "personal-refresh-old",
        "personal-id",
    );
    let work = save_account("new-work", "work-access", "work-refresh", "work-id");
    jcode_base::auth::codex::set_active_account(&personal).unwrap();
    let provider = OpenAIProvider::new(
        jcode_base::auth::codex::load_credentials_for_account(&personal).unwrap(),
    );
    provider.set_oauth_account(Some(&personal)).unwrap();
    provider
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    assert_eq!(
        openai_access_token(provider.refresh_account_label(), &provider.credentials)
            .await
            .unwrap(),
        "personal-access"
    );

    jcode_base::auth::codex::set_active_account(&work).unwrap();
    provider.reload_credentials_now();
    provider.invalidate_credentials().await;
    provider
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    assert_eq!(
        openai_access_token(provider.refresh_account_label(), &provider.credentials)
            .await
            .unwrap(),
        "personal-access"
    );
    assert_eq!(
        provider.credentials.read().await.account_id.as_deref(),
        Some("personal-id")
    );
    assert_eq!(provider.catalog_scope(), personal);
    assert_eq!(
        provider.refresh_account_label().as_deref(),
        Some(personal.as_str())
    );

    // An already-rotated account refresh must join personal's single-flight
    // transaction even though work is now the globally active saved login.
    save_account(
        &personal,
        "personal-access-rotated",
        "personal-refresh-new",
        "personal-id",
    );
    let token = openai_stream_runtime::force_refresh_openai_token(
        &provider.credentials,
        "personal-refresh-old",
        provider.refresh_account_label(),
    )
    .await
    .unwrap();
    assert_eq!(token, "personal-access-rotated");
    assert_eq!(
        provider.credentials.read().await.account_id.as_deref(),
        Some("personal-id")
    );
    assert_eq!(
        jcode_base::auth::codex::load_credentials_for_account(&work)
            .unwrap()
            .access_token,
        "work-access"
    );
    assert_eq!(
        jcode_base::auth::codex::active_account_label().as_deref(),
        Some(work.as_str())
    );

    provider
        .model_reasoning_efforts
        .write()
        .unwrap()
        .insert("personal-only".into(), vec!["max".into()]);
    let fork = provider.fork();
    assert_eq!(
        fork.oauth_account_label().as_deref(),
        Some(personal.as_str())
    );
    fork.set_oauth_account(Some(&work)).unwrap();
    fork.set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    assert!(
        provider
            .model_reasoning_efforts
            .read()
            .unwrap()
            .contains_key("personal-only")
    );
    assert_eq!(
        provider.oauth_account_label().as_deref(),
        Some(personal.as_str())
    );
    assert_eq!(
        provider.credentials.read().await.access_token,
        "personal-access-rotated"
    );
    assert_eq!(fork.oauth_account_label().as_deref(), Some(work.as_str()));
    assert!(fork.set_oauth_account(Some("missing-account")).is_err());
    assert_eq!(fork.oauth_account_label().as_deref(), Some(work.as_str()));
    provider.invalidate_credentials().await;
    assert_eq!(
        provider.credentials.read().await.access_token,
        "personal-access-rotated"
    );
    provider.set_oauth_account(Some(&work)).unwrap();
    provider
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    jcode_base::auth::codex::remove_account(&work).unwrap();
    assert!(
        provider.complete(&[], &[], "test", None).await.is_err(),
        "removed pinned account must fail before websocket continuation"
    );
    assert!(
        openai_access_token(provider.refresh_account_label(), &provider.credentials)
            .await
            .is_err(),
        "removed pinned account must not use cached bearer"
    );
}
