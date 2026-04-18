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

    if let Ok(Some(session_id)) = crate::dictation::last_focused_session()
        && live_sessions.contains(&session_id)
    {
        return Ok(session_id);
    }

    if let Ok(Some(session_id)) = crate::dictation::focused_jcode_session()
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
                            let (client_id, tx) = {
                                let mut debug_state = client_debug_state.write().await;
                                match debug_state.active_sender() {
                                    Some(active) => active,
                                    None => {
                                        break Err(anyhow::anyhow!("No TUI client connected"));
                                    }
                                }
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
                                if debug_state.clients.is_empty() || attempts > 8 {
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
mod tests {
    use super::*;
    use crate::server::debug_jobs::DebugJobStatus;

    #[test]
    fn client_debug_state_registers_unregisters_and_falls_back() {
        let mut state = ClientDebugState::default();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let (tx2, _rx2) = mpsc::unbounded_channel();

        state.register("client-a".to_string(), tx1.clone());
        state.register("client-b".to_string(), tx2.clone());

        let (active_id, _sender) = state.active_sender().expect("active sender present");
        assert_eq!(active_id, "client-b");

        state.unregister("client-b");
        let (fallback_id, _sender) = state.active_sender().expect("fallback sender present");
        assert_eq!(fallback_id, "client-a");

        state.unregister("client-a");
        assert!(state.active_sender().is_none());
    }

    #[test]
    fn debug_job_payloads_include_expected_fields() {
        let now = Instant::now();
        let job = DebugJob {
            id: "job_123".to_string(),
            status: DebugJobStatus::Completed,
            command: "message:hello".to_string(),
            session_id: Some("session_abc".to_string()),
            created_at: now,
            started_at: Some(now),
            finished_at: Some(now),
            output: Some("done".to_string()),
            error: None,
        };

        let summary = job.summary_payload();
        assert_eq!(summary.get("id").and_then(|v| v.as_str()), Some("job_123"));
        assert_eq!(
            summary.get("status").and_then(|v| v.as_str()),
            Some("completed")
        );
        assert_eq!(
            summary.get("session_id").and_then(|v| v.as_str()),
            Some("session_abc")
        );

        let status = job.status_payload();
        assert_eq!(status.get("output").and_then(|v| v.as_str()), Some("done"));
        assert!(status.get("error").is_some());
    }

    #[test]
    fn debug_help_text_mentions_key_namespaces_and_commands() {
        let help = debug_help_text();
        assert!(help.contains("SERVER COMMANDS"));
        assert!(help.contains("CLIENT COMMANDS"));
        assert!(help.contains("TESTER COMMANDS"));
        assert!(help.contains("message_async:<text>"));
        assert!(help.contains("client:frame"));
    }

    #[test]
    fn swarm_debug_help_text_mentions_core_swarm_sections() {
        let help = swarm_debug_help_text();
        assert!(help.contains("MEMBERS & STRUCTURE"));
        assert!(help.contains("PLAN PROPOSALS"));
        assert!(help.contains("REAL-TIME EVENTS"));
        assert!(help.contains("swarm:list"));
    }

    #[test]
    fn parse_namespaced_command_defaults_to_server_namespace() {
        assert_eq!(parse_namespaced_command("state"), ("server", "state"));
        assert_eq!(
            parse_namespaced_command("swarm:list"),
            ("server", "swarm:list")
        );
    }

    #[test]
    fn parse_namespaced_command_recognizes_known_namespaces() {
        assert_eq!(
            parse_namespaced_command("client:frame"),
            ("client", "frame")
        );
        assert_eq!(parse_namespaced_command("tester:list"), ("tester", "list"));
        assert_eq!(
            parse_namespaced_command("server:state"),
            ("server", "state")
        );
    }
}

#[cfg(test)]
mod transcript_routing_tests {
    use super::{ClientConnectionInfo, ClientDebugState, resolve_transcript_target_session};
    use crate::protocol::ServerEvent;
    use crate::server::SwarmMember;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::{RwLock, mpsc};

    fn live_member(session_id: &str, connection_id: &str) -> SwarmMember {
        let (event_tx, _event_rx) = mpsc::unbounded_channel::<ServerEvent>();
        let now = Instant::now();
        SwarmMember {
            session_id: session_id.to_string(),
            event_tx: event_tx.clone(),
            event_txs: HashMap::from([(connection_id.to_string(), event_tx)]),
            working_dir: None,
            swarm_id: None,
            swarm_enabled: false,
            status: "ready".to_string(),
            detail: None,
            friendly_name: None,
            report_back_to_session_id: None,
            role: "agent".to_string(),
            joined_at: now,
            last_status_change: now,
            is_headless: false,
        }
    }

    fn connection(
        session_id: &str,
        debug_client_id: &str,
        last_seen: Instant,
    ) -> ClientConnectionInfo {
        ClientConnectionInfo {
            client_id: format!("conn-{session_id}"),
            session_id: session_id.to_string(),
            client_instance_id: None,
            debug_client_id: Some(debug_client_id.to_string()),
            connected_at: last_seen,
            last_seen,
            is_processing: false,
            current_tool_name: None,
            disconnect_tx: mpsc::unbounded_channel().0,
        }
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_uses_requested_connected_session() {
        let client_connections = Arc::new(RwLock::new(HashMap::from([(
            "conn-1".to_string(),
            connection("session_abc", "debug-1", Instant::now()),
        )])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session_abc".to_string(),
            live_member("session_abc", "conn-1"),
        )])));

        let resolved = resolve_transcript_target_session(
            Some("session_abc".to_string()),
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve connected requested session");

        assert_eq!(resolved, "session_abc");
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_prefers_last_focused_live_session() {
        let _guard = crate::storage::lock_test_env();
        let jcode_dir = crate::storage::jcode_dir().expect("jcode dir");
        let active_dir = jcode_dir.join("active_pids");
        std::fs::create_dir_all(&active_dir).expect("create active_pids");
        std::fs::write(active_dir.join("session_focus"), "12345").expect("write active pid");
        crate::dictation::remember_last_focused_session("session_focus")
            .expect("remember last focused session");

        let client_connections = Arc::new(RwLock::new(HashMap::from([(
            "conn-1".to_string(),
            connection("session_focus", "debug-1", Instant::now()),
        )])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session_focus".to_string(),
            live_member("session_focus", "conn-1"),
        )])));

        let resolved = resolve_transcript_target_session(
            None,
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve last-focused session");

        assert_eq!(resolved, "session_focus");
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_rejects_requested_session_without_connected_tui() {
        let client_connections = Arc::new(RwLock::new(HashMap::from([(
            "conn-1".to_string(),
            connection("session_abc", "debug-1", Instant::now()),
        )])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState::default()));
        let swarm_members = Arc::new(RwLock::new(HashMap::new()));

        let err = resolve_transcript_target_session(
            Some("session_abc".to_string()),
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect_err("requested session without connected tui should error");

        assert!(
            err.to_string()
                .contains("does not have a connected TUI client")
        );
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_falls_back_to_most_recent_live_tui_when_last_focused_not_connected()
     {
        let _guard = crate::storage::lock_test_env();
        let jcode_dir = crate::storage::jcode_dir().expect("jcode dir");
        let active_dir = jcode_dir.join("active_pids");
        std::fs::create_dir_all(&active_dir).expect("create active_pids");
        std::fs::write(active_dir.join("session_stale"), "12345").expect("write active pid");
        crate::dictation::remember_last_focused_session("session_stale")
            .expect("remember last focused session");

        let now = Instant::now();
        let client_connections = Arc::new(RwLock::new(HashMap::from([
            (
                "conn-1".to_string(),
                connection(
                    "session_stale_debug",
                    "debug-1",
                    now - std::time::Duration::from_secs(60),
                ),
            ),
            (
                "conn-2".to_string(),
                connection("session_recent", "debug-2", now),
            ),
        ])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState {
            active_id: Some("debug-1".to_string()),
            clients: HashMap::new(),
        }));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session_recent".to_string(),
            live_member("session_recent", "conn-2"),
        )])));

        let resolved = resolve_transcript_target_session(
            None,
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve fallback live session");

        assert_eq!(resolved, "session_recent");
    }

    #[tokio::test]
    async fn resolve_transcript_target_session_ignores_non_live_requesting_clients() {
        let now = Instant::now();
        let client_connections = Arc::new(RwLock::new(HashMap::from([
            (
                "conn-cli".to_string(),
                connection("session_cli", "debug-cli", now),
            ),
            (
                "conn-tui".to_string(),
                connection(
                    "session_tui",
                    "debug-tui",
                    now - std::time::Duration::from_secs(30),
                ),
            ),
        ])));
        let client_debug_state = Arc::new(RwLock::new(ClientDebugState {
            active_id: Some("debug-cli".to_string()),
            clients: HashMap::new(),
        }));
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            "session_tui".to_string(),
            live_member("session_tui", "conn-tui"),
        )])));

        let resolved = resolve_transcript_target_session(
            None,
            &client_connections,
            &client_debug_state,
            &swarm_members,
        )
        .await
        .expect("resolve live tui session");

        assert_eq!(resolved, "session_tui");
    }
}

