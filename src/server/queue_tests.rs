use super::{
    SessionInterruptQueues, queue_soft_interrupt_for_session, register_session_interrupt_queue,
};
use crate::agent::Agent;
use crate::message::{Message, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use jcode_agent_runtime::SoftInterruptSource;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

struct TestProvider;

#[async_trait]
impl Provider for TestProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        unimplemented!("test provider")
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(TestProvider)
    }
}

async fn test_agent() -> Arc<Mutex<Agent>> {
    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let registry = Registry::new(provider.clone()).await;
    Arc::new(Mutex::new(Agent::new(provider, registry)))
}

#[tokio::test]
async fn queue_soft_interrupt_for_session_uses_registered_queue_when_agent_busy() {
    let agent = test_agent().await;
    let session_id = {
        let guard = agent.lock().await;
        guard.session_id().to_string()
    };
    let queue = {
        let guard = agent.lock().await;
        guard.soft_interrupt_queue()
    };
    let queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    register_session_interrupt_queue(&queues, &session_id, queue.clone()).await;
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        agent.clone(),
    )])));

    let _busy_guard = agent.lock().await;
    let queued = queue_soft_interrupt_for_session(
        &session_id,
        "queued while busy".to_string(),
        false,
        SoftInterruptSource::User,
        &queues,
        &sessions,
    )
    .await;

    assert!(
        queued,
        "interrupt should queue even while agent lock is held"
    );
    let pending = queue.lock().expect("queue lock");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "queued while busy");
    assert!(!pending[0].urgent);
    assert_eq!(pending[0].source, SoftInterruptSource::User);
}

#[tokio::test]
async fn queue_soft_interrupt_for_session_registers_queue_on_fallback_lookup() {
    let agent = test_agent().await;
    let session_id = {
        let guard = agent.lock().await;
        guard.session_id().to_string()
    };
    let queue = {
        let guard = agent.lock().await;
        guard.soft_interrupt_queue()
    };
    let queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        agent.clone(),
    )])));

    let queued = queue_soft_interrupt_for_session(
        &session_id,
        "fallback lookup".to_string(),
        true,
        SoftInterruptSource::System,
        &queues,
        &sessions,
    )
    .await;

    assert!(queued, "interrupt should queue via session fallback");
    assert!(
        queues.read().await.contains_key(&session_id),
        "fallback should cache the session queue for later busy sends"
    );
    let pending = queue.lock().expect("queue lock");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "fallback lookup");
    assert!(pending[0].urgent);
    assert_eq!(pending[0].source, SoftInterruptSource::System);
}
