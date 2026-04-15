use super::*;
use crate::transport::WriteHalf;
use anyhow::{Result, anyhow};

fn setup_runtime_dir() -> Result<(tempfile::TempDir, Option<std::ffi::OsString>)> {
    let runtime = tempfile::TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());
    Ok((runtime, prev_runtime))
}

fn restore_runtime_dir(prev_runtime: Option<std::ffi::OsString>) {
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

fn test_writer() -> Result<(Arc<Mutex<WriteHalf>>, crate::transport::Stream)> {
    let (stream_a, stream_b) = crate::transport::stream_pair().map_err(|e| anyhow!(e))?;
    let (_reader, writer_half) = stream_a.into_split();
    Ok((Arc::new(Mutex::new(writer_half)), stream_b))
}

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

#[tokio::test]
async fn handle_resume_session_allows_reconnect_takeover_with_local_history() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;

    let target_session_id = "session_existing_live_takeover";
    let temp_session_id = "session_temp_connecting_takeover";

    let mut persisted = crate::session::Session::create_with_id(
        target_session_id.to_string(),
        None,
        Some("Reconnect Takeover".to_string()),
    );
    persisted.save()?;

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
    let (disconnect_tx, mut disconnect_rx) = mpsc::unbounded_channel();
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
                disconnect_tx,
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
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
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
        43,
        target_session_id.to_string(),
        None,
        true,
        true,
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
        &client_debug_state,
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

    while let Ok(event) = client_event_rx.try_recv() {
        assert!(
            !matches!(event, ServerEvent::Error { .. }),
            "resume takeover should not queue an error event: {event:?}"
        );
    }
    assert_eq!(client_session_id, target_session_id);

    let disconnect_signal = disconnect_rx.recv().await;
    assert!(
        disconnect_signal.is_some(),
        "old client should be told to disconnect"
    );

    let connections = client_connections.read().await;
    assert!(!connections.contains_key("conn_existing"));
    assert_eq!(
        connections
            .get("conn_new")
            .map(|info| info.session_id.as_str()),
        Some(target_session_id)
    );

    restore_runtime_dir(prev_runtime);
    Ok(())
}

#[tokio::test]
async fn handle_resume_session_allows_attach_without_local_history() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;

    let target_session_id = "session_existing_live_takeover_rejected";
    let temp_session_id = "session_temp_connecting_takeover_rejected";

    let mut persisted = crate::session::Session::create_with_id(
        target_session_id.to_string(),
        None,
        Some("Reconnect Takeover Rejected".to_string()),
    );
    persisted.save()?;

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
    let (disconnect_tx, mut disconnect_rx) = mpsc::unbounded_channel();
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
                disconnect_tx,
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
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
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
        44,
        target_session_id.to_string(),
        None,
        false,
        true,
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
        &client_debug_state,
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

    let events = collect_events_until_done(&mut client_event_rx, 44).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 44)),
        "expected Done event for live attach, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ServerEvent::Error { .. })),
        "attach should not emit error events: {events:?}"
    );

    assert_eq!(client_session_id, target_session_id);
    assert!(
        disconnect_rx.try_recv().is_err(),
        "existing live client must not be kicked"
    );
    let connections = client_connections.read().await;
    assert!(connections.contains_key("conn_existing"));
    assert_eq!(
        connections
            .get("conn_new")
            .map(|info| info.session_id.as_str()),
        Some(target_session_id)
    );
    drop(connections);
    let sessions_guard = sessions.read().await;
    assert!(Arc::ptr_eq(
        sessions_guard
            .get(target_session_id)
            .ok_or_else(|| anyhow!("existing live session should remain mapped"))?,
        &existing_agent
    ));
    assert!(!sessions_guard.contains_key(temp_session_id));

    restore_runtime_dir(prev_runtime);
    Ok(())
}

