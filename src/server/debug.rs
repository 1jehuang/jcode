#![cfg_attr(
    test,
    allow(clippy::await_holding_lock, clippy::items_after_test_module)
)]

use super::debug_ambient::maybe_handle_ambient_command;
use super::debug_command_exec::{execute_debug_command, resolve_debug_session};
use super::debug_events::{
    maybe_handle_event_query_command, maybe_handle_event_subscription_command,
};
use super::debug_help::{debug_help_text, parse_namespaced_command, swarm_debug_help_text};
use super::debug_jobs::{DebugJob, maybe_handle_job_command};
use super::debug_server_state::maybe_handle_server_state_command;
use super::debug_session_admin::maybe_handle_session_admin_command;
use super::debug_swarm_read::maybe_handle_swarm_read_command;
use super::debug_swarm_write::{DebugSwarmWriteContext, maybe_handle_swarm_write_command};
use super::debug_testers::execute_tester_command;
use super::{
    FileAccess, ServerIdentity, SharedContext, SwarmEvent, SwarmMember, VersionedPlan,
    debug_control_allowed, fanout_session_event,
};
use crate::agent::Agent;
use crate::ambient_runner::AmbientRunnerHandle;
use crate::protocol::{Request, ServerEvent, TranscriptMode, decode_request, encode_event};
use crate::provider::Provider;
use crate::transport::Stream;
use anyhow::Result;
use jcode_agent_runtime::InterruptSignal;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};

type ChannelSubscriptions = Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

#[derive(Default)]
pub(super) struct ClientDebugState {
    pub(super) active_id: Option<String>,
    pub(super) clients: HashMap<String, mpsc::UnboundedSender<(u64, String)>>,
}

#[derive(Clone, Debug)]
pub(super) struct ClientConnectionInfo {
    pub(super) client_id: String,
    pub(super) session_id: String,
    pub(super) client_instance_id: Option<String>,
    pub(super) debug_client_id: Option<String>,
    pub(super) connected_at: Instant,
    pub(super) last_seen: Instant,
    pub(super) is_processing: bool,
    pub(super) current_tool_name: Option<String>,
    pub(super) disconnect_tx: mpsc::UnboundedSender<()>,
}

impl ClientDebugState {
    pub(super) fn register(&mut self, client_id: String, tx: mpsc::UnboundedSender<(u64, String)>) {
        self.active_id = Some(client_id.clone());
        self.clients.insert(client_id, tx);
    }

    pub(super) fn unregister(&mut self, client_id: &str) {
        self.clients.remove(client_id);
        if self.active_id.as_deref() == Some(client_id) {
            self.active_id = self.clients.keys().next().cloned();
        }
    }

    pub(super) fn active_sender(
        &mut self,
    ) -> Option<(String, mpsc::UnboundedSender<(u64, String)>)> {
        if let Some(active_id) = self.active_id.clone()
            && let Some(tx) = self.clients.get(&active_id)
        {
            return Some((active_id, tx.clone()));
        }
        if let Some((id, tx)) = self.clients.iter().next() {
            let id = id.clone();
            self.active_id = Some(id.clone());
            return Some((id, tx.clone()));
        }
        None
    }

    pub(super) fn sender_for_id(
        &self,
        client_id: &str,
    ) -> Option<mpsc::UnboundedSender<(u64, String)>> {
        self.clients.get(client_id).cloned()
    }
}

async fn resolve_client_debug_sender(
    requested_session: Option<&str>,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
) -> Result<(String, mpsc::UnboundedSender<(u64, String)>)> {
    if let Some(session_id) = requested_session.filter(|value| !value.trim().is_empty()) {
        let active_debug_id = client_debug_state.read().await.active_id.clone();
        let target_debug_id = {
            let connections = client_connections.read().await;
            connections
                .values()
                .filter(|info| info.session_id == session_id)
                .filter_map(|info| {
                    info.debug_client_id.as_ref().map(|debug_client_id| {
                        let is_active =
                            active_debug_id.as_deref() == Some(debug_client_id.as_str());
                        (debug_client_id.clone(), info.last_seen, is_active)
                    })
                })
                .max_by(|left, right| left.1.cmp(&right.1).then_with(|| left.2.cmp(&right.2)))
                .map(|(debug_client_id, _, _)| debug_client_id)
        };

        let Some(debug_client_id) = target_debug_id else {
            anyhow::bail!(
                "Session '{}' does not have a connected TUI client for client: debug commands",
                session_id
            );
        };

        let sender = client_debug_state
            .read()
            .await
            .sender_for_id(&debug_client_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Session '{}' debug client '{}' is not currently available",
                    session_id,
                    debug_client_id
                )
            })?;

        return Ok((debug_client_id, sender));
    }

    let (client_id, sender) = {
        let mut debug_state = client_debug_state.write().await;
        debug_state
            .active_sender()
            .ok_or_else(|| anyhow::anyhow!("No TUI client connected"))?
    };
    Ok((client_id, sender))
}

