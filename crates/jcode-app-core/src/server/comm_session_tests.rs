use super::{
    ensure_spawn_coordinator_swarm, prepare_visible_spawn_session, register_visible_spawned_member,
    require_coordinator_swarm, resolve_spawn_working_dir, resolve_stop_target_session,
    resolve_swarm_spawn_model_and_provider, session_has_live_attachment,
    swarm_stop_allowed_by_owner, wait_for_live_attachment, wait_for_live_attachment_with,
};
use crate::agent::Agent;
use crate::message::{Message, ToolDefinition};
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::{EventStream, Provider};
use crate::server::{SwarmEventType, SwarmMember, VersionedPlan};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

struct MockProvider;

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Err(anyhow::anyhow!("mock provider should not be called"))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(MockProvider)
    }
}

fn member(
    session_id: &str,
    swarm_id: Option<&str>,
    role: &str,
) -> (SwarmMember, mpsc::UnboundedReceiver<ServerEvent>) {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    (
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: swarm_id.map(|id| id.to_string()),
            swarm_enabled: true,
            status: "ready".to_string(),
            detail: None,
            friendly_name: Some(session_id.to_string()),
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: role.to_string(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: false,
        },
        event_rx,
    )
}

async fn test_agent_with_working_dir(session_id: &str, working_dir: &str) -> Arc<Mutex<Agent>> {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut session = crate::session::Session::create_with_id(session_id.to_string(), None, None);
    session.model = Some("mock".to_string());
    session.working_dir = Some(working_dir.to_string());
    let mut agent = Agent::new_with_session(provider, registry, session, None);
    agent.set_working_dir(working_dir);
    Arc::new(Mutex::new(agent))
}

#[tokio::test]
async fn resolve_spawn_working_dir_prefers_explicit_then_spawner_agent_dir() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    sessions.write().await.insert(
        "req".to_string(),
        test_agent_with_working_dir("req", "/tmp/spawner-agent").await,
    );
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));

    assert_eq!(
        resolve_spawn_working_dir(
            Some("/tmp/explicit".to_string()),
            "req",
            &sessions,
            &swarm_members,
        )
        .await
        .as_deref(),
        Some("/tmp/explicit")
    );
    assert_eq!(
        resolve_spawn_working_dir(None, "req", &sessions, &swarm_members)
            .await
            .as_deref(),
        Some("/tmp/spawner-agent")
    );
}

#[tokio::test]
async fn resolve_spawn_working_dir_falls_back_to_member_dir() {
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let (mut req_member, _rx) = member("req", Some("swarm-1"), "coordinator");
    req_member.working_dir = Some(std::path::PathBuf::from("/tmp/member-dir"));
    swarm_members
        .write()
        .await
        .insert("req".to_string(), req_member);

    assert_eq!(
        resolve_spawn_working_dir(None, "req", &sessions, &swarm_members)
            .await
            .as_deref(),
        Some("/tmp/member-dir")
    );
}

#[test]
fn stop_permission_defaults_to_sessions_spawned_by_requesting_coordinator() {
    let (mut owned, _owned_rx) = member("worker-owned", Some("swarm-1"), "agent");
    owned.report_back_to_session_id = Some("coord".to_string());
    let (mut user_created, _user_rx) = member("worker-user", Some("swarm-1"), "agent");
    user_created.report_back_to_session_id = None;
    let (mut other_owned, _other_rx) = member("worker-other", Some("swarm-1"), "agent");
    other_owned.report_back_to_session_id = Some("other-coord".to_string());

    assert!(swarm_stop_allowed_by_owner("coord", &owned, false));
    assert!(!swarm_stop_allowed_by_owner("coord", &user_created, false));
    assert!(!swarm_stop_allowed_by_owner("coord", &other_owned, false));
    assert!(swarm_stop_allowed_by_owner("coord", &user_created, true));
}

