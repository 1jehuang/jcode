//! Restored headless workers own an Agent, not a client attachment.
use super::*;
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use async_trait::async_trait;
use std::time::Instant;
use tokio::sync::mpsc;

#[derive(Clone)]
struct UnusedProvider;
#[async_trait]
impl Provider for UnusedProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        Ok(Box::pin(tokio_stream::iter([
            Ok(StreamEvent::TextDelta("resumed worker".into())),
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

async fn fixture(
    headless: bool,
    status: &str,
    attached: bool,
) -> (
    String,
    SessionAgents,
    Arc<RwLock<HashMap<String, SwarmMember>>>,
    Option<mpsc::UnboundedReceiver<ServerEvent>>,
) {
    let provider: Arc<dyn Provider> = Arc::new(UnusedProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider, registry)));
    let id = agent.lock().await.session_id().to_string();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let receiver = if attached {
        Some(event_rx)
    } else {
        drop(event_rx);
        None
    };
    let member = SwarmMember {
        session_id: id.clone(),
        event_tx,
        event_txs: HashMap::new(),
        working_dir: None,
        swarm_id: None,
        swarm_enabled: false,
        status: status.into(),
        detail: None,
        task_label: None,
        friendly_name: None,
        report_back_to_session_id: None,
        latest_completion_report: None,
        role: "agent".into(),
        joined_at: Instant::now(),
        last_status_change: Instant::now(),
        is_headless: headless,
        output_tail: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: Default::default(),
    };
    (
        id.clone(),
        Arc::new(RwLock::new(HashMap::from([(id.clone(), agent)]))),
        Arc::new(RwLock::new(HashMap::from([(id, member)]))),
        receiver,
    )
}

#[tokio::test]
async fn restored_headless_worker_without_client_can_be_reserved_once() {
    let (id, sessions, members, _) = fixture(true, "ready", false).await;
    let first = idle_live_agent(&id, &sessions, &members).await;
    assert!(first.is_some(), "restored Agent is a live headless runtime");
    assert!(idle_live_agent(&id, &sessions, &members).await.is_none());
    drop(first);
    assert!(idle_live_agent(&id, &sessions, &members).await.is_some());
}

#[tokio::test]
async fn detached_client_and_missing_headless_agent_are_not_live() {
    let (id, sessions, members, _) = fixture(false, "ready", false).await;
    assert!(idle_live_agent(&id, &sessions, &members).await.is_none());
    let (stale_tx, stale_rx) = mpsc::unbounded_channel();
    drop(stale_rx);
    members
        .write()
        .await
        .get_mut(&id)
        .unwrap()
        .event_txs
        .insert("detached-client".into(), stale_tx);
    assert!(
        idle_live_agent(&id, &sessions, &members).await.is_none(),
        "closed cached attachments are not live"
    );
    members.write().await.get_mut(&id).unwrap().is_headless = true;
    sessions.write().await.clear();
    assert!(idle_live_agent(&id, &sessions, &members).await.is_none());
}

#[tokio::test]
async fn automatic_wakes_preserve_terminal_and_suspended_workers() {
    for status in ["stopped", "completed", "done", "suspended", "paused"] {
        let (id, sessions, members, _receiver) = fixture(true, status, true).await;
        assert!(
            idle_live_agent(&id, &sessions, &members).await.is_none(),
            "must not wake {status}"
        );
    }
}

#[tokio::test]
async fn usage_limit_wait_owns_continuation_even_when_agent_is_unlocked() {
    let (id, sessions, members, _receiver) = fixture(true, "ready", true).await;
    let ticket = crate::server::usage_limit_resume::register_pending_resume(&id);
    assert!(idle_live_agent(&id, &sessions, &members).await.is_none());
    assert!(crate::server::usage_limit_resume::has_pending_resume(&id));
    drop(ticket);
    assert!(idle_live_agent(&id, &sessions, &members).await.is_some());
}

async fn resume_all_count(
    sessions: &SessionAgents,
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> usize {
    let (swarm_tx, _) = broadcast::channel(16);
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    crate::server::client_actions::handle_resume_all_sessions(
        7,
        sessions,
        members,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(AtomicU64::new(0)),
        &swarm_tx,
        &client_tx,
        None,
    )
    .await;
    match client_rx.recv().await.unwrap() {
        ServerEvent::ResumeAllResult { resumed, .. } => resumed,
        other => panic!("unexpected reply: {other:?}"),
    }
}

async fn add_unanswered_user(id: &str, sessions: &SessionAgents) {
    sessions
        .read()
        .await
        .get(id)
        .unwrap()
        .lock()
        .await
        .append_user_context_message("unfinished work", vec![])
        .unwrap();
}

#[tokio::test]
async fn resume_all_does_not_revive_stopped_completed_or_suspended_work() {
    for status in ["stopped", "completed", "done", "suspended", "paused"] {
        let (id, sessions, members, _receiver) = fixture(true, status, true).await;
        add_unanswered_user(&id, &sessions).await;
        assert_eq!(
            resume_all_count(&sessions, &members).await,
            0,
            "must preserve {status}"
        );
    }
}

#[tokio::test]
async fn resume_all_does_not_bypass_pending_usage_limit_reset() {
    let (id, sessions, members, _receiver) = fixture(true, "ready", true).await;
    add_unanswered_user(&id, &sessions).await;
    let _ticket = crate::server::usage_limit_resume::register_pending_resume(&id);
    assert_eq!(resume_all_count(&sessions, &members).await, 0);
    assert!(crate::server::usage_limit_resume::has_pending_resume(&id));
}

#[tokio::test]
async fn resume_all_continues_restored_headless_worker_without_attachment() {
    let (id, sessions, members, _) = fixture(true, "failed", false).await;
    add_unanswered_user(&id, &sessions).await;
    assert_eq!(resume_all_count(&sessions, &members).await, 1);
    assert_eq!(resume_all_count(&sessions, &members).await, 0);
    let agent = sessions.read().await.get(&id).unwrap().clone();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let agent = agent.lock().await;
        assert_eq!(
            agent.last_visible_conversation_role(),
            Some(crate::message::Role::Assistant)
        );
        assert_eq!(members.read().await[&id].status, "ready");
    })
    .await
    .expect("tracked resume must complete and release its reservation");
}

#[tokio::test]
async fn resume_all_does_not_continue_empty_restored_worker() {
    let (_id, sessions, members, _) = fixture(true, "ready", false).await;
    assert_eq!(resume_all_count(&sessions, &members).await, 0);
}
