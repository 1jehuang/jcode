mod await_members_state;
mod client_actions;
mod client_api;
mod client_comm;
mod client_disconnect_cleanup;
mod client_lifecycle;
mod client_session;
mod client_state;
mod comm_control;
mod comm_plan;
mod comm_session;
mod comm_sync;
mod debug;
mod debug_ambient;
mod debug_command_exec;
mod debug_events;
mod debug_help;
mod debug_jobs;
mod debug_server_state;
mod debug_session_admin;
mod debug_swarm_read;
mod debug_swarm_write;
mod debug_testers;
mod headless;
mod provider_control;
mod reload;
mod reload_state;
mod runtime;
mod socket;
mod swarm;
mod util;

pub(super) use self::await_members_state::AwaitMembersRuntime;
use self::debug::{ClientConnectionInfo, ClientDebugState};
use self::debug_jobs::DebugJob;
use self::headless::create_headless_session;
use self::reload::await_reload_signal;
use self::runtime::ServerRuntime;
#[allow(unused_imports)]
use self::swarm::{
    broadcast_swarm_plan, broadcast_swarm_status, record_swarm_event,
    record_swarm_event_for_session, remove_plan_participant, remove_session_channel_subscriptions,
    remove_session_file_touches, remove_session_from_swarm, rename_plan_participant,
    run_swarm_message, subscribe_session_to_channel, summarize_plan_items, truncate_detail,
    unsubscribe_session_from_channel, update_member_status,
};
use crate::agent::Agent;
use crate::ambient_runner::AmbientRunnerHandle;
use crate::bus::{Bus, BusEvent, FileOp};
use crate::message::format_background_task_notification_markdown;
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::runtime_memory_log::{
    ServerRuntimeMemoryBackground, ServerRuntimeMemoryClients, ServerRuntimeMemoryEmbeddings,
    ServerRuntimeMemorySample, ServerRuntimeMemoryServer, ServerRuntimeMemorySessions,
    ServerRuntimeMemoryTopSession,
};
use crate::transport::Listener;
use anyhow::Result;
use jcode_agent_runtime::{InterruptSignal, SoftInterruptSource};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast};

pub(super) type SessionAgents = Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>;
pub(super) type ChannelSubscriptions =
    Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>;

async fn run_background_task_message_in_live_session_if_idle(
    session_id: &str,
    message: &str,
    sessions: &SessionAgents,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    let agent = {
        let guard = sessions.read().await;
        guard.get(session_id).cloned()
    };
    let Some(agent) = agent else {
        return false;
    };

    let has_live_attachments = {
        let members = swarm_members.read().await;
        members
            .get(session_id)
            .map(|member| !member.event_txs.is_empty() || !member.event_tx.is_closed())
            .unwrap_or(false)
    };
    if !has_live_attachments {
        return false;
    }

    let is_idle = match agent.try_lock() {
        Ok(guard) => {
            drop(guard);
            true
        }
        Err(_) => false,
    };

    if !is_idle {
        return false;
    }

    let session_id = session_id.to_string();
    let message = message.to_string();
    let event_tx = session_event_fanout_sender(session_id.clone(), Arc::clone(swarm_members));
    tokio::spawn(async move {
        if let Err(err) = self::client_lifecycle::process_message_streaming_mpsc(
            agent,
            &message,
            vec![],
            Some(
                "A background task for this session just finished. Review the completion message and continue if useful."
                    .to_string(),
            ),
            event_tx,
        )
        .await
        {
            crate::logging::error(&format!(
                "Failed to run background task completion immediately for live session {}: {}",
                session_id, err
            ));
        }
    });

    true
}

async fn dispatch_background_task_completion(
    task: &crate::bus::BackgroundTaskCompleted,
    sessions: &SessionAgents,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) {
    let notification = format_background_task_notification_markdown(task);

    if task.notify
        && fanout_session_event(
            swarm_members,
            &task.session_id,
            ServerEvent::Notification {
                from_session: "background_task".to_string(),
                from_name: Some("background task".to_string()),
                notification_type: NotificationType::Message {
                    scope: Some("background_task".to_string()),
                    channel: None,
                },
                message: notification.clone(),
            },
        )
        .await
            == 0
    {
        crate::logging::warn(&format!(
            "Failed to notify attached clients for background task completion on session {}",
            task.session_id
        ));
    }

    if task.wake
        && !run_background_task_message_in_live_session_if_idle(
            &task.session_id,
            &notification,
            sessions,
            swarm_members,
        )
        .await
        && !queue_soft_interrupt_for_session(
            &task.session_id,
            notification.clone(),
            false,
            SoftInterruptSource::BackgroundTask,
            soft_interrupt_queues,
            sessions,
        )
        .await
    {
        crate::logging::warn(&format!(
            "Failed to deliver background task completion to session {}",
            task.session_id
        ));
    }
}

fn json_u64_field(value: Option<&serde_json::Value>, field: &str) -> u64 {
    value
        .and_then(|value| value.get(field))
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
}

