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

async fn reset_provider_sessions(
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
) {
    let sessions_guard = sessions.read().await;
    for agent in sessions_guard.values() {
        let mut agent_guard = agent.lock().await;
        agent_guard.reset_provider_session();
    }
}

fn debug_control_allowed() -> bool {
    if is_selfdev_env() {
        return true;
    }
    std::env::var("JCODE_DEBUG_CONTROL")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn server_has_newer_binary() -> bool {
    let startup_mtime = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(&p).ok())
        .and_then(|m| m.modified().ok());
    let Some(startup_mtime) = startup_mtime else {
        return false;
    };

    let Some(repo_dir) = crate::build::get_repo_dir() else {
        return false;
    };

    let exe = repo_dir.join("target/release/jcode");
    if let Ok(metadata) = std::fs::metadata(&exe) {
        if let Ok(current_mtime) = metadata.modified() {
            return current_mtime > startup_mtime;
        }
    }

    false
}

/// Exit code when server shuts down due to idle timeout
pub const EXIT_IDLE_TIMEOUT: i32 = 44;

/// Server state
pub struct Server {
    provider: Arc<dyn Provider>,
    registry: Registry,
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
}

impl Server {
    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            provider,
            registry,
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
        }
    }

    pub fn new_with_paths(
        provider: Arc<dyn Provider>,
        registry: Registry,
        socket_path: PathBuf,
        debug_socket_path: PathBuf,
    ) -> Self {
        let mut server = Self::new(provider, registry);
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
                    eprintln!("Bus monitor lagged by {} events", n);
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

        eprintln!("Server listening on {:?}", self.socket_path);
        eprintln!("Debug socket on {:?}", self.debug_socket_path);

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
                        eprintln!(
                            "No clients connected. Server will exit after {} minutes of idle.",
                            IDLE_TIMEOUT_SECS / 60
                        );
                    }

                    if let Some(since) = idle_since {
                        let idle_duration = since.elapsed().as_secs();
                        if idle_duration >= IDLE_TIMEOUT_SECS {
                            eprintln!(
                                "Server idle for {} minutes with no clients. Shutting down.",
                                idle_duration / 60
                            );
                            std::process::exit(EXIT_IDLE_TIMEOUT);
                        }
                    }
                } else {
                    // Clients connected - reset idle timer
                    if idle_since.is_some() {
                        eprintln!("Client connected. Idle timer cancelled.");
                    }
                    idle_since = None;
                }
            }
        });

        // Spawn main socket handler
        let main_sessions = Arc::clone(&self.sessions);
        let main_event_tx = self.event_tx.clone();
        let main_provider = Arc::clone(&self.provider);
        let main_registry = self.registry.clone();
        let main_is_processing = Arc::clone(&self.is_processing);
        let main_session_id = Arc::clone(&self.session_id);
        let main_client_count = Arc::clone(&self.client_count);
        let main_swarm_members = Arc::clone(&self.swarm_members);
        let main_swarms_by_cwd = Arc::clone(&self.swarms_by_cwd);
        let main_shared_context = Arc::clone(&self.shared_context);
        let main_file_touches = Arc::clone(&self.file_touches);

        let main_handle = tokio::spawn(async move {
            loop {
                match main_listener.accept().await {
                    Ok((stream, _)) => {
                        let sessions = Arc::clone(&main_sessions);
                        let event_tx = main_event_tx.clone();
                        let provider = Arc::clone(&main_provider);
                        let registry = main_registry.clone();
                        let is_processing = Arc::clone(&main_is_processing);
                        let session_id = Arc::clone(&main_session_id);
                        let client_count = Arc::clone(&main_client_count);
                        let swarm_members = Arc::clone(&main_swarm_members);
                        let swarms_by_cwd = Arc::clone(&main_swarms_by_cwd);
                        let shared_context = Arc::clone(&main_shared_context);
                        let file_touches = Arc::clone(&main_file_touches);

                        // Increment client count
                        *client_count.write().await += 1;

                        tokio::spawn(async move {
                            let result = handle_client(
                                stream,
                                sessions,
                                event_tx,
                                provider,
                                registry,
                                is_processing,
                                session_id,
                                Arc::clone(&client_count),
                                swarm_members,
                                swarms_by_cwd,
                                shared_context,
                                file_touches,
                            )
                            .await;

                            // Decrement client count when done
                            *client_count.write().await -= 1;

                            if let Err(e) = result {
                                eprintln!("Client error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Main accept error: {}", e);
                    }
                }
            }
        });

        // Spawn debug socket handler
        let debug_sessions = Arc::clone(&self.sessions);
        let debug_is_processing = Arc::clone(&self.is_processing);
        let debug_session_id = Arc::clone(&self.session_id);

        let debug_handle = tokio::spawn(async move {
            loop {
                match debug_listener.accept().await {
                    Ok((stream, _)) => {
                        let sessions = Arc::clone(&debug_sessions);
                        let is_processing = Arc::clone(&debug_is_processing);
                        let session_id = Arc::clone(&debug_session_id);

                        tokio::spawn(async move {
                            if let Err(e) =
                                handle_debug_client(stream, sessions, is_processing, session_id)
                                    .await
                            {
                                eprintln!("Debug client error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Debug accept error: {}", e);
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
    provider: Arc<dyn Provider>,
    registry: Registry,
    _global_is_processing: Arc<RwLock<bool>>,
    global_session_id: Arc<RwLock<String>>,
    client_count: Arc<RwLock<usize>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_cwd: Arc<RwLock<HashMap<PathBuf, HashSet<String>>>>,
    shared_context: Arc<RwLock<HashMap<PathBuf, HashMap<String, SharedContext>>>>,
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
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

    loop {
        line.clear();
        tokio::select! {
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
                let (messages, is_canary) = {
                    let agent_guard = agent.lock().await;
                    (agent_guard.get_history(), agent_guard.is_canary())
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
                    provider_name: Some(provider.name().to_string()),
                    provider_model: Some(provider.model().to_string()),
                    available_models: provider
                        .available_models()
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
                        eprintln!("Reload failed: {}", e);
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
                    if is_selfdev_env() {
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
                        let (messages, is_canary) = {
                            let agent_guard = agent.lock().await;
                            (agent_guard.get_history(), agent_guard.is_canary())
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
                            provider_name: Some(provider.name().to_string()),
                            provider_model: Some(provider.model().to_string()),
                            available_models: provider
                                .available_models()
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
                let models = provider.available_models();
                if models.is_empty() {
                    let _ = client_event_tx.send(ServerEvent::ModelChanged {
                        id,
                        model: provider.model(),
                        error: Some(
                            "Model switching is not available for this provider.".to_string(),
                        ),
                    });
                    continue;
                }

                let current = provider.model();
                let current_index = models.iter().position(|m| *m == current).unwrap_or(0);
                let len = models.len();
                let next_index = if direction >= 0 {
                    (current_index + 1) % len
                } else {
                    (current_index + len - 1) % len
                };
                let next_model = models[next_index];

                match provider.set_model(next_model) {
                    Ok(()) => {
                        reset_provider_sessions(&sessions).await;
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: next_model.to_string(),
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
                let models = provider.available_models();
                if models.is_empty() {
                    let _ = client_event_tx.send(ServerEvent::ModelChanged {
                        id,
                        model: provider.model(),
                        error: Some(
                            "Model switching is not available for this provider.".to_string(),
                        ),
                    });
                    continue;
                }

                let current = provider.model();
                match provider.set_model(&model) {
                    Ok(()) => {
                        reset_provider_sessions(&sessions).await;
                        let updated = provider.model();
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

async fn execute_debug_command(
    agent: Arc<Mutex<Agent>>,
    command: &str,
) -> Result<String> {
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
        return Ok(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        );
    }

    if trimmed == "history" {
        let agent = agent.lock().await;
        let history = agent.get_history();
        return Ok(
            serde_json::to_string_pretty(&history).unwrap_or_else(|_| "[]".to_string())
        );
    }

    if trimmed == "tools" {
        let agent = agent.lock().await;
        let tools = agent.tool_names().await;
        return Ok(
            serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".to_string())
        );
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
        return Ok(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        );
    }

    if trimmed == "help" {
        return Ok(
            "debug commands: state, history, tools, last_response, message:<text>, tool:<name> <json>, help".to_string()
        );
    }

    Err(anyhow::anyhow!("Unknown debug command '{}'", trimmed))
}

/// Handle debug socket connections (introspection + optional debug control)
async fn handle_debug_client(
    stream: UnixStream,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    is_processing: Arc<RwLock<bool>>,
    session_id: Arc<RwLock<String>>,
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

                let result = match resolve_debug_session(&sessions, &session_id, requested_session)
                    .await
                {
                    Ok((_session, agent)) => execute_debug_command(agent, &command).await,
                    Err(e) => Err(e),
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

    eprintln!("Server hot-reload starting...");

    // Pull latest changes
    eprintln!("Pulling latest changes...");
    let pull = ProcessCommand::new("git")
        .args(["pull", "-q"])
        .current_dir(&repo_dir)
        .status()?;

    if !pull.success() {
        eprintln!("Warning: git pull failed, continuing with current code");
    }

    // Build release
    eprintln!("Building...");
    let build = ProcessCommand::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .status()?;

    if !build.success() {
        anyhow::bail!("Build failed");
    }

    if let Err(e) = build::install_local_release(&repo_dir) {
        eprintln!("Warning: install failed: {}", e);
    }

    eprintln!("✓ Build complete, restarting server...");

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
    let exe = repo_dir.join("target/release/jcode");
    if !exe.exists() {
        send_progress(
            "verify",
            "❌ No binary found at target/release/jcode",
            Some(false),
            None,
        );
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
        &format!("✓ Binary: {:.1} MB, built {}", size_mb, age_str),
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

    eprintln!("Exec'ing into binary: {:?}", exe);

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
                eprintln!("Server: reload signal received, exiting with code 42");
                std::process::exit(42);
            }
        }

        // Check for rollback signal (switch to stable)
        let rollback_path = jcode_dir.join("rollback-signal");
        if rollback_path.exists() {
            if let Ok(_hash) = std::fs::read_to_string(&rollback_path) {
                let _ = std::fs::remove_file(&rollback_path);
                eprintln!("Server: rollback signal received, exiting with code 43");
                std::process::exit(43);
            }
        }
    }
}
