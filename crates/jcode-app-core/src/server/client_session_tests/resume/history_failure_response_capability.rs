#[tokio::test]
async fn resume_history_send_failure_withdraws_source_response_capability() -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let _lock = crate::storage::lock_test_env();
    let root = tempfile::tempdir()?;
    struct RestoreEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for RestoreEnvironment {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                match value {
                    Some(value) => crate::env::set_var(key, value),
                    None => crate::env::remove_var(key),
                }
            }
            crate::config::Config::invalidate_cache();
        }
    }
    let _environment = RestoreEnvironment(
        [
            "JCODE_HOME",
            "JCODE_RUNTIME_DIR",
            "TMPDIR",
            "JCODE_TEST_SESSION",
            "JCODE_NO_TELEMETRY",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect(),
    );
    for (key, directory) in [
        ("JCODE_HOME", "home"),
        ("JCODE_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
    ] {
        let path = root.path().join(directory);
        std::fs::create_dir(&path)?;
        crate::env::set_var(key, path);
    }
    crate::env::set_var("JCODE_TEST_SESSION", "1");
    crate::env::set_var("JCODE_NO_TELEMETRY", "1");
    crate::config::Config::invalidate_cache();

    for live in [false, true] {
        let source_id = format!("history-failure-source-{live}");
        let target_id = format!("history-failure-target-{live}");
        let mut persisted = crate::session::Session::create_with_id(target_id.clone(), None, None);
        persisted.model = Some("mock".into());
        persisted.save()?;
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let source_registry = Registry::new(provider.clone()).await;
        let source_agent = Arc::new(Mutex::new(build_test_agent_with_id(
            provider.clone(),
            source_registry.clone(),
            &source_id,
            Vec::new(),
        )));
        source_agent
            .lock()
            .await
            .set_desktop_response_capability(true)
            .await;
        let mut agents = HashMap::from([(source_id.clone(), source_agent.clone())]);
        if live {
            let target_registry = Registry::new(provider.clone()).await;
            agents.insert(
                target_id.clone(),
                Arc::new(Mutex::new(build_test_agent_with_id(
                    provider.clone(),
                    target_registry,
                    &target_id,
                    Vec::new(),
                ))),
            );
        }
        let sessions = Arc::new(crate::server::session_manager::SessionManager::from_agents(
            agents,
        ));
        let now = Instant::now();
        let connections = Arc::new(RwLock::new(HashMap::from_iter(
            [("responder", true), ("viewer", false)].map(|(client_id, supports_user_response)| {
                (
                    client_id.to_owned(),
                    ClientConnectionInfo {
                        client_id: client_id.to_owned(),
                        session_id: source_id.clone(),
                        client_instance_id: None,
                        debug_client_id: None,
                        connected_at: now,
                        last_seen: now,
                        is_processing: false,
                        current_tool_name: None,
                        terminal_env: Vec::new(),
                        supports_user_response,
                        disconnect_tx: mpsc::unbounded_channel().0,
                    },
                )
            }),
        )));
        let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
        let soft_interrupt_queues = Arc::new(RwLock::new(HashMap::new()));
        let debug = Arc::new(RwLock::new(ClientDebugState::default()));
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));
        let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
        let file_touch = FileTouchService::new();
        let subscriptions = Arc::new(RwLock::new(HashMap::new()));
        let subscriptions_by_session = Arc::new(RwLock::new(HashMap::new()));
        let plans = Arc::new(RwLock::new(HashMap::new()));
        let coordinators = Arc::new(RwLock::new(HashMap::new()));
        let client_count = Arc::new(RwLock::new(2));
        let (writer, peer) = test_writer()?;
        let (read, _write) = peer.into_split();
        let mut reader = BufReader::new(read);
        let (events, _events_rx) = mpsc::unbounded_channel();
        let pool = Arc::new(crate::mcp::SharedMcpPool::new(Default::default()));
        let history = Arc::new(RwLock::new(VecDeque::new()));
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (swarm_events, _swarm_events_rx) = broadcast::channel(8);
        let mut client_selfdev = false;
        let mut client_session_id = source_id.clone();
        let mut connection_registry = source_registry.clone();
        let busy = source_agent.lock().await;
        let owners = Arc::strong_count(&source_agent);

        // History reads client_count before writing its frame. Let Todos succeed,
        // close the writer while History is behind this barrier, then release it.
        let count_guard = client_count.write().await;
        let fail_history = async {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).await? > 0);
            assert!(matches!(
                serde_json::from_str::<ServerEvent>(&line)?,
                ServerEvent::Todos { .. }
            ));
            writer.lock().await.shutdown().await?;
            drop(count_guard);
            Ok::<_, anyhow::Error>(())
        };
        let resume = handle_resume_session(
            2369,
            target_id.clone(),
            None,
            None,
            false,
            None,
            false,
            &mut client_selfdev,
            &mut client_session_id,
            "responder",
            &source_agent,
            &provider,
            &mut connection_registry,
            &sessions,
            &shutdown_signals,
            &soft_interrupt_queues,
            &connections,
            &debug,
            &swarm_members,
            &swarms_by_id,
            &file_touch,
            &subscriptions,
            &subscriptions_by_session,
            &plans,
            &coordinators,
            &client_count,
            &writer,
            "test-server",
            "test",
            &events,
            &pool,
            &history,
            &counter,
            &swarm_events,
            false,
            false,
        );
        let (result, injected) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(resume, fail_history)
        })
        .await?;
        injected?;
        assert!(result.is_err(), "closed writer must fail History delivery");
        assert_eq!(client_session_id, target_id);
        assert_eq!(connections.read().await["responder"].session_id, target_id);
        assert_eq!(connections.read().await["viewer"].session_id, source_id);
        assert_eq!(
            Arc::strong_count(&source_agent),
            owners,
            "refresh retained busy source Agent"
        );
        for name in ["user_response", "applet"] {
            assert!(
                !source_registry
                    .tool_names()
                    .await
                    .iter()
                    .any(|tool| tool == name),
                "remaining source viewers cannot answer {name}, live={live}"
            );
        }
        assert!(
            busy.response_capability_invalidator()
                .load(std::sync::atomic::Ordering::Acquire)
        );
        let target = sessions.read().await.get(&target_id).unwrap().clone();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while Arc::strong_count(&target) > 2 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(busy);
        sessions.clear_for_test().await;
    }
    Ok(())
}