#[tokio::test]
async fn handle_resume_session_allows_attach_from_different_client_instance() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;

    let target_session_id = "session_existing_live_local_history_rejected";
    let temp_session_id = "session_temp_connecting_local_history_rejected";

    let mut persisted = crate::session::Session::create_with_id(
        target_session_id.to_string(),
        None,
        Some("Reconnect Local History Rejected".to_string()),
    );
    persisted.save()?;

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
    let (disconnect_tx, mut disconnect_rx) = mpsc::unbounded_channel();
    let client_connections = Arc::new(RwLock::new(HashMap::from([
        (
            "conn_existing".to_string(),
            ClientConnectionInfo {
                client_id: "conn_existing".to_string(),
                session_id: target_session_id.to_string(),
                client_instance_id: Some("client_instance_existing".to_string()),
                debug_client_id: Some("debug_existing".to_string()),
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                disconnect_tx,
            },
        ),
        (
            "conn_new".to_string(),
            ClientConnectionInfo {
                client_id: "conn_new".to_string(),
                session_id: temp_session_id.to_string(),
                client_instance_id: Some("client_instance_new".to_string()),
                debug_client_id: Some("debug_new".to_string()),
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                disconnect_tx: mpsc::unbounded_channel().0,
            },
        ),
    ])));
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
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
        45,
        target_session_id.to_string(),
        Some("client_instance_new"),
        true,
        true,
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
        &client_debug_state,
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

    let events = collect_events_until_done(&mut client_event_rx, 45).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 45)),
        "expected Done event for live attach, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ServerEvent::Error { .. })),
        "attach should not emit error events: {events:?}"
    );

    assert_eq!(client_session_id, target_session_id);
    assert!(
        disconnect_rx.try_recv().is_err(),
        "existing live client must not be kicked"
    );
    let connections = client_connections.read().await;
    assert!(connections.contains_key("conn_existing"));
    assert_eq!(
        connections
            .get("conn_new")
            .map(|info| (info.session_id.as_str(), info.client_instance_id.as_deref())),
        Some((target_session_id, Some("client_instance_new")))
    );
    drop(connections);
    let sessions_guard = sessions.read().await;
    assert!(Arc::ptr_eq(
        sessions_guard
            .get(target_session_id)
            .ok_or_else(|| anyhow!("existing live session should remain mapped"))?,
        &existing_agent
    ));
    assert!(!sessions_guard.contains_key(temp_session_id));

    restore_runtime_dir(prev_runtime);
    Ok(())
}

