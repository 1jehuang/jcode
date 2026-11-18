//! Wire-level: an agent task must not stall the connection's request loop.
use super::system_prompt::SystemPromptSocketFixture;
use super::*;
use serde_json::json;
use tokio::sync::Notify;

/// Holds the turn inside the provider call until the test releases it.
#[derive(Clone)]
struct GatedProvider {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl Provider for GatedProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta("done".into())),
            Ok(StreamEvent::MessageEnd { stop_reason: None }),
        ])))
    }
    fn name(&self) -> &str {
        "gated"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

/// While an agent task's turn runs, the same connection must still be read:
/// its `ask_user` answers and cancels arrive there. A Ping stands in for them.
#[tokio::test]
async fn agent_task_leaves_the_request_loop_reading() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let fixture = SystemPromptSocketFixture::with_provider(
        Arc::new(GatedProvider {
            started: started.clone(),
            release: release.clone(),
        }),
        Arc::default(),
    );
    let (server_task, stream) = fixture.connect();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut send = async |value: serde_json::Value| {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    };
    async fn until(
        reader: &mut BufReader<crate::transport::ReadHalf>,
        predicate: impl Fn(&ServerEvent) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                let event: ServerEvent = serde_json::from_str(&line).unwrap();
                let event = turn_payload(event);
                assert!(
                    !matches!(event, ServerEvent::Error { .. }),
                    "unexpected server error: {event:?}"
                );
                if predicate(&event) {
                    return;
                }
            }
        })
        .await
        .expect("daemon event timeout: the request loop stopped reading");
    }

    send(json!({"type":"subscribe","id":1,"working_dir":home._home.path()})).await;
    until(&mut reader, |e| matches!(e, ServerEvent::Done { id: 1 })).await;
    send(json!({"type":"agent_task","id":2,"from_agent":"peer","task":"work"})).await;
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("agent task turn started");

    send(json!({"type":"ping","id":3})).await;
    until(&mut reader, |e| {
        matches!(e, ServerEvent::Pong { id: 3, .. })
    })
    .await;

    release.notify_one();
    until(&mut reader, |e| matches!(e, ServerEvent::Done { id: 2 })).await;
    drop(send);
    drop(reader);
    server_task.abort();
}

/// An agent task is this connection's turn. A message sent while it runs is
/// refused at once; it must not wait in the request loop for the Agent the
/// task holds, which would stop the loop reading answers and cancels.
#[tokio::test]
async fn message_during_agent_task_is_refused_without_stalling_the_loop() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let fixture = SystemPromptSocketFixture::with_provider(
        Arc::new(GatedProvider {
            started: started.clone(),
            release: release.clone(),
        }),
        Arc::default(),
    );
    let (server_task, stream) = fixture.connect();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut send = async |value: serde_json::Value| {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    };
    async fn next_matching(
        reader: &mut BufReader<crate::transport::ReadHalf>,
        predicate: impl Fn(&ServerEvent) -> bool,
    ) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                let event = turn_payload(serde_json::from_str(&line).unwrap());
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("daemon event timeout: the request loop stopped reading")
    }

    send(json!({"type":"subscribe","id":1,"working_dir":home._home.path()})).await;
    next_matching(&mut reader, |e| matches!(e, ServerEvent::Done { id: 1 })).await;
    send(json!({"type":"agent_task","id":2,"from_agent":"peer","task":"work"})).await;
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("agent task turn started");

    send(json!({"type":"message","id":3,"content":"second"})).await;
    let refused = next_matching(&mut reader, |e| {
        matches!(e, ServerEvent::Error { id: 3, .. })
    })
    .await;
    let ServerEvent::Error { message, .. } = refused else {
        unreachable!()
    };
    assert_eq!(message, "Already processing a message");
    send(json!({"type":"ping","id":4})).await;
    next_matching(&mut reader, |e| {
        matches!(e, ServerEvent::Pong { id: 4, .. })
    })
    .await;

    release.notify_one();
    next_matching(&mut reader, |e| matches!(e, ServerEvent::Done { id: 2 })).await;
    drop(send);
    drop(reader);
    server_task.abort();
}

/// An agent task sent on a second attachment while another connection's turn
/// holds the shared Agent is refused at once, and that connection keeps
/// reading instead of waiting for the other turn to finish.
#[tokio::test]
async fn agent_task_on_a_busy_shared_session_is_refused_without_stalling() {
    let _lock = crate::storage::lock_test_env();
    let home = IsolatedReloadRecoveryEnv::new();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let fixture = SystemPromptSocketFixture::with_provider(
        Arc::new(GatedProvider {
            started: started.clone(),
            release: release.clone(),
        }),
        Arc::default(),
    );
    async fn send(writer: &mut crate::transport::WriteHalf, value: serde_json::Value) {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn next_matching(
        reader: &mut BufReader<crate::transport::ReadHalf>,
        predicate: impl Fn(&ServerEvent) -> bool,
    ) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                let event = turn_payload(serde_json::from_str(&line).unwrap());
                if predicate(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("daemon event timeout: the request loop stopped reading")
    }

    let (owner_task, owner) = fixture.connect();
    let (owner_reader, mut owner_writer) = owner.into_split();
    let mut owner_reader = BufReader::new(owner_reader);
    send(
        &mut owner_writer,
        json!({"type":"subscribe","id":1,"working_dir":home._home.path()}),
    )
    .await;
    next_matching(&mut owner_reader, |e| {
        matches!(e, ServerEvent::Done { id: 1 })
    })
    .await;
    send(&mut owner_writer, json!({"type":"state","id":2})).await;
    let ServerEvent::State { session_id, .. } = next_matching(&mut owner_reader, |e| {
        matches!(e, ServerEvent::State { id: 2, .. })
    })
    .await
    else {
        unreachable!()
    };
    send(
        &mut owner_writer,
        json!({"type":"message","id":3,"content":"long turn"}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("owner turn started");

    let (peer_task, peer) = fixture.connect();
    let (peer_reader, mut peer_writer) = peer.into_split();
    let mut peer_reader = BufReader::new(peer_reader);
    send(
        &mut peer_writer,
        json!({"type":"subscribe","id":11,"target_session_id":session_id}),
    )
    .await;
    next_matching(&mut peer_reader, |e| {
        matches!(e, ServerEvent::Done { id: 11 })
    })
    .await;
    send(
        &mut peer_writer,
        json!({"type":"agent_task","id":12,"from_agent":"peer","task":"work"}),
    )
    .await;
    let ServerEvent::Error { message, .. } = next_matching(&mut peer_reader, |e| {
        matches!(e, ServerEvent::Error { id: 12, .. })
    })
    .await
    else {
        unreachable!()
    };
    assert!(message.contains("session is busy"), "{message}");
    send(&mut peer_writer, json!({"type":"ping","id":13})).await;
    next_matching(&mut peer_reader, |e| {
        matches!(e, ServerEvent::Pong { id: 13, .. })
    })
    .await;

    release.notify_one();
    next_matching(&mut owner_reader, |e| {
        matches!(e, ServerEvent::Done { id: 3 })
    })
    .await;
    owner_task.abort();
    peer_task.abort();
}
