use super::*;

/// Provider that only has the Claude OAuth credential: any request pinned to
/// the API-key route fails exactly like the real runtime does.
#[derive(Clone, Default)]
struct OAuthOnlyClaudeProvider {
    model: Arc<std::sync::Mutex<String>>,
    set_model_requests: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl Provider for OAuthOnlyClaudeProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        unreachable!("OAuthOnlyClaudeProvider does not complete requests")
    }

    fn name(&self) -> &str {
        "Claude"
    }

    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    fn set_model(&self, request: &str) -> Result<()> {
        self.set_model_requests
            .lock()
            .unwrap()
            .push(request.to_string());
        if request.starts_with("claude-api:") {
            anyhow::bail!("No Anthropic API key found");
        }
        let model = request
            .split_once(':')
            .map(|(_, model)| model)
            .unwrap_or(request);
        *self.model.lock().unwrap() = model.to_string();
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

/// A session created while the deployment used an Anthropic API key, resumed
/// after it moved to Claude OAuth, must come back on OAuth and persist that
/// route. Before the fix the restore failed, the stale `provider_key` stayed
/// on disk, and every swarm worker inheriting it died with
/// "No Anthropic API key found".
#[tokio::test]
async fn resume_heals_stale_credential_route_after_credential_type_change() {
    let _guard = crate::storage::lock_test_env();
    struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(value) => crate::env::set_var(key, value),
                    None => crate::env::remove_var(key),
                }
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _restore = RestoreEnv(
        ["JCODE_HOME", "JCODE_RUNTIME_PROVIDER"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect(),
    );
    let home = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", home.path());
    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude");
    crate::config::Config::invalidate_cache();

    let mut stale = crate::session::Session::create_with_id(
        "session_stale_api_key_route".to_string(),
        None,
        None,
    );
    stale.model = Some("claude-sonnet-5".to_string());
    stale.provider_key = Some("anthropic-api".to_string());
    stale.route_api_method = None;
    stale.save_prepared().expect("persist stale session");

    let provider = Arc::new(OAuthOnlyClaudeProvider::default());
    let provider_dyn: Arc<dyn Provider> = provider.clone();
    let registry = Registry::new(provider_dyn.clone()).await;
    let mut agent = Agent::new(provider_dyn, registry);
    agent
        .restore_session("session_stale_api_key_route")
        .expect("restore stale session");

    assert_eq!(
        provider.set_model_requests.lock().unwrap().as_slice(),
        [
            "claude-api:claude-sonnet-5",
            "claude-api:claude-sonnet-5",
            "claude-oauth:claude-sonnet-5"
        ],
        "restore tries the persisted route (plus the auth-refresh retry), then the runtime's"
    );
    assert_eq!(agent.provider_model(), "claude-sonnet-5");
    assert_eq!(
        agent.session_provider_key().as_deref(),
        Some("claude-oauth")
    );
    assert_eq!(
        agent.session_route_api_method().as_deref(),
        Some("claude-oauth")
    );

    // What a swarm spawn reads when the coordinator is mid-turn.
    let persisted = crate::session::Session::load("session_stale_api_key_route").unwrap();
    assert_eq!(persisted.provider_key.as_deref(), Some("claude-oauth"));
    assert_eq!(persisted.route_api_method.as_deref(), Some("claude-oauth"));
    assert_eq!(
        crate::provider::MultiProvider::model_switch_request_for_session_route(
            "claude-sonnet-5",
            persisted.provider_key.as_deref(),
            persisted.route_api_method.as_deref(),
        ),
        "claude-oauth:claude-sonnet-5",
        "workers inheriting the healed session must target the live credential"
    );
}

#[derive(Clone)]
struct IdentityPinnedProvider {
    pin: Arc<std::sync::Mutex<jcode_provider_core::AccountPin>>,
    api: Arc<std::sync::atomic::AtomicBool>,
}
#[async_trait]
impl Provider for IdentityPinnedProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        panic!("account bookkeeping never sends a provider request")
    }
    fn name(&self) -> &str {
        "OpenAI"
    }
    fn model(&self) -> String {
        "gpt-future-identity".into()
    }
    fn set_model(&self, _: &str) -> Result<()> {
        Ok(())
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        Some(if self.api.load(std::sync::atomic::Ordering::Acquire) {
            jcode_provider_core::ResolvedCredential::ApiKey
        } else {
            jcode_provider_core::ResolvedCredential::Oauth
        })
    }
    fn credential_mode(&self) -> jcode_provider_core::CredentialMode {
        if self.api.load(std::sync::atomic::Ordering::Acquire) {
            jcode_provider_core::CredentialMode::ApiKey
        } else {
            jcode_provider_core::CredentialMode::OAuth
        }
    }
    fn oauth_account_pin(&self) -> Option<jcode_provider_core::AccountPin> {
        Some(self.pin.lock().unwrap().clone())
    }
    fn oauth_account_label(&self) -> Option<String> {
        let pin = self.pin.lock().unwrap();
        Some(crate::auth::account_pins::resolve_openai(&pin).unwrap_or_else(|_| pin.label.clone()))
    }
    fn set_oauth_account_pin(&self, pin: &jcode_provider_core::AccountPin) -> Result<()> {
        *self.pin.lock().unwrap() = crate::auth::account_pins::normalize_openai_pin(pin)?;
        Ok(())
    }
}