async fn resolve_transcript_target_session(
    requested_session: Option<String>,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Result<String> {
    let live_sessions: std::collections::HashSet<String> = swarm_members
        .read()
        .await
        .iter()
        .filter(|(_, member)| !member.is_headless && !member.event_txs.is_empty())
        .map(|(session_id, _)| session_id.clone())
        .collect();

    if let Some(session_id) = requested_session.filter(|value| !value.trim().is_empty()) {
        if !live_sessions.contains(&session_id) {
            anyhow::bail!(
                "Session '{}' does not have a connected TUI client for transcript injection",
                session_id
            );
        }
        return Ok(session_id);
    }

    if let Ok(Some(session_id)) = crate::dictation::focused_jcode_session()
        && live_sessions.contains(&session_id)
    {
        return Ok(session_id);
    }

    if let Ok(Some(session_id)) = crate::dictation::last_focused_session()
        && live_sessions.contains(&session_id)
    {
        return Ok(session_id);
    }

    let active_debug_id = client_debug_state.read().await.active_id.clone();
    let connections = client_connections.read().await;

    connections
        .values()
        .filter(|info| live_sessions.contains(&info.session_id))
        .max_by(|left, right| {
            left.last_seen
                .cmp(&right.last_seen)
                .then_with(|| {
                    let left_is_active =
                        active_debug_id.as_deref() == left.debug_client_id.as_deref();
                    let right_is_active =
                        active_debug_id.as_deref() == right.debug_client_id.as_deref();
                    left_is_active.cmp(&right_is_active)
                })
        })
        .map(|info| info.session_id.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Transcript target could not be resolved from focused window, last-focused session, or any live TUI client"
            )
        })
}