fn json_bool_field(value: Option<&serde_json::Value>, field: &str) -> bool {
    value
        .and_then(|value| value.get(field))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

async fn capture_runtime_memory_sample(
    identity: &ServerIdentity,
    sessions: &SessionAgents,
    client_count: &Arc<RwLock<usize>>,
    server_start_time: Instant,
    source: &str,
) -> ServerRuntimeMemorySample {
    let now = chrono::Utc::now();
    let process = crate::process_memory::snapshot_with_source(format!("server:runtime-log:{source}"));
    let connected_count = *client_count.read().await;
    let background_task_count = crate::background::global().list().await.len();
    let embedder_stats = crate::embedding::stats();
    let embedding_model_available = crate::embedding::is_model_available();

    let sessions_guard = sessions.read().await;
    let live_count = sessions_guard.len();
    let mut sampled_count = 0usize;
    let mut contended_count = 0usize;
    let mut memory_enabled_session_count = 0usize;
    let mut total_message_count = 0u64;
    let mut total_provider_cache_message_count = 0u64;
    let mut total_json_bytes = 0u64;
    let mut total_payload_text_bytes = 0u64;
    let mut total_provider_cache_json_bytes = 0u64;
    let mut total_tool_result_bytes = 0u64;
    let mut total_provider_cache_tool_result_bytes = 0u64;
    let mut total_large_blob_bytes = 0u64;
    let mut total_provider_cache_large_blob_bytes = 0u64;
    let mut top_sessions: Vec<ServerRuntimeMemoryTopSession> = Vec::new();

    for (session_id, agent_arc) in sessions_guard.iter() {
        let Ok(agent) = agent_arc.try_lock() else {
            contended_count += 1;
            continue;
        };

        sampled_count += 1;
        let profile = agent.debug_memory_profile();
        let session_profile = profile.get("session");
        let totals = session_profile.and_then(|value| value.get("totals"));
        let messages = session_profile.and_then(|value| value.get("messages"));
        let provider_cache = session_profile.and_then(|value| value.get("provider_messages_cache"));
        let agent_profile = profile.get("agent");
        let memory_enabled = json_bool_field(agent_profile, "memory_enabled");
        if memory_enabled {
            memory_enabled_session_count += 1;
        }

        let message_count = json_u64_field(messages, "count");
        let provider_cache_message_count = json_u64_field(provider_cache, "count");
        let json_bytes = json_u64_field(totals, "json_bytes");
        let payload_text_bytes = json_u64_field(totals, "payload_text_bytes");
        let provider_cache_json_bytes = json_u64_field(totals, "provider_cache_json_bytes");
        let tool_result_bytes = json_u64_field(totals, "canonical_tool_result_bytes");
        let provider_cache_tool_result_bytes =
            json_u64_field(totals, "provider_cache_tool_result_bytes");
        let large_blob_bytes = json_u64_field(totals, "canonical_large_blob_bytes");
        let provider_cache_large_blob_bytes =
            json_u64_field(totals, "provider_cache_large_blob_bytes");

        total_message_count += message_count;
        total_provider_cache_message_count += provider_cache_message_count;
        total_json_bytes += json_bytes;
        total_payload_text_bytes += payload_text_bytes;
        total_provider_cache_json_bytes += provider_cache_json_bytes;
        total_tool_result_bytes += tool_result_bytes;
        total_provider_cache_tool_result_bytes += provider_cache_tool_result_bytes;
        total_large_blob_bytes += large_blob_bytes;
        total_provider_cache_large_blob_bytes += provider_cache_large_blob_bytes;

        top_sessions.push(ServerRuntimeMemoryTopSession {
            session_id: session_id.clone(),
            provider: agent.provider_name(),
            model: agent.provider_model(),
            memory_enabled,
            message_count,
            provider_cache_message_count,
            json_bytes,
            payload_text_bytes,
            provider_cache_json_bytes,
            tool_result_bytes,
            provider_cache_tool_result_bytes,
            large_blob_bytes,
            provider_cache_large_blob_bytes,
        });
    }
    drop(sessions_guard);

    top_sessions.sort_by(|left, right| right.json_bytes.cmp(&left.json_bytes));
    top_sessions.truncate(5);

    ServerRuntimeMemorySample {
        schema_version: 1,
        timestamp: now.to_rfc3339(),
        timestamp_ms: now.timestamp_millis(),
        source: source.to_string(),
        server: ServerRuntimeMemoryServer {
            id: identity.id.clone(),
            name: identity.name.clone(),
            icon: identity.icon.clone(),
            version: identity.version.clone(),
            git_hash: identity.git_hash.clone(),
            uptime_secs: server_start_time.elapsed().as_secs(),
        },
        process,
        clients: ServerRuntimeMemoryClients { connected_count },
        sessions: ServerRuntimeMemorySessions {
            live_count,
            sampled_count,
            contended_count,
            memory_enabled_session_count,
            total_message_count,
            total_provider_cache_message_count,
            total_json_bytes,
            total_payload_text_bytes,
            total_provider_cache_json_bytes,
            total_tool_result_bytes,
            total_provider_cache_tool_result_bytes,
            total_large_blob_bytes,
            total_provider_cache_large_blob_bytes,
            top_by_json_bytes: top_sessions,
        },
        background: ServerRuntimeMemoryBackground {
            task_count: background_task_count,
        },
        embeddings: ServerRuntimeMemoryEmbeddings {
            model_available: embedding_model_available,
            stats: embedder_stats,
        },
    }
}

mod state;

use self::state::latest_peer_touches;
pub use self::state::{
    FileAccess, SharedContext, SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan,
};
use self::state::{
    SessionInterruptQueues, enqueue_soft_interrupt, fanout_live_client_event, fanout_session_event,
    queue_soft_interrupt_for_session, register_session_event_sender,
    register_session_interrupt_queue, remove_session_interrupt_queue,
    rename_session_interrupt_queue, session_event_fanout_sender, unregister_session_event_sender,
};

pub use self::await_members_state::pending_await_members_for_session;
use self::reload_state::clear_reload_marker_if_stale_for_pid;
#[cfg(test)]
pub(crate) use self::reload_state::subscribe_reload_signal_for_tests;
pub use self::reload_state::{
    ReloadAck, ReloadPhase, ReloadSignal, ReloadState, ReloadWaitStatus, acknowledge_reload_signal,
    await_reload_handoff, clear_reload_marker, inspect_reload_wait_status,
    publish_reload_socket_ready, recent_reload_state, reload_marker_active, reload_marker_exists,
    reload_marker_path, reload_process_alive, reload_state_summary, send_reload_signal,
    wait_for_reload_ack, wait_for_reload_handoff_event, write_reload_marker, write_reload_state,
};

#[cfg(unix)]
pub use self::socket::spawn_server_notify;
#[cfg(unix)]
use self::socket::{acquire_daemon_lock, mark_close_on_exec};
pub use self::socket::{
    cleanup_socket_pair, connect_socket, debug_socket_path, has_live_listener, is_server_ready,
    set_socket_path, socket_path, wait_for_server_ready,
};
use self::socket::{signal_ready_fd, socket_has_live_listener};

pub use self::util::ServerIdentity;
use self::util::{
    debug_control_allowed, embedding_idle_unload_secs, git_common_dir_for, server_has_newer_binary,
    server_update_candidate, swarm_id_for_dir,
};

#[cfg(test)]
mod socket_tests;

#[cfg(test)]
mod startup_tests;

#[cfg(test)]
mod queue_tests;

#[cfg(test)]
mod file_activity_tests;

/// Idle timeout for the shared server when no clients are connected (5 minutes)
const IDLE_TIMEOUT_SECS: u64 = 300;

/// How often to check whether the embedding model can be unloaded.
const EMBEDDING_IDLE_CHECK_SECS: u64 = 30;

/// Exit code when server shuts down due to idle timeout
pub const EXIT_IDLE_TIMEOUT: i32 = 44;

/// Server state
pub struct Server {
    provider: Arc<dyn Provider>,
    socket_path: PathBuf,
    debug_socket_path: PathBuf,
    gateway_config_override: Option<crate::gateway::GatewayConfig>,
    /// Server identity for multi-server support
    identity: ServerIdentity,
    /// Broadcast channel for streaming events to all subscribers
    event_tx: broadcast::Sender<ServerEvent>,
    /// Active sessions (session_id -> Agent)
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    /// Current processing state
    is_processing: Arc<RwLock<bool>>,
    /// Session ID for the default session
    session_id: Arc<RwLock<String>>,
    /// Number of connected clients
    client_count: Arc<RwLock<usize>>,
    /// Connected client mapping (client_id -> session_id)
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    /// Track file touches: path -> list of accesses
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    /// Reverse index for file touches: session_id -> touched paths
    files_touched_by_session: Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
    /// Swarm members: session_id -> SwarmMember info
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    /// Swarm groupings by swarm id -> set of session_ids
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    /// Shared context by swarm (swarm_id -> key -> SharedContext)
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    /// Shared plans by swarm (swarm_id -> plan)
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    /// Coordinator per swarm (swarm_id -> session_id)
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    /// Active and available TUI debug channels (request_id, command)
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    /// Channel to receive client debug responses from TUI (request_id, response)
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    /// Background debug jobs (async debug commands)
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    /// Channel subscriptions (swarm_id -> channel -> session_ids)
    channel_subscriptions: ChannelSubscriptions,
    /// Reverse index for channel subscriptions: session_id -> swarm_id -> channels
    channel_subscriptions_by_session: ChannelSubscriptions,
    /// Event history for real-time event subscription (ring buffer)
    event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    /// Counter for event IDs
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    /// Broadcast channel for swarm event subscriptions (debug socket subscribers)
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
    /// Ambient mode runner handle (None if ambient is disabled)
    ambient_runner: Option<AmbientRunnerHandle>,
    /// Shared MCP server pool (processes shared across sessions), initialized lazily.
    mcp_pool: Arc<OnceCell<Arc<crate::mcp::SharedMcpPool>>>,
    /// Graceful shutdown signals by session_id (stored outside agent mutex so they
    /// can be signaled without locking the agent during active tool execution)
    shutdown_signals: Arc<RwLock<HashMap<String, InterruptSignal>>>,
    /// Soft interrupt queues by session_id (stored outside agent mutex so swarm/debug
    /// notifications can be enqueued while an agent is actively processing)
    soft_interrupt_queues: SessionInterruptQueues,
    /// Persisted communicate await_members wait registry.
    await_members_runtime: AwaitMembersRuntime,
}

impl Server {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        use crate::id::{new_memorable_server_id, server_icon};

        let (event_tx, _) = broadcast::channel(1024);
        let (client_debug_response_tx, _) = broadcast::channel(64);

        // Generate a memorable server name
        let (id, name) = new_memorable_server_id();
        let icon = server_icon(&name).to_string();
        let identity = ServerIdentity {
            id,
            name,
            icon,
            git_hash: env!("JCODE_GIT_HASH").to_string(),
            version: env!("JCODE_VERSION").to_string(),
        };
        crate::process_title::set_server_title(&identity.name);

        // Initialize the background runner even when ambient mode is disabled so
        // session-targeted scheduled tasks still have a live delivery loop.
        let ambient_runner = {
            let safety = Arc::new(crate::safety::SafetySystem::new());
            let handle = AmbientRunnerHandle::new(safety);
            crate::tool::ambient::init_schedule_runner(handle.clone());
            Some(handle)
        };

        Self {
            provider,
            socket_path: socket_path(),
            debug_socket_path: debug_socket_path(),
            gateway_config_override: None,
            identity,
            event_tx,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            is_processing: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(String::new())),
            client_count: Arc::new(RwLock::new(0)),
            client_connections: Arc::new(RwLock::new(HashMap::new())),
            file_touches: Arc::new(RwLock::new(HashMap::new())),
            files_touched_by_session: Arc::new(RwLock::new(HashMap::new())),
            swarm_members: Arc::new(RwLock::new(HashMap::new())),
            swarms_by_id: Arc::new(RwLock::new(HashMap::new())),
            shared_context: Arc::new(RwLock::new(HashMap::new())),
            swarm_plans: Arc::new(RwLock::new(HashMap::new())),
            swarm_coordinators: Arc::new(RwLock::new(HashMap::new())),
            client_debug_state: Arc::new(RwLock::new(ClientDebugState::default())),
            client_debug_response_tx,
            debug_jobs: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions_by_session: Arc::new(RwLock::new(HashMap::new())),
            event_history: Arc::new(RwLock::new(std::collections::VecDeque::new())),
            event_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            swarm_event_tx: broadcast::channel(256).0,
            ambient_runner,
            mcp_pool: Arc::new(OnceCell::new()),
            shutdown_signals: Arc::new(RwLock::new(HashMap::new())),
            soft_interrupt_queues: Arc::new(RwLock::new(HashMap::new())),
            await_members_runtime: AwaitMembersRuntime::default(),
        }
    }

    pub fn new_with_paths(
        provider: Arc<dyn Provider>,
        socket_path: PathBuf,
        debug_socket_path: PathBuf,
    ) -> Self {
        let mut server = Self::new(provider);
        server.socket_path = socket_path;
        server.debug_socket_path = debug_socket_path;
        server
    }

    pub fn with_gateway_config(mut self, gateway_config: crate::gateway::GatewayConfig) -> Self {
        self.gateway_config_override = Some(gateway_config);
        self
    }

    /// Get the server identity
    pub fn identity(&self) -> &ServerIdentity {
        &self.identity
    }

    fn runtime(&self) -> ServerRuntime {
        ServerRuntime::from_server(self)
    }

    fn build_registry_info(&self) -> crate::registry::ServerInfo {
        crate::registry::ServerInfo {
            id: self.identity.id.clone(),
            name: self.identity.name.clone(),
            icon: self.identity.icon.clone(),
            socket: self.socket_path.clone(),
            debug_socket: self.debug_socket_path.clone(),
            git_hash: self.identity.git_hash.clone(),
            version: self.identity.version.clone(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            sessions: Vec::new(),
        }
    }

    fn spawn_registry_prewarm(&self) {
        let registry_warm_provider = Arc::clone(&self.provider);
        tokio::spawn(async move {
            let start = Instant::now();
            let provider = registry_warm_provider.fork();
            let _ = crate::tool::Registry::new(provider).await;
            crate::logging::info(&format!(
                "Registry prewarm completed in {}ms",
                start.elapsed().as_millis()
            ));
        });
    }

    fn spawn_background_tasks(&self, server_start_time: Instant) {
        // Preload the embedding model in background so warm startups get fast
        // memory recall. On a cold install, skip eager preload because the
        // first-time model download can make the first spawned client look hung
        // while the daemon finishes bootstrapping.
        if crate::embedding::is_model_available() {
            tokio::task::spawn_blocking(|| {
                let start = std::time::Instant::now();
                match crate::embedding::get_embedder() {
                    Ok(_) => {
                        crate::logging::info(&format!(
                            "Embedding model preloaded in {}ms",
                            start.elapsed().as_millis()
                        ));
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Embedding model preload failed (non-fatal): {}",
                            e
                        ));
                    }
                }
            });
        } else {
            crate::logging::info(
                "Embedding model not installed yet; skipping eager preload during server startup",
            );
        }

        // Spawn reload monitor (event-driven via in-process channel).
        // In the unified server design, self-dev sessions share the main server,
        // so the shared server must always listen for reload signals.
        let signal_sessions = Arc::clone(&self.sessions);
        let signal_swarm_members = Arc::clone(&self.swarm_members);
        let signal_shutdown_signals = Arc::clone(&self.shutdown_signals);
        let signal_swarm_event_tx = self.swarm_event_tx.clone();
        tokio::spawn(async move {
            await_reload_signal(
                signal_sessions,
                signal_swarm_members,
                signal_shutdown_signals,
                signal_swarm_event_tx,
            )
            .await;
        });

        // Log when we receive SIGTERM for debugging
        #[cfg(unix)]
        {
            let sigterm_server_name = self.identity.name.clone();
            tokio::spawn(async move {
                use tokio::signal::unix::{SignalKind, signal};
                if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                    sigterm.recv().await;
                    crate::logging::info("Server received SIGTERM, shutting down gracefully");
                    let _ = crate::registry::unregister_server(&sigterm_server_name).await;
                    std::process::exit(0);
                }
            });
        }

        // Spawn the bus monitor for swarm coordination
        let monitor_file_touches = Arc::clone(&self.file_touches);
        let monitor_files_touched_by_session = Arc::clone(&self.files_touched_by_session);
        let monitor_swarm_members = Arc::clone(&self.swarm_members);
        let monitor_swarms_by_id = Arc::clone(&self.swarms_by_id);
        let monitor_swarm_plans = Arc::clone(&self.swarm_plans);
        let monitor_swarm_coordinators = Arc::clone(&self.swarm_coordinators);
        let monitor_shared_context = Arc::clone(&self.shared_context);
        let monitor_sessions = Arc::clone(&self.sessions);
        let monitor_soft_interrupt_queues = Arc::clone(&self.soft_interrupt_queues);
        let monitor_event_history = Arc::clone(&self.event_history);
        let monitor_event_counter = Arc::clone(&self.event_counter);
        let monitor_swarm_event_tx = self.swarm_event_tx.clone();
        tokio::spawn(async move {
            Self::monitor_bus(
                monitor_file_touches,
                monitor_files_touched_by_session,
                monitor_swarm_members,
                monitor_swarms_by_id,
                monitor_swarm_plans,
                monitor_swarm_coordinators,
                monitor_shared_context,
                monitor_sessions,
                monitor_soft_interrupt_queues,
                monitor_event_history,
                monitor_event_counter,
                monitor_swarm_event_tx,
            )
            .await;
        });

        // Initialize the memory agent early so it's ready for all sessions
        if crate::config::config().features.memory {
            tokio::spawn(async {
                let _ = crate::memory_agent::init().await;
            });
        }

        // Spawn the background ambient/schedule loop.
        if let Some(ref runner) = self.ambient_runner {
            let ambient_handle = runner.clone();
            let ambient_provider = Arc::clone(&self.provider);
            crate::logging::info("Starting ambient/schedule background loop");
            tokio::spawn(async move {
                ambient_handle.run_loop(ambient_provider).await;
            });
        }

        // Spawn embedding idle monitor so the model can be unloaded when this
        // server has been quiet for a while.
        let embedding_idle_secs = embedding_idle_unload_secs();
        tokio::spawn(async move {
            let idle_for = std::time::Duration::from_secs(embedding_idle_secs);
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(EMBEDDING_IDLE_CHECK_SECS));
            loop {
                interval.tick().await;
                let unloaded = crate::embedding::maybe_unload_if_idle(idle_for);
                if unloaded {
                    let stats = crate::embedding::stats();
                    crate::logging::info(&format!(
                        "Embedding idle monitor: model unloaded (loads={}, unloads={}, calls={}, avg_ms={})",
                        stats.load_count,
                        stats.unload_count,
                        stats.embed_calls,
                        stats
                            .avg_embed_ms
                            .map(|v| format!("{:.1}", v))
                            .unwrap_or_else(|| "n/a".to_string())
                    ));
                }
            }
        });

        if crate::runtime_memory_log::server_logging_enabled() {
            let log_identity = self.identity.clone();
            let log_sessions = Arc::clone(&self.sessions);
            let log_client_count = Arc::clone(&self.client_count);
            tokio::spawn(async move {
                match crate::runtime_memory_log::prune_old_server_logs() {
                    Ok(removed) if removed > 0 => {
                        crate::logging::info(&format!(
                            "Runtime memory logging pruned {} old log files",
                            removed
                        ));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        crate::logging::info(&format!(
                            "Runtime memory logging could not prune old logs: {}",
                            err
                        ));
                    }
                }

                let log_interval = crate::runtime_memory_log::server_logging_interval();
                match crate::runtime_memory_log::current_server_log_path() {
                    Ok(path) => crate::logging::info(&format!(
                        "Runtime memory logging enabled every {}s -> {}",
                        log_interval.as_secs(),
                        path.display()
                    )),
                    Err(err) => crate::logging::info(&format!(
                        "Runtime memory logging enabled every {}s (path unavailable: {})",
                        log_interval.as_secs(),
                        err
                    )),
                }

                let startup_sample = capture_runtime_memory_sample(
                    &log_identity,
                    &log_sessions,
                    &log_client_count,
                    server_start_time,
                    "startup",
                )
                .await;
                if let Err(err) = crate::runtime_memory_log::append_server_sample(&startup_sample) {
                    crate::logging::info(&format!(
                        "Runtime memory logging startup sample failed: {}",
                        err
                    ));
                }

                let mut interval = tokio::time::interval(log_interval);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    let sample = capture_runtime_memory_sample(
                        &log_identity,
                        &log_sessions,
                        &log_client_count,
                        server_start_time,
                        "interval",
                    )
                    .await;
                    if let Err(err) = crate::runtime_memory_log::append_server_sample(&sample) {
                        crate::logging::info(&format!(
                            "Runtime memory logging interval sample failed: {}",
                            err
                        ));
                    }
                }
            });
        }

        if debug_control_allowed() {
            crate::logging::info("Debug control enabled; idle timeout monitor disabled.");
        } else {
            let idle_client_count = Arc::clone(&self.client_count);
            let idle_server_name = self.identity.name.clone();
            tokio::spawn(async move {
                let mut idle_since: Option<std::time::Instant> = None;
                let mut check_interval = tokio::time::interval(std::time::Duration::from_secs(10));

                loop {
                    check_interval.tick().await;

                    let count = *idle_client_count.read().await;

                    if count == 0 {
                        // No clients connected
                        if idle_since.is_none() {
                            idle_since = Some(std::time::Instant::now());
                            crate::logging::info(&format!(
                                "No clients connected. Server will exit after {} minutes of idle.",
                                IDLE_TIMEOUT_SECS / 60
                            ));
                        }

                        if let Some(since) = idle_since {
                            let idle_duration = since.elapsed().as_secs();
                            if idle_duration >= IDLE_TIMEOUT_SECS {
                                crate::logging::info(&format!(
                                    "Server idle for {} minutes with no clients. Shutting down.",
                                    idle_duration / 60
                                ));
                                let _ = crate::registry::unregister_server(&idle_server_name).await;
                                std::process::exit(EXIT_IDLE_TIMEOUT);
                            }
                        }
                    } else {
                        // Clients connected - reset idle timer
                        if idle_since.is_some() {
                            crate::logging::info("Client connected. Idle timer cancelled.");
                        }
                        idle_since = None;
                    }
                }
            });
        }
    }

    fn spawn_registry_metadata_publisher(&self, registry_info: crate::registry::ServerInfo) {
        let registry_identity = self.identity.display_name();
        tokio::spawn(async move {
            let hash_path = format!("{}.hash", registry_info.socket.display());
            let _ = std::fs::write(&hash_path, env!("JCODE_GIT_HASH"));

            let mut registry = crate::registry::ServerRegistry::load()
                .await
                .unwrap_or_default();
            registry.register(registry_info);
            let _ = registry.save().await;
            crate::logging::info(&format!(
                "Registered as {} in server registry",
                registry_identity,
            ));

            if let Ok(mut registry) = crate::registry::ServerRegistry::load().await {
                let _ = registry.cleanup_stale().await;
                let _ = registry.save().await;
            }
        });
    }

    /// Monitor the global Bus for FileTouch events and detect conflicts
    #[expect(
        clippy::too_many_arguments,
        reason = "bus monitor needs file state, swarm state, sessions, queues, and event history sinks"
    )]
    async fn monitor_bus(
        file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
        files_touched_by_session: Arc<RwLock<HashMap<String, HashSet<PathBuf>>>>,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
        swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
        _swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
        _swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
        _shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
        sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
        soft_interrupt_queues: SessionInterruptQueues,
        event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
        event_counter: Arc<std::sync::atomic::AtomicU64>,
        swarm_event_tx: broadcast::Sender<SwarmEvent>,
    ) {
        let mut receiver = Bus::global().subscribe();
        let mut last_cleanup = Instant::now();
        const TOUCH_EXPIRY: Duration = Duration::from_secs(30 * 60); // 30 min
        const CLEANUP_INTERVAL: Duration = Duration::from_secs(5 * 60); // 5 min

        loop {
            // Periodic cleanup of expired file touches
            if last_cleanup.elapsed() > CLEANUP_INTERVAL {
                let mut touches = file_touches.write().await;
                let now = Instant::now();
                touches.retain(|_, accesses| {
                    accesses.retain(|a| now.duration_since(a.timestamp) < TOUCH_EXPIRY);
                    !accesses.is_empty()
                });
                let mut rebuilt_reverse_index: HashMap<String, HashSet<PathBuf>> = HashMap::new();
                for (path, accesses) in touches.iter() {
                    for access in accesses {
                        rebuilt_reverse_index
                            .entry(access.session_id.clone())
                            .or_default()
                            .insert(path.clone());
                    }
                }
                drop(touches);
                *files_touched_by_session.write().await = rebuilt_reverse_index;
                last_cleanup = Instant::now();
            }

            match receiver.recv().await {
                Ok(BusEvent::FileTouch(touch)) => {
                    let path = touch.path.clone();
                    let session_id = touch.session_id.clone();

                    // Record this touch
                    {
                        let mut touches = file_touches.write().await;
                        let accesses = touches.entry(path.clone()).or_insert_with(Vec::new);
                        accesses.push(FileAccess {
                            session_id: session_id.clone(),
                            op: touch.op.clone(),
                            timestamp: Instant::now(),
                            absolute_time: std::time::SystemTime::now(),
                            summary: touch.summary.clone(),
                            detail: touch.detail.clone(),
                        });
                    }
                    {
                        let mut reverse_index = files_touched_by_session.write().await;
                        reverse_index
                            .entry(session_id.clone())
                            .or_default()
                            .insert(path.clone());
                    }

                    // Record event for subscription
                    {
                        let members = swarm_members.read().await;
                        let member = members.get(&session_id);
                        let session_name = member.and_then(|m| m.friendly_name.clone());
                        let swarm_id = member.and_then(|m| m.swarm_id.clone());

                        drop(members);
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            session_id.clone(),
                            session_name,
                            swarm_id,
                            SwarmEventType::FileTouch {
                                path: path.to_string_lossy().to_string(),
                                op: touch.op.as_str().to_string(),
                                summary: touch.summary.clone(),
                                detail: touch.detail.clone(),
                            },
                        )
                        .await;
                    }

                    // Find the swarm this session belongs to
                    let swarm_session_ids: Vec<String> = {
                        let members = swarm_members.read().await;
                        if let Some(member) = members.get(&session_id) {
                            if let Some(ref swarm_id) = member.swarm_id {
                                let swarms = swarms_by_id.read().await;
                                if let Some(swarm) = swarms.get(swarm_id) {
                                    swarm.iter().cloned().collect()
                                } else {
                                    vec![]
                                }
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        }
                    };

                    // Only notify on modifications; plain reads are tracked for later context
                    // but should not proactively alert the swarm.
                    let is_modification = matches!(touch.op, FileOp::Write | FileOp::Edit);
                    if is_modification {
                        crate::logging::info(&format!(
                            "[file-activity] modification by {} on {}, swarm_peers: {:?}",
                            &session_id[..8.min(session_id.len())],
                            path.display(),
                            swarm_session_ids
                                .iter()
                                .map(|s| &s[..8.min(s.len())])
                                .collect::<Vec<_>>()
                        ));
                    }
                    let previous_touches: Vec<FileAccess> = if is_modification {
                        let touches = file_touches.read().await;
                        if let Some(accesses) = touches.get(&path) {
                            let swarm_session_ids_set: HashSet<String> =
                                swarm_session_ids.iter().cloned().collect();
                            let result =
                                latest_peer_touches(accesses, &session_id, &swarm_session_ids_set);
                            crate::logging::info(&format!(
                                "[file-activity] {} prior peer touches ({} total accesses)",
                                result.len(),
                                accesses.len()
                            ));
                            result
                        } else {
                            crate::logging::info("[file-activity] no touches for this path yet");
                            vec![]
                        }
                    } else {
                        vec![]
                    };

                    // If swarm peers previously touched this file, notify both sides so they
                    // can coordinate before the work diverges further.
                    if !previous_touches.is_empty() {
                        crate::logging::info(&format!(
                            "[file-activity] {} touched by peers before modification — sending alerts",
                            path.display()
                        ));
                        let members = swarm_members.read().await;
                        let current_member = members.get(&session_id);
                        let current_name = current_member.and_then(|m| m.friendly_name.clone());

                        // Alert the current agent about previous peer touches (one per agent).
                        if let Some(member) = current_member {
                            for prev in &previous_touches {
                                let prev_member = members.get(&prev.session_id);
                                let prev_name = prev_member.and_then(|m| m.friendly_name.clone());
                                let alert_msg = format!(
                                    "⚠️ File activity: {} — {} previously {} this file{}",
                                    path.display(),
                                    prev_name.as_deref().unwrap_or(&prev.session_id[..8]),
                                    prev.op.as_str(),
                                    prev.summary
                                        .as_ref()
                                        .map(|s| format!(": {}", s))
                                        .unwrap_or_default()
                                );
                                let notification = ServerEvent::Notification {
                                    from_session: prev.session_id.clone(),
                                    from_name: prev_name,
                                    notification_type: NotificationType::FileConflict {
                                        path: path.display().to_string(),
                                        operation: prev.op.as_str().to_string(),
                                        summary: prev.summary.clone(),
                                        detail: prev.detail.clone(),
                                    },
                                    message: alert_msg.clone(),
                                };
                                let _ = member.event_tx.send(notification);

                                if !queue_soft_interrupt_for_session(
                                    &session_id,
                                    alert_msg.clone(),
                                    false,
                                    SoftInterruptSource::System,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-activity soft interrupt for session {}",
                                        session_id
                                    ));
                                }
                            }
                        }

                        // Alert previous agents about the current modification.
                        for prev in &previous_touches {
                            if let Some(prev_member) = members.get(&prev.session_id) {
                                let alert_msg = format!(
                                    "⚠️ File activity: {} — {} just {} this file you previously worked with{}",
                                    path.display(),
                                    current_name
                                        .as_deref()
                                        .unwrap_or(&session_id[..8.min(session_id.len())]),
                                    touch.op.as_str(),
                                    touch
                                        .summary
                                        .as_ref()
                                        .map(|s| format!(": {}", s))
                                        .unwrap_or_default()
                                );
                                let notification = ServerEvent::Notification {
                                    from_session: session_id.clone(),
                                    from_name: current_name.clone(),
                                    notification_type: NotificationType::FileConflict {
                                        path: path.display().to_string(),
                                        operation: touch.op.as_str().to_string(),
                                        summary: touch.summary.clone(),
                                        detail: touch.detail.clone(),
                                    },
                                    message: alert_msg.clone(),
                                };
                                let _ = prev_member.event_tx.send(notification);

                                if !queue_soft_interrupt_for_session(
                                    &prev.session_id,
                                    alert_msg.clone(),
                                    false,
                                    SoftInterruptSource::System,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-activity soft interrupt for session {}",
                                        prev.session_id
                                    ));
                                }
                            }
                        }
                    }
                }
                Ok(BusEvent::BackgroundTaskCompleted(task)) => {
                    dispatch_background_task_completion(
                        &task,
                        &sessions,
                        &soft_interrupt_queues,
                        &swarm_members,
                    )
                    .await;
                }
                // Session todos are private. Swarm plans are updated via explicit
                // communication actions (comm_propose_plan / comm_approve_plan), not
                // todowrite broadcasts.
                Ok(BusEvent::TodoUpdated(_)) => {}
                Ok(_) => {
                    // Ignore other events
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    crate::logging::info(&format!("Bus monitor lagged by {} events", n));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    }

    /// Start the server (both main and debug sockets)
    pub async fn run(&self) -> Result<()> {
        // Ensure socket directory exists (for named sockets like /run/user/1000/jcode/)
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        #[cfg(unix)]
        let _daemon_lock = acquire_daemon_lock()?;

        if socket_has_live_listener(&self.socket_path).await {
            anyhow::bail!(
                "Refusing to replace active server socket at {}",
                self.socket_path.display()
            );
        }

        // Remove existing sockets (uses transport abstraction for cross-platform cleanup)
        crate::transport::remove_socket(&self.socket_path);
        crate::transport::remove_socket(&self.debug_socket_path);

        #[allow(unused_mut)]
        let mut main_listener = Listener::bind(&self.socket_path)?;
        #[allow(unused_mut)]
        let mut debug_listener = Listener::bind(&self.debug_socket_path)?;

        #[cfg(unix)]
        {
            // Server reload uses exec. Force the published listener fds to close
            // across exec so the replacement daemon can safely rebind them.
            mark_close_on_exec(&main_listener);
            mark_close_on_exec(&debug_listener);
        }

        // Preserve an in-flight reload marker for exec-based reloads owned by this
        // process, but clear stale markers from unrelated/stale processes.
        clear_reload_marker_if_stale_for_pid(std::process::id());

        // Restrict socket files to owner-only so other local users cannot connect.
        let _ = crate::platform::set_permissions_owner_only(&self.socket_path);
        let _ = crate::platform::set_permissions_owner_only(&self.debug_socket_path);

        // Set logging context for this server
        crate::logging::set_server(&self.identity.name);

        // Log server identity
        crate::logging::info(&format!(
            "Server {} starting ({})",
            self.identity.display_name(),
            self.identity.version
        ));
        crate::logging::info(&format!("Server listening on {:?}", self.socket_path));
        crate::logging::info(&format!("Debug socket on {:?}", self.debug_socket_path));

        let server_start_time = Instant::now();

        self.spawn_registry_prewarm();
        let registry_info = self.build_registry_info();
        self.spawn_background_tasks(server_start_time);

        // Note: No default session created here - each client creates its own session
        let runtime = self.runtime();
        let main_handle = runtime.spawn_main_accept_loop(main_listener);
        let debug_handle = runtime.spawn_debug_accept_loop(debug_listener, server_start_time);

        crate::logging::info("Accept loop tasks spawned");

        // Signal readiness to the spawning client only after the accept loops
        // are live, so a "ready" server can immediately handle requests.
        publish_reload_socket_ready();
        signal_ready_fd();

        // Persist auxiliary discovery metadata after the server is already live.
        self.spawn_registry_metadata_publisher(registry_info);

        // Spawn WebSocket gateway for iOS/web clients (if enabled)
        let _gateway_handle = self.spawn_gateway(runtime);

        // Wait for both to complete (they won't normally)
        let _ = tokio::join!(main_handle, debug_handle);
        Ok(())
    }

    /// Spawn the WebSocket gateway if enabled in config.
    /// Returns a task handle that accepts gateway clients and feeds them
    /// into handle_client just like Unix socket connections.
    fn spawn_gateway(&self, runtime: ServerRuntime) -> Option<tokio::task::JoinHandle<()>> {
        let config = if let Some(override_config) = &self.gateway_config_override {
            override_config.clone()
        } else {
            let gw_config = &crate::config::config().gateway;
            crate::gateway::GatewayConfig {
                port: gw_config.port,
                bind_addr: gw_config.bind_addr.clone(),
                enabled: gw_config.enabled,
            }
        };

        if !config.enabled {
            return None;
        }

        let (client_tx, client_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::gateway::GatewayClient>();

        // Spawn the TCP/WebSocket listener
        tokio::spawn(async move {
            if let Err(e) = crate::gateway::run_gateway(config, client_tx).await {
                crate::logging::error(&format!("Gateway error: {}", e));
            }
        });

        Some(runtime.spawn_gateway_accept_loop(client_rx))
    }
}

