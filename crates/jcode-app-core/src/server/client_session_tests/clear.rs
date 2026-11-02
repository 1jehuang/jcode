use super::*;
use anyhow::{Result, anyhow};
use std::time::Duration;

#[tokio::test]
async fn handle_clear_session_replaces_runtime_handles_and_updates_shutdown_registration()
-> Result<()> {
    exercise_clear_session(false).await
}

#[tokio::test]
async fn handle_clear_session_preserves_shared_source_runtime_and_viewer() -> Result<()> {
    exercise_clear_session(true).await
}

async fn exercise_clear_session(shared_source: bool) -> Result<()> {
    let _guard = crate::storage::lock_test_env();

    let old_session_id = "session_before_clear";
    let session_env = HashMap::from([("CLEAR_SESSION_SECRET".into(), "test-only".into())]);
    crate::session_env::set_session_env(old_session_id, session_env.clone())?;
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider.clone(),
        registry.clone(),
        old_session_id,
        Vec::new(),
    )));

    let old_queue = {
        let guard = agent.lock().await;
        guard.soft_interrupt_queue()
    };
    let old_background_signal = {
        let guard = agent.lock().await;
        guard.background_tool_signal()
    };
    let old_cancel_signal = {
        let guard = agent.lock().await;
        guard.graceful_shutdown_signal()
    };

    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::from([(old_session_id.to_string(), Arc::clone(&agent))]),
    ));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
        old_session_id.to_string(),
        old_cancel_signal.clone(),
    )])));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::from([(
        old_session_id.to_string(),
        old_queue.clone(),
    )])));
    let now = Instant::now();
    let client_connections = Arc::new(RwLock::new(HashMap::from([(
        "conn_clear".to_string(),
        ClientConnectionInfo {
            client_id: "conn_clear".to_string(),
            session_id: old_session_id.to_string(),
            client_instance_id: None,
            debug_client_id: Some("debug_clear".to_string()),
            connected_at: now,
            last_seen: now,
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        old_session_id.to_string(),
        test_swarm_member(old_session_id, "ready"),
    )])));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::from([(
        "swarm-test".to_string(),
        HashSet::from([old_session_id.to_string()]),
    )])));
    let file_touch = FileTouchService::new();
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::from([(
        "swarm-test".to_string(),
        VersionedPlan {
            items: Vec::new(),
            version: 1,
            participants: HashSet::from([old_session_id.to_string()]),
            task_progress: HashMap::new(),
        },
    )])));
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();

    let (viewer_tx, mut viewer_rx) = mpsc::unbounded_channel::<ServerEvent>();
    if shared_source {
        let mut connections = client_connections.write().await;
        let mut viewer = connections.get("conn_clear").unwrap().clone();
        viewer.client_id = "conn_viewer".into();
        connections.insert("conn_viewer".into(), viewer);
        drop(connections);
        let mut members = swarm_members.write().await;
        let member = members.get_mut(old_session_id).unwrap();
        member.event_tx = client_event_tx.clone();
        member
            .event_txs
            .insert("conn_clear".into(), client_event_tx.clone());
        member.event_txs.insert("conn_viewer".into(), viewer_tx);
    }
    let mut client_session_id = old_session_id.to_string();
    // A reserved turn can outlive the Agent mutex while its terminal events drain.
    // Clear must respect that ownership gap, not just mutex availability.
    let lease = agent.lock().await.claim_server_turn()?;
    assert!(
        agent.try_lock().is_ok(),
        "the test must leave the Agent mutex free"
    );
    let rejected_agent = tokio::time::timeout(
        Duration::from_secs(1),
        handle_clear_session(
            6,
            false,
            &mut client_session_id,
            "conn_clear",
            &agent,
            &provider,
            &registry,
            &sessions,
            &shutdown_signals,
            &soft_interrupt_queues,
            &client_connections,
            &swarm_members,
            &swarms_by_id,
            &file_touch,
            &channel_subscriptions,
            &channel_subscriptions_by_session,
            &swarm_plans,
            &event_history,
            &event_counter,
            &swarm_event_tx,
            &client_event_tx,
        ),
    )
    .await
    .expect("clear must reject an active turn lease without waiting for its release");
    assert!(Arc::ptr_eq(&rejected_agent, &agent));
    assert_eq!(client_session_id, old_session_id);
    assert!(Arc::ptr_eq(
        sessions.read().await.get(old_session_id).unwrap(),
        &agent
    ));
    assert!(shutdown_signals.read().await.contains_key(old_session_id));
    assert!(
        soft_interrupt_queues
            .read()
            .await
            .contains_key(old_session_id)
    );
    assert_eq!(crate::session_env::session_env(old_session_id), session_env);
    assert!(matches!(
        client_event_rx.recv().await,
        Some(ServerEvent::Error { id: 6, .. })
    ));
    assert!(
        client_event_rx.try_recv().is_err(),
        "rejected clear must not publish a new session"
    );
    drop(lease);

    let old_agent = Arc::clone(&agent);
    let agent = handle_clear_session(
        7,
        false,
        &mut client_session_id,
        "conn_clear",
        &agent,
        &provider,
        &registry,
        &sessions,
        &shutdown_signals,
        &soft_interrupt_queues,
        &client_connections,
        &swarm_members,
        &swarms_by_id,
        &file_touch,
        &channel_subscriptions,
        &channel_subscriptions_by_session,
        &swarm_plans,
        &event_history,
        &event_counter,
        &swarm_event_tx,
        &client_event_tx,
    )
    .await;

    assert_ne!(client_session_id, old_session_id);
    assert!(
        agent.lock().await.claim_server_turn().is_ok(),
        "fresh runtime gate must release after clear bookkeeping"
    );
    assert!(
        !Arc::ptr_eq(&agent, &old_agent),
        "clear must create a new runtime Arc"
    );
    assert!(Arc::ptr_eq(
        sessions.read().await.get(&client_session_id).unwrap(),
        &agent
    ));
    assert_eq!(old_agent.lock().await.session_id(), old_session_id);
    if shared_source {
        assert!(Arc::ptr_eq(
            sessions.read().await.get(old_session_id).unwrap(),
            &old_agent
        ));
        assert_eq!(crate::session_env::session_env(old_session_id), session_env);
        assert_eq!(
            crate::session_env::session_env(&client_session_id),
            session_env
        );
        assert!(old_agent.lock().await.has_concurrency_tracking());
        assert!(old_agent.lock().await.claim_server_turn().is_ok());
        let queues = soft_interrupt_queues.read().await;
        assert!(Arc::ptr_eq(queues.get(old_session_id).unwrap(), &old_queue));
        assert!(queues.contains_key(&client_session_id));
        drop(queues);
        assert!(shutdown_signals.read().await.contains_key(old_session_id));
        assert!(
            shutdown_signals
                .read()
                .await
                .contains_key(&client_session_id)
        );
        let members = swarm_members.read().await;
        let source = members.get(old_session_id).unwrap();
        assert!(source.event_txs.contains_key("conn_viewer"));
        assert!(!source.event_txs.contains_key("conn_clear"));
        assert!(
            members
                .get(&client_session_id)
                .unwrap()
                .event_txs
                .contains_key("conn_clear")
        );
        drop(members);
        assert!(swarms_by_id.read().await["swarm-test"].contains(old_session_id));
        assert!(
            swarm_plans.read().await["swarm-test"]
                .participants
                .contains(old_session_id)
        );
        let connections = client_connections.read().await;
        assert_eq!(connections["conn_viewer"].session_id, old_session_id);
        assert_eq!(connections["conn_clear"].session_id, client_session_id);
        drop(connections);
        crate::server::fanout_session_event(
            &swarm_members,
            old_session_id,
            ServerEvent::TextDelta {
                text: "source still live".into(),
            },
        )
        .await;
        assert!(
            matches!(viewer_rx.recv().await, Some(ServerEvent::TextDelta { text }) if text == "source still live")
        );
        assert!(
            matches!(client_event_rx.recv().await, Some(ServerEvent::SessionId { session_id }) if session_id == client_session_id)
        );
        assert!(
            matches!(client_event_rx.recv().await, Some(ServerEvent::TurnState { session_id, turn_id: None, .. }) if session_id == client_session_id)
        );
        assert!(matches!(
            client_event_rx.recv().await,
            Some(ServerEvent::Done { id: 7 })
        ));
        assert!(client_event_rx.try_recv().is_err());
        crate::session_env::remove_session_env(old_session_id);
        crate::session_env::remove_session_env(&client_session_id);
        return Ok(());
    }
    assert_eq!(
        crate::session_env::session_env(&client_session_id),
        session_env
    );
    assert!(crate::session_env::session_env(old_session_id).is_empty());
    crate::session_env::remove_session_env(&client_session_id);
    let members = swarm_members.read().await;
    assert!(members.get(old_session_id).is_none());
    let replacement_member = members
        .get(&client_session_id)
        .expect("replacement session should remain registered for swarm tools");
    assert!(replacement_member.swarm_enabled);
    assert_eq!(replacement_member.status, "ready");
    assert_ne!(replacement_member.swarm_id.as_deref(), Some("swarm-test"));
    let replacement_swarm_id = replacement_member
        .swarm_id
        .clone()
        .expect("replacement session should get a fresh swarm identity");
    drop(members);
    assert!(swarms_by_id.read().await.get("swarm-test").is_none());
    assert!(
        swarms_by_id
            .read()
            .await
            .get(&replacement_swarm_id)
            .is_some_and(|sessions| sessions.contains(&client_session_id))
    );
    let plans = swarm_plans.read().await;
    assert!(!plans["swarm-test"].participants.contains(old_session_id));
    assert!(
        !plans["swarm-test"]
            .participants
            .contains(&client_session_id)
    );
    drop(plans);

    old_queue
        .lock()
        .map_err(|_| anyhow!("old queue lock"))?
        .push(jcode_agent_runtime::SoftInterruptMessage {
            content: "stale queued message".to_string(),
            images: Vec::new(),
            urgent: false,
            source: jcode_agent_runtime::SoftInterruptSource::User,
            client_id: None,
        });
    old_background_signal.fire();
    old_cancel_signal.fire();

    let (new_queue, new_background_signal, new_cancel_signal) = {
        let guard = agent.lock().await;
        (
            guard.soft_interrupt_queue(),
            guard.background_tool_signal(),
            guard.graceful_shutdown_signal(),
        )
    };

    assert!(!Arc::ptr_eq(&old_queue, &new_queue));
    assert!(!new_background_signal.is_set());
    assert!(!new_cancel_signal.is_set());
    assert!(!agent.lock().await.has_soft_interrupts());

    let queue_map = soft_interrupt_queues.read().await;
    assert!(!queue_map.contains_key(old_session_id));
    assert!(queue_map.contains_key(&client_session_id));
    drop(queue_map);

    let signals = shutdown_signals.read().await;
    assert!(!signals.contains_key(old_session_id));
    let registered_signal = signals
        .get(&client_session_id)
        .ok_or_else(|| anyhow!("new session should have shutdown signal"))?
        .clone();
    drop(signals);
    registered_signal.fire();
    assert!(new_cancel_signal.is_set());

    let first = client_event_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("session id event"))?;
    assert!(matches!(first, ServerEvent::SessionId { .. }));
    let state = client_event_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("turn state event"))?;
    assert!(
        matches!(state, ServerEvent::TurnState { session_id, turn_id: None, .. } if session_id == client_session_id)
    );
    let terminal = client_event_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("done event"))?;
    assert!(matches!(terminal, ServerEvent::Done { id: 7 }));
    Ok(())
}
