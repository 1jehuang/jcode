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

async fn abrupt_desktop_disconnect(turn: Turn) -> Result<()> {
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
    abrupt_desktop_disconnect(Turn::Idle).await
}

#[tokio::test]
async fn desktop_force_quit_after_done_preserves_completed_session() -> Result<()> {
    abrupt_desktop_disconnect(Turn::Completed).await
}

#[tokio::test]
async fn desktop_force_quit_while_streaming_marks_session_crashed() -> Result<()> {
    abrupt_desktop_disconnect(Turn::Streaming).await
}