#[tokio::test]
async fn handle_resume_session_registers_live_events_before_history_replay() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;

    let target_session_id = "session_restore_target";
    let temp_session_id = "session_restore_temp";

    let mut persisted = crate::session::Session::create_with_id(
        target_session_id.to_string(),
        None,
        Some("Resume Registration Ordering".to_string()),
    );
    persisted.save()?;

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(build_test_agent_with_id(
        provider.clone(),
        registry.clone(),
        temp_session_id,
        Vec::new(),
    )));

    let sessions = Arc::new(RwLock::new(HashMap::from([(
        temp_session_id.to_string(),
        Arc::clone(&agent),
    )])));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::<String, InterruptSignal>::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let now = Instant::now();
    let client_connections = Arc::new(RwLock::new(HashMap::from([(
        "conn_restore".to_string(),
        ClientConnectionInfo {
            client_id: "conn_restore".to_string(),
            session_id: temp_session_id.to_string(),
            client_instance_id: None,
            debug_client_id: Some("debug_restore".to_string()),
            connected_at: now,
            last_seen: now,
            is_processing: false,
            current_tool_name: None,
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
    let (placeholder_event_tx, _placeholder_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let swarm_members = Arc::new(RwLock::new(HashMap::from([(
        temp_session_id.to_string(),
        SwarmMember {
            session_id: temp_session_id.to_string(),
            event_tx: placeholder_event_tx,
            event_txs: HashMap::new(),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            friendly_name: Some("restore".to_string()),
            report_back_to_session_id: None,
            role: "agent".to_string(),
            joined_at: now,
            last_status_change: now,
            is_headless: false,
        },
    )])));
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
    let client_count = Arc::new(RwLock::new(1usize));
    let (writer, _peer_stream) = test_writer()?;
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel::<ServerEvent>();
    let event_history = Arc::new(RwLock::new(VecDeque::<SwarmEvent>::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel::<SwarmEvent>(8);
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    let mut client_selfdev = false;
    let mut client_session_id = temp_session_id.to_string();
    let writer_guard = writer.lock().await;

    let resume_task = tokio::spawn({
        let agent = Arc::clone(&agent);
        let provider = Arc::clone(&provider);
        let registry = registry.clone();
        let sessions = Arc::clone(&sessions);
        let shutdown_signals = Arc::clone(&shutdown_signals);
        let soft_interrupt_queues = Arc::clone(&soft_interrupt_queues);
        let client_connections = Arc::clone(&client_connections);
        let client_debug_state = Arc::clone(&client_debug_state);
        let swarm_members = Arc::clone(&swarm_members);
        let swarms_by_id = Arc::clone(&swarms_by_id);
        let file_touches = Arc::clone(&file_touches);
        let files_touched_by_session = Arc::clone(&files_touched_by_session);
        let channel_subscriptions = Arc::clone(&channel_subscriptions);
        let channel_subscriptions_by_session = Arc::clone(&channel_subscriptions_by_session);
        let swarm_plans = Arc::clone(&swarm_plans);
        let swarm_coordinators = Arc::clone(&swarm_coordinators);
        let client_count = Arc::clone(&client_count);
        let writer = Arc::clone(&writer);
        let client_event_tx = client_event_tx.clone();
        let mcp_pool = Arc::clone(&mcp_pool);
        let event_history = Arc::clone(&event_history);
        let event_counter = Arc::clone(&event_counter);
        let swarm_event_tx = swarm_event_tx.clone();
        async move {
            handle_resume_session(
                46,
                target_session_id.to_string(),
                None,
                false,
                false,
                &mut client_selfdev,
                &mut client_session_id,
                "conn_restore",
                &agent,
                &provider,
                &registry,
                &sessions,
                &shutdown_signals,
                &soft_interrupt_queues,
                &client_connections,
                &client_debug_state,
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
            .await
        }
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let registered = {
                let members = swarm_members.read().await;
                members
                    .get(target_session_id)
                    .map(|member| member.event_txs.contains_key("conn_restore"))
                    .unwrap_or(false)
            };
            if registered {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| anyhow!("live event sender should register before history replay completes"))?;

    assert!(
        !resume_task.is_finished(),
        "resume should still be blocked on history replay while writer is locked"
    );

    drop(writer_guard);

    resume_task
        .await
        .map_err(|e| anyhow!("resume task join: {e}"))??;

    let events = collect_events_until_done(&mut client_event_rx, 46).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 46)),
        "expected Done event for restore resume, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ServerEvent::Error { .. })),
        "restore resume should not emit error events: {events:?}"
    );

    restore_runtime_dir(prev_runtime);
    Ok(())
}

#[tokio::test]
async fn handle_resume_session_allows_same_client_instance_takeover_without_local_history()
-> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let (_runtime, prev_runtime) = setup_runtime_dir()?;

    let target_session_id = "session_existing_live_same_instance_takeover";
    let temp_session_id = "session_temp_connecting_same_instance_takeover";
    let shared_instance_id = "client_instance_same_window";

    let mut persisted = crate::session::Session::create_with_id(
        target_session_id.to_string(),
        None,
        Some("Reconnect Same Instance Takeover".to_string()),
    );
    persisted.save()?;

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
    let (disconnect_tx, mut disconnect_rx) = mpsc::unbounded_channel();
    let client_connections = Arc::new(RwLock::new(HashMap::from([
        (
            "conn_existing".to_string(),
            ClientConnectionInfo {
                client_id: "conn_existing".to_string(),
                session_id: target_session_id.to_string(),
                client_instance_id: Some(shared_instance_id.to_string()),
                debug_client_id: Some("debug_existing".to_string()),
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                disconnect_tx,
            },
        ),
        (
            "conn_new".to_string(),
            ClientConnectionInfo {
                client_id: "conn_new".to_string(),
                session_id: temp_session_id.to_string(),
                client_instance_id: Some(shared_instance_id.to_string()),
                debug_client_id: Some("debug_new".to_string()),
                connected_at: now,
                last_seen: now,
                is_processing: false,
                current_tool_name: None,
                disconnect_tx: mpsc::unbounded_channel().0,
            },
        ),
    ])));
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
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
        45,
        target_session_id.to_string(),
        Some(shared_instance_id),
        false,
        true,
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
        &client_debug_state,
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

    let events = collect_events_until_done(&mut client_event_rx, 45).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ServerEvent::Done { id } if *id == 45)),
        "expected Done event for live attach, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ServerEvent::Error { .. })),
        "same-instance attach should not queue an error event: {events:?}"
    );
    assert_eq!(client_session_id, target_session_id);

    assert!(
        disconnect_rx.try_recv().is_err(),
        "existing live client should remain connected"
    );

    let connections = client_connections.read().await;
    assert!(connections.contains_key("conn_existing"));
    assert_eq!(
        connections
            .get("conn_new")
            .map(|info| (info.session_id.as_str(), info.client_instance_id.as_deref())),
        Some((target_session_id, Some(shared_instance_id)))
    );
    drop(connections);
    let sessions_guard = sessions.read().await;
    assert!(Arc::ptr_eq(
        sessions_guard
            .get(target_session_id)
            .ok_or_else(|| anyhow!("existing live session should remain mapped"))?,
        &existing_agent
    ));
    assert!(!sessions_guard.contains_key(temp_session_id));

    restore_runtime_dir(prev_runtime);
    Ok(())
}
