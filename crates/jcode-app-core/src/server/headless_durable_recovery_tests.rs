//! Durable disk input through the real Server startup recovery entrypoint.
#![allow(clippy::await_holding_lock)]
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::server::usage_limit_resume::{self, ResumeKind, ResumeState};
use crate::server::*;
use crate::session::{Session, SessionStatus};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::ffi::OsString;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const SWARM: &str = "durable-startup-regression";
struct IsolatedEnv(Vec<(&'static str, Option<OsString>)>);
impl IsolatedEnv {
    fn new(root: &std::path::Path) -> Self {
        let values = [
            ("JCODE_HOME", root.join("home").into_os_string()),
            ("JCODE_RUNTIME_DIR", root.join("runtime").into_os_string()),
            ("JCODE_MEMORY_ENABLED", OsString::from("0")),
            ("JCODE_MEMORY_SIDECAR_ENABLED", OsString::from("0")),
        ];
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        let previous = values
            .into_iter()
            .map(|(key, value)| {
                let previous = std::env::var_os(key);
                crate::env::set_var(key, value);
                (key, previous)
            })
            .collect();
        crate::config::invalidate_config_cache();
        Self(previous)
    }
}
impl Drop for IsolatedEnv {
    fn drop(&mut self) {
        for (key, previous) in self.0.drain(..).rev() {
            match previous {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
        crate::config::invalidate_config_cache();
    }
}

#[derive(Default, Clone)]
struct RecordingProvider(Arc<StdMutex<Vec<Vec<Message>>>>);
impl RecordingProvider {
    fn calls(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}
#[async_trait]
impl Provider for RecordingProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        self.0.lock().unwrap().push(messages.to_vec());
        if messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(block, ContentBlock::Text { text, .. } if text.contains("followup-quota-wake"))
            })
        }) {
            let error = jcode_provider_core::usage_limit_resume::with_usage_limit_reset(
                anyhow::anyhow!("Usage limit reached for this account"),
                Some(Duration::from_secs(7200)),
            );
            return Ok(Box::pin(tokio_stream::iter([Err(error)])));
        }
        Ok(Box::pin(tokio_stream::iter([
            Ok(StreamEvent::TextDelta("durable-startup-complete".into())),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }
    fn name(&self) -> &str {
        "test"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn seed(status: &str, waiting: bool, assigned: bool) -> Result<String> {
    let mut session = Session::create(None, Some("durable-worker".into()));
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "original-durable-assignment".into(),
            cache_control: None,
        }],
    );
    session.status = SessionStatus::Closed;
    session.save()?;
    let id = session.id.clone();
    let (event_tx, _) = mpsc::unbounded_channel();
    let member = SwarmMember {
        session_id: id.clone(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: Some(SWARM.into()),
        swarm_enabled: true,
        status: status.into(),
        detail: None,
        task_label: None,
        friendly_name: Some("durable-worker".into()),
        report_back_to_session_id: Some("durable-owner".into()),
        latest_completion_report: None,
        role: "agent".into(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: true,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: Default::default(),
    };
    let mut plan = VersionedPlan::new();
    plan.mode = "light".into();
    if assigned {
        plan.items.push(crate::plan::PlanItem {
            id: "original-node".into(),
            content: "original-durable-assignment".into(),
            status: "running".into(),
            priority: "high".into(),
            assigned_to: Some(id.clone()),
            subsystem: None,
            file_scope: vec![],
            blocked_by: vec![],
        });
    }
    persist_swarm_state_snapshot(
        SWARM,
        assigned.then_some(&plan),
        Some("durable-owner"),
        &[member],
    );
    if waiting {
        let state = ResumeState {
            session_id: id.clone(),
            generation: "pre-daemon-restart".into(),
            resume_at: chrono::Utc::now() + chrono::Duration::hours(2),
            quota_resumes_used: 1,
            overload_resumes_used: 1,
            kind: ResumeKind::UsageLimit,
            system_reminder: Some("preserved original reminder".into()),
            in_flight: false,
            terminal: false,
        };
        // Seed exactly the durable wire format, with no in-memory ticket alive.
        let name: String = id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let path = crate::storage::jcode_dir()?
            .join("provider-waits")
            .join(format!("{name}.json"));
        std::fs::create_dir_all(path.parent().unwrap())?;
        crate::storage::write_json(&path, &state)?;
        assert!(usage_limit_resume::pending_state(&id)?.is_some());
    }
    Ok(id)
}

async fn wait_status(server: &Server, id: &str, expected: &str) {
    for _ in 0..10000 {
        if server
            .swarm_state
            .members
            .read()
            .await
            .get(id)
            .is_some_and(|member| member.status == expected)
        {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!(
        "worker did not reach {expected}: {:?}",
        server
            .swarm_state
            .members
            .read()
            .await
            .get(id)
            .map(|member| member.status.clone())
    );
}
async fn settle() {
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
}
async fn resume_all(server: &Server) -> usize {
    let (tx, mut rx) = mpsc::unbounded_channel();
    crate::server::client_actions::handle_resume_all_sessions(
        8,
        &server.sessions,
        &server.swarm_state.members,
        &server.swarm_state.swarms_by_id,
        &server.event_history,
        &server.event_counter,
        &server.swarm_event_tx,
        &tx,
        Some(&server.swarm_state),
    )
    .await;
    match rx.recv().await.unwrap() {
        crate::protocol::ServerEvent::ResumeAllResult { resumed, .. } => resumed,
        event => panic!("unexpected reply: {event:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn startup_disk_wait_preserves_deadline_transcript_and_single_plan_completion() -> Result<()>
{
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::tempdir()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed("rate_limited", true, true)?;
    let deadline = usage_limit_resume::pending_state(&id)?.unwrap().resume_at;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    tokio::join!(
        server.recover_headless_sessions_on_startup(),
        server.recover_headless_sessions_on_startup()
    );
    wait_status(&server, &id, "rate_limited").await;
    assert_eq!(server.sessions.read().await.len(), 1);
    assert_eq!(provider.calls(), 0);
    assert_eq!(
        usage_limit_resume::pending_state(&id)?.unwrap().resume_at,
        deadline
    );
    assert_eq!(
        resume_all(&server).await,
        0,
        "manual sweep cannot bypass the original reset"
    );
    tokio::time::advance(Duration::from_secs(7190)).await;
    settle().await;
    assert_eq!(
        provider.calls(),
        0,
        "startup must not make an early provider request"
    );
    tokio::time::advance(Duration::from_secs(20)).await;
    wait_status(&server, &id, "ready").await;
    let agent = server.sessions.read().await[&id].clone();
    let agent = agent.lock().await;
    assert_eq!(agent.session_id(), id);
    assert_eq!(
        agent
            .messages()
            .iter()
            .filter(|message| message.role == Role::User)
            .count(),
        1,
        "durable resume must not append a synthetic user turn"
    );
    assert_eq!(
        agent
            .messages()
            .iter()
            .filter(|message| message
                .content_preview()
                .contains("original-durable-assignment"))
            .count(),
        1
    );
    drop(agent);
    assert_eq!(provider.calls(), 1);
    assert!(usage_limit_resume::pending_state(&id)?.is_none());
    let version = {
        let plans = server.swarm_state.plans.read().await;
        assert_eq!(plans[SWARM].items[0].status, "done");
        assert_eq!(
            plans[SWARM].items[0].assigned_to.as_deref(),
            Some(id.as_str())
        );
        assert!(
            plans[SWARM].task_progress["original-node"]
                .completed_at_unix_ms
                .is_some()
        );
        plans[SWARM].version
    };
    server.recover_headless_sessions_on_startup().await;
    assert_eq!(resume_all(&server).await, 0);
    assert_eq!(provider.calls(), 1);
    assert_eq!(
        server.swarm_state.plans.read().await[SWARM].version,
        version
    );
    Ok(())
}

#[tokio::test]
async fn startup_without_plan_persists_report_before_next_daemon() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::tempdir()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed("running", false, false)?;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    server.recover_headless_sessions_on_startup().await;
    wait_status(&server, &id, "ready").await;
    // The reservation outlives terminal snapshot publication.
    let agent = server.sessions.read().await[&id].clone();
    drop(agent.lock().await);
    let restarted = Server::new(provider.clone());
    restarted.recover_headless_sessions_on_startup().await;
    settle().await;
    assert_eq!(provider.calls(), 1);
    assert!(
        restarted.swarm_state.members.read().await[&id]
            .latest_completion_report
            .as_deref()
            .is_some_and(|report| report.contains("durable-startup-complete"))
    );
    assert!(!restarted.sessions.read().await.contains_key(&id));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn startup_disk_wait_respects_terminal_and_pause_precedence() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    for status in ["stopped", "completed", "done", "paused", "suspended"] {
        let root = tempfile::tempdir()?;
        let _env = IsolatedEnv::new(root.path());
        let id = seed(status, true, false)?;
        let provider = Arc::new(RecordingProvider::default());
        let server = Server::new(provider.clone());
        server.recover_headless_sessions_on_startup().await;
        assert!(!server.sessions.read().await.contains_key(&id));
        assert!(
            usage_limit_resume::pending_state(&id)?.is_none(),
            "{status} must cancel stale durable continuation"
        );
        assert_eq!(server.swarm_state.members.read().await[&id].status, status);
        tokio::time::advance(Duration::from_secs(7210)).await;
        settle().await;
        assert_eq!(provider.calls(), 0, "must never resurrect {status}");
    }
    Ok(())
}

async fn stop_during_capture(waiting: bool) -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::tempdir()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed(
        if waiting { "rate_limited" } else { "running" },
        waiting,
        true,
    )?;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    // Freeze exactly after insertion, at recovered assignment capture.
    let plans = server.swarm_state.plans.write().await;
    let recovery = server.recover_headless_sessions_on_startup();
    tokio::pin!(recovery);
    tokio::select! {
        _ = &mut recovery => panic!("recovery must wait for assignment capture"),
        _ = async {
            loop {
                if server.sessions.read().await.contains_key(&id) { break; }
                tokio::task::yield_now().await;
            }
        } => {}
    }
    assert!(remove_session_entry(&server.sessions, &id).await.is_some());
    server.swarm_state.members.write().await.remove(&id);
    drop(plans);
    recovery.await;
    tokio::time::advance(Duration::from_secs(7210)).await;
    settle().await;
    assert_eq!(
        provider.calls(),
        0,
        "successful Stop must beat pending startup dispatch"
    );
    assert!(usage_limit_resume::pending_state(&id)?.is_none());
    assert!(!server.sessions.read().await.contains_key(&id));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stop_after_startup_exposes_runtime_cancels_disk_wait() -> Result<()> {
    stop_during_capture(true).await
}

#[tokio::test(start_paused = true)]
async fn stop_after_startup_exposes_runtime_cancels_normal_continuation() -> Result<()> {
    stop_during_capture(false).await
}

#[tokio::test]
async fn manual_resume_all_completes_original_assigned_plan_once() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::tempdir()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed("running", false, true)?;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    let registry = Registry::new(provider.clone()).await;
    let session = Session::load(&id)?;
    let agent = Arc::new(tokio::sync::Mutex::new(
        crate::agent::Agent::new_with_session(provider.clone(), registry, session, None),
    ));
    server.sessions.write().await.insert(id.clone(), agent);
    assert_eq!(resume_all(&server).await, 1);
    assert_eq!(resume_all(&server).await, 0);
    wait_status(&server, &id, "ready").await;
    assert_eq!(provider.calls(), 1);
    let version = {
        let plans = server.swarm_state.plans.read().await;
        assert_eq!(plans[SWARM].items[0].status, "done");
        assert_eq!(
            plans[SWARM].items[0].assigned_to.as_deref(),
            Some(id.as_str())
        );
        plans[SWARM].version
    };
    assert_eq!(resume_all(&server).await, 0);
    assert_eq!(
        server.swarm_state.plans.read().await[SWARM].version,
        version
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancelled_startup_wait_stays_stopped_in_fresh_server_from_disk() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::tempdir()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed("rate_limited", true, true)?;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    server.recover_headless_sessions_on_startup().await;
    wait_status(&server, &id, "rate_limited").await;
    let stopped = crate::server::swarm::cancel_spawned_descendant_turns(
        "durable-owner",
        &server.swarm_state.members,
        &server.swarm_state.swarms_by_id,
        Some(&server.event_history),
        Some(&server.event_counter),
        Some(&server.swarm_event_tx),
    )
    .await;
    assert_eq!(stopped, vec![id.clone()]);
    assert!(usage_limit_resume::pending_state(&id)?.is_none());
    assert_eq!(
        server.swarm_state.members.read().await[&id].status,
        "stopped"
    );
    let restarted = Server::new(provider.clone());
    assert_eq!(
        restarted.swarm_state.members.read().await[&id].status,
        "stopped",
        "cancellation must commit the member lifecycle, not just delete the quota timer"
    );
    restarted.recover_headless_sessions_on_startup().await;
    tokio::time::advance(Duration::from_secs(7210)).await;
    settle().await;
    assert_eq!(
        provider.calls(),
        0,
        "a new daemon must never revive cancelled durable work"
    );
    assert!(!restarted.sessions.read().await.contains_key(&id));
    assert_eq!(
        restarted.swarm_state.plans.read().await[SWARM].items[0]
            .assigned_to
            .as_deref(),
        Some(id.as_str()),
        "cancellation persistence must not discard the original plan"
    );
    assert_ne!(
        restarted.swarm_state.plans.read().await[SWARM].items[0].status,
        "done"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn quota_on_later_generic_wake_survives_ready_snapshot_from_prior_recovery() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::tempdir()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed("running", false, false)?;
    let provider = Arc::new(RecordingProvider::default());
    let first_id = id.clone();
    let first_provider = provider.clone();
    // Dropping this runtime models daemon death. Keeping its waiting task alive
    // would instead test explicit ownership supersession, which clears the wait.
    let deadline = std::thread::spawn(move || -> Result<_> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()?;
        runtime.block_on(async move {
            let server = Server::new(first_provider);
            server.recover_headless_sessions_on_startup().await;
            wait_status(&server, &first_id, "ready").await;
            let agent = server.sessions.read().await[&first_id].clone();
            drop(agent.lock().await);
            let context = crate::server::live_turn::LiveTurnSwarmContext::new(
                &server.swarm_state.members,
                &server.swarm_state.swarms_by_id,
                &server.event_history,
                &server.event_counter,
                &server.swarm_event_tx,
            );
            assert!(
                crate::server::live_turn::run_live_turn_if_idle(
                    &first_id,
                    "followup-quota-wake",
                    None,
                    &server.sessions,
                    context
                )
                .await
            );
            wait_status(&server, &first_id, "rate_limited").await;
            Ok(usage_limit_resume::pending_state(&first_id)?
                .unwrap()
                .resume_at)
        })
    })
    .join()
    .expect("first daemon fixture panicked")?;
    assert_eq!(provider.calls(), 2);
    let restarted = Server::new(provider.clone());
    assert_ne!(
        restarted.swarm_state.members.read().await[&id].status,
        "stopped",
        "generic wake start must replace the prior durable ready status"
    );
    restarted.recover_headless_sessions_on_startup().await;
    wait_status(&restarted, &id, "rate_limited").await;
    settle().await;
    assert_eq!(
        provider.calls(),
        2,
        "restored later wake must keep waiting for its original deadline"
    );
    assert_eq!(
        usage_limit_resume::pending_state(&id)?.unwrap().resume_at,
        deadline
    );
    crate::server::swarm::cancel_spawned_descendant_turns(
        "durable-owner",
        &restarted.swarm_state.members,
        &restarted.swarm_state.swarms_by_id,
        Some(&restarted.event_history),
        Some(&restarted.event_counter),
        Some(&restarted.swarm_event_tx),
    )
    .await;
    assert!(usage_limit_resume::pending_state(&id)?.is_none());
    Ok(())
}

#[tokio::test]
async fn manual_resume_all_stop_during_assignment_capture_never_runs_orphan() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::tempdir()?;
    let _env = IsolatedEnv::new(root.path());
    let id = seed("running", false, true)?;
    let provider = Arc::new(RecordingProvider::default());
    let server = Server::new(provider.clone());
    let registry = Registry::new(provider.clone()).await;
    let session = Session::load(&id)?;
    let agent = Arc::new(tokio::sync::Mutex::new(
        crate::agent::Agent::new_with_session(provider.clone(), registry, session, None),
    ));
    server
        .sessions
        .write()
        .await
        .insert(id.clone(), agent.clone());
    let plans = server.swarm_state.plans.write().await;
    let request = resume_all(&server);
    tokio::pin!(request);
    tokio::select! {
        _ = &mut request => panic!("manual resume must wait for original assignment capture"),
        _ = async {
            loop {
                if agent.try_lock().is_err() { break; }
                tokio::task::yield_now().await;
            }
        } => {}
    }
    assert!(remove_session_entry(&server.sessions, &id).await.is_some());
    server.swarm_state.members.write().await.remove(&id);
    drop(plans);
    assert_eq!(request.await, 0);
    settle().await;
    assert_eq!(provider.calls(), 0);
    Ok(())
}
