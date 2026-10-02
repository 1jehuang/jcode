#![allow(clippy::await_holding_lock)]

use super::*;
use crate::protocol::ServerEvent;
use crate::provider::{EventStream, Provider};
use crate::session::{Session, SessionStatus};
use crate::tool::Registry;
use async_trait::async_trait;
use std::time::Instant;
use tokio::time::timeout;

struct NoRequests;
#[async_trait]
impl Provider for NoRequests {
    async fn complete(
        &self,
        _: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        anyhow::bail!("disconnect tests must not request a provider")
    }
    fn name(&self) -> &str {
        "mock"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

struct Home {
    _dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}
impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", dir.path());
        Self {
            _dir: dir,
            previous,
        }
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

struct Fixture {
    id: String,
    agent: Arc<Mutex<Agent>>,
    sessions: SessionAgents,
    members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    events: mpsc::UnboundedSender<ServerEvent>,
}
impl Fixture {
    async fn new(persisted: bool) -> Self {
        let provider: Arc<dyn Provider> = Arc::new(NoRequests);
        let registry = Registry::new(provider.clone()).await;
        let mut session = Session::create(None, None);
        if persisted {
            session.title = Some("Explicit saved panel".into());
            session.save().unwrap();
        }
        let id = session.id.clone();
        let mut agent = Agent::new_with_session(provider, registry, session, None);
        agent.set_memory_enabled(false);
        let agent = Arc::new(Mutex::new(agent));
        let (events, _) = mpsc::unbounded_channel();
        let members = Arc::new(RwLock::new(HashMap::from([(
            id.clone(),
            SwarmMember {
                session_id: id.clone(),
                event_tx: events.clone(),
                event_txs: HashMap::from([("original".into(), events.clone())]),
                working_dir: None,
                swarm_id: None,
                swarm_enabled: false,
                status: "ready".into(),
                detail: None,
                task_label: None,
                friendly_name: None,
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "agent".into(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: Default::default(),
            },
        )])));
        let sessions = Arc::new(RwLock::new(HashMap::from([(id.clone(), agent.clone())])));
        let connections = Arc::new(RwLock::new(HashMap::from([(
            "original".into(),
            connection("original", &id),
        )])));
        Self {
            id,
            agent,
            sessions,
            members,
            connections,
            events,
        }
    }

    async fn cleanup(&self, processing: bool, grace: Duration) {
        assert!(
            self.cleanup_task(processing, grace, &mut None)
                .await
                .is_none()
        );
    }

    async fn cleanup_task(
        &self,
        processing: bool,
        grace: Duration,
        task: &mut Option<tokio::task::JoinHandle<()>>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let (swarm_events, _) = broadcast::channel(8);
        cleanup_client_connection(
            &self.sessions,
            &self.id,
            processing,
            task,
            tokio::spawn(std::future::pending()),
            &self.members,
            &Arc::new(RwLock::new(HashMap::new())),
            &Arc::new(RwLock::new(HashMap::new())),
            &Arc::new(RwLock::new(HashMap::new())),
            &FileTouchService::new(),
            &Arc::new(RwLock::new(HashMap::new())),
            &Arc::new(RwLock::new(HashMap::new())),
            &Arc::new(RwLock::new(ClientDebugState::default())),
            "debug-original",
            &self.connections,
            "original",
            &Arc::new(RwLock::new(HashMap::new())),
            &Arc::new(RwLock::new(HashMap::new())),
            &Arc::new(RwLock::new(std::collections::VecDeque::new())),
            &Arc::new(std::sync::atomic::AtomicU64::new(0)),
            &swarm_events,
            &self.events,
            grace,
        )
        .await
        .unwrap()
    }

    async fn wait_for_detach(&self) {
        timeout(Duration::from_secs(1), async {
            while self.connections.read().await.contains_key("original") {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cleanup releases attachment registry promptly");
    }

    async fn attach_successor(&self) {
        // Reserve the same live Agent under the same registry lock order used
        // by claim_live_target_agent, then register the real event attachment.
        let mut connections = self.connections.write().await;
        assert!(Arc::ptr_eq(
            self.sessions.read().await.get(&self.id).unwrap(),
            &self.agent
        ));
        connections.insert("successor".into(), connection("successor", &self.id));
        drop(connections);
        let (sender, _) = mpsc::unbounded_channel();
        crate::server::register_session_event_sender(&self.members, &self.id, "successor", sender)
            .await;
    }
}

fn connection(name: &str, id: &str) -> ClientConnectionInfo {
    let (disconnect_tx, _) = mpsc::unbounded_channel();
    ClientConnectionInfo {
        client_id: name.into(),
        session_id: id.into(),
        client_instance_id: None,
        debug_client_id: None,
        connected_at: Instant::now(),
        last_seen: Instant::now(),
        is_processing: false,
        current_tool_name: None,
        terminal_env: vec![],
        disconnect_tx,
    }
}

#[tokio::test]
async fn unsaved_idle_session_retains_same_agent_for_reattachment() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let fixture = Fixture::new(false).await;
    let ((), ()) = tokio::join!(fixture.cleanup(false, Duration::from_secs(2)), async {
        fixture.wait_for_detach().await;
        assert!(!crate::session::session_exists(&fixture.id));
        // These locks must remain available during the reconnect grace.
        let _agent = fixture
            .agent
            .try_lock()
            .expect("grace cannot hold agent lock");
        drop(_agent);
        fixture.attach_successor().await;
    });
    assert!(Arc::ptr_eq(
        fixture.sessions.read().await.get(&fixture.id).unwrap(),
        &fixture.agent
    ));
    assert!(fixture.connections.read().await.contains_key("successor"));
    assert!(fixture.members.read().await.contains_key(&fixture.id));
    assert!(!crate::session::session_exists(&fixture.id));
}

#[tokio::test]
async fn unsaved_idle_session_expires_without_persisting_or_leaking() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let fixture = Fixture::new(false).await;
    let grace = Duration::from_millis(60);
    let start = Instant::now();
    timeout(Duration::from_secs(1), fixture.cleanup(false, grace))
        .await
        .unwrap();
    assert!(start.elapsed() >= grace);
    assert!(!fixture.sessions.read().await.contains_key(&fixture.id));
    assert!(!fixture.members.read().await.contains_key(&fixture.id));
    assert!(fixture.connections.read().await.is_empty());
    assert!(!crate::session::session_exists(&fixture.id));
}

#[tokio::test]
async fn old_grace_cannot_remove_successor_that_already_detached_again() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let fixture = Fixture::new(false).await;
    tokio::join!(fixture.cleanup(false, Duration::from_millis(100)), async {
        fixture.wait_for_detach().await;
        fixture.attach_successor().await;
        detach_client_attachment(
            &fixture.id,
            "successor",
            "successor-debug",
            &fixture.connections,
            &Arc::new(RwLock::new(ClientDebugState::default())),
            &fixture.members,
        )
        .await;
    });
    assert!(fixture.connections.read().await.is_empty());
    assert!(
        fixture.sessions.read().await.contains_key(&fixture.id),
        "successor owns its own grace/cleanup"
    );
    assert!(fixture.members.read().await.contains_key(&fixture.id));
}

#[tokio::test]
async fn persisted_idle_session_does_not_wait_for_reconnect_grace() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let fixture = Fixture::new(true).await;
    timeout(
        Duration::from_secs(1),
        fixture.cleanup(false, Duration::from_secs(30)),
    )
    .await
    .unwrap();
    assert!(fixture.sessions.read().await.is_empty());
    assert!(crate::session::session_exists(&fixture.id));
}

#[tokio::test]
async fn interrupted_session_does_not_wait_for_reconnect_grace() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    crate::server::clear_reload_marker();
    let fixture = Fixture::new(false).await;
    timeout(
        Duration::from_secs(1),
        fixture.cleanup(true, Duration::from_secs(30)),
    )
    .await
    .unwrap();
    assert!(fixture.sessions.read().await.is_empty());
    assert!(matches!(
        fixture.agent.lock().await.session_for_split().status,
        SessionStatus::Crashed { .. }
    ));
}

#[tokio::test]
async fn busy_successor_retains_owner_task_and_completion_receiver() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let fixture = Fixture::new(true).await;
    fixture.attach_successor().await;
    let (finish, wait) = tokio::sync::oneshot::channel();
    let (done, mut completions) = mpsc::unbounded_channel();
    let mut task = Some(tokio::spawn(async move {
        wait.await.unwrap();
        done.send("completed").unwrap();
    }));
    let writer = fixture
        .cleanup_task(true, Duration::ZERO, &mut task)
        .await
        .expect("successor returns ownership to lifecycle continuation");
    assert!(!task.as_ref().unwrap().is_finished());
    assert!(fixture.sessions.read().await.contains_key(&fixture.id));
    assert!(fixture.connections.read().await.contains_key("successor"));
    assert!(!fixture.connections.read().await.contains_key("original"));
    writer.abort();
    finish.send(()).unwrap();
    task.take().unwrap().await.expect("owner was not cancelled");
    assert_eq!(completions.recv().await, Some("completed"));
    fixture.cleanup(false, Duration::ZERO).await;
    assert!(fixture.sessions.read().await.contains_key(&fixture.id));
}

#[tokio::test]
async fn busy_without_successor_aborts_owner_and_marks_crashed() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    crate::server::clear_reload_marker();
    let fixture = Fixture::new(true).await;
    let (dropped, observed_drop) = tokio::sync::oneshot::channel::<()>();
    let mut task = Some(tokio::spawn(async move {
        let _guard = dropped;
        std::future::pending::<()>().await;
    }));
    assert!(
        fixture
            .cleanup_task(true, Duration::ZERO, &mut task)
            .await
            .is_none()
    );
    assert!(task.is_none());
    assert!(
        timeout(Duration::from_secs(1), observed_drop)
            .await
            .unwrap()
            .is_err()
    );
    assert!(fixture.sessions.read().await.is_empty());
    assert!(matches!(
        fixture.agent.lock().await.session_for_split().status,
        SessionStatus::Crashed { .. }
    ));
}
