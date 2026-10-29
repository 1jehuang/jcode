//! Fresh headless spawn must use the same quota wait and cancellation lifecycle.
#![allow(clippy::await_holding_lock)]
use super::*;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use crate::provider::EventStream;
use crate::server::{Server, get_shared_mcp_pool, usage_limit_resume};
use anyhow::Result;
use async_trait::async_trait;
use std::ffi::OsString;
use std::time::Duration;

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
struct LimitedOnce(Arc<StdMutex<Vec<Vec<Message>>>>);
impl LimitedOnce {
    fn calls(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}
#[async_trait]
impl Provider for LimitedOnce {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        let call = {
            let mut requests = self.0.lock().unwrap();
            requests.push(messages.to_vec());
            requests.len()
        };
        if call == 1 {
            let error = jcode_provider_core::usage_limit_resume::with_usage_limit_reset(
                anyhow::anyhow!(
                    "Anthropic API error (429 Too Many Requests): Usage limit reached for this account"
                ),
                Some(Duration::from_secs(1)),
            );
            return Ok(Box::pin(tokio_stream::iter([Err(error)])));
        }
        Ok(Box::pin(tokio_stream::iter([
            Ok(StreamEvent::TextDelta(
                "fresh-worker-completion-marker".into(),
            )),
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

async fn spawn_worker(
    server: &Server,
    root: &std::path::Path,
) -> Result<(String, String, mpsc::UnboundedReceiver<ServerEvent>)> {
    let mut owner = Session::create(None, Some("fresh-spawn-owner".into()));
    owner.system_prompt = Some("COORDINATOR_SYSTEM_PROMPT_MUST_NOT_INHERIT".into());
    owner.agent_profile = Some(crate::agent_profile::AgentProfile {
        name: "coordinator-profile".into(),
        instructions: "COORDINATOR_PROFILE_MUST_NOT_INHERIT".into(),
        instruction_sources: Vec::new(),
    });
    owner.save_prepared()?;
    let owner_id = owner.id.clone();
    let swarm_id = "fresh-spawn-recovery".to_string();
    let (tx, mut rx) = mpsc::unbounded_channel();
    server.swarm_state.members.write().await.insert(
        owner_id.clone(),
        SwarmMember {
            session_id: owner_id.clone(),
            event_tx: tx.clone(),
            event_txs: HashMap::new(),
            working_dir: Some(root.to_path_buf()),
            swarm_id: Some(swarm_id.clone()),
            swarm_enabled: true,
            status: "running".into(),
            detail: None,
            friendly_name: Some("fresh-spawn-owner".into()),
            report_back_to_session_id: None,
            latest_completion_report: None,
            role: "coordinator".into(),
            joined_at: Instant::now(),
            last_status_change: Instant::now(),
            is_headless: false,
            output_tail: None,
            todo_progress: None,
            todo_items: Vec::new(),
            runtime: Default::default(),
            task_label: None,
        },
    );
    server
        .swarm_state
        .swarms_by_id
        .write()
        .await
        .insert(swarm_id.clone(), HashSet::from([owner_id.clone()]));
    server
        .swarm_state
        .coordinators
        .write()
        .await
        .insert(swarm_id, owner_id.clone());
    let pool = get_shared_mcp_pool(&server.mcp_pool).await;
    handle_comm_spawn(
        71,
        owner_id.clone(),
        Some(root.to_string_lossy().into_owned()),
        Some("fresh-worker-original-prompt-marker".into()),
        Some("fresh-quota-spawn".into()),
        Some(SwarmSpawnMode::Headless),
        None,
        None,
        Some("fresh quota worker".into()),
        &tx,
        &server.sessions,
        &server.session_id,
        &server.provider,
        &server.swarm_state.members,
        &server.swarm_state.swarms_by_id,
        &server.swarm_state.coordinators,
        &server.swarm_state.plans,
        &server.channel_subscriptions,
        &server.channel_subscriptions_by_session,
        &server.event_history,
        &server.event_counter,
        &server.swarm_event_tx,
        &pool,
        &server.soft_interrupt_queues,
        &server.swarm_mutation_runtime,
        &server.client_connections,
    )
    .await;
    let id = loop {
        match rx.try_recv()? {
            ServerEvent::CommSpawnResponse {
                id: 71,
                new_session_id,
                ..
            } => break new_session_id,
            ServerEvent::Error {
                id: 71, message, ..
            } => anyhow::bail!("spawn failed: {message}"),
            _ => {}
        }
    };
    Ok((owner_id, id, rx))
}

async fn wait_status(server: &Server, id: &str, expected: &str) {
    for _ in 0..2000 {
        let status = server.swarm_state.members.read().await[id].status.clone();
        if status == expected {
            return;
        }
        assert_ne!(
            status, "failed",
            "fresh worker must not fail while waiting for quota"
        );
        tokio::task::yield_now().await;
    }
    panic!("worker did not reach {expected}");
}

#[tokio::test(start_paused = true)]
async fn fresh_headless_spawn_quota_resumes_same_turn_once() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let provider = Arc::new(LimitedOnce::default());
    let server = Server::new(provider.clone());
    let (owner, id, mut owner_events) = spawn_worker(&server, root.path()).await?;
    let worker = Session::load(&id)?;
    assert_eq!(worker.system_prompt, None);
    assert_eq!(worker.agent_profile, None);
    wait_status(&server, &id, "rate_limited").await;
    assert_eq!(provider.calls(), 1);
    assert!(usage_limit_resume::pending_state(&id)?.is_some());
    tokio::time::advance(Duration::from_secs(40)).await;
    wait_status(&server, &id, "ready").await;
    // The agent reservation is released only after notifications and durable completion.
    let agent = server.sessions.read().await[&id].clone();
    let held = agent.lock().await;
    assert_eq!(provider.calls(), 2, "exactly one quota retry");
    assert!(usage_limit_resume::pending_state(&id)?.is_none());
    let member = server.swarm_state.members.read().await[&id].clone();
    assert_eq!(
        member.report_back_to_session_id.as_deref(),
        Some(owner.as_str())
    );
    assert_eq!(member.task_label.as_deref(), Some("fresh quota worker"));
    assert!(
        member
            .latest_completion_report
            .as_deref()
            .is_some_and(|text| text.contains("fresh-worker-completion-marker"))
    );
    let events: Vec<_> = std::iter::from_fn(|| owner_events.try_recv().ok()).collect();
    assert!(
        events.iter().any(
            |event| matches!(event, ServerEvent::Notification { message, .. }
        if message.contains("fresh-worker-completion-marker"))
        ),
        "owner receives completion report"
    );
    assert_eq!(held.session_id(), id);
    assert_eq!(
        held.messages()
            .iter()
            .filter(|message| message.role == Role::User
                && message
                    .content_preview()
                    .contains("fresh-worker-original-prompt-marker"))
            .count(),
        1
    );
    drop(held);
    for request in provider.0.lock().unwrap().iter() {
        assert_eq!(
            request
                .iter()
                .filter(|message| message.role == Role::User
                    && message
                        .content
                        .iter()
                        .any(|block| matches!(block, ContentBlock::Text { text, .. }
                if text.contains("fresh-worker-original-prompt-marker"))))
                .count(),
            1
        );
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn fresh_headless_spawn_stop_during_quota_wait_never_resumes() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let provider = Arc::new(LimitedOnce::default());
    let server = Server::new(provider.clone());
    let (owner, id, _owner_events) = spawn_worker(&server, root.path()).await?;
    wait_status(&server, &id, "rate_limited").await;
    let stopped = crate::server::swarm::cancel_spawned_descendant_turns(
        &owner,
        &server.swarm_state.members,
        &server.swarm_state.swarms_by_id,
        Some(&server.event_history),
        Some(&server.event_counter),
        Some(&server.swarm_event_tx),
    )
    .await;
    assert_eq!(stopped, vec![id.clone()]);
    assert!(usage_limit_resume::pending_state(&id)?.is_none());
    tokio::time::advance(Duration::from_secs(120)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        provider.calls(),
        1,
        "Stop cancels the sleeping initial turn"
    );
    assert_eq!(
        server.swarm_state.members.read().await[&id].status,
        "stopped"
    );
    assert!(!usage_limit_resume::has_pending_resume(&id));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn public_stop_without_plan_persists_removal_and_preserves_other_workers() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let root = tempfile::TempDir::new()?;
    let _env = IsolatedEnv::new(root.path());
    let provider = Arc::new(LimitedOnce::default());
    let server = Server::new(provider.clone());
    let (owner, target, mut events) = spawn_worker(&server, root.path()).await?;
    wait_status(&server, &target, "rate_limited").await;
    assert!(usage_limit_resume::pending_state(&target)?.is_some());
    let tx = server.swarm_state.members.read().await[&owner]
        .event_tx
        .clone();
    let pool = get_shared_mcp_pool(&server.mcp_pool).await;
    handle_comm_spawn(
        72,
        owner.clone(),
        Some(root.path().to_string_lossy().into_owned()),
        None,
        Some("preserved-idle-sibling".into()),
        Some(SwarmSpawnMode::Headless),
        None,
        None,
        Some("preserved sibling".into()),
        &tx,
        &server.sessions,
        &server.session_id,
        &server.provider,
        &server.swarm_state.members,
        &server.swarm_state.swarms_by_id,
        &server.swarm_state.coordinators,
        &server.swarm_state.plans,
        &server.channel_subscriptions,
        &server.channel_subscriptions_by_session,
        &server.event_history,
        &server.event_counter,
        &server.swarm_event_tx,
        &pool,
        &server.soft_interrupt_queues,
        &server.swarm_mutation_runtime,
        &server.client_connections,
    )
    .await;
    let sibling = loop {
        match events.try_recv()? {
            ServerEvent::CommSpawnResponse {
                id: 72,
                new_session_id,
                ..
            } => break new_session_id,
            ServerEvent::Error {
                id: 72, message, ..
            } => anyhow::bail!("sibling spawn failed: {message}"),
            _ => {}
        }
    };
    assert!(server.sessions.read().await.contains_key(&sibling));
    let before = crate::server::swarm_persistence::load_runtime_state();
    assert!(
        before.plans.is_empty(),
        "exercise the ad-hoc no-plan removal branch"
    );
    for id in [&owner, &target, &sibling] {
        assert!(
            before.members.contains_key(id),
            "initial snapshot contains {id}"
        );
    }
    handle_comm_stop(
        73,
        owner.clone(),
        target.clone(),
        false,
        &tx,
        &server.sessions,
        &server.swarm_state.members,
        &server.swarm_state.swarms_by_id,
        &server.swarm_state.coordinators,
        &server.swarm_state.plans,
        &server.channel_subscriptions,
        &server.channel_subscriptions_by_session,
        &server.event_history,
        &server.event_counter,
        &server.swarm_event_tx,
        &server.soft_interrupt_queues,
        &server.swarm_mutation_runtime,
    )
    .await;
    let terminal = loop {
        match events.try_recv()? {
            event @ ServerEvent::Done { id: 73 } | event @ ServerEvent::Error { id: 73, .. } => {
                break event;
            }
            _ => {}
        }
    };
    assert!(
        matches!(terminal, ServerEvent::Done { id: 73 }),
        "{terminal:?}"
    );
    assert!(
        !server
            .swarm_state
            .members
            .read()
            .await
            .contains_key(&target)
    );
    assert!(!server.sessions.read().await.contains_key(&target));
    assert!(usage_limit_resume::pending_state(&target)?.is_none());
    let restored = Server::new(provider.clone());
    {
        let members = restored.swarm_state.members.read().await;
        assert!(
            !members.contains_key(&target),
            "acknowledged Stop must survive a fresh disk load"
        );
        assert!(members.contains_key(&owner), "preserve the coordinator");
        assert!(
            members.contains_key(&sibling),
            "preserve the other live worker record"
        );
    }
    assert!(restored.swarm_state.plans.read().await.is_empty());
    restored.recover_headless_sessions_on_startup().await;
    tokio::time::advance(Duration::from_secs(120)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        provider.calls(),
        1,
        "neither old timer nor startup may replay stopped quota work"
    );
    assert!(!restored.sessions.read().await.contains_key(&target));
    Ok(())
}
