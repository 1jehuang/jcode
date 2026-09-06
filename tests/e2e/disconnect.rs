//! Desktop-style abrupt disconnects through the real server and durable storage.

use crate::test_support::*;
use jcode::session::SessionStatus;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct StreamingProvider;

#[async_trait]
impl Provider for StreamingProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume: Option<&str>,
    ) -> Result<EventStream> {
        Ok(Box::pin(
            stream::iter([Ok(StreamEvent::TextDelta("still working".into()))])
                .chain(stream::pending()),
        ))
    }

    fn name(&self) -> &str {
        "disconnect-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

#[derive(Clone, Copy)]
enum Turn {
    Idle,
    Completed,
    Streaming,
}

async fn abrupt_disconnect(turn: Turn, continue_on_disconnect: bool) -> Result<()> {
    let _env = setup_test_env()?;
    let runtime = tempfile::tempdir()?;
    let socket = runtime.path().join("server.sock");
    let debug_socket = runtime.path().join("debug.sock");
    let provider: Arc<dyn Provider> = if matches!(turn, Turn::Streaming) {
        Arc::new(StreamingProvider)
    } else {
        let provider = MockProvider::new();
        provider.queue_response(vec![
            StreamEvent::TextDelta("finished successfully".into()),
            StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            },
        ]);
        Arc::new(provider)
    };
    let server = server::Server::new_with_paths(provider, socket.clone(), debug_socket.clone());
    let handle = tokio::spawn(async move { server.run().await });
    let result = async {
        wait_for_server_ready(&socket, &debug_socket).await?;
        let connection = server::connect_socket(&socket).await?;
        let (reader, mut writer) = connection.into_split();
        let mut reader = BufReader::new(reader);
        // This is the ownership flag sent by the Desktop API bridge, including
        // older clients. Do not send prepare_disconnect before closing the socket.
        writer
            .write_all(
                format!(
                    "{}\n",
                    serde_json::json!({
                        "type": "subscribe", "id": 1,
                        "working_dir": std::env::current_dir()?,
                        "crash_on_disconnect": true,
                        "continue_on_disconnect": continue_on_disconnect,
                    })
                )
                .as_bytes(),
            )
            .await?;
        let mut session_id = None;
        timeout(Duration::from_secs(10), async {
            loop {
                let mut line = String::new();
                anyhow::ensure!(reader.read_line(&mut line).await? > 0, "unexpected EOF");
                match serde_json::from_str::<ServerEvent>(&line)? {
                    ServerEvent::SessionId { session_id: id } => session_id = Some(id),
                    ServerEvent::Done { id: 1 } => break,
                    ServerEvent::Error { message, .. } => anyhow::bail!(message),
                    _ => {}
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        let session_id = session_id.context("subscribe did not identify the session")?;
        {
            // Empty sessions intentionally have no transcript file. A context-only
            // message gives the idle case durable state without starting a model.
            writer
                .write_all(
                    format!(
                        "{}\n",
                        serde_json::json!({
                            "type": "message", "id": 2, "content": "hello",
                            "no_reply": matches!(turn, Turn::Idle),
                        })
                    )
                    .as_bytes(),
                )
                .await?;
            timeout(Duration::from_secs(10), async {
                loop {
                    let mut line = String::new();
                    anyhow::ensure!(reader.read_line(&mut line).await? > 0, "unexpected EOF");
                    match serde_json::from_str::<ServerEvent>(&line)? {
                        ServerEvent::Done { id: 2 } if matches!(turn, Turn::Completed) => break,
                        ServerEvent::ContextMessageAdded { id: 2 }
                            if matches!(turn, Turn::Idle) =>
                        {
                            break;
                        }
                        ServerEvent::TextDelta { .. } if matches!(turn, Turn::Streaming) => break,
                        ServerEvent::Error { message, .. } => anyhow::bail!(message),
                        _ => {}
                    }
                }
                Ok::<_, anyhow::Error>(())
            })
            .await??;
        }
        // An abrupt socket EOF is what the runtime sees when Desktop is killed.
        drop(writer);
        drop(reader);
        timeout(Duration::from_secs(10), async {
            loop {
                let session = Session::load(&session_id)?;
                if session.status != SessionStatus::Active {
                    if matches!(turn, Turn::Streaming) {
                        assert!(matches!(session.status, SessionStatus::Crashed { .. }));
                    } else {
                        assert_eq!(session.status, SessionStatus::Closed);
                    }
                    if matches!(turn, Turn::Completed) {
                        assert!(serde_json::to_string(&session)?.contains("finished successfully"));
                    }
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        Ok(())
    }
    .await;
    abort_server_and_cleanup(&handle, &socket, &debug_socket);
    result
}

#[tokio::test]
async fn desktop_force_quit_while_idle_closes_session() -> Result<()> {
    abrupt_disconnect(Turn::Idle, false).await
}

#[tokio::test]
async fn desktop_force_quit_after_done_preserves_completed_session() -> Result<()> {
    abrupt_disconnect(Turn::Completed, false).await
}

#[tokio::test]
async fn desktop_force_quit_while_streaming_marks_session_crashed() -> Result<()> {
    abrupt_disconnect(Turn::Streaming, false).await
}

#[derive(Clone, Default)]
struct ReconnectProvider {
    finish: Arc<tokio::sync::Notify>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl Provider for ReconnectProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume: Option<&str>,
    ) -> Result<EventStream> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::pin(stream::unfold(
            (0, Arc::clone(&self.finish)),
            |(step, finish)| async move {
                let event = match step {
                    0 => StreamEvent::TextDelta("before disconnect ".into()),
                    1 => {
                        finish.notified().await;
                        StreamEvent::TextDelta("after reconnect".into())
                    }
                    2 => StreamEvent::MessageEnd {
                        stop_reason: Some("end_turn".into()),
                    },
                    _ => return None,
                };
                Some((Ok(event), (step + 1, finish)))
            },
        )))
    }

    fn name(&self) -> &str {
        "reconnect-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

async fn send_native<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    value: serde_json::Value,
) -> Result<()> {
    writer.write_all(format!("{value}\n").as_bytes()).await?;
    Ok(())
}

async fn native_until<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    predicate: impl Fn(&ServerEvent) -> bool,
) -> Result<Vec<ServerEvent>> {
    timeout(Duration::from_secs(10), async {
        let mut events = Vec::new();
        loop {
            let mut line = String::new();
            anyhow::ensure!(reader.read_line(&mut line).await? > 0, "unexpected EOF");
            let event = serde_json::from_str::<ServerEvent>(&line)?;
            if let ServerEvent::Error { message, .. } = &event {
                anyhow::bail!("native request failed: {message}");
            }
            let done = predicate(&event);
            events.push(event);
            if done {
                return Ok(events);
            }
        }
    })
    .await?
}

#[derive(Clone, Copy)]
enum RemoteTurnEnd {
    Reattach,
    Detached,
    Cancel,
}

async fn remote_disconnect_turn(end: RemoteTurnEnd) -> Result<()> {
    let _env = setup_test_env()?;
    let runtime = tempfile::tempdir()?;
    let socket = runtime.path().join("server.sock");
    let debug_socket = runtime.path().join("debug.sock");
    let provider = ReconnectProvider::default();
    let server = server::Server::new_with_paths(
        Arc::new(provider.clone()),
        socket.clone(),
        debug_socket.clone(),
    );
    let handle = tokio::spawn(async move { server.run().await });
    let result = async {
        wait_for_server_ready(&socket, &debug_socket).await?;

        // Capability probing must not allocate a session. Old Pong payloads
        // deserialize but lack the feature marker required by the SSH bridge.
        let connection = server::connect_socket(&socket).await?;
        let (reader, mut writer) = connection.into_split();
        let mut reader = BufReader::new(reader);
        send_native(&mut writer, serde_json::json!({"type":"ping", "id":91})).await?;
        native_until(&mut reader, |e| {
            matches!(
                e,
                ServerEvent::Pong {
                    id: 91,
                    native_ssh_protocol: Some(1),
                }
            )
        })
        .await?;
        drop((reader, writer));

        let not_a_directory = runtime.path().join("not-a-directory");
        std::fs::write(&not_a_directory, "file")?;
        for invalid in [runtime.path().join("missing"), not_a_directory] {
            let connection = server::connect_socket(&socket).await?;
            let (reader, mut writer) = connection.into_split();
            let mut reader = BufReader::new(reader);
            send_native(
                &mut writer,
                serde_json::json!({
                    "type":"subscribe", "id":92, "working_dir":invalid,
                    "continue_on_disconnect":true,
                }),
            )
            .await?;
            let mut line = String::new();
            timeout(Duration::from_secs(5), reader.read_line(&mut line)).await??;
            anyhow::ensure!(
                matches!(serde_json::from_str::<ServerEvent>(&line)?,
                    ServerEvent::Error {id:92, message, ..}
                        if message.contains("must exist and be a directory on the server")
                ),
                "invalid remote cwd must fail before publishing a session"
            );
        }

        let connection = server::connect_socket(&socket).await?;
        let (reader, mut writer) = connection.into_split();
        let mut reader = BufReader::new(reader);
        send_native(
            &mut writer,
            serde_json::json!({
                "type":"subscribe", "id":1, "working_dir":std::env::current_dir()?,
                "continue_on_disconnect":true, "client_instance_id":"remote-test",
            }),
        )
        .await?;
        let events =
            native_until(&mut reader, |e| matches!(e, ServerEvent::Done { id: 1 })).await?;
        let session_id = events
            .iter()
            .find_map(|e| match e {
                ServerEvent::SessionId { session_id } => Some(session_id.clone()),
                _ => None,
            })
            .context("subscribe did not identify session")?;
        send_native(
            &mut writer,
            serde_json::json!({
                "type":"message", "id":2, "content":"finish exactly once",
            }),
        )
        .await?;
        native_until(&mut reader, |e| matches!(e, ServerEvent::TextDelta { .. })).await?;
        drop((reader, writer)); // Actual transport EOF while the provider is gated.

        if !matches!(end, RemoteTurnEnd::Detached) {
            // Repeated attachment loss must not destroy the original owner's
            // task. No client resends the prompt, and provider calls stay at one.
            for attempt in 0..2 {
                let connection = server::connect_socket(&socket).await?;
                let (reader, mut writer) = connection.into_split();
                let mut reader = BufReader::new(reader);
                send_native(
                    &mut writer,
                    serde_json::json!({
                        "type":"subscribe", "id":3, "working_dir":std::env::current_dir()?,
                        "target_session_id":session_id, "continue_on_disconnect":true,
                        "client_instance_id":"remote-test", "client_has_local_history":false,
                    }),
                )
                .await?;
                let events =
                    native_until(&mut reader, |e| matches!(e, ServerEvent::Done { id: 3 })).await?;
                anyhow::ensure!(
                    events.iter().any(|e| matches!(e,
                        ServerEvent::History {session_id: id, activity: Some(activity), ..}
                        if id == &session_id && activity.is_processing
                    )),
                    "reattach must identify the same busy session"
                );
                if attempt == 1 {
                    if matches!(end, RemoteTurnEnd::Cancel) {
                        send_native(&mut writer, serde_json::json!({"type":"cancel", "id":4}))
                            .await?;
                        native_until(&mut reader, |e| matches!(e, ServerEvent::Done { id: 2 }))
                            .await?;
                    } else {
                        provider.finish.notify_one();
                        let events =
                            native_until(&mut reader, |e| matches!(e, ServerEvent::Done { id: 2 }))
                                .await?;
                        anyhow::ensure!(
                            events.iter().any(|e| matches!(e,
                                ServerEvent::TextDelta {text} if text == "after reconnect"
                            )),
                            "new attachment must receive the continuing stream"
                        );
                        let end_pos = events
                            .iter()
                            .position(|e| matches!(e, ServerEvent::MessageEnd { .. }));
                        anyhow::ensure!(
                            end_pos.is_some_and(|i| i < events.len() - 1),
                            "MessageEnd must precede Done"
                        );
                    }
                    // A successor still owns the session after the detached
                    // supervisor completes. Its cleanup must not close it.
                    send_native(
                        &mut writer,
                        serde_json::json!({"type":"get_history", "id":5}),
                    )
                    .await?;
                    native_until(&mut reader, |e| {
                        matches!(e,
                            ServerEvent::History {id:5, session_id: id, ..} if id == &session_id
                        )
                    })
                    .await?;
                    anyhow::ensure!(
                        !matches!(
                            Session::load(&session_id)?.status,
                            SessionStatus::Closed | SessionStatus::Crashed { .. }
                        ),
                        "old supervisor must not close its live successor"
                    );
                }
                drop((reader, writer));
            }
        } else {
            provider.finish.notify_one();
        }

        timeout(Duration::from_secs(10), async {
            loop {
                let session = Session::load(&session_id)?;
                if session.status == SessionStatus::Closed {
                    let transcript = serde_json::to_string(&session)?;
                    if matches!(end, RemoteTurnEnd::Cancel) {
                        anyhow::ensure!(
                            !transcript.contains("after reconnect"),
                            "cancel must stop provider stream"
                        );
                    } else {
                        anyhow::ensure!(
                            transcript.contains("after reconnect"),
                            "completed work must persist"
                        );
                    }
                    break Ok::<_, anyhow::Error>(());
                }
                anyhow::ensure!(
                    !matches!(session.status, SessionStatus::Crashed { .. }),
                    "opted-in disconnected turn must not crash"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        if matches!(end, RemoteTurnEnd::Detached) {
            let connection = server::connect_socket(&socket).await?;
            let (reader, mut writer) = connection.into_split();
            let mut reader = BufReader::new(reader);
            send_native(
                &mut writer,
                serde_json::json!({
                    "type":"subscribe", "id":6, "working_dir":std::env::current_dir()?,
                    "target_session_id":session_id, "continue_on_disconnect":true,
                }),
            )
            .await?;
            let events =
                native_until(&mut reader, |e| matches!(e, ServerEvent::Done { id: 6 })).await?;
            anyhow::ensure!(
                events.iter().any(|e| match e {
                    ServerEvent::History {
                        session_id: id,
                        messages,
                        ..
                    } if id == &session_id => serde_json::to_string(messages)
                        .is_ok_and(|text| text.contains("after reconnect")),
                    _ => false,
                }),
                "late reconnect must restore the completed detached transcript"
            );
            drop((reader, writer));
        }
        anyhow::ensure!(
            provider.calls.load(std::sync::atomic::Ordering::SeqCst) == 1,
            "reconnection must never replay a prompt"
        );
        Ok(())
    }
    .await;
    abort_server_and_cleanup(&handle, &socket, &debug_socket);
    result
}

#[tokio::test]
async fn remote_disconnect_reattaches_twice_and_finishes_original_turn() -> Result<()> {
    remote_disconnect_turn(RemoteTurnEnd::Reattach).await
}

#[tokio::test]
async fn remote_disconnect_finishes_without_reattach_and_closes_session() -> Result<()> {
    remote_disconnect_turn(RemoteTurnEnd::Detached).await
}

#[tokio::test]
async fn remote_disconnect_reattaches_and_cancels_original_turn() -> Result<()> {
    remote_disconnect_turn(RemoteTurnEnd::Cancel).await
}

#[tokio::test]
async fn remote_disconnect_while_idle_closes_session() -> Result<()> {
    abrupt_disconnect(Turn::Idle, true).await
}

#[tokio::test]
async fn remote_disconnect_after_done_closes_session() -> Result<()> {
    abrupt_disconnect(Turn::Completed, true).await
}

#[tokio::test]
async fn native_ping_ping_subscribe_history_keeps_one_socket() -> Result<()> {
    let _env = setup_test_env()?;
    let runtime = tempfile::tempdir()?;
    let socket = runtime.path().join("server.sock");
    let debug_socket = runtime.path().join("debug.sock");
    let server = server::Server::new_with_paths(
        Arc::new(MockProvider::new()),
        socket.clone(),
        debug_socket.clone(),
    );
    let handle = tokio::spawn(async move { server.run().await });
    let result = async {
        wait_for_server_ready(&socket, &debug_socket).await?;
        let connection = server::connect_socket(&socket).await?;
        let (reader, mut writer) = connection.into_split();
        let mut reader = BufReader::new(reader);
        for ping_id in [71, 72] {
            send_native(
                &mut writer,
                serde_json::json!({"type":"ping", "id":ping_id}),
            )
            .await?;
            native_until(&mut reader, |event| {
                matches!(event,
                    ServerEvent::Pong {id, native_ssh_protocol: Some(1)} if *id == ping_id
                )
            })
            .await?;
        }
        send_native(
            &mut writer,
            serde_json::json!({
                "type":"subscribe", "id":73, "working_dir":std::env::current_dir()?,
                "continue_on_disconnect":true,
            }),
        )
        .await?;
        let events = native_until(&mut reader, |event| {
            matches!(event, ServerEvent::Done { id: 73 })
        })
        .await?;
        let session_id = events
            .iter()
            .find_map(|event| match event {
                ServerEvent::SessionId { session_id } => Some(session_id.clone()),
                _ => None,
            })
            .context("subscribe after capability probes must create a session")?;
        send_native(
            &mut writer,
            serde_json::json!({"type":"get_history", "id":74}),
        )
        .await?;
        native_until(&mut reader, |event| {
            matches!(event,
                ServerEvent::History {id:74, session_id: id, ..} if id == &session_id
            )
        })
        .await?;
        drop((reader, writer));
        Ok(())
    }
    .await;
    abort_server_and_cleanup(&handle, &socket, &debug_socket);
    result
}
