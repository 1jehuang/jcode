use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct SessionAffinityProvider {
    session: Mutex<Option<String>>,
    prewarmed: Mutex<Vec<String>>,
}

#[async_trait]
impl Provider for SessionAffinityProvider {
    fn set_session_id(&self, session_id: &str) {
        *self.session.lock().unwrap() = Some(session_id.to_string());
    }

    async fn prewarm(&self, _tools: &[ToolDefinition], _system: &str) {
        self.prewarmed.lock().unwrap().push(
            self.session
                .lock()
                .unwrap()
                .clone()
                .expect("session bound before prewarm"),
        );
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("session affinity test must not call the network")
    }

    fn name(&self) -> &'static str {
        "session-affinity-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self::default())
    }
}

#[tokio::test]
async fn cache_session_affinity_binds_new_and_restored_agents_before_prewarm() {
    let provider = Arc::new(SessionAffinityProvider::default());
    let agent = Agent::new(provider.clone(), Registry::empty());
    let session_id = agent.session.id.clone();
    assert_eq!(
        provider.session.lock().unwrap().as_deref(),
        Some(session_id.as_str())
    );
    agent.prewarm_provider_idle().await;
    agent.prewarm_provider().await;
    assert_eq!(
        *provider.prewarmed.lock().unwrap(),
        vec![session_id.clone(), session_id.clone()]
    );

    // Restoring an existing session must reuse its persisted identity, rather
    // than minting a new routing key each time the daemon constructs a provider.
    let restored_provider = Arc::new(SessionAffinityProvider::default());
    let restored = Agent::new_with_session(
        restored_provider.clone(),
        Registry::empty(),
        agent.session.clone(),
        None,
    );
    assert_eq!(restored.session.id, session_id);
    assert_eq!(
        restored_provider.session.lock().unwrap().as_deref(),
        Some(session_id.as_str())
    );

    let independent_provider = Arc::new(SessionAffinityProvider::default());
    let independent = Agent::new(independent_provider.clone(), Registry::empty());
    assert_ne!(independent.session.id, session_id);
    assert_eq!(
        independent_provider.session.lock().unwrap().as_deref(),
        Some(independent.session.id.as_str())
    );
}
