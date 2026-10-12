use super::{handle_comm_list, handle_comm_message};
use crate::agent::Agent;
use crate::message::{Message, ToolDefinition};
use crate::protocol::{CommDeliveryMode, NotificationType, ServerEvent};
use crate::provider::{EventStream, Provider};
use crate::server::{ClientConnectionInfo, SessionInterruptQueues, SwarmEvent, SwarmMember};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, atomic::AtomicU64};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

struct TestProvider;

#[async_trait]
impl Provider for TestProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Err(anyhow::anyhow!(
            "test provider complete should not be called in client_comm tests"
        ))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(TestProvider)
    }
}

async fn test_agent() -> Arc<Mutex<Agent>> {
    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let registry = Registry::new(provider.clone()).await;
    Arc::new(Mutex::new(Agent::new(provider, registry)))
}

#[tokio::test]
async fn comm_message_default_does_not_queue_soft_interrupt_for_connected_session() {
    let sender = test_agent().await;
    let target = test_agent().await;

    let sender_id = sender.lock().await.session_id().to_string();
    let target_id = target.lock().await.session_id().to_string();
    let target_queue = target.lock().await.soft_interrupt_queue();

    let sessions = Arc::new(RwLock::new(HashMap::from([
        (sender_id.clone(), sender.clone()),
        (target_id.clone(), target.clone()),
    ])));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));

    let (sender_event_tx, _sender_event_rx) = mpsc::unbounded_channel();
    let (target_event_tx, mut target_event_rx) = mpsc::unbounded_channel();
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let swarm_id = "swarm-test".to_string();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (
            sender_id.clone(),
            SwarmMember {
                session_id: sender_id.clone(),
                event_tx: sender_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("falcon".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
        (
            target_id.clone(),
            SwarmMember {
                session_id: target_id.clone(),
                event_tx: target_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("bear".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.clone(),
        HashSet::from([sender_id.clone(), target_id.clone()]),
    )])));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.clone(),
        HashMap::from([(
            "religion-debate".to_string(),
            HashSet::from([target_id.clone()]),
        )]),
    )])));
    let event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>> =
        Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(16);
    let client_connections = Arc::new(RwLock::new(HashMap::from([(
        "client-1".to_string(),
        ClientConnectionInfo {
            client_id: "client-1".to_string(),
            session_id: target_id.clone(),
            client_instance_id: None,
            debug_client_id: None,
            connected_at: Instant::now(),
            last_seen: Instant::now(),
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));

    handle_comm_message(
        1,
        sender_id.clone(),
        "hello".to_string(),
        None,
        Some("religion-debate".to_string()),
        None,
        None,
        None,
        None,
        &client_event_tx,
        &sessions,
        &soft_interrupt_queues,
        &swarm_members,
        &swarms_by_id,
        &channel_subscriptions,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_connections,
    )
    .await;

    match target_event_rx.recv().await.expect("target notification") {
        ServerEvent::Notification {
            from_session,
            from_name,
            notification_type,
            message,
        } => {
            assert_eq!(from_session, sender_id);
            assert_eq!(from_name.as_deref(), Some("falcon"));
            match notification_type {
                NotificationType::Message { scope, channel, .. } => {
                    assert_eq!(scope.as_deref(), Some("channel"));
                    assert_eq!(channel.as_deref(), Some("religion-debate"));
                }
                other => panic!("unexpected notification type: {:?}", other),
            }
            assert_eq!(message, "#religion-debate from falcon: hello");
        }
        other => panic!("unexpected event: {:?}", other),
    }

    match client_event_rx.recv().await.expect("done event") {
        ServerEvent::Done { id } => assert_eq!(id, 1),
        other => panic!("unexpected client event: {:?}", other),
    }

    let pending = target_queue.lock().expect("target queue lock");
    assert!(
        pending.is_empty(),
        "connected interactive session should not get synthetic user-message interrupt"
    );
}

#[tokio::test]
async fn comm_message_with_wake_queues_soft_interrupt_for_busy_connected_session() {
    let sender = test_agent().await;
    let target = test_agent().await;

    let sender_id = sender.lock().await.session_id().to_string();
    let target_id = target.lock().await.session_id().to_string();
    let target_queue = target.lock().await.soft_interrupt_queue();

    let sessions = Arc::new(RwLock::new(HashMap::from([
        (sender_id.clone(), sender.clone()),
        (target_id.clone(), target.clone()),
    ])));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    crate::server::register_session_interrupt_queue(
        &soft_interrupt_queues,
        &target_id,
        target_queue.clone(),
    )
    .await;

    let (sender_event_tx, _sender_event_rx) = mpsc::unbounded_channel();
    let (target_event_tx, mut target_event_rx) = mpsc::unbounded_channel();
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let swarm_id = "swarm-test".to_string();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (
            sender_id.clone(),
            SwarmMember {
                session_id: sender_id.clone(),
                event_tx: sender_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("falcon".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
        (
            target_id.clone(),
            SwarmMember {
                session_id: target_id.clone(),
                event_tx: target_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("bear".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.clone(),
        HashSet::from([sender_id.clone(), target_id.clone()]),
    )])));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>> =
        Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(16);
    let client_connections = Arc::new(RwLock::new(HashMap::from([(
        "client-1".to_string(),
        ClientConnectionInfo {
            client_id: "client-1".to_string(),
            session_id: target_id.clone(),
            client_instance_id: None,
            debug_client_id: None,
            connected_at: Instant::now(),
            last_seen: Instant::now(),
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));

    let _busy_guard = target.lock().await;

    tokio::time::timeout(
        Duration::from_secs(2),
        handle_comm_message(
            1,
            sender_id.clone(),
            "hello now".to_string(),
            Some(target_id.clone()),
            None,
            Some(CommDeliveryMode::Wake),
            None,
            None,
            None,
            &client_event_tx,
            &sessions,
            &soft_interrupt_queues,
            &swarm_members,
            &swarms_by_id,
            &channel_subscriptions,
            &event_history,
            &event_counter,
            &swarm_event_tx,
            &client_connections,
        ),
    )
    .await
    .expect("comm message should not deadlock");

    match target_event_rx.recv().await.expect("target notification") {
        ServerEvent::Notification {
            from_session,
            from_name,
            notification_type,
            message,
        } => {
            assert_eq!(from_session, sender_id);
            assert_eq!(from_name.as_deref(), Some("falcon"));
            match notification_type {
                NotificationType::Message { scope, channel, .. } => {
                    assert_eq!(scope.as_deref(), Some("dm"));
                    assert_eq!(channel, None);
                }
                other => panic!("unexpected notification type: {:?}", other),
            }
            assert_eq!(message, "DM from falcon: hello now");
        }
        other => panic!("unexpected event: {:?}", other),
    }

    match client_event_rx.recv().await.expect("done event") {
        ServerEvent::Done { id } => assert_eq!(id, 1),
        other => panic!("unexpected client event: {:?}", other),
    }

    let pending = target_queue.lock().expect("target queue lock");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "DM from falcon: hello now");
    assert_eq!(
        pending[0].source,
        jcode_agent_runtime::SoftInterruptSource::System
    );
}

#[tokio::test]
async fn comm_list_includes_member_status_and_detail() {
    let requester = test_agent().await;
    let peer = test_agent().await;

    let requester_id = requester.lock().await.session_id().to_string();
    let peer_id = peer.lock().await.session_id().to_string();
    let swarm_id = "swarm-test".to_string();

    let (requester_event_tx, _requester_event_rx) = mpsc::unbounded_channel();
    let (peer_event_tx, _peer_event_rx) = mpsc::unbounded_channel();
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (
            requester_id.clone(),
            SwarmMember {
                session_id: requester_id.clone(),
                event_tx: requester_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("falcon".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
        (
            peer_id.clone(),
            SwarmMember {
                session_id: peer_id.clone(),
                event_tx: peer_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "running".to_string(),
                detail: Some("working on tests".to_string()),
                friendly_name: Some("bear".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id,
        HashSet::from([requester_id.clone(), peer_id.clone()]),
    )])));
    let file_touch = crate::server::FileTouchService::new();
    let sessions = Arc::new(RwLock::new(HashMap::from([
        (requester_id.clone(), requester.clone()),
        (peer_id.clone(), peer.clone()),
    ])));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));

    handle_comm_list(
        1,
        requester_id,
        &client_event_tx,
        &swarm_members,
        &swarms_by_id,
        &file_touch,
        &sessions,
        &client_connections,
    )
    .await;

    match client_event_rx.recv().await.expect("comm list response") {
        ServerEvent::CommMembers { id, members } => {
            assert_eq!(id, 1);
            let peer = members
                .into_iter()
                .find(|member| member.friendly_name.as_deref() == Some("bear"))
                .expect("peer entry present");
            assert_eq!(peer.status.as_deref(), Some("running"));
            assert_eq!(peer.detail.as_deref(), Some("working on tests"));
        }
        other => panic!("unexpected response: {other:?}"),
    }
}

#[tokio::test]
async fn comm_message_accepts_friendly_name_dm_target() {
    let sender = test_agent().await;
    let target = test_agent().await;

    let sender_id = sender.lock().await.session_id().to_string();
    let target_id = target.lock().await.session_id().to_string();
    let swarm_id = "swarm-test".to_string();

    let sessions = Arc::new(RwLock::new(HashMap::from([
        (sender_id.clone(), sender.clone()),
        (target_id.clone(), target.clone()),
    ])));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));

    let (sender_event_tx, _sender_event_rx) = mpsc::unbounded_channel();
    let (target_event_tx, mut target_event_rx) = mpsc::unbounded_channel();
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (
            sender_id.clone(),
            SwarmMember {
                session_id: sender_id.clone(),
                event_tx: sender_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("falcon".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
        (
            target_id.clone(),
            SwarmMember {
                session_id: target_id.clone(),
                event_tx: target_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("bear".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.clone(),
        HashSet::from([sender_id.clone(), target_id.clone()]),
    )])));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>> =
        Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(16);
    let client_connections = Arc::new(RwLock::new(HashMap::new()));

    handle_comm_message(
        1,
        sender_id.clone(),
        "hello bear".to_string(),
        Some("bear".to_string()),
        None,
        Some(CommDeliveryMode::Notify),
        None,
        None,
        None,
        &client_event_tx,
        &sessions,
        &soft_interrupt_queues,
        &swarm_members,
        &swarms_by_id,
        &channel_subscriptions,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_connections,
    )
    .await;

    match target_event_rx.recv().await.expect("target notification") {
        ServerEvent::Notification {
            from_session,
            from_name,
            notification_type,
            message,
        } => {
            assert_eq!(from_session, sender_id);
            assert_eq!(from_name.as_deref(), Some("falcon"));
            match notification_type {
                NotificationType::Message { scope, channel, .. } => {
                    assert_eq!(scope.as_deref(), Some("dm"));
                    assert_eq!(channel, None);
                }
                other => panic!("unexpected notification type: {:?}", other),
            }
            assert_eq!(message, "DM from falcon: hello bear");
        }
        other => panic!("unexpected event: {:?}", other),
    }

    match client_event_rx.recv().await.expect("done event") {
        ServerEvent::Done { id } => assert_eq!(id, 1),
        other => panic!("unexpected client event: {:?}", other),
    }
}

#[tokio::test]
async fn comm_message_rejects_ambiguous_friendly_name_dm_target() {
    let sender = test_agent().await;
    let target_one = test_agent().await;
    let target_two = test_agent().await;

    let sender_id = sender.lock().await.session_id().to_string();
    let target_one_id = target_one.lock().await.session_id().to_string();
    let target_two_id = target_two.lock().await.session_id().to_string();
    let swarm_id = "swarm-test".to_string();

    let sessions = Arc::new(RwLock::new(HashMap::from([
        (sender_id.clone(), sender.clone()),
        (target_one_id.clone(), target_one.clone()),
        (target_two_id.clone(), target_two.clone()),
    ])));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));

    let (sender_event_tx, _sender_event_rx) = mpsc::unbounded_channel();
    let (target_one_event_tx, _target_one_event_rx) = mpsc::unbounded_channel();
    let (target_two_event_tx, _target_two_event_rx) = mpsc::unbounded_channel();
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        (
            sender_id.clone(),
            SwarmMember {
                session_id: sender_id.clone(),
                event_tx: sender_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("falcon".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
        (
            target_one_id.clone(),
            SwarmMember {
                session_id: target_one_id.clone(),
                event_tx: target_one_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("bear".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
        (
            target_two_id.clone(),
            SwarmMember {
                session_id: target_two_id.clone(),
                event_tx: target_two_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.clone()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("bear".to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.clone(),
        HashSet::from([
            sender_id.clone(),
            target_one_id.clone(),
            target_two_id.clone(),
        ]),
    )])));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>> =
        Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(16);
    let client_connections = Arc::new(RwLock::new(HashMap::new()));

    handle_comm_message(
        1,
        sender_id,
        "hello bears".to_string(),
        Some("bear".to_string()),
        None,
        None,
        None,
        None,
        None,
        &client_event_tx,
        &sessions,
        &soft_interrupt_queues,
        &swarm_members,
        &swarms_by_id,
        &channel_subscriptions,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_connections,
    )
    .await;

    match client_event_rx.recv().await.expect("error event") {
        ServerEvent::Error { id, message, .. } => {
            assert_eq!(id, 1);
            assert!(message.contains("ambiguous in swarm"), "{message}");
            assert!(message.contains("Use an exact session id"), "{message}");
            assert!(message.contains(&target_one_id), "{message}");
            assert!(message.contains(&target_two_id), "{message}");
            assert!(message.contains("bear ["), "{message}");
        }
        other => panic!("unexpected client event: {:?}", other),
    }
}

/// Broadcasts are subtree-scoped: a non-coordinator sender reaches only the
/// agents it (transitively) spawned, never unrelated peers, while a
/// coordinator retains whole-swarm reach.
#[tokio::test]
async fn comm_broadcast_reaches_only_senders_spawned_subtree() {
    fn member(
        session_id: &str,
        role: &str,
        report_back_to: Option<&str>,
        swarm_id: &str,
    ) -> (SwarmMember, mpsc::UnboundedReceiver<ServerEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        (
            SwarmMember {
                session_id: session_id.to_string(),
                event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.to_string()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some(session_id.to_string()),
                report_back_to_session_id: report_back_to.map(str::to_string),
                latest_completion_report: None,
                role: role.to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: true,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
            event_rx,
        )
    }

    let swarm_id = "swarm-subtree";
    // Tree: coord (coordinator, root)
    //       sender (root peer) -> child -> grandchild
    //       outsider (root peer, unrelated)
    let (coord, mut coord_rx) = member("coord", "coordinator", None, swarm_id);
    let (sender, _sender_rx) = member("sender", "agent", None, swarm_id);
    let (child, mut child_rx) = member("child", "agent", Some("sender"), swarm_id);
    let (grandchild, mut grandchild_rx) = member("grandchild", "agent", Some("child"), swarm_id);
    let (outsider, mut outsider_rx) = member("outsider", "agent", None, swarm_id);

    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        ("coord".to_string(), coord),
        ("sender".to_string(), sender),
        ("child".to_string(), child),
        ("grandchild".to_string(), grandchild),
        ("outsider".to_string(), outsider),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id.to_string(),
        HashSet::from([
            "coord".to_string(),
            "sender".to_string(),
            "child".to_string(),
            "grandchild".to_string(),
            "outsider".to_string(),
        ]),
    )])));
    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>> =
        Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(16);
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    handle_comm_message(
        1,
        "sender".to_string(),
        "subtree update".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        &client_event_tx,
        &sessions,
        &soft_interrupt_queues,
        &swarm_members,
        &swarms_by_id,
        &channel_subscriptions,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_connections,
    )
    .await;

    match client_event_rx.recv().await.expect("done event") {
        ServerEvent::Done { id } => assert_eq!(id, 1),
        other => panic!("unexpected client event: {:?}", other),
    }

    // Direct child and transitive grandchild both receive the broadcast.
    assert!(matches!(
        child_rx.try_recv(),
        Ok(ServerEvent::Notification { .. })
    ));
    assert!(matches!(
        grandchild_rx.try_recv(),
        Ok(ServerEvent::Notification { .. })
    ));
    // Unrelated root peers and the coordinator do not.
    assert!(outsider_rx.try_recv().is_err());
    assert!(coord_rx.try_recv().is_err());

    // Coordinator broadcast still reaches the whole swarm.
    handle_comm_message(
        2,
        "coord".to_string(),
        "swarm-wide notice".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        &client_event_tx,
        &sessions,
        &soft_interrupt_queues,
        &swarm_members,
        &swarms_by_id,
        &channel_subscriptions,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_connections,
    )
    .await;
    match client_event_rx.recv().await.expect("done event") {
        ServerEvent::Done { id } => assert_eq!(id, 2),
        other => panic!("unexpected client event: {:?}", other),
    }
    assert!(matches!(
        outsider_rx.try_recv(),
        Ok(ServerEvent::Notification { .. })
    ));
    assert!(matches!(
        child_rx.try_recv(),
        Ok(ServerEvent::Notification { .. })
    ));
}

/// Cross-swarm DMs: `to_swarm` resolves a label (or id) of another live
/// swarm. Without `to_session` the DM lands on that swarm's coordinator; with
/// it, on the named agent. Without `to_swarm`, DMs stay swarm-local.
#[tokio::test]
#[allow(clippy::await_holding_lock)] // serializes tests sharing the global swarm label registry
async fn comm_message_cross_swarm_dm_by_label() {
    let _labels_guard = crate::server::swarm_labels::SWARM_LABELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    crate::server::swarm_labels::reset_swarm_labels_for_test();

    fn member(
        session_id: &str,
        role: &str,
        swarm_id: &str,
    ) -> (SwarmMember, mpsc::UnboundedReceiver<ServerEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        (
            SwarmMember {
                session_id: session_id.to_string(),
                event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: Some(swarm_id.to_string()),
                swarm_enabled: true,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some(session_id.to_string()),
                report_back_to_session_id: None,
                latest_completion_report: None,
                role: role.to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: true,
                output_tail: None,
                todo_progress: None,
                todo_items: Vec::new(),
                runtime: crate::protocol::SwarmMemberRuntime::default(),
                task_label: None,
            },
            event_rx,
        )
    }

    let (alpha, _alpha_rx) = member("alpha", "coordinator", "swarm-a");
    let (beta_coord, mut beta_coord_rx) = member("beta-coord", "coordinator", "swarm-b");
    let (beta_worker, mut beta_worker_rx) = member("beta-worker", "agent", "swarm-b");
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        ("alpha".to_string(), alpha),
        ("beta-coord".to_string(), beta_coord),
        ("beta-worker".to_string(), beta_worker),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([
        ("swarm-a".to_string(), HashSet::from(["alpha".to_string()])),
        (
            "swarm-b".to_string(),
            HashSet::from(["beta-coord".to_string(), "beta-worker".to_string()]),
        ),
    ])));
    let live: HashSet<String> = ["swarm-a", "swarm-b"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    crate::server::swarm_labels::set_swarm_label("swarm-b", "Backend", &live).unwrap();

    let sessions = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>> =
        Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(16);
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();

    macro_rules! send {
        ($id:expr, $to_session:expr, $to_swarm:expr) => {
            handle_comm_message(
                $id,
                "alpha".to_string(),
                "hello from alpha".to_string(),
                $to_session,
                None,
                Some(CommDeliveryMode::Notify),
                None,
                None,
                $to_swarm,
                &client_event_tx,
                &sessions,
                &soft_interrupt_queues,
                &swarm_members,
                &swarms_by_id,
                &channel_subscriptions,
                &event_history,
                &event_counter,
                &swarm_event_tx,
                &client_connections,
            )
            .await
        };
    }

    // Without to_swarm, a session in another swarm is unreachable.
    send!(1, Some("beta-worker".to_string()), None);
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Error { id: 1, .. })
    ));
    assert!(beta_worker_rx.try_recv().is_err());

    // Label only: delivered to the other swarm's coordinator.
    send!(2, None, Some("backend".to_string()));
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Done { id: 2 })
    ));
    match beta_coord_rx.try_recv() {
        Ok(ServerEvent::Notification {
            message,
            notification_type: NotificationType::Message { scope, .. },
            ..
        }) => {
            assert_eq!(scope.as_deref(), Some("dm"));
            assert!(message.starts_with("Cross-swarm DM from alpha (swarm 'swarm-a')"));
        }
        other => panic!("expected cross-swarm DM, got {other:?}"),
    }
    assert!(beta_worker_rx.try_recv().is_err());

    // Label + friendly name: delivered to that agent.
    send!(
        3,
        Some("beta-worker".to_string()),
        Some("swarm-b".to_string())
    );
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Done { id: 3 })
    ));
    assert!(matches!(
        beta_worker_rx.try_recv(),
        Ok(ServerEvent::Notification { .. })
    ));

    // Unknown swarm label is rejected.
    send!(4, None, Some("nope".to_string()));
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Error { id: 4, .. })
    ));

    crate::server::swarm_labels::reset_swarm_labels_for_test();
}
