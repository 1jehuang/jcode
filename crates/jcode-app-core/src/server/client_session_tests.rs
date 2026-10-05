use super::super::subscribe_working_dir::{
    SubscribeWorkingDirRefusal, apply_or_defer_subscribe_working_dir,
    effective_subscribe_working_dir, handle_reload, handle_subscribe, prewarm_idle_agent,
    remove_detached_source_if_unclaimed, rename_swarm_member_session, subscribe_should_mark_ready,
    subscribe_working_dir_refusal_message, subscribe_working_dir_refusal_reason,
    subscribe_working_dir_replacement,
};
use super::{
    claim_live_target_agent, handle_clear_session, handle_resume_session,
    mark_remote_reload_started, rename_shutdown_signal, restored_session_was_interrupted,
    session_was_interrupted_by_reload, session_working_dir_for_client,
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

#[path = "client_session_tests/concurrency.rs"]
mod concurrency;

struct MockProvider;

struct IdlePrewarmProvider(Arc<tokio::sync::Notify>, bool);

#[async_trait]
impl Provider for IdlePrewarmProvider {
    async fn prewarm(&self, _tools: &[ToolDefinition], _system: &str) {
        self.0.notify_one();
        if self.1 {
            std::future::pending::<()>().await;
        }
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        panic!("idle prewarm must not generate a response");
    }

    fn name(&self) -> &str {
        "idle-prewarm-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self(Arc::clone(&self.0), self.1))
    }
}

#[tokio::test]
async fn idle_prewarm_starts_before_user_input_and_skips_busy_sessions() {
    let notification = Arc::new(tokio::sync::Notify::new());
    let provider: Arc<dyn Provider> =
        Arc::new(IdlePrewarmProvider(Arc::clone(&notification), false));
    let registry = Registry::new(Arc::clone(&provider)).await;
    let _env = crate::storage::lock_test_env();
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let busy = agent.lock().await;
    assert!(
        !prewarm_idle_agent(&agent),
        "reconnect must not wait for an active turn"
    );
    drop(busy);
    assert!(prewarm_idle_agent(&agent));
    tokio::time::timeout(std::time::Duration::from_secs(5), notification.notified())
        .await
        .expect("idle subscription should prewarm before any user message");
}

#[tokio::test]
async fn idle_prewarm_never_holds_agent_lock_across_pending_preparation() {
    let notification = Arc::new(tokio::sync::Notify::new());
    let provider: Arc<dyn Provider> =
        Arc::new(IdlePrewarmProvider(Arc::clone(&notification), true));
    let registry = Registry::new(Arc::clone(&provider)).await;
    let _env = crate::storage::lock_test_env();
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    assert!(!prewarm_idle_agent(&agent));
    assert!(
        agent.try_lock().is_ok(),
        "foreground must not wait for warmup"
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), notification.notified())
        .await
        .expect("pending provider hook was polled once and cancelled");
}

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

/// The log text must state the cause, not merely classify it.
///
/// The defect being fixed here was a message, not a behavior: the code already refused
/// correctly but described every refusal as a home-directory report, so a cross-project
/// attach was logged with the wrong cause. A test that only checks which enum variant
/// is returned would pass while the message stayed wrong, so assert the wording itself.
#[test]
fn subscribe_working_dir_refusal_log_states_the_actual_cause() {
    let cross = subscribe_working_dir_refusal_message(
        "session_target",
        "/work/project",
        "/work/other",
        SubscribeWorkingDirRefusal::CrossProject,
    );
    assert!(
        !cross.contains("home directory"),
        "a cross-project refusal must not be described as a home-directory report: {cross}"
    );
    assert!(
        cross.contains("creation-only"),
        "a cross-project refusal must say why it was refused: {cross}"
    );
    assert!(
        cross.contains("/work/project") && cross.contains("/work/other"),
        "the message must name both the session's project and the reported one: {cross}"
    );

    let home = subscribe_working_dir_refusal_message(
        "session_target",
        "/work/project",
        "/home/tester",
        SubscribeWorkingDirRefusal::HomeDirectory,
    );
    assert!(
        home.contains("home directory"),
        "a home-directory refusal must say so: {home}"
    );
    assert!(
        !home.contains("creation-only"),
        "a home-directory refusal is not the creation-only rule: {home}"
    );
}

