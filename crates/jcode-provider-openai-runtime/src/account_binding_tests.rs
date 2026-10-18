use super::*;

struct AccountSandbox {
    _home: tempfile::TempDir,
    previous_home: Option<std::ffi::OsString>,
    previous_runtime: Option<std::ffi::OsString>,
    previous_api_key: Option<std::ffi::OsString>,
    previous_account: Option<String>,
}

impl AccountSandbox {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("JCODE_HOME");
        let previous_runtime = std::env::var_os("JCODE_RUNTIME_PROVIDER");
        let previous_api_key = std::env::var_os("OPENAI_API_KEY");
        let previous_account = jcode_base::auth::codex::get_active_account_override();
        jcode_base::env::set_var("JCODE_HOME", home.path());
        jcode_base::env::set_var("JCODE_RUNTIME_PROVIDER", "openai-oauth");
        jcode_base::auth::codex::set_active_account_override(None);
        Self {
            _home: home,
            previous_home,
            previous_runtime,
            previous_api_key,
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
            ("OPENAI_API_KEY", self.previous_api_key.take()),
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
async fn service_tier_catalog_is_scoped_across_account_mode_reload_and_fork() {
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let personal = save_account("tier-personal", "tier-personal-token", "refresh-p", "id-p");
    let work = save_account("tier-work", "tier-work-token", "refresh-w", "id-w");
    let model = "gpt-6-astra";
    for (scope, tiers) in [
        (&personal, vec!["priority".into()]),
        (&work, vec!["ultrafast".into()]),
    ] {
        let mut catalog = jcode_base::provider::OpenAIModelCatalog::default();
        catalog.available_models.push(model.into());
        catalog.service_tiers.insert(model.into(), tiers);
        jcode_base::provider::persist_openai_model_catalog_for_scope(scope, &catalog);
    }
    jcode_base::auth::codex::set_active_account(&personal).unwrap();
    let provider = OpenAIProvider::new(
        jcode_base::auth::codex::load_credentials_for_account(&personal).unwrap(),
    );
    *provider.model.try_write().unwrap() = model.into();
    provider.set_oauth_account(Some(&personal)).unwrap();
    *provider.service_tier.write().unwrap() = Some("ultrafast".into());
    assert_eq!(provider.service_tier(), None);
    let fork = provider.fork();
    fork.set_oauth_account(Some(&work)).unwrap();
    assert_eq!(fork.service_tier().as_deref(), Some("ultrafast"));
    assert_eq!(provider.service_tier(), None);
    jcode_base::auth::codex::set_active_account(&work).unwrap();
    // These sync reloads formerly read metadata while holding the credential
    // write lock, accidentally falling back to the globally selected account.
    provider.reload_credentials_now();
    provider
        .set_credential_mode(OpenAICredentialMode::OAuth)
        .unwrap();
    provider.invalidate_credentials().await;
    assert_eq!(provider.available_service_tiers(), vec!["priority", "flex"]);
    assert_eq!(provider.service_tier(), None);
    provider.set_oauth_account(Some(&work)).unwrap();
    assert_eq!(provider.service_tier().as_deref(), Some("ultrafast"));
    assert_eq!(fork.service_tier().as_deref(), Some("ultrafast"));
    // API-key catalogs with unknown capability metadata retain legacy behavior.
    let api = CodexCredentials {
        access_token: "sk-tier-scope".into(),
        refresh_token: String::new(),
        expires_at: None,
        id_token: None,
        account_id: None,
    };
    let api_scope = OpenAIProvider::catalog_scope_for(&api);
    let api_catalog = jcode_base::provider::OpenAIModelCatalog {
        available_models: vec![model.into()],
        ..Default::default()
    };
    jcode_base::provider::persist_openai_model_catalog_for_scope(&api_scope, &api_catalog);
    jcode_base::env::set_var("OPENAI_API_KEY", &api.access_token);
    provider
        .set_credential_mode(OpenAICredentialMode::ApiKey)
        .unwrap();
    assert_eq!(provider.service_tier().as_deref(), Some("ultrafast"));
    assert_eq!(
        provider.available_service_tiers(),
        vec!["priority", "ultrafast", "flex"]
    );
    assert_eq!(fork.available_service_tiers(), vec!["ultrafast", "flex"]);
}

#[tokio::test]
async fn active_oauth_email_follows_cached_unbound_credentials_not_global_account() {
    let _lock = jcode_base::storage::lock_test_env();
    let _sandbox = AccountSandbox::new();
    let personal = save_account(
        "new-personal",
        "personal-access",
        "personal-refresh",
        "personal-id",
    );
    let work = save_account("new-work", "work-access", "work-refresh", "work-id");
    jcode_base::auth::codex::update_account_profile(&personal, Some("personal@example.com".into()))
        .unwrap();
    jcode_base::auth::codex::update_account_profile(&work, Some("work@example.com".into()))
        .unwrap();
    jcode_base::auth::codex::set_active_account(&personal).unwrap();
    let provider = OpenAIProvider::new(
        jcode_base::auth::codex::load_credentials_for_account(&personal).unwrap(),
    );
    assert_eq!(provider.oauth_account_label(), None);
    jcode_base::auth::codex::set_active_account(&work).unwrap();
    assert_eq!(
        provider.active_oauth_account_email().as_deref(),
        Some("personal@example.com")
    );
    provider.credentials.write().await.access_token = "rotated-access".into();
    assert_eq!(
        provider.active_oauth_account_email().as_deref(),
        Some("personal@example.com")
    );

    // An actual API-key credential excludes saved identities even if access matches.
    {
        let mut credentials = provider.credentials.write().await;
        credentials.access_token = "personal-access".into();
        credentials.refresh_token.clear();
        credentials.id_token = None;
    }
    assert_eq!(provider.active_oauth_account_email(), None);
    {
        let mut credentials = provider.credentials.write().await;
        credentials.access_token = "unknown-access".into();
        credentials.refresh_token = "unknown-refresh".into();
    }
    assert_eq!(provider.active_oauth_account_email(), None);
    {
        let mut credentials = provider.credentials.write().await;
        // JWT payload: {"email":"id@example.com"}. No saved-account match.
        credentials.id_token = Some("header.eyJlbWFpbCI6ImlkQGV4YW1wbGUuY29tIn0.signature".into());
        assert_eq!(provider.active_oauth_account_email(), None);
    }
    assert_eq!(
        provider.active_oauth_account_email().as_deref(),
        Some("id@example.com")
    );
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
