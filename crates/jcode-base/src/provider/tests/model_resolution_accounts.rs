#[test]
fn test_initial_openai_provider_can_switch_to_anthropic_auth_routes() {
    with_clean_provider_test_env(|| {
        crate::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-api-key");
        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            record_id: None,
            label: "claude-1".to_string(),
            access: "oauth-access-token".to_string(),
            refresh: "oauth-refresh-token".to_string(),
            expires: chrono::Utc::now().timestamp_millis() + 3_600_000,
            email: None,
            subscription_type: Some("max".to_string()),
            scopes: vec!["user:inference".to_string()],
        })
        .expect("save Claude OAuth account");

        let anthropic = test_anthropic_runtime();
        let provider = MultiProvider {
            anthropic: RwLock::new(Some(Arc::clone(&anthropic) as Arc<dyn Provider>)),
            openai: RwLock::new(None),
            copilot_api: RwLock::new(None),
            antigravity: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            bedrock: RwLock::new(None),
            openrouter: RwLock::new(None),
            openai_compatible_profiles: RwLock::new(std::collections::HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            session_id: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            startup_notices: RwLock::new(Vec::new()),
            initial_provider: Some(ActiveProvider::OpenAI),
            catalog_scope: next_catalog_scope(),
            routes_memo: std::sync::Mutex::new(None),
            post_auth_refreshes_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let rt = enter_test_runtime();
        let _runtime_guard = rt.enter();

        // Route pinning is MultiProvider's job; the concrete token resolution
        // for each pin is covered by jcode-provider-anthropic-runtime's
        // credential-mode tests.
        assert_eq!(
            anthropic.credential_mode(),
            jcode_provider_core::CredentialMode::Auto,
            "default (Auto) leaves the credential pin unset"
        );

        provider
            .set_model("claude-oauth:claude-opus-4-6")
            .expect("OAuth route should select Claude OAuth credentials");
        assert_eq!(
            anthropic.credential_mode(),
            jcode_provider_core::CredentialMode::OAuth
        );

        provider
            .set_model("claude-api:claude-opus-4-6")
            .expect("API route should select Anthropic API-key credentials");
        assert_eq!(
            anthropic.credential_mode(),
            jcode_provider_core::CredentialMode::ApiKey
        );
    });
}
