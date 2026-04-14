    use super::{
        handle_clear_session, handle_reload, handle_resume_session, mark_remote_reload_started,
        rename_shutdown_signal, restored_session_was_interrupted,
        session_was_interrupted_by_reload,
    };
    use crate::agent::Agent;
    use crate::message::ContentBlock;
    use crate::message::{Message, ToolDefinition};
    use crate::protocol::ServerEvent;
    use crate::provider::{EventStream, Provider};
    use crate::server::{
        ClientConnectionInfo, ClientDebugState, FileAccess, SessionInterruptQueues, SwarmEvent,
        SwarmMember, VersionedPlan,
    };
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use jcode_agent_runtime::InterruptSignal;
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            unimplemented!("Mock provider")
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MockProvider)
        }
    }

    fn test_agent(messages: Vec<crate::session::StoredMessage>) -> Agent {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let registry = rt.block_on(Registry::new(provider.clone()));
        build_test_agent(provider, registry, messages)
    }

    fn build_test_agent(
        provider: Arc<dyn Provider>,
        registry: Registry,
        messages: Vec<crate::session::StoredMessage>,
    ) -> Agent {
        let mut session =
            crate::session::Session::create_with_id("session_test_reload".to_string(), None, None);
        session.model = Some("mock".to_string());
        session.replace_messages(messages);
        Agent::new_with_session(provider, registry, session, None)
    }

    fn build_test_agent_with_id(
        provider: Arc<dyn Provider>,
        registry: Registry,
        session_id: &str,
        messages: Vec<crate::session::StoredMessage>,
    ) -> Agent {
        let mut session =
            crate::session::Session::create_with_id(session_id.to_string(), None, None);
        session.model = Some("mock".to_string());
        session.replace_messages(messages);
        Agent::new_with_session(provider, registry, session, None)
    }

    async fn collect_events_until_done(
        client_event_rx: &mut mpsc::UnboundedReceiver<ServerEvent>,
        done_id: u64,
    ) -> Vec<ServerEvent> {
        let mut events = Vec::new();
        for _ in 0..16 {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(1), client_event_rx.recv())
                    .await
                    .expect("timed out waiting for server event")
                    .expect("expected server event");
            let is_done = matches!(event, ServerEvent::Done { id } if id == done_id);
            events.push(event);
            if is_done {
                break;
            }
        }
        events
    }

    #[test]
    fn detects_reload_interrupted_generation_text() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_1".to_string(),
            role: crate::message::Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "partial\n\n[generation interrupted - server reloading]".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(session_was_interrupted_by_reload(&agent));
    }

    #[test]
    fn detects_reload_interrupted_tool_result() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_2".to_string(),
            role: crate::message::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "[Tool 'bash' interrupted by server reload after 0.2s]".to_string(),
                is_error: Some(true),
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(session_was_interrupted_by_reload(&agent));
    }

    #[test]
    fn detects_reload_skipped_tool_result() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_3".to_string(),
            role: crate::message::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_2".to_string(),
                content: "[Skipped - server reloading]".to_string(),
                is_error: Some(true),
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(session_was_interrupted_by_reload(&agent));
    }

    #[test]
    fn ignores_normal_tool_errors() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_4".to_string(),
            role: crate::message::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_3".to_string(),
                content: "Error: file not found".to_string(),
                is_error: Some(true),
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(!session_was_interrupted_by_reload(&agent));
    }

    #[test]
    fn restored_closed_session_with_reload_marker_still_counts_as_interrupted() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_5".to_string(),
            role: crate::message::Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "partial\n\n[generation interrupted - server reloading]".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(restored_session_was_interrupted(
            "session_test_reload",
            &crate::session::SessionStatus::Closed,
            &agent,
        ));
    }

    #[test]
    fn restored_closed_session_without_reload_marker_is_not_interrupted() {
        let agent = test_agent(vec![crate::session::StoredMessage {
            id: "msg_6".to_string(),
            role: crate::message::Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "finished normally".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }]);

        assert!(!restored_session_was_interrupted(
            "session_test_reload",
            &crate::session::SessionStatus::Closed,
            &agent,
        ));
    }

    #[test]
    fn mark_remote_reload_started_writes_starting_marker() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

        mark_remote_reload_started("reload-test");

        let state = crate::server::recent_reload_state(std::time::Duration::from_secs(5))
            .expect("reload state should exist");
        assert_eq!(state.request_id, "reload-test");
        assert_eq!(state.phase, crate::server::ReloadPhase::Starting);

        crate::server::clear_reload_marker();
        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[test]
    fn handle_reload_queues_signal_for_canary_session() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async {
            let mut rx = crate::server::subscribe_reload_signal_for_tests();
            let provider: Arc<dyn Provider> = Arc::new(MockProvider);
            let registry = Registry::new(provider.clone()).await;
            let mut agent = build_test_agent(provider, registry, Vec::new());
            agent.set_canary("self-dev");
            let agent = Arc::new(Mutex::new(agent));
            let (tx, mut events) = mpsc::unbounded_channel::<ServerEvent>();
            let (peer_tx, mut peer_events) = mpsc::unbounded_channel::<ServerEvent>();
            let now = Instant::now();
            let swarm_members = Arc::new(RwLock::new(HashMap::from([
                (
                    "session_test_reload".to_string(),
                    SwarmMember {
                        session_id: "session_test_reload".to_string(),
                        event_tx: tx.clone(),
                        event_txs: HashMap::from([("conn-trigger".to_string(), tx.clone())]),
                        working_dir: None,
                        swarm_id: None,
                        swarm_enabled: false,
                        status: "ready".to_string(),
                        detail: None,
                        friendly_name: Some("trigger".to_string()),
                        role: "agent".to_string(),
                        joined_at: now,
                        last_status_change: now,
                        is_headless: false,
                    },
                ),
                (
                    "session_peer".to_string(),
                    SwarmMember {
                        session_id: "session_peer".to_string(),
                        event_tx: peer_tx.clone(),
                        event_txs: HashMap::from([("conn-peer".to_string(), peer_tx.clone())]),
                        working_dir: None,
                        swarm_id: None,
                        swarm_enabled: false,
                        status: "ready".to_string(),
                        detail: None,
                        friendly_name: Some("peer".to_string()),
                        role: "agent".to_string(),
                        joined_at: now,
                        last_status_change: now,
                        is_headless: false,
                    },
                ),
            ])));

            handle_reload(7, &agent, &swarm_members, &tx).await;

            let reloading = events.recv().await.expect("reloading event");
            assert!(matches!(reloading, ServerEvent::Reloading { .. }));
            let peer_reloading = peer_events.recv().await.expect("peer reloading event");
            assert!(matches!(peer_reloading, ServerEvent::Reloading { .. }));
            let done = events.recv().await.expect("done event");
            assert!(matches!(done, ServerEvent::Done { id: 7 }));

            tokio::time::timeout(std::time::Duration::from_secs(1), rx.changed())
                .await
                .expect("reload signal timeout")
                .expect("reload signal should be delivered");
            let signal = rx
                .borrow_and_update()
                .clone()
                .expect("reload signal payload should exist");
            assert_eq!(
                signal.triggering_session.as_deref(),
                Some("session_test_reload")
            );
            assert!(signal.prefer_selfdev_binary);
            assert_eq!(signal.hash, env!("JCODE_GIT_HASH"));

            let state = crate::server::recent_reload_state(std::time::Duration::from_secs(5))
                .expect("reload state should exist");
            assert_eq!(state.phase, crate::server::ReloadPhase::Starting);
        });

        crate::server::clear_reload_marker();
        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[tokio::test]
    async fn rename_shutdown_signal_moves_registration_to_restored_session() {
        let signal = InterruptSignal::new();
        let shutdown_signals = Arc::new(RwLock::new(HashMap::from([(
            "session_old".to_string(),
            signal.clone(),
        )])));

        rename_shutdown_signal(&shutdown_signals, "session_old", "session_restored").await;

        let signals = shutdown_signals.read().await;
        assert!(!signals.contains_key("session_old"));
        let renamed = signals
            .get("session_restored")
            .expect("restored session should retain shutdown signal");
        renamed.fire();
        assert!(signal.is_set());
    }

    #[tokio::test]
    async fn handle_clear_session_replaces_runtime_handles_and_updates_shutdown_registration() {
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
        let soft_interrupt_queues: SessionInterruptQueues =
            Arc::new(RwLock::new(HashMap::from([(
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
            .expect("old queue lock")
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
            .expect("new session should have shutdown signal")
            .clone();
        drop(signals);
        registered_signal.fire();
        assert!(new_cancel_signal.is_set());

        let first = client_event_rx.recv().await.expect("session id event");
        assert!(matches!(first, ServerEvent::SessionId { .. }));
        let second = client_event_rx.recv().await.expect("done event");
        assert!(matches!(second, ServerEvent::Done { id: 7 }));
    }

    #[tokio::test]
    async fn handle_resume_session_allows_multiple_live_tui_attach() {
        let _guard = crate::storage::lock_test_env();
        let runtime = tempfile::TempDir::new().expect("create runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());

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
        let (stream_a, _stream_b) = crate::transport::stream_pair().expect("stream pair");
        let (_reader, writer_half) = stream_a.into_split();
        let writer = Arc::new(Mutex::new(writer_half));
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
        .await
        .expect("resume attach should succeed");

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
            .expect("existing live session should remain mapped");
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

        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[tokio::test]
    async fn handle_resume_session_allows_reconnect_takeover_with_local_history() {
        let _guard = crate::storage::lock_test_env();
        let runtime = tempfile::TempDir::new().expect("create runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());

        let target_session_id = "session_existing_live_takeover";
        let temp_session_id = "session_temp_connecting_takeover";

        let mut persisted = crate::session::Session::create_with_id(
            target_session_id.to_string(),
            None,
            Some("Reconnect Takeover".to_string()),
        );
        persisted
            .save()
            .expect("persist reconnect takeover session");

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
        let (stream_a, _stream_b) = crate::transport::stream_pair().expect("stream pair");
        let (_reader, writer_half) = stream_a.into_split();
        let writer = Arc::new(Mutex::new(writer_half));
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
        .await
        .expect("takeover resume should succeed");

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

        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[tokio::test]
    async fn handle_resume_session_allows_attach_without_local_history() {
        let _guard = crate::storage::lock_test_env();
        let runtime = tempfile::TempDir::new().expect("create runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());

        let target_session_id = "session_existing_live_takeover_rejected";
        let temp_session_id = "session_temp_connecting_takeover_rejected";

        let mut persisted = crate::session::Session::create_with_id(
            target_session_id.to_string(),
            None,
            Some("Reconnect Takeover Rejected".to_string()),
        );
        persisted
            .save()
            .expect("persist reconnect takeover rejected session");

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
        let (stream_a, _stream_b) = crate::transport::stream_pair().expect("stream pair");
        let (_reader, writer_half) = stream_a.into_split();
        let writer = Arc::new(Mutex::new(writer_half));
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
        .await
        .expect("attach without local history should succeed");

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
                .expect("existing live session should remain mapped"),
            &existing_agent
        ));
        assert!(!sessions_guard.contains_key(temp_session_id));

        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[tokio::test]
    async fn handle_resume_session_allows_attach_from_different_client_instance() {
        let _guard = crate::storage::lock_test_env();
        let runtime = tempfile::TempDir::new().expect("create runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());

        let target_session_id = "session_existing_live_local_history_rejected";
        let temp_session_id = "session_temp_connecting_local_history_rejected";

        let mut persisted = crate::session::Session::create_with_id(
            target_session_id.to_string(),
            None,
            Some("Reconnect Local History Rejected".to_string()),
        );
        persisted
            .save()
            .expect("persist reconnect local-history rejected session");

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
        let (stream_a, _stream_b) = crate::transport::stream_pair().expect("stream pair");
        let (_reader, writer_half) = stream_a.into_split();
        let writer = Arc::new(Mutex::new(writer_half));
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
        .await
        .expect("different-instance attach should succeed");

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
                .expect("existing live session should remain mapped"),
            &existing_agent
        ));
        assert!(!sessions_guard.contains_key(temp_session_id));

        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[tokio::test]
    async fn handle_resume_session_registers_live_events_before_history_replay() {
        let _guard = crate::storage::lock_test_env();
        let runtime = tempfile::TempDir::new().expect("create runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());

        let target_session_id = "session_restore_target";
        let temp_session_id = "session_restore_temp";

        let mut persisted = crate::session::Session::create_with_id(
            target_session_id.to_string(),
            None,
            Some("Resume Registration Ordering".to_string()),
        );
        persisted
            .save()
            .expect("persist resume registration ordering session");

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
        let (placeholder_event_tx, _placeholder_event_rx) =
            mpsc::unbounded_channel::<ServerEvent>();
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
        let (stream_a, _stream_b) = crate::transport::stream_pair().expect("stream pair");
        let (_reader, writer_half) = stream_a.into_split();
        let writer = Arc::new(Mutex::new(writer_half));
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
        .expect("live event sender should register before history replay completes");

        assert!(
            !resume_task.is_finished(),
            "resume should still be blocked on history replay while writer is locked"
        );

        drop(writer_guard);

        resume_task
            .await
            .expect("resume task join")
            .expect("restore resume should succeed");

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

        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }

    #[tokio::test]
    async fn handle_resume_session_allows_same_client_instance_takeover_without_local_history() {
        let _guard = crate::storage::lock_test_env();
        let runtime = tempfile::TempDir::new().expect("create runtime dir");
        let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
        crate::env::set_var("JCODE_RUNTIME_DIR", runtime.path());

        let target_session_id = "session_existing_live_same_instance_takeover";
        let temp_session_id = "session_temp_connecting_same_instance_takeover";
        let shared_instance_id = "client_instance_same_window";

        let mut persisted = crate::session::Session::create_with_id(
            target_session_id.to_string(),
            None,
            Some("Reconnect Same Instance Takeover".to_string()),
        );
        persisted
            .save()
            .expect("persist reconnect same-instance session");

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
        let (stream_a, _stream_b) = crate::transport::stream_pair().expect("stream pair");
        let (_reader, writer_half) = stream_a.into_split();
        let writer = Arc::new(Mutex::new(writer_half));
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
        .await
        .expect("same-instance attach should succeed");

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
                .expect("existing live session should remain mapped"),
            &existing_agent
        ));
        assert!(!sessions_guard.contains_key(temp_session_id));

        if let Some(prev_runtime) = prev_runtime {
            crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
        } else {
            crate::env::remove_var("JCODE_RUNTIME_DIR");
        }
    }