use super::*;

#[test]
fn subscribe_system_prompt_is_creation_only_and_preserves_empty() {
    for prompt in [None, Some("custom system prompt"), Some("")] {
        assert_eq!(new_session_system_prompt(true, None, prompt), prompt);
        assert_eq!(new_session_system_prompt(false, None, prompt), None);
        assert_eq!(
            new_session_system_prompt(true, Some("existing"), prompt),
            None
        );
        assert_eq!(
            new_session_system_prompt(false, Some("existing"), prompt),
            None
        );
    }
}

#[derive(Clone)]
struct SystemPromptCaptureProvider(Arc<std::sync::Mutex<Vec<String>>>);

#[async_trait]
impl Provider for SystemPromptCaptureProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.0.lock().unwrap().push(system.to_string());
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta("ok".into())),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }
    fn name(&self) -> &str {
        "prompt-capture"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

#[tokio::test]
async fn system_prompt_socket_creation_attach_resume_fork_and_no_leaking() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let provider_template: Arc<dyn Provider> =
        Arc::new(SystemPromptCaptureProvider(captured.clone()));

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
    let (server_task, client_stream) = connect();

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
                assert!(
                    !matches!(event, ServerEvent::Error { .. }),
                    "unexpected server error: {event:?}"
                );
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("daemon event timeout")
    }
    use serde_json::json;

    for prompt in ["SDK custom system prompt", ""] {
        // A fresh connection creates a session and persists its explicit prompt
        // before acknowledging Subscribe, even with no conversation messages.
        let (owner_task, owner_stream) = connect();
        let (owner_reader, mut owner_writer) = owner_stream.into_split();
        let mut owner_reader = BufReader::new(owner_reader);
        send(
            &mut owner_writer,
            json!({"type":"subscribe","id":101,
            "working_dir":home._home.path(),"system_prompt":prompt}),
        )
        .await;
        until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::Done { id: 101 })
        })
        .await;
        send(
            &mut owner_writer,
            serde_json::to_value(Request::GetState { id: 102 }).unwrap(),
        )
        .await;
        let ServerEvent::State {
            session_id: parent_id,
            ..
        } = until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::State { id: 102, .. })
        })
        .await
        else {
            unreachable!()
        };
        let saved = crate::session::Session::load(&parent_id).unwrap();
        assert_eq!(saved.system_prompt.as_deref(), Some(prompt));
        assert_eq!(saved.visible_conversation_message_count(), 0);

        // Repeated Subscribe cannot mutate even the current owner's prompt.
        send(
            &mut owner_writer,
            json!({"type":"subscribe","id":103,
            "working_dir":home._home.path(),"system_prompt":"replacement forbidden"}),
        )
        .await;
        until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::Done { id: 103 })
        })
        .await;
        assert_eq!(
            crate::session::Session::load(&parent_id)
                .unwrap()
                .system_prompt
                .as_deref(),
            Some(prompt)
        );

        // An attaching observer likewise cannot replace another session's prompt.
        send(
            &mut client_writer,
            json!({"type":"subscribe","id":104,
            "target_session_id":parent_id,"system_prompt":"attachment forbidden"}),
        )
        .await;
        until(&mut client_reader, |e| {
            matches!(e, ServerEvent::Done { id: 104 })
        })
        .await;
        send(
            &mut owner_writer,
            json!({"type":"message","id":105,"content":"hello"}),
        )
        .await;
        until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::Done { id: 105 })
        })
        .await;
        assert_eq!(
            captured.lock().unwrap().last().map(String::as_str),
            Some(prompt)
        );

        send(&mut owner_writer, json!({"type":"split","id":106})).await;
        let ServerEvent::SplitResponse {
            new_session_id: child_id,
            ..
        } = until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::SplitResponse { id: 106, .. })
        })
        .await
        else {
            unreachable!()
        };
        assert_eq!(
            crate::session::Session::load(&child_id)
                .unwrap()
                .system_prompt
                .as_deref(),
            Some(prompt)
        );
        // This attachment restores a persisted fork, rather than a live Agent.
        send(
            &mut client_writer,
            json!({"type":"subscribe","id":107,
            "target_session_id":child_id,"system_prompt":"fork replacement forbidden"}),
        )
        .await;
        until(&mut client_reader, |e| {
            matches!(e, ServerEvent::Done { id: 107 })
        })
        .await;
        send(
            &mut client_writer,
            json!({"type":"message","id":108,"content":"fork hello"}),
        )
        .await;
        until(&mut client_reader, |e| {
            matches!(e, ServerEvent::Done { id: 108 })
        })
        .await;
        assert_eq!(
            captured.lock().unwrap().last().map(String::as_str),
            Some(prompt)
        );

        send(&mut owner_writer, json!({"type":"clear","id":109})).await;
        until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::Done { id: 109 })
        })
        .await;
        send(
            &mut owner_writer,
            json!({"type":"message","id":110,"content":"new session"}),
        )
        .await;
        until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::Done { id: 110 })
        })
        .await;
        assert_ne!(
            captured.lock().unwrap().last().map(String::as_str),
            Some(prompt)
        );
        send(
            &mut owner_writer,
            json!({"type":"resume_session","id":111,"session_id":parent_id}),
        )
        .await;
        until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::Done { id: 111 })
        })
        .await;
        send(
            &mut owner_writer,
            json!({"type":"message","id":112,"content":"resumed hello"}),
        )
        .await;
        until(&mut owner_reader, |e| {
            matches!(e, ServerEvent::Done { id: 112 })
        })
        .await;
        assert_eq!(
            captured.lock().unwrap().last().map(String::as_str),
            Some(prompt)
        );

        drop(owner_writer);
        drop(owner_reader);
        tokio::time::timeout(Duration::from_secs(5), owner_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    drop(client_writer);
    drop(client_reader);
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
