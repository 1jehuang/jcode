use super::{
    apply_or_defer_subscribe_working_dir, claim_live_target_agent, effective_subscribe_working_dir,
    handle_clear_session, handle_reload, handle_resume_session, handle_subscribe,
    mark_remote_reload_started, remove_detached_source_if_unclaimed, rename_shutdown_signal,
    rename_swarm_member_session, restored_session_was_interrupted,
    session_was_interrupted_by_reload, subscribe_should_mark_ready,
    subscribe_working_dir_replacement,
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

#[tokio::test]
async fn resume_rename_releases_member_lock_before_waiting_for_swarm_map() {
    let old_session_id = "session-old";
    let new_session_id = "session-new";
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (
            old_session_id.to_string(),
            test_swarm_member(old_session_id, "spawned"),
        ),
        (
            "child".to_string(),
            SwarmMember {
                report_back_to_session_id: Some(old_session_id.to_string()),
                ..test_swarm_member("child", "running")
            },
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-test".to_string(),
        HashSet::from([old_session_id.to_string(), "child".to_string()]),
    )])));

    // Force the rename to wait for swarms_by_id. While it waits, the member map
    // must remain readable or coordinator cleanup can form a permanent cycle.
    let swarm_map_guard = swarms_by_id.write().await;
    let rename_task = tokio::spawn({
        let swarm_members = Arc::clone(&swarm_members);
        let swarms_by_id = Arc::clone(&swarms_by_id);
        async move {
            rename_swarm_member_session(
                old_session_id,
                new_session_id,
                &swarm_members,
                &swarms_by_id,
            )
            .await;
        }
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let members = swarm_members.read().await;
            if members.contains_key(new_session_id) {
                assert_eq!(
                    members
                        .get("child")
                        .and_then(|member| member.report_back_to_session_id.as_deref()),
                    Some(new_session_id)
                );
                break;
            }
            drop(members);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("member map stayed locked while waiting for swarm map");

    drop(swarm_map_guard);
    rename_task.await.expect("rename task");
    let swarms = swarms_by_id.read().await;
    let swarm = swarms.get("swarm-test").expect("swarm remains present");
    assert!(!swarm.contains(old_session_id));
    assert!(swarm.contains(new_session_id));
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

/// Issue #481: a subscribe cwd that is merely absolute is not enough. A client
/// reporting the *home* directory must not silently re-pin (or clobber) a
/// session that is already bound to a real project directory, because tools then
/// run in home while the UI still shows the project.
#[test]
fn subscribe_working_dir_ignores_home_when_session_has_a_project_dir() {
    let home = std::path::Path::new("/home/tester");
    let project = "/home/tester/work/project";

    assert_eq!(
        subscribe_working_dir_replacement(Some(project), "/home/tester", Some(home)),
        None,
        "home must not clobber an established project cwd"
    );

    // A session with no cwd yet, or one already in home, may legitimately use home.
    assert_eq!(
        subscribe_working_dir_replacement(None, "/home/tester", Some(home)),
        Some("/home/tester".to_string())
    );
    assert_eq!(
        subscribe_working_dir_replacement(Some("/home/tester"), "/home/tester", Some(home)),
        None,
        "an unchanged cwd needs no reassignment"
    );

    // Genuine project-to-project moves still apply.
    assert_eq!(
        subscribe_working_dir_replacement(Some(project), "/home/tester/work/other", Some(home)),
        Some("/home/tester/work/other".to_string())
    );

    // A subdirectory of home that is not home itself is a real project path.
    assert_eq!(
        subscribe_working_dir_replacement(Some(project), "/home/tester/scratch", Some(home)),
        Some("/home/tester/scratch".to_string())
    );

    // Blank/whitespace reports are never applied, and an unknown home disables
    // the guard rather than rejecting valid directories.
    assert_eq!(
        subscribe_working_dir_replacement(Some(project), "   ", Some(home)),
        None
    );
    assert_eq!(
        subscribe_working_dir_replacement(Some(project), "/home/tester", None),
        Some("/home/tester".to_string())
    );
}

/// Issue #481: agent cwd, swarm grouping, and project-local MCP resolution must
/// all bind to the *same* directory. If a rejected home-dir report still reached
/// the swarm id or the MCP resolver, tools would run in the project while swarm
/// membership and `.jcode/mcp.json` discovery pointed at home.
#[test]
fn effective_subscribe_working_dir_binds_all_consumers_to_one_directory() {
    let home = std::path::Path::new("/home/tester");
    let project = "/home/tester/work/project";

    // Rejected home report: every consumer keeps the project dir.
    assert_eq!(
        effective_subscribe_working_dir(Some(project), "/home/tester", Some(home)),
        project
    );

    // Accepted move: every consumer follows to the new dir.
    assert_eq!(
        effective_subscribe_working_dir(Some(project), "/home/tester/work/other", Some(home)),
        "/home/tester/work/other"
    );

    // No prior cwd: the report is authoritative, including home.
    assert_eq!(
        effective_subscribe_working_dir(None, "/home/tester", Some(home)),
        "/home/tester"
    );

    // Unchanged report resolves to the same dir rather than dropping it.
    assert_eq!(
        effective_subscribe_working_dir(Some(project), project, Some(home)),
        project
    );
}

/// Issue #481 end to end: drive the real subscribe-cwd application path against
/// a live `Agent` and assert the agent's stored working directory, which is what
/// bash/file tools actually run in. The pure-resolver tests above cover the
/// decision; this covers the wiring that applies it.
#[tokio::test]
async fn apply_subscribe_working_dir_keeps_project_when_client_reports_home() {
    let home = dirs::home_dir().expect("home directory");
    let home_str = home.to_string_lossy().to_string();
    let project = home.join("jcode-481-project");
    let project_str = project.to_string_lossy().to_string();

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(Arc::clone(&provider)).await;
    let agent = Arc::new(Mutex::new(Agent::new_with_initial_working_dir(
        provider,
        registry,
        Some(&project_str),
    )));

    assert_eq!(
        agent.lock().await.working_dir(),
        Some(project_str.as_str()),
        "precondition: session starts bound to the project"
    );

    // A client whose inherited cwd is home must not re-pin the session.
    apply_or_defer_subscribe_working_dir(&agent, &home_str, "session_test_481");
    assert_eq!(
        agent.lock().await.working_dir(),
        Some(project_str.as_str()),
        "a home-dir subscribe must not clobber the project cwd"
    );

    // A genuine project-to-project move still applies.
    let other = home.join("jcode-481-other");
    let other_str = other.to_string_lossy().to_string();
    apply_or_defer_subscribe_working_dir(&agent, &other_str, "session_test_481");
    assert_eq!(
        agent.lock().await.working_dir(),
        Some(other_str.as_str()),
        "a real directory change must still be honored"
    );
}

#[path = "client_session_tests/clear.rs"]
mod clear_tests;
#[path = "client_session_tests/reload.rs"]
mod reload_tests;
#[path = "client_session_tests/resume.rs"]
mod resume_tests;