#[tokio::test]
async fn stop_target_resolves_unique_friendly_name_and_suffix() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let (mut worker, _worker_rx) = member("session_jellyfish_1234_abcd", Some("swarm-1"), "agent");
    worker.friendly_name = Some("jellyfish".to_string());
    swarm_members
        .write()
        .await
        .insert(worker.session_id.clone(), worker);

    assert_eq!(
        resolve_stop_target_session("swarm-1", "jellyfish", &swarm_members)
            .await
            .as_deref(),
        Ok("session_jellyfish_1234_abcd")
    );
    assert_eq!(
        resolve_stop_target_session("swarm-1", "abcd", &swarm_members)
            .await
            .as_deref(),
        Ok("session_jellyfish_1234_abcd")
    );
}

#[tokio::test]
async fn stop_target_rejects_ambiguous_friendly_name() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let (mut first, _first_rx) = member("session_bear_1", Some("swarm-1"), "agent");
    first.friendly_name = Some("bear".to_string());
    let (mut second, _second_rx) = member("session_bear_2", Some("swarm-1"), "agent");
    second.friendly_name = Some("bear".to_string());
    let mut members = swarm_members.write().await;
    members.insert(first.session_id.clone(), first);
    members.insert(second.session_id.clone(), second);
    drop(members);

    let err = resolve_stop_target_session("swarm-1", "bear", &swarm_members)
        .await
        .expect_err("ambiguous friendly names should be rejected");
    assert!(err.contains("Ambiguous swarm session 'bear'"));
}

#[tokio::test]
async fn register_visible_spawned_member_marks_startup_as_running() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(8);

    register_visible_spawned_member(
        "child-1",
        "swarm-1",
        Some("/tmp/worktree"),
        true,
        Some("owner"),
        &swarm_members,
        &swarms_by_id,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    )
    .await;

    let members = swarm_members.read().await;
    let member = members.get("child-1").expect("spawned member should exist");
    assert_eq!(member.status, "running");
    assert_eq!(member.detail.as_deref(), Some("startup queued"));
    assert_eq!(member.swarm_id.as_deref(), Some("swarm-1"));
    assert_eq!(
        member.working_dir.as_deref(),
        Some(std::path::Path::new("/tmp/worktree"))
    );
    drop(members);

    assert!(
        swarms_by_id
            .read()
            .await
            .get("swarm-1")
            .is_some_and(|members| members.contains("child-1"))
    );

    let history = event_history.read().await;
    assert!(history.iter().any(|event| {
            event.session_id == "child-1"
                && matches!(event.event, SwarmEventType::MemberChange { ref action } if action == "joined")
        }));
}

#[tokio::test]
async fn wait_for_live_attachment_times_out_when_no_client_attaches() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    // Member exists but has no live event channels (the visible-launch ghost).
    {
        let (ghost, _rx) = member("child-ghost", Some("swarm-1"), "agent");
        swarm_members
            .write()
            .await
            .insert("child-ghost".to_string(), ghost);
    }
    // Use short timeout/poll so the test is fast; a session that never attaches
    // must return false.
    let attached = wait_for_live_attachment_with(
        "child-ghost",
        &swarm_members,
        std::time::Duration::from_millis(120),
        std::time::Duration::from_millis(20),
    )
    .await;
    assert!(
        !attached,
        "no client ever attached, so wait_for_live_attachment must return false"
    );
}

#[tokio::test]
async fn wait_for_live_attachment_detects_attached_client() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let (client_tx, _client_rx_keep_alive) = mpsc::unbounded_channel();
    {
        let (mut attached, _rx) = member("child-live", Some("swarm-1"), "agent");
        attached
            .event_txs
            .insert("conn-1".to_string(), client_tx);
        swarm_members
            .write()
            .await
            .insert("child-live".to_string(), attached);
    }
    let attached = wait_for_live_attachment("child-live", &swarm_members).await;
    assert!(
        attached,
        "a member with a live event channel must be detected as attached"
    );
}

#[tokio::test]
async fn session_has_live_attachment_false_for_headless_coordinator() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    {
        // Coordinator member present but with no live event channels: this is a
        // coordinator running headless inside a `jcode serve` shared server.
        let (coordinator, _rx) = member("coordinator-headless", Some("swarm-1"), "coordinator");
        swarm_members
            .write()
            .await
            .insert("coordinator-headless".to_string(), coordinator);
    }
    assert!(
        !session_has_live_attachment("coordinator-headless", &swarm_members).await,
        "a coordinator with no live event channels must report no attachment so Auto skips the visible spawn"
    );
}