/// The refusal log must name the reason it refused, and only when there is one.
///
/// One log helper was reachable from four call sites with two different causes: a
/// client that inherited `$HOME` (issue #481) and a client in a genuinely different
/// project. Both were logged with the home-directory wording, so a cross-project
/// attach was reported as a home-directory problem and the real cause was invisible.
/// The classification is pinned here rather than only asserted through the apply path,
/// because a wrong reason produces a plausible log line and nothing else fails.
#[test]
fn subscribe_working_dir_refusal_reason_names_the_actual_cause() {
    let home = std::path::PathBuf::from(if cfg!(windows) {
        r"C:\Users\tester"
    } else {
        "/home/tester"
    });
    let project = if cfg!(windows) {
        r"C:\work\project"
    } else {
        "/work/project"
    };
    let other = if cfg!(windows) {
        r"C:\work\other"
    } else {
        "/work/other"
    };

    // A client in another project: not the home case, so it must not be logged as one.
    assert_eq!(
        subscribe_working_dir_refusal_reason(project, other, Some(&home)),
        Some(SubscribeWorkingDirRefusal::CrossProject),
        "a different project must be classified as cross-project, not as a home-directory report"
    );

    // A client that inherited $HOME while the session is in a project: the #481 case.
    assert_eq!(
        subscribe_working_dir_refusal_reason(project, &home.to_string_lossy(), Some(&home)),
        Some(SubscribeWorkingDirRefusal::HomeDirectory),
        "a home report for a session already in a project is the issue #481 case"
    );

    // The session is already in home, so a home report changes nothing. This is not a
    // refusal and must not be logged as one: working in home on purpose still works.
    assert_eq!(
        subscribe_working_dir_refusal_reason(
            &home.to_string_lossy(),
            &home.to_string_lossy(),
            Some(&home)
        ),
        None,
        "a session already in home must not treat a home report as a refusal"
    );

    // A cosmetic spelling difference is not a different project, so no log line.
    assert_eq!(
        subscribe_working_dir_refusal_reason(project, &format!("{project}/."), Some(&home)),
        None,
        "a trailing /. is the same project and must not be logged as a refusal"
    );
    assert_eq!(
        subscribe_working_dir_refusal_reason(project, project, Some(&home)),
        None,
        "an identical report is not a refusal"
    );

    // No home known (a daemon that cannot resolve one) must still classify a
    // cross-project report rather than defaulting to the home wording.
    assert_eq!(
        subscribe_working_dir_refusal_reason(project, other, None),
        Some(SubscribeWorkingDirRefusal::CrossProject),
        "an unknown home must not turn a cross-project report into a home-directory report"
    );
}

