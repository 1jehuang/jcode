#![allow(dead_code)]
#![allow(dead_code)]

use crate::agent::Agent;
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

/// Default socket path for main communication
pub fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    runtime_dir.join("jcode.sock")
}

/// Debug socket path for testing/introspection
pub fn debug_socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    runtime_dir.join("jcode-debug.sock")
}

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

        // Create default session
        let agent = Agent::new(Arc::clone(&self.provider), self.registry.clone());
        let session_id = agent.session_id().to_string();
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), Arc::new(Mutex::new(agent)));
            *self.session_id.write().await = session_id.clone();
        }

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
    _global_session_id: Arc<RwLock<String>>,
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

    // Create a new session for this client
    let new_agent = Agent::new(Arc::clone(&provider), registry.clone());
    let client_session_id = new_agent.session_id().to_string();
    let friendly_name = new_agent.session_short_name().map(|s| s.to_string());
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
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // Client disconnected
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

                // Process message with streaming to this client's channel
                let result = process_message_streaming_mpsc(
                    Arc::clone(&agent),
                    &content,
                    client_event_tx.clone(),
                )
                .await;

                // Clear processing flag
                client_is_processing = false;

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

            Request::Cancel { id } => {
                // TODO: Implement cancellation
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::Clear { id } => {
                // Clear this client's session (create new agent)
                let new_agent = Agent::new(Arc::clone(&provider), registry.clone());
                let new_id = new_agent.session_id().to_string();

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

            Request::Subscribe { id } => {
                // Send this client's session ID
                let json = encode_event(&ServerEvent::SessionId {
                    session_id: client_session_id.clone(),
                });
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::GetHistory { id } => {
                let messages = {
                    let agent_guard = agent.lock().await;
                    agent_guard.get_history()
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
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
            }

            Request::Reload { id } => {
                // Notify this client that server is reloading
                let _ = client_event_tx.send(ServerEvent::Reloading { new_socket: None });
                let _ = client_event_tx.send(ServerEvent::Done { id });

                // Spawn reload process (will exec into new binary)
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    if let Err(e) = do_server_reload() {
                        eprintln!("Reload failed: {}", e);
                    }
                });
            }

            Request::ResumeSession { id, session_id } => {
                // Load the specified session into this client's agent
                let result = {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.restore_session(&session_id)
                };

                match result {
                    Ok(()) => {
                        // Send updated history to client
                        let messages = {
                            let agent_guard = agent.lock().await;
                            agent_guard.get_history()
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
                        members.get(&req_session_id).and_then(|m| m.friendly_name.clone())
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
                                .map(|c| vec![ContextEntry {
                                    key: c.key.clone(),
                                    value: c.value.clone(),
                                    from_session: c.from_session.clone(),
                                    from_name: c.from_name.clone(),
                                }])
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
                    members.get(&from_session).and_then(|m| m.working_dir.clone())
                };

                if let Some(cwd) = cwd {
                    let friendly_name = {
                        let members = swarm_members.read().await;
                        members.get(&from_session).and_then(|m| m.friendly_name.clone())
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
                                    friendly_name.as_deref().unwrap_or(&from_session[..8.min(from_session.len())]),
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

/// Handle debug socket connections (read-only introspection)
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

            _ => {
                // Debug socket only allows read-only operations
                let event = ServerEvent::Error {
                    id: request.id(),
                    message: "Debug socket only allows ping and state queries".to_string(),
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
        let path = socket_path();
        let stream = UnixStream::connect(&path).await?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        })
    }

    pub async fn connect_debug() -> Result<Self> {
        let path = debug_socket_path();
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
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::Subscribe { id };
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
        let id = self.next_id;
        self.next_id += 1;

        let request = Request::GetHistory { id };
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;

        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
        let event: ServerEvent = serde_json::from_str(&line)?;

        match event {
            ServerEvent::History { messages, .. } => Ok(messages),
            _ => Ok(Vec::new()),
        }
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
