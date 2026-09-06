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
    source: &Arc<Mutex<Agent>>,
    provider: &Arc<dyn Provider>,
    registry: &Registry,
    sessions: &crate::server::SessionAgents,
) -> Result<Arc<Mutex<Agent>>> {
    let mut client_selfdev = false;
    let mut client_session_id = source.lock().await.session_id().to_owned();
    let (stream, _peer) = crate::transport::stream_pair()?;
    let (_, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));
    let (client_event_tx, _client_event_rx) = mpsc::unbounded_channel();
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
    handle_resume_session(
        1,
        target_id.to_owned(),
        None,
        None,
        false,
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
    )
    .await
}

#[tokio::test]
async fn failed_server_resume_keeps_original_concurrency_owner() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let _env = IsolatedConcurrencyEnv::new();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider.clone()).await;
    let source = Arc::new(Mutex::new(Agent::new(provider.clone(), registry.clone())));
    let source_id = source.lock().await.session_id().to_owned();
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        source_id.clone(),
        source.clone(),
    )])));
    let restored = restore_for_concurrency_test(
        "missing-concurrency-target",
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
    let sessions = Arc::new(RwLock::new(HashMap::from([
        (live_id.clone(), live.clone()),
        (placeholder_id, placeholder.clone()),
    ])));
    assert!(!placeholder.lock().await.has_concurrency_tracking());
    let attached =
        restore_for_concurrency_test(&live_id, &placeholder, &provider, &registry, &sessions)
            .await?;
    assert!(Arc::ptr_eq(&attached, &live));
    assert!(live.lock().await.has_concurrency_tracking());
    assert!(
        !placeholder.lock().await.has_concurrency_tracking(),
        "a viewer placeholder must never publish a join"
    );
    Ok(())
}
