#![allow(dead_code)]
#![allow(dead_code)]

use crate::agent::Agent;
use crate::build;
use crate::bus::{Bus, BusEvent, FileOp};
use crate::protocol::{
    decode_request, encode_event, AgentInfo, ContextEntry, HistoryMessage, NotificationType,
    Request, ServerEvent,
};
use crate::provider::Provider;
use crate::tool::Registry;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

/// Record of a file access by an agent
#[derive(Clone, Debug)]
pub struct FileAccess {
    pub session_id: String,
    pub op: FileOp,
    pub timestamp: Instant,
    pub summary: Option<String>,
}

/// Information about a session in a swarm
#[derive(Clone, Debug)]
pub struct SwarmMember {
    pub session_id: String,
    /// Channel to send events to this session
    pub event_tx: mpsc::UnboundedSender<ServerEvent>,
    /// Working directory (used for auto-swarm by cwd)
    pub working_dir: Option<PathBuf>,
    /// Friendly name like "fox"
    pub friendly_name: Option<String>,
}

/// A shared context entry stored by the server
#[derive(Clone, Debug)]
pub struct SharedContext {
    pub key: String,
    pub value: String,
    pub from_session: String,
    pub from_name: Option<String>,
}

/// Socket path for main communication
/// Can be overridden via JCODE_SOCKET env var
pub fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("JCODE_SOCKET") {
        return PathBuf::from(custom);
    }
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    runtime_dir.join("jcode.sock")
}

/// Debug socket path for testing/introspection
/// Derived from main socket path
pub fn debug_socket_path() -> PathBuf {
    let main_path = socket_path();
    let filename = main_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("jcode.sock");
    let debug_filename = filename.replace(".sock", "-debug.sock");
    main_path.with_file_name(debug_filename)
}

/// Set custom socket path (sets JCODE_SOCKET env var)
pub fn set_socket_path(path: &str) {
    std::env::set_var("JCODE_SOCKET", path);
}

/// Idle timeout for self-dev server (5 minutes)
const IDLE_TIMEOUT_SECS: u64 = 300;

/// Self-dev socket path (used for detection when env var isn't set)
const SELFDEV_SOCKET: &str = "/tmp/jcode-selfdev.sock";

fn is_selfdev_env() -> bool {
    if std::env::var("JCODE_SELFDEV_MODE").is_ok() {
        return true;
    }
    if std::env::var("JCODE_SOCKET").ok().as_deref() == Some(SELFDEV_SOCKET) {
        return true;
    }
    std::env::current_dir()
        .ok()
        .map(|p| crate::build::is_jcode_repo(&p))
        .unwrap_or(false)
}

fn is_jcode_repo_or_parent(path: &std::path::Path) -> bool {
    let mut current = Some(path);
    while let Some(dir) = current {
        if crate::build::is_jcode_repo(dir) {
            return true;
        }
        current = dir.parent();
    }
    false
}

fn debug_control_allowed() -> bool {
    if is_selfdev_env() {
        return true;
    }
    // Check config file setting
    if crate::config::config().display.debug_socket {
        return true;
    }
    if std::env::var("JCODE_DEBUG_CONTROL")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        return true;
    }
    // Check for file-based toggle (allows enabling without restart)
    if let Ok(jcode_dir) = crate::storage::jcode_dir() {
        if jcode_dir.join("debug_control").exists() {
            return true;
        }
    }
    false
}

fn server_update_candidate() -> Option<(PathBuf, &'static str)> {
    if is_selfdev_env() {
        if let Ok(canary) = crate::build::canary_binary_path() {
            if canary.exists() {
                return Some((canary, "canary"));
            }
        }
        if let Ok(stable) = crate::build::stable_binary_path() {
            if stable.exists() {
                return Some((stable, "stable"));
            }
        }
    }

    let repo_dir = crate::build::get_repo_dir()?;
    let exe = repo_dir.join("target/release/jcode");
    if exe.exists() {
        return Some((exe, "release"));
    }
    None
}

