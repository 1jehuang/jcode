use crate::agent::Agent;
use crate::provider::Provider;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use tokio::sync::Mutex;

/// Provider handle of each shared agent, readable without the agent lock.
/// An agent's provider is fixed at construction, so this never goes stale.
/// Keyed by the agent's `Arc` address, checked against a `Weak` so a reused
/// address never matches a dead agent. The provider is held weakly too: the
/// agent owns it, so a closed session's provider is freed with the agent.
type AgentProviderEntry = (std::sync::Weak<Mutex<Agent>>, std::sync::Weak<dyn Provider>);
static AGENT_PROVIDERS: LazyLock<StdMutex<HashMap<usize, AgentProviderEntry>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

fn prune_agent_providers(map: &mut HashMap<usize, AgentProviderEntry>) {
    map.retain(|_, (agent, provider)| agent.strong_count() > 0 && provider.strong_count() > 0);
}

/// Record the provider an agent streams with. Call wherever a shared agent is built.
pub(crate) fn register_agent_provider(agent: &Arc<Mutex<Agent>>, provider: Arc<dyn Provider>) {
    let mut map = AGENT_PROVIDERS.lock().unwrap_or_else(|e| e.into_inner());
    prune_agent_providers(&mut map);
    map.insert(
        Arc::as_ptr(agent) as usize,
        (Arc::downgrade(agent), Arc::downgrade(&provider)),
    );
}

/// The provider `agent` streams with, without taking the agent lock.
pub(crate) fn agent_provider(agent: &Arc<Mutex<Agent>>) -> Option<Arc<dyn Provider>> {
    let mut map = AGENT_PROVIDERS.lock().unwrap_or_else(|e| e.into_inner());
    prune_agent_providers(&mut map);
    map.get(&(Arc::as_ptr(agent) as usize))
        .filter(|(weak, _)| weak.upgrade().is_some_and(|live| Arc::ptr_eq(&live, agent)))
        .and_then(|(_, provider)| provider.upgrade())
}

/// Do not probe any identity until telemetry is enabled. Only the provider
/// may identify its effective OAuth credential. An API-key route must not expose the email
/// of an unrelated saved subscription account.
pub(crate) fn provider_oauth_email(
    provider: &dyn Provider,
) -> Option<crate::protocol::ProviderOAuthEmail> {
    selected_provider_oauth_email(provider, crate::telemetry::is_enabled(), || {
        provider.active_oauth_account_email()
    })
}

pub(super) fn selected_provider_oauth_email(
    provider: &dyn Provider,
    telemetry_enabled: bool,
    resolve: impl FnOnce() -> Option<String>,
) -> Option<crate::protocol::ProviderOAuthEmail> {
    if !telemetry_enabled
        || provider.active_resolved_credential()
            == Some(jcode_provider_core::ResolvedCredential::ApiKey)
    {
        return None;
    }
    // The provider owns the effective credential snapshot, including unbound
    // active OAuth and account failover. Never substitute a global login here.
    resolve().map(crate::protocol::ProviderOAuthEmail::new)
}