#[tokio::test]
async fn session_has_live_attachment_false_for_unknown_session() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    assert!(
        !session_has_live_attachment("does-not-exist", &swarm_members).await,
        "an unknown session must report no attachment"
    );
}

#[tokio::test]
async fn session_has_live_attachment_true_for_attached_coordinator() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let (client_tx, _client_rx_keep_alive) = mpsc::unbounded_channel();
    {
        let (mut coordinator, _rx) = member("coordinator-live", Some("swarm-1"), "coordinator");
        coordinator
            .event_txs
            .insert("conn-1".to_string(), client_tx);
        swarm_members
            .write()
            .await
            .insert("coordinator-live".to_string(), coordinator);
    }
    assert!(
        session_has_live_attachment("coordinator-live", &swarm_members).await,
        "a coordinator with a live event channel must report an attachment so Auto tries the visible spawn"
    );
}

#[tokio::test]
async fn register_visible_spawned_member_does_not_clobber_attached_client() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let event_history = Arc::new(RwLock::new(VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(8);

    // Simulate a real interactive client having already attached and registered
    // this member with a live event channel before registration runs. Keep the
    // receiver alive so the sender is not considered closed (and pruned).
    let (client_tx, _client_rx_keep_alive) = mpsc::unbounded_channel();
    {
        let (mut attached, _rx) = member("child-1", Some("swarm-1"), "agent");
        attached
            .event_txs
            .insert("conn-1".to_string(), client_tx);
        attached.status = "ready".to_string();
        swarm_members
            .write()
            .await
            .insert("child-1".to_string(), attached);
    }

    register_visible_spawned_member(
        "child-1",
        "swarm-1",
        Some("/tmp/worktree"),
        true,
        Some("owner"),
        &swarm_members,
        &swarms_by_id,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    )
    .await;

    let members = swarm_members.read().await;
    let member = members.get("child-1").expect("member should still exist");
    assert!(
        !member.event_txs.is_empty(),
        "registration must not clobber the live client's event channels"
    );
    assert_eq!(
        member.status, "ready",
        "registration must not reset the attached client's status to startup queued"
    );
}

#[test]
fn prepare_visible_spawn_session_persists_startup_before_launch() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let worktree = tempfile::TempDir::new().expect("temp worktree");
    let startup = "Please start by auditing prompt delivery.";

    let (session_id, launched) = prepare_visible_spawn_session(
        Some(worktree.path().to_str().expect("utf8 worktree path")),
        None,
        None,
        false,
        Some(startup),
        |session_id, _cwd: &std::path::Path, _selfdev, provider_key| {
            assert_eq!(provider_key, None);
            let path = crate::storage::jcode_dir()
                .expect("jcode dir")
                .join(format!("client-input-{}", session_id));
            let data = std::fs::read_to_string(&path).expect("startup file should exist");
            assert!(
                data.contains(startup),
                "startup payload should be written before launch"
            );
            assert!(
                data.contains(r#""submit_on_restore":true"#),
                "startup payload should auto-submit on restore"
            );
            Ok(true)
        },
    )
    .expect("visible spawn preparation should succeed");

    assert!(launched);
    let path = crate::storage::jcode_dir()
        .expect("jcode dir")
        .join(format!("client-input-{}", session_id));
    assert!(
        path.exists(),
        "startup file should remain for launched visible session"
    );

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn prepare_visible_spawn_session_cleans_startup_when_launch_not_started() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let worktree = tempfile::TempDir::new().expect("temp worktree");

    let (session_id, launched) = prepare_visible_spawn_session(
        Some(worktree.path().to_str().expect("utf8 worktree path")),
        None,
        None,
        false,
        Some("Do the thing."),
        |_session_id, _cwd: &std::path::Path, _selfdev, _provider_key| Ok(false),
    )
    .expect("visible spawn preparation should succeed even when launch is skipped");

    assert!(!launched);
    let path = crate::storage::jcode_dir()
        .expect("jcode dir")
        .join(format!("client-input-{}", session_id));
    assert!(
        !path.exists(),
        "startup file should be removed when visible launch does not start"
    );
    assert!(
        !crate::session::session_exists(&session_id),
        "prepared session should be cleaned up when visible launch does not start"
    );

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn prepare_visible_spawn_session_cleans_session_when_launch_errors() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let worktree = tempfile::TempDir::new().expect("temp worktree");

    let error = prepare_visible_spawn_session(
        Some(worktree.path().to_str().expect("utf8 worktree path")),
        None,
        None,
        false,
        Some("Do the thing."),
        |_session_id, _cwd: &std::path::Path, _selfdev, _provider_key| {
            Err(anyhow::anyhow!("launch failed"))
        },
    )
    .expect_err("visible spawn preparation should surface launch error");

    assert!(error.to_string().contains("launch failed"));
    let sessions_dir = crate::storage::jcode_dir()
        .expect("jcode dir")
        .join("sessions");
    let remaining_sessions = std::fs::read_dir(&sessions_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        remaining_sessions, 0,
        "failed visible launch should not leave orphan prepared sessions"
    );

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn prepare_visible_spawn_session_persists_and_launches_provider_key_for_openrouter_model() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let worktree = tempfile::TempDir::new().expect("temp worktree");
    let (session_id, launched) = prepare_visible_spawn_session(
        Some(worktree.path().to_str().expect("utf8 worktree path")),
        Some("openai/gpt-5.4@OpenAI"),
        None,
        false,
        None,
        |_session_id, _cwd: &std::path::Path, _selfdev, provider_key| {
            assert_eq!(provider_key, Some("openrouter"));
            Ok(true)
        },
    )
    .expect("visible spawn preparation should succeed");

    assert!(launched);
    let session = crate::session::Session::load(&session_id).expect("prepared session should save");
    assert_eq!(session.model.as_deref(), Some("openai/gpt-5.4@OpenAI"));
    assert_eq!(session.provider_key.as_deref(), Some("openrouter"));

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn prepare_visible_spawn_session_prefers_parent_provider_key_over_model_guess() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let worktree = tempfile::TempDir::new().expect("temp worktree");
    let (session_id, launched) = prepare_visible_spawn_session(
        Some(worktree.path().to_str().expect("utf8 worktree path")),
        Some("gpt-5.4"),
        Some("ollama"),
        false,
        None,
        |_session_id, _cwd: &std::path::Path, _selfdev, provider_key| {
            assert_eq!(provider_key, Some("ollama"));
            Ok(true)
        },
    )
    .expect("visible spawn preparation should succeed");

    assert!(launched);
    let session = crate::session::Session::load(&session_id).expect("prepared session should save");
    assert_eq!(session.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(session.provider_key.as_deref(), Some("ollama"));

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn resolve_swarm_spawn_model_prefers_configured_model_over_coordinator_model() {
    let (model, provider_key) = resolve_swarm_spawn_model_and_provider(
        Some("openai/gpt-5.4@OpenAI".to_string()),
        Some("nvidia/llama-3.3-nemotron-super-49b-v1".to_string()),
        Some("nvidia".to_string()),
    );

    assert_eq!(model.as_deref(), Some("openai/gpt-5.4@OpenAI"));
    assert_eq!(provider_key.as_deref(), Some("openrouter"));
}

#[test]
fn resolve_swarm_spawn_model_inherits_coordinator_when_unconfigured() {
    let (model, provider_key) = resolve_swarm_spawn_model_and_provider(
        None,
        Some("nvidia/llama-3.3-nemotron-super-49b-v1".to_string()),
        Some("nvidia".to_string()),
    );

    assert_eq!(
        model.as_deref(),
        Some("nvidia/llama-3.3-nemotron-super-49b-v1")
    );
    assert_eq!(provider_key.as_deref(), Some("nvidia"));
}

#[test]
fn resolve_swarm_spawn_model_keeps_provider_key_when_config_matches_coordinator() {
    let (model, provider_key) = resolve_swarm_spawn_model_and_provider(
        Some("custom-model".to_string()),
        Some("custom-model".to_string()),
        Some("custom-provider".to_string()),
    );

    assert_eq!(model.as_deref(), Some("custom-model"));
    assert_eq!(provider_key.as_deref(), Some("custom-provider"));
}

#[tokio::test]
async fn spawn_bootstraps_coordinator_when_swarm_has_none() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["req".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::<String, VersionedPlan>::new()));
    let (req_member, _req_rx) = member("req", Some("swarm-1"), "agent");
    swarm_members
        .write()
        .await
        .insert("req".to_string(), req_member);
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let swarm_id = ensure_spawn_coordinator_swarm(
        1,
        "req",
        "Only the coordinator can spawn new agents.",
        &client_event_tx,
        &swarm_members,
        &swarms_by_id,
        &swarm_coordinators,
        &swarm_plans,
    )
    .await;

    assert_eq!(swarm_id.as_deref(), Some("swarm-1"));
    assert_eq!(
        swarm_coordinators
            .read()
            .await
            .get("swarm-1")
            .map(String::as_str),
        Some("req")
    );
    assert_eq!(
        swarm_members
            .read()
            .await
            .get("req")
            .map(|member| member.role.as_str()),
        Some("coordinator")
    );
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Notification {
            notification_type: NotificationType::Message { .. },
            message,
            ..
        }) if message == "You are the coordinator for this swarm."
    ));
}

