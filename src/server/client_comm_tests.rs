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
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                NotificationType::Message { scope, channel } => {
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
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                NotificationType::Message { scope, channel } => {
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
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
            },
        ),
    ])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        swarm_id,
        HashSet::from([requester_id.clone(), peer_id.clone()]),
    )])));
    let file_touches = Arc::new(RwLock::new(HashMap::new()));

    handle_comm_list(
        1,
        requester_id,
        &client_event_tx,
        &swarm_members,
        &swarms_by_id,
        &file_touches,
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
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                NotificationType::Message { scope, channel } => {
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
                role: "coordinator".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
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