struct AccountTestEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl AccountTestEnv {
    fn new(root: &std::path::Path) -> Self {
        let keys = [
            "HOME",
            "JCODE_GLOBAL_HOME",
            "XDG_CONFIG_HOME",
            "CLAUDE_CONFIG_DIR",
            "JCODE_TEST_SESSION",
        ];
        let saved = keys
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        for key in &keys[..4] {
            crate::env::set_var(key, root.join(key));
        }
        crate::env::set_var("JCODE_TEST_SESSION", "1");
        crate::config::Config::invalidate_cache();
        Self(saved)
    }
}
impl Drop for AccountTestEnv {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
        crate::config::Config::invalidate_cache();
    }
}
fn identity_provider() -> IdentityPinnedProvider {
    let label = crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
        record_id: None,
        label: "first".into(),
        access_token: "offline-access".into(),
        refresh_token: "offline-refresh".into(),
        id_token: None,
        account_id: Some("first-workspace".into()),
        email: None,
        expires_at: Some(i64::MAX),
    })
    .unwrap();
    IdentityPinnedProvider {
        pin: Arc::new(std::sync::Mutex::new(
            crate::auth::account_pins::openai_pin(&label).unwrap(),
        )),
        api: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
}

#[test]
fn identity_pin_agent_lease_follows_rename_and_rejects_reused_label() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let _env = AccountTestEnv::new(sandbox.root());
    let provider = Arc::new(identity_provider());
    let pin = provider.oauth_account_pin().unwrap();
    let mut agent = Agent::new(provider.clone(), Registry::empty());
    agent.session.route_api_method = Some(format!("openai-oauth@{}", pin.label));
    crate::provider::account_routes::record_account_pin(provider.as_ref(), &mut agent.session);
    let mut store = crate::auth::codex::load_auth_file().unwrap();
    store.openai_accounts[0].label = "renamed-workspace".into();
    crate::auth::codex::save_auth_file(&store).unwrap();
    agent.ensure_session_lease().unwrap();
    agent.sync_bound_account_route();
    assert_eq!(
        agent.session.route_api_method.as_deref(),
        Some("openai-oauth@renamed-workspace")
    );
    assert_eq!(
        agent.session.account_pins["openai"].record_id,
        pin.record_id
    );
    let saved = Session::load(&agent.session.id).unwrap();
    assert_eq!(saved.account_pins, agent.session.account_pins);
    let mut restored = Agent::new_with_session(provider.clone(), Registry::empty(), saved, None);
    restored.ensure_session_lease().unwrap();
    store.openai_accounts[0].record_id = Some("replacement-row".into());
    store.openai_accounts[0].account_id = Some("replacement-workspace".into());
    crate::auth::codex::save_auth_file(&store).unwrap();
    assert!(restored.ensure_session_lease().is_err());
    let replacement = crate::auth::account_pins::openai_pin("renamed-workspace").unwrap();
    provider.set_oauth_account_pin(&replacement).unwrap();
    assert!(
        restored.ensure_session_lease().is_err(),
        "runtime selection alone must not silently replace the saved identity"
    );
    restored.sync_bound_account_route();
    assert!(
        restored.ensure_session_lease().is_err(),
        "observing a failed request must not adopt a replaced same-label login"
    );
    crate::provider::account_routes::record_account_pin(provider.as_ref(), &mut restored.session);
    restored.ensure_session_lease().unwrap();
}

#[test]
fn identity_pin_agent_preserves_api_billing_and_clears_stale_generic_oauth() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let _env = AccountTestEnv::new(sandbox.root());
    let provider = Arc::new(identity_provider());
    let pin = provider.oauth_account_pin().unwrap();
    provider
        .api
        .store(true, std::sync::atomic::Ordering::Release);
    let mut agent = Agent::new(provider.clone(), Registry::empty());
    assert_eq!(
        agent.provider_key_for_new_session().as_deref(),
        Some("openai-api")
    );
    agent
        .session
        .account_pins
        .insert("openai".into(), pin.clone());
    crate::auth::codex::remove_account(&pin.label).unwrap();
    agent.session.provider_key = Some("openai-api".into());
    agent.session.route_api_method = Some("openai-api".into());
    agent.sync_bound_account_route();
    assert_eq!(
        agent.session_route_api_method().as_deref(),
        Some("openai-api")
    );
    agent.set_model("gpt-future-identity").unwrap();
    assert_eq!(
        agent.session.route_api_method.as_deref(),
        Some("openai-api")
    );
    agent.ensure_session_lease().unwrap();
    assert_eq!(agent.session.account_pins["openai"], pin);
    for stale in ["openai-oauth", "openai-oauth@removed"] {
        agent.session.route_api_method = Some(stale.into());
        agent.set_model("gpt-future-identity").unwrap();
        assert!(agent.session.route_api_method.is_none());
    }
    agent.session.route_api_method = Some("openai-oauth".into());
    agent.sync_bound_account_route();
    assert!(agent.session.route_api_method.is_none());
}
