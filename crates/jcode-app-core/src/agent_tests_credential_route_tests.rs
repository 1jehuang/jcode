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