pub(super) async fn inject_transcript(
    id: u64,
    text: String,
    mode: TranscriptMode,
    requested_session: Option<String>,
    client_connections: &Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    client_debug_state: &Arc<RwLock<ClientDebugState>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> Result<ServerEvent> {
    let session_id = resolve_transcript_target_session(
        requested_session,
        client_connections,
        client_debug_state,
        swarm_members,
    )
    .await?;

    let delivered = fanout_session_event(
        swarm_members,
        &session_id,
        ServerEvent::Transcript { text, mode },
    )
    .await
        > 0;

    if !delivered {
        anyhow::bail!("Failed to deliver transcript to session '{}'", session_id);
    }

    Ok(ServerEvent::Done { id })
}

#[expect(
    clippy::too_many_arguments,
    reason = "debug client wiring fans out across sessions, swarms, files, channels, jobs, and transport state"
)]
pub(super) async fn handle_debug_client(
    stream: Stream,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    is_processing: Arc<RwLock<bool>>,
    session_id: Arc<RwLock<String>>,
    provider: Arc<dyn Provider>,
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    files_touched_by_session: Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    channel_subscriptions: ChannelSubscriptions,
    channel_subscriptions_by_session: ChannelSubscriptions,
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    server_identity: ServerIdentity,
    server_start_time: std::time::Instant,
    ambient_runner: Option<AmbientRunnerHandle>,
    mcp_pool: Option<Arc<crate::mcp::SharedMcpPool>>,
    shutdown_signals: Arc<RwLock<HashMap<String, InterruptSignal>>>,
    soft_interrupt_queues: super::SessionInterruptQueues,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }

        let request = match decode_request(&line) {
            Ok(r) => r,
            Err(e) => {
                let event = ServerEvent::Error {
                    id: 0,
                    message: format!("Invalid request: {}", e),
                    retry_after_secs: None,
                };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
                continue;
            }
        };

        match request {
            Request::Ping { id } => {
                let event = ServerEvent::Pong { id };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }

            Request::GetState { id } => {
                let current_session_id = session_id.read().await.clone();
                let sessions = sessions.read().await;
                let message_count = sessions.len();

                let event = ServerEvent::State {
                    id,
                    session_id: current_session_id,
                    message_count,
                    is_processing: *is_processing.read().await,
                };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }

            Request::Transcript {
                id,
                text,
                mode,
                session_id: requested_session,
            } => {
                let event = match inject_transcript(
                    id,
                    text,
                    mode,
                    requested_session,
                    &client_connections,
                    &client_debug_state,
                    &swarm_members,
                )
                .await
                {
                    Ok(event) => event,
                    Err(err) => ServerEvent::Error {
                        id,
                        message: err.to_string(),
                        retry_after_secs: None,
                    },
                };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }

            Request::DebugCommand {
                id,
                command,
                session_id: requested_session,
            } => {
                if !debug_control_allowed() {
                    let event = ServerEvent::Error {
                        id,
                        message: "Debug control is disabled. Set JCODE_DEBUG_CONTROL=1, enable display.debug_socket, or start the shared server from a self-dev session.".to_string(),
                        retry_after_secs: None,
                    };
                    let json = encode_event(&event);
                    writer.write_all(json.as_bytes()).await?;
                    continue;
                }

                // Parse namespaced command
                let (namespace, cmd) = parse_namespaced_command(&command);

                let result = match namespace {
                    "client" => {
                        // Forward to TUI client
                        let mut response_rx = client_debug_response_tx.subscribe();
                        let mut attempts = 0usize;

                        loop {
                            let (client_id, tx) = match resolve_client_debug_sender(
                                requested_session.as_deref(),
                                &client_connections,
                                &client_debug_state,
                            )
                            .await
                            {
                                Ok(target) => target,
                                Err(err) => break Err(err),
                            };

                            if tx.send((id, cmd.to_string())).is_ok() {
                                // Wait for response with timeout
                                let timeout = tokio::time::Duration::from_secs(30);
                                match tokio::time::timeout(timeout, async {
                                    loop {
                                        if let Ok((resp_id, output)) = response_rx.recv().await
                                            && resp_id == id
                                        {
                                            return Ok(output);
                                        }
                                    }
                                })
                                .await
                                {
                                    Ok(result) => break result,
                                    Err(_) => {
                                        break Err(anyhow::anyhow!(
                                            "Timeout waiting for client response"
                                        ));
                                    }
                                }
                            } else {
                                let mut debug_state = client_debug_state.write().await;
                                debug_state.unregister(&client_id);
                                attempts += 1;
                                if requested_session.is_some()
                                    || debug_state.clients.is_empty()
                                    || attempts > 8
                                {
                                    break Err(anyhow::anyhow!("No TUI client connected"));
                                }
                            }
                        }
                    }
                    "tester" => {
                        // Handle tester commands
                        execute_tester_command(cmd).await
                    }
                    _ => {
                        // Server commands (default)
                        if let Some(output) = maybe_handle_job_command(cmd, &debug_jobs).await? {
                            Ok(output)
                        } else if let Some(output) = maybe_handle_session_admin_command(
                            cmd,
                            &sessions,
                            &session_id,
                            &provider,
                            &swarm_members,
                            &swarms_by_id,
                            &swarm_coordinators,
                            &swarm_plans,
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            &soft_interrupt_queues,
                            mcp_pool.clone(),
                        )
                        .await?
                        {
                            Ok(output)
                        } else if let Some(output) = maybe_handle_server_state_command(
                            cmd,
                            &sessions,
                            &client_connections,
                            &swarm_members,
                            &client_debug_state,
                            &server_identity,
                            server_start_time,
                            &swarms_by_id,
                            &shared_context,
                            &swarm_plans,
                            &swarm_coordinators,
                            &file_touches,
                            &files_touched_by_session,
                            &channel_subscriptions,
                            &channel_subscriptions_by_session,
                            &debug_jobs,
                            &event_history,
                            &shutdown_signals,
                            &soft_interrupt_queues,
                        )
                        .await?
                        {
                            Ok(output)
                        } else if let Some(output) = maybe_handle_swarm_read_command(
                            cmd,
                            &sessions,
                            &swarm_members,
                            &swarms_by_id,
                            &shared_context,
                            &swarm_plans,
                            &swarm_coordinators,
                            &file_touches,
                            &channel_subscriptions,
                            &server_identity,
                        )
                        .await?
                        {
                            Ok(output)
                        } else if let Some(output) = maybe_handle_swarm_write_command(
                            cmd,
                            &DebugSwarmWriteContext {
                                session_id: &session_id,
                                swarm_members: &swarm_members,
                                swarms_by_id: &swarms_by_id,
                                shared_context: &shared_context,
                                swarm_plans: &swarm_plans,
                                swarm_coordinators: &swarm_coordinators,
                            },
                        )
                        .await?
                        {
                            Ok(output)
                        } else if let Some(output) =
                            maybe_handle_ambient_command(cmd, &ambient_runner, &provider).await?
                        {
                            Ok(output)
                        } else if maybe_handle_event_subscription_command(
                            id,
                            cmd,
                            &swarm_event_tx,
                            &mut writer,
                        )
                        .await?
                        {
                            return Ok(());
                        } else if let Some(output) =
                            maybe_handle_event_query_command(cmd, &event_history).await
                        {
                            Ok(output)
                        } else if cmd == "swarm:help" {
                            Ok(swarm_debug_help_text())
                        } else if cmd == "help" {
                            Ok(debug_help_text())
                        } else {
                            match resolve_debug_session(&sessions, &session_id, requested_session)
                                .await
                            {
                                Ok((_session, agent)) => {
                                    execute_debug_command(
                                        agent,
                                        cmd,
                                        Arc::clone(&debug_jobs),
                                        Some(&server_identity),
                                    )
                                    .await
                                }
                                Err(e) => Err(e),
                            }
                        }
                    }
                };

                let (ok, output) = match result {
                    Ok(output) => (true, output),
                    Err(e) => (false, e.to_string()),
                };
                let event = ServerEvent::DebugResponse { id, ok, output };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }

            _ => {
                // Debug socket only allows ping, state, and debug_command
                let event = ServerEvent::Error {
                    id: request.id(),
                    message: "Debug socket only allows ping, state, and debug_command".to_string(),
                    retry_after_secs: None,
                };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "debug_tests.rs"]
mod debug_tests;
