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
mod debug_client_commands;
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
mod socket;
mod swarm;
mod util;

pub(super) use self::await_members_state::AwaitMembersRuntime;
use self::client_lifecycle::handle_client;
use self::debug::{ClientConnectionInfo, ClientDebugState, handle_debug_client};
use self::debug_jobs::DebugJob;
use self::headless::create_headless_session;
use self::reload::await_reload_signal;
#[allow(unused_imports)]
use self::swarm::{
    broadcast_swarm_plan, broadcast_swarm_status, record_swarm_event,
    record_swarm_event_for_session, remove_plan_participant, remove_session_channel_subscriptions,
    remove_session_from_swarm, rename_plan_participant, run_swarm_message, summarize_plan_items,
    truncate_detail, update_member_status,
};
use crate::agent::{Agent, SoftInterruptSource};
use crate::ambient_runner::AmbientRunnerHandle;
use crate::bus::{Bus, BusEvent, FileOp};
use crate::protocol::{NotificationType, ServerEvent};
use crate::provider::Provider;
use crate::transport::Listener;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast};

mod state;

pub use self::state::{
    FileAccess, SharedContext, SwarmEvent, SwarmEventType, SwarmMember, VersionedPlan,
};
use self::state::{MAX_EVENT_HISTORY, latest_peer_touches};
use self::state::{
    SessionInterruptQueues, enqueue_soft_interrupt, queue_soft_interrupt_for_session,
    register_session_interrupt_queue, remove_session_interrupt_queue,
    rename_session_interrupt_queue,
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

use self::socket::{
    acquire_daemon_lock, mark_close_on_exec, signal_ready_fd, socket_has_live_listener,
};
pub use self::socket::{
    cleanup_socket_pair, connect_socket, debug_socket_path, has_live_listener, is_server_ready,
    set_socket_path, socket_path, spawn_server_notify, wait_for_server_ready,
};

pub use self::util::ServerIdentity;
use self::util::{
    debug_control_allowed, embedding_idle_unload_secs, get_shared_mcp_pool, git_common_dir_for,
    server_has_newer_binary, server_update_candidate, swarm_id_for_dir,
};

#[cfg(test)]
mod socket_tests;

#[cfg(test)]
mod startup_tests;

#[cfg(test)]
mod queue_tests;

#[cfg(test)]
mod file_activity_tests;

/// Set custom socket path (sets JCODE_SOCKET env var)

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
    channel_subscriptions: Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    /// Event history for real-time event subscription (ring buffer)
    event_history: Arc<RwLock<Vec<SwarmEvent>>>,
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
    shutdown_signals: Arc<RwLock<HashMap<String, crate::agent::InterruptSignal>>>,
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
            swarm_members: Arc::new(RwLock::new(HashMap::new())),
            swarms_by_id: Arc::new(RwLock::new(HashMap::new())),
            shared_context: Arc::new(RwLock::new(HashMap::new())),
            swarm_plans: Arc::new(RwLock::new(HashMap::new())),
            swarm_coordinators: Arc::new(RwLock::new(HashMap::new())),
            client_debug_state: Arc::new(RwLock::new(ClientDebugState::default())),
            client_debug_response_tx,
            debug_jobs: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_history: Arc::new(RwLock::new(Vec::new())),
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

    /// Monitor the global Bus for FileTouch events and detect conflicts
    async fn monitor_bus(
        file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
        swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
        _swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
        _swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
        _shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
        sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
        soft_interrupt_queues: SessionInterruptQueues,
        event_history: Arc<RwLock<Vec<SwarmEvent>>>,
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
                        });
                    }

                    // Record event for subscription
                    {
                        let members = swarm_members.read().await;
                        let member = members.get(&session_id);
                        let session_name = member.and_then(|m| m.friendly_name.clone());
                        let swarm_id = member.and_then(|m| m.swarm_id.clone());

                        let event = SwarmEvent {
                            id: event_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                            session_id: session_id.clone(),
                            session_name,
                            swarm_id,
                            event: SwarmEventType::FileTouch {
                                path: path.to_string_lossy().to_string(),
                                op: touch.op.as_str().to_string(),
                                summary: touch.summary.clone(),
                            },
                            timestamp: Instant::now(),
                            absolute_time: std::time::SystemTime::now(),
                        };

                        let mut history = event_history.write().await;
                        history.push(event.clone());
                        if history.len() > MAX_EVENT_HISTORY {
                            history.remove(0);
                        }
                        let _ = swarm_event_tx.send(event);
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

        let registry_info = crate::registry::ServerInfo {
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
        };

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

        // Note: No default session created here - each client creates its own session

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

        // Spawn main socket handler
        let main_sessions = Arc::clone(&self.sessions);
        let main_event_tx = self.event_tx.clone();
        let main_provider = Arc::clone(&self.provider);
        let main_is_processing = Arc::clone(&self.is_processing);
        let main_session_id = Arc::clone(&self.session_id);
        let main_client_count = Arc::clone(&self.client_count);
        let main_client_connections = Arc::clone(&self.client_connections);
        let main_swarm_members = Arc::clone(&self.swarm_members);
        let main_swarms_by_id = Arc::clone(&self.swarms_by_id);
        let main_shared_context = Arc::clone(&self.shared_context);
        let main_swarm_plans = Arc::clone(&self.swarm_plans);
        let main_swarm_coordinators = Arc::clone(&self.swarm_coordinators);
        let main_file_touches = Arc::clone(&self.file_touches);
        let main_channel_subscriptions = Arc::clone(&self.channel_subscriptions);
        let main_client_debug_state = Arc::clone(&self.client_debug_state);
        let main_client_debug_response_tx = self.client_debug_response_tx.clone();
        let main_event_history = Arc::clone(&self.event_history);
        let main_event_counter = Arc::clone(&self.event_counter);
        let main_swarm_event_tx = self.swarm_event_tx.clone();
        let main_server_name = self.identity.name.clone();
        let main_server_icon = self.identity.icon.clone();
        let main_ambient_runner = self.ambient_runner.clone();
        let main_mcp_pool = Arc::clone(&self.mcp_pool);
        let main_shutdown_signals = Arc::clone(&self.shutdown_signals);
        let main_soft_interrupt_queues = Arc::clone(&self.soft_interrupt_queues);
        let main_await_members_runtime = self.await_members_runtime.clone();

        let main_handle = tokio::spawn(async move {
            loop {
                match main_listener.accept().await {
                    Ok((stream, _)) => {
                        let sessions = Arc::clone(&main_sessions);
                        let event_tx = main_event_tx.clone();
                        let provider = Arc::clone(&main_provider);
                        let is_processing = Arc::clone(&main_is_processing);
                        let session_id = Arc::clone(&main_session_id);
                        let client_count = Arc::clone(&main_client_count);
                        let client_connections = Arc::clone(&main_client_connections);
                        let swarm_members = Arc::clone(&main_swarm_members);
                        let swarms_by_id = Arc::clone(&main_swarms_by_id);
                        let shared_context = Arc::clone(&main_shared_context);
                        let swarm_plans = Arc::clone(&main_swarm_plans);
                        let swarm_coordinators = Arc::clone(&main_swarm_coordinators);
                        let file_touches = Arc::clone(&main_file_touches);
                        let channel_subscriptions = Arc::clone(&main_channel_subscriptions);
                        let client_debug_state = Arc::clone(&main_client_debug_state);
                        let client_debug_response_tx = main_client_debug_response_tx.clone();
                        let event_history = Arc::clone(&main_event_history);
                        let event_counter = Arc::clone(&main_event_counter);
                        let swarm_event_tx = main_swarm_event_tx.clone();
                        let server_name = main_server_name.clone();
                        let server_icon = main_server_icon.clone();
                        let ambient_runner = main_ambient_runner.clone();
                        let mcp_pool = Arc::clone(&main_mcp_pool);
                        let shutdown_signals = Arc::clone(&main_shutdown_signals);
                        let soft_interrupt_queues = Arc::clone(&main_soft_interrupt_queues);
                        let await_members_runtime = main_await_members_runtime.clone();

                        // Increment client count
                        *client_count.write().await += 1;

                        tokio::spawn(async move {
                            let mcp_pool = get_shared_mcp_pool(&mcp_pool).await;

                            let result = handle_client(
                                stream,
                                sessions,
                                event_tx,
                                provider,
                                is_processing,
                                session_id,
                                Arc::clone(&client_count),
                                client_connections,
                                swarm_members,
                                swarms_by_id,
                                shared_context,
                                swarm_plans,
                                swarm_coordinators,
                                file_touches,
                                channel_subscriptions,
                                client_debug_state,
                                client_debug_response_tx,
                                event_history,
                                event_counter,
                                swarm_event_tx,
                                server_name,
                                server_icon,
                                mcp_pool,
                                shutdown_signals,
                                soft_interrupt_queues,
                                await_members_runtime,
                            )
                            .await;

                            // Decrement client count when done
                            *client_count.write().await -= 1;

                            // Nudge ambient runner on session close
                            if let Some(ref runner) = ambient_runner {
                                runner.nudge();
                            }

                            if let Err(e) = result {
                                crate::logging::error(&format!("Client error: {}", e));
                            }
                        });
                    }
                    Err(e) => {
                        crate::logging::error(&format!("Main accept error: {}", e));
                    }
                }
            }
        });

        // Spawn debug socket handler
        let debug_sessions = Arc::clone(&self.sessions);
        let debug_is_processing = Arc::clone(&self.is_processing);
        let debug_session_id = Arc::clone(&self.session_id);
        let debug_provider = Arc::clone(&self.provider);
        let debug_client_debug_state = Arc::clone(&self.client_debug_state);
        let debug_client_connections = Arc::clone(&self.client_connections);
        let debug_swarm_members = Arc::clone(&self.swarm_members);
        let debug_swarms_by_id = Arc::clone(&self.swarms_by_id);
        let debug_shared_context = Arc::clone(&self.shared_context);
        let debug_swarm_plans = Arc::clone(&self.swarm_plans);
        let debug_swarm_coordinators = Arc::clone(&self.swarm_coordinators);
        let debug_file_touches = Arc::clone(&self.file_touches);
        let debug_channel_subscriptions = Arc::clone(&self.channel_subscriptions);
        let debug_client_debug_response_tx = self.client_debug_response_tx.clone();
        let debug_jobs = Arc::clone(&self.debug_jobs);
        let debug_event_history = Arc::clone(&self.event_history);
        let debug_event_counter = Arc::clone(&self.event_counter);
        let debug_swarm_event_tx = self.swarm_event_tx.clone();
        let debug_server_identity = self.identity.clone();
        let debug_start_time = std::time::Instant::now();
        let debug_ambient_runner = self.ambient_runner.clone();
        let debug_mcp_pool = Arc::clone(&self.mcp_pool);
        let debug_soft_interrupt_queues = Arc::clone(&self.soft_interrupt_queues);

        let debug_handle = tokio::spawn(async move {
            loop {
                match debug_listener.accept().await {
                    Ok((stream, _)) => {
                        let sessions = Arc::clone(&debug_sessions);
                        let is_processing = Arc::clone(&debug_is_processing);
                        let session_id = Arc::clone(&debug_session_id);
                        let provider = Arc::clone(&debug_provider);
                        let client_debug_state = Arc::clone(&debug_client_debug_state);
                        let client_connections = Arc::clone(&debug_client_connections);
                        let swarm_members = Arc::clone(&debug_swarm_members);
                        let swarms_by_id = Arc::clone(&debug_swarms_by_id);
                        let shared_context = Arc::clone(&debug_shared_context);
                        let swarm_plans = Arc::clone(&debug_swarm_plans);
                        let swarm_coordinators = Arc::clone(&debug_swarm_coordinators);
                        let file_touches = Arc::clone(&debug_file_touches);
                        let channel_subscriptions = Arc::clone(&debug_channel_subscriptions);
                        let client_debug_response_tx = debug_client_debug_response_tx.clone();
                        let debug_jobs = Arc::clone(&debug_jobs);
                        let event_history = Arc::clone(&debug_event_history);
                        let event_counter = Arc::clone(&debug_event_counter);
                        let swarm_event_tx = debug_swarm_event_tx.clone();
                        let server_identity = debug_server_identity.clone();
                        let server_start_time = debug_start_time;
                        let ambient_runner = debug_ambient_runner.clone();
                        let mcp_pool = Arc::clone(&debug_mcp_pool);
                        let soft_interrupt_queues = Arc::clone(&debug_soft_interrupt_queues);

                        tokio::spawn(async move {
                            let mcp_pool = Some(get_shared_mcp_pool(&mcp_pool).await);

                            if let Err(e) = handle_debug_client(
                                stream,
                                sessions,
                                is_processing,
                                session_id,
                                provider,
                                client_connections,
                                swarm_members,
                                swarms_by_id,
                                shared_context,
                                swarm_plans,
                                swarm_coordinators,
                                file_touches,
                                channel_subscriptions,
                                client_debug_state,
                                client_debug_response_tx,
                                debug_jobs,
                                event_history,
                                event_counter,
                                swarm_event_tx,
                                server_identity,
                                server_start_time,
                                ambient_runner,
                                mcp_pool,
                                soft_interrupt_queues,
                            )
                            .await
                            {
                                crate::logging::error(&format!("Debug client error: {}", e));
                            }
                        });
                    }
                    Err(e) => {
                        crate::logging::error(&format!("Debug accept error: {}", e));
                    }
                }
            }
        });

        crate::logging::info("Accept loop tasks spawned");

        // Signal readiness to the spawning client only after the accept loops
        // are live, so a "ready" server can immediately handle requests.
        publish_reload_socket_ready();
        signal_ready_fd();

        // Persist auxiliary discovery metadata after the server is already live.
        let registry_identity = self.identity.display_name();
        let registry_info_for_task = registry_info.clone();
        tokio::spawn(async move {
            let hash_path = format!("{}.hash", registry_info_for_task.socket.display());
            let _ = std::fs::write(&hash_path, env!("JCODE_GIT_HASH"));

            let mut registry = crate::registry::ServerRegistry::load()
                .await
                .unwrap_or_default();
            registry.register(registry_info_for_task);
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

        // Spawn WebSocket gateway for iOS/web clients (if enabled)
        let _gateway_handle = self.spawn_gateway();

        // Wait for both to complete (they won't normally)
        let _ = tokio::join!(main_handle, debug_handle);
        Ok(())
    }

    /// Spawn the WebSocket gateway if enabled in config.
    /// Returns a task handle that accepts gateway clients and feeds them
    /// into handle_client just like Unix socket connections.
    fn spawn_gateway(&self) -> Option<tokio::task::JoinHandle<()>> {
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

        let (client_tx, mut client_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::gateway::GatewayClient>();

        // Spawn the TCP/WebSocket listener
        tokio::spawn(async move {
            if let Err(e) = crate::gateway::run_gateway(config, client_tx).await {
                crate::logging::error(&format!("Gateway error: {}", e));
            }
        });

        // Spawn a task that receives gateway clients and plugs them into handle_client
        let gw_sessions = Arc::clone(&self.sessions);
        let gw_event_tx = self.event_tx.clone();
        let gw_provider = Arc::clone(&self.provider);
        let gw_is_processing = Arc::clone(&self.is_processing);
        let gw_session_id = Arc::clone(&self.session_id);
        let gw_client_count = Arc::clone(&self.client_count);
        let gw_client_connections = Arc::clone(&self.client_connections);
        let gw_swarm_members = Arc::clone(&self.swarm_members);
        let gw_swarms_by_id = Arc::clone(&self.swarms_by_id);
        let gw_shared_context = Arc::clone(&self.shared_context);
        let gw_swarm_plans = Arc::clone(&self.swarm_plans);
        let gw_swarm_coordinators = Arc::clone(&self.swarm_coordinators);
        let gw_file_touches = Arc::clone(&self.file_touches);
        let gw_channel_subscriptions = Arc::clone(&self.channel_subscriptions);
        let gw_client_debug_state = Arc::clone(&self.client_debug_state);
        let gw_client_debug_response_tx = self.client_debug_response_tx.clone();
        let gw_event_history = Arc::clone(&self.event_history);
        let gw_event_counter = Arc::clone(&self.event_counter);
        let gw_swarm_event_tx = self.swarm_event_tx.clone();
        let gw_server_name = self.identity.name.clone();
        let gw_server_icon = self.identity.icon.clone();
        let gw_ambient_runner = self.ambient_runner.clone();
        let gw_mcp_pool = Arc::clone(&self.mcp_pool);
        let gw_shutdown_signals = Arc::clone(&self.shutdown_signals);
        let gw_soft_interrupt_queues = Arc::clone(&self.soft_interrupt_queues);
        let gw_await_members_runtime = self.await_members_runtime.clone();

        let handle = tokio::spawn(async move {
            while let Some(gw_client) = client_rx.recv().await {
                let sessions = Arc::clone(&gw_sessions);
                let event_tx = gw_event_tx.clone();
                let provider = Arc::clone(&gw_provider);
                let is_processing = Arc::clone(&gw_is_processing);
                let session_id = Arc::clone(&gw_session_id);
                let client_count = Arc::clone(&gw_client_count);
                let client_connections = Arc::clone(&gw_client_connections);
                let swarm_members = Arc::clone(&gw_swarm_members);
                let swarms_by_id = Arc::clone(&gw_swarms_by_id);
                let shared_context = Arc::clone(&gw_shared_context);
                let swarm_plans = Arc::clone(&gw_swarm_plans);
                let swarm_coordinators = Arc::clone(&gw_swarm_coordinators);
                let file_touches = Arc::clone(&gw_file_touches);
                let channel_subscriptions = Arc::clone(&gw_channel_subscriptions);
                let client_debug_state = Arc::clone(&gw_client_debug_state);
                let client_debug_response_tx = gw_client_debug_response_tx.clone();
                let event_history = Arc::clone(&gw_event_history);
                let event_counter = Arc::clone(&gw_event_counter);
                let swarm_event_tx = gw_swarm_event_tx.clone();
                let server_name = gw_server_name.clone();
                let server_icon = gw_server_icon.clone();
                let _ambient_runner = gw_ambient_runner.clone();
                let mcp_pool = Arc::clone(&gw_mcp_pool);
                let shutdown_signals = Arc::clone(&gw_shutdown_signals);
                let soft_interrupt_queues = Arc::clone(&gw_soft_interrupt_queues);
                let await_members_runtime = gw_await_members_runtime.clone();

                *client_count.write().await += 1;

                crate::logging::info(&format!(
                    "Gateway client connected: {} ({})",
                    gw_client.device_name, gw_client.device_id
                ));

                tokio::spawn(async move {
                    let mcp_pool = get_shared_mcp_pool(&mcp_pool).await;

                    let result = handle_client(
                        gw_client.stream,
                        sessions,
                        event_tx,
                        provider,
                        is_processing,
                        session_id,
                        Arc::clone(&client_count),
                        client_connections,
                        swarm_members,
                        swarms_by_id,
                        shared_context,
                        swarm_plans,
                        swarm_coordinators,
                        file_touches,
                        channel_subscriptions,
                        client_debug_state,
                        client_debug_response_tx,
                        event_history,
                        event_counter,
                        swarm_event_tx,
                        server_name,
                        server_icon,
                        mcp_pool,
                        shutdown_signals,
                        soft_interrupt_queues,
                        await_members_runtime,
                    )
                    .await;

                    *client_count.write().await -= 1;

                    if let Err(e) = result {
                        crate::logging::error(&format!("Gateway client error: {}", e));
                    }
                });
            }
        });

        Some(handle)
    }
}

pub use self::client_api::Client;
