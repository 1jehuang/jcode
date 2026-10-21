use super::*;
use crate::server::client_session::response_capability::refresh_session;
use serde_json::{Value, json};

type Reader = BufReader<crate::transport::ReadHalf>;
type Writer = crate::transport::WriteHalf;

#[tokio::test]
async fn user_response_socket_registration_follows_explicit_capability() {
    sdk_socket_workflow(true).await;
}

#[tokio::test]
async fn user_response_deferred_refresh_reads_current_connections() {
    let _lock = crate::storage::lock_test_env();
    let _home = IsolatedReloadRecoveryEnv::new();
    let provider: Arc<dyn Provider> = Arc::new(SdkCallbackProvider::default());
    let session = crate::session::Session::create_with_id("capability-deferred".into(), None, None);
    let session_id = session.id.clone();
    let registry = Registry::new(provider.clone()).await;
    let agent = Arc::new(Mutex::new(Agent::new_with_session(
        provider, registry, session, None,
    )));
    let sessions = Arc::new(RwLock::new(HashMap::from([(
        session_id.clone(),
        agent.clone(),
    )])));
    let connections = Arc::new(RwLock::new(HashMap::from([(
        "desktop".into(),
        ClientConnectionInfo {
            client_id: "desktop".into(),
            session_id: session_id.clone(),
            client_instance_id: None,
            debug_client_id: None,
            connected_at: Instant::now(),
            last_seen: Instant::now(),
            is_processing: false,
            current_tool_name: None,
            terminal_env: Vec::new(),
            supports_user_response: true,
            disconnect_tx: mpsc::unbounded_channel().0,
        },
    )])));
    for enabled_before_queue in [true, false] {
        let mut guard = agent.lock().await;
        guard
            .set_desktop_response_capability(enabled_before_queue)
            .await;
        connections
            .write()
            .await
            .get_mut("desktop")
            .unwrap()
            .supports_user_response = enabled_before_queue;
        // Refresh must return while the Agent is held, then read the new
        // connection snapshot when its deferred task finally acquires it.
        tokio::time::timeout(
            Duration::from_secs(1),
            refresh_session(&sessions, &connections, &session_id),
        )
        .await
        .expect("busy capability refresh blocked attachment");
        connections
            .write()
            .await
            .get_mut("desktop")
            .unwrap()
            .supports_user_response = !enabled_before_queue;
        drop(guard);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let enabled = agent
                    .lock()
                    .await
                    .tool_names()
                    .await
                    .iter()
                    .any(|name| name == "user_response");
                if enabled != enabled_before_queue {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("deferred refresh replayed stale support");
    }
}

async fn send(writer: &mut Writer, value: Value) {
    writer
        .write_all(format!("{value}\n").as_bytes())
        .await
        .unwrap();
}

async fn until(reader: &mut Reader, predicate: impl Fn(&ServerEvent) -> bool) -> ServerEvent {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).await.unwrap() > 0);
            let event: ServerEvent = serde_json::from_str(&line).unwrap();
            assert!(!matches!(event, ServerEvent::Error { .. }), "{event:?}");
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("capability socket event timeout")
}

async fn done(reader: &mut Reader, writer: &mut Writer, value: Value) {
    let id = value["id"].as_u64().unwrap();
    send(writer, value).await;
    until(
        reader,
        |event| matches!(event, ServerEvent::Done { id: actual } if *actual == id),
    )
    .await;
    // Done may be queued before the server finishes attachment bookkeeping.
    // A same-transport read-only request is an ordering barrier for other clients.
    send(writer, json!({"type":"list_tools","id":901})).await;
    until(reader, |event| {
        matches!(event, ServerEvent::Tools { id: 901, .. })
    })
    .await;
}

