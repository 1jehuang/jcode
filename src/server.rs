#![allow(dead_code)]

#![allow(dead_code)]

use crate::agent::Agent;
use crate::protocol::{decode_request, encode_event, HistoryMessage, Request, ServerEvent};
use crate::provider::Provider;
use crate::tool::Registry;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, Mutex, RwLock};

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

                        tokio::spawn(async move {
                            if let Err(e) = handle_client(
                                stream,
                                sessions,
                                event_tx,
                                provider,
                                registry,
                                is_processing,
                                session_id,
                            )
                            .await
                            {
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
    event_tx: broadcast::Sender<ServerEvent>,
    provider: Arc<dyn Provider>,
    registry: Registry,
    is_processing: Arc<RwLock<bool>>,
    session_id: Arc<RwLock<String>>,
) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));
    let mut line = String::new();

    // Subscribe to events
    let mut event_rx = event_tx.subscribe();
    let writer_clone = Arc::clone(&writer);

    // Spawn event forwarder
    let event_handle = tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            let json = encode_event(&event);
            let mut w = writer_clone.lock().await;
            if w.write_all(json.as_bytes()).await.is_err() {
                break;
            }
        }
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
                // Check if already processing
                {
                    let processing = is_processing.read().await;
                    if *processing {
                        let event = ServerEvent::Error {
                            id,
                            message: "Already processing a message".to_string(),
                        };
                        let _ = event_tx.send(event);
                        continue;
                    }
                }

                // Set processing flag
                *is_processing.write().await = true;

                // Get or create session
                let current_session_id = session_id.read().await.clone();
                let agent = {
                    let sessions = sessions.read().await;
                    sessions.get(&current_session_id).cloned()
                };

                let agent = match agent {
                    Some(a) => a,
                    None => {
                        let new_agent = Agent::new(Arc::clone(&provider), registry.clone());
                        let new_id = new_agent.session_id().to_string();
                        let agent = Arc::new(Mutex::new(new_agent));
                        {
                            let mut sessions = sessions.write().await;
                            sessions.insert(new_id.clone(), Arc::clone(&agent));
                            *session_id.write().await = new_id;
                        }
                        agent
                    }
                };

                // Process message with streaming
                let event_tx_clone = event_tx.clone();
                let is_processing_clone = Arc::clone(&is_processing);

                tokio::spawn(async move {
                    let result = process_message_streaming(agent, &content, event_tx_clone.clone()).await;

                    // Clear processing flag
                    *is_processing_clone.write().await = false;

                    match result {
                        Ok(()) => {
                            let _ = event_tx_clone.send(ServerEvent::Done { id });
                        }
                        Err(e) => {
                            let _ = event_tx_clone.send(ServerEvent::Error {
                                id,
                                message: e.to_string(),
                            });
                        }
                    }
                });
            }

            Request::Cancel { id } => {
                // TODO: Implement cancellation
                let event = ServerEvent::Done { id };
                let _ = event_tx.send(event);
            }

            Request::Clear { id } => {
                // Create new session
                let new_agent = Agent::new(Arc::clone(&provider), registry.clone());
                let new_id = new_agent.session_id().to_string();
                {
                    let mut sessions = sessions.write().await;
                    sessions.insert(new_id.clone(), Arc::new(Mutex::new(new_agent)));
                    *session_id.write().await = new_id.clone();
                }

                let event = ServerEvent::SessionId {
                    session_id: new_id,
                };
                let _ = event_tx.send(event);
                let _ = event_tx.send(ServerEvent::Done { id });
            }

            Request::Ping { id } => {
                let event = ServerEvent::Pong { id };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
            }

            Request::GetState { id } => {
                let current_session_id = session_id.read().await.clone();
                let _sessions = sessions.read().await;
                let message_count = 0; // TODO: Get actual count

                let event = ServerEvent::State {
                    id,
                    session_id: current_session_id,
                    message_count,
                    is_processing: *is_processing.read().await,
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
            }

            Request::Subscribe { id } => {
                // Already subscribed via event_rx
                let current_session_id = session_id.read().await.clone();
                let event = ServerEvent::SessionId {
                    session_id: current_session_id,
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
                let _ = event_tx.send(ServerEvent::Done { id });
            }

            Request::GetHistory { id } => {
                let current_session_id = session_id.read().await.clone();
                let agent = {
                    let sessions = sessions.read().await;
                    sessions.get(&current_session_id).cloned()
                };

                let messages = if let Some(agent) = agent {
                    let agent = agent.lock().await;
                    agent.get_history()
                } else {
                    Vec::new()
                };

                let event = ServerEvent::History {
                    id,
                    session_id: current_session_id,
                    messages,
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await?;
            }

            Request::Reload { id } => {
                // Notify all clients that server is reloading
                let _ = event_tx.send(ServerEvent::Reloading { new_socket: None });
                let _ = event_tx.send(ServerEvent::Done { id });

                // Spawn reload process (will exec into new binary)
                tokio::spawn(async move {
                    // Give clients time to receive the notification
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    if let Err(e) = do_server_reload() {
                        eprintln!("Reload failed: {}", e);
                    }
                });
            }

            // Agent-to-agent communication
            Request::AgentRegister { id, .. } => {
                // TODO: Implement agent registration
                let _ = event_tx.send(ServerEvent::Done { id });
            }

            Request::AgentTask { id, task, .. } => {
                // Treat as a regular message for now
                let current_session_id = session_id.read().await.clone();
                let agent = {
                    let sessions = sessions.read().await;
                    sessions.get(&current_session_id).cloned()
                };

                if let Some(agent) = agent {
                    let event_tx_clone = event_tx.clone();
                    tokio::spawn(async move {
                        let result = process_message_streaming(agent, &task, event_tx_clone.clone()).await;
                        match result {
                            Ok(()) => {
                                let _ = event_tx_clone.send(ServerEvent::Done { id });
                            }
                            Err(e) => {
                                let _ = event_tx_clone.send(ServerEvent::Error {
                                    id,
                                    message: e.to_string(),
                                });
                            }
                        }
                    });
                }
            }

            Request::AgentCapabilities { id } => {
                // Return basic capabilities
                let _ = event_tx.send(ServerEvent::Done { id });
            }

            Request::AgentContext { id } => {
                // TODO: Return conversation context
                let _ = event_tx.send(ServerEvent::Done { id });
            }
        }
    }

    event_handle.abort();
    Ok(())
}

/// Process a message and stream events
async fn process_message_streaming(
    agent: Arc<Mutex<Agent>>,
    content: &str,
    event_tx: broadcast::Sender<ServerEvent>,
) -> Result<()> {
    let mut agent = agent.lock().await;

    // Use streaming version that sends events to broadcast channel
    agent.run_once_streaming(content, event_tx).await
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
        if let Some(repo) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
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

    let repo_dir = get_repo_dir().ok_or_else(|| anyhow::anyhow!("Could not find jcode repository"))?;

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
    let err = ProcessCommand::new(&exe)
        .arg("serve")
        .exec();

    // exec() only returns on error
    Err(anyhow::anyhow!("Failed to exec: {}", err))
}
