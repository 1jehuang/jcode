#![cfg_attr(test, allow(clippy::await_holding_lock))]

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
        Err(anyhow::anyhow!(
            "test provider complete should not be called in queue tests"
        ))
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

#[tokio::test]
async fn queue_soft_interrupt_for_session_persists_when_live_queue_is_unavailable() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let agent = test_agent().await;
    let session_id = {
        let guard = agent.lock().await;
        guard.session_id().to_string()
    };
    crate::session::Session::create_with_id(session_id.clone(), None, None)
        .save()
        .expect("save session snapshot");

    let queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let sessions = Arc::new(RwLock::new(HashMap::new()));

    let queued = queue_soft_interrupt_for_session(
        &session_id,
        "persist while reloading".to_string(),
        false,
        SoftInterruptSource::BackgroundTask,
        &queues,
        &sessions,
    )
    .await;

    assert!(
        queued,
        "interrupt should persist when live queue is unavailable"
    );

    let persisted =
        crate::soft_interrupt_store::load(&session_id).expect("load persisted interrupts");
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].content, "persist while reloading");
    assert_eq!(persisted[0].source, SoftInterruptSource::BackgroundTask);

    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut restored = Agent::new(provider, registry);
    restored
        .restore_session(&session_id)
        .expect("restore session should rehydrate interrupts");
    assert_eq!(restored.soft_interrupt_count(), 1);
    assert!(
        crate::soft_interrupt_store::load(&session_id)
            .expect("load persisted interrupts after restore")
            .is_empty()
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
