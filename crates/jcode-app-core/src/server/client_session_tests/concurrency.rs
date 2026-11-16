use super::*;

struct IsolatedConcurrencyEnv {
    _home: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl IsolatedConcurrencyEnv {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let previous = ["JCODE_HOME", "JCODE_NO_TELEMETRY"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        crate::env::set_var("JCODE_HOME", home.path());
        crate::env::set_var("JCODE_NO_TELEMETRY", "1");
        Self {
            _home: home,
            previous,
        }
    }
}

impl Drop for IsolatedConcurrencyEnv {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

async fn restore_for_concurrency_test(
    target_id: &str,
    source_id: &str,
    source: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &crate::server::SessionAgents,
) -> Result<Arc<Mutex<Agent>>> {
    let mut client_selfdev = false;
    let mut client_session_id = source_id.to_owned();
    let (stream, _peer) = crate::transport::stream_pair()?;
    let (_, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let (client_event_tx, mut client_event_rx) = mpsc::unbounded_channel();
    let (swarm_event_tx, _swarm_event_rx) = broadcast::channel(8);
    let now = Instant::now();
    let connections = Arc::new(RwLock::new(HashMap::from([(
        "concurrency-test-connection".to_owned(),
        ClientConnectionInfo {
            client_id: "concurrency-test-connection".to_owned(),
            session_id: client_session_id.clone(),
            client_instance_id: None,
            debug_client_id: None,
            connected_at: now,
            last_seen: now,
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));
    let result = handle_resume_session(
        1,
        target_id.to_owned(),
        None,
        None,
        false,
        None,
        false,
        &mut client_selfdev,
        &mut client_session_id,
        "concurrency-test-connection",
        source,
        provider,
        registry,
        sessions,
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &connections,
        &Arc::new(RwLock::new(ClientDebugState::default())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &FileTouchService::new(),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(HashMap::new())),
        &Arc::new(RwLock::new(1)),
        &writer,
        "test-server",
        "test",
        &client_event_tx,
        &Arc::new(crate::mcp::SharedMcpPool::from_default_config()),
        &Arc::new(RwLock::new(VecDeque::new())),
        &Arc::new(std::sync::atomic::AtomicU64::new(0)),
        &swarm_event_tx,
        false,
        false,
    )
    .await;
    while let Ok(event) = client_event_rx.try_recv() {
        if let ServerEvent::Error { message, .. } = event {
            assert_eq!(
                target_id, "missing-concurrency-target",
                "resume failed: {message}"
            );
        }
    }
    result
}

#[tokio::test]
async fn failed_server_resume_keeps_original_concurrency_owner() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedConcurrencyEnv::new();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let source = Arc::new(Mutex::new(Agent::new(provider.clone(), registry.clone())));
    let source_id = source.lock().await.session_id().to_owned();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::from([(source_id.clone(), source.clone())]),
    ));
    let restored = restore_for_concurrency_test(
        "missing-concurrency-target",
        &source_id,
        &source,
        &provider,
        &registry,
        &sessions,
    )
    .await?;
    assert!(Arc::ptr_eq(&restored, &source));
    let source = source.lock().await;
    assert_eq!(source.session_id(), source_id);
    assert!(
        source.has_concurrency_tracking(),
        "failed resume must not close the original logical owner"
    );
    Ok(())
}

#[tokio::test]
async fn viewer_attach_reuses_live_owner_without_tracking_placeholder() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedConcurrencyEnv::new();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let live = Arc::new(Mutex::new(Agent::new(provider.clone(), registry.clone())));
    let live_id = live.lock().await.session_id().to_owned();
    let placeholder = Arc::new(Mutex::new(Agent::new_provisional_with_initial_working_dir(
        provider.clone(),
        registry.clone(),
        None,
    )));
    let placeholder_id = placeholder.lock().await.session_id().to_owned();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::from([
            (live_id.clone(), live.clone()),
            (placeholder_id.clone(), placeholder.clone()),
        ]),
    ));
    assert!(!placeholder.lock().await.has_concurrency_tracking());
    let attached = restore_for_concurrency_test(
        &live_id,
        &placeholder_id,
        &placeholder,
        &provider,
        &registry,
        &sessions,
    )
    .await?;
    assert!(Arc::ptr_eq(&attached, &live));
    assert!(live.lock().await.has_concurrency_tracking());
    assert!(
        !placeholder.lock().await.has_concurrency_tracking(),
        "a viewer placeholder must never publish a join"
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_cold_resumes_share_one_restored_runtime() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedConcurrencyEnv::new();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut persisted = crate::session::Session::create(None, Some("Cold resume race".into()));
    persisted.save()?;
    let target_id = persisted.id.clone();
    let first = Arc::new(Mutex::new(Agent::new_provisional_with_initial_working_dir(
        provider.clone(),
        registry.clone(),
        None,
    )));
    let second = Arc::new(Mutex::new(Agent::new_provisional_with_initial_working_dir(
        provider.clone(),
        registry.clone(),
        None,
    )));
    let first_id = first.lock().await.session_id().to_owned();
    let second_id = second.lock().await.session_id().to_owned();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::from([
            (first_id.clone(), first.clone()),
            (second_id.clone(), second.clone()),
        ]),
    ));
    // Force both requests to enter before either can restore. Without the
    // target gate each passes live lookup and waits on its own source mutex.
    let first_guard = first.lock().await;
    let second_guard = second.lock().await;
    let first_resume = restore_for_concurrency_test(
        &target_id, &first_id, &first, &provider, &registry, &sessions,
    );
    let second_resume = restore_for_concurrency_test(
        &target_id, &second_id, &second, &provider, &registry, &sessions,
    );
    tokio::pin!(first_resume, second_resume);
    assert!(futures::poll!(&mut first_resume).is_pending());
    assert!(futures::poll!(&mut second_resume).is_pending());
    drop(first_guard);
    drop(second_guard);
    let (first_result, second_result) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(first_resume, second_resume)
        })
        .await
        .expect("concurrent resumes must not deadlock");
    let (first_result, second_result) = (first_result?, second_result?);
    assert!(Arc::ptr_eq(&first_result, &second_result));
    assert!(Arc::ptr_eq(
        &first_result,
        sessions.read().await.get(&target_id).unwrap()
    ));
    assert!(!Arc::ptr_eq(&first_result, &first));
    assert!(!Arc::ptr_eq(&first_result, &second));
    assert!(first_result.lock().await.has_concurrency_tracking());
    let first = first.lock().await;
    let second = second.lock().await;
    assert_eq!(first.session_id(), first_id);
    assert_eq!(second.session_id(), second_id);
    assert!(!first.has_concurrency_tracking());
    assert!(!second.has_concurrency_tracking());
    Ok(())
}

