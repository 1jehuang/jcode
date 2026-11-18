//! End-to-end continuation coverage through the ordinary daemon socket protocol.
use super::*;
use serde_json::json;
use std::sync::atomic::AtomicUsize;

#[derive(Clone)]
struct ContinuationProvider {
    calls: Arc<AtomicUsize>,
    continuation_started: Arc<tokio::sync::Notify>,
    release_continuation: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Provider for ContinuationProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 1 {
            self.continuation_started.notify_one();
            self.release_continuation.notified().await;
        }
        Ok(Box::pin(stream::iter([
            Ok(StreamEvent::TextDelta(format!("model turn {}", call + 1))),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "daemon-continuation-socket-test"
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn send(writer: &mut crate::transport::WriteHalf, value: serde_json::Value) {
    writer
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
}

async fn receive(reader: &mut BufReader<crate::transport::ReadHalf>) -> ServerEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).await.unwrap() > 0);
        serde_json::from_str(&line).unwrap()
    })
    .await
    .expect("daemon socket event timeout")
}

#[tokio::test]
async fn ordinary_socket_auto_poke_is_bounded_and_has_one_final_done() {
    continuation_socket_workflow(false).await;
}

#[tokio::test]
async fn second_socket_can_disable_auto_poke_during_a_continuation() {
    continuation_socket_workflow(true).await;
}

async fn continuation_socket_workflow(disable_from_observer: bool) {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    let provider = ContinuationProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        continuation_started: Arc::new(tokio::sync::Notify::new()),
        release_continuation: Arc::new(tokio::sync::Notify::new()),
    };
    let sessions: SessionAgents =
        Arc::new(crate::server::SessionManager::from_agents(HashMap::new()));
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
    let (debug_response_tx, _) = broadcast::channel(8);
    let event_history = Arc::new(RwLock::new(std::collections::VecDeque::new()));
    let event_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (swarm_event_tx, _) = broadcast::channel(8);
    let (global_event_tx, _) = broadcast::channel(8);
    let global_is_processing = Arc::new(RwLock::new(false));
    let shutdown_signals = Arc::new(RwLock::new(HashMap::new()));
    let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
    let mcp_pool = Arc::new(crate::mcp::SharedMcpPool::from_default_config());
    let connect = || {
        let (server, client) = crate::transport::Stream::pair().unwrap();
        let task = tokio::spawn(handle_client(
            server,
            sessions.clone(),
            global_event_tx.clone(),
            Arc::new(provider.clone()),
            global_is_processing.clone(),
            global_session_id.clone(),
            client_count.clone(),
            client_connections.clone(),
            swarm_members.clone(),
            swarms_by_id.clone(),
            shared_context.clone(),
            swarm_plans.clone(),
            swarm_coordinators.clone(),
            file_touch.clone(),
            channel_subscriptions.clone(),
            channel_subscriptions_by_session.clone(),
            client_debug_state.clone(),
            debug_response_tx.clone(),
            event_history.clone(),
            event_counter.clone(),
            swarm_event_tx.clone(),
            "jcode-test".into(),
            "test".into(),
            mcp_pool.clone(),
            shutdown_signals.clone(),
            soft_interrupt_queues.clone(),
            AwaitMembersRuntime::default(),
            SwarmMutationRuntime::default(),
        ));
        let (reader, writer) = client.into_split();
        (task, BufReader::new(reader), writer)
    };
    let (task, mut reader, mut writer) = connect();
    // Deliberately omit supports_runtime_continuation and all special capabilities.
    send(
        &mut writer,
        json!({"type":"subscribe","id":1,"working_dir":home._home.path()}),
    )
    .await;
    while !matches!(receive(&mut reader).await, ServerEvent::Done { id: 1 }) {}
    let session_id = sessions.read().await.keys().next().unwrap().clone();
    let mut observer = if disable_from_observer {
        let (task, mut reader, mut writer) = connect();
        send(&mut writer, json!({"type":"subscribe","id":10,"target_session_id":session_id,"working_dir":home._home.path()})).await;
        while !matches!(receive(&mut reader).await, ServerEvent::Done { id: 10 }) {}
        Some((task, reader, writer))
    } else {
        None
    };
    crate::todo::save_todos(
        &session_id,
        &[crate::todo::TodoItem {
            id: "unfinished".into(),
            content: "Finish the ordinary client task".into(),
            status: "pending".into(),
            priority: "high".into(),
            ..Default::default()
        }],
    )
    .unwrap();
    send(
        &mut writer,
        json!({"type":"message","id":2,"content":"work on the pending task"}),
    )
    .await;
    tokio::time::timeout(
        Duration::from_secs(10),
        provider.continuation_started.notified(),
    )
    .await
    .expect("ordinary client did not get daemon-owned continuation");
    // Drain the real wire while the second model request is held open. A Done
    // from the initial turn would be a premature terminal, even if more follows.
    let before_release = tokio::time::timeout(Duration::from_millis(150), async {
        loop {
            let event = receive(&mut reader).await;
            assert!(
                !matches!(event, ServerEvent::Done { id: 2 }),
                "premature Done: {event:?}"
            );
            assert!(
                !matches!(event, ServerEvent::Error { .. }),
                "unexpected error: {event:?}"
            );
        }
    })
    .await;
    assert!(before_release.is_err());

    if let Some((_, observer_reader, observer_writer)) = observer.as_mut() {
        send(
            observer_writer,
            json!({"type":"runtime_command","id":11,"command":"poke","args":["off"]}),
        )
        .await;
        loop {
            let event = receive(observer_reader).await;
            assert!(
                !matches!(event, ServerEvent::Error { id: 11, .. }),
                "control rejected while busy: {event:?}"
            );
            if matches!(event, ServerEvent::RuntimeCommandResult { id: 11, .. }) {
                break;
            }
        }
    }
    provider.release_continuation.notify_one();
    loop {
        let event = receive(&mut reader).await;
        assert!(
            !matches!(event, ServerEvent::Error { .. }),
            "unexpected error: {event:?}"
        );
        if matches!(event, ServerEvent::Done { id: 2 }) {
            break;
        }
    }
    let expected = if disable_from_observer {
        2
    } else {
        1 + usize::from(crate::todo::followup::INCOMPLETE_TODO_MAX_ATTEMPTS)
    };
    assert_eq!(provider.calls.load(Ordering::SeqCst), expected);
    // An ordered protocol barrier detects queued duplicate terminals without
    // relying on a sleep or racing the daemon's final event publication.
    send(&mut writer, json!({"type":"get_history","id":3})).await;
    loop {
        let event = receive(&mut reader).await;
        assert!(
            !matches!(event, ServerEvent::Done { id: 2 }),
            "duplicate final Done"
        );
        if matches!(event, ServerEvent::History { id: 3, .. }) {
            break;
        }
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), expected);
    task.abort();
    let _ = task.await;
    if let Some((task, _, _)) = observer {
        task.abort();
        let _ = task.await;
    }
}