pub use self::client_api::Client;

#[cfg(test)]
mod tests {
    use super::{SessionInterruptQueues, SwarmMember, dispatch_background_task_completion};
    use crate::agent::Agent;
    use crate::bus::{BackgroundTaskCompleted, BackgroundTaskStatus};
    use crate::message::{Message, Role, StreamEvent, ToolDefinition};
    use crate::protocol::{NotificationType, ServerEvent};
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::sync::{Mutex, RwLock, mpsc};
    use tokio::time::timeout;

    #[derive(Default)]
    struct StreamingMockProvider {
        responses: std::sync::Mutex<Vec<Vec<StreamEvent>>>,
    }

    impl StreamingMockProvider {
        fn queue_response(&self, response: Vec<StreamEvent>) {
            self.responses.lock().unwrap().push(response);
        }
    }

    #[async_trait]
    impl Provider for StreamingMockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            let response = self.responses.lock().unwrap().remove(0);
            Ok(Box::pin(tokio_stream::iter(response.into_iter().map(Ok))))
        }

        fn name(&self) -> &str {
            "test"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(StreamingMockProvider::default())
        }
    }

    async fn test_agent(provider: Arc<dyn Provider>) -> Arc<Mutex<Agent>> {
        let registry = Registry::new(provider.clone()).await;
        Arc::new(Mutex::new(Agent::new(provider, registry)))
    }

    #[tokio::test]
    async fn background_task_wake_runs_live_session_immediately_when_idle() {
        let provider = Arc::new(StreamingMockProvider::default());
        provider.queue_response(vec![
            StreamEvent::TextDelta("Build result processed.".to_string()),
            StreamEvent::MessageEnd { stop_reason: None },
        ]);
        let provider_dyn: Arc<dyn Provider> = provider.clone();
        let agent = test_agent(provider_dyn).await;
        let session_id = agent.lock().await.session_id().to_string();
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let soft_interrupt_queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
        let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            SwarmMember {
                session_id: session_id.clone(),
                event_tx: member_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: None,
                swarm_enabled: false,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("otter".to_string()),
                report_back_to_session_id: None,
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
            },
        )])));
        let task = BackgroundTaskCompleted {
            task_id: "bgwake".to_string(),
            tool_name: "selfdev-build".to_string(),
            session_id: session_id.clone(),
            status: BackgroundTaskStatus::Completed,
            exit_code: Some(0),
            output_preview: "done\n".to_string(),
            output_file: std::env::temp_dir().join("bgwake.output"),
            duration_secs: 1.4,
            notify: true,
            wake: true,
        };

        dispatch_background_task_completion(
            &task,
            &sessions,
            &soft_interrupt_queues,
            &swarm_members,
        )
        .await;

        let notification = timeout(Duration::from_secs(2), async {
            loop {
                match member_event_rx.recv().await {
                    Some(ServerEvent::Notification {
                        notification_type,
                        message,
                        ..
                    }) => return (notification_type, message),
                    Some(_) => continue,
                    None => panic!("member stream closed before notification"),
                }
            }
        })
        .await
        .expect("background task notification should arrive promptly");

        match notification.0 {
            NotificationType::Message { scope, channel } => {
                assert_eq!(scope.as_deref(), Some("background_task"));
                assert!(channel.is_none());
            }
            other => panic!("unexpected notification type: {other:?}"),
        }
        assert!(notification.1.contains("**Background task** `bgwake`"));

        let streamed = timeout(Duration::from_secs(2), async {
            loop {
                match member_event_rx.recv().await {
                    Some(ServerEvent::TextDelta { text })
                        if text.contains("Build result processed.") =>
                    {
                        return text;
                    }
                    Some(_) => continue,
                    None => panic!("member stream closed before wake ran"),
                }
            }
        })
        .await
        .expect("wake delivery should start streaming promptly");
        assert!(streamed.contains("Build result processed."));

        let guard = agent.lock().await;
        assert!(guard.messages().iter().any(|message| {
            message.role == Role::User
                && message
                    .content_preview()
                    .contains("**Background task** `bgwake`")
        }));
    }

    #[tokio::test]
    async fn background_task_notify_without_wake_does_not_queue_soft_interrupt() {
        let provider: Arc<dyn Provider> = Arc::new(StreamingMockProvider::default());
        let agent = test_agent(provider).await;
        let session_id = agent.lock().await.session_id().to_string();
        let queue = agent.lock().await.soft_interrupt_queue();
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));
        let soft_interrupt_queues: SessionInterruptQueues =
            Arc::new(RwLock::new(HashMap::from([(
                session_id.clone(),
                queue.clone(),
            )])));
        let (member_event_tx, mut member_event_rx) = mpsc::unbounded_channel();
        let swarm_members = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            SwarmMember {
                session_id: session_id.clone(),
                event_tx: member_event_tx,
                event_txs: HashMap::new(),
                working_dir: None,
                swarm_id: None,
                swarm_enabled: false,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some("otter".to_string()),
                report_back_to_session_id: None,
                role: "agent".to_string(),
                joined_at: Instant::now(),
                last_status_change: Instant::now(),
                is_headless: false,
            },
        )])));
        let task = BackgroundTaskCompleted {
            task_id: "bgnotify".to_string(),
            tool_name: "bash".to_string(),
            session_id: session_id.clone(),
            status: BackgroundTaskStatus::Completed,
            exit_code: Some(0),
            output_preview: "ok\n".to_string(),
            output_file: std::env::temp_dir().join("bgnotify.output"),
            duration_secs: 0.7,
            notify: true,
            wake: false,
        };

        dispatch_background_task_completion(
            &task,
            &sessions,
            &soft_interrupt_queues,
            &swarm_members,
        )
        .await;

        let notification = timeout(Duration::from_secs(2), member_event_rx.recv())
            .await
            .expect("background task notification should arrive promptly")
            .expect("member stream should stay open");
        match notification {
            ServerEvent::Notification { message, .. } => {
                assert!(message.contains("**Background task** `bgnotify`"));
            }
            other => panic!("expected notification, got {other:?}"),
        }

        let pending = queue.lock().expect("queue lock");
        assert!(
            pending.is_empty(),
            "notify-only delivery should not wake the session"
        );
    }
}