/// A target session belongs to a project, and attaching to it from another
/// project must not move it. The subscriber's directory describes where the
/// subscriber is sitting, not where the target session belongs, so letting it win
/// re-points the target's tools, project-local MCP config, memory scope, and
/// swarm grouping at the wrong tree.
#[test]
fn resume_preserves_target_working_dir_on_cross_project_attach() {
    let target = "/home/tester/work/project-b";
    let subscriber = "/home/tester/work/project-a";

    assert_eq!(
        session_working_dir_for_client(Some(target), Some(subscriber), false),
        Some(target.to_string()),
        "a cross-project attach must not re-pin the target session"
    );

    // Same project from both sides: unchanged either way, including when the two
    // sides spell it differently (trailing separator, `..`, symlink).
    assert_eq!(
        session_working_dir_for_client(Some(target), Some(target), false),
        Some(target.to_string())
    );
    assert_eq!(
        session_working_dir_for_client(
            Some("/home/tester/work/project-b"),
            Some("/home/tester/work/./project-b"),
            false
        ),
        Some("/home/tester/work/project-b".to_string())
    );
    assert_eq!(
        session_working_dir_for_client(
            Some("/home/tester/work/project-b"),
            Some("/home/tester/work/project-b/"),
            false
        ),
        Some("/home/tester/work/project-b".to_string())
    );

    // At creation there is no stored directory to lose, so the client's is the only
    // description of the project available and must be adopted.
    assert_eq!(
        session_working_dir_for_client(None, Some(subscriber), true),
        Some(subscriber.to_string()),
        "a session being created must adopt the client's directory"
    );

    // Once the session exists, a later report from another project is refused even
    // when the session has no stored directory: the reported path is then a
    // statement about the reconnecting client, not about this session. Staying
    // unattributed is correct, because falling back to a process-global default
    // would reintroduce the daemon-cwd bug.
    assert_eq!(
        session_working_dir_for_client(None, Some(subscriber), false),
        None,
        "a client-reported directory must not be adopted after creation"
    );

    // A target directory with no subscriber report is kept as-is.
    assert_eq!(
        session_working_dir_for_client(Some(target), None, false),
        Some(target.to_string())
    );

    // Neither side reports a directory: stay unattributed rather than inventing
    // one. Falling back to the daemon's cwd here would be the isolation bug.
    assert_eq!(session_working_dir_for_client(None, None, false), None);
    assert_eq!(session_working_dir_for_client(None, None, true), None);

    // Blank reports are not directories and must not clobber or override.
    assert_eq!(
        session_working_dir_for_client(Some(target), Some("   "), false),
        Some(target.to_string())
    );
    assert_eq!(
        session_working_dir_for_client(Some("  "), Some(subscriber), true),
        Some(subscriber.to_string())
    );
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
async fn apply_subscribe_working_dir_keeps_project_when_client_reports_another_dir() {
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

    // A client whose inherited cwd is home must not re-pin the session (issue #481).
    apply_or_defer_subscribe_working_dir(&agent, &home_str, "session_test_481");
    assert_eq!(
        agent.lock().await.working_dir(),
        Some(project_str.as_str()),
        "a home-dir subscribe must not clobber the project cwd"
    );

    // A different project reported by a later client is refused, not just a home
    // directory. This used to assert that a project-to-project move was honored,
    // which is the bug this rule removes: the request is identical to the one an
    // attach sends, so honoring it let any reconnecting client retarget an
    // existing session. Moving a session to another project on purpose needs an
    // explicit request that does not exist yet.
    let other = home.join("jcode-481-other");
    let other_str = other.to_string_lossy().to_string();
    apply_or_defer_subscribe_working_dir(&agent, &other_str, "session_test_481");
    assert_eq!(
        agent.lock().await.working_dir(),
        Some(project_str.as_str()),
        "a client-reported directory must not move an existing session to another project"
    );

    // A session with no directory yet still adopts one. That is the creation case,
    // where the reported path is the only description of the project available.
    let fresh_provider: Arc<dyn Provider> = Arc::new(MockProvider);
    // `Agent::new_with_initial_working_dir(.., None)` is not dir-less in practice:
    // `Session::ensure_initial_session_context_message` stamps the daemon process
    // cwd when the directory is None (jcode-base/src/session.rs). Seed a session
    // that already has its context message so that stamping never runs, leaving
    // the directory genuinely unset for the creation case.
    let mut fresh_session = crate::session::Session::create(None, None);
    fresh_session.ensure_initial_session_context_message();
    assert!(
        fresh_session.working_dir.is_some(),
        "precondition: seeding the context stamps the daemon process cwd"
    );
    fresh_session.working_dir = None;
    let fresh = Agent::new_with_session(
        Arc::clone(&fresh_provider),
        Registry::new(Arc::clone(&fresh_provider)).await,
        fresh_session,
        None,
    );
    let fresh = Arc::new(Mutex::new(fresh));
    assert_eq!(
        fresh.lock().await.working_dir(),
        None,
        "precondition: the fresh session has no directory yet"
    );

    apply_or_defer_subscribe_working_dir(&fresh, &other_str, "session_test_481_fresh");
    assert_eq!(
        fresh.lock().await.working_dir(),
        Some(other_str.as_str()),
        "a session being created must adopt the reported directory"
    );
}

#[path = "client_session_tests/clear.rs"]
mod clear_tests;
#[path = "client_session_tests/reload.rs"]
mod reload_tests;
#[path = "client_session_tests/resume.rs"]
mod resume_tests;