#[tokio::test]
async fn same_session_reattach_preserves_runtime_owner() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedConcurrencyEnv::new();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new(provider.clone(), registry.clone())));
    let id = agent.lock().await.session_id().to_owned();
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::from([(id.clone(), agent.clone())]),
    ));
    let attached = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        restore_for_concurrency_test(&id, &id, &agent, &provider, &registry, &sessions),
    )
    .await
    .expect("same-session reattach must not deadlock")?;
    assert!(Arc::ptr_eq(&attached, &agent));
    assert!(Arc::ptr_eq(sessions.read().await.get(&id).unwrap(), &agent));
    assert!(agent.lock().await.has_concurrency_tracking());
    Ok(())
}

#[tokio::test]
async fn resume_reservations_are_scoped_and_release_on_drop() {
    let sessions = crate::server::session_manager::SessionManager::new();
    let other_sessions = crate::server::session_manager::SessionManager::new();
    let held = sessions.reserve("target").await;
    let same = sessions.reserve("target");
    tokio::pin!(same);
    assert!(futures::poll!(&mut same).is_pending());
    let other_target = sessions.reserve("other");
    tokio::pin!(other_target);
    assert!(futures::poll!(&mut other_target).is_ready());
    let other_manager = other_sessions.reserve("target");
    tokio::pin!(other_manager);
    assert!(futures::poll!(&mut other_manager).is_ready());
    drop(held);
    assert!(
        futures::poll!(&mut same).is_ready(),
        "dropping a reservation must release its gate"
    );
}

#[tokio::test]
async fn comm_status_releases_member_guard_before_reading_connections() {
    let swarm_members = Arc::new(RwLock::new(HashMap::from([
        ("caller".to_string(), test_swarm_member("caller", "ready")),
        ("target".to_string(), test_swarm_member("target", "ready")),
    ])));
    let client_connections = Arc::new(RwLock::new(HashMap::<String, ClientConnectionInfo>::new()));
    let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
        HashMap::new(),
    ));
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();

    // Disconnect cleanup holds client_connections.write, then needs swarm_members.write.
    let cleanup_connections = client_connections.write().await;
    let status = tokio::spawn({
        let (swarm_members, client_connections) =
            (Arc::clone(&swarm_members), Arc::clone(&client_connections));
        async move {
            crate::server::comm_sync::handle_comm_status(
                1,
                "caller".to_string(),
                "target".to_string(),
                &sessions,
                &swarm_members,
                &client_connections,
                &FileTouchService::new(),
                &event_tx,
            )
            .await;
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let _members = tokio::time::timeout(std::time::Duration::from_secs(2), swarm_members.write())
        .await
        .expect("comm status must not hold swarm_members while waiting on client_connections");
    drop(_members);
    drop(cleanup_connections);
    status.await.unwrap();
    assert!(matches!(
        event_rx.recv().await,
        Some(ServerEvent::CommStatusResponse { .. })
    ));
}
