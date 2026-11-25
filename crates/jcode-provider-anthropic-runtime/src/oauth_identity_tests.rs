use super::*;

#[tokio::test]
async fn identity_pin_survives_unknown_profile_refresh_relabel_and_fork() {
    let _env = TestEnv::new();
    let mut saved = account("unknown", "original");
    saved.email = None;
    let label = auth::claude::upsert_account(saved).unwrap();
    let other = save("other", "other");
    let session = offline_provider();
    session.set_oauth_account(Some(&label)).unwrap();
    let original = session.oauth_account_pin().unwrap();
    assert_eq!(
        session.get_access_token().await.unwrap(),
        ("original".into(), true)
    );
    auth::claude::update_account_tokens(&label, "rotated", "rotated-refresh", i64::MAX).unwrap();
    auth::claude::update_account_profile(&label, Some("enriched@example.test".into())).unwrap();
    let mut store = auth::claude::load_auth_file().unwrap();
    store
        .anthropic_accounts
        .iter_mut()
        .find(|account| account.label == label)
        .unwrap()
        .label = "claude-panda".into();
    auth::claude::save_auth_file(&store).unwrap();
    auth::claude::set_active_account(&other).unwrap();
    assert_eq!(
        session.oauth_account_label().as_deref(),
        Some("claude-panda")
    );
    assert_eq!(
        session.get_access_token().await.unwrap(),
        ("rotated".into(), true)
    );
    assert_eq!(
        session.oauth_account_pin().unwrap().record_id,
        original.record_id
    );
    let fork = session.fork();
    fork.set_oauth_account(Some(&other)).unwrap();
    assert_eq!(
        session.oauth_account_label().as_deref(),
        Some("claude-panda")
    );
    assert_eq!(fork.oauth_account_label().as_deref(), Some(other.as_str()));
    auth::claude::remove_account("claude-panda").unwrap();
    save("claude-panda", "foreign-login");
    assert!(session.get_oauth_access_token().await.is_err());
}

#[tokio::test]
async fn bound_auto_identity_cannot_fall_back_to_api_billing_but_explicit_api_still_works() {
    let _env = TestEnv::new();
    let _key = EnvGuard::set("ANTHROPIC_API_KEY", "fixture-platform-key");
    let _token = EnvGuard::remove("ANTHROPIC_AUTH_TOKEN");
    let label = save("selected", "selected");
    let session = offline_provider();
    session.set_oauth_account(Some(&label)).unwrap();
    let pin = session.oauth_account_pin();
    assert_eq!(session.credential_mode(), AnthropicCredentialMode::Auto);
    auth::claude::remove_account(&label).unwrap();
    assert!(
        session.get_access_token().await.is_err(),
        "an unavailable selected subscription cannot silently spend an API key"
    );
    session
        .set_credential_mode(AnthropicCredentialMode::ApiKey)
        .unwrap();
    assert_eq!(
        session.get_access_token().await.unwrap(),
        ("fixture-platform-key".into(), false)
    );
    assert_eq!(session.oauth_account_pin(), pin);
    let fork = session.fork();
    assert_eq!(
        fork.credential_mode(),
        jcode_provider_core::CredentialMode::ApiKey
    );
    assert_eq!(
        fork.active_resolved_credential(),
        Some(jcode_provider_core::ResolvedCredential::ApiKey)
    );
    assert_eq!(fork.oauth_account_pin(), pin);
}
