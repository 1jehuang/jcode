use super::*;

#[tokio::test]
async fn decision_channel_wiring_follows_supports_decisions_capability() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    let provider_template: Arc<dyn Provider> = Arc::new(SdkCallbackProvider::default());

    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::new()));
    let global_session_id = Arc::new(RwLock::new(String::new()));
    let client_count = Arc::new(RwLock::new(0usize));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let shared_context = Arc::new(RwLock::new(HashMap::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let file_touch = FileTouchService::new();
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::new()));
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
    let (_debug_response_tx, _) = broadcast::channel(8);
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let (_global_event_tx, _) = broadcast::channel(8);
    let global_is_processing = Arc::new(RwLock::new(false));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    let connect = || {
        let (server_stream, client_stream) = crate::transport::Stream::pair().expect("socket pair");
        let server_task = tokio::spawn(handle_client(
            server_stream,
            Arc::clone(&sessions),
            _global_event_tx.clone(),
            provider_template.clone(),
            global_is_processing.clone(),
            global_session_id.clone(),
            client_count.clone(),
            Arc::clone(&client_connections),
            swarm_members.clone(),
            swarms_by_id.clone(),
            shared_context.clone(),
            swarm_plans.clone(),
            swarm_coordinators.clone(),
            file_touch.clone(),
            channel_subscriptions.clone(),
            channel_subscriptions_by_session.clone(),
            client_debug_state.clone(),
            _debug_response_tx.clone(),
            event_history.clone(),
            event_counter.clone(),
            swarm_event_tx.clone(),
            "jcode-test".to_string(),
            "🧪".to_string(),
            mcp_pool.clone(),
            shutdown_signals.clone(),
            soft_interrupt_queues.clone(),
            AwaitMembersRuntime::default(),
            SwarmMutationRuntime::default(),
        ));
        (server_task, client_stream)
    };

    async fn send(writer: &mut crate::transport::WriteHalf, value: serde_json::Value) {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn until(
        reader: &mut BufReader<crate::transport::ReadHalf>,
        predicate: impl Fn(&ServerEvent) -> bool,
    ) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                let event: ServerEvent = serde_json::from_str(&line).unwrap();
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("daemon event timeout")
    }
    use serde_json::json;

    // A client that never opts in (desktop/SDK/ACP style) must leave the
    // agent without a decision channel: ask_user degrades to plain text.
    let (_server_task, client_stream) = connect();
    let (first_client_reader, mut first_client_writer) = client_stream.into_split();
    let mut first_client_reader = BufReader::new(first_client_reader);
    send(
        &mut first_client_writer,
        json!({"type":"subscribe","id":1,"working_dir":home._home.path()}),
    )
    .await;
    until(&mut first_client_reader, |e| {
        matches!(e, ServerEvent::Done { id: 1 })
    })
    .await;
    let session_id = sessions.read().await.keys().next().unwrap().clone();
    {
        let sessions_guard = sessions.read().await;
        let agent = sessions_guard.get(&session_id).unwrap();
        assert!(
            !agent.lock().await.has_decision_request_tx(),
            "a client that never opts in must not wire the decision channel"
        );
    }
    // Reattach a TUI-capable client to the existing session. This is the
    // important ordering case: target-aware Subscribe replaces the
    // provisional agent before the capability channel is registered.
    let (_server_task, client_stream) = connect();
    let (client_reader, mut client_writer) = client_stream.into_split();
    let mut client_reader = BufReader::new(client_reader);
    send(
        &mut client_writer,
        json!({
            "type":"subscribe",
            "id":2,
            "working_dir":home._home.path(),
            "supports_decisions":true,
            "target_session_id":session_id,
            "allow_session_takeover":true
        }),
    )
    .await;
    until(&mut client_reader, |e| {
        matches!(e, ServerEvent::Done { id: 2 })
    })
    .await;
    let sessions_guard = sessions.read().await;
    let agent = sessions_guard
        .get(&session_id)
        .expect("target session remains registered");
    assert!(
        agent.lock().await.has_decision_request_tx(),
        "reattaching a supports_decisions client must wire the target agent"
    );
    drop(client_writer);
    drop(client_reader);
    drop(sessions_guard);
    drop(first_client_writer);
    drop(first_client_reader);
}

/// Counts ask_user executions still in flight across the whole process.
/// The disconnect test wraps the registry's real ask_user tool with this so a
/// leaked waiting tool future stays visible after the connection is gone.
#[derive(Clone, Default)]
struct AskUserLiveCounter {
    live: Arc<std::sync::atomic::AtomicIsize>,
}

struct ObservableAskUser {
    inner: Arc<dyn crate::tool::Tool>,
    counter: AskUserLiveCounter,
}

#[async_trait]
impl crate::tool::Tool for ObservableAskUser {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.inner.parameters_schema()
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: crate::tool::ToolContext,
    ) -> anyhow::Result<crate::tool::ToolOutput> {
        self.counter
            .live
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let result = self.inner.execute(input, ctx).await;
        self.counter
            .live
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        result
    }
}

