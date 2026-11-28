#[tokio::test]
async fn auto_mode_falls_back_to_api_key_when_oauth_is_expired() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());
    let _api_key = EnvVarGuard::set("ANTHROPIC_API_KEY", "test-anthropic-api-key");
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_PROVIDER", "auto");

    jcode_base::auth::claude::upsert_account(jcode_base::auth::claude::AnthropicAccount {
        record_id: None,
        label: "claude-1".to_string(),
        access: "expired-oauth-access".to_string(),
        refresh: String::new(),
        expires: 0,
        email: None,
        subscription_type: Some("max".to_string()),
        scopes: vec!["user:inference".to_string()],
    })
    .unwrap();

    let provider = AnthropicProvider::new();
    assert_eq!(
        provider.credential_mode_snapshot(),
        AnthropicCredentialMode::Auto
    );
    assert_eq!(provider.active_resolved_credential(), None);

    let (token, is_oauth) = provider.get_access_token().await.unwrap();
    assert_eq!(token, "test-anthropic-api-key");
    assert!(
        !is_oauth,
        "automatic fallback must use API-key request semantics"
    );
    assert_eq!(
        provider.active_resolved_credential(),
        Some(jcode_provider_core::ResolvedCredential::ApiKey),
        "Auto must expose the actual API fallback rather than credential presence"
    );
    assert_eq!(provider.fork().active_resolved_credential(), None);
    provider.invalidate_credentials().await;
    assert_eq!(provider.active_resolved_credential(), None);
}