fn canonicalize_or(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn server_has_newer_binary() -> bool {
    let current_exe = std::env::current_exe().ok();
    let startup_mtime = current_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let Some((candidate, _label)) = server_update_candidate() else {
        return false;
    };

    if let Some(current_exe) = current_exe {
        let current = canonicalize_or(current_exe);
        let candidate_path = canonicalize_or(candidate.clone());
        if candidate_path != current {
            return true;
        }
    }

    if let Some(startup_mtime) = startup_mtime {
        if let Ok(metadata) = std::fs::metadata(&candidate) {
            if let Ok(current_mtime) = metadata.modified() {
                return current_mtime > startup_mtime;
            }
        }
    }

    false
}

/// Exit code when server shuts down due to idle timeout
pub const EXIT_IDLE_TIMEOUT: i32 = 44;

/// Server state
pub struct Server {
    provider: Arc<dyn Provider>,
    socket_path: PathBuf,
    debug_socket_path: PathBuf,
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
    /// Track file touches: path -> list of accesses
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    /// Swarm members: session_id -> SwarmMember info
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    /// Swarm groupings by working directory: canonical cwd -> set of session_ids
    swarms_by_cwd: Arc<RwLock<HashMap<PathBuf, HashSet<String>>>>,
    /// Shared context by swarm (cwd -> key -> SharedContext)
    shared_context: Arc<RwLock<HashMap<PathBuf, HashMap<String, SharedContext>>>>,
    /// Channel to forward client debug commands to TUI (request_id, command)
    client_debug_tx: Arc<RwLock<Option<mpsc::UnboundedSender<(u64, String)>>>>,
    /// Channel to receive client debug responses from TUI (request_id, response)
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
}

impl Server {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        let (client_debug_response_tx, _) = broadcast::channel(64);
        Self {
            provider,
            socket_path: socket_path(),
            debug_socket_path: debug_socket_path(),
            event_tx,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            is_processing: Arc::new(RwLock::new(false)),
            session_id: Arc::new(RwLock::new(String::new())),
            client_count: Arc::new(RwLock::new(0)),
            file_touches: Arc::new(RwLock::new(HashMap::new())),
            swarm_members: Arc::new(RwLock::new(HashMap::new())),
            swarms_by_cwd: Arc::new(RwLock::new(HashMap::new())),
            shared_context: Arc::new(RwLock::new(HashMap::new())),
            client_debug_tx: Arc::new(RwLock::new(None)),
            client_debug_response_tx,
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

    /// Monitor the global Bus for FileTouch events and detect conflicts
    async fn monitor_bus(
        file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
        swarms_by_cwd: Arc<RwLock<HashMap<PathBuf, HashSet<String>>>>,
        sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    ) {
        let mut receiver = Bus::global().subscribe();

        loop {
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
                            summary: touch.summary.clone(),
                        });
                    }

                    // Find the swarm this session belongs to
                    let swarm_session_ids: Vec<String> = {
                        let members = swarm_members.read().await;
                        if let Some(member) = members.get(&session_id) {
                            if let Some(ref cwd) = member.working_dir {
                                let swarms = swarms_by_cwd.read().await;
                                if let Some(swarm) = swarms.get(cwd) {
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

                    // Check if any other session in the same swarm has touched this file
                    let previous_touches: Vec<FileAccess> = {
                        let touches = file_touches.read().await;
                        if let Some(accesses) = touches.get(&path) {
                            accesses
                                .iter()
                                .filter(|a| {
                                    a.session_id != session_id
                                        && swarm_session_ids.contains(&a.session_id)
                                })
                                .cloned()
                                .collect()
                        } else {
                            vec![]
                        }
                    };

                    // If there are previous touches from swarm members, send alerts
                    if !previous_touches.is_empty() {
                        let members = swarm_members.read().await;
                        let current_member = members.get(&session_id);
                        let current_name = current_member.and_then(|m| m.friendly_name.clone());
                        let agent_sessions = sessions.read().await;

                        // Alert the current agent about previous touches
                        if let Some(member) = current_member {
                            for prev in &previous_touches {
                                let prev_member = members.get(&prev.session_id);
                                let prev_name = prev_member.and_then(|m| m.friendly_name.clone());
                                let alert_msg = format!(
                                    "File conflict: {} ({}) - Another agent ({}) previously {} this file{}",
                                    path.display(),
                                    touch.op.as_str(),
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

                                // Also push to the agent's pending alerts
                                if let Some(agent) = agent_sessions.get(&session_id) {
                                    if let Ok(mut agent) = agent.try_lock() {
                                        agent.push_alert(alert_msg);
                                    }
                                }
                            }
                        }

                        // Alert previous agents about the current touch
                        for prev in &previous_touches {
                            if let Some(prev_member) = members.get(&prev.session_id) {
                                let alert_msg = format!(
                                    "File conflict: {} - Another agent ({}) just {} this file you previously worked with{}",
                                    path.display(),
                                    current_name.as_deref().unwrap_or(&session_id[..8.min(session_id.len())]),
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

                                // Also push to the agent's pending alerts
                                if let Some(agent) = agent_sessions.get(&prev.session_id) {
                                    if let Ok(mut agent) = agent.try_lock() {
                                        agent.push_alert(alert_msg);
                                    }
                                }
                            }
                        }
                    }
                }
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
        // Remove existing sockets
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(&self.debug_socket_path);

        let main_listener = UnixListener::bind(&self.socket_path)?;
        let debug_listener = UnixListener::bind(&self.debug_socket_path)?;

        crate::logging::info(&format!("Server listening on {:?}", self.socket_path));
        crate::logging::info(&format!("Debug socket on {:?}", self.debug_socket_path));

        // Spawn selfdev signal monitor (checks for reload/rollback signals)
        tokio::spawn(async {
            monitor_selfdev_signals().await;
        });

        // Spawn the bus monitor for swarm coordination
        let monitor_file_touches = Arc::clone(&self.file_touches);
        let monitor_swarm_members = Arc::clone(&self.swarm_members);
        let monitor_swarms_by_cwd = Arc::clone(&self.swarms_by_cwd);
        let monitor_sessions = Arc::clone(&self.sessions);
        tokio::spawn(async move {
            Self::monitor_bus(
                monitor_file_touches,
                monitor_swarm_members,
                monitor_swarms_by_cwd,
                monitor_sessions,
            )
            .await;
        });

        // Note: No default session created here - each client creates its own session

        // Spawn idle timeout monitor (for self-dev mode)
        // Server exits after IDLE_TIMEOUT_SECS with no connected clients
        let idle_client_count = Arc::clone(&self.client_count);
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

        // Spawn main socket handler
        let main_sessions = Arc::clone(&self.sessions);
        let main_event_tx = self.event_tx.clone();
        let main_provider = Arc::clone(&self.provider);
        let main_is_processing = Arc::clone(&self.is_processing);
        let main_session_id = Arc::clone(&self.session_id);
        let main_client_count = Arc::clone(&self.client_count);
        let main_swarm_members = Arc::clone(&self.swarm_members);
        let main_swarms_by_cwd = Arc::clone(&self.swarms_by_cwd);
        let main_shared_context = Arc::clone(&self.shared_context);
        let main_file_touches = Arc::clone(&self.file_touches);
        let main_client_debug_tx = Arc::clone(&self.client_debug_tx);
        let main_client_debug_response_tx = self.client_debug_response_tx.clone();

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
                        let swarm_members = Arc::clone(&main_swarm_members);
                        let swarms_by_cwd = Arc::clone(&main_swarms_by_cwd);
                        let shared_context = Arc::clone(&main_shared_context);
                        let file_touches = Arc::clone(&main_file_touches);
                        let client_debug_tx = Arc::clone(&main_client_debug_tx);
                        let client_debug_response_tx = main_client_debug_response_tx.clone();

                        // Increment client count
                        *client_count.write().await += 1;

                        tokio::spawn(async move {
                            let result = handle_client(
                                stream,
                                sessions,
                                event_tx,
                                provider,
                                is_processing,
                                session_id,
                                Arc::clone(&client_count),
                                swarm_members,
                                swarms_by_cwd,
                                shared_context,
                                file_touches,
                                client_debug_tx,
                                client_debug_response_tx,
                            )
                            .await;

                            // Decrement client count when done
                            *client_count.write().await -= 1;

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
        let debug_client_debug_tx = Arc::clone(&self.client_debug_tx);
        let debug_client_debug_response_tx = self.client_debug_response_tx.clone();

        let debug_handle = tokio::spawn(async move {
            loop {
                match debug_listener.accept().await {
                    Ok((stream, _)) => {
                        let sessions = Arc::clone(&debug_sessions);
                        let is_processing = Arc::clone(&debug_is_processing);
                        let session_id = Arc::clone(&debug_session_id);
                        let provider = Arc::clone(&debug_provider);
                        let client_debug_tx = Arc::clone(&debug_client_debug_tx);
                        let client_debug_response_tx = debug_client_debug_response_tx.clone();

                        tokio::spawn(async move {
                            if let Err(e) = handle_debug_client(
                                stream,
                                sessions,
                                is_processing,
                                session_id,
                                provider,
                                client_debug_tx,
                                client_debug_response_tx,
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

        // Wait for both to complete (they won't normally)
        let _ = tokio::join!(main_handle, debug_handle);
        Ok(())
    }
}

async fn handle_client(
    stream: UnixStream,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    _global_event_tx: broadcast::Sender<ServerEvent>,
    provider_template: Arc<dyn Provider>,
    _global_is_processing: Arc<RwLock<bool>>,
    global_session_id: Arc<RwLock<String>>,
    client_count: Arc<RwLock<usize>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_cwd: Arc<RwLock<HashMap<PathBuf, HashSet<String>>>>,
    shared_context: Arc<RwLock<HashMap<PathBuf, HashMap<String, SharedContext>>>>,
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    client_debug_tx: Arc<RwLock<Option<mpsc::UnboundedSender<(u64, String)>>>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let mut line = String::new();

    // Per-client state
    let mut client_is_processing = false;
    let (processing_done_tx, mut processing_done_rx) =
        mpsc::unbounded_channel::<(u64, Result<()>)>();
    let mut processing_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut processing_message_id: Option<u64> = None;
    let mut client_selfdev = is_selfdev_env();

    let provider = provider_template.fork();
    let registry = Registry::new(provider.clone()).await;

    // Create a new session for this client
    let mut new_agent = Agent::new(Arc::clone(&provider), registry.clone());
    let client_session_id = new_agent.session_id().to_string();
    let friendly_name = new_agent.session_short_name().map(|s| s.to_string());

    {
        let mut current = global_session_id.write().await;
        if current.is_empty() || *current != client_session_id {
            *current = client_session_id.clone();
        }
    }

    // Enable self-dev mode when running in a self-dev environment
    if client_selfdev {
        new_agent.set_canary("self-dev");
        registry.register_selfdev_tools().await;
    }

    let agent = Arc::new(Mutex::new(new_agent));
    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.insert(client_session_id.clone(), Arc::clone(&agent));
    }

    // Per-client event channel (not shared with other clients)
    let (client_event_tx, mut client_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ServerEvent>();

    // Get the working directory for swarm grouping
    let working_dir = std::env::current_dir().ok();

    // Register this client as a swarm member
    {
        let mut members = swarm_members.write().await;
        members.insert(
            client_session_id.clone(),
            SwarmMember {
                session_id: client_session_id.clone(),
                event_tx: client_event_tx.clone(),
                working_dir: working_dir.clone(),
                friendly_name: friendly_name.clone(),
            },
        );

        // Add to swarm by working directory
        if let Some(ref cwd) = working_dir {
            let mut swarms = swarms_by_cwd.write().await;
            swarms
                .entry(cwd.clone())
                .or_insert_with(HashSet::new)
                .insert(client_session_id.clone());
        }
    }

    // Spawn event forwarder for this client only
    let writer_clone = Arc::clone(&writer);
    let event_handle = tokio::spawn(async move {
        while let Some(event) = client_event_rx.recv().await {
            let json = encode_event(&event);
            let mut w = writer_clone.lock().await;
            if w.write_all(json.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    // Send initial session ID to client
    let _ = client_event_tx.send(ServerEvent::SessionId {
        session_id: client_session_id.clone(),
    });

    // Set up client debug command channel
    // This client becomes the "active" debug client that receives client: commands
    let (debug_cmd_tx, mut debug_cmd_rx) = mpsc::unbounded_channel::<(u64, String)>();
    {
        let mut tx_guard = client_debug_tx.write().await;
        *tx_guard = Some(debug_cmd_tx);
    }

    loop {
        line.clear();
        tokio::select! {
            // Handle client debug commands from debug socket
            debug_cmd = debug_cmd_rx.recv() => {
                if let Some((request_id, command)) = debug_cmd {
                    let response = execute_client_debug_command(&command);
                    let _ = client_debug_response_tx.send((request_id, response));
                }
                continue;
            }
            done = processing_done_rx.recv() => {
                if let Some((done_id, result)) = done {
                    if Some(done_id) != processing_message_id {
                        continue;
                    }
                    processing_message_id = None;
                    processing_task = None;
                    client_is_processing = false;

                    match result {
                        Ok(()) => {
                            let _ = client_event_tx.send(ServerEvent::Done { id: done_id });
                        }
                        Err(e) => {
                            let _ = client_event_tx.send(ServerEvent::Error {
                                id: done_id,
                                message: e.to_string(),
                            });
                        }
                    }
                } else {
                    break;
                }
                continue;
            }
            n = reader.read_line(&mut line) => {
                let n = n?;
                if n == 0 {
                    break; // Client disconnected
                }
            }
        }

        let request = match decode_request(&line) {
            Ok(r) => r,
            Err(e) => {
                let event = ServerEvent::Error {
                    id: 0,
                    message: format!("Invalid request: {}", e),
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
                continue;
            }
        };

        // Send ack
        let ack = ServerEvent::Ack { id: request.id() };
        let json = encode_event(&ack);
        {
            let mut w = writer.lock().await;
            w.write_all(json.as_bytes()).await?;
        }

        match request {
            Request::Message { id, content } => {
                // Check if this client is already processing
                if client_is_processing {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Already processing a message".to_string(),
                    });
                    continue;
                }

                // Set processing flag for this client
                client_is_processing = true;
                processing_message_id = Some(id);

                let agent = Arc::clone(&agent);
                let tx = client_event_tx.clone();
                let done_tx = processing_done_tx.clone();
                processing_task = Some(tokio::spawn(async move {
                    let result = process_message_streaming_mpsc(agent, &content, tx).await;
                    let _ = done_tx.send((id, result));
                }));
            }

            Request::Cancel { id } => {
                let _ = id; // cancel request id (not the message id)
                if let Some(handle) = processing_task.take() {
                    if handle.is_finished() {
                        processing_task = Some(handle);
                        continue;
                    }
                    handle.abort();
                    processing_task = None;
                    client_is_processing = false;
                    if let Some(message_id) = processing_message_id.take() {
                        let _ = client_event_tx.send(ServerEvent::TextDelta {
                            text: "Interrupted".to_string(),
                        });
                        let _ = client_event_tx.send(ServerEvent::Done { id: message_id });
                    }
                }
            }

            Request::Clear { id } => {
                // Clear this client's session (create new agent)
                let mut new_agent = Agent::new(Arc::clone(&provider), registry.clone());
                let new_id = new_agent.session_id().to_string();

                // Enable self-dev mode when running in a self-dev environment
                if client_selfdev {
                    new_agent.set_canary("self-dev");
                    // selfdev tools should already be registered from initial connection
                }

                // Replace the agent in place
                let mut agent_guard = agent.lock().await;
                *agent_guard = new_agent;
                drop(agent_guard);

                // Update sessions map
                {
                    let mut sessions_guard = sessions.write().await;
                    sessions_guard.remove(&client_session_id);
                    sessions_guard.insert(new_id.clone(), Arc::clone(&agent));
                }

                let _ = client_event_tx.send(ServerEvent::SessionId { session_id: new_id });
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::Ping { id } => {
                let json = encode_event(&ServerEvent::Pong { id });
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
            }

            Request::GetState { id } => {
                let sessions_guard = sessions.read().await;
                let all_sessions: Vec<String> = sessions_guard.keys().cloned().collect();
                let session_count = all_sessions.len();
                drop(sessions_guard);

                let event = ServerEvent::State {
                    id,
                    session_id: client_session_id.clone(),
                    message_count: session_count,
                    is_processing: client_is_processing,
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
            }

            Request::Subscribe {
                id,
                working_dir,
                selfdev,
            } => {
                let mut should_selfdev = client_selfdev;
                if matches!(selfdev, Some(true)) {
                    should_selfdev = true;
                }

                if !should_selfdev {
                    if let Some(ref dir) = working_dir {
                        let path = PathBuf::from(dir);
                        if is_jcode_repo_or_parent(&path) {
                            should_selfdev = true;
                        }
                    }
                }

                if should_selfdev {
                    client_selfdev = true;
                    let mut agent_guard = agent.lock().await;
                    if !agent_guard.is_canary() {
                        agent_guard.set_canary("self-dev");
                    }
                    drop(agent_guard);
                    registry.register_selfdev_tools().await;
                }

                // Send this client's session ID
                let json = encode_event(&ServerEvent::SessionId {
                    session_id: client_session_id.clone(),
                });
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::GetHistory { id } => {
                let (messages, is_canary, provider_name, provider_model, available_models) = {
                    let agent_guard = agent.lock().await;
                    (
                        agent_guard.get_history(),
                        agent_guard.is_canary(),
                        agent_guard.provider_name(),
                        agent_guard.provider_model(),
                        agent_guard.available_models(),
                    )
                };

                // Get all session IDs and client count
                let (all_sessions, current_client_count) = {
                    let sessions_guard = sessions.read().await;
                    let all: Vec<String> = sessions_guard.keys().cloned().collect();
                    let count = *client_count.read().await;
                    (all, count)
                };

                let event = ServerEvent::History {
                    id,
                    session_id: client_session_id.clone(),
                    messages,
                    provider_name: Some(provider_name),
                    provider_model: Some(provider_model),
                    available_models: available_models.iter().map(|m| (*m).to_string()).collect(),
                    mcp_servers: Vec::new(),
                    skills: Vec::new(),
                    total_tokens: None,
                    all_sessions,
                    client_count: Some(current_client_count),
                    is_canary: Some(is_canary),
                    server_version: Some(env!("JCODE_VERSION").to_string()),
                    server_has_update: Some(server_has_newer_binary()),
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
            }

            Request::DebugCommand { id, .. } => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: "debug_command is only supported on the debug socket".to_string(),
                });
            }

            Request::Reload { id } => {
                // Notify this client that server is reloading
                let _ = client_event_tx.send(ServerEvent::Reloading { new_socket: None });

                // Spawn reload process with progress streaming
                let progress_tx = client_event_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if let Err(e) = do_server_reload_with_progress(progress_tx.clone()).await {
                        let _ = progress_tx.send(ServerEvent::ReloadProgress {
                            step: "error".to_string(),
                            message: format!("Reload failed: {}", e),
                            success: Some(false),
                            output: None,
                        });
                        crate::logging::error(&format!("Reload failed: {}", e));
                    }
                });

                // Send Done after starting the reload (client will reconnect after server restarts)
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::ResumeSession { id, session_id } => {
                // Load the specified session into this client's agent
                let (result, is_canary) = {
                    let mut agent_guard = agent.lock().await;
                    let result = agent_guard.restore_session(&session_id);
                    if client_selfdev || is_selfdev_env() {
                        agent_guard.set_canary("self-dev");
                    }
                    let is_canary = agent_guard.is_canary();
                    (result, is_canary)
                };

                if result.is_ok() && is_canary {
                    client_selfdev = true;
                    registry.register_selfdev_tools().await;
                }

                match result {
                    Ok(()) => {
                        // Send updated history to client
                        let (messages, is_canary, provider_name, provider_model, available_models) = {
                            let agent_guard = agent.lock().await;
                            (
                                agent_guard.get_history(),
                                agent_guard.is_canary(),
                                agent_guard.provider_name(),
                                agent_guard.provider_model(),
                                agent_guard.available_models(),
                            )
                        };

                        let (all_sessions, current_client_count) = {
                            let sessions_guard = sessions.read().await;
                            let all: Vec<String> = sessions_guard.keys().cloned().collect();
                            let count = *client_count.read().await;
                            (all, count)
                        };

                        let event = ServerEvent::History {
                            id,
                            session_id: session_id.clone(),
                            messages,
                            provider_name: Some(provider_name),
                            provider_model: Some(provider_model),
                            available_models: available_models
                                .iter()
                                .map(|m| (*m).to_string())
                                .collect(),
                            mcp_servers: Vec::new(),
                            skills: Vec::new(),
                            total_tokens: None,
                            all_sessions,
                            client_count: Some(current_client_count),
                            is_canary: Some(is_canary),
                            server_version: Some(env!("JCODE_VERSION").to_string()),
                            server_has_update: Some(server_has_newer_binary()),
                        };
                        let json = encode_event(&event);
                        let mut w = writer.lock().await;
                        w.write_all(json.as_bytes()).await?;
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Failed to restore session: {}", e),
                        });
                    }
                }
            }

            Request::CycleModel { id, direction } => {
                let models = {
                    let agent_guard = agent.lock().await;
                    agent_guard.available_models()
                };
                if models.is_empty() {
                    let model = {
                        let agent_guard = agent.lock().await;
                        agent_guard.provider_model()
                    };
                    let _ = client_event_tx.send(ServerEvent::ModelChanged {
                        id,
                        model,
                        error: Some(
                            "Model switching is not available for this provider.".to_string(),
                        ),
                    });
                    continue;
                }

                let current = {
                    let agent_guard = agent.lock().await;
                    agent_guard.provider_model()
                };
                let current_index = models.iter().position(|m| *m == current).unwrap_or(0);
                let len = models.len();
                let next_index = if direction >= 0 {
                    (current_index + 1) % len
                } else {
                    (current_index + len - 1) % len
                };
                let next_model = models[next_index];

                let result = {
                    let mut agent_guard = agent.lock().await;
                    let result = agent_guard.set_model(next_model);
                    if result.is_ok() {
                        agent_guard.reset_provider_session();
                    }
                    result.map(|_| agent_guard.provider_model())
                };

                match result {
                    Ok(updated) => {
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: updated,
                            error: None,
                        });
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: current,
                            error: Some(e.to_string()),
                        });
                    }
                }
            }

            Request::SetModel { id, model } => {
                let models = {
                    let agent_guard = agent.lock().await;
                    agent_guard.available_models()
                };
                if models.is_empty() {
                    let current = {
                        let agent_guard = agent.lock().await;
                        agent_guard.provider_model()
                    };
                    let _ = client_event_tx.send(ServerEvent::ModelChanged {
                        id,
                        model: current,
                        error: Some(
                            "Model switching is not available for this provider.".to_string(),
                        ),
                    });
                    continue;
                }

                let current = {
                    let agent_guard = agent.lock().await;
                    agent_guard.provider_model()
                };
                let result = {
                    let mut agent_guard = agent.lock().await;
                    let result = agent_guard.set_model(&model);
                    if result.is_ok() {
                        agent_guard.reset_provider_session();
                    }
                    result.map(|_| agent_guard.provider_model())
                };
                match result {
                    Ok(updated) => {
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: updated,
                            error: None,
                        });
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: current,
                            error: Some(e.to_string()),
                        });
                    }
                }
            }

            // Agent-to-agent communication
            Request::AgentRegister { id, .. } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::AgentTask { id, task, .. } => {
                // Process as a message on this client's agent
                let result = process_message_streaming_mpsc(
                    Arc::clone(&agent),
                    &task,
                    client_event_tx.clone(),
                )
                .await;
                match result {
                    Ok(()) => {
                        let _ = client_event_tx.send(ServerEvent::Done { id });
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: e.to_string(),
                        });
                    }
                }
            }

            Request::AgentCapabilities { id } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::AgentContext { id } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            // === Agent communication ===
            Request::CommShare {
                id,
                session_id: req_session_id,
                key,
                value,
            } => {
                // Find the swarm (cwd) for this session
                let cwd = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.working_dir.clone())
                };

                if let Some(cwd) = cwd {
                    let friendly_name = {
                        let members = swarm_members.read().await;
                        members
                            .get(&req_session_id)
                            .and_then(|m| m.friendly_name.clone())
                    };

                    // Store the shared context
                    {
                        let mut ctx = shared_context.write().await;
                        let swarm_ctx = ctx.entry(cwd.clone()).or_insert_with(HashMap::new);
                        swarm_ctx.insert(
                            key.clone(),
                            SharedContext {
                                key: key.clone(),
                                value: value.clone(),
                                from_session: req_session_id.clone(),
                                from_name: friendly_name.clone(),
                            },
                        );
                    }

                    // Notify other swarm members
                    let swarm_session_ids: Vec<String> = {
                        let swarms = swarms_by_cwd.read().await;
                        swarms
                            .get(&cwd)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default()
                    };

                    let members = swarm_members.read().await;
                    for sid in &swarm_session_ids {
                        if sid != &req_session_id {
                            if let Some(member) = members.get(sid) {
                                let _ = member.event_tx.send(ServerEvent::Notification {
                                    from_session: req_session_id.clone(),
                                    from_name: friendly_name.clone(),
                                    notification_type: NotificationType::SharedContext {
                                        key: key.clone(),
                                        value: value.clone(),
                                    },
                                    message: format!("Shared context: {} = {}", key, value),
                                });
                            }
                        }
                    }
                }
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommRead {
                id,
                session_id: req_session_id,
                key,
            } => {
                // Find the swarm (cwd) for this session
                let cwd = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.working_dir.clone())
                };

                let entries = if let Some(cwd) = cwd {
                    let ctx = shared_context.read().await;
                    if let Some(swarm_ctx) = ctx.get(&cwd) {
                        let entries: Vec<ContextEntry> = if let Some(k) = key {
                            // Get specific key
                            swarm_ctx
                                .get(&k)
                                .map(|c| {
                                    vec![ContextEntry {
                                        key: c.key.clone(),
                                        value: c.value.clone(),
                                        from_session: c.from_session.clone(),
                                        from_name: c.from_name.clone(),
                                    }]
                                })
                                .unwrap_or_default()
                        } else {
                            // Get all
                            swarm_ctx
                                .values()
                                .map(|c| ContextEntry {
                                    key: c.key.clone(),
                                    value: c.value.clone(),
                                    from_session: c.from_session.clone(),
                                    from_name: c.from_name.clone(),
                                })
                                .collect()
                        };
                        entries
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                };

                let _ = client_event_tx.send(ServerEvent::CommContext { id, entries });
            }

            Request::CommMessage {
                id,
                from_session,
                message,
            } => {
                // Find the swarm (cwd) for this session
                let cwd = {
                    let members = swarm_members.read().await;
                    members
                        .get(&from_session)
                        .and_then(|m| m.working_dir.clone())
                };

                if let Some(cwd) = cwd {
                    let friendly_name = {
                        let members = swarm_members.read().await;
                        members
                            .get(&from_session)
                            .and_then(|m| m.friendly_name.clone())
                    };

                    // Send to all swarm members except sender
                    let swarm_session_ids: Vec<String> = {
                        let swarms = swarms_by_cwd.read().await;
                        swarms
                            .get(&cwd)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default()
                    };

                    let members = swarm_members.read().await;
                    let sessions = sessions.read().await;
                    for sid in &swarm_session_ids {
                        if sid != &from_session {
                            if let Some(member) = members.get(sid) {
                                let notification_msg = format!(
                                    "Message from {}: {}",
                                    friendly_name
                                        .as_deref()
                                        .unwrap_or(&from_session[..8.min(from_session.len())]),
                                    message
                                );
                                let _ = member.event_tx.send(ServerEvent::Notification {
                                    from_session: from_session.clone(),
                                    from_name: friendly_name.clone(),
                                    notification_type: NotificationType::Message,
                                    message: notification_msg.clone(),
                                });

                                // Also push to the agent's pending alerts
                                if let Some(agent) = sessions.get(sid) {
                                    if let Ok(mut agent) = agent.try_lock() {
                                        agent.push_alert(notification_msg);
                                    }
                                }
                            }
                        }
                    }
                }
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommList {
                id,
                session_id: req_session_id,
            } => {
                // Find the swarm (cwd) for this session
                let cwd = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.working_dir.clone())
                };

                let member_list = if let Some(cwd) = cwd {
                    let swarm_session_ids: Vec<String> = {
                        let swarms = swarms_by_cwd.read().await;
                        swarms
                            .get(&cwd)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default()
                    };

                    let members = swarm_members.read().await;
                    let touches = file_touches.read().await;

                    swarm_session_ids
                        .iter()
                        .filter_map(|sid| {
                            members.get(sid).map(|m| {
                                // Get files this member has touched
                                let files: Vec<String> = touches
                                    .iter()
                                    .filter_map(|(path, accesses)| {
                                        if accesses.iter().any(|a| &a.session_id == sid) {
                                            Some(path.display().to_string())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();

                                AgentInfo {
                                    session_id: sid.clone(),
                                    friendly_name: m.friendly_name.clone(),
                                    files_touched: files,
                                }
                            })
                        })
                        .collect()
                } else {
                    vec![]
                };

                let _ = client_event_tx.send(ServerEvent::CommMembers {
                    id,
                    members: member_list,
                });
            }

            // These are handled via channels, not direct requests from TUI
            Request::ClientDebugCommand { id, .. } | Request::ClientDebugResponse { id, .. } => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: "ClientDebugCommand/Response are for internal use only".to_string(),
                });
            }
        }
    }

    // Clean up: remove this client's session from the map
    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.remove(&client_session_id);
    }

    // Clean up: remove from swarm tracking
    {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.remove(&client_session_id) {
            // Remove from swarm by cwd
            if let Some(ref cwd) = member.working_dir {
                let mut swarms = swarms_by_cwd.write().await;
                if let Some(swarm) = swarms.get_mut(cwd) {
                    swarm.remove(&client_session_id);
                    // Remove empty swarms
                    if swarm.is_empty() {
                        swarms.remove(cwd);
                    }
                }
            }
        }
    }

    // Clean up: remove client debug channel
    {
        let mut tx_guard = client_debug_tx.write().await;
        *tx_guard = None;
    }

    if let Some(handle) = processing_task.take() {
        handle.abort();
    }

    event_handle.abort();
    Ok(())
}

/// Process a message and stream events (broadcast channel - deprecated)
#[allow(dead_code)]
async fn process_message_streaming(
    agent: Arc<Mutex<Agent>>,
    content: &str,
    event_tx: broadcast::Sender<ServerEvent>,
) -> Result<()> {
    let mut agent = agent.lock().await;
    agent.run_once_streaming(content, event_tx).await
}

/// Process a message and stream events (mpsc channel - per-client)
async fn process_message_streaming_mpsc(
    agent: Arc<Mutex<Agent>>,
    content: &str,
    event_tx: tokio::sync::mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    let mut agent = agent.lock().await;
    agent.run_once_streaming_mpsc(content, event_tx).await
}

async fn resolve_debug_session(
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    session_id: &Arc<RwLock<String>>,
    requested: Option<String>,
) -> Result<(String, Arc<Mutex<Agent>>)> {
    let mut target = requested;
    if target.is_none() {
        let current = session_id.read().await.clone();
        if !current.is_empty() {
            target = Some(current);
        }
    }

    let sessions_guard = sessions.read().await;
    if let Some(id) = target {
        let agent = sessions_guard
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown session_id '{}'", id))?;
        return Ok((id, agent));
    }

    if sessions_guard.len() == 1 {
        let (id, agent) = sessions_guard.iter().next().unwrap();
        return Ok((id.clone(), Arc::clone(agent)));
    }

    Err(anyhow::anyhow!(
        "No active session found. Connect a client or provide session_id."
    ))
}

/// Create a headless session (no TUI client needed)
async fn create_headless_session(
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    global_session_id: &Arc<RwLock<String>>,
    provider_template: &Arc<dyn Provider>,
    command: &str,
) -> Result<String> {
    // Parse optional working directory from command: create_session:/path/to/dir
    let working_dir = if let Some(path_str) = command.strip_prefix("create_session:") {
        let path_str = path_str.trim();
        if !path_str.is_empty() {
            Some(std::path::PathBuf::from(path_str))
        } else {
            None
        }
    } else {
        None
    };

    // Fork the provider for this session
    let provider = provider_template.fork();
    let registry = Registry::new(provider.clone()).await;

    // Create a new agent
    let mut new_agent = Agent::new(Arc::clone(&provider), registry);
    let client_session_id = new_agent.session_id().to_string();

    // Enable self-dev mode if in self-dev environment or working in jcode repo
    if is_selfdev_env() {
        new_agent.set_canary("self-dev");
    } else if let Some(ref dir) = working_dir {
        if crate::build::is_jcode_repo(dir) || is_jcode_repo_or_parent(dir) {
            new_agent.set_canary("self-dev");
        }
    }

    // Set as current session if none exists
    {
        let mut current = global_session_id.write().await;
        if current.is_empty() {
            *current = client_session_id.clone();
        }
    }

    // Add to sessions map
    let agent = Arc::new(Mutex::new(new_agent));
    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.insert(client_session_id.clone(), agent);
    }

    Ok(serde_json::json!({
        "session_id": client_session_id,
        "working_dir": working_dir,
    })
    .to_string())
}

async fn execute_debug_command(agent: Arc<Mutex<Agent>>, command: &str) -> Result<String> {
    let trimmed = command.trim();

    if trimmed.starts_with("message:") {
        let msg = trimmed.strip_prefix("message:").unwrap_or("").trim();
        let mut agent = agent.lock().await;
        let output = agent.run_once_capture(msg).await?;
        return Ok(output);
    }

    if trimmed.starts_with("tool:") {
        let raw = trimmed.strip_prefix("tool:").unwrap_or("").trim();
        if raw.is_empty() {
            return Err(anyhow::anyhow!("tool: requires a tool name"));
        }
        let mut parts = raw.splitn(2, |c: char| c.is_whitespace());
        let name = parts.next().unwrap_or("").trim();
        let input_raw = parts.next().unwrap_or("").trim();
        let input = if input_raw.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str::<serde_json::Value>(input_raw)?
        };
        let agent = agent.lock().await;
        let output = agent.execute_tool(name, input).await?;
        let payload = serde_json::json!({
            "output": output.output,
            "title": output.title,
            "metadata": output.metadata,
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "history" {
        let agent = agent.lock().await;
        let history = agent.get_history();
        return Ok(serde_json::to_string_pretty(&history).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "tools" {
        let agent = agent.lock().await;
        let tools = agent.tool_names().await;
        return Ok(serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "last_response" {
        let agent = agent.lock().await;
        return Ok(agent
            .last_assistant_text()
            .unwrap_or_else(|| "last_response: none".to_string()));
    }

    if trimmed == "state" {
        let agent = agent.lock().await;
        let payload = serde_json::json!({
            "session_id": agent.session_id(),
            "messages": agent.message_count(),
            "is_canary": agent.is_canary(),
            "provider": agent.provider_name(),
            "model": agent.provider_model(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "help" {
        return Ok(
            "debug commands: state, history, tools, last_response, message:<text>, tool:<name> <json>, sessions, create_session, create_session:<path>, help".to_string()
        );
    }

    Err(anyhow::anyhow!("Unknown debug command '{}'", trimmed))
}

/// Execute a client debug command (visual debug, TUI state, etc.)
/// These commands access the TUI's visual debug module which uses global state.
fn execute_client_debug_command(command: &str) -> String {
    use crate::tui::visual_debug;

    let trimmed = command.trim();

    // Visual debug commands
    if trimmed == "frame" || trimmed == "screen-json" {
        visual_debug::enable(); // Ensure enabled
        return visual_debug::latest_frame_json().unwrap_or_else(|| {
            "No frames captured yet. Try again after some UI activity.".to_string()
        });
    }

    if trimmed == "frame-normalized" || trimmed == "screen-json-normalized" {
        visual_debug::enable();
        return visual_debug::latest_frame_json_normalized()
            .unwrap_or_else(|| "No frames captured yet.".to_string());
    }

    if trimmed == "screen" {
        visual_debug::enable();
        match visual_debug::dump_to_file() {
            Ok(path) => return format!("Frames written to: {}", path.display()),
            Err(e) => return format!("Error dumping frames: {}", e),
        }
    }

    if trimmed == "enable" || trimmed == "debug-enable" {
        visual_debug::enable();
        return "Visual debugging enabled.".to_string();
    }

    if trimmed == "disable" || trimmed == "debug-disable" {
        visual_debug::disable();
        return "Visual debugging disabled.".to_string();
    }

    if trimmed == "status" {
        let enabled = visual_debug::is_enabled();
        return serde_json::json!({
            "visual_debug_enabled": enabled,
        })
        .to_string();
    }

    if trimmed == "help" {
        return r#"Client debug commands:
  frame / screen-json      - Get latest visual debug frame (JSON)
  frame-normalized         - Get normalized frame (for diffs)
  screen                   - Dump visual debug frames to file
  enable                   - Enable visual debug capture
  disable                  - Disable visual debug capture
  status                   - Get client debug status
  help                     - Show this help

Note: Visual debug captures TUI rendering state for debugging UI issues.
Frames are captured automatically when visual debug is enabled."#
            .to_string();
    }

    format!(
        "Unknown client command: {}. Use client:help for available commands.",
        trimmed
    )
}

/// Parse namespaced debug command (e.g., "server:state", "client:frame", "tester:list")
fn parse_namespaced_command(command: &str) -> (&str, &str) {
    let trimmed = command.trim();
    if let Some(idx) = trimmed.find(':') {
        let namespace = &trimmed[..idx];
        let rest = &trimmed[idx + 1..];
        // Only recognize known namespaces
        match namespace {
            "server" | "client" | "tester" => (namespace, rest),
            _ => ("server", trimmed), // Default to server namespace
        }
    } else {
        ("server", trimmed) // No namespace = server
    }
}

/// Handle debug socket connections (introspection + optional debug control)
async fn handle_debug_client(
    stream: UnixStream,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    is_processing: Arc<RwLock<bool>>,
    session_id: Arc<RwLock<String>>,
    provider: Arc<dyn Provider>,
    client_debug_tx: Arc<RwLock<Option<mpsc::UnboundedSender<(u64, String)>>>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
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

            Request::DebugCommand {
                id,
                command,
                session_id: requested_session,
            } => {
                if !debug_control_allowed() {
                    let event = ServerEvent::Error {
                        id,
                        message: "Debug control is disabled. Set JCODE_DEBUG_CONTROL=1 or run in self-dev mode.".to_string(),
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
                        let tx_guard = client_debug_tx.read().await;
                        if let Some(ref tx) = *tx_guard {
                            // Subscribe to response channel before sending
                            let mut response_rx = client_debug_response_tx.subscribe();

                            // Send command to TUI
                            if tx.send((id, cmd.to_string())).is_ok() {
                                // Wait for response with timeout
                                let timeout = tokio::time::Duration::from_secs(30);
                                match tokio::time::timeout(timeout, async {
                                    loop {
                                        if let Ok((resp_id, output)) = response_rx.recv().await {
                                            if resp_id == id {
                                                return Ok(output);
                                            }
                                        }
                                    }
                                })
                                .await
                                {
                                    Ok(result) => result,
                                    Err(_) => {
                                        Err(anyhow::anyhow!("Timeout waiting for client response"))
                                    }
                                }
                            } else {
                                Err(anyhow::anyhow!("Failed to send command to TUI client"))
                            }
                        } else {
                            Err(anyhow::anyhow!("No TUI client connected"))
                        }
                    }
                    "tester" => {
                        // Handle tester commands
                        execute_tester_command(cmd).await
                    }
                    _ => {
                        // Server commands (default)
                        if cmd == "create_session" || cmd.starts_with("create_session:") {
                            create_headless_session(&sessions, &session_id, &provider, cmd).await
                        } else if cmd == "sessions" {
                            let sessions_guard = sessions.read().await;
                            let session_list: Vec<_> = sessions_guard.keys().collect();
                            Ok(serde_json::to_string_pretty(&session_list)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "help" {
                            Ok(debug_help_text())
                        } else {
                            match resolve_debug_session(&sessions, &session_id, requested_session)
                                .await
                            {
                                Ok((_session, agent)) => execute_debug_command(agent, cmd).await,
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
                };
                let json = encode_event(&event);
                writer.write_all(json.as_bytes()).await?;
            }
        }
    }

    Ok(())
}

/// Generate help text for debug commands
fn debug_help_text() -> String {
    r#"Debug socket commands (namespaced):

SERVER COMMANDS (server: prefix or no prefix):
  state                    - Get agent state
  history                  - Get conversation history
  tools                    - List available tools
  last_response            - Get last assistant response
  message:<text>           - Send message to agent
  tool:<name> <json>       - Execute tool directly
  sessions                 - List all sessions
  create_session           - Create headless session
  create_session:<path>    - Create session with working dir

CLIENT COMMANDS (client: prefix):
  client:state             - Get TUI state
  client:frame             - Get latest visual debug frame (JSON)
  client:frame-normalized  - Get normalized frame (for diffs)
  client:screen            - Dump visual debug to file
  client:input             - Get current input buffer
  client:set_input:<text>  - Set input buffer
  client:keys:<keyspec>    - Inject key events
  client:message:<text>    - Inject and submit message
  client:scroll:<dir>      - Scroll (up/down/top/bottom)
  client:wait              - Check if processing
  client:history           - Get display messages
  client:help              - Client command help

TESTER COMMANDS (tester: prefix):
  tester:spawn             - Spawn new tester instance
  tester:list              - List active testers
  tester:<id>:frame        - Get frame from tester
  tester:<id>:message:<t>  - Send message to tester
  tester:<id>:state        - Get tester state
  tester:<id>:stop         - Stop tester

Examples:
  {"type":"debug_command","id":1,"command":"state"}
  {"type":"debug_command","id":2,"command":"client:frame"}
  {"type":"debug_command","id":3,"command":"tester:list"}"#
        .to_string()
}

/// Execute tester commands
async fn execute_tester_command(command: &str) -> Result<String> {
    let trimmed = command.trim();

    if trimmed == "list" {
        // List active testers from manifest
        let testers = load_testers()?;
        if testers.is_empty() {
            return Ok("No active testers.".to_string());
        }
        return Ok(serde_json::to_string_pretty(&testers)?);
    }

    if trimmed == "spawn" || trimmed.starts_with("spawn ") {
        // Parse spawn options
        let opts: serde_json::Value = if trimmed == "spawn" {
            serde_json::json!({})
        } else {
            serde_json::from_str(trimmed.strip_prefix("spawn ").unwrap_or("{}"))?
        };
        return spawn_tester(opts).await;
    }

    // Check for tester:<id>:<command> format
    if let Some(rest) = trimmed.strip_prefix("") {
        // Parse <id>:<command> or <id>:<command>:<arg>
        let parts: Vec<&str> = rest.splitn(3, ':').collect();
        if parts.len() >= 2 {
            let tester_id = parts[0];
            let cmd = parts[1];
            let arg = parts.get(2).map(|s| *s);
            return execute_tester_subcommand(tester_id, cmd, arg).await;
        }
    }

    Err(anyhow::anyhow!(
        "Unknown tester command: {}. Use tester:help for usage.",
        trimmed
    ))
}

/// Load testers from manifest file
fn load_testers() -> Result<Vec<serde_json::Value>> {
    let path = crate::storage::jcode_dir()?.join("testers.json");
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&content)?)
    } else {
        Ok(vec![])
    }
}

/// Save testers to manifest file
fn save_testers(testers: &[serde_json::Value]) -> Result<()> {
    let path = crate::storage::jcode_dir()?.join("testers.json");
    std::fs::write(&path, serde_json::to_string_pretty(testers)?)?;
    Ok(())
}

/// Spawn a new tester instance
async fn spawn_tester(opts: serde_json::Value) -> Result<String> {
    use std::process::Stdio;

    let id = format!("tester_{}", crate::id::new_id("tui"));
    let cwd = opts.get("cwd").and_then(|v| v.as_str()).unwrap_or(".");
    let binary = opts.get("binary").and_then(|v| v.as_str());

    // Find binary to use
    let binary_path = if let Some(b) = binary {
        PathBuf::from(b)
    } else if let Ok(canary) = crate::build::canary_binary_path() {
        if canary.exists() {
            canary
        } else {
            std::env::current_exe()?
        }
    } else {
        std::env::current_exe()?
    };

    if !binary_path.exists() {
        return Err(anyhow::anyhow!(
            "Binary not found: {}",
            binary_path.display()
        ));
    }

    // Set up debug file paths for this tester
    let debug_cmd = std::env::temp_dir().join(format!("jcode_debug_cmd_{}", id));
    let debug_resp = std::env::temp_dir().join(format!("jcode_debug_response_{}", id));
    let stdout_path = std::env::temp_dir().join(format!("jcode_tester_stdout_{}", id));
    let stderr_path = std::env::temp_dir().join(format!("jcode_tester_stderr_{}", id));

    let stdout_file = std::fs::File::create(&stdout_path)?;
    let stderr_file = std::fs::File::create(&stderr_path)?;

    let mut cmd = tokio::process::Command::new(&binary_path);
    cmd.current_dir(cwd);
    cmd.env("JCODE_SELFDEV_MODE", "1");
    cmd.env(
        "JCODE_DEBUG_CMD_PATH",
        debug_cmd.to_string_lossy().to_string(),
    );
    cmd.env(
        "JCODE_DEBUG_RESPONSE_PATH",
        debug_resp.to_string_lossy().to_string(),
    );
    cmd.arg("--debug-socket");
    cmd.stdout(Stdio::from(stdout_file));
    cmd.stderr(Stdio::from(stderr_file));

    let child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);

    // Save tester info
    let info = serde_json::json!({
        "id": id,
        "pid": pid,
        "binary": binary_path.to_string_lossy(),
        "cwd": cwd,
        "debug_cmd_path": debug_cmd.to_string_lossy(),
        "debug_response_path": debug_resp.to_string_lossy(),
        "stdout_path": stdout_path.to_string_lossy(),
        "stderr_path": stderr_path.to_string_lossy(),
        "started_at": chrono::Utc::now().to_rfc3339(),
    });

    let mut testers = load_testers()?;
    testers.push(info);
    save_testers(&testers)?;

    Ok(serde_json::json!({
        "id": id,
        "pid": pid,
        "message": format!("Spawned tester {} (pid {})", id, pid)
    })
    .to_string())
}

/// Execute a command on a specific tester
async fn execute_tester_subcommand(
    tester_id: &str,
    cmd: &str,
    arg: Option<&str>,
) -> Result<String> {
    let testers = load_testers()?;
    let tester = testers
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(tester_id))
        .ok_or_else(|| anyhow::anyhow!("Tester not found: {}", tester_id))?;

    let debug_cmd_path = tester
        .get("debug_cmd_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid tester config"))?;
    let debug_resp_path = tester
        .get("debug_response_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid tester config"))?;

    // Map commands to the TUI file protocol
    let file_cmd = match cmd {
        "frame" => "screen-json".to_string(),
        "frame-normalized" => "screen-json-normalized".to_string(),
        "state" => "state".to_string(),
        "history" => "history".to_string(),
        "wait" => "wait".to_string(),
        "input" => "input".to_string(),
        "message" => format!("message:{}", arg.unwrap_or("")),
        "keys" => format!("keys:{}", arg.unwrap_or("")),
        "set_input" => format!("set_input:{}", arg.unwrap_or("")),
        "scroll" => format!("scroll:{}", arg.unwrap_or("down")),
        "stop" => {
            // Kill the tester
            if let Some(pid) = tester.get("pid").and_then(|v| v.as_u64()) {
                let _ = std::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .output();
            }
            // Remove from testers list
            let mut testers = load_testers()?;
            testers.retain(|t| t.get("id").and_then(|v| v.as_str()) != Some(tester_id));
            save_testers(&testers)?;
            return Ok("Stopped tester.".to_string());
        }
        _ => return Err(anyhow::anyhow!("Unknown tester command: {}", cmd)),
    };

    // Write command to tester's debug file
    std::fs::write(debug_cmd_path, &file_cmd)?;

    // Wait for response with timeout
    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            return Err(anyhow::anyhow!("Timeout waiting for tester response"));
        }
        if let Ok(response) = std::fs::read_to_string(debug_resp_path) {
            if !response.is_empty() {
                // Clear response file
                let _ = std::fs::remove_file(debug_resp_path);
                return Ok(response);
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Client for connecting to a running server
pub struct Client {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
    next_id: u64,
}

impl Client {
    pub async fn connect() -> Result<Self> {
        Self::connect_with_path(socket_path()).await
    }

    pub async fn connect_with_path(path: PathBuf) -> Result<Self> {
        let stream = UnixStream::connect(&path).await?;
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
        let stream = UnixStream::connect(&path).await?;
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
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    /// Subscribe to events
    pub async fn subscribe(&mut self) -> Result<u64> {
        self.subscribe_with_info(None, None).await
    }

    pub async fn subscribe_with_info(
        &mut self,
        working_dir: Option<String>,
        selfdev: Option<bool>,
    ) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Subscribe {
            id,
            working_dir,
            selfdev,
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    /// Read the next event from the server
    pub async fn read_event(&mut self) -> Result<ServerEvent> {
        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
        let event: ServerEvent = serde_json::from_str(&line)?;
        Ok(event)
    }

    pub async fn ping(&mut self) -> Result<bool> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Ping { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;

        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
        let event: ServerEvent = serde_json::from_str(&line)?;

        match event {
            ServerEvent::Pong { .. } => Ok(true),
            _ => Ok(false),
        }
    }

    pub async fn get_state(&mut self) -> Result<ServerEvent> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::GetState { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;

        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
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

    pub async fn get_history_event(&mut self) -> Result<ServerEvent> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::GetHistory { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        for _ in 0..10 {
            let mut line = String::new();
            self.reader.read_line(&mut line).await?;
            let event: ServerEvent = serde_json::from_str(&line)?;
            match event {
                ServerEvent::Ack { .. } => continue,
                _ => return Ok(event),
            }
        }

        Ok(ServerEvent::Error {
            id,
            message: "History response not received".to_string(),
        })
    }

    pub async fn resume_session(&mut self, session_id: &str) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::ResumeSession {
            id,
            session_id: session_id.to_string(),
        };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }

    pub async fn reload(&mut self) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Reload { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(())
    }

    pub async fn cycle_model(&mut self, direction: i8) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::CycleModel { id, direction };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(id)
    }
}

/// Get the jcode repository directory
fn get_repo_dir() -> Option<PathBuf> {
    // Try CARGO_MANIFEST_DIR first (works when running from source)
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = PathBuf::from(manifest_dir);
    if path.join(".git").exists() {
        return Some(path);
    }

    // Fallback: check relative to executable
    if let Ok(exe) = std::env::current_exe() {
        // Assume structure: repo/target/release/jcode
        if let Some(repo) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            if repo.join(".git").exists() {
                return Some(repo.to_path_buf());
            }
        }
    }

    None
}

/// Server hot-reload: pull, build, and exec into new binary
#[allow(dead_code)]
fn do_server_reload() -> Result<()> {
    use std::os::unix::process::CommandExt;

    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    crate::logging::info("Server hot-reload starting...");

    // Pull latest changes
    crate::logging::info("Pulling latest changes...");
    let pull = ProcessCommand::new("git")
        .args(["pull", "-q"])
        .current_dir(&repo_dir)
        .status()?;

    if !pull.success() {
        crate::logging::info("Warning: git pull failed, continuing with current code");
    }

    // Build release
    crate::logging::info("Building...");
    let build = ProcessCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .status()?;

    if !build.success() {
        anyhow::bail!("Build failed");
    }

    if let Err(e) = build::install_local_release(&repo_dir) {
        crate::logging::info(&format!("Warning: install failed: {}", e));
    }

    crate::logging::info("✓ Build complete, restarting server...");

    // Find the new executable
    let exe = repo_dir.join("target/release/jcode");
    if !exe.exists() {
        anyhow::bail!("Built executable not found at {:?}", exe);
    }

    // Exec into new binary with serve command
    let err = ProcessCommand::new(&exe).arg("serve").exec();

    // exec() only returns on error
    Err(anyhow::anyhow!("Failed to exec: {}", err))
}

/// Server hot-reload with progress streaming to client
/// This just restarts with the existing binary - no rebuild
async fn do_server_reload_with_progress(
    tx: tokio::sync::mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let send_progress =
        |step: &str, message: &str, success: Option<bool>, output: Option<String>| {
            let _ = tx.send(ServerEvent::ReloadProgress {
                step: step.to_string(),
                message: message.to_string(),
                success,
                output,
            });
        };

    // Step 1: Find repo
    send_progress("init", "🔄 Starting hot-reload...", None, None);

    let repo_dir =
        get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

    send_progress(
        "init",
        &format!("📁 Repository: {}", repo_dir.display()),
        Some(true),
        None,
    );

    // Step 2: Check for binary
    let (exe, exe_label) = server_update_candidate().ok_or_else(|| {
        anyhow::anyhow!("No reloadable binary found (canary/stable or target/release)")
    })?;
    if !exe.exists() {
        send_progress("verify", "❌ No reloadable binary found", Some(false), None);
        send_progress(
            "verify",
            "💡 Run 'cargo build --release' first, then use 'selfdev reload'",
            Some(false),
            None,
        );
        anyhow::bail!("No binary found. Build first with 'cargo build --release'");
    }

    // Step 3: Get binary info
    let metadata = std::fs::metadata(&exe)?;
    let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
    let modified = metadata.modified().ok();

    let age_str = if let Some(mod_time) = modified {
        if let Ok(elapsed) = mod_time.elapsed() {
            let secs = elapsed.as_secs();
            if secs < 60 {
                format!("{} seconds ago", secs)
            } else if secs < 3600 {
                format!("{} minutes ago", secs / 60)
            } else if secs < 86400 {
                format!("{} hours ago", secs / 3600)
            } else {
                format!("{} days ago", secs / 86400)
            }
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };

    send_progress(
        "verify",
        &format!(
            "✓ Binary ({}): {:.1} MB, built {}",
            exe_label, size_mb, age_str
        ),
        Some(true),
        None,
    );

    // Step 4: Show current git state (informational)
    let head_output = ProcessCommand::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(&repo_dir)
        .output();

    if let Ok(output) = head_output {
        let head_str = String::from_utf8_lossy(&output.stdout);
        send_progress(
            "git",
            &format!("📍 HEAD: {}", head_str.trim()),
            Some(true),
            None,
        );
    }

    // Step 5: Exec
    send_progress(
        "exec",
        "🚀 Restarting server with existing binary...",
        None,
        None,
    );

    // Small delay to ensure the progress message is sent
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    crate::logging::info(&format!("Exec'ing into binary: {:?}", exe));

    // Exec into new binary with serve command
    let err = ProcessCommand::new(&exe).arg("serve").exec();

    // exec() only returns on error
    Err(anyhow::anyhow!("Failed to exec: {}", err))
}

/// Monitor for selfdev signal files and exit with appropriate codes
/// This allows the canary wrapper to handle reload/rollback requests
async fn monitor_selfdev_signals() {
    use tokio::time::{interval, Duration};

    let mut check_interval = interval(Duration::from_millis(500));

    loop {
        check_interval.tick().await;

        let jcode_dir = match crate::storage::jcode_dir() {
            Ok(dir) => dir,
            Err(_) => continue,
        };

        // Check for rebuild signal (reload with new canary)
        let rebuild_path = jcode_dir.join("rebuild-signal");
        if rebuild_path.exists() {
            if let Ok(_hash) = std::fs::read_to_string(&rebuild_path) {
                let _ = std::fs::remove_file(&rebuild_path);
                crate::logging::info("Server: reload signal received, exiting with code 42");
                std::process::exit(42);
            }
        }

        // Check for rollback signal (switch to stable)
        let rollback_path = jcode_dir.join("rollback-signal");
        if rollback_path.exists() {
            if let Ok(_hash) = std::fs::read_to_string(&rollback_path) {
                let _ = std::fs::remove_file(&rollback_path);
                crate::logging::info("Server: rollback signal received, exiting with code 43");
                std::process::exit(43);
            }
        }
    }
}