/// First provider response: ask the user. Second: finish the turn, so the
/// only thing that can hold the turn up is the ask_user wait itself.
#[derive(Clone)]
struct AskUserProvider {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl Provider for AskUserProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Box::pin(stream::iter([
                Ok(StreamEvent::ToolUseStart {
                    id: "ask-1".into(),
                    name: "ask_user".into(),
                }),
                Ok(StreamEvent::ToolInputDelta(
                    serde_json::json!({
                        "intent": "pick deploy strategy",
                        "question": "Deploy now?",
                        "options": [
                            {"label": "Yes"},
                            {"label": "No"}
                        ]
                    })
                    .to_string(),
                )),
                Ok(StreamEvent::ToolUseEnd),
                Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("tool_use".into()),
                }),
            ])))
        } else {
            Ok(Box::pin(stream::iter([
                Ok(StreamEvent::TextDelta("done".into())),
                Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".into()),
                }),
            ])))
        }
    }

    fn name(&self) -> &str {
        "ask-user-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

/// Regression: a client that disconnects while ask_user is waiting must not
/// leave the tool future waiting forever. The turn abort drops the tool
/// handle, but the tool task lives on; only closing the transport's decision
/// channels releases it. Without that cleanup, decision_timeout_secs 0 means
/// the tool, the forwarder task, and the response map keep each other alive
/// indefinitely (round-5 greptile finding).
#[tokio::test]
async fn normal_disconnect_releases_waiting_ask_user() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    crate::config::invalidate_config_cache();
    let counter = AskUserLiveCounter::default();
    let provider_template: Arc<dyn Provider> = Arc::new(AskUserProvider {
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });

    let sessions: SessionAgents = Arc::new(RwLock::new(HashMap::new()));
    let global_session_id = Arc::new(RwLock::new(String::new()));
    let client_count = Arc::new(RwLock::new(0usize));
    let client_connections = Arc::new(RwLock::new(HashMap::new()));
    let swarm_members = Arc::new(RwLock::new(HashMap::new()));
    let swarms_by_id = Arc::new(RwLock::new(HashMap::new()));
    let shared_context = Arc::new(RwLock::new(HashMap::new()));
    let swarm_plans = Arc::new(RwLock::new(HashMap::new()));
    let swarm_coordinators = Arc::new(RwLock::new(HashMap::new()));
    let file_touch = FileTouchService::new();
    let channel_subscriptions = Arc::new(RwLock::new(HashMap::new()));
    let channel_subscriptions_by_session = Arc::new(RwLock::new(HashMap::new()));
    let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
    let (_debug_response_tx, _) = broadcast::channel(8);
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let (_global_event_tx, _) = broadcast::channel(8);
    let global_is_processing = Arc::new(RwLock::new(false));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());

    let (server_stream, client_stream) = crate::transport::Stream::pair().expect("socket pair");
    let server_task = tokio::spawn(handle_client(
        server_stream,
        Arc::clone(&sessions),
        _global_event_tx.clone(),
        provider_template.clone(),
        global_is_processing.clone(),
        global_session_id.clone(),
        client_count.clone(),
        Arc::clone(&client_connections),
        swarm_members.clone(),
        swarms_by_id.clone(),
        shared_context.clone(),
        swarm_plans.clone(),
        swarm_coordinators.clone(),
        file_touch.clone(),
        channel_subscriptions.clone(),
        channel_subscriptions_by_session.clone(),
        client_debug_state.clone(),
        _debug_response_tx.clone(),
        event_history.clone(),
        event_counter.clone(),
        swarm_event_tx.clone(),
        "jcode-test".to_string(),
        "\u{1f9ea}".to_string(),
        mcp_pool.clone(),
        shutdown_signals.clone(),
        soft_interrupt_queues.clone(),
        AwaitMembersRuntime::default(),
        SwarmMutationRuntime::default(),
    ));
    let (client_reader, mut client_writer) = client_stream.into_split();
    let mut client_reader = BufReader::new(client_reader);

    async fn send(writer: &mut crate::transport::WriteHalf, value: serde_json::Value) {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn until(
        reader: &mut BufReader<crate::transport::ReadHalf>,
        predicate: impl Fn(&ServerEvent) -> bool,
    ) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                let event: ServerEvent = serde_json::from_str(&line).unwrap();
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("daemon event timeout")
    }
    use serde_json::json;

    send(
        &mut client_writer,
        json!({"type":"subscribe","id":1,"working_dir":home._home.path(),"supports_decisions":true}),
    )
    .await;
    until(&mut client_reader, |e| {
        matches!(e, ServerEvent::Done { id: 1 })
    })
    .await;

    // Swap the real ask_user for an observing wrapper on this session's
    // registry so the test can see the tool future come and go.
    let session_id = sessions.read().await.keys().next().unwrap().clone();
    {
        let sessions_guard = sessions.read().await;
        let agent = sessions_guard.get(&session_id).unwrap();
        let registry = agent.lock().await.registry();
        let real_ask_user = registry
            .unregister("ask_user")
            .await
            .expect("registry must carry ask_user");
        registry
            .register(
                "ask_user".to_string(),
                Arc::new(ObservableAskUser {
                    inner: real_ask_user,
                    counter: counter.clone(),
                }),
            )
            .await;
    }

    send(
        &mut client_writer,
        json!({"type":"message","id":2,"content":"ask me"}),
    )
    .await;
    let decision = until(&mut client_reader, |e| {
        matches!(e, ServerEvent::DecisionRequest { .. })
    })
    .await;
    assert!(
        matches!(&decision, ServerEvent::DecisionRequest { request_id, .. } if request_id.starts_with("decision-ask-1")),
        "{decision:?}"
    );
    assert_eq!(
        counter.live.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "ask_user must be waiting on the user"
    );

    // Normal disconnect while the turn waits on ask_user. The server must run
    // its disconnect cleanup and the waiting tool future must be released
    // instead of waiting forever on a client that can never answer.
    drop(client_writer);
    drop(client_reader);
    tokio::time::timeout(Duration::from_secs(10), server_task)
        .await
        .expect("server task must finish after disconnect")
        .expect("server task join")
        .expect("server task result");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while counter.live.load(std::sync::atomic::Ordering::SeqCst) > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "ask_user future must terminate after the disconnect cleanup"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
