//! Registry-mode swarm model delegation tests.
use crate::agent::Agent;
use crate::message::{Message, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::{Mutex, RwLock, broadcast};

fn mock_route(
    model: &str,
    provider: &str,
    api: &str,
    available: bool,
) -> crate::provider::ModelRoute {
    crate::provider::ModelRoute {
        model: model.into(),
        provider: provider.into(),
        api_method: api.into(),
        available,
        detail: String::new(),
        cheapness: None,
        usage: None,
    }
}

fn mock_registry() -> Vec<crate::provider::ModelRoute> {
    vec![
        mock_route("mock-alpha", "ProviderA", "code-assist-oauth", true),
        mock_route("mock-beta", "ProviderB", "openai-oauth@acct-one", true),
        mock_route("mock-beta", "ProviderB", "openai-oauth@acct-two", true),
        mock_route(
            "mock-gamma",
            "ProviderC",
            "openai-compatible:profile-c",
            true,
        ),
        mock_route("mock-down", "ProviderA", "claude-api", false),
    ]
}

#[test]
fn registry_resolves_models_across_providers_from_the_live_catalog() {
    let routes = mock_registry();
    // Bare model on a non-credential-pinned route stays bare.
    assert_eq!(
        super::resolve_registry_model("mock-alpha", &routes).unwrap(),
        "mock-alpha"
    );
    assert_eq!(
        super::resolve_registry_model(" mock-gamma ", &routes).unwrap(),
        "mock-gamma"
    );
    // Account-pinned OAuth route keeps its credential.
    assert_eq!(
        super::resolve_registry_model("mock-beta", &routes).unwrap(),
        "openai-oauth@acct-one:mock-beta"
    );
    // `model@api_method` selects the exact route even when the method contains '@'.
    assert_eq!(
        super::resolve_registry_model("mock-beta@openai-oauth@acct-two", &routes).unwrap(),
        "openai-oauth@acct-two:mock-beta"
    );
}

#[test]
fn registry_rejects_unknown_or_unavailable_models_with_the_live_list() {
    let routes = mock_registry();
    for bad in ["no-such-model", "mock-down", "mock-alpha@claude-api"] {
        let err = super::resolve_registry_model(bad, &routes).unwrap_err();
        assert!(err.contains(&format!("'{bad}'")), "{err}");
        assert!(err.contains("Available (4)"), "{err}");
        for listed in [
            "mock-alpha@code-assist-oauth",
            "mock-beta@openai-oauth@acct-two",
            "mock-gamma@openai-compatible:profile-c",
        ] {
            assert!(err.contains(listed), "{err}");
        }
        assert!(
            !err.contains("mock-down@"),
            "unavailable routes are not offered: {err}"
        );
    }
    // The list is the registry, not a hardcoded catalog.
    let err =
        super::resolve_registry_model("x", &[mock_route("only-one", "P", "m", true)]).unwrap_err();
    assert!(err.ends_with("only-one@m"), "{err}");
}

struct RegistryProvider(std::sync::Mutex<String>);

#[async_trait]
impl Provider for RegistryProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("registry delegation test must not make model calls")
    }
    fn name(&self) -> &str {
        "mock"
    }
    fn model(&self) -> String {
        self.0.lock().unwrap().clone()
    }
    fn set_model(&self, model: &str) -> Result<()> {
        *self.0.lock().unwrap() = model
            .rsplit_once(':')
            .map(|(_, m)| m)
            .unwrap_or(model)
            .to_string();
        Ok(())
    }
    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        mock_registry()
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self(std::sync::Mutex::new(self.model())))
    }
}

#[tokio::test]
async fn registry_delegation_spawns_requested_model_and_leaves_parent_untouched() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let previous = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::write(
        temp.path().join("config.toml"),
        "[agents]\nswarm_model_registry = true\n",
    )
    .unwrap();
    crate::config::invalidate_config_cache();
    assert!(crate::config::config().agents.swarm_model_registry);

    let provider: Arc<dyn Provider> = Arc::new(RegistryProvider(std::sync::Mutex::new(
        "mock-parent".into(),
    )));
    let registry = Registry::new(provider.clone()).await;
    let parent = Arc::new(Mutex::new(Agent::new_with_session(
        provider.clone(),
        registry,
        crate::session::Session::create_with_id("registry_parent".into(), None, None),
        None,
    )));
    let (parent_model, parent_messages) = {
        let p = parent.lock().await;
        (p.provider_model(), p.message_count())
    };
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::from([("registry_parent".to_string(), parent.clone())]),
    ));
    let state = crate::server::SwarmState::new(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
    );
    let mcp = Arc::new(crate::mcp::SharedMcpPool::new(
        crate::mcp::McpConfig::default(),
    ));
    let dir = temp.path().to_string_lossy().into_owned();
    let (gsid, hist, ctr, tx, q, cc) = (
        Arc::new(RwLock::new(String::new())),
        Arc::new(RwLock::new(VecDeque::new())),
        Arc::new(AtomicU64::new(0)),
        broadcast::channel(16).0,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(HashMap::new())),
    );
    let spawn = |model: &'static str| {
        super::super::spawn_swarm_agent(
            "registry_parent",
            "registry-swarm",
            None,
            Some(dir.clone()),
            None,
            Some(crate::config::SwarmSpawnMode::Headless),
            Some(model.into()),
            None,
            Some("registry test".into()),
            &sessions,
            &gsid,
            &provider,
            &state.members,
            &state.swarms_by_id,
            &state.coordinators,
            &state.plans,
            &hist,
            &ctr,
            &tx,
            &mcp,
            &q,
            &cc,
        )
    };

    let child_id = spawn("mock-gamma").await.unwrap();
    let child = sessions.read().await.get(&child_id).cloned().unwrap();
    assert_eq!(child.lock().await.provider_model(), "mock-gamma");

    let err = spawn("not-configured").await.unwrap_err().to_string();
    assert!(err.contains("not an available configured route"), "{err}");
    assert!(err.contains("mock-alpha@code-assist-oauth"), "{err}");

    // The parent never switched model and gained no messages from delegation.
    let p = parent.lock().await;
    assert_eq!(p.provider_model(), parent_model);
    assert_eq!(p.message_count(), parent_messages);
    drop(p);
    match previous {
        Some(v) => crate::env::set_var("JCODE_HOME", v),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    crate::config::invalidate_config_cache();
}
