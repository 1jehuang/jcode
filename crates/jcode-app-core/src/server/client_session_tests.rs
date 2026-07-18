use super::{
    claim_live_target_agent, handle_clear_session, handle_reload, handle_resume_session,
    handle_subscribe, mark_remote_reload_started, remove_detached_source_if_unclaimed,
    rename_shutdown_signal, restored_session_was_interrupted, session_was_interrupted_by_reload,
    subscribe_should_mark_ready,
};
use crate::agent::Agent;
use crate::message::ContentBlock;
use crate::message::{Message, ToolDefinition};
use crate::protocol::ServerEvent;
use crate::provider::{EventStream, Provider};
use crate::server::{
    ClientConnectionInfo, ClientDebugState, FileTouchService, SessionInterruptQueues, SwarmEvent,
    SwarmMember, VersionedPlan,
};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

struct MockProvider;

fn test_swarm_member(session_id: &str, status: &str) -> SwarmMember {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    SwarmMember {
        session_id: session_id.to_string(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some("swarm-test".to_string()),
        swarm_enabled: true,
        status: status.to_string(),
        detail: None,
        task_label: None,
        friendly_name: Some(session_id.to_string()),
        report_back_to_session_id: Some("coord".to_string()),
        latest_completion_report: None,
        role: "agent".to_string(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: false,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: crate::protocol::SwarmMemberRuntime::default(),
    }
}

#[tokio::test]
async fn subscribe_does_not_mark_running_startup_worker_ready() {
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        "worker".to_string(),
        test_swarm_member("worker", "running"),
    )])));
    assert!(!subscribe_should_mark_ready("worker", &swarm_members).await);
}

#[tokio::test]
async fn subscribe_marks_non_running_member_ready() {
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        "worker".to_string(),
        test_swarm_member("worker", "spawned"),
    )])));
    assert!(subscribe_should_mark_ready("worker", &swarm_members).await);
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Err(anyhow::anyhow!(
            "mock provider complete should not be called in client_session tests"
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(MockProvider)
    }
}

fn test_agent(messages: Vec<crate::session::StoredMessage>) -> Agent {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let registry = rt.block_on(Registry::new(provider.clone()));
    build_test_agent(provider, registry, messages)
}

fn build_test_agent(
    provider: Arc<dyn Provider>,
    registry: Registry,
    messages: Vec<crate::session::StoredMessage>,
) -> Agent {
    let mut session =
        crate::session::Session::create_with_id("session_test_reload".to_string(), None, None);
    session.model = Some("mock".to_string());
    session.replace_messages(messages);
    Agent::new_with_session(provider, registry, session, None)
}

fn build_test_agent_with_id(
    provider: Arc<dyn Provider>,
    registry: Registry,
    session_id: &str,
    messages: Vec<crate::session::StoredMessage>,
) -> Agent {
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.model = Some("mock".to_string());
    session.replace_messages(messages);
    Agent::new_with_session(provider, registry, session, None)
}

async fn collect_events_until_done(
    client_event_rx: &mut mpsc::UnboundedReceiver<ServerEvent>,
    done_id: u64,
) -> Vec<ServerEvent> {
    let mut events = Vec::new();
    for _ in 0..16 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), client_event_rx.recv())
            .await
            .expect("timed out waiting for server event")
            .expect("expected server event");
        let is_done = matches!(event, ServerEvent::Done { id } if id == done_id);
        events.push(event);
        if is_done {
            break;
        }
    }
    events
}

#[tokio::test]
async fn live_target_claim_is_atomic_with_detached_source_cleanup() {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;

    for iteration in 0..32 {
        let target_id = format!("session_atomic_target_{iteration}");
        let source_id = format!("session_atomic_source_{iteration}");
        let target_agent = Arc::new(Mutex::new(build_test_agent_with_id(
            provider.clone(),
            registry.clone(),
            &target_id,
            Vec::new(),
        )));
        let source_agent = Arc::new(Mutex::new(build_test_agent_with_id(
            provider.clone(),
            registry.clone(),
            &source_id,
            Vec::new(),
        )));
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            target_id.clone(),
            Arc::clone(&target_agent),
        )])));
        let now = Instant::now();
        let (disconnect_tx, _disconnect_rx) = mpsc::unbounded_channel();
        let connections = Arc::new(RwLock::new(HashMap::from([(
            "incoming".to_string(),
            ClientConnectionInfo {
                client_id: "incoming".to_string(),
                session_id: source_id,
                client_instance_id: None,
                debug_client_id: None,
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                terminal_env: Vec::new(),
                disconnect_tx,
            },
        )])));
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let claim = {
            let barrier = Arc::clone(&barrier);
            let sessions = Arc::clone(&sessions);
            let connections = Arc::clone(&connections);
            let source_agent = Arc::clone(&source_agent);
            let target_id = target_id.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                claim_live_target_agent(
                    &target_id,
                    "incoming",
                    Some("instance-a"),
                    &source_agent,
                    &sessions,
                    &connections,
                )
                .await
                .is_some()
            })
        };
        let cleanup = {
            let barrier = Arc::clone(&barrier);
            let sessions = Arc::clone(&sessions);
            let connections = Arc::clone(&connections);
            let target_agent = Arc::clone(&target_agent);
            let target_id = target_id.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                remove_detached_source_if_unclaimed(
                    &target_id,
                    "cleanup",
                    &target_agent,
                    &sessions,
                    &connections,
                )
                .await
            })
        };

        barrier.wait().await;
        let claimed = claim.await.expect("claim task should complete");
        let removed = cleanup.await.expect("cleanup task should complete");
        assert_ne!(claimed, removed, "exactly one transition must win");
        assert_eq!(
            sessions.read().await.contains_key(&target_id),
            claimed,
            "a successful claim must keep its target registered"
        );
        if claimed {
            let connections = connections.read().await;
            let incoming = connections.get("incoming").expect("incoming connection");
            assert_eq!(incoming.session_id, target_id);
            assert_eq!(incoming.client_instance_id.as_deref(), Some("instance-a"));
        }
    }
}

#[path = "client_session_tests/clear.rs"]
mod clear_tests;
#[path = "client_session_tests/reload.rs"]
mod reload_tests;
#[path = "client_session_tests/resume.rs"]
mod resume_tests;