#[tokio::test]
async fn spawn_requires_existing_coordinator_when_one_is_set() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        HashSet::from(["req".to_string(), "coord".to_string()]),
    )])));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "coord".to_string(),
    )])));
    let swarm_plans = Arc::new(RwLock::new(HashMap::<String, VersionedPlan>::new()));
    let (req_member, _req_rx) = member("req", Some("swarm-1"), "agent");
    let (coord_member, _coord_rx) = member("coord", Some("swarm-1"), "coordinator");
    let mut members = swarm_members.write().await;
    members.insert("req".to_string(), req_member);
    members.insert("coord".to_string(), coord_member);
    drop(members);
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let swarm_id = ensure_spawn_coordinator_swarm(
        2,
        "req",
        "Only the coordinator can spawn new agents.",
        &client_event_tx,
        &swarm_members,
        &swarms_by_id,
        &swarm_coordinators,
        &swarm_plans,
    )
    .await;

    assert!(swarm_id.is_none());
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Error { message, .. })
            if message == "Only the coordinator can spawn new agents."
    ));
    assert_eq!(
        swarm_members
            .read()
            .await
            .get("req")
            .map(|member| member.role.as_str()),
        Some("agent")
    );
}

#[tokio::test]
async fn coordinator_actions_self_promote_when_recorded_coordinator_is_stale() {
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::from([(
        "swarm-1".to_string(),
        "old-coord".to_string(),
    )])));
    let (req_member, _req_rx) = member("req", Some("swarm-1"), "agent");
    let (mut old_coord, _old_rx) = member("old-coord", Some("swarm-1"), "coordinator");
    old_coord.status = "crashed".to_string();
    let mut members = swarm_members.write().await;
    members.insert("req".to_string(), req_member);
    members.insert("old-coord".to_string(), old_coord);
    drop(members);
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let swarm_id = require_coordinator_swarm(
        3,
        "req",
        "Only the coordinator can stop agents.",
        &client_event_tx,
        &swarm_members,
        &swarm_coordinators,
    )
    .await;

    assert_eq!(swarm_id.as_deref(), Some("swarm-1"));
    assert_eq!(
        swarm_coordinators
            .read()
            .await
            .get("swarm-1")
            .map(String::as_str),
        Some("req")
    );
    assert_eq!(
        swarm_members
            .read()
            .await
            .get("req")
            .map(|member| member.role.as_str()),
        Some("coordinator")
    );
    assert!(client_event_rx.try_recv().is_err());
}