async fn catalog(reader: &mut Reader, writer: &mut Writer, enabled: bool) {
    send(writer, json!({"type":"list_tools","id":900})).await;
    let ServerEvent::Tools { tools, .. } = until(reader, |event| {
        matches!(event, ServerEvent::Tools { id: 900, .. })
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(
        tools.iter().any(|tool| tool.name == "user_response"),
        enabled
    );
}

async fn connection_count(
    connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    expected: usize,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while connections.read().await.len() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("connection transition timeout");
}

#[tokio::test]
async fn user_response_multiclient_subscribe_resume_and_disconnect() {
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
        let (server_stream, client_stream) = crate::transport::Stream::pair().unwrap();
        let task = tokio::spawn(handle_client(
            server_stream,
            sessions.clone(),
            global_event_tx.clone(),
            provider_template.clone(),
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
        let (reader, writer) = client_stream.into_split();
        (task, BufReader::new(reader), writer)
    };
    let subscribe = |id, target: Option<&str>, supports| {
        json!({
            "type":"subscribe", "id":id, "working_dir":home._home.path(),
            "target_session_id":target, "supports_user_response":supports,
            "supports_pdf_panels":true,
        })
    };

    let (desktop_task, mut desktop_reader, mut desktop_writer) = connect();
    done(
        &mut desktop_reader,
        &mut desktop_writer,
        subscribe(1, None, true),
    )
    .await;
    let source = sessions.read().await.keys().next().unwrap().clone();
    catalog(&mut desktop_reader, &mut desktop_writer, true).await;

    let (tui_task, mut tui_reader, mut tui_writer) = connect();
    done(
        &mut tui_reader,
        &mut tui_writer,
        subscribe(2, Some(&source), false),
    )
    .await;
    catalog(&mut tui_reader, &mut tui_writer, true).await;
    // Repeated non-supporting snapshots must not override the Desktop.
    done(&mut tui_reader, &mut tui_writer, subscribe(3, None, false)).await;
    catalog(&mut tui_reader, &mut tui_writer, true).await;

    let (second_task, mut second_reader, mut second_writer) = connect();
    done(
        &mut second_reader,
        &mut second_writer,
        subscribe(4, Some(&source), true),
    )
    .await;
    done(
        &mut desktop_reader,
        &mut desktop_writer,
        subscribe(5, None, false),
    )
    .await;
    catalog(&mut tui_reader, &mut tui_writer, true).await;
    done(
        &mut desktop_reader,
        &mut desktop_writer,
        subscribe(51, None, true),
    )
    .await;
    // Disconnecting one Desktop must leave the other responder available.
    drop(second_reader);
    drop(second_writer);
    connection_count(&client_connections, 2).await;
    catalog(&mut tui_reader, &mut tui_writer, true).await;
    done(
        &mut desktop_reader,
        &mut desktop_writer,
        subscribe(52, None, false),
    )
    .await;
    catalog(&mut tui_reader, &mut tui_writer, false).await;
    tokio::time::timeout(Duration::from_secs(10), second_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    done(
        &mut desktop_reader,
        &mut desktop_writer,
        subscribe(6, None, true),
    )
    .await;
    catalog(&mut tui_reader, &mut tui_writer, true).await;
    let (target_task, mut target_reader, mut target_writer) = connect();
    done(
        &mut target_reader,
        &mut target_writer,
        subscribe(7, None, false),
    )
    .await;
    let target = sessions
        .read()
        .await
        .keys()
        .find(|id| **id != source)
        .unwrap()
        .clone();
    catalog(&mut target_reader, &mut target_writer, false).await;

    // Resume carries this connection's support to the target and removes it
    // from the old session, whose non-supporting subscriber remains attached.
    done(
        &mut desktop_reader,
        &mut desktop_writer,
        json!({
            "type":"resume_session", "id":8, "session_id":target,
        }),
    )
    .await;
    catalog(&mut tui_reader, &mut tui_writer, false).await;
    catalog(&mut target_reader, &mut target_writer, true).await;
    done(
        &mut target_reader,
        &mut target_writer,
        subscribe(9, None, false),
    )
    .await;
    catalog(&mut target_reader, &mut target_writer, true).await;

    // Explicit detach withdraws capability before the socket actually closes.
    done(
        &mut desktop_reader,
        &mut desktop_writer,
        json!({
            "type":"prepare_disconnect", "id":10,
        }),
    )
    .await;
    catalog(&mut target_reader, &mut target_writer, false).await;
    done(
        &mut desktop_reader,
        &mut desktop_writer,
        subscribe(11, None, true),
    )
    .await;
    catalog(&mut target_reader, &mut target_writer, true).await;
    drop(desktop_reader);
    drop(desktop_writer);
    connection_count(&client_connections, 2).await;
    catalog(&mut target_reader, &mut target_writer, false).await;
    tokio::time::timeout(Duration::from_secs(10), desktop_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Avoid idle unsaved-session grace timers in this isolated fixture.
    sessions.write().await.clear();
    drop(tui_reader);
    drop(tui_writer);
    drop(target_reader);
    drop(target_writer);
    for task in [tui_task, target_task] {
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
