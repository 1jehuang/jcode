use crate::model::{BackendCommand, BackendEvent, ChatEntry, RuntimeFeature};
use anyhow::{Context, Result};
use jcode::protocol::{FeatureToggle, NotificationType, Request, ServerEvent};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, Mutex};

#[derive(Clone)]
pub struct BackendBridge {
    command_tx: mpsc::UnboundedSender<BackendCommand>,
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<BackendEvent>>>,
}

impl BackendBridge {
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            run_backend_loop(command_rx, event_tx).await;
        });

        Self {
            command_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
        }
    }

    pub fn send(&self, command: BackendCommand) {
        let _ = self.command_tx.send(command);
    }

    pub async fn next_event(&self) -> Option<BackendEvent> {
        let mut rx = self.event_rx.lock().await;
        rx.recv().await
    }
}

async fn run_backend_loop(
    mut command_rx: mpsc::UnboundedReceiver<BackendCommand>,
    event_tx: mpsc::UnboundedSender<BackendEvent>,
) {
    loop {
        if command_rx.is_closed() {
            return;
        }

        let socket = jcode::server::socket_path();
        match jcode::server::connect_socket(&socket).await {
            Ok(stream) => {
                let _ = event_tx.send(BackendEvent::Connected);
                match run_connection(stream, &mut command_rx, &event_tx).await {
                    Ok(()) => {
                        if command_rx.is_closed() {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = event_tx.send(BackendEvent::Disconnected {
                            reason: err.to_string(),
                        });
                    }
                }
            }
            Err(err) => {
                let _ = event_tx.send(BackendEvent::Disconnected {
                    reason: format!("{} ({})", socket.display(), err),
                });
            }
        }

        if command_rx.is_closed() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn run_connection(
    stream: tokio::net::UnixStream,
    command_rx: &mut mpsc::UnboundedReceiver<BackendCommand>,
    event_tx: &mpsc::UnboundedSender<BackendEvent>,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut next_id = 1u64;

    send_request(
        &mut write_half,
        Request::Subscribe {
            id: take_id(&mut next_id),
            working_dir: std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().to_string()),
            selfdev: Some(false),
        },
    )
    .await
    .context("subscribe request failed")?;

    send_request(
        &mut write_half,
        Request::GetHistory {
            id: take_id(&mut next_id),
        },
    )
    .await
    .context("history request failed")?;

    let mut line = String::new();

    loop {
        tokio::select! {
            maybe_command = command_rx.recv() => {
                let Some(command) = maybe_command else {
                    return Ok(());
                };

                if let Some(request) = command_to_request(command, &mut next_id) {
                    send_request(&mut write_half, request).await?;
                }
            }
            read_res = reader.read_line(&mut line) => {
                match read_res {
                    Ok(0) => anyhow::bail!("server disconnected"),
                    Ok(_) => {
                        match serde_json::from_str::<ServerEvent>(&line) {
                            Ok(server_event) => {
                                if let Some(event) = map_server_event(server_event) {
                                    let _ = event_tx.send(event);
                                }
                            }
                            Err(err) => {
                                let _ = event_tx.send(BackendEvent::Status(format!("Invalid server event: {}", err)));
                            }
                        }
                        line.clear();
                    }
                    Err(err) => anyhow::bail!("read error: {}", err),
                }
            }
        }
    }
}

async fn send_request(writer: &mut OwnedWriteHalf, request: Request) -> Result<()> {
    let json = serde_json::to_string(&request)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    Ok(())
}

fn command_to_request(command: BackendCommand, next_id: &mut u64) -> Option<Request> {
    let id = take_id(next_id);

    match command {
        BackendCommand::SendMessage(content) => {
            let content = content.trim().to_string();
            if content.is_empty() {
                return None;
            }
            Some(Request::Message { id, content })
        }
        BackendCommand::Cancel => Some(Request::Cancel { id }),
        BackendCommand::SoftInterrupt { content, urgent } => {
            let content = content.trim().to_string();
            if content.is_empty() {
                return None;
            }
            Some(Request::SoftInterrupt {
                id,
                content,
                urgent,
            })
        }
        BackendCommand::Clear => Some(Request::Clear { id }),
        BackendCommand::Reload => Some(Request::Reload { id }),
        BackendCommand::ResumeSession(session_id) => {
            let session_id = session_id.trim().to_string();
            if session_id.is_empty() {
                return None;
            }
            Some(Request::ResumeSession { id, session_id })
        }
        BackendCommand::CycleModel(direction) => Some(Request::CycleModel { id, direction }),
        BackendCommand::SetModel(model) => {
            let model = model.trim().to_string();
            if model.is_empty() {
                return None;
            }
            Some(Request::SetModel { id, model })
        }
        BackendCommand::SetFeature { feature, enabled } => Some(Request::SetFeature {
            id,
            feature: match feature {
                RuntimeFeature::Memory => FeatureToggle::Memory,
                RuntimeFeature::Swarm => FeatureToggle::Swarm,
            },
            enabled,
        }),
    }
}

fn take_id(next_id: &mut u64) -> u64 {
    let out = *next_id;
    *next_id = next_id.saturating_add(1);
    out
}

fn map_server_event(event: ServerEvent) -> Option<BackendEvent> {
    match event {
        ServerEvent::Ack { .. } => None,
        ServerEvent::TextDelta { text } => Some(BackendEvent::TextDelta(text)),
        ServerEvent::ToolStart { id, name } => Some(BackendEvent::ToolStart { id, name }),
        ServerEvent::ToolInput { .. } => None,
        ServerEvent::ToolExec { id, name } => Some(BackendEvent::ToolExec { id, name }),
        ServerEvent::ToolDone {
            id,
            name,
            output,
            error,
        } => Some(BackendEvent::ToolDone {
            id,
            name,
            output,
            error,
        }),
        ServerEvent::TokenUsage {
            input,
            output,
            cache_read_input,
            cache_creation_input,
        } => Some(BackendEvent::TokenUsage {
            input,
            output,
            cache_read_input,
            cache_creation_input,
        }),
        ServerEvent::UpstreamProvider { provider } => {
            Some(BackendEvent::UpstreamProvider(provider))
        }
        ServerEvent::SwarmStatus { members } => Some(BackendEvent::SwarmStatus(
            members
                .into_iter()
                .map(|member| {
                    let detail = member.detail.unwrap_or_default();
                    format!(
                        "{}:{}{}",
                        member.friendly_name.unwrap_or(member.session_id),
                        member.status,
                        if detail.is_empty() {
                            "".to_string()
                        } else {
                            format!(" ({})", detail)
                        }
                    )
                })
                .collect(),
        )),
        ServerEvent::SoftInterruptInjected {
            content,
            point,
            tools_skipped,
        } => Some(BackendEvent::SoftInterruptInjected {
            content,
            point,
            tools_skipped,
        }),
        ServerEvent::MemoryInjected {
            count,
            prompt_chars,
            computed_age_ms,
            ..
        } => Some(BackendEvent::MemoryInjected {
            count,
            prompt_chars,
            computed_age_ms,
        }),
        ServerEvent::Done { .. } => Some(BackendEvent::Done),
        ServerEvent::Error { message, .. } => Some(BackendEvent::Error(message)),
        ServerEvent::Pong { .. } => None,
        ServerEvent::State { .. } => None,
        ServerEvent::DebugResponse { output, .. } => Some(BackendEvent::Status(output)),
        ServerEvent::McpStatus { servers } => Some(BackendEvent::Status(format!(
            "MCP connected servers: {}",
            if servers.is_empty() {
                "none".to_string()
            } else {
                servers.join(", ")
            }
        ))),
        ServerEvent::ClientDebugRequest { command, .. } => Some(BackendEvent::Status(format!(
            "Debug request forwarded to client: {}",
            command
        ))),
        ServerEvent::SessionId { session_id } => Some(BackendEvent::SessionAssigned(session_id)),
        ServerEvent::History {
            session_id,
            messages,
            provider_name,
            provider_model,
            available_models,
            mcp_servers,
            skills,
            total_tokens,
            client_count,
            is_canary,
            server_version,
            server_name,
            server_icon,
            server_has_update,
            ..
        } => Some(BackendEvent::HistoryLoaded {
            session_id,
            messages: messages
                .into_iter()
                .map(|msg| ChatEntry {
                    role: msg.role,
                    content: msg.content,
                    tool_calls: msg.tool_calls.unwrap_or_default(),
                })
                .collect(),
            provider_name,
            provider_model,
            available_models,
            mcp_servers,
            skills,
            total_tokens,
            client_count,
            is_canary,
            server_version,
            server_name,
            server_icon,
            server_has_update,
        }),
        ServerEvent::Reloading { new_socket } => {
            if let Some(path) = new_socket {
                Some(BackendEvent::Status(format!(
                    "Server requested socket move: {}",
                    PathBuf::from(path).display()
                )))
            } else {
                Some(BackendEvent::Reloading)
            }
        }
        ServerEvent::ReloadProgress {
            step,
            message,
            success,
            ..
        } => Some(BackendEvent::ReloadProgress {
            step,
            message,
            success,
        }),
        ServerEvent::ModelChanged {
            model,
            provider_name,
            error,
            ..
        } => Some(BackendEvent::ModelChanged {
            model,
            provider_name,
            error,
        }),
        ServerEvent::Notification {
            from_session,
            from_name,
            notification_type,
            message,
        } => Some(BackendEvent::Notification(format_notification(
            &from_session,
            from_name.as_deref(),
            &notification_type,
            &message,
        ))),
        ServerEvent::CommContext { id, entries } => Some(BackendEvent::Status(format!(
            "comm_context id={} entries={}",
            id,
            entries.len()
        ))),
        ServerEvent::CommMembers { id, members } => Some(BackendEvent::Status(format!(
            "comm_members id={} count={}",
            id,
            members.len()
        ))),
        ServerEvent::CommSummaryResponse {
            id,
            session_id,
            tool_calls,
        } => Some(BackendEvent::Status(format!(
            "comm_summary id={} session={} tool_calls={}",
            id,
            session_id,
            tool_calls.len()
        ))),
        ServerEvent::CommContextHistory {
            id,
            session_id,
            messages,
        } => Some(BackendEvent::Status(format!(
            "comm_context_history id={} session={} messages={}",
            id,
            session_id,
            messages.len()
        ))),
        ServerEvent::CommSpawnResponse {
            id,
            session_id,
            new_session_id,
        } => Some(BackendEvent::Status(format!(
            "comm_spawn id={} source={} new={}",
            id, session_id, new_session_id
        ))),
    }
}

fn format_notification(
    from_session: &str,
    from_name: Option<&str>,
    notification_type: &NotificationType,
    message: &str,
) -> String {
    let sender = from_name.unwrap_or(from_session);

    let kind = match notification_type {
        NotificationType::FileConflict { path, operation } => {
            format!("file_conflict {} {}", operation, path)
        }
        NotificationType::SharedContext { key, .. } => format!("shared_context {}", key),
        NotificationType::Message { scope, channel } => format!(
            "message scope={} channel={}",
            scope.clone().unwrap_or_else(|| "direct".to_string()),
            channel.clone().unwrap_or_else(|| "-".to_string())
        ),
    };

    format!("{} [{}]: {}", sender, kind, message)
}