#[cfg(test)]
mod debug_execution_tests {
    use crate::agent::Agent;
    use crate::provider;
    use crate::server::debug_command_exec::{debug_message_timeout_secs, resolve_debug_session};
    use crate::tool::Registry;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::Arc;
    use tokio::sync::{Mutex as AsyncMutex, RwLock};

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    struct EnvVarGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let lock = lock_env();
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self {
                _lock: lock,
                key,
                previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let lock = lock_env();
            let previous = std::env::var_os(key);
            crate::env::remove_var(key);
            Self {
                _lock: lock,
                key,
                previous,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.previous {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    struct TestProvider;

    #[async_trait::async_trait]
    impl provider::Provider for TestProvider {
        fn name(&self) -> &str {
            "test"
        }

        fn model(&self) -> String {
            "test".to_string()
        }

        fn available_models(&self) -> Vec<&'static str> {
            vec![]
        }

        fn available_models_display(&self) -> Vec<String> {
            vec![]
        }

        async fn prefetch_models(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_model(&self, _model: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn handles_tools_internally(&self) -> bool {
            false
        }

        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _session_id: Option<&str>,
        ) -> anyhow::Result<crate::provider::EventStream> {
            Err(anyhow::anyhow!(
                "test provider complete should not be called in debug tests"
            ))
        }

        fn fork(&self) -> Arc<dyn provider::Provider> {
            Arc::new(TestProvider)
        }
    }

    async fn test_agent() -> Arc<AsyncMutex<Agent>> {
        let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
        let registry = Registry::new(provider.clone()).await;
        Arc::new(AsyncMutex::new(Agent::new(provider, registry)))
    }

    #[tokio::test]
    async fn resolve_debug_session_uses_requested_session_when_present() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(String::new()));

        let (resolved_id, resolved_agent) =
            resolve_debug_session(&sessions, &current, Some(session_id.clone()))
                .await
                .expect("resolve requested session");

        assert_eq!(resolved_id, session_id);
        assert!(Arc::ptr_eq(&resolved_agent, &agent));
    }

    #[tokio::test]
    async fn resolve_debug_session_falls_back_to_current_session() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(session_id.clone()));

