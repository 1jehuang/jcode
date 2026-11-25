use super::*;

struct VerifiedRuntime(StubExternalRuntime);
#[async_trait::async_trait]
impl Provider for VerifiedRuntime {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        panic!("offline route test must not infer");
    }
    fn name(&self) -> &str {
        "Anthropic"
    }
    fn model(&self) -> String {
        self.0.model()
    }
    fn set_model(&self, model: &str) -> Result<()> {
        self.0.set_model(model)
    }
    fn credential_mode(&self) -> CredentialMode {
        self.0.credential_mode()
    }
    fn set_credential_mode(&self, mode: CredentialMode) -> Result<()> {
        self.0.set_credential_mode(mode)
    }
    fn set_oauth_account(&self, account: Option<&str>) -> Result<()> {
        self.0.set_oauth_account(account)
    }
    fn oauth_account_label(&self) -> Option<String> {
        self.0.oauth_account_label()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self(StubExternalRuntime::anthropic()))
    }
    fn set_route_selection_verified(
        &self,
        route: &RouteSelection,
        fingerprint: &str,
    ) -> Result<()> {
        let credentials = match self.oauth_account_label() {
            Some(label) => auth::claude::load_credentials_for_account(&label)?,
            None => auth::claude::load_credentials()?,
        };
        anyhow::ensure!(
            auth::claude_cli_completion::fingerprint(&credentials) == fingerprint,
            "different grant"
        );
        self.set_model(&route.model)
    }
}

#[test]
fn verified_claude_routes_commit_only_matching_grants_and_default_route_clears_old_binding() {
    with_clean_provider_test_env(|| {
        let rt = enter_test_runtime();
        let _guard = rt.enter();
        let mut labels = Vec::new();
        for name in ["route-previous", "route-selected"] {
            labels.push(
                auth::claude::upsert_account(auth::claude::AnthropicAccount {
                    record_id: None,
                    label: name.into(),
                    access: name.into(),
                    refresh: format!("{name}-refresh"),
                    expires: i64::MAX,
                    email: None,
                    subscription_type: None,
                    scopes: vec![],
                })
                .unwrap(),
            );
        }
        auth::claude::set_active_account(&labels[0]).unwrap();
        let provider = test_multi_provider_with_openai();
        let runtime = Arc::new(VerifiedRuntime(StubExternalRuntime::anthropic()));
        runtime.set_oauth_account(Some(&labels[0])).unwrap();
        let identity_pin = auth::account_pins::claude_pin(&labels[1]).unwrap();
        assert!(
            runtime.set_oauth_account_pin(&identity_pin).is_err(),
            "providers without identity support must not reduce a strong pin to a label"
        );
        assert_eq!(runtime.oauth_account_label(), Some(labels[0].clone()));
        *provider.anthropic.write().unwrap() = Some(runtime.clone());
        let route = RouteSelection {
            model: ALL_CLAUDE_MODELS[0].into(),
            runtime_key: RuntimeKey::ClaudeOAuth,
            api_method: format!("claude-oauth@{}", labels[1]),
            provider_label: "Claude".into(),
            detail: String::new(),
        };
        assert!(
            provider
                .set_route_selection_verified(&route, "wrong-grant")
                .is_err()
        );
        assert_eq!(provider.active_provider(), ActiveProvider::OpenAI);
        assert_eq!(
            provider.anthropic_provider().unwrap().oauth_account_label(),
            Some(labels[0].clone())
        );
        let fingerprint = auth::claude_cli_completion::fingerprint(
            &auth::claude::load_credentials_for_account(&labels[1]).unwrap(),
        );
        provider
            .set_route_selection_verified(&route, &fingerprint)
            .unwrap();
        assert_eq!(provider.active_provider(), ActiveProvider::Claude);
        assert_eq!(provider.oauth_account_label(), Some(labels[1].clone()));
        let mut default = route.clone();
        default.api_method = "claude-oauth".into();
        provider.set_route_selection(&default).unwrap();
        assert_eq!(provider.oauth_account_label(), None);
        assert_eq!(
            provider.anthropic_provider().unwrap().credential_mode(),
            CredentialMode::OAuth
        );
        assert_eq!(
            auth::claude::active_account_label(),
            Some(labels[0].clone())
        );
        let old = provider.anthropic_provider().unwrap();
        default.model = "not-a-model".into();
        assert!(provider.set_route_selection(&default).is_err());
        assert!(Arc::ptr_eq(&provider.anthropic_provider().unwrap(), &old));
        assert_eq!(provider.model(), route.model);
    });
}
