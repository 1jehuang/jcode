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
