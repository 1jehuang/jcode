#![allow(clippy::await_holding_lock)]

use super::*;
use crate::protocol::ServerEvent;
use crate::provider::{EventStream, Provider};
use crate::session::{Session, SessionStatus};
use crate::tool::Registry;
use async_trait::async_trait;
use std::time::Instant;
use tokio::sync::Mutex;
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
    swarms: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    coordinators: Arc<RwLock<HashMap<String, String>>>,
    history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
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
        let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
            HashMap::from([(id.clone(), agent.clone())]),
        ));
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
            swarms: Arc::new(RwLock::new(HashMap::new())),
            coordinators: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(std::collections::VecDeque::new())),
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
        self.cleanup_with_continuation(processing, false, grace, task)
            .await
    }

    async fn cleanup_with_continuation(
        &self,
        processing: bool,
        continue_on_disconnect: bool,
        grace: Duration,
        task: &mut Option<tokio::task::JoinHandle<()>>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let (swarm_events, _) = broadcast::channel(8);
        cleanup_client_connection(
            &self.sessions,
            &self.id,
            processing,
            continue_on_disconnect,
            task,
            tokio::spawn(std::future::pending()),
            &self.members,
            &self.swarms,
            &self.coordinators,
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
            &self.history,
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
async fn continued_idle_coordinator_survives_disconnect_without_reconnect_grace() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    for persisted in [false, true] {
        let fixture = Fixture::new(persisted).await;
        fixture
            .swarms
            .write()
            .await
            .insert("remote-swarm".into(), HashSet::from([fixture.id.clone()]));
        fixture
            .coordinators
            .write()
            .await
            .insert("remote-swarm".into(), fixture.id.clone());
        {
            let mut members = fixture.members.write().await;
            let member = members.get_mut(&fixture.id).unwrap();
            member.role = "coordinator".into();
            member.swarm_id = Some("remote-swarm".into());
            member.swarm_enabled = true;
        }
        assert!(
            timeout(
                Duration::from_secs(1),
                fixture.cleanup_with_continuation(false, true, Duration::from_secs(30), &mut None),
            )
            .await
            .unwrap()
            .is_none()
        );
        assert!(fixture.connections.read().await.is_empty());
        assert!(
            fixture.history.read().await.is_empty(),
            "no stopped/member-left event"
        );
        assert!(fixture.swarms.read().await["remote-swarm"].contains(&fixture.id));
        assert_eq!(
            fixture.coordinators.read().await["remote-swarm"],
            fixture.id
        );
        assert!(Arc::ptr_eq(
            fixture.sessions.read().await.get(&fixture.id).unwrap(),
            &fixture.agent
        ));
        let members = fixture.members.read().await;
        let member = members
            .get(&fixture.id)
            .expect("worker reports still resolve coordinator");
        assert_eq!(member.status, "ready");
        assert_eq!(member.swarm_id.as_deref(), Some("remote-swarm"));
        assert!(member.event_txs.is_empty(), "transport sender was detached");
        drop(members);
        fixture.attach_successor().await;
    }
}

#[tokio::test]
async fn continued_turn_remains_live_after_owner_finishes() {
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let fixture = Fixture::new(true).await;
    let (finish, wait) = tokio::sync::oneshot::channel();
    let mut task = Some(tokio::spawn(async move { wait.await.unwrap() }));
    let writer = fixture
        .cleanup_with_continuation(true, true, Duration::ZERO, &mut task)
        .await
        .expect("lifecycle retains completion ownership");
    finish.send(()).unwrap();
    task.take()
        .unwrap()
        .await
        .expect("disconnect must not cancel turn");
    writer.abort();
    // Mirrors the lifecycle's final cleanup after recording completion. The
    // continuation policy must still apply when there is no processing task.
    assert!(
        fixture
            .cleanup_with_continuation(false, true, Duration::ZERO, &mut task)
            .await
            .is_none()
    );
    assert!(fixture.sessions.read().await.contains_key(&fixture.id));
    assert_eq!(fixture.members.read().await[&fixture.id].status, "ready");
    assert!(!matches!(
        fixture.agent.lock().await.session_for_split().status,
        SessionStatus::Closed | SessionStatus::Crashed { .. }
    ));
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
async fn unsaved_idle_session_without_swarm_member_still_expires() {
    // A client that disconnects before subscribe registers a swarm member
    // (window closed mid-subscribe) must not be mistaken for a replaced
    // attachment. Previously this leaked the Agent for the server lifetime.
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let fixture = Fixture::new(false).await;
    fixture.members.write().await.clear();
    timeout(
        Duration::from_secs(1),
        fixture.cleanup(false, Duration::from_millis(60)),
    )
    .await
    .unwrap();
    assert!(!fixture.sessions.read().await.contains_key(&fixture.id));
    assert!(fixture.connections.read().await.is_empty());
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

fn run_extraction_child(name: &str) -> bool {
    if std::env::var_os("JCODE_DISCONNECT_EXTRACTION_CHILD").is_some() {
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("server::client_disconnect_cleanup::grace_tests::{name}"),
            "--nocapture",
        ])
        .env("JCODE_DISCONNECT_EXTRACTION_CHILD", "1")
        .env("HOME", home.path())
        .env("JCODE_HOME", home.path().join("state"))
        .env("JCODE_RUNTIME_DIR", home.path().join("runtime"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[test]
fn reload_disconnect_skips_final_extraction_but_normal_disconnect_retains_it() {
    if run_extraction_child(
        "reload_disconnect_skips_final_extraction_but_normal_disconnect_retains_it",
    ) {
        return;
    }
    let _lock = crate::storage::lock_test_env();
    let _home = Home::new();
    let log_dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".jcode/logs");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut ids = Vec::new();
        for reloading in ["before", "during", "normal"] {
            let fixture = Fixture::new(true).await;
            {
                let mut agent = fixture.agent.lock().await;
                agent.set_memory_enabled(true);
                agent.add_message(
                    crate::message::Role::User,
                    vec![crate::message::ContentBlock::Text {
                        text: "Keep this durable build instruction in memory. ".repeat(10),
                        cache_control: None,
                    }],
                );
            }
            if reloading == "before" {
                crate::server::write_reload_state(
                    "memory-reload-fixture",
                    "fixture-hash",
                    crate::server::ReloadPhase::Starting,
                    None,
                );
            } else {
                crate::server::clear_reload_marker();
            }
            if reloading == "during" {
                let agent = fixture.agent.lock().await;
                let cleanup = fixture.cleanup(false, Duration::ZERO);
                tokio::pin!(cleanup);
                timeout(Duration::from_secs(2), async {
                    loop {
                        tokio::select! {
                            result = &mut cleanup => panic!("cleanup must wait for held Agent: {result:?}"),
                            _ = tokio::task::yield_now() => {
                                if !fixture.sessions.read().await.contains_key(&fixture.id) {
                                    break;
                                }
                            }
                        }
                    }
                }).await.expect("cleanup reaches the removed-session Agent lock");
                crate::server::write_reload_state(
                    "memory-reload-late",
                    "fixture-hash",
                    crate::server::ReloadPhase::Starting,
                    None,
                );
                drop(agent);
                cleanup.await;
            } else {
                fixture.cleanup(false, Duration::ZERO).await;
            }
            assert!(fixture.sessions.read().await.is_empty());
            ids.push(fixture.id);
        }
        let entries: Vec<serde_json::Value> = std::fs::read_dir(log_dir)
            .unwrap()
            .flat_map(|entry| {
                std::fs::read_to_string(entry.unwrap().path())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .collect::<Vec<_>>()
            })
            .filter(|entry| entry["event"] == "final_extraction_started")
            .collect();
        assert!(
            !entries
                .iter()
                .any(|entry| entry["detail"]["target_session"] == ids[0]),
            "reload must not trigger final extraction"
        );
        assert!(
            !entries
                .iter()
                .any(|entry| entry["detail"]["target_session"] == ids[1]),
            "reload starting while cleanup waits must not trigger extraction"
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry["detail"]["target_session"] == ids[2]),
            "normal disconnect still triggers final extraction"
        );
    });
}

#[derive(Clone, Default)]
struct ExtractionCounter(Arc<std::sync::Mutex<Vec<String>>>);

#[async_trait]
impl Provider for ExtractionCounter {
    async fn complete(
        &self,
        messages: &[crate::message::Message],
        _: &[crate::message::ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        let text = messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| {
                if let crate::message::ContentBlock::Text { text, .. } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.0.lock().unwrap().push(text);
        Ok(Box::pin(futures::stream::once(async {
            Ok(crate::message::StreamEvent::TextDelta(String::new()))
        })))
    }
    fn name(&self) -> &str {
        "memory-checkpoint-fixture"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn extraction_completed() {
    timeout(Duration::from_secs(5), async {
        loop {
            if crate::memory::get_activity().is_some_and(|activity| {
                activity.recent_events.iter().any(|event| {
                    matches!(
                        event.kind,
                        crate::memory_types::MemoryEventKind::ExtractionComplete { .. }
                    )
                })
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("scheduled memory extraction settles");
}

#[test]
fn periodic_then_disconnect_deduplicates_equivalent_input_but_keeps_full_history() {
    if run_extraction_child(
        "periodic_then_disconnect_deduplicates_equivalent_input_but_keeps_full_history",
    ) {
        return;
    }
    let _sandbox = jcode_base::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    crate::env::set_var("JCODE_MEMORY_JEV_PROVIDER", "disabled-for-test");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let provider = ExtractionCounter::default();
        let mut expected_calls = 0;
        for large in [false, true] {
            let fixture = Fixture::new(true).await;
            let (messages, working_dir) = {
                let mut agent = fixture.agent.lock().await;
                agent.set_memory_enabled(true);
                let count = if large { 60 } else { 2 };
                for index in 0..count {
                    let text = if index == 0 {
                        "EARLY_HISTORY_ONLY: ".to_string()
                    } else {
                        String::new()
                    } + &format!("Remember fixture message {index}. ").repeat(8);
                    agent.add_message(
                        crate::message::Role::User,
                        vec![crate::message::ContentBlock::Text {
                            text,
                            cache_control: None,
                        }],
                    );
                }
                (agent.session_for_split().messages_for_provider_uncached(), agent.working_dir().map(str::to_owned))
            };
            crate::provider::set_active_provider(Arc::new(provider.clone()));
            assert_eq!(crate::sidecar::Sidecar::new().backend_name(), "provider");
            crate::memory::clear_activity();
            let handle = crate::memory_agent::init().await.unwrap();
            for _ in 0..12 {
                handle
                    .update_context(&fixture.id, messages.clone().into(), working_dir.clone())
                    .await;
            }
            extraction_completed().await;
            expected_calls += 1;
            assert_eq!(provider.0.lock().unwrap().len(), expected_calls);
            if large {
                assert!(
                    !provider
                        .0
                        .lock()
                        .unwrap()
                        .last()
                        .unwrap()
                        .contains("EARLY_HISTORY_ONLY")
                );
            }
            crate::memory::clear_activity();
            fixture.cleanup(false, Duration::ZERO).await;
            extraction_completed().await;
            if large {
                expected_calls += 1;
                assert!(
                    provider
                        .0
                        .lock()
                        .unwrap()
                        .last()
                        .unwrap()
                        .contains("EARLY_HISTORY_ONLY")
                );
            }
            {
                let inputs = provider.0.lock().unwrap();
                assert_eq!(inputs.len(), expected_calls, "unchanged represented history must not call the sidecar twice; input byte lengths={:?}; first differing byte={:?}", inputs.iter().map(String::len).collect::<Vec<_>>(), inputs.first().zip(inputs.last()).and_then(|(first,last)| first.bytes().zip(last.bytes()).position(|(first,last)|first!=last)));
            }
        }
    });
}
