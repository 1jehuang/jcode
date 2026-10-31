use super::{connect_socket, debug_socket_path, socket_path};
use crate::protocol::{HistoryMessage, Request, ServerEvent, TranscriptMode};
use crate::transport::{ReadHalf, WriteHalf};
use anyhow::Result;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Client for connecting to a running server
pub struct Client {
    reader: BufReader<ReadHalf>,
    writer: WriteHalf,
    next_id: u64,
}

impl Client {
    pub async fn connect() -> Result<Self> {
        Self::connect_with_path(socket_path()).await
    }

    pub async fn connect_with_path(path: PathBuf) -> Result<Self> {
        let stream = connect_socket(&path).await?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        })
    }

    pub async fn connect_debug() -> Result<Self> {
        Self::connect_debug_with_path(debug_socket_path()).await
    }

    pub async fn connect_debug_with_path(path: PathBuf) -> Result<Self> {
        let stream = connect_socket(&path).await?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        })
    }

    /// Send a message and return immediately (events come via read_event)
    pub async fn send_message(&mut self, content: &str) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Message {
            id,
            content: content.to_string(),
            images: vec![],
            system_reminder: None,
            active_skill: None,
            no_reply: false,
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    /// Subscribe to events
    pub async fn subscribe(&mut self) -> Result<u64> {
        self.subscribe_with_info(None, None, None, false, false)
            .await
    }

    pub async fn subscribe_with_info(
        &mut self,
        working_dir: Option<String>,
        selfdev: Option<bool>,
        target_session_id: Option<String>,
        client_has_local_history: bool,
        allow_session_takeover: bool,
    ) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let working_dir = match working_dir {
            Some(working_dir) => working_dir,
            None => std::env::current_dir()?.to_string_lossy().into_owned(),
        };

        let request = Request::Subscribe {
            system_prompt: None,
            supports_pdf_panels: false,
            supports_user_response: false,
            supports_server_failover: false,
            id,
            working_dir: Some(working_dir),
            selfdev,
            target_session_id,
            client_instance_id: None,
            client_has_local_history,
            history_base: None,
            allow_session_takeover,
            crash_on_disconnect: false,
            continue_on_disconnect: false,
            terminal_env: crate::terminal_launch::snapshot_client_terminal_env(),
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    /// Read the next event from the server
    pub async fn read_event(&mut self) -> Result<ServerEvent> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("Server disconnected");
        }
        let event: ServerEvent = serde_json::from_str(&line)?;
        Ok(event)
    }

    pub async fn ping(&mut self) -> Result<bool> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Ping { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;

        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("Server disconnected");
            }
            let event: ServerEvent = serde_json::from_str(&line)?;

            match event {
                ServerEvent::Pong { id: pong_id, .. } => return Ok(pong_id == id),
                ServerEvent::Ack { id: ack_id } if ack_id == id => continue,
                ServerEvent::Error { id: error_id, .. } if error_id == id => return Ok(false),
                _ => return Ok(false),
            }
        }
    }

    pub async fn get_state(&mut self) -> Result<ServerEvent> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::GetState { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;

        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("Server disconnected");
        }
        let event: ServerEvent = serde_json::from_str(&line)?;
        Ok(event)
    }

    pub async fn clear(&mut self) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Clear { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(())
    }

    pub async fn get_history(&mut self) -> Result<Vec<HistoryMessage>> {
        let event = self.get_history_event().await?;
        match event {
            ServerEvent::History { messages, .. } => Ok(messages),
            _ => Ok(Vec::new()),
        }
    }

    /// Request history and wait for the matching response, skipping unrelated broadcasts.
    pub async fn get_history_event(&mut self) -> Result<ServerEvent> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::GetHistory { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        for _ in 0..10 {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("Server disconnected");
            }
            let event: ServerEvent = serde_json::from_str(&line)?;
            match event {
                ServerEvent::History {
                    id: response_id, ..
                }
                | ServerEvent::Error {
                    id: response_id, ..
                } if response_id == id => return Ok(event),
                _ => continue,
            }
        }

        Ok(ServerEvent::Error {
            id,
            message: "History response not received".to_string(),
            retry_after_secs: None,
        })
    }

    pub async fn resume_session(&mut self, session_id: &str) -> Result<u64> {
        self.resume_session_with_options(session_id, false, false)
            .await
    }

    pub async fn resume_session_with_options(
        &mut self,
        session_id: &str,
        client_has_local_history: bool,
        allow_session_takeover: bool,
    ) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::ResumeSession {
            id,
            session_id: session_id.to_string(),
            client_instance_id: None,
            client_has_local_history,
            allow_session_takeover,
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    pub async fn notify_session(&mut self, session_id: &str, message: &str) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::NotifySession {
            id,
            session_id: session_id.to_string(),
            message: message.to_string(),
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    /// Ask the server to continue every live session that was interrupted and
    /// would auto-resume on a reload. Returns the request id so callers can
    /// correlate the `ResumeAllResult` event.
    pub async fn resume_all_sessions(&mut self) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::ResumeAllSessions { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    pub async fn send_transcript(
        &mut self,
        text: &str,
        mode: TranscriptMode,
        session_id: Option<String>,
    ) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Transcript {
            id,
            text: text.to_string(),
            mode,
            session_id,
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    pub async fn reload(&mut self) -> Result<()> {
        self.reload_with_force(true).await?;
        Ok(())
    }

    /// Request a graceful, conditional reload: when `force` is false the server
    /// only reloads if it has a strictly-newer reload candidate binary. Used by
    /// `jcode server reload` so an upgrade is picked up without risking a
    /// downgrade. Returns the request id so callers can correlate events.
    pub async fn reload_with_force(&mut self, force: bool) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Reload { id, force };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    pub async fn cycle_model(&mut self, direction: i8) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::CycleModel { id, direction };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    pub async fn refresh_models(&mut self) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::RefreshModels { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    pub async fn notify_auth_changed(&mut self) -> Result<u64> {
        self.notify_auth_changed_for_provider(None).await
    }

    pub async fn notify_auth_changed_for_provider(
        &mut self,
        provider: Option<&str>,
    ) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::NotifyAuthChanged {
            id,
            provider: provider.map(str::to_string),
            auth: None,
            prefer_strongest: false,
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    pub async fn debug_command(&mut self, command: &str, session_id: Option<&str>) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::DebugCommand {
            id,
            command: command.to_string(),
            session_id: session_id.map(|s| s.to_string()),
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }
}

#[cfg(test)]
mod history_response_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// A status broadcast can arrive before the requested history response.
    #[tokio::test]
    async fn history_request_ignores_interleaved_status() {
        let (stream, server) = crate::transport::stream_pair().expect("stream pair");
        let (reader, writer) = stream.into_split();
        let mut client = Client {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        };
        let (server_reader, mut server_writer) = server.into_split();
        let server_task = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let mut request = String::new();
            reader.read_line(&mut request).await.expect("read request");
            let parsed: Request = serde_json::from_str(&request).expect("parse request");
            assert!(matches!(parsed, Request::GetHistory { id: 1 }));
            server_writer.write_all(b"{\"type\":\"swarm_status\",\"members\":[]}\n{\"type\":\"history\",\"id\":99,\"session_id\":\"stale\",\"messages\":[]}\n{\"type\":\"history\",\"id\":1,\"session_id\":\"session\",\"messages\":[]}\n").await.expect("write responses");
            request.clear();
            reader
                .read_line(&mut request)
                .await
                .expect("read second request");
            let parsed: Request = serde_json::from_str(&request).expect("parse second request");
            assert!(matches!(parsed, Request::GetHistory { id: 2 }));
            server_writer
                .write_all(b"{\"type\":\"error\",\"id\":2,\"message\":\"history unavailable\"}\n")
                .await
                .expect("write error");
        });
        let response = client.get_history_event().await.expect("history response");
        assert!(
            matches!(response, ServerEvent::History { id: 1, .. }),
            "unexpected response: {response:?}"
        );
        let error = client.get_history_event().await.expect("error response");
        assert!(
            matches!(error, ServerEvent::Error { id: 2, .. }),
            "unexpected response: {error:?}"
        );
        server_task.await.expect("server task");
    }
}
