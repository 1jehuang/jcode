#[tokio::test]
async fn handle_resume_session_allows_multiple_live_tui_attach() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;

    let target_session_id = "session_existing_live";
    let temp_session_id = "session_temp_connecting";

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let existing_registry = Registry::new(provider.clone()).await;
    let existing_agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider.clone(),
        existing_registry,
        target_session_id,
        Vec::new(),
    )));

    let new_registry = Registry::new(provider.clone()).await;
    let new_agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider.clone(),
        new_registry.clone(),
        temp_session_id,
        Vec::new(),
    )));

    let sessions = Arc::new(RwLock::new(HashMap::from([
        (target_session_id.to_string(), Arc::clone(&existing_agent)),
        (temp_session_id.to_string(), Arc::clone(&new_agent)),
    ])));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::<String, InterruptSignal>::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let now = Instant::now();
    let client_connections = Arc::new(RwLock::new(HashMap::from([
        (
            "conn_existing".to_string(),
            ClientConnectionInfo {
                client_id: "conn_existing".to_string(),
                session_id: target_session_id.to_string(),
                client_instance_id: None,
                debug_client_id: Some("debug_existing".to_string()),
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                disconnect_tx: mpsc::unbounded_channel().0,
            },
        ),
        (
            "conn_new".to_string(),
            ClientConnectionInfo {
                client_id: "conn_new".to_string(),
                session_id: temp_session_id.to_string(),
                client_instance_id: None,
                debug_client_id: Some("debug_new".to_string()),
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                disconnect_tx: mpsc::unbounded_channel().0,
            },
        ),
    ])));
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
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::<String, String>::new()));
    let client_count = Arc::new(RwLock::new(2usize));
    let (writer, _peer_stream) = test_writer()?;
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    let mut client_selfdev = false;
    let mut client_session_id = temp_session_id.to_string();

    handle_resume_session(
        42,
        target_session_id.to_string(),
        None,
        false,
        false,
        &mut client_selfdev,
        &mut client_session_id,
        "conn_new",
        &new_agent,
        &provider,
        &new_registry,
        &sessions,
        &shutdown_signals,
        &soft_interrupt_queues,
        &client_connections,
        &Arc::new(RwLock::new(ClientDebugState::default())),
        &swarm_members,
        &swarms_by_id,
        &file_touches,
        &files_touched_by_session,
        &channel_subscriptions,
        &channel_subscriptions_by_session,
        &swarm_plans,
        &swarm_coordinators,
        &client_count,
        &writer,
        "test-server",
        "🌿",
        &client_event_tx,
        &mcp_pool,
        &event_history,
        &event_counter,
        &swarm_event_tx,
    )
    .await?;

    let events = collect_events_until_done(&mut client_event_rx, 42).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 42)),
        "expected Done event for live attach, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ServerEvent::Error { .. })),
        "attach should not emit error events: {events:?}"
    );

    assert_eq!(client_session_id, target_session_id);
    let sessions_guard = sessions.read().await;
    let mapped_agent = sessions_guard
        .get(target_session_id)
        .ok_or_else(|| anyhow!("existing live session should remain mapped"))?;
    assert!(Arc::ptr_eq(mapped_agent, &existing_agent));
    assert!(!sessions_guard.contains_key(temp_session_id));
    drop(sessions_guard);

    let connections = client_connections.read().await;
    assert!(connections.contains_key("conn_existing"));
    assert_eq!(
        connections
            .get("conn_new")
            .map(|info| info.session_id.as_str()),
        Some(target_session_id)
    );

    restore_runtime_dir(prev_runtime);
    Ok(())
}