        let (resolved_id, resolved_agent) = resolve_debug_session(&sessions, &current, None)
            .await
            .expect("resolve current session");

        assert_eq!(resolved_id, session_id);
        assert!(Arc::ptr_eq(&resolved_agent, &agent));
    }

    #[tokio::test]
    async fn resolve_debug_session_uses_only_session_when_singleton() {
        let agent = test_agent().await;
        let session_id = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let current = Arc::new(RwLock::new(String::new()));

        let (resolved_id, _) = resolve_debug_session(&sessions, &current, None)
            .await
            .expect("resolve single session");

        assert_eq!(resolved_id, session_id);
    }

    #[tokio::test]
    async fn resolve_debug_session_errors_for_unknown_or_missing_session() {
        let agent_a = test_agent().await;
        let id_a = {
            let agent = agent_a.lock().await;
            agent.session_id().to_string()
        };
        let agent_b = test_agent().await;
        let id_b = {
            let agent = agent_b.lock().await;
            agent.session_id().to_string()
        };

        let sessions = Arc::new(RwLock::new(HashMap::from([
            (id_a.clone(), agent_a),
            (id_b.clone(), agent_b),
        ])));
        let current = Arc::new(RwLock::new(String::new()));

        let unknown = resolve_debug_session(&sessions, &current, Some("missing".to_string())).await;
        let unknown_err = match unknown {
            Ok(_) => panic!("expected unknown session to error"),
            Err(err) => err,
        };
        assert!(unknown_err.to_string().contains("Unknown session_id"));

        let missing = resolve_debug_session(&sessions, &current, None).await;
        let missing_err = match missing {
            Ok(_) => panic!("expected missing active session to error"),
            Err(err) => err,
        };
        assert!(missing_err.to_string().contains("No active session found"));
    }

    #[test]
    fn debug_message_timeout_secs_reads_valid_env_values() {
        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "17");
        assert_eq!(debug_message_timeout_secs(), Some(17));
    }

    #[test]
    fn debug_message_timeout_secs_ignores_missing_empty_invalid_and_zero() {
        let _guard = EnvVarGuard::remove("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "   ");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "abc");
        assert_eq!(debug_message_timeout_secs(), None);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS", "0");
        assert_eq!(debug_message_timeout_secs(), None);
    }
}
