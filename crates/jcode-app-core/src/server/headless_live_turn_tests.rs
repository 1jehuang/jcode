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
        Arc::new(crate::server::session_manager::SessionManager::from_agents(
            HashMap::from([(id.clone(), agent)]),
        )),
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
    sessions.clear_for_test().await;
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

#[tokio::test]
async fn peer_message_queued_as_a_turn_ends_resumes_with_reply_routing() {
    use jcode_agent_runtime::SoftInterruptSource;
    for busy in [false, true] {
        let (id, sessions, members, mut receiver) = fixture(true, "ready", true).await;
        let agent = sessions.read().await.get(&id).unwrap().clone();
        let queue = agent.lock().await.soft_interrupt_queue();
        let queues = Arc::new(RwLock::new(HashMap::from([(id.clone(), queue.clone())])));
        let (swarm_tx, _) = broadcast::channel(16);
        let swarm = LiveTurnSwarmContext::new(
            &members,
            &Arc::new(RwLock::new(HashMap::new())),
            &Arc::new(RwLock::new(VecDeque::new())),
            &Arc::new(AtomicU64::new(0)),
            &swarm_tx,
        );
        let busy_guard = if busy { Some(agent.lock().await) } else { None };
        let notification = "<channel source=\"claude-peers\" from_id=\"peer-123\">\nPlease check the build.\n</channel>";
        super::super::background_tasks::dispatch_peer_message(
            &crate::bus::PeerMessageReceived {
                session_id: id.clone(),
                notification: notification.into(),
            },
            &sessions,
            &queues,
            swarm.clone(),
        )
        .await;
        if busy {
            super::super::background_tasks::dispatch_peer_message(
                &crate::bus::PeerMessageReceived {
                    session_id: id.clone(),
                    notification: notification.replace("peer-123", "peer-456"),
                },
                &sessions,
                &queues,
                swarm.clone(),
            )
            .await;
            let pending = queue.lock().unwrap();
            assert_eq!(pending.len(), 2);
            assert_eq!(pending[0].source, SoftInterruptSource::PeerMessage);
            assert!(pending[0].content.contains("using the sender's from_id"));
        }
        drop(busy_guard);
        // Simulate the turn's completion after its last interrupt drain.
        run_stranded_background_completions(id.clone(), agent.clone(), swarm).await;
        let receiver = receiver.as_mut().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let event = receiver.recv().await.expect("turn event");
                let event = match event {
                    ServerEvent::TurnEvent { event, .. } => *event,
                    event => event,
                };
                match event {
                    ServerEvent::Done { .. } => break,
                    ServerEvent::Error { message, .. } => panic!("peer wake failed: {message}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("peer message wakes the session without user input");
        let agent = agent.lock().await;
        let deliveries: Vec<_> = agent
            .messages()
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                crate::message::ContentBlock::Text { text, .. } if text.contains(notification) => {
                    Some(text)
                }
                _ => None,
            })
            .collect();
        assert_eq!(deliveries.len(), 1, "peer message delivered once");
        assert!(deliveries[0].contains("using the sender's from_id"));
        assert!(
            !deliveries[0].contains("peer-456"),
            "separate peers keep separate context entries"
        );
        if busy {
            assert_eq!(agent.messages().iter().flat_map(|message| message.content.iter())
                .filter(|block| matches!(block, crate::message::ContentBlock::Text { text, .. } if text.contains("peer-456"))).count(), 1);
        }
        assert!(queue.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn completion_queued_as_a_turn_ends_starts_the_follow_up_turn() {
    use jcode_agent_runtime::{SoftInterruptMessage, SoftInterruptSource};
    let (id, sessions, members, _receiver) = fixture(true, "ready", true).await;
    let agent = sessions.read().await.get(&id).unwrap().clone();
    let queue = agent.lock().await.soft_interrupt_queue();
    for (content, source) in [
        ("user note stays queued", SoftInterruptSource::User),
        (
            "stranded completion bg-1",
            SoftInterruptSource::BackgroundTask,
        ),
    ] {
        queue.lock().unwrap().push(SoftInterruptMessage {
            content: content.into(),
            images: Vec::new(),
            urgent: false,
            source,
            client_id: None,
        });
    }
    let (swarm_tx, _) = broadcast::channel(16);
    let swarm = LiveTurnSwarmContext::new(
        &members,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(AtomicU64::new(0)),
        &swarm_tx,
    );

    run_stranded_background_completions(id.clone(), Arc::clone(&agent), swarm).await;

    let pending: Vec<_> = queue
        .lock()
        .unwrap()
        .iter()
        .map(|message| message.content.clone())
        .collect();
    assert_eq!(pending, vec!["user note stays queued".to_string()]);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            {
                let agent = agent.lock().await;
                if agent.last_visible_conversation_role() == Some(crate::message::Role::Assistant) {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the stranded completion must start a follow-up turn");
}

#[tokio::test]
async fn stranded_completion_starts_after_the_client_releases_the_finished_turn() {
    use jcode_agent_runtime::{SoftInterruptMessage, SoftInterruptSource};
    let (id, sessions, members, _receiver) = fixture(true, "ready", true).await;
    let agent = sessions.read().await.get(&id).unwrap().clone();
    let queue = agent.lock().await.soft_interrupt_queue();
    queue.lock().unwrap().push(SoftInterruptMessage {
        content: "stranded completion bg-2".into(),
        images: Vec::new(),
        urgent: false,
        source: SoftInterruptSource::BackgroundTask,
        client_id: None,
    });
    // A finished client turn: its lease is gone, but the client's completion
    // reconciler still holds the turn's publication pin.
    let (pin, released) = {
        let lease = agent.lock().await.claim_server_turn().unwrap();
        (lease.publication_pin(), lease.released())
    };
    let (swarm_tx, _) = broadcast::channel(16);
    let swarm = LiveTurnSwarmContext::new(
        &members,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(AtomicU64::new(0)),
        &swarm_tx,
    );

    let follow_up = spawn_stranded_background_completions_after_release(
        released,
        id.clone(),
        Arc::clone(&agent),
        swarm,
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(!follow_up.is_finished(), "must wait for the pin");
    assert_eq!(queue.lock().unwrap().len(), 1);

    drop(pin);
    follow_up.await.unwrap();
    assert!(queue.lock().unwrap().is_empty());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            {
                let agent = agent.lock().await;
                if agent.last_visible_conversation_role() == Some(crate::message::Role::Assistant) {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the stranded completion must start a follow-up turn");
}

/// A completion retried while a client turn released the agent mutex but
/// still owns its turn lease must neither run nor supersede that turn: the
/// turn keeps its terminal publication and its own drain delivers the
/// completion, which stays queued.
#[tokio::test]
async fn completion_retry_leaves_the_active_turn_its_terminal_event() {
    let (id, sessions, members, _receiver) = fixture(true, "ready", true).await;
    let agent = sessions.read().await.get(&id).unwrap().clone();
    let queue = agent.lock().await.soft_interrupt_queue();
    let lease = agent.lock().await.claim_server_turn().unwrap();
    let publication = crate::server::swarm::begin_turn_publication(&id).await;
    let task = crate::bus::BackgroundTaskCompleted {
        task_id: "bg-mid-turn".into(),
        tool_name: "bash".into(),
        display_name: None,
        session_id: id.clone(),
        status: crate::bus::BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "done\n".into(),
        output_file: std::env::temp_dir().join("bg-mid-turn.output"),
        duration_secs: 1.0,
        notify: true,
        wake: true,
    };
    let (swarm_tx, _) = broadcast::channel(16);

    crate::server::background_tasks::dispatch_background_task_completion(
        &task,
        &sessions,
        &Arc::new(RwLock::new(HashMap::new())),
        &members,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(AtomicU64::new(0)),
        &swarm_tx,
    )
    .await;

    let pending: Vec<_> = queue
        .lock()
        .unwrap()
        .iter()
        .map(|message| message.content.clone())
        .collect();
    assert_eq!(pending.len(), 1, "left queued for the turn's own drain");
    assert!(pending[0].contains("bg-mid-turn"));
    let mut completed = false;
    publication.complete(async { completed = true }).await;
    assert!(completed, "the active turn must still publish Done/Error");
    drop(lease);
}
