use super::*;
use anyhow::{Result, anyhow};

#[tokio::test]
async fn handle_clear_session_replaces_runtime_handles_and_updates_shutdown_registration()
-> Result<()> {
    let _guard = crate::storage::lock_test_env();

    let old_session_id = "session_before_clear";
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

    let sessions = Arc::new(RwLock::new(HashMap::from([(
        old_session_id.to_string(),
        Arc::clone(&agent),
    )])));
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
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));
    let swarm_members = Arc::new(RwLock::new(HashMap::<String, SwarmMember>::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::<String, HashSet<String>>::new()));
    let file_touches = Arc::new(RwLock::new(HashMap::<PathBuf, Vec<FileAccess>>::new()));
    let files_touched_by_session =
        Arc::new(RwLock::new(HashMap::<String, HashSet<PathBuf>>::new()));
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::<
        String,
        HashMap<String, HashSet<String>>,
    >::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::<String, VersionedPlan>::new()));
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();

    let mut client_session_id = old_session_id.to_string();
    handle_clear_session(
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
        &file_touches,
        &files_touched_by_session,
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

    old_queue
        .lock()
        .map_err(|_| anyhow!("old queue lock"))?
        .push(jcode_agent_runtime::SoftInterruptMessage {
            content: "stale queued message".to_string(),
            urgent: false,
            source: jcode_agent_runtime::SoftInterruptSource::User,
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
    let second = client_event_rx
        .recv()
        .await
        .ok_or_else(|| anyhow!("done event"))?;
    assert!(matches!(second, ServerEvent::Done { id: 7 }));
    Ok(())
}
