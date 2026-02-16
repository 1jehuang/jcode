#![allow(dead_code)]

use crate::agent::Agent;
use crate::ambient_runner::AmbientRunnerHandle;
use crate::build;
use crate::bus::{Bus, BusEvent, FileOp};
use crate::id;
use crate::mcp::McpConfig;
use crate::plan::PlanItem;
use crate::protocol::{
    decode_request, encode_event, AgentInfo, ContextEntry, FeatureToggle, HistoryMessage,
    NotificationType, Request, ServerEvent,
};
use crate::provider::Provider;
use crate::registry::{server_debug_socket_path, server_socket_path};
use crate::session::Session;
use crate::tool::Registry;
use anyhow::Result;
use futures::future::try_join_all;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

/// Record of a file access by an agent
#[derive(Clone, Debug)]
pub struct FileAccess {
    pub session_id: String,
    pub op: FileOp,
    pub timestamp: Instant,
    pub absolute_time: std::time::SystemTime,
    pub summary: Option<String>,
}

/// Information about a session in a swarm
#[derive(Clone, Debug)]
pub struct SwarmMember {
    pub session_id: String,
    /// Channel to send events to this session
    pub event_tx: mpsc::UnboundedSender<ServerEvent>,
    /// Working directory (used to derive swarm id)
    pub working_dir: Option<PathBuf>,
    /// Swarm identifier (shared across worktrees)
    pub swarm_id: Option<String>,
    /// Whether swarm coordination is enabled for this member
    pub swarm_enabled: bool,
    /// Lifecycle status (ready, running, completed, failed, stopped, etc.)
    pub status: String,
    /// Optional detail (current task, error, etc.)
    pub detail: Option<String>,
    /// Friendly name like "fox"
    pub friendly_name: Option<String>,
    /// Role: "agent", "coordinator", "worktree_manager"
    pub role: String,
    /// When this member joined the swarm
    pub joined_at: Instant,
    /// When status was last changed
    pub last_status_change: Instant,
}

/// A versioned plan for a swarm.
#[derive(Clone, Debug)]
pub struct VersionedPlan {
    pub items: Vec<PlanItem>,
    pub version: u64,
    /// Session ids that should receive this plan's updates.
    pub participants: HashSet<String>,
}

impl VersionedPlan {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            version: 0,
            participants: HashSet::new(),
        }
    }
}

/// A shared context entry stored by the server
#[derive(Clone, Debug)]
pub struct SharedContext {
    pub key: String,
    pub value: String,
    pub from_session: String,
    pub from_name: Option<String>,
    /// When this context was created
    pub created_at: Instant,
    /// When this context was last updated
    pub updated_at: Instant,
}

/// Event types for real-time event subscription
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SwarmEventType {
    /// A file was touched (read/write/edit)
    FileTouch {
        path: String,
        op: String,
        summary: Option<String>,
    },
    /// A notification was broadcast
    Notification {
        notification_type: String,
        message: String,
    },
    /// A swarm plan was updated
    PlanUpdate { swarm_id: String, item_count: usize },
    /// A plan proposal was submitted
    PlanProposal {
        swarm_id: String,
        proposer_session: String,
        item_count: usize,
    },
    /// Shared context was updated
    ContextUpdate { swarm_id: String, key: String },
    /// Session status changed
    StatusChange {
        old_status: String,
        new_status: String,
    },
    /// Session joined/left swarm
    MemberChange {
        action: String, // "joined" or "left"
    },
}

/// A swarm event with metadata
#[derive(Clone, Debug)]
pub struct SwarmEvent {
    pub id: u64,
    pub session_id: String,
    pub session_name: Option<String>,
    pub swarm_id: Option<String>,
    pub event: SwarmEventType,
    pub timestamp: Instant,
    pub absolute_time: std::time::SystemTime,
}

/// Ring buffer for recent swarm events
const MAX_EVENT_HISTORY: usize = 500;

#[derive(Default)]
struct ClientDebugState {
    active_id: Option<String>,
    clients: HashMap<String, mpsc::UnboundedSender<(u64, String)>>,
}

#[derive(Clone, Debug)]
struct ClientConnectionInfo {
    client_id: String,
    session_id: String,
    connected_at: Instant,
    last_seen: Instant,
}

#[derive(Clone, Debug)]
enum DebugJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl DebugJobStatus {
    fn as_str(&self) -> &'static str {
        match self {
            DebugJobStatus::Queued => "queued",
            DebugJobStatus::Running => "running",
            DebugJobStatus::Completed => "completed",
            DebugJobStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
struct DebugJob {
    id: String,
    status: DebugJobStatus,
    command: String,
    session_id: Option<String>,
    created_at: Instant,
    started_at: Option<Instant>,
    finished_at: Option<Instant>,
    output: Option<String>,
    error: Option<String>,
}

impl DebugJob {
    fn summary_payload(&self) -> Value {
        let now = Instant::now();
        let elapsed_secs = now.duration_since(self.created_at).as_secs_f64();
        let run_secs = self.started_at.map(|s| now.duration_since(s).as_secs_f64());
        let total_secs = self
            .finished_at
            .map(|f| f.duration_since(self.created_at).as_secs_f64());

        serde_json::json!({
            "id": self.id.clone(),
            "status": self.status.as_str(),
            "command": self.command.clone(),
            "session_id": self.session_id.clone(),
            "elapsed_secs": elapsed_secs,
            "run_secs": run_secs,
            "total_secs": total_secs,
        })
    }

    fn status_payload(&self) -> Value {
        let mut payload = self.summary_payload();
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("output".to_string(), serde_json::json!(self.output.clone()));
            obj.insert("error".to_string(), serde_json::json!(self.error.clone()));
        }
        payload
    }
}

impl ClientDebugState {
    fn register(&mut self, client_id: String, tx: mpsc::UnboundedSender<(u64, String)>) {
        self.active_id = Some(client_id.clone());
        self.clients.insert(client_id, tx);
    }

    fn unregister(&mut self, client_id: &str) {
        self.clients.remove(client_id);
        if self.active_id.as_deref() == Some(client_id) {
            self.active_id = self.clients.keys().next().cloned();
        }
    }

    fn active_sender(&mut self) -> Option<(String, mpsc::UnboundedSender<(u64, String)>)> {
        if let Some(active_id) = self.active_id.clone() {
            if let Some(tx) = self.clients.get(&active_id) {
                return Some((active_id, tx.clone()));
            }
        }
        if let Some((id, tx)) = self.clients.iter().next() {
            let id = id.clone();
            self.active_id = Some(id.clone());
            return Some((id, tx.clone()));
        }
        None
    }
}

/// Socket path for main communication
/// Can be overridden via JCODE_SOCKET env var
pub fn socket_path() -> PathBuf {
    if let Ok(custom) = std::env::var("JCODE_SOCKET") {
        return PathBuf::from(custom);
    }
    crate::storage::runtime_dir().join("jcode.sock")
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

fn sibling_socket_path(path: &std::path::Path) -> Option<PathBuf> {
    let filename = path.file_name()?.to_str()?;

    if let Some(base) = filename.strip_suffix("-debug.sock") {
        return Some(path.with_file_name(format!("{}.sock", base)));
    }

    if let Some(base) = filename.strip_suffix(".sock") {
        return Some(path.with_file_name(format!("{}-debug.sock", base)));
    }

    None
}

/// Remove a socket file and its sibling (main/debug) if present.
pub fn cleanup_socket_pair(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    if let Some(sibling) = sibling_socket_path(path) {
        let _ = std::fs::remove_file(sibling);
    }
}

/// Connect to a Unix socket, cleaning up stale socket files on connection-refused.
pub async fn connect_socket(path: &std::path::Path) -> Result<UnixStream> {
    match UnixStream::connect(path).await {
        Ok(stream) => Ok(stream),
        Err(err) => {
            let is_stale = err.kind() == std::io::ErrorKind::ConnectionRefused && path.exists();
            if is_stale {
                cleanup_socket_pair(path);
                anyhow::bail!(
                    "Stale socket removed at {}. Start/restart jcode and retry.",
                    path.display()
                );
            }
            Err(err.into())
        }
    }
}

#[cfg(test)]
mod socket_tests {
    use super::{cleanup_socket_pair, sibling_socket_path};

    #[test]
    fn sibling_socket_path_roundtrip() {
        let main = std::path::PathBuf::from("/tmp/jcode.sock");
        let debug = std::path::PathBuf::from("/tmp/jcode-debug.sock");

        assert_eq!(sibling_socket_path(&main), Some(debug.clone()));
        assert_eq!(sibling_socket_path(&debug), Some(main));
    }

    #[test]
    fn cleanup_socket_pair_removes_main_and_debug_files() {
        let stamp = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let dir = std::env::temp_dir();
        let main = dir.join(format!("jcode-test-{}.sock", stamp));
        let debug = dir.join(format!("jcode-test-{}-debug.sock", stamp));

        std::fs::write(&main, b"").expect("create main socket placeholder");
        std::fs::write(&debug, b"").expect("create debug socket placeholder");

        cleanup_socket_pair(&main);

        assert!(!main.exists(), "main socket file should be removed");
        assert!(!debug.exists(), "debug socket file should be removed");
    }
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
        if let Ok(rollback) = crate::build::rollback_binary_path() {
            if rollback.exists() {
                return Some((rollback, "rollback"));
            }
        }
    }

    if let Some(repo_dir) = crate::build::get_repo_dir() {
        let exe = repo_dir.join("target/release/jcode");
        if exe.exists() {
            return Some((exe, "release"));
        }
    }

    if let Some(exe) = crate::build::jcode_path_in_path() {
        return Some((exe, "path"));
    }
    None
}

fn canonicalize_or(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn git_common_dir_for(path: &std::path::Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(dir) = current {
        let dotgit = dir.join(".git");
        if dotgit.is_dir() {
            return Some(canonicalize_or(dotgit));
        }
        if dotgit.is_file() {
            let content = std::fs::read_to_string(&dotgit).ok()?;
            let gitdir_line = content
                .lines()
                .find(|line| line.trim_start().starts_with("gitdir:"))?;
            let raw = gitdir_line
                .trim_start()
                .trim_start_matches("gitdir:")
                .trim();
            if raw.is_empty() {
                return None;
            }
            let gitdir = if std::path::Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                dir.join(raw)
            };
            let gitdir = canonicalize_or(gitdir);
            // Worktree gitdir looks like: <repo>/.git/worktrees/<name>
            if let Some(parent) = gitdir.parent() {
                if parent.file_name().and_then(|s| s.to_str()) == Some("worktrees") {
                    if let Some(common) = parent.parent() {
                        return Some(canonicalize_or(common.to_path_buf()));
                    }
                }
            }
            return Some(gitdir);
        }
        current = dir.parent();
    }
    None
}

fn swarm_id_for_dir(dir: Option<PathBuf>) -> Option<String> {
    if let Ok(sw_id) = std::env::var("JCODE_SWARM_ID") {
        let trimmed = sw_id.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let dir = dir?;
    if let Some(git_common) = git_common_dir_for(&dir) {
        return Some(git_common.to_string_lossy().to_string());
    }
    Some(dir.to_string_lossy().to_string())
}

fn server_has_newer_binary() -> bool {
    let current_exe = std::env::current_exe().ok();
    let Some((candidate, _label)) = server_update_candidate() else {
        return false;
    };

    let current_mtime = current_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let candidate_mtime = std::fs::metadata(&candidate)
        .ok()
        .and_then(|m| m.modified().ok());

    match (current_mtime, candidate_mtime) {
        (Some(current), Some(candidate)) => candidate > current,
        _ => {
            if let Some(current_exe) = current_exe {
                let current = canonicalize_or(current_exe);
                let candidate_path = canonicalize_or(candidate);
                current != candidate_path
            } else {
                false
            }
        }
    }
}

/// Exit code when server shuts down due to idle timeout
pub const EXIT_IDLE_TIMEOUT: i32 = 44;

/// Server identity for multi-server support
#[derive(Debug, Clone)]
pub struct ServerIdentity {
    /// Full server ID (e.g., "server_blazing_1705012345678")
    pub id: String,
    /// Short name (e.g., "blazing")
    pub name: String,
    /// Icon for display (e.g., "🔥")
    pub icon: String,
    /// Git hash of the binary
    pub git_hash: String,
    /// Version string (e.g., "v0.1.123")
    pub version: String,
}

impl ServerIdentity {
    /// Create identity for unnamed/legacy server
    pub fn unnamed() -> Self {
        Self {
            id: "server_default".to_string(),
            name: "default".to_string(),
            icon: "🔮".to_string(),
            git_hash: String::new(),
            version: env!("JCODE_VERSION").to_string(),
        }
    }

    /// Display name with icon (e.g., "🔥 blazing")
    pub fn display_name(&self) -> String {
        format!("{} {}", self.icon, self.name)
    }
}

/// Server state
pub struct Server {
    provider: Arc<dyn Provider>,
    socket_path: PathBuf,
    debug_socket_path: PathBuf,
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
    /// Ambient mode runner handle (None if ambient is disabled)
    ambient_runner: Option<AmbientRunnerHandle>,
    /// Shared MCP server pool (processes shared across sessions)
    mcp_pool: Arc<crate::mcp::SharedMcpPool>,
}

impl Server {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        let (client_debug_response_tx, _) = broadcast::channel(64);

        // Initialize ambient runner if enabled
        let ambient_runner = if crate::config::config().ambient.enabled {
            let safety = Arc::new(crate::safety::SafetySystem::new());
            Some(AmbientRunnerHandle::new(safety))
        } else {
            None
        };

        Self {
            provider,
            socket_path: socket_path(),
            debug_socket_path: debug_socket_path(),
            identity: ServerIdentity::unnamed(),
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
            ambient_runner,
            mcp_pool: Arc::new(crate::mcp::SharedMcpPool::from_default_config()),
        }
    }

    /// Create a new server with a generated name and identity
    /// Uses the registry for named socket paths
    pub fn new_named(provider: Arc<dyn Provider>, git_hash: &str) -> Self {
        use crate::id::{new_memorable_server_id, server_icon};

        let (id, name) = new_memorable_server_id();
        let icon = server_icon(&name).to_string();

        let identity = ServerIdentity {
            id,
            name: name.clone(),
            icon,
            git_hash: git_hash.to_string(),
            version: env!("JCODE_VERSION").to_string(),
        };

        let socket = server_socket_path(&name);
        let debug_socket = server_debug_socket_path(&name);

        let (event_tx, _) = broadcast::channel(1024);
        let (client_debug_response_tx, _) = broadcast::channel(64);

        let ambient_runner = if crate::config::config().ambient.enabled {
            let safety = Arc::new(crate::safety::SafetySystem::new());
            Some(AmbientRunnerHandle::new(safety))
        } else {
            None
        };

        Self {
            provider,
            socket_path: socket,
            debug_socket_path: debug_socket,
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
            ambient_runner,
            mcp_pool: Arc::new(crate::mcp::SharedMcpPool::from_default_config()),
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
        event_history: Arc<RwLock<Vec<SwarmEvent>>>,
        event_counter: Arc<std::sync::atomic::AtomicU64>,
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
                        history.push(event);
                        if history.len() > MAX_EVENT_HISTORY {
                            history.remove(0);
                        }
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

                    // Only check for conflicts when someone writes/edits (reads never conflict)
                    let is_write = matches!(touch.op, FileOp::Write | FileOp::Edit);
                    let previous_touches: Vec<FileAccess> = if is_write {
                        let touches = file_touches.read().await;
                        if let Some(accesses) = touches.get(&path) {
                            accesses
                                .iter()
                                .filter(|a| {
                                    a.session_id != session_id
                                        && swarm_session_ids.contains(&a.session_id)
                                        && matches!(a.op, FileOp::Write | FileOp::Edit)
                                })
                                .cloned()
                                .collect()
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    };

                    // If there are previous write conflicts from swarm members, send alerts
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
                                    if let Ok(agent) = agent.try_lock() {
                                        agent.queue_soft_interrupt(alert_msg.clone(), false);
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
                                    if let Ok(agent) = agent.try_lock() {
                                        agent.queue_soft_interrupt(alert_msg.clone(), false);
                                    }
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

        // Remove existing sockets
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(&self.debug_socket_path);

        let main_listener = UnixListener::bind(&self.socket_path)?;
        let debug_listener = UnixListener::bind(&self.debug_socket_path)?;

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

        // Write server git hash next to socket so clients can detect version mismatches
        let hash_path = format!("{}.hash", self.socket_path.display());
        let _ = std::fs::write(&hash_path, env!("JCODE_GIT_HASH"));

        // Spawn selfdev signal monitor (checks for reload/rollback signals)
        // Only run on selfdev servers to avoid non-selfdev servers picking up
        // rebuild-signal and exec'ing, which would kill all their client connections
        if is_selfdev_env() {
            tokio::spawn(async {
                monitor_selfdev_signals().await;
            });
        }

        // Log when we receive SIGTERM for debugging
        #[cfg(unix)]
        tokio::spawn(async {
            use tokio::signal::unix::{signal, SignalKind};
            if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                sigterm.recv().await;
                crate::logging::info("Server received SIGTERM, shutting down gracefully");
            }
        });

        // Spawn the bus monitor for swarm coordination
        let monitor_file_touches = Arc::clone(&self.file_touches);
        let monitor_swarm_members = Arc::clone(&self.swarm_members);
        let monitor_swarms_by_id = Arc::clone(&self.swarms_by_id);
        let monitor_swarm_plans = Arc::clone(&self.swarm_plans);
        let monitor_swarm_coordinators = Arc::clone(&self.swarm_coordinators);
        let monitor_shared_context = Arc::clone(&self.shared_context);
        let monitor_sessions = Arc::clone(&self.sessions);
        let monitor_event_history = Arc::clone(&self.event_history);
        let monitor_event_counter = Arc::clone(&self.event_counter);
        tokio::spawn(async move {
            Self::monitor_bus(
                monitor_file_touches,
                monitor_swarm_members,
                monitor_swarms_by_id,
                monitor_swarm_plans,
                monitor_swarm_coordinators,
                monitor_shared_context,
                monitor_sessions,
                monitor_event_history,
                monitor_event_counter,
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

        // Spawn ambient mode background loop if enabled
        if let Some(ref runner) = self.ambient_runner {
            let ambient_handle = runner.clone();
            let ambient_provider = Arc::clone(&self.provider);
            crate::logging::info("Ambient mode enabled - starting background loop");
            tokio::spawn(async move {
                ambient_handle.run_loop(ambient_provider).await;
            });
        }

        if debug_control_allowed() {
            crate::logging::info("Debug control enabled; idle timeout monitor disabled.");
        } else {
            // Spawn idle timeout monitor
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
        let main_server_name = self.identity.name.clone();
        let main_server_icon = self.identity.icon.clone();
        let main_ambient_runner = self.ambient_runner.clone();
        let main_mcp_pool = Arc::clone(&self.mcp_pool);

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
                        let server_name = main_server_name.clone();
                        let server_icon = main_server_icon.clone();
                        let ambient_runner = main_ambient_runner.clone();
                        let mcp_pool = Arc::clone(&main_mcp_pool);

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
                                server_name,
                                server_icon,
                                mcp_pool,
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
        let debug_server_identity = self.identity.clone();
        let debug_start_time = std::time::Instant::now();
        let debug_ambient_runner = self.ambient_runner.clone();

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
                        let server_identity = debug_server_identity.clone();
                        let server_start_time = debug_start_time;
                        let ambient_runner = debug_ambient_runner.clone();

                        tokio::spawn(async move {
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
                                server_identity,
                                server_start_time,
                                ambient_runner,
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
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    channel_subscriptions: Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    event_history: Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    server_name: String,
    server_icon: String,
    mcp_pool: Arc<crate::mcp::SharedMcpPool>,
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
    let mut processing_session_id: Option<String> = None;
    // Client selfdev status is determined by Subscribe request, not server's env
    let mut client_selfdev = false;

    let provider = provider_template.fork();
    let registry = Registry::new(provider.clone()).await;
    let mut swarm_enabled = crate::config::config().features.swarm;

    // Create a new session for this client
    let mut new_agent = Agent::new(Arc::clone(&provider), registry.clone());
    new_agent.set_memory_enabled(crate::config::config().features.memory);
    let mut client_session_id = new_agent.session_id().to_string();
    let friendly_name = new_agent.session_short_name().map(|s| s.to_string());
    let client_connection_id = id::new_id("conn");
    let connected_at = Instant::now();

    {
        let mut connections = client_connections.write().await;
        connections.insert(
            client_connection_id.clone(),
            ClientConnectionInfo {
                client_id: client_connection_id.clone(),
                session_id: client_session_id.clone(),
                connected_at,
                last_seen: connected_at,
            },
        );
    }

    {
        let mut current = global_session_id.write().await;
        if current.is_empty() || *current != client_session_id {
            *current = client_session_id.clone();
        }
    }

    // Get a handle to the soft interrupt queue BEFORE wrapping in Mutex
    // This allows queueing interrupts while the agent is processing
    let soft_interrupt_queue = new_agent.soft_interrupt_queue();

    let agent = Arc::new(Mutex::new(new_agent));
    {
        let mut sessions_guard = sessions.write().await;
        sessions_guard.insert(client_session_id.clone(), Arc::clone(&agent));
    }

    // Per-client event channel (not shared with other clients)
    let (client_event_tx, mut client_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ServerEvent>();

    // Get the working directory and swarm id (shared across worktrees)
    let working_dir = std::env::current_dir().ok();
    let swarm_id = if swarm_enabled {
        swarm_id_for_dir(working_dir.clone())
    } else {
        None
    };

    // Register this client as a swarm member
    {
        let mut members = swarm_members.write().await;
        let now = Instant::now();
        members.insert(
            client_session_id.clone(),
            SwarmMember {
                session_id: client_session_id.clone(),
                event_tx: client_event_tx.clone(),
                working_dir: working_dir.clone(),
                swarm_id: swarm_id.clone(),
                swarm_enabled,
                status: "spawned".to_string(),
                detail: None,
                friendly_name: friendly_name.clone(),
                role: "agent".to_string(),
                joined_at: now,
                last_status_change: now,
            },
        );
    }
    // Add to swarm by swarm_id (separate scope to avoid holding swarm_members lock)
    if let Some(ref id) = swarm_id {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(id.clone())
            .or_insert_with(HashSet::new)
            .insert(client_session_id.clone());
        record_swarm_event(
            &event_history,
            &event_counter,
            client_session_id.clone(),
            friendly_name.clone(),
            Some(id.clone()),
            SwarmEventType::MemberChange {
                action: "joined".to_string(),
            },
        )
        .await;
    }
    let mut is_new_coordinator = false;
    if let Some(ref id) = swarm_id {
        let mut coordinators = swarm_coordinators.write().await;
        if coordinators.get(id).is_none() {
            coordinators.insert(id.clone(), client_session_id.clone());
            is_new_coordinator = true;
        }
    }
    // Update role separately to avoid nested lock
    if is_new_coordinator {
        let mut members = swarm_members.write().await;
        if let Some(m) = members.get_mut(&client_session_id) {
            m.role = "coordinator".to_string();
        }
    }
    if let Some(ref id) = swarm_id {
        broadcast_swarm_status(id, &swarm_members, &swarms_by_id).await;
    }
    update_member_status(
        &client_session_id,
        "ready",
        None,
        &swarm_members,
        &swarms_by_id,
        Some(&event_history),
        Some(&event_counter),
    )
    .await;
    if is_new_coordinator {
        let msg = "You are the coordinator for this swarm.".to_string();
        let _ = client_event_tx.send(ServerEvent::Notification {
            from_session: client_session_id.clone(),
            from_name: friendly_name.clone(),
            notification_type: NotificationType::Message {
                scope: Some("swarm".to_string()),
                channel: None,
            },
            message: msg.clone(),
        });
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

    // Note: Don't send initial SessionId here - it's sent by the Subscribe handler
    // Sending it via the channel causes race conditions where it can arrive after
    // other events (like History) that are written directly to the socket.

    // Set up client debug command channel
    // This client becomes the "active" debug client that receives client: commands
    let (debug_cmd_tx, mut debug_cmd_rx) = mpsc::unbounded_channel::<(u64, String)>();
    let client_debug_id = id::new_id("client");
    {
        let mut debug_state = client_debug_state.write().await;
        debug_state.register(client_debug_id.clone(), debug_cmd_tx);
    }

    loop {
        line.clear();
        tokio::select! {
            // Handle client debug commands from debug socket
            debug_cmd = debug_cmd_rx.recv() => {
                if let Some((request_id, command)) = debug_cmd {
                    if client_event_tx
                        .send(ServerEvent::ClientDebugRequest {
                            id: request_id,
                            command,
                        })
                        .is_err()
                    {
                        let _ = client_debug_response_tx.send((
                            request_id,
                            "No TUI client connected".to_string(),
                        ));
                    }
                }
                continue;
            }
            done = processing_done_rx.recv() => {
                if let Some((done_id, result)) = done {
                    if Some(done_id) != processing_message_id {
                        crate::logging::warn(&format!(
                            "Done event id={} doesn't match processing_message_id={:?}, dropping",
                            done_id, processing_message_id
                        ));
                        continue;
                    }
                    crate::logging::info(&format!(
                        "Processing done for message id={}, result={}",
                        done_id,
                        if result.is_ok() { "ok" } else { "err" }
                    ));
                    processing_message_id = None;
                    processing_task = None;
                    client_is_processing = false;

                    let done_session = processing_session_id.take();
                    match result {
                        Ok(()) => {
                            if let Some(session_id) = done_session.as_deref() {
                                update_member_status(
                                    session_id,
                                    "ready",
                                    None,
                                    &swarm_members,
                                    &swarms_by_id,
                                    Some(&event_history),
                                    Some(&event_counter),
                                )
                                .await;
                            }
                            let _ = client_event_tx.send(ServerEvent::Done { id: done_id });
                        }
                        Err(e) => {
                            if let Some(session_id) = done_session.as_deref() {
                                update_member_status(
                                    session_id,
                                    "failed",
                                    Some(truncate_detail(&e.to_string(), 120)),
                                    &swarm_members,
                                    &swarms_by_id,
                                    Some(&event_history),
                                    Some(&event_counter),
                                )
                                .await;
                            }
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
                let n = match n {
                    Ok(n) => n,
                    Err(e) => {
                        crate::logging::error(&format!("Client read error: {}", e));
                        break;
                    }
                };
                if n == 0 {
                    break; // Client disconnected
                }
                let mut connections = client_connections.write().await;
                if let Some(info) = connections.get_mut(&client_connection_id) {
                    info.last_seen = Instant::now();
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
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
                continue;
            }
        };

        // Send ack
        let ack = ServerEvent::Ack { id: request.id() };
        let json = encode_event(&ack);
        {
            let mut w = writer.lock().await;
            if w.write_all(json.as_bytes()).await.is_err() {
                break;
            }
        }

        match request {
            Request::Message { id, content, images } => {
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
                processing_session_id = Some(client_session_id.clone());

                update_member_status(
                    &client_session_id,
                    "running",
                    Some(truncate_detail(&content, 120)),
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                )
                .await;

                let agent = Arc::clone(&agent);
                let tx = client_event_tx.clone();
                let done_tx = processing_done_tx.clone();
                crate::logging::info(&format!("Processing message id={} spawning task", id));
                processing_task = Some(tokio::spawn(async move {
                    let result = match std::panic::AssertUnwindSafe(
                        process_message_streaming_mpsc(agent, &content, images, tx),
                    )
                    .catch_unwind()
                    .await
                    {
                        Ok(r) => r,
                        Err(panic_payload) => {
                            let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                                s.to_string()
                            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic".to_string()
                            };
                            crate::logging::error(&format!(
                                "Processing task PANICKED for message id={}: {}",
                                id, msg
                            ));
                            Err(anyhow::anyhow!("Processing task panicked: {}", msg))
                        }
                    };
                    match &result {
                        Ok(()) => crate::logging::info(&format!(
                            "Processing task completed OK for message id={}",
                            id
                        )),
                        Err(e) => crate::logging::warn(&format!(
                            "Processing task completed with error for message id={}: {}",
                            id, e
                        )),
                    }
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
                    if let Some(session_id) = processing_session_id.take() {
                        update_member_status(
                            &session_id,
                            "stopped",
                            Some("cancelled".to_string()),
                            &swarm_members,
                            &swarms_by_id,
                            Some(&event_history),
                            Some(&event_counter),
                        )
                        .await;
                    }
                    if let Some(message_id) = processing_message_id.take() {
                        let _ = client_event_tx.send(ServerEvent::TextDelta {
                            text: "Interrupted".to_string(),
                        });
                        let _ = client_event_tx.send(ServerEvent::Done { id: message_id });
                    }
                }
            }

            Request::SoftInterrupt {
                id,
                content,
                urgent,
            } => {
                // Queue a soft interrupt message to be injected at the next safe point
                // Uses the pre-extracted queue handle so we don't need the agent lock
                if let Ok(mut q) = soft_interrupt_queue.lock() {
                    q.push(crate::agent::SoftInterruptMessage { content, urgent });
                }
                let _ = client_event_tx.send(ServerEvent::Ack { id });
            }

            Request::Clear { id } => {
                // Clear this client's session (create new agent)
                let preserve_debug = {
                    let agent_guard = agent.lock().await;
                    agent_guard.is_debug()
                };

                // Mark the old session as closed before replacing
                {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.mark_closed();
                }

                let mut new_agent = Agent::new(Arc::clone(&provider), registry.clone());
                let new_id = new_agent.session_id().to_string();

                // Enable self-dev mode when running in a self-dev environment
                if client_selfdev {
                    new_agent.set_canary("self-dev");
                    // selfdev tools should already be registered from initial connection
                }
                if preserve_debug {
                    new_agent.set_debug(true);
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

                // Update swarm membership to the new session id
                let swarm_id_for_update = {
                    let mut members = swarm_members.write().await;
                    if let Some(mut member) = members.remove(&client_session_id) {
                        let sid = member.swarm_id.clone();
                        member.session_id = new_id.clone();
                        member.status = "ready".to_string();
                        member.detail = None;
                        members.insert(new_id.clone(), member);
                        sid
                    } else {
                        None
                    }
                };
                if let Some(ref swarm_id) = swarm_id_for_update {
                    let mut swarms = swarms_by_id.write().await;
                    if let Some(swarm) = swarms.get_mut(swarm_id) {
                        swarm.remove(&client_session_id);
                        swarm.insert(new_id.clone());
                    }
                }
                update_member_status(
                    &new_id,
                    "ready",
                    None,
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                )
                .await;
                if let Some(swarm_id) = swarm_id_for_update {
                    rename_plan_participant(&swarm_id, &client_session_id, &new_id, &swarm_plans)
                        .await;
                }

                client_session_id = new_id.clone();
                {
                    let mut connections = client_connections.write().await;
                    if let Some(info) = connections.get_mut(&client_connection_id) {
                        info.session_id = new_id.clone();
                        info.last_seen = Instant::now();
                    }
                }
                let _ = client_event_tx.send(ServerEvent::SessionId { session_id: new_id });
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::Ping { id } => {
                let json = encode_event(&ServerEvent::Pong { id });
                let mut w = writer.lock().await;
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
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
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
            }

            Request::Subscribe {
                id,
                working_dir: subscribe_working_dir,
                selfdev,
            } => {
                // Update session working directory from client's cwd
                if let Some(ref dir) = subscribe_working_dir {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.set_working_dir(dir);
                    drop(agent_guard);

                    // Update swarm member's working directory + swarm id
                    let new_path = PathBuf::from(dir);
                    let new_swarm_id = swarm_id_for_dir(Some(new_path.clone()));
                    let mut old_swarm_id: Option<String> = None;
                    let mut updated_swarm_id: Option<String> = None;
                    // Update member fields (separate scope to avoid nested locks)
                    {
                        let mut members = swarm_members.write().await;
                        if let Some(member) = members.get_mut(&client_session_id) {
                            old_swarm_id = member.swarm_id.clone();
                            member.working_dir = Some(new_path);
                            member.swarm_id = if member.swarm_enabled {
                                new_swarm_id.clone()
                            } else {
                                None
                            };
                            updated_swarm_id = member.swarm_id.clone();
                        }
                    }
                    // Remove from old swarm group
                    if let Some(ref old_id) = old_swarm_id {
                        let mut swarms = swarms_by_id.write().await;
                        if let Some(swarm) = swarms.get_mut(old_id) {
                            swarm.remove(&client_session_id);
                            if swarm.is_empty() {
                                swarms.remove(old_id);
                            }
                        }
                    }
                    // Add to new swarm group
                    if let Some(ref new_id) = updated_swarm_id {
                        let mut swarms = swarms_by_id.write().await;
                        swarms
                            .entry(new_id.clone())
                            .or_insert_with(HashSet::new)
                            .insert(client_session_id.clone());
                    }
                    if let Some(old_id) = old_swarm_id.clone() {
                        let was_coordinator = {
                            let coordinators = swarm_coordinators.read().await;
                            coordinators
                                .get(&old_id)
                                .map(|id| id == &client_session_id)
                                .unwrap_or(false)
                        };
                        if was_coordinator {
                            let mut new_coordinator: Option<String> = None;
                            {
                                let swarms = swarms_by_id.read().await;
                                if let Some(swarm) = swarms.get(&old_id) {
                                    new_coordinator = swarm.iter().min().cloned();
                                }
                            }
                            {
                                let mut coordinators = swarm_coordinators.write().await;
                                coordinators.remove(&old_id);
                                if let Some(ref new_id) = new_coordinator {
                                    coordinators.insert(old_id.clone(), new_id.clone());
                                }
                            }
                            // Notify new coordinator (separate scope to avoid nested lock)
                            if let Some(new_id) = new_coordinator.clone() {
                                let members = swarm_members.read().await;
                                if let Some(member) = members.get(&new_id) {
                                    let msg =
                                        "You are now the coordinator for this swarm.".to_string();
                                    let _ = member.event_tx.send(ServerEvent::Notification {
                                        from_session: new_id.clone(),
                                        from_name: member.friendly_name.clone(),
                                        notification_type: NotificationType::Message {
                                            scope: Some("swarm".to_string()),
                                            channel: None,
                                        },
                                        message: msg.clone(),
                                    });
                                }
                            }
                        }
                    }
                    if let Some(new_id) = updated_swarm_id.clone() {
                        let mut coordinators = swarm_coordinators.write().await;
                        if coordinators.get(&new_id).is_none() {
                            coordinators.insert(new_id.clone(), client_session_id.clone());
                            let msg = "You are the coordinator for this swarm.".to_string();
                            let _ = client_event_tx.send(ServerEvent::Notification {
                                from_session: client_session_id.clone(),
                                from_name: friendly_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some("swarm".to_string()),
                                    channel: None,
                                },
                                message: msg.clone(),
                            });
                        }
                    }
                    if let Some(old_id) = old_swarm_id.clone() {
                        if updated_swarm_id.as_ref() != Some(&old_id) {
                            remove_plan_participant(&old_id, &client_session_id, &swarm_plans)
                                .await;
                        }
                        broadcast_swarm_status(&old_id, &swarm_members, &swarms_by_id).await;
                    }
                    if let Some(new_id) = updated_swarm_id {
                        if old_swarm_id.as_ref() != Some(&new_id) {
                            broadcast_swarm_status(&new_id, &swarm_members, &swarms_by_id).await;
                        }
                    }
                }

                let mut should_selfdev = client_selfdev;
                if matches!(selfdev, Some(true)) {
                    should_selfdev = true;
                }

                if !should_selfdev {
                    if let Some(ref dir) = subscribe_working_dir {
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

                // Register MCP tools (management tool + server tool proxies)
                // Shared pool means shared servers only spawn once across sessions
                registry
                    .register_mcp_tools(
                        Some(client_event_tx.clone()),
                        Some(Arc::clone(&mcp_pool)),
                        Some(client_session_id.clone()),
                    )
                    .await;

                // Note: Don't send SessionId here - it's included in the History response
                // from GetHistory. Sending it here causes race conditions when ResumeSession
                // is also called, as client_session_id may not yet be updated.
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::GetHistory { id } => {
                let _ = provider.prefetch_models().await;
                let (
                    messages,
                    is_canary,
                    provider_name,
                    provider_model,
                    available_models,
                    tool_names,
                ) = {
                    let agent_guard = agent.lock().await;
                    (
                        agent_guard.get_history(),
                        agent_guard.is_canary(),
                        agent_guard.provider_name(),
                        agent_guard.provider_model(),
                        agent_guard.available_models_display(),
                        agent_guard.tool_names().await,
                    )
                };

                // Build MCP server list with tool counts from registered tool names
                let mut mcp_map: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                for name in &tool_names {
                    if let Some(rest) = name.strip_prefix("mcp__") {
                        if let Some((server, _tool)) = rest.split_once("__") {
                            *mcp_map.entry(server.to_string()).or_default() += 1;
                        }
                    }
                }
                let mcp_servers: Vec<String> = mcp_map
                    .into_iter()
                    .map(|(name, count)| format!("{}:{}", name, count))
                    .collect();

                // Build skills list
                let skills = crate::skill::SkillRegistry::load()
                    .map(|r| r.list().iter().map(|s| s.name.clone()).collect())
                    .unwrap_or_default();

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
                    available_models,
                    mcp_servers,
                    skills,
                    total_tokens: None,
                    all_sessions,
                    client_count: Some(current_client_count),
                    is_canary: Some(is_canary),
                    server_version: Some(env!("JCODE_VERSION").to_string()),
                    server_name: Some(server_name.clone()),
                    server_icon: Some(server_icon.clone()),
                    server_has_update: Some(server_has_newer_binary()),
                };
                let json = encode_event(&event);
                let mut w = writer.lock().await;
                if w.write_all(json.as_bytes()).await.is_err() {
                    break;
                }
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
                let provider_arg = provider_cli_arg(provider.name());
                let model_arg = normalize_model_arg(provider.model());
                let socket_arg = socket_path().to_string_lossy().to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if let Err(e) = do_server_reload_with_progress(
                        progress_tx.clone(),
                        provider_arg,
                        model_arg,
                        socket_arg,
                    )
                    .await
                    {
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
                // Mark the current session as closed before switching
                {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.mark_closed();
                }

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

                // Register MCP tools for resumed sessions
                if result.is_ok() {
                    registry
                        .register_mcp_tools(
                            Some(client_event_tx.clone()),
                            Some(Arc::clone(&mcp_pool)),
                            Some(client_session_id.clone()),
                        )
                        .await;
                }

                match result {
                    Ok(()) => {
                        // Update client_session_id to match the restored session
                        let old_session_id = client_session_id.clone();
                        client_session_id = session_id.clone();

                        {
                            let mut sessions_guard = sessions.write().await;
                            sessions_guard.remove(&old_session_id);
                            sessions_guard.insert(session_id.clone(), Arc::clone(&agent));
                        }
                        {
                            let mut connections = client_connections.write().await;
                            if let Some(info) = connections.get_mut(&client_connection_id) {
                                info.session_id = session_id.clone();
                                info.last_seen = Instant::now();
                            }
                        }

                        {
                            let mut members = swarm_members.write().await;
                            if let Some(mut member) = members.remove(&old_session_id) {
                                if let Some(ref swarm_id) = member.swarm_id {
                                    let mut swarms = swarms_by_id.write().await;
                                    if let Some(swarm) = swarms.get_mut(swarm_id) {
                                        swarm.remove(&old_session_id);
                                        swarm.insert(session_id.clone());
                                    }
                                }
                                member.session_id = session_id.clone();
                                member.status = "ready".to_string();
                                member.detail = None;
                                members.insert(session_id.clone(), member);
                            }
                        }
                        {
                            let mut coordinators = swarm_coordinators.write().await;
                            for coord in coordinators.values_mut() {
                                if *coord == old_session_id {
                                    *coord = session_id.clone();
                                }
                            }
                        }
                        update_member_status(
                            &session_id,
                            "ready",
                            None,
                            &swarm_members,
                            &swarms_by_id,
                            Some(&event_history),
                            Some(&event_counter),
                        )
                        .await;
                        if let Some(swarm_id) = {
                            let members = swarm_members.read().await;
                            members.get(&session_id).and_then(|m| m.swarm_id.clone())
                        } {
                            rename_plan_participant(
                                &swarm_id,
                                &old_session_id,
                                &session_id,
                                &swarm_plans,
                            )
                            .await;
                        }

                        // Send updated history to client
                        let _ = provider.prefetch_models().await;
                        let (
                            messages,
                            is_canary,
                            provider_name,
                            provider_model,
                            available_models,
                            tool_names,
                        ) = {
                            let agent_guard = agent.lock().await;
                            (
                                agent_guard.get_history(),
                                agent_guard.is_canary(),
                                agent_guard.provider_name(),
                                agent_guard.provider_model(),
                                agent_guard.available_models_display(),
                                agent_guard.tool_names().await,
                            )
                        };

                        // Build MCP server list with tool counts
                        let mut mcp_map: std::collections::BTreeMap<String, usize> =
                            std::collections::BTreeMap::new();
                        for name in &tool_names {
                            if let Some(rest) = name.strip_prefix("mcp__") {
                                if let Some((server, _tool)) = rest.split_once("__") {
                                    *mcp_map.entry(server.to_string()).or_default() += 1;
                                }
                            }
                        }
                        let mcp_servers: Vec<String> = mcp_map
                            .into_iter()
                            .map(|(name, count)| format!("{}:{}", name, count))
                            .collect();

                        let skills = crate::skill::SkillRegistry::load()
                            .map(|r| r.list().iter().map(|s| s.name.clone()).collect())
                            .unwrap_or_default();

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
                            available_models,
                            mcp_servers,
                            skills,
                            total_tokens: None,
                            all_sessions,
                            client_count: Some(current_client_count),
                            is_canary: Some(is_canary),
                            server_version: Some(env!("JCODE_VERSION").to_string()),
                            server_name: Some(server_name.clone()),
                            server_icon: Some(server_icon.clone()),
                            server_has_update: Some(server_has_newer_binary()),
                        };
                        let json = encode_event(&event);
                        let mut w = writer.lock().await;
                        if w.write_all(json.as_bytes()).await.is_err() {
                            break;
                        }
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
                        provider_name: None,
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
                    result.map(|_| (agent_guard.provider_model(), agent_guard.provider_name()))
                };

                match result {
                    Ok((updated, pname)) => {
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: updated,
                            provider_name: Some(pname),
                            error: None,
                        });
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: current,
                            provider_name: None,
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
                        provider_name: None,
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
                    result.map(|_| (agent_guard.provider_model(), agent_guard.provider_name()))
                };
                match result {
                    Ok((updated, pname)) => {
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: updated,
                            provider_name: Some(pname),
                            error: None,
                        });
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::ModelChanged {
                            id,
                            model: current,
                            provider_name: None,
                            error: Some(e.to_string()),
                        });
                    }
                }
            }

            Request::SetFeature {
                id,
                feature,
                enabled,
            } => match feature {
                FeatureToggle::Memory => {
                    let mut agent_guard = agent.lock().await;
                    agent_guard.set_memory_enabled(enabled);
                    drop(agent_guard);
                    if !enabled {
                        crate::memory::clear_pending_memory();
                    }
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                }
                FeatureToggle::Swarm => {
                    if swarm_enabled == enabled {
                        let _ = client_event_tx.send(ServerEvent::Done { id });
                        continue;
                    }
                    swarm_enabled = enabled;

                    let (old_swarm_id, working_dir) = {
                        let mut members = swarm_members.write().await;
                        if let Some(member) = members.get_mut(&client_session_id) {
                            let old = member.swarm_id.clone();
                            let wd = member.working_dir.clone();
                            member.swarm_enabled = enabled;
                            if !enabled {
                                member.swarm_id = None;
                                member.role = "agent".to_string();
                            }
                            (old, wd)
                        } else {
                            (None, None)
                        }
                    };

                    if let Some(ref old_id) = old_swarm_id {
                        remove_session_from_swarm(
                            &client_session_id,
                            old_id,
                            &swarm_members,
                            &swarms_by_id,
                            &swarm_coordinators,
                            &swarm_plans,
                        )
                        .await;
                    }

                    if enabled {
                        let new_swarm_id = swarm_id_for_dir(working_dir);
                        if let Some(ref id) = new_swarm_id {
                            {
                                let mut swarms = swarms_by_id.write().await;
                                swarms
                                    .entry(id.clone())
                                    .or_insert_with(HashSet::new)
                                    .insert(client_session_id.clone());
                            }

                            let mut is_new_coordinator = false;
                            {
                                let mut coordinators = swarm_coordinators.write().await;
                                if coordinators.get(id).is_none() {
                                    coordinators.insert(id.clone(), client_session_id.clone());
                                    is_new_coordinator = true;
                                }
                            }

                            {
                                let mut members = swarm_members.write().await;
                                if let Some(member) = members.get_mut(&client_session_id) {
                                    member.swarm_id = Some(id.clone());
                                    member.role = if is_new_coordinator {
                                        "coordinator".to_string()
                                    } else {
                                        "agent".to_string()
                                    };
                                }
                            }

                            broadcast_swarm_status(id, &swarm_members, &swarms_by_id).await;

                            if is_new_coordinator {
                                let msg = "You are the coordinator for this swarm.".to_string();
                                let _ = client_event_tx.send(ServerEvent::Notification {
                                    from_session: client_session_id.clone(),
                                    from_name: friendly_name.clone(),
                                    notification_type: NotificationType::Message {
                                        scope: Some("swarm".to_string()),
                                        channel: None,
                                    },
                                    message: msg,
                                });
                            }
                        } else {
                            let _ = client_event_tx.send(ServerEvent::SwarmStatus {
                                members: Vec::new(),
                            });
                        }
                    } else {
                        let _ = client_event_tx.send(ServerEvent::SwarmStatus {
                            members: Vec::new(),
                        });
                    }

                    let _ = client_event_tx.send(ServerEvent::Done { id });
                }
            },

            Request::Split { id } => {
                // Clone the current session's messages into a new session
                let (new_session_id, new_session_name) = {
                    let agent_guard = agent.lock().await;
                    let parent_session_id = agent_guard.session_id().to_string();
                    let messages = agent_guard.messages().to_vec();
                    let working_dir = agent_guard.working_dir().map(|s| s.to_string());
                    let model = Some(agent_guard.provider_model());

                    // Create a new session with the same messages
                    let mut child = Session::create(Some(parent_session_id), None);
                    child.messages = messages;
                    child.working_dir = working_dir;
                    child.model = model;
                    child.status = crate::session::SessionStatus::Closed;

                    if let Err(e) = child.save() {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Failed to save split session: {}", e),
                        });
                        continue;
                    }

                    let name = child.display_name().to_string();
                    (child.id.clone(), name)
                };

                let _ = client_event_tx.send(ServerEvent::SplitResponse {
                    id,
                    new_session_id,
                    new_session_name,
                });
            }

            // Agent-to-agent communication
            Request::AgentRegister { id, .. } => {
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::AgentTask { id, task, .. } => {
                // Process as a message on this client's agent
                update_member_status(
                    &client_session_id,
                    "running",
                    Some(truncate_detail(&task, 120)),
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                )
                .await;
                let result = process_message_streaming_mpsc(
                    Arc::clone(&agent),
                    &task,
                    vec![],
                    client_event_tx.clone(),
                )
                .await;
                match result {
                    Ok(()) => {
                        update_member_status(
                            &client_session_id,
                            "completed",
                            None,
                            &swarm_members,
                            &swarms_by_id,
                            Some(&event_history),
                            Some(&event_counter),
                        )
                        .await;
                        let _ = client_event_tx.send(ServerEvent::Done { id });
                    }
                    Err(e) => {
                        update_member_status(
                            &client_session_id,
                            "failed",
                            Some(truncate_detail(&e.to_string(), 120)),
                            &swarm_members,
                            &swarms_by_id,
                            Some(&event_history),
                            Some(&event_counter),
                        )
                        .await;
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
                // Find the swarm id for this session
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    let friendly_name = {
                        let members = swarm_members.read().await;
                        members
                            .get(&req_session_id)
                            .and_then(|m| m.friendly_name.clone())
                    };

                    // Store the shared context
                    {
                        let mut ctx = shared_context.write().await;
                        let swarm_ctx = ctx.entry(swarm_id.clone()).or_insert_with(HashMap::new);
                        let now = Instant::now();
                        // Preserve created_at if updating existing context
                        let created_at = swarm_ctx.get(&key).map(|c| c.created_at).unwrap_or(now);
                        swarm_ctx.insert(
                            key.clone(),
                            SharedContext {
                                key: key.clone(),
                                value: value.clone(),
                                from_session: req_session_id.clone(),
                                from_name: friendly_name.clone(),
                                created_at,
                                updated_at: now,
                            },
                        );
                    }

                    // Notify other swarm members
                    let swarm_session_ids: Vec<String> = {
                        let swarms = swarms_by_id.read().await;
                        swarms
                            .get(&swarm_id)
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
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        req_session_id.clone(),
                        friendly_name.clone(),
                        Some(swarm_id.clone()),
                        SwarmEventType::ContextUpdate {
                            swarm_id: swarm_id.clone(),
                            key: key.clone(),
                        },
                    )
                    .await;
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm. Use a git repository to enable swarm features."
                            .to_string(),
                    });
                }
            }

            Request::CommRead {
                id,
                session_id: req_session_id,
                key,
            } => {
                // Find the swarm id for this session
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                let entries = if let Some(swarm_id) = swarm_id {
                    let ctx = shared_context.read().await;
                    if let Some(swarm_ctx) = ctx.get(&swarm_id) {
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
                to_session,
                channel,
            } => {
                // Find the swarm id for this session
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members.get(&from_session).and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    let friendly_name = {
                        let members = swarm_members.read().await;
                        members
                            .get(&from_session)
                            .and_then(|m| m.friendly_name.clone())
                    };

                    let swarm_session_ids: Vec<String> = {
                        let swarms = swarms_by_id.read().await;
                        swarms
                            .get(&swarm_id)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default()
                    };

                    // Validate DM recipient exists in swarm
                    if let Some(ref target) = to_session {
                        if !swarm_session_ids.contains(target) {
                            let _ = client_event_tx.send(ServerEvent::Error {
                                id,
                                message: format!("DM failed: session '{}' not in swarm", target),
                            });
                            continue;
                        }
                    }

                    let scope = if to_session.is_some() {
                        "dm"
                    } else if channel.is_some() {
                        "channel"
                    } else {
                        "broadcast"
                    };

                    let members = swarm_members.read().await;
                    let sessions = sessions.read().await;

                    let target_sessions: Vec<String> = if let Some(target) = to_session.clone() {
                        vec![target]
                    } else if let Some(ref ch) = channel {
                        // Channel message: check subscriptions
                        let subs = channel_subscriptions.read().await;
                        if let Some(channel_subs) = subs.get(&swarm_id).and_then(|s| s.get(ch)) {
                            // Send only to subscribed members (excluding sender)
                            channel_subs
                                .iter()
                                .filter(|sid| *sid != &from_session)
                                .cloned()
                                .collect()
                        } else {
                            // No subscriptions for this channel: broadcast to all
                            swarm_session_ids
                                .iter()
                                .filter(|sid| *sid != &from_session)
                                .cloned()
                                .collect()
                        }
                    } else {
                        swarm_session_ids
                            .iter()
                            .filter(|sid| *sid != &from_session)
                            .cloned()
                            .collect()
                    };

                    for sid in &target_sessions {
                        if !swarm_session_ids.contains(sid) {
                            continue;
                        }
                        if let Some(member) = members.get(sid) {
                            let from_label = friendly_name
                                .as_deref()
                                .unwrap_or(&from_session[..8.min(from_session.len())]);
                            let scope_label = match (scope, channel.as_deref()) {
                                ("channel", Some(ch)) => format!("#{}", ch),
                                ("dm", _) => "DM".to_string(),
                                _ => "broadcast".to_string(),
                            };
                            let notification_msg =
                                format!("{} from {}: {}", scope_label, from_label, message);
                            let _ = member.event_tx.send(ServerEvent::Notification {
                                from_session: from_session.clone(),
                                from_name: friendly_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some(scope.to_string()),
                                    channel: channel.clone(),
                                },
                                message: notification_msg.clone(),
                            });

                            // Also push to the agent's pending alerts
                            if let Some(agent) = sessions.get(sid) {
                                if let Ok(agent) = agent.try_lock() {
                                    agent.queue_soft_interrupt(notification_msg.clone(), false);
                                }
                            }
                        }
                    }
                    let scope_value = if scope == "channel" {
                        format!("#{}", channel.clone().unwrap_or_default())
                    } else {
                        scope.to_string()
                    };
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        from_session.clone(),
                        friendly_name.clone(),
                        Some(swarm_id.clone()),
                        SwarmEventType::Notification {
                            notification_type: scope_value,
                            message: truncate_detail(&message, 220),
                        },
                    )
                    .await;
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm. Use a git repository to enable swarm features."
                            .to_string(),
                    });
                }
            }

            Request::CommList {
                id,
                session_id: req_session_id,
            } => {
                // Find the swarm id for this session
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    let swarm_session_ids: Vec<String> = {
                        let swarms = swarms_by_id.read().await;
                        swarms
                            .get(&swarm_id)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default()
                    };

                    let members = swarm_members.read().await;
                    let touches = file_touches.read().await;

                    let member_list: Vec<AgentInfo> = swarm_session_ids
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
                                    role: Some(m.role.clone()),
                                }
                            })
                        })
                        .collect();

                    let _ = client_event_tx.send(ServerEvent::CommMembers {
                        id,
                        members: member_list,
                    });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm. Use a git repository to enable swarm features."
                            .to_string(),
                    });
                }
            }

            Request::CommProposePlan {
                id,
                session_id: req_session_id,
                items,
            } => {
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                let swarm_id = match swarm_id.as_ref() {
                    Some(sid) => sid.clone(),
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                        });
                        continue;
                    }
                };

                let (from_name, coordinator_id) = {
                    let members = swarm_members.read().await;
                    let from_name = members
                        .get(&req_session_id)
                        .and_then(|m| m.friendly_name.clone());
                    let coordinators = swarm_coordinators.read().await;
                    let coordinator_id = coordinators.get(&swarm_id).cloned();
                    (from_name, coordinator_id)
                };
                let from_label = from_name
                    .clone()
                    .unwrap_or_else(|| req_session_id.chars().take(8).collect());

                let Some(coordinator_id) = coordinator_id else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "No coordinator for this swarm.".to_string(),
                    });
                    continue;
                };

                if coordinator_id == req_session_id {
                    let (version, participant_ids) = {
                        let mut plans = swarm_plans.write().await;
                        let vp = plans
                            .entry(swarm_id.clone())
                            .or_insert_with(VersionedPlan::new);
                        vp.participants.insert(req_session_id.clone());
                        for item in &items {
                            if let Some(owner) = &item.assigned_to {
                                vp.participants.insert(owner.clone());
                            }
                        }
                        vp.items = items.clone();
                        vp.version += 1;
                        (vp.version, vp.participants.clone())
                    };

                    let members = swarm_members.read().await;
                    let agent_sessions = sessions.read().await;
                    let notification_msg = format!(
                        "Plan updated by {} ({} items, v{})",
                        from_label,
                        items.len(),
                        version
                    );
                    for sid in participant_ids {
                        if sid == req_session_id {
                            continue;
                        }
                        if let Some(member) = members.get(&sid) {
                            let _ = member.event_tx.send(ServerEvent::Notification {
                                from_session: req_session_id.clone(),
                                from_name: from_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some("plan".to_string()),
                                    channel: None,
                                },
                                message: notification_msg.clone(),
                            });
                        }
                        if let Some(agent) = agent_sessions.get(&sid) {
                            if let Ok(agent) = agent.try_lock() {
                                agent.queue_soft_interrupt(notification_msg.clone(), false);
                            }
                        }
                    }

                    broadcast_swarm_plan(
                        &swarm_id,
                        Some("coordinator_direct_update".to_string()),
                        &swarm_plans,
                        &swarm_members,
                        &swarms_by_id,
                    )
                    .await;
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        req_session_id.clone(),
                        from_name.clone(),
                        Some(swarm_id.clone()),
                        SwarmEventType::PlanUpdate {
                            swarm_id: swarm_id.clone(),
                            item_count: items.len(),
                        },
                    )
                    .await;

                    let _ = client_event_tx.send(ServerEvent::Done { id });
                    continue;
                }

                let proposal_key = format!("plan_proposal:{}", req_session_id);
                let proposal_value =
                    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
                {
                    let mut ctx = shared_context.write().await;
                    let swarm_ctx = ctx.entry(swarm_id.clone()).or_insert_with(HashMap::new);
                    let now = Instant::now();
                    swarm_ctx.insert(
                        proposal_key.clone(),
                        SharedContext {
                            key: proposal_key.clone(),
                            value: proposal_value,
                            from_session: req_session_id.clone(),
                            from_name: from_name.clone(),
                            created_at: now,
                            updated_at: now,
                        },
                    );
                }
                record_swarm_event(
                    &event_history,
                    &event_counter,
                    req_session_id.clone(),
                    from_name.clone(),
                    Some(swarm_id.clone()),
                    SwarmEventType::PlanProposal {
                        swarm_id: swarm_id.clone(),
                        proposer_session: req_session_id.clone(),
                        item_count: items.len(),
                    },
                )
                .await;

                let summary = summarize_plan_items(&items, 3);
                let notification_msg = format!(
                    "Plan proposal from {} ({} items). Summary: {}. Review with communicate read key '{}'.",
                    from_label,
                    items.len(),
                    summary,
                    proposal_key
                );

                let members = swarm_members.read().await;
                let agent_sessions = sessions.read().await;
                if let Some(member) = members.get(&coordinator_id) {
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: req_session_id.clone(),
                        from_name: from_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some("plan_proposal".to_string()),
                            channel: None,
                        },
                        message: notification_msg.clone(),
                    });
                    let _ = member.event_tx.send(ServerEvent::SwarmPlanProposal {
                        swarm_id: swarm_id.clone(),
                        proposer_session: req_session_id.clone(),
                        proposer_name: from_name.clone(),
                        items: items.clone(),
                        summary: summary.clone(),
                        proposal_key: proposal_key.clone(),
                    });
                }
                if let Some(agent) = agent_sessions.get(&coordinator_id) {
                    if let Ok(agent) = agent.try_lock() {
                        agent.queue_soft_interrupt(notification_msg.clone(), false);
                    }
                }

                if let Some(member) = members.get(&req_session_id) {
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: req_session_id.clone(),
                        from_name: from_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some("plan_proposal".to_string()),
                            channel: None,
                        },
                        message: "Plan proposal sent to coordinator (not yet applied).".to_string(),
                    });
                }
                if let Some(agent) = agent_sessions.get(&req_session_id) {
                    if let Ok(agent) = agent.try_lock() {
                        agent.queue_soft_interrupt(
                            "Plan proposal sent to coordinator (not yet applied).".to_string(),
                            false,
                        );
                    }
                }

                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommApprovePlan {
                id,
                session_id: req_session_id,
                proposer_session,
            } => {
                // Verify the requester is the coordinator
                let (_swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can approve plan proposals.".to_string(),
                    });
                    continue;
                }

                let swarm_id = match swarm_id.as_ref() {
                    Some(sid) => sid.clone(),
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                        });
                        continue;
                    }
                };

                // Read proposal from shared context
                let proposal_key = format!("plan_proposal:{}", proposer_session);
                let proposal_value = {
                    let ctx = shared_context.read().await;
                    ctx.get(&swarm_id)
                        .and_then(|sc| sc.get(&proposal_key))
                        .map(|c| c.value.clone())
                };

                let proposal = match proposal_value {
                    Some(v) => v,
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!(
                                "No pending plan proposal from session '{}'",
                                proposer_session
                            ),
                        });
                        continue;
                    }
                };

                // Parse and apply to swarm_plans
                if let Ok(items) = serde_json::from_str::<Vec<PlanItem>>(&proposal) {
                    let participant_ids = {
                        let mut plans = swarm_plans.write().await;
                        let vp = plans
                            .entry(swarm_id.clone())
                            .or_insert_with(VersionedPlan::new);
                        vp.items.extend(items.clone());
                        vp.version += 1;
                        vp.participants.insert(req_session_id.clone());
                        vp.participants.insert(proposer_session.clone());
                        for item in &items {
                            if let Some(owner) = &item.assigned_to {
                                vp.participants.insert(owner.clone());
                            }
                        }
                        vp.participants.clone()
                    };

                    // Remove proposal from shared context
                    {
                        let mut ctx = shared_context.write().await;
                        if let Some(swarm_ctx) = ctx.get_mut(&swarm_id) {
                            swarm_ctx.remove(&proposal_key);
                        }
                    }

                    broadcast_swarm_plan(
                        &swarm_id,
                        Some("proposal_approved".to_string()),
                        &swarm_plans,
                        &swarm_members,
                        &swarms_by_id,
                    )
                    .await;
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        req_session_id.clone(),
                        None,
                        Some(swarm_id.clone()),
                        SwarmEventType::PlanUpdate {
                            swarm_id: swarm_id.clone(),
                            item_count: items.len(),
                        },
                    )
                    .await;

                    let coordinator_name = {
                        let members = swarm_members.read().await;
                        members
                            .get(&req_session_id)
                            .and_then(|m| m.friendly_name.clone())
                    };

                    let members = swarm_members.read().await;
                    let agent_sessions = sessions.read().await;
                    for sid in participant_ids {
                        if let Some(member) = members.get(&sid) {
                            let msg = format!(
                                "Plan approved by coordinator: {} items added from {}",
                                items.len(),
                                proposer_session
                            );
                            let _ = member.event_tx.send(ServerEvent::Notification {
                                from_session: req_session_id.clone(),
                                from_name: coordinator_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some("plan".to_string()),
                                    channel: None,
                                },
                                message: msg.clone(),
                            });

                            if let Some(agent) = agent_sessions.get(&sid) {
                                if let Ok(agent) = agent.try_lock() {
                                    agent.queue_soft_interrupt(msg.clone(), false);
                                }
                            }
                        }
                    }
                }

                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommRejectPlan {
                id,
                session_id: req_session_id,
                proposer_session,
                reason,
            } => {
                // Verify the requester is the coordinator
                let (swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can reject plan proposals.".to_string(),
                    });
                    continue;
                }

                let swarm_id = match swarm_id {
                    Some(sid) => sid,
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                        });
                        continue;
                    }
                };

                // Check if proposal exists
                let proposal_key = format!("plan_proposal:{}", proposer_session);
                let proposal_exists = {
                    let ctx = shared_context.read().await;
                    ctx.get(&swarm_id)
                        .and_then(|sc| sc.get(&proposal_key))
                        .is_some()
                };

                if !proposal_exists {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!(
                            "No pending plan proposal from session '{}'",
                            proposer_session
                        ),
                    });
                    continue;
                }

                // Remove proposal from shared context
                {
                    let mut ctx = shared_context.write().await;
                    if let Some(swarm_ctx) = ctx.get_mut(&swarm_id) {
                        swarm_ctx.remove(&proposal_key);
                    }
                }

                // Notify the proposer
                let coordinator_name = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.friendly_name.clone())
                };

                let members = swarm_members.read().await;
                let agent_sessions = sessions.read().await;
                if let Some(member) = members.get(&proposer_session) {
                    let reason_msg = reason
                        .as_ref()
                        .map(|r| format!(": {}", r))
                        .unwrap_or_default();
                    let msg = format!(
                        "Your plan proposal was rejected by the coordinator{}",
                        reason_msg
                    );
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: req_session_id.clone(),
                        from_name: coordinator_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some("dm".to_string()),
                            channel: None,
                        },
                        message: msg.clone(),
                    });

                    if let Some(agent) = agent_sessions.get(&proposer_session) {
                        if let Ok(agent) = agent.try_lock() {
                            agent.queue_soft_interrupt(msg, false);
                        }
                    }
                }
                record_swarm_event(
                    &event_history,
                    &event_counter,
                    req_session_id.clone(),
                    coordinator_name,
                    Some(swarm_id.clone()),
                    SwarmEventType::Notification {
                        notification_type: "plan_rejected".to_string(),
                        message: proposer_session.clone(),
                    },
                )
                .await;

                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommSpawn {
                id,
                session_id: req_session_id,
                working_dir,
                initial_message,
            } => {
                // Verify the requester is the coordinator
                let (swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can spawn new agents.".to_string(),
                    });
                    continue;
                }
                let swarm_id = match swarm_id {
                    Some(sid) => sid,
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                        });
                        continue;
                    }
                };

                let cmd = if let Some(ref dir) = working_dir {
                    format!("create_session:{}", dir)
                } else {
                    "create_session".to_string()
                };
                let coordinator_model = {
                    let agent_sessions = sessions.read().await;
                    agent_sessions.get(&req_session_id).and_then(|agent| {
                        agent
                            .try_lock()
                            .ok()
                            .map(|agent_guard| agent_guard.provider_model())
                    })
                };

                match create_headless_session(
                    &sessions,
                    &global_session_id,
                    &provider_template,
                    &cmd,
                    &swarm_members,
                    &swarms_by_id,
                    &swarm_coordinators,
                    &swarm_plans,
                    coordinator_model,
                )
                .await
                {
                    Ok(result_json) => {
                        let new_session_id =
                            serde_json::from_str::<serde_json::Value>(&result_json)
                                .ok()
                                .and_then(|v| {
                                    v.get("session_id")
                                        .and_then(|s| s.as_str())
                                        .map(|s| s.to_string())
                                })
                                .unwrap_or_default();

                        {
                            let mut plans = swarm_plans.write().await;
                            if let Some(vp) = plans.get_mut(&swarm_id) {
                                if !vp.items.is_empty() || !vp.participants.is_empty() {
                                    vp.participants.insert(req_session_id.clone());
                                    vp.participants.insert(new_session_id.clone());
                                }
                            }
                        }

                        broadcast_swarm_plan(
                            &swarm_id,
                            Some("participant_spawned".to_string()),
                            &swarm_plans,
                            &swarm_members,
                            &swarms_by_id,
                        )
                        .await;
                        record_swarm_event_for_session(
                            &new_session_id,
                            SwarmEventType::MemberChange {
                                action: "joined".to_string(),
                            },
                            &swarm_members,
                            &event_history,
                            &event_counter,
                        )
                        .await;

                        // Queue initial message as soft interrupt if provided
                        if let Some(ref msg) = initial_message {
                            let agent_sessions = sessions.read().await;
                            if let Some(agent) = agent_sessions.get(&new_session_id) {
                                if let Ok(agent) = agent.try_lock() {
                                    agent.queue_soft_interrupt(msg.clone(), false);
                                }
                            }
                        }

                        let _ = client_event_tx.send(ServerEvent::CommSpawnResponse {
                            id,
                            session_id: req_session_id.clone(),
                            new_session_id,
                        });
                    }
                    Err(e) => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Failed to spawn agent: {}", e),
                        });
                    }
                }
            }

            Request::CommStop {
                id,
                session_id: req_session_id,
                target_session,
            } => {
                // Verify the requester is the coordinator
                let (swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can stop agents.".to_string(),
                    });
                    continue;
                }

                // Remove session and trigger final memory extraction
                let mut sessions_guard = sessions.write().await;
                let removed_agent = sessions_guard.remove(&target_session);
                drop(sessions_guard);
                if let Some(agent_arc) = removed_agent {
                    {
                        let agent = agent_arc.lock().await;
                        let memory_enabled = agent.memory_enabled();
                        let transcript = if memory_enabled {
                            Some(agent.build_transcript_for_extraction())
                        } else {
                            None
                        };
                        let sid = target_session.clone();
                        drop(agent);
                        if let Some(transcript) = transcript {
                            crate::memory_agent::trigger_final_extraction(transcript, sid);
                        }
                    }
                    // Clean up swarm membership
                    let (removed_swarm_id, removed_name) = {
                        let mut members = swarm_members.write().await;
                        if let Some(member) = members.remove(&target_session) {
                            (member.swarm_id, member.friendly_name)
                        } else {
                            (None, None)
                        }
                    };
                    if let Some(ref sid) = removed_swarm_id {
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            target_session.clone(),
                            removed_name.clone(),
                            Some(sid.clone()),
                            SwarmEventType::MemberChange {
                                action: "left".to_string(),
                            },
                        )
                        .await;
                        remove_plan_participant(sid, &target_session, &swarm_plans).await;
                        let mut swarms = swarms_by_id.write().await;
                        if let Some(swarm) = swarms.get_mut(sid) {
                            swarm.remove(&target_session);
                            if swarm.is_empty() {
                                swarms.remove(sid);
                            }
                        }
                        // Handle coordinator re-election if needed
                        let was_coordinator = {
                            let coordinators = swarm_coordinators.read().await;
                            coordinators
                                .get(sid)
                                .map(|c| c == &target_session)
                                .unwrap_or(false)
                        };
                        if was_coordinator {
                            let new_coordinator = {
                                let swarms = swarms_by_id.read().await;
                                swarms.get(sid).and_then(|s| s.iter().min().cloned())
                            };
                            let mut coordinators = swarm_coordinators.write().await;
                            coordinators.remove(sid);
                            if let Some(ref new_id) = new_coordinator {
                                coordinators.insert(sid.clone(), new_id.clone());
                                let mut members = swarm_members.write().await;
                                if let Some(m) = members.get_mut(new_id) {
                                    m.role = "coordinator".to_string();
                                }
                                let mut plans = swarm_plans.write().await;
                                if let Some(vp) = plans.get_mut(sid) {
                                    vp.participants.insert(new_id.clone());
                                }
                            }
                        }
                        broadcast_swarm_status(sid, &swarm_members, &swarms_by_id).await;
                    }
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!("Unknown session '{}'", target_session),
                    });
                }
            }

            Request::CommAssignRole {
                id,
                session_id: req_session_id,
                target_session,
                role,
            } => {
                // Verify the requester is the coordinator
                let (swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can assign roles.".to_string(),
                    });
                    continue;
                }

                let swarm_id = match swarm_id {
                    Some(sid) => sid,
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                        });
                        continue;
                    }
                };

                // Update role on target member
                {
                    let mut members = swarm_members.write().await;
                    if let Some(m) = members.get_mut(&target_session) {
                        m.role = role.clone();
                    } else {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Unknown session '{}'", target_session),
                        });
                        continue;
                    }
                }

                // If assigning coordinator, update swarm_coordinators
                if role == "coordinator" {
                    {
                        let mut coordinators = swarm_coordinators.write().await;
                        coordinators.insert(swarm_id.clone(), target_session.clone());
                    }
                    // Demote the old coordinator to agent (separate scope to avoid nested lock)
                    let mut members = swarm_members.write().await;
                    if let Some(m) = members.get_mut(&req_session_id) {
                        if m.session_id != target_session {
                            m.role = "agent".to_string();
                        }
                    }
                }

                broadcast_swarm_status(&swarm_id, &swarm_members, &swarms_by_id).await;
                record_swarm_event(
                    &event_history,
                    &event_counter,
                    req_session_id.clone(),
                    None,
                    Some(swarm_id.clone()),
                    SwarmEventType::Notification {
                        notification_type: "role_assignment".to_string(),
                        message: format!("{} -> {}", target_session, role),
                    },
                )
                .await;
                let _ = client_event_tx.send(ServerEvent::Done { id });
            }

            Request::CommSummary {
                id,
                session_id: _req_session_id,
                target_session,
                limit,
            } => {
                let limit = limit.unwrap_or(10);
                let agent_sessions = sessions.read().await;
                if let Some(agent) = agent_sessions.get(&target_session) {
                    let tool_calls = if let Ok(agent) = agent.try_lock() {
                        let history = agent.get_history();
                        let mut calls: Vec<crate::protocol::ToolCallSummary> = Vec::new();
                        for msg in history.iter().rev() {
                            if calls.len() >= limit {
                                break;
                            }
                            if let Some(tool_names) = &msg.tool_calls {
                                for name in tool_names {
                                    calls.push(crate::protocol::ToolCallSummary {
                                        tool_name: name.clone(),
                                        brief_output: truncate_detail(&msg.content, 200),
                                        timestamp_secs: None,
                                    });
                                    if calls.len() >= limit {
                                        break;
                                    }
                                }
                            }
                        }
                        calls.reverse();
                        calls
                    } else {
                        Vec::new()
                    };
                    let _ = client_event_tx.send(ServerEvent::CommSummaryResponse {
                        id,
                        session_id: target_session,
                        tool_calls,
                    });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!("Unknown session '{}'", target_session),
                    });
                }
            }

            Request::CommReadContext {
                id,
                session_id: _req_session_id,
                target_session,
            } => {
                let agent_sessions = sessions.read().await;
                if let Some(agent) = agent_sessions.get(&target_session) {
                    let messages = if let Ok(agent) = agent.try_lock() {
                        agent.get_history()
                    } else {
                        Vec::new()
                    };
                    let _ = client_event_tx.send(ServerEvent::CommContextHistory {
                        id,
                        session_id: target_session,
                        messages,
                    });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: format!("Unknown session '{}'", target_session),
                    });
                }
            }

            Request::CommResyncPlan {
                id,
                session_id: req_session_id,
            } => {
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    let plan_state = {
                        let mut plans = swarm_plans.write().await;
                        plans.get_mut(&swarm_id).map(|vp| {
                            vp.participants.insert(req_session_id.clone());
                            (vp.version, vp.items.len())
                        })
                    };
                    if let Some((version, item_count)) = plan_state {
                        if let Some(member) = swarm_members.read().await.get(&req_session_id) {
                            let _ = member.event_tx.send(ServerEvent::Notification {
                                from_session: req_session_id.clone(),
                                from_name: member.friendly_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some("plan".to_string()),
                                    channel: None,
                                },
                                message: format!(
                                    "Plan attached to this session (v{}, {} items).",
                                    version, item_count
                                ),
                            });
                        }
                        broadcast_swarm_plan(
                            &swarm_id,
                            Some("resync".to_string()),
                            &swarm_plans,
                            &swarm_members,
                            &swarms_by_id,
                        )
                        .await;
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            req_session_id.clone(),
                            None,
                            Some(swarm_id.clone()),
                            SwarmEventType::PlanUpdate {
                                swarm_id: swarm_id.clone(),
                                item_count,
                            },
                        )
                        .await;
                        let _ = client_event_tx.send(ServerEvent::Done { id });
                    } else {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "No swarm plan exists for this swarm.".to_string(),
                        });
                    }
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm.".to_string(),
                    });
                }
            }

            Request::CommAssignTask {
                id,
                session_id: req_session_id,
                target_session,
                task_id,
                message,
            } => {
                // Verify the requester is the coordinator
                let (swarm_id, is_coordinator) = {
                    let members = swarm_members.read().await;
                    let swarm_id = members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone());
                    let is_coordinator = if let Some(ref sid) = swarm_id {
                        let coordinators = swarm_coordinators.read().await;
                        coordinators
                            .get(sid)
                            .map(|c| c == &req_session_id)
                            .unwrap_or(false)
                    } else {
                        false
                    };
                    (swarm_id, is_coordinator)
                };

                if !is_coordinator {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Only the coordinator can assign tasks.".to_string(),
                    });
                    continue;
                }

                let swarm_id = match swarm_id {
                    Some(sid) => sid,
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: "Not in a swarm.".to_string(),
                        });
                        continue;
                    }
                };

                // Find and update the plan item
                let (task_content, participant_ids, plan_item_count) = {
                    let mut plans = swarm_plans.write().await;
                    let vp = plans
                        .entry(swarm_id.clone())
                        .or_insert_with(VersionedPlan::new);
                    let found = vp.items.iter_mut().find(|t| t.id == task_id);
                    if let Some(item) = found {
                        item.assigned_to = Some(target_session.clone());
                        item.status = "queued".to_string();
                        vp.version += 1;
                        vp.participants.insert(req_session_id.clone());
                        vp.participants.insert(target_session.clone());
                        (
                            Some(item.content.clone()),
                            vp.participants.clone(),
                            vp.items.len(),
                        )
                    } else {
                        (None, HashSet::new(), 0)
                    }
                };

                match task_content {
                    Some(content) => {
                        broadcast_swarm_plan(
                            &swarm_id,
                            Some("task_assigned".to_string()),
                            &swarm_plans,
                            &swarm_members,
                            &swarms_by_id,
                        )
                        .await;
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            req_session_id.clone(),
                            None,
                            Some(swarm_id.clone()),
                            SwarmEventType::PlanUpdate {
                                swarm_id: swarm_id.clone(),
                                item_count: plan_item_count,
                            },
                        )
                        .await;

                        // Notify target via soft interrupt
                        let coordinator_name = {
                            let members = swarm_members.read().await;
                            members
                                .get(&req_session_id)
                                .and_then(|m| m.friendly_name.clone())
                        };
                        let msg = if let Some(ref extra) = message {
                            format!(
                                "Task assigned to you by coordinator: {} — {}",
                                content, extra
                            )
                        } else {
                            format!("Task assigned to you by coordinator: {}", content)
                        };

                        let target_agent = {
                            let agent_sessions = sessions.read().await;
                            agent_sessions.get(&target_session).cloned()
                        };
                        if let Some(agent) = target_agent.as_ref() {
                            if let Ok(agent) = agent.try_lock() {
                                agent.queue_soft_interrupt(msg.clone(), false);
                            }
                        }
                        if let Some(member) = swarm_members.read().await.get(&target_session) {
                            let _ = member.event_tx.send(ServerEvent::Notification {
                                from_session: req_session_id.clone(),
                                from_name: coordinator_name.clone(),
                                notification_type: NotificationType::Message {
                                    scope: Some("dm".to_string()),
                                    channel: None,
                                },
                                message: msg,
                            });
                        }

                        // Headless sessions do not have a request loop to consume queued
                        // interrupts, so run assigned work immediately when no client owns it.
                        let target_has_client = {
                            let conns = client_connections.read().await;
                            conns.values().any(|c| c.session_id == target_session)
                        };
                        if !target_has_client {
                            if let Some(agent_arc) = target_agent {
                                let target_session_for_run = target_session.clone();
                                let swarm_members_for_run = Arc::clone(&swarm_members);
                                let swarms_for_run = Arc::clone(&swarms_by_id);
                                let swarm_plans_for_run = Arc::clone(&swarm_plans);
                                let swarm_id_for_run = swarm_id.clone();
                                let task_id_for_run = task_id.clone();
                                let event_history_for_run = Arc::clone(&event_history);
                                let event_counter_for_run = Arc::clone(&event_counter);
                                let assignment_text = if let Some(extra) = message.clone() {
                                    format!(
                                        "{}\n\nAdditional coordinator instructions:\n{}",
                                        content, extra
                                    )
                                } else {
                                    content.clone()
                                };
                                tokio::spawn(async move {
                                    {
                                        let mut plans = swarm_plans_for_run.write().await;
                                        if let Some(vp) = plans.get_mut(&swarm_id_for_run) {
                                            if let Some(item) = vp
                                                .items
                                                .iter_mut()
                                                .find(|t| t.id == task_id_for_run)
                                            {
                                                item.status = "running".to_string();
                                                vp.version += 1;
                                            }
                                        }
                                    }
                                    broadcast_swarm_plan(
                                        &swarm_id_for_run,
                                        Some("task_running".to_string()),
                                        &swarm_plans_for_run,
                                        &swarm_members_for_run,
                                        &swarms_for_run,
                                    )
                                    .await;
                                    update_member_status(
                                        &target_session_for_run,
                                        "running",
                                        Some(truncate_detail(&assignment_text, 120)),
                                        &swarm_members_for_run,
                                        &swarms_for_run,
                                        Some(&event_history_for_run),
                                        Some(&event_counter_for_run),
                                    )
                                    .await;

                                    let result = {
                                        let mut agent = agent_arc.lock().await;
                                        agent.run_once_capture(&assignment_text).await
                                    };

                                    match result {
                                        Ok(_) => {
                                            {
                                                let mut plans = swarm_plans_for_run.write().await;
                                                if let Some(vp) = plans.get_mut(&swarm_id_for_run) {
                                                    if let Some(item) = vp
                                                        .items
                                                        .iter_mut()
                                                        .find(|t| t.id == task_id_for_run)
                                                    {
                                                        item.status = "done".to_string();
                                                        vp.version += 1;
                                                    }
                                                }
                                            }
                                            broadcast_swarm_plan(
                                                &swarm_id_for_run,
                                                Some("task_completed".to_string()),
                                                &swarm_plans_for_run,
                                                &swarm_members_for_run,
                                                &swarms_for_run,
                                            )
                                            .await;
                                            update_member_status(
                                                &target_session_for_run,
                                                "completed",
                                                None,
                                                &swarm_members_for_run,
                                                &swarms_for_run,
                                                Some(&event_history_for_run),
                                                Some(&event_counter_for_run),
                                            )
                                            .await;
                                        }
                                        Err(e) => {
                                            {
                                                let mut plans = swarm_plans_for_run.write().await;
                                                if let Some(vp) = plans.get_mut(&swarm_id_for_run) {
                                                    if let Some(item) = vp
                                                        .items
                                                        .iter_mut()
                                                        .find(|t| t.id == task_id_for_run)
                                                    {
                                                        item.status = "failed".to_string();
                                                        vp.version += 1;
                                                    }
                                                }
                                            }
                                            broadcast_swarm_plan(
                                                &swarm_id_for_run,
                                                Some("task_failed".to_string()),
                                                &swarm_plans_for_run,
                                                &swarm_members_for_run,
                                                &swarms_for_run,
                                            )
                                            .await;
                                            update_member_status(
                                                &target_session_for_run,
                                                "failed",
                                                Some(truncate_detail(&e.to_string(), 120)),
                                                &swarm_members_for_run,
                                                &swarms_for_run,
                                                Some(&event_history_for_run),
                                                Some(&event_counter_for_run),
                                            )
                                            .await;
                                        }
                                    }
                                });
                            }
                        }

                        let plan_msg = format!(
                            "Plan updated: task '{}' assigned to {}.",
                            task_id, target_session
                        );
                        let members = swarm_members.read().await;
                        for sid in participant_ids {
                            if sid == target_session || sid == req_session_id {
                                continue;
                            }
                            if let Some(member) = members.get(&sid) {
                                let _ = member.event_tx.send(ServerEvent::Notification {
                                    from_session: req_session_id.clone(),
                                    from_name: coordinator_name.clone(),
                                    notification_type: NotificationType::Message {
                                        scope: Some("plan".to_string()),
                                        channel: None,
                                    },
                                    message: plan_msg.clone(),
                                });
                            }
                        }

                        let _ = client_event_tx.send(ServerEvent::Done { id });
                    }
                    None => {
                        let _ = client_event_tx.send(ServerEvent::Error {
                            id,
                            message: format!("Task '{}' not found in swarm plan", task_id),
                        });
                    }
                }
            }

            Request::CommSubscribeChannel {
                id,
                session_id: req_session_id,
                channel,
            } => {
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    let mut subs = channel_subscriptions.write().await;
                    subs.entry(swarm_id.clone())
                        .or_default()
                        .entry(channel.clone())
                        .or_default()
                        .insert(req_session_id.clone());
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        req_session_id.clone(),
                        None,
                        Some(swarm_id.clone()),
                        SwarmEventType::Notification {
                            notification_type: "channel_subscribe".to_string(),
                            message: channel.clone(),
                        },
                    )
                    .await;
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm.".to_string(),
                    });
                }
            }

            Request::CommUnsubscribeChannel {
                id,
                session_id: req_session_id,
                channel,
            } => {
                let swarm_id = {
                    let members = swarm_members.read().await;
                    members
                        .get(&req_session_id)
                        .and_then(|m| m.swarm_id.clone())
                };

                if let Some(swarm_id) = swarm_id {
                    let mut subs = channel_subscriptions.write().await;
                    if let Some(swarm_subs) = subs.get_mut(&swarm_id) {
                        if let Some(channel_subs) = swarm_subs.get_mut(&channel) {
                            channel_subs.remove(&req_session_id);
                            if channel_subs.is_empty() {
                                swarm_subs.remove(&channel);
                            }
                        }
                    }
                    record_swarm_event(
                        &event_history,
                        &event_counter,
                        req_session_id.clone(),
                        None,
                        Some(swarm_id.clone()),
                        SwarmEventType::Notification {
                            notification_type: "channel_unsubscribe".to_string(),
                            message: channel.clone(),
                        },
                    )
                    .await;
                    let _ = client_event_tx.send(ServerEvent::Done { id });
                } else {
                    let _ = client_event_tx.send(ServerEvent::Error {
                        id,
                        message: "Not in a swarm.".to_string(),
                    });
                }
            }

            // These are handled via channels, not direct requests from TUI
            Request::ClientDebugCommand { id, .. } => {
                let _ = client_event_tx.send(ServerEvent::Error {
                    id,
                    message: "ClientDebugCommand is for internal use only".to_string(),
                });
            }
            Request::ClientDebugResponse { id, output } => {
                let _ = client_debug_response_tx.send((id, output));
            }
        }
    }

    // Clean up: remove this client's session from the map
    // Mark session status and persist before dropping the agent
    let disconnected_while_processing = client_is_processing
        || processing_task
            .as_ref()
            .map(|handle| !handle.is_finished())
            .unwrap_or(false);
    {
        let mut sessions_guard = sessions.write().await;
        if let Some(agent_arc) = sessions_guard.remove(&client_session_id) {
            drop(sessions_guard);
            let mut agent = agent_arc.lock().await;

            if disconnected_while_processing {
                agent.mark_crashed(Some("Client disconnected while processing".to_string()));
            } else {
                agent.mark_closed();
            }

            let memory_enabled = agent.memory_enabled();
            let transcript = if memory_enabled {
                Some(agent.build_transcript_for_extraction())
            } else {
                None
            };
            let sid = client_session_id.clone();
            drop(agent);
            if let Some(transcript) = transcript {
                crate::memory_agent::trigger_final_extraction(transcript, sid);
            }
        }
    }

    // Clean up: remove from swarm tracking
    {
        let crashed = disconnected_while_processing;
        let (status, detail) = if crashed {
            ("crashed", Some("disconnect while running".to_string()))
        } else {
            ("stopped", Some("disconnected".to_string()))
        };
        update_member_status(
            &client_session_id,
            status,
            detail,
            &swarm_members,
            &swarms_by_id,
            Some(&event_history),
            Some(&event_counter),
        )
        .await;

        let (swarm_id, removed_name) = {
            let mut members = swarm_members.write().await;
            if let Some(member) = members.remove(&client_session_id) {
                (member.swarm_id, member.friendly_name)
            } else {
                (None, None)
            }
        };

        if let Some(ref swarm_id) = swarm_id {
            record_swarm_event(
                &event_history,
                &event_counter,
                client_session_id.clone(),
                removed_name.clone(),
                Some(swarm_id.clone()),
                SwarmEventType::MemberChange {
                    action: "left".to_string(),
                },
            )
            .await;
            remove_plan_participant(swarm_id, &client_session_id, &swarm_plans).await;
            let was_coordinator = {
                let coordinators = swarm_coordinators.read().await;
                coordinators
                    .get(swarm_id)
                    .map(|id| id == &client_session_id)
                    .unwrap_or(false)
            };

            let mut new_coordinator: Option<String> = None;
            {
                let mut swarms = swarms_by_id.write().await;
                if let Some(swarm) = swarms.get_mut(swarm_id) {
                    swarm.remove(&client_session_id);
                    if swarm.is_empty() {
                        swarms.remove(swarm_id);
                    } else if was_coordinator {
                        new_coordinator = swarm.iter().min().cloned();
                    }
                }
            }

            if was_coordinator {
                let mut coordinators = swarm_coordinators.write().await;
                coordinators.remove(swarm_id);
                if let Some(new_id) = new_coordinator.clone() {
                    coordinators.insert(swarm_id.clone(), new_id.clone());
                    let mut plans = swarm_plans.write().await;
                    if let Some(vp) = plans.get_mut(swarm_id) {
                        vp.participants.insert(new_id.clone());
                    }
                }
            }

            if let Some(new_id) = new_coordinator {
                let members = swarm_members.read().await;
                if let Some(member) = members.get(&new_id) {
                    let msg = "You are now the coordinator for this swarm.".to_string();
                    let _ = member.event_tx.send(ServerEvent::Notification {
                        from_session: new_id.clone(),
                        from_name: member.friendly_name.clone(),
                        notification_type: NotificationType::Message {
                            scope: Some("swarm".to_string()),
                            channel: None,
                        },
                        message: msg.clone(),
                    });
                }
            }

            broadcast_swarm_status(swarm_id, &swarm_members, &swarms_by_id).await;
        }
    }

    // Clean up: remove client debug channel
    {
        let mut debug_state = client_debug_state.write().await;
        debug_state.unregister(&client_debug_id);
    }
    {
        let mut connections = client_connections.write().await;
        connections.remove(&client_connection_id);
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
    images: Vec<(String, String)>,
    event_tx: tokio::sync::mpsc::UnboundedSender<ServerEvent>,
) -> Result<()> {
    let mut agent = agent.lock().await;
    agent.run_once_streaming_mpsc(content, images, event_tx).await
}

async fn broadcast_swarm_status(
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let session_ids: Vec<String> = {
        let swarms = swarms_by_id.read().await;
        swarms
            .get(swarm_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    };
    if session_ids.is_empty() {
        return;
    }

    let members_guard = swarm_members.read().await;
    let members_list: Vec<crate::protocol::SwarmMemberStatus> = session_ids
        .iter()
        .filter_map(|sid| {
            members_guard
                .get(sid)
                .map(|m| crate::protocol::SwarmMemberStatus {
                    session_id: m.session_id.clone(),
                    friendly_name: m.friendly_name.clone(),
                    status: m.status.clone(),
                    detail: m.detail.clone(),
                    role: Some(m.role.clone()),
                })
        })
        .collect();

    let event = ServerEvent::SwarmStatus {
        members: members_list,
    };
    for sid in session_ids {
        if let Some(member) = members_guard.get(&sid) {
            let _ = member.event_tx.send(event.clone());
        }
    }
}

/// Broadcast the authoritative swarm plan snapshot.
///
/// Plan snapshots are sent to explicit plan participants. If a plan has no
/// participants yet, fall back to all current swarm members.
async fn broadcast_swarm_plan(
    swarm_id: &str,
    reason: Option<String>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let (version, items, mut participants): (u64, Vec<PlanItem>, Vec<String>) = {
        let plans = swarm_plans.read().await;
        let Some(vp) = plans.get(swarm_id) else {
            return;
        };
        let mut p: Vec<String> = vp.participants.iter().cloned().collect();
        p.sort();
        (vp.version, vp.items.clone(), p)
    };

    if participants.is_empty() {
        let swarms = swarms_by_id.read().await;
        participants = swarms
            .get(swarm_id)
            .map(|s| {
                let mut ids: Vec<String> = s.iter().cloned().collect();
                ids.sort();
                ids
            })
            .unwrap_or_default();
    }

    if participants.is_empty() {
        return;
    }

    let event = ServerEvent::SwarmPlan {
        swarm_id: swarm_id.to_string(),
        version,
        items,
        participants: participants.clone(),
        reason,
    };

    let members = swarm_members.read().await;
    for sid in participants {
        if let Some(member) = members.get(&sid) {
            let _ = member.event_tx.send(event.clone());
        }
    }
}

async fn rename_plan_participant(
    swarm_id: &str,
    old_session_id: &str,
    new_session_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    let mut plans = swarm_plans.write().await;
    if let Some(vp) = plans.get_mut(swarm_id) {
        if vp.participants.remove(old_session_id) {
            vp.participants.insert(new_session_id.to_string());
        }
        for item in &mut vp.items {
            if item.assigned_to.as_deref() == Some(old_session_id) {
                item.assigned_to = Some(new_session_id.to_string());
            }
        }
    }
}

async fn remove_plan_participant(
    swarm_id: &str,
    session_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    let mut plans = swarm_plans.write().await;
    if let Some(vp) = plans.get_mut(swarm_id) {
        vp.participants.remove(session_id);
    }
}

async fn remove_session_from_swarm(
    session_id: &str,
    swarm_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) {
    remove_plan_participant(swarm_id, session_id, swarm_plans).await;

    {
        let mut swarms = swarms_by_id.write().await;
        if let Some(swarm) = swarms.get_mut(swarm_id) {
            swarm.remove(session_id);
            if swarm.is_empty() {
                swarms.remove(swarm_id);
            }
        }
    }

    let was_coordinator = {
        let coordinators = swarm_coordinators.read().await;
        coordinators
            .get(swarm_id)
            .map(|id| id == session_id)
            .unwrap_or(false)
    };

    if was_coordinator {
        let new_coordinator = {
            let swarms = swarms_by_id.read().await;
            swarms.get(swarm_id).and_then(|s| s.iter().min().cloned())
        };

        {
            let mut coordinators = swarm_coordinators.write().await;
            coordinators.remove(swarm_id);
            if let Some(ref new_id) = new_coordinator {
                coordinators.insert(swarm_id.to_string(), new_id.clone());
            }
        }

        if let Some(new_id) = new_coordinator {
            {
                let mut members = swarm_members.write().await;
                if let Some(member) = members.get_mut(&new_id) {
                    member.role = "coordinator".to_string();
                }
            }
            let mut plans = swarm_plans.write().await;
            if let Some(vp) = plans.get_mut(swarm_id) {
                vp.participants.insert(new_id.clone());
            }
            let members = swarm_members.read().await;
            if let Some(member) = members.get(&new_id) {
                let _ = member.event_tx.send(ServerEvent::Notification {
                    from_session: new_id.clone(),
                    from_name: member.friendly_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("swarm".to_string()),
                        channel: None,
                    },
                    message: "You are now the coordinator for this swarm.".to_string(),
                });
            }
        }
    }

    {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(session_id) {
            member.role = "agent".to_string();
        }
    }

    broadcast_swarm_status(swarm_id, swarm_members, swarms_by_id).await;
}

async fn record_swarm_event(
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
    session_id: String,
    session_name: Option<String>,
    swarm_id: Option<String>,
    event: SwarmEventType,
) {
    let mut history = event_history.write().await;
    history.push(SwarmEvent {
        id: event_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        session_id,
        session_name,
        swarm_id,
        event,
        timestamp: Instant::now(),
        absolute_time: std::time::SystemTime::now(),
    });
    if history.len() > MAX_EVENT_HISTORY {
        history.remove(0);
    }
}

async fn record_swarm_event_for_session(
    session_id: &str,
    event: SwarmEventType,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    event_history: &Arc<RwLock<Vec<SwarmEvent>>>,
    event_counter: &Arc<std::sync::atomic::AtomicU64>,
) {
    let (session_name, swarm_id) = {
        let members = swarm_members.read().await;
        if let Some(member) = members.get(session_id) {
            (member.friendly_name.clone(), member.swarm_id.clone())
        } else {
            (None, None)
        }
    };
    record_swarm_event(
        event_history,
        event_counter,
        session_id.to_string(),
        session_name,
        swarm_id,
        event,
    )
    .await;
}

async fn update_member_status(
    session_id: &str,
    status: &str,
    detail: Option<String>,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: Option<&Arc<RwLock<Vec<SwarmEvent>>>>,
    event_counter: Option<&Arc<std::sync::atomic::AtomicU64>>,
) {
    let (swarm_id, agent_name, status_changed, old_status) = {
        let mut members = swarm_members.write().await;
        if let Some(member) = members.get_mut(session_id) {
            let previous_status = member.status.clone();
            let changed = member.status != status;
            if changed {
                member.last_status_change = Instant::now();
            }
            let name = member.friendly_name.clone();
            member.status = status.to_string();
            member.detail = detail;
            (member.swarm_id.clone(), name, changed, previous_status)
        } else {
            (None, None, false, String::new())
        }
    };
    if let Some(ref id) = swarm_id {
        if status_changed {
            if let (Some(history), Some(counter)) = (event_history, event_counter) {
                record_swarm_event(
                    history,
                    counter,
                    session_id.to_string(),
                    agent_name.clone(),
                    Some(id.clone()),
                    SwarmEventType::StatusChange {
                        old_status,
                        new_status: status.to_string(),
                    },
                )
                .await;
            }
        }

        broadcast_swarm_status(id, swarm_members, swarms_by_id).await;

        // Notify coordinator when an agent completes
        if status_changed && status == "completed" {
            let coordinator_id = {
                // We don't have swarm_coordinators here, so we check the members
                let members = swarm_members.read().await;
                members
                    .values()
                    .find(|m| {
                        m.swarm_id.as_deref() == Some(id)
                            && m.role == "coordinator"
                            && m.session_id != session_id
                    })
                    .map(|m| (m.session_id.clone(), m.event_tx.clone()))
            };
            if let Some((coord_id, coord_tx)) = coordinator_id {
                let name = agent_name
                    .as_deref()
                    .unwrap_or(&session_id[..8.min(session_id.len())]);
                let msg = format!(
                    "Agent {} completed their work. Use assign_task to give them new work, or stop to remove them.",
                    name
                );
                let _ = coord_tx.send(ServerEvent::Notification {
                    from_session: session_id.to_string(),
                    from_name: agent_name.clone(),
                    notification_type: NotificationType::Message {
                        scope: Some("swarm".to_string()),
                        channel: None,
                    },
                    message: msg,
                });
            }
        }
    }
}

fn truncate_detail(text: &str, max_len: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    let max_len = max_len.max(1);
    if trimmed.chars().count() <= max_len {
        return trimmed.to_string();
    }
    if max_len <= 3 {
        return trimmed.chars().take(max_len).collect();
    }
    let mut out: String = trimmed.chars().take(max_len - 3).collect();
    out.push_str("...");
    out
}

fn summarize_plan_items(items: &[PlanItem], max_items: usize) -> String {
    if items.is_empty() {
        return "no items".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    for item in items.iter().take(max_items.max(1)) {
        parts.push(item.content.clone());
    }
    let mut summary = parts.join("; ");
    if items.len() > max_items.max(1) {
        summary.push_str(&format!(" (+{} more)", items.len() - max_items.max(1)));
    }
    summary
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
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
    _swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
    model_override: Option<String>,
) -> Result<String> {
    let memory_enabled = crate::config::config().features.memory;
    let swarm_enabled = crate::config::config().features.swarm;

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

    // Enable test mode for memory tools (isolated storage for debug sessions)
    registry.enable_memory_test_mode().await;

    // Check if this should be a selfdev session BEFORE creating agent
    // (registry is moved into agent, so we need to register tools first)
    let should_selfdev = is_selfdev_env()
        || working_dir
            .as_ref()
            .map(|d| crate::build::is_jcode_repo(d) || is_jcode_repo_or_parent(d))
            .unwrap_or(false);

    if should_selfdev {
        registry.register_selfdev_tools().await;
    }

    // Register MCP tools for headless sessions (no event channel)
    registry.register_mcp_tools(None, None, None).await;

    // Create a new agent
    let mut new_agent = Agent::new(Arc::clone(&provider), registry);
    new_agent.set_memory_enabled(memory_enabled);
    let client_session_id = new_agent.session_id().to_string();

    if let Some(model) = model_override {
        if let Err(e) = new_agent.set_model(&model) {
            crate::logging::warn(&format!(
                "Failed to set headless session model override '{}': {}",
                model, e
            ));
        }
    }

    // Apply working dir for headless sessions (if provided)
    if let Some(ref dir) = working_dir {
        if let Some(path) = dir.to_str() {
            new_agent.set_working_dir(path);
        }
    }

    // Mark as debug/test session (created via debug socket)
    new_agent.set_debug(true);

    if let Some(ref dir) = working_dir {
        if let Some(dir_str) = dir.to_str() {
            new_agent.set_working_dir(dir_str);
        } else {
            new_agent.set_working_dir(&dir.display().to_string());
        }
    }

    // Enable self-dev mode if determined above
    if should_selfdev {
        new_agent.set_canary("self-dev");
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

    // Calculate swarm_id and register as swarm member
    let swarm_id = if swarm_enabled {
        swarm_id_for_dir(working_dir.clone())
    } else {
        None
    };
    let friendly_name = crate::id::extract_session_name(&client_session_id)
        .map(|s| s.to_string())
        .unwrap_or_else(|| client_session_id[..8.min(client_session_id.len())].to_string());

    // Create an event channel for this headless session.
    // Spawn a drain task so sends succeed (headless sessions have no TUI consumer).
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<ServerEvent>();
    tokio::spawn(async move {
        while event_rx.recv().await.is_some() {
            // Drain events to keep channel alive
        }
    });

    // Register as swarm member
    {
        let now = Instant::now();
        let mut members = swarm_members.write().await;
        members.insert(
            client_session_id.clone(),
            SwarmMember {
                session_id: client_session_id.clone(),
                event_tx: event_tx.clone(),
                working_dir: working_dir.clone(),
                swarm_id: swarm_id.clone(),
                swarm_enabled,
                status: "ready".to_string(),
                detail: None,
                friendly_name: Some(friendly_name.clone()),
                role: "agent".to_string(),
                joined_at: now,
                last_status_change: now,
            },
        );
    }

    // Add to swarm by swarm_id (separate scope to avoid holding swarm_members lock)
    if let Some(ref id) = swarm_id {
        let mut swarms = swarms_by_id.write().await;
        swarms
            .entry(id.clone())
            .or_insert_with(HashSet::new)
            .insert(client_session_id.clone());
    }

    // Set up coordinator if needed
    let mut is_new_coordinator = false;
    if let Some(ref id) = swarm_id {
        let mut coordinators = swarm_coordinators.write().await;
        if coordinators.get(id).is_none() {
            coordinators.insert(id.clone(), client_session_id.clone());
            is_new_coordinator = true;
        }
    }
    // Update role separately to avoid nested lock
    if is_new_coordinator {
        let mut members = swarm_members.write().await;
        if let Some(m) = members.get_mut(&client_session_id) {
            m.role = "coordinator".to_string();
        }
    }

    // Broadcast status to the swarm; plan attachment is explicit via comm_resync_plan.
    if let Some(ref id) = swarm_id {
        broadcast_swarm_status(id, swarm_members, swarms_by_id).await;
    }

    Ok(serde_json::json!({
        "session_id": client_session_id,
        "working_dir": working_dir,
        "swarm_id": swarm_id,
        "friendly_name": friendly_name,
    })
    .to_string())
}

async fn run_swarm_task(
    agent: Arc<Mutex<Agent>>,
    description: &str,
    subagent_type: &str,
    prompt: &str,
) -> Result<String> {
    let (provider, registry, session_id, working_dir, coordinator_model) = {
        let agent = agent.lock().await;
        (
            agent.provider_fork(),
            agent.registry(),
            agent.session_id().to_string(),
            agent.working_dir().map(PathBuf::from),
            agent.provider_model(),
        )
    };
    let mut session = Session::create(
        Some(session_id),
        Some(format!("{} (@{} swarm)", description, subagent_type)),
    );
    session.model = Some(coordinator_model);
    if let Some(dir) = working_dir {
        session.working_dir = Some(dir.display().to_string());
    }
    session.save()?;

    let mut allowed: HashSet<String> = registry.tool_names().await.into_iter().collect();
    for blocked in ["subagent", "task", "todowrite", "todoread"] {
        allowed.remove(blocked);
    }

    let mut worker = Agent::new_with_session(provider, registry, session, Some(allowed));
    let output = worker.run_once_capture(prompt).await?;
    Ok(output)
}

async fn run_swarm_message(agent: Arc<Mutex<Agent>>, message: &str) -> Result<String> {
    let working_dir = {
        let agent = agent.lock().await;
        agent.working_dir().map(|dir| dir.to_string())
    };
    let working_dir_hint = working_dir
        .as_deref()
        .map(|dir| format!("Working directory: {}\n", dir))
        .unwrap_or_default();

    let planner_prompt = format!(
        "{working_dir_hint}You are a task planner. Break the request into 2-4 subtasks. \
Return ONLY a JSON array of objects with keys: description, prompt, subagent_type. \
No extra text.\n\nRequest:\n{message}"
    );

    let plan_text = {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&planner_prompt).await?
    };

    let mut tasks = parse_swarm_tasks(&plan_text);
    if tasks.is_empty() {
        tasks.push(SwarmTaskSpec {
            description: "Main task".to_string(),
            prompt: message.to_string(),
            subagent_type: Some("general".to_string()),
        });
    }

    let task_futures = tasks.iter().map(|task| {
        let agent = agent.clone();
        let working_dir_hint = working_dir_hint.clone();
        let description = task.description.clone();
        let prompt = format!("{working_dir_hint}{}", task.prompt);
        let subagent_type = task
            .subagent_type
            .clone()
            .unwrap_or_else(|| "general".to_string());
        async move {
            let output = run_swarm_task(agent, &description, &subagent_type, &prompt).await?;
            Ok::<(String, String), anyhow::Error>((description, output))
        }
    });
    let task_outputs = try_join_all(task_futures).await?;

    let mut integration_prompt = String::new();
    integration_prompt.push_str(
        "You are the coordinator. Complete the original request using the subagent outputs below. ",
    );
    integration_prompt.push_str("Do not stop early; run any requested tests and fix failures.\n\n");
    integration_prompt.push_str("Original request:\n");
    integration_prompt.push_str(message);
    integration_prompt.push_str("\n\nSubagent outputs:\n");
    for (desc, output) in &task_outputs {
        integration_prompt.push_str(&format!("\n--- {} ---\n{}\n", desc, output));
    }
    integration_prompt.push_str("\nNow complete the task.\n");

    let final_output = {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&integration_prompt).await?
    };

    Ok(final_output)
}

#[derive(Debug, Deserialize)]
struct SwarmTaskSpec {
    description: String,
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
}

fn parse_swarm_tasks(text: &str) -> Vec<SwarmTaskSpec> {
    if let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(text) {
        return tasks;
    }

    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']')) {
        if start < end {
            if let Ok(tasks) = serde_json::from_str::<Vec<SwarmTaskSpec>>(&text[start..=end]) {
                return tasks;
            }
        }
    }

    Vec::new()
}

fn debug_message_timeout_secs() -> Option<u64> {
    let raw = std::env::var("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let secs = trimmed.parse::<u64>().ok()?;
    if secs == 0 {
        None
    } else {
        Some(secs)
    }
}

async fn run_debug_message_with_timeout(
    agent: Arc<Mutex<Agent>>,
    msg: &str,
    timeout_secs: u64,
) -> Result<String> {
    let msg = msg.to_string();
    let mut handle = tokio::spawn(async move {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&msg).await
    });

    tokio::select! {
        join_result = &mut handle => {
            match join_result {
                Ok(result) => result,
                Err(e) => Err(anyhow::anyhow!("debug message task failed: {}", e)),
            }
        }
        _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            handle.abort();
            Err(anyhow::anyhow!(
                "debug message timed out after {}s",
                timeout_secs
            ))
        }
    }
}

async fn execute_debug_command(
    agent: Arc<Mutex<Agent>>,
    command: &str,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
) -> Result<String> {
    let trimmed = command.trim();

    if trimmed.starts_with("swarm_message_async:") {
        let msg = trimmed
            .strip_prefix("swarm_message_async:")
            .unwrap_or("")
            .trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("swarm_message_async: requires content"));
        }
        let session = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let job_id = id::new_id("job");
        {
            let mut jobs = debug_jobs.write().await;
            jobs.insert(
                job_id.clone(),
                DebugJob {
                    id: job_id.clone(),
                    status: DebugJobStatus::Queued,
                    command: format!("swarm_message:{}", msg),
                    session_id: Some(session),
                    created_at: Instant::now(),
                    started_at: None,
                    finished_at: None,
                    output: None,
                    error: None,
                },
            );
        }

        let jobs = Arc::clone(&debug_jobs);
        let agent = Arc::clone(&agent);
        let msg = msg.to_string();
        let job_id_inner = job_id.clone();
        tokio::spawn(async move {
            {
                let mut jobs = jobs.write().await;
                if let Some(job) = jobs.get_mut(&job_id_inner) {
                    job.status = DebugJobStatus::Running;
                    job.started_at = Some(Instant::now());
                }
            }

            let result = run_swarm_message(agent.clone(), &msg).await;
            let partial_output = if result.is_err() {
                let agent = agent.lock().await;
                agent.last_assistant_text()
            } else {
                None
            };

            let mut jobs = jobs.write().await;
            if let Some(job) = jobs.get_mut(&job_id_inner) {
                job.finished_at = Some(Instant::now());
                match result {
                    Ok(output) => {
                        job.status = DebugJobStatus::Completed;
                        job.output = Some(output);
                    }
                    Err(e) => {
                        job.status = DebugJobStatus::Failed;
                        job.error = Some(e.to_string());
                        if let Some(output) = partial_output {
                            job.output = Some(output);
                        }
                    }
                }
            }
        });

        return Ok(serde_json::json!({ "job_id": job_id }).to_string());
    }

    if trimmed.starts_with("message_async:") {
        let msg = trimmed.strip_prefix("message_async:").unwrap_or("").trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("message_async: requires content"));
        }
        let session = {
            let agent = agent.lock().await;
            agent.session_id().to_string()
        };
        let job_id = id::new_id("job");
        {
            let mut jobs = debug_jobs.write().await;
            jobs.insert(
                job_id.clone(),
                DebugJob {
                    id: job_id.clone(),
                    status: DebugJobStatus::Queued,
                    command: format!("message:{}", msg),
                    session_id: Some(session),
                    created_at: Instant::now(),
                    started_at: None,
                    finished_at: None,
                    output: None,
                    error: None,
                },
            );
        }

        let jobs = Arc::clone(&debug_jobs);
        let agent = Arc::clone(&agent);
        let msg = msg.to_string();
        let job_id_inner = job_id.clone();
        tokio::spawn(async move {
            {
                let mut jobs = jobs.write().await;
                if let Some(job) = jobs.get_mut(&job_id_inner) {
                    job.status = DebugJobStatus::Running;
                    job.started_at = Some(Instant::now());
                }
            }

            let result = {
                let mut agent = agent.lock().await;
                agent.run_once_capture(&msg).await
            };
            let partial_output = if result.is_err() {
                let agent = agent.lock().await;
                agent.last_assistant_text()
            } else {
                None
            };

            let mut jobs = jobs.write().await;
            if let Some(job) = jobs.get_mut(&job_id_inner) {
                job.finished_at = Some(Instant::now());
                match result {
                    Ok(output) => {
                        job.status = DebugJobStatus::Completed;
                        job.output = Some(output);
                    }
                    Err(e) => {
                        job.status = DebugJobStatus::Failed;
                        job.error = Some(e.to_string());
                        if let Some(output) = partial_output {
                            job.output = Some(output);
                        }
                    }
                }
            }
        });

        return Ok(serde_json::json!({ "job_id": job_id }).to_string());
    }

    if trimmed.starts_with("swarm_message:") {
        let msg = trimmed.strip_prefix("swarm_message:").unwrap_or("").trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("swarm_message: requires content"));
        }

        let final_text = run_swarm_message(agent.clone(), msg).await?;
        return Ok(final_text);
    }

    if trimmed.starts_with("message:") {
        let msg = trimmed.strip_prefix("message:").unwrap_or("").trim();
        if let Some(timeout_secs) = debug_message_timeout_secs() {
            return run_debug_message_with_timeout(agent, msg, timeout_secs).await;
        }
        let mut agent = agent.lock().await;
        let output = agent.run_once_capture(msg).await?;
        return Ok(output);
    }

    // queue_interrupt:<content> - Queue soft interrupt (for testing)
    // This adds a message to the agent's soft interrupt queue without blocking
    if trimmed.starts_with("queue_interrupt:") {
        let content = trimmed
            .strip_prefix("queue_interrupt:")
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(anyhow::anyhow!("queue_interrupt: requires content"));
        }
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(content.to_string(), false);
        return Ok("queued".to_string());
    }

    // queue_interrupt_urgent:<content> - Queue urgent soft interrupt (can skip tools)
    if trimmed.starts_with("queue_interrupt_urgent:") {
        let content = trimmed
            .strip_prefix("queue_interrupt_urgent:")
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(anyhow::anyhow!("queue_interrupt_urgent: requires content"));
        }
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(content.to_string(), true);
        return Ok("queued (urgent)".to_string());
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

    if trimmed == "tools:full" {
        let agent = agent.lock().await;
        let definitions = agent.tool_definitions_for_debug().await;
        return Ok(serde_json::to_string_pretty(&definitions).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "mcp" || trimmed == "mcp:servers" {
        let agent = agent.lock().await;
        let tool_names = agent.tool_names().await;
        let mut connected: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for name in tool_names {
            if let Some(rest) = name.strip_prefix("mcp__") {
                let mut parts = rest.splitn(2, "__");
                if let (Some(server), Some(tool)) = (parts.next(), parts.next()) {
                    connected
                        .entry(server.to_string())
                        .or_default()
                        .push(tool.to_string());
                }
            }
        }
        for tools in connected.values_mut() {
            tools.sort();
        }
        let connected_servers: Vec<String> = connected.keys().cloned().collect();

        // Load merged MCP config (handles ~/.jcode/mcp.json + project-local configs)
        let config = McpConfig::load();
        let config_path = if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join("mcp.json");
            if path.exists() {
                Some(path.to_string_lossy().to_string())
            } else {
                None
            }
        } else {
            None
        };
        let mut configured_servers: Vec<String> = config.servers.keys().cloned().collect();
        configured_servers.sort();

        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "config_path": config_path,
            "configured_servers": configured_servers,
            "connected_servers": connected_servers,
            "connected_tools": connected,
        }))
        .unwrap_or_else(|_| "{}".to_string()));
    }

    // mcp:tools - list all registered MCP tools
    if trimmed == "mcp:tools" {
        let agent = agent.lock().await;
        let tool_names = agent.tool_names().await;
        let mcp_tools: Vec<&str> = tool_names
            .iter()
            .filter(|n| n.starts_with("mcp__"))
            .map(|n| n.as_str())
            .collect();
        return Ok(serde_json::to_string_pretty(&mcp_tools).unwrap_or_else(|_| "[]".to_string()));
    }

    // mcp:connect:<server> <json> - connect to an MCP server
    if let Some(rest) = trimmed.strip_prefix("mcp:connect:") {
        let (server_name, config_json) = match rest.find(' ') {
            Some(idx) => (rest[..idx].trim(), &rest[idx + 1..]),
            None => {
                return Err(anyhow::anyhow!(
                    "Usage: mcp:connect:<server> {{\"command\":\"...\",\"args\":[...]}}"
                ))
            }
        };
        let mut input: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;
        input["action"] = serde_json::json!("connect");
        input["server"] = serde_json::json!(server_name);
        let agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        return Ok(result.output);
    }

    // mcp:disconnect:<server> - disconnect from an MCP server
    if let Some(server_name) = trimmed.strip_prefix("mcp:disconnect:") {
        let server_name = server_name.trim();
        let input = serde_json::json!({"action": "disconnect", "server": server_name});
        let agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        return Ok(result.output);
    }

    // mcp:reload - reload MCP config and reconnect
    if trimmed == "mcp:reload" {
        let input = serde_json::json!({"action": "reload"});
        let mut agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        // Unlock tool list so next request picks up new MCP tools
        agent.unlock_tools();
        return Ok(result.output);
    }

    // mcp:call:<server>:<tool> <json> - call an MCP tool directly
    if let Some(rest) = trimmed.strip_prefix("mcp:call:") {
        let (tool_path, args_json) = match rest.find(' ') {
            Some(idx) => (rest[..idx].trim(), rest[idx + 1..].trim()),
            None => (rest.trim(), "{}"),
        };
        let mut parts = tool_path.splitn(2, ':');
        let server = parts.next().unwrap_or("");
        let tool = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Usage: mcp:call:<server>:<tool> <json>"))?;
        let tool_name = format!("mcp__{}__{}", server, tool);
        let input: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;
        let agent = agent.lock().await;
        let result = agent.execute_tool(&tool_name, input).await?;
        return Ok(result.output);
    }

    if trimmed == "cancel" {
        // Queue an urgent interrupt to cancel in-flight generation
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(
            "[CANCELLED] Generation cancelled via debug socket".to_string(),
            true,
        );
        return Ok(serde_json::json!({
            "status": "cancel_queued",
            "message": "Urgent interrupt queued - will cancel at next tool boundary"
        })
        .to_string());
    }

    if trimmed == "clear" || trimmed == "clear_history" {
        // Clear conversation history
        let mut agent = agent.lock().await;
        agent.clear();
        return Ok(serde_json::json!({
            "status": "cleared",
            "message": "Conversation history cleared"
        })
        .to_string());
    }

    if trimmed == "agent:info" {
        // Get comprehensive agent internal state
        let agent = agent.lock().await;
        let info = agent.debug_info();
        return Ok(serde_json::to_string_pretty(&info).unwrap_or_else(|_| "{}".to_string()));
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
            "upstream_provider": agent.last_upstream_provider(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "usage" {
        let agent = agent.lock().await;
        let usage = agent.last_usage();
        return Ok(serde_json::to_string_pretty(&usage).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "help" {
        return Ok(
            "debug commands: state, usage, history, tools, tools:full, mcp:servers, mcp:tools, mcp:connect:<server> <json>, mcp:disconnect:<server>, mcp:reload, mcp:call:<server>:<tool> <json>, last_response, message:<text>, message_async:<text>, swarm_message:<text>, swarm_message_async:<text>, tool:<name> <json>, queue_interrupt:<content>, queue_interrupt_urgent:<content>, jobs, job_status:<id>, job_wait:<id>, sessions, create_session, create_session:<path>, set_model:<model>, set_provider:<name>, trigger_extraction, available_models, reload, help".to_string()
        );
    }

    // set_model:<model> - Switch to a different model (may change provider)
    if trimmed.starts_with("set_model:") {
        let model = trimmed.strip_prefix("set_model:").unwrap_or("").trim();
        if model.is_empty() {
            return Err(anyhow::anyhow!("set_model: requires a model name"));
        }
        let mut agent = agent.lock().await;
        agent.set_model(model)?;
        let payload = serde_json::json!({
            "model": agent.provider_model(),
            "provider": agent.provider_name(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    // set_provider:<name> - Switch to a provider with default model
    if trimmed.starts_with("set_provider:") {
        let provider = trimmed
            .strip_prefix("set_provider:")
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let default_model = match provider.as_str() {
            "claude" | "anthropic" => "claude-opus-4-5-20251101",
            "openai" | "codex" => "gpt-5.3-codex-spark",
            "openrouter" => "anthropic/claude-sonnet-4",
            "cursor" => "gpt-5",
            "copilot" => "claude-sonnet-4",
            "antigravity" => "default",
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown provider '{}'. Use: claude, openai, openrouter, cursor, copilot, antigravity",
                    provider
                ))
            }
        };
        let mut agent = agent.lock().await;
        agent.set_model(default_model)?;
        let payload = serde_json::json!({
            "model": agent.provider_model(),
            "provider": agent.provider_name(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    // trigger_extraction - Force end-of-session memory extraction
    if trimmed == "trigger_extraction" {
        let agent = agent.lock().await;
        let count = agent.extract_session_memories().await;
        let payload = serde_json::json!({
            "extracted": count,
            "message_count": agent.message_count(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    // available_models - List all available models
    if trimmed == "available_models" {
        let agent = agent.lock().await;
        let models = agent.available_models_display();
        return Ok(serde_json::to_string_pretty(&models).unwrap_or_else(|_| "[]".to_string()));
    }

    // reload - Trigger server reload with current binary (direct signal, bypasses tool system)
    if trimmed == "reload" {
        // Get repo directory and check for binary
        let repo_dir = crate::build::get_repo_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find jcode repository directory"))?;

        let target_binary = repo_dir.join("target/release/jcode");
        if !target_binary.exists() {
            return Err(anyhow::anyhow!(
                "No binary found at target/release/jcode. Run 'cargo build --release' first."
            ));
        }

        let hash = crate::build::current_git_hash(&repo_dir)?;

        // Install version and update canary symlink
        crate::build::install_version(&repo_dir, &hash)?;
        crate::build::update_canary_symlink(&hash)?;

        // Update manifest
        let mut manifest = crate::build::BuildManifest::load()?;
        manifest.canary = Some(hash.clone());
        manifest.canary_status = Some(crate::build::CanaryStatus::Testing);
        manifest.save()?;

        // Write reload info for post-restart display
        let jcode_dir = crate::storage::jcode_dir()?;
        let info_path = jcode_dir.join("reload-info");
        std::fs::write(&info_path, format!("reload:{}", hash))?;

        // Write signal file to trigger server restart
        let signal_path = jcode_dir.join("rebuild-signal");
        std::fs::write(&signal_path, &hash)?;

        return Ok(format!(
            "Reload signal written for build {}. Server will restart.",
            hash
        ));
    }

    Err(anyhow::anyhow!("Unknown debug command '{}'", trimmed))
}

/// Execute a client debug command (visual debug, TUI state, etc.)
/// These commands access the TUI's visual debug module which uses global state.
fn execute_client_debug_command(command: &str) -> String {
    use crate::tui::{markdown, mermaid, visual_debug};

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
        let overlay = visual_debug::overlay_enabled();
        return serde_json::json!({
            "visual_debug_enabled": enabled,
            "visual_debug_overlay": overlay,
        })
        .to_string();
    }

    if trimmed == "overlay" || trimmed == "overlay:status" {
        let overlay = visual_debug::overlay_enabled();
        return serde_json::json!({
            "visual_debug_overlay": overlay,
        })
        .to_string();
    }

    if trimmed == "overlay:on" || trimmed == "overlay:enable" {
        visual_debug::set_overlay(true);
        return "Visual debug overlay enabled.".to_string();
    }

    if trimmed == "overlay:off" || trimmed == "overlay:disable" {
        visual_debug::set_overlay(false);
        return "Visual debug overlay disabled.".to_string();
    }

    if trimmed == "layout" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "layout: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "terminal_size": frame.terminal_size,
                    "layout": frame.layout,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            },
        );
    }

    if trimmed == "margins" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "margins: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "margins": frame.layout.margins,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            },
        );
    }

    if trimmed == "widgets" || trimmed == "info-widgets" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "widgets: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "info_widgets": frame.info_widgets,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            },
        );
    }

    if trimmed == "render-stats" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "render-stats: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&serde_json::json!({
                    "frame_id": frame.frame_id,
                    "render_timing": frame.render_timing,
                    "render_order": frame.render_order,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            },
        );
    }

    if trimmed == "render-order" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "render-order: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&frame.render_order)
                    .unwrap_or_else(|_| "[]".to_string())
            },
        );
    }

    if trimmed == "anomalies" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "anomalies: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&frame.anomalies).unwrap_or_else(|_| "[]".to_string())
            },
        );
    }

    if trimmed == "theme" {
        visual_debug::enable();
        return visual_debug::latest_frame().map_or_else(
            || "theme: no frames captured".to_string(),
            |frame| {
                serde_json::to_string_pretty(&frame.theme).unwrap_or_else(|_| "null".to_string())
            },
        );
    }

    if trimmed == "mermaid:stats" {
        let stats = mermaid::debug_stats();
        return serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "mermaid:memory" {
        let profile = mermaid::debug_memory_profile();
        return serde_json::to_string_pretty(&profile).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "mermaid:memory-bench" {
        let result = mermaid::debug_memory_benchmark(40);
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if let Some(raw_iterations) = trimmed.strip_prefix("mermaid:memory-bench ") {
        let iterations = match raw_iterations.trim().parse::<usize>() {
            Ok(v) => v,
            Err(_) => return "Invalid iterations (expected integer)".to_string(),
        };
        let result = mermaid::debug_memory_benchmark(iterations);
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "mermaid:cache" {
        let entries = mermaid::debug_cache();
        return serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string());
    }

    if trimmed == "mermaid:evict" || trimmed == "mermaid:clear-cache" {
        return match mermaid::clear_cache() {
            Ok(_) => "mermaid: cache cleared".to_string(),
            Err(e) => format!("mermaid: cache clear failed: {}", e),
        };
    }

    if trimmed == "mermaid:state" {
        let state = mermaid::debug_image_state();
        return serde_json::to_string_pretty(&state).unwrap_or_else(|_| "[]".to_string());
    }

    if trimmed == "mermaid:test" {
        let result = mermaid::debug_test_render();
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "mermaid:scroll" {
        let result = mermaid::debug_test_scroll(None);
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if let Some(content) = trimmed.strip_prefix("mermaid:render ") {
        let result = mermaid::debug_render(content);
        return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
    }

    if let Some(hash_str) = trimmed.strip_prefix("mermaid:stability ") {
        if let Ok(hash) = u64::from_str_radix(hash_str, 16) {
            let result = mermaid::debug_test_resize_stability(hash);
            return serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
        }
        return "Invalid hash (expected hex)".to_string();
    }

    if trimmed == "mermaid:active" {
        let diagrams = mermaid::get_active_diagrams();
        let info: Vec<serde_json::Value> = diagrams
            .iter()
            .map(|d| {
                serde_json::json!({
                    "hash": format!("{:016x}", d.hash),
                    "width": d.width,
                    "height": d.height,
                    "label": d.label,
                })
            })
            .collect();
        return serde_json::to_string_pretty(&serde_json::json!({
            "count": diagrams.len(),
            "diagrams": info,
        }))
        .unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "markdown:stats" {
        let stats = markdown::debug_stats();
        return serde_json::to_string_pretty(&stats).unwrap_or_else(|_| "{}".to_string());
    }

    if trimmed == "help" {
        return r#"Client debug commands:
  frame / screen-json      - Get latest visual debug frame (JSON)
  frame-normalized         - Get normalized frame (for diffs)
  screen                   - Dump visual debug frames to file
  layout                   - Get latest layout JSON
  margins                  - Get layout margins JSON
  widgets                  - Get info widget summary/placements
  render-stats             - Get render timing + order JSON
  render-order             - Get render order list
  anomalies                - Get latest visual debug anomalies
  theme                    - Get palette snapshot
  mermaid:stats            - Get mermaid render/cache stats
  mermaid:memory           - Mermaid memory profile (RSS + cache estimates)
  mermaid:memory-bench [n] - Run synthetic Mermaid memory benchmark
  mermaid:cache            - List mermaid cache entries
  mermaid:state            - Get image state (resize modes, areas)
  mermaid:test             - Render test diagram, return results
  mermaid:scroll           - Run scroll simulation test
  mermaid:render <content> - Render arbitrary mermaid content
  mermaid:stability <hash> - Test resize mode stability for hash
  mermaid:active           - List active diagrams (for pinned widget)
  mermaid:evict            - Clear mermaid cache
  markdown:stats           - Get markdown render stats
  overlay:on/off/status    - Toggle overlay boxes
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
    client_connections: Arc<RwLock<HashMap<String, ClientConnectionInfo>>>,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    file_touches: Arc<RwLock<HashMap<PathBuf, Vec<FileAccess>>>>,
    channel_subscriptions: Arc<RwLock<HashMap<String, HashMap<String, HashSet<String>>>>>,
    client_debug_state: Arc<RwLock<ClientDebugState>>,
    client_debug_response_tx: broadcast::Sender<(u64, String)>,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    event_history: Arc<RwLock<Vec<SwarmEvent>>>,
    server_identity: ServerIdentity,
    server_start_time: std::time::Instant,
    ambient_runner: Option<AmbientRunnerHandle>,
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
                                        if let Ok((resp_id, output)) = response_rx.recv().await {
                                            if resp_id == id {
                                                return Ok(output);
                                            }
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
                        if cmd == "jobs" {
                            let jobs_guard = debug_jobs.read().await;
                            let payload: Vec<Value> = jobs_guard
                                .values()
                                .map(|job| job.summary_payload())
                                .collect();
                            Ok(serde_json::to_string_pretty(&payload)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("job_status:") {
                            let job_id = cmd.strip_prefix("job_status:").unwrap_or("").trim();
                            if job_id.is_empty() {
                                Err(anyhow::anyhow!("job_status: requires a job id"))
                            } else {
                                let jobs_guard = debug_jobs.read().await;
                                if let Some(job) = jobs_guard.get(job_id) {
                                    Ok(serde_json::to_string_pretty(&job.status_payload())
                                        .unwrap_or_else(|_| "{}".to_string()))
                                } else {
                                    Err(anyhow::anyhow!("Unknown job id '{}'", job_id))
                                }
                            }
                        } else if cmd.starts_with("job_cancel:") {
                            // Cancel a running job
                            let job_id = cmd.strip_prefix("job_cancel:").unwrap_or("").trim();
                            if job_id.is_empty() {
                                Err(anyhow::anyhow!("job_cancel: requires a job id"))
                            } else {
                                let mut jobs_guard = debug_jobs.write().await;
                                if let Some(job) = jobs_guard.get_mut(job_id) {
                                    if matches!(
                                        job.status,
                                        DebugJobStatus::Running | DebugJobStatus::Queued
                                    ) {
                                        job.status = DebugJobStatus::Failed;
                                        job.output = Some("[CANCELLED]".to_string());
                                        Ok(serde_json::json!({
                                            "status": "cancelled",
                                            "job_id": job_id,
                                        })
                                        .to_string())
                                    } else {
                                        Err(anyhow::anyhow!("Job '{}' is not running", job_id))
                                    }
                                } else {
                                    Err(anyhow::anyhow!("Unknown job id '{}'", job_id))
                                }
                            }
                        } else if cmd == "jobs:purge" {
                            // Remove all completed/failed jobs
                            let mut jobs_guard = debug_jobs.write().await;
                            let before = jobs_guard.len();
                            jobs_guard.retain(|_, job| {
                                matches!(
                                    job.status,
                                    DebugJobStatus::Running | DebugJobStatus::Queued
                                )
                            });
                            let removed = before - jobs_guard.len();
                            Ok(serde_json::json!({
                                "status": "purged",
                                "removed": removed,
                                "remaining": jobs_guard.len(),
                            })
                            .to_string())
                        } else if cmd.starts_with("jobs:session:") {
                            // List jobs for a specific session
                            let sess_id = cmd.strip_prefix("jobs:session:").unwrap_or("").trim();
                            let jobs_guard = debug_jobs.read().await;
                            let payload: Vec<Value> = jobs_guard
                                .values()
                                .filter(|job| job.session_id.as_deref() == Some(sess_id))
                                .map(|job| job.summary_payload())
                                .collect();
                            Ok(serde_json::to_string_pretty(&payload)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("job_wait:") {
                            let job_id = cmd.strip_prefix("job_wait:").unwrap_or("").trim();
                            if job_id.is_empty() {
                                Err(anyhow::anyhow!("job_wait: requires a job id"))
                            } else {
                                let timeout = Duration::from_secs(900);
                                let start = Instant::now();
                                loop {
                                    {
                                        let jobs_guard = debug_jobs.read().await;
                                        if let Some(job) = jobs_guard.get(job_id) {
                                            if matches!(
                                                job.status,
                                                DebugJobStatus::Completed | DebugJobStatus::Failed
                                            ) {
                                                break Ok(serde_json::to_string_pretty(
                                                    &job.status_payload(),
                                                )
                                                .unwrap_or_else(|_| "{}".to_string()));
                                            }
                                        } else {
                                            break Err(anyhow::anyhow!(
                                                "Unknown job id '{}'",
                                                job_id
                                            ));
                                        }
                                    }
                                    if start.elapsed() > timeout {
                                        break Err(anyhow::anyhow!(
                                            "Timeout waiting for job '{}'",
                                            job_id
                                        ));
                                    }
                                    tokio::time::sleep(Duration::from_millis(500)).await;
                                }
                            }
                        } else if cmd == "create_session" || cmd.starts_with("create_session:") {
                            create_headless_session(
                                &sessions,
                                &session_id,
                                &provider,
                                cmd,
                                &swarm_members,
                                &swarms_by_id,
                                &swarm_coordinators,
                                &swarm_plans,
                                None,
                            )
                            .await
                        } else if cmd.starts_with("destroy_session:") {
                            let target_id =
                                cmd.strip_prefix("destroy_session:").unwrap_or("").trim();
                            if target_id.is_empty() {
                                Err(anyhow::anyhow!("destroy_session: requires a session_id"))
                            } else {
                                // Remove session first, extract transcript for final memory extraction
                                let removed_agent = {
                                    let mut sessions_guard = sessions.write().await;
                                    sessions_guard.remove(target_id)
                                };
                                if let Some(ref agent_arc) = removed_agent {
                                    let agent = agent_arc.lock().await;
                                    let memory_enabled = agent.memory_enabled();
                                    let transcript = if memory_enabled {
                                        Some(agent.build_transcript_for_extraction())
                                    } else {
                                        None
                                    };
                                    let sid = target_id.to_string();
                                    drop(agent);
                                    if let Some(transcript) = transcript {
                                        crate::memory_agent::trigger_final_extraction(
                                            transcript, sid,
                                        );
                                    }
                                }
                                let removed = removed_agent.is_some();
                                if removed {
                                    // Clean up swarm membership
                                    let swarm_id = {
                                        let mut members = swarm_members.write().await;
                                        members.remove(target_id).and_then(|m| m.swarm_id)
                                    };
                                    if let Some(ref id) = swarm_id {
                                        // Remove from swarm (scoped to drop write guard)
                                        {
                                            let mut swarms = swarms_by_id.write().await;
                                            if let Some(swarm) = swarms.get_mut(id) {
                                                swarm.remove(target_id);
                                                if swarm.is_empty() {
                                                    swarms.remove(id);
                                                }
                                            }
                                        }
                                        // Handle coordinator change if needed
                                        let was_coordinator = {
                                            let coordinators = swarm_coordinators.read().await;
                                            coordinators
                                                .get(id)
                                                .map(|c| c == target_id)
                                                .unwrap_or(false)
                                        };
                                        if was_coordinator {
                                            let new_coordinator = {
                                                let swarms = swarms_by_id.read().await;
                                                swarms.get(id).and_then(|s| s.iter().min().cloned())
                                            };
                                            let mut coordinators = swarm_coordinators.write().await;
                                            coordinators.remove(id);
                                            if let Some(new_id) = new_coordinator {
                                                coordinators.insert(id.clone(), new_id);
                                            }
                                        }
                                        broadcast_swarm_status(id, &swarm_members, &swarms_by_id)
                                            .await;
                                    }
                                    Ok(format!("Session '{}' destroyed", target_id))
                                } else {
                                    Err(anyhow::anyhow!("Unknown session_id '{}'", target_id))
                                }
                            }
                        } else if cmd == "sessions" {
                            // Return full session metadata
                            let sessions_guard = sessions.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (sid, agent_arc) in sessions_guard.iter() {
                                let member_info = members.get(sid);
                                let (provider, model, is_processing, working_dir_str, token_usage): (
                                    Option<String>,
                                    Option<String>,
                                    bool,
                                    Option<String>,
                                    Option<serde_json::Value>,
                                ) = if let Ok(agent) = agent_arc.try_lock() {
                                    let usage = agent.last_usage();
                                    (
                                        Some(agent.provider_name()),
                                        Some(agent.provider_model()),
                                        false, // Not processing if we can lock
                                        agent.working_dir().map(|p| p.to_string()),
                                        Some(serde_json::json!({
                                            "input": usage.input_tokens,
                                            "output": usage.output_tokens,
                                            "cache_read": usage.cache_read_input_tokens,
                                            "cache_write": usage.cache_creation_input_tokens,
                                        })),
                                    )
                                } else {
                                    (None, None, true, None, None) // Processing if locked
                                };
                                let final_working_dir: Option<String> =
                                    working_dir_str.or_else(|| {
                                        member_info.and_then(|m| {
                                            m.working_dir
                                                .as_ref()
                                                .map(|p| p.to_string_lossy().to_string())
                                        })
                                    });
                                out.push(serde_json::json!({
                                    "session_id": sid,
                                    "friendly_name": member_info.and_then(|m| m.friendly_name.clone()),
                                    "provider": provider,
                                    "model": model,
                                    "is_processing": is_processing,
                                    "working_dir": final_working_dir,
                                    "swarm_id": member_info.and_then(|m| m.swarm_id.clone()),
                                    "status": member_info.map(|m| m.status.clone()),
                                    "detail": member_info.and_then(|m| m.detail.clone()),
                                    "token_usage": token_usage,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm" || cmd == "swarm_status" || cmd == "swarm:members"
                        {
                            // List all swarm members with full details
                            let members = swarm_members.read().await;
                            let sessions_guard = sessions.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for member in members.values() {
                                // Get provider/model from the agent if session exists
                                let (provider, model) = if let Some(agent_arc) =
                                    sessions_guard.get(&member.session_id)
                                {
                                    if let Ok(agent) = agent_arc.try_lock() {
                                        (Some(agent.provider_name()), Some(agent.provider_model()))
                                    } else {
                                        (None, None)
                                    }
                                } else {
                                    (None, None)
                                };
                                out.push(serde_json::json!({
                                    "session_id": member.session_id,
                                    "friendly_name": member.friendly_name,
                                    "swarm_id": member.swarm_id,
                                    "working_dir": member.working_dir,
                                    "status": member.status,
                                    "detail": member.detail,
                                    "joined_secs_ago": member.joined_at.elapsed().as_secs(),
                                    "status_changed_secs_ago": member.last_status_change.elapsed().as_secs(),
                                    "provider": provider,
                                    "model": model,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm:list" {
                            // List all swarm IDs with member counts
                            let swarms = swarms_by_id.read().await;
                            let coordinators = swarm_coordinators.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, session_ids) in swarms.iter() {
                                let coordinator = coordinators.get(swarm_id);
                                let coordinator_name = coordinator.and_then(|cid| {
                                    members.get(cid).and_then(|m| m.friendly_name.clone())
                                });
                                out.push(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "member_count": session_ids.len(),
                                    "members": session_ids.iter().collect::<Vec<_>>(),
                                    "coordinator": coordinator,
                                    "coordinator_name": coordinator_name,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm:coordinators" {
                            // List all coordinators
                            let coordinators = swarm_coordinators.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, session_id) in coordinators.iter() {
                                let name = members
                                    .get(session_id)
                                    .and_then(|m| m.friendly_name.clone());
                                out.push(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "coordinator_session": session_id,
                                    "coordinator_name": name,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:coordinator:") {
                            // Get coordinator for specific swarm
                            let swarm_id =
                                cmd.strip_prefix("swarm:coordinator:").unwrap_or("").trim();
                            let coordinators = swarm_coordinators.read().await;
                            let members = swarm_members.read().await;
                            if let Some(session_id) = coordinators.get(swarm_id) {
                                let name = members
                                    .get(session_id)
                                    .and_then(|m| m.friendly_name.clone());
                                Ok(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "coordinator_session": session_id,
                                    "coordinator_name": name,
                                })
                                .to_string())
                            } else {
                                Err(anyhow::anyhow!("No coordinator for swarm '{}'", swarm_id))
                            }
                        } else if cmd == "swarm:roles" {
                            // List all members with their roles
                            let members = swarm_members.read().await;
                            let coordinators = swarm_coordinators.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (sid, m) in members.iter() {
                                let is_coordinator = m
                                    .swarm_id
                                    .as_ref()
                                    .map(|swid| {
                                        coordinators.get(swid).map(|c| c == sid).unwrap_or(false)
                                    })
                                    .unwrap_or(false);
                                out.push(serde_json::json!({
                                    "session_id": sid,
                                    "friendly_name": m.friendly_name,
                                    "role": m.role,
                                    "swarm_id": m.swarm_id,
                                    "status": m.status,
                                    "is_coordinator": is_coordinator,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm:channels" {
                            // List channel subscriptions per swarm
                            let subs = channel_subscriptions.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, channels) in subs.iter() {
                                let mut channel_data: Vec<serde_json::Value> = Vec::new();
                                for (channel, session_ids) in channels.iter() {
                                    channel_data.push(serde_json::json!({
                                        "channel": channel,
                                        "subscribers": session_ids.iter().collect::<Vec<_>>(),
                                        "count": session_ids.len(),
                                    }));
                                }
                                out.push(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "channels": channel_data,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:plan_version:") {
                            // Get plan version for a specific swarm
                            let swarm_id =
                                cmd.strip_prefix("swarm:plan_version:").unwrap_or("").trim();
                            let plans = swarm_plans.read().await;
                            if let Some(vp) = plans.get(swarm_id) {
                                Ok(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "version": vp.version,
                                    "item_count": vp.items.len(),
                                })
                                .to_string())
                            } else {
                                Ok(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "version": 0,
                                    "item_count": 0,
                                })
                                .to_string())
                            }
                        } else if cmd == "swarm:plans" {
                            // List all swarm plans
                            let plans = swarm_plans.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, vp) in plans.iter() {
                                out.push(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "item_count": vp.items.len(),
                                    "version": vp.version,
                                    "participants": vp.participants,
                                    "items": vp.items,
                                }));
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:plan:") {
                            // Get plan for specific swarm
                            let swarm_id = cmd.strip_prefix("swarm:plan:").unwrap_or("").trim();
                            let plans = swarm_plans.read().await;
                            if let Some(vp) = plans.get(swarm_id) {
                                Ok(serde_json::json!({
                                    "version": vp.version,
                                    "participants": vp.participants,
                                    "items": vp.items,
                                })
                                .to_string())
                            } else {
                                Ok("[]".to_string())
                            }
                        } else if cmd == "swarm:context" {
                            // List all shared context
                            let ctx = shared_context.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, entries) in ctx.iter() {
                                for (key, context) in entries.iter() {
                                    out.push(serde_json::json!({
                                        "swarm_id": swarm_id,
                                        "key": key,
                                        "value": context.value,
                                        "from_session": context.from_session,
                                        "from_name": context.from_name,
                                        "created_secs_ago": context.created_at.elapsed().as_secs(),
                                        "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                                    }));
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:context:") {
                            // Get context for specific swarm or key
                            let arg = cmd.strip_prefix("swarm:context:").unwrap_or("").trim();
                            let ctx = shared_context.read().await;
                            // Check if arg contains a key separator
                            if let Some((swarm_id, key)) = arg.split_once(':') {
                                // Get specific key in specific swarm
                                if let Some(entries) = ctx.get(swarm_id) {
                                    if let Some(context) = entries.get(key) {
                                        Ok(serde_json::json!({
                                            "swarm_id": swarm_id,
                                            "key": key,
                                            "value": context.value,
                                            "from_session": context.from_session,
                                            "from_name": context.from_name,
                                            "created_secs_ago": context.created_at.elapsed().as_secs(),
                                            "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                                        }).to_string())
                                    } else {
                                        Err(anyhow::anyhow!(
                                            "No context key '{}' in swarm '{}'",
                                            key,
                                            swarm_id
                                        ))
                                    }
                                } else {
                                    Err(anyhow::anyhow!("No context for swarm '{}'", swarm_id))
                                }
                            } else {
                                // Get all context for swarm
                                if let Some(entries) = ctx.get(arg) {
                                    let mut out: Vec<serde_json::Value> = Vec::new();
                                    for (key, context) in entries.iter() {
                                        out.push(serde_json::json!({
                                            "key": key,
                                            "value": context.value,
                                            "from_session": context.from_session,
                                            "from_name": context.from_name,
                                            "created_secs_ago": context.created_at.elapsed().as_secs(),
                                            "updated_secs_ago": context.updated_at.elapsed().as_secs(),
                                        }));
                                    }
                                    Ok(serde_json::to_string_pretty(&out)
                                        .unwrap_or_else(|_| "[]".to_string()))
                                } else {
                                    Ok("[]".to_string())
                                }
                            }
                        } else if cmd == "swarm:touches" {
                            // List all file touches
                            let touches = file_touches.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (path, accesses) in touches.iter() {
                                for access in accesses.iter() {
                                    let name = members
                                        .get(&access.session_id)
                                        .and_then(|m| m.friendly_name.clone());
                                    let timestamp_iso = access
                                        .absolute_time
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    out.push(serde_json::json!({
                                        "path": path.to_string_lossy(),
                                        "session_id": access.session_id,
                                        "session_name": name,
                                        "op": access.op.as_str(),
                                        "summary": access.summary,
                                        "age_secs": access.timestamp.elapsed().as_secs(),
                                        "timestamp_unix": timestamp_iso,
                                    }));
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:touches:") {
                            // Get touches for specific path or filter by swarm
                            let arg = cmd.strip_prefix("swarm:touches:").unwrap_or("").trim();
                            let touches = file_touches.read().await;
                            let members = swarm_members.read().await;

                            // Check if filtering by swarm
                            if arg.starts_with("swarm:") {
                                let swarm_id = arg.strip_prefix("swarm:").unwrap_or("");
                                // Get session IDs for this swarm
                                let swarm_sessions: HashSet<String> = members
                                    .iter()
                                    .filter(|(_, m)| m.swarm_id.as_deref() == Some(swarm_id))
                                    .map(|(id, _)| id.clone())
                                    .collect();

                                let mut out: Vec<serde_json::Value> = Vec::new();
                                for (path, accesses) in touches.iter() {
                                    for access in accesses.iter() {
                                        if swarm_sessions.contains(&access.session_id) {
                                            let name = members
                                                .get(&access.session_id)
                                                .and_then(|m| m.friendly_name.clone());
                                            let timestamp_unix = access
                                                .absolute_time
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_secs())
                                                .unwrap_or(0);
                                            out.push(serde_json::json!({
                                                "path": path.to_string_lossy(),
                                                "session_id": access.session_id,
                                                "session_name": name,
                                                "op": access.op.as_str(),
                                                "summary": access.summary,
                                                "age_secs": access.timestamp.elapsed().as_secs(),
                                                "timestamp_unix": timestamp_unix,
                                            }));
                                        }
                                    }
                                }
                                Ok(serde_json::to_string_pretty(&out)
                                    .unwrap_or_else(|_| "[]".to_string()))
                            } else {
                                // Get touches for specific path
                                let path = PathBuf::from(arg);
                                if let Some(accesses) = touches.get(&path) {
                                    let mut out: Vec<serde_json::Value> = Vec::new();
                                    for access in accesses.iter() {
                                        let name = members
                                            .get(&access.session_id)
                                            .and_then(|m| m.friendly_name.clone());
                                        let timestamp_unix = access
                                            .absolute_time
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_secs())
                                            .unwrap_or(0);
                                        out.push(serde_json::json!({
                                            "session_id": access.session_id,
                                            "session_name": name,
                                            "op": access.op.as_str(),
                                            "summary": access.summary,
                                            "age_secs": access.timestamp.elapsed().as_secs(),
                                            "timestamp_unix": timestamp_unix,
                                        }));
                                    }
                                    Ok(serde_json::to_string_pretty(&out)
                                        .unwrap_or_else(|_| "[]".to_string()))
                                } else {
                                    Ok("[]".to_string())
                                }
                            }
                        } else if cmd == "swarm:conflicts" {
                            // List files touched by multiple sessions
                            let touches = file_touches.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (path, accesses) in touches.iter() {
                                // Get unique session IDs
                                let unique_sessions: HashSet<_> =
                                    accesses.iter().map(|a| &a.session_id).collect();
                                if unique_sessions.len() > 1 {
                                    // Build full access history for this conflicting file
                                    let access_history: Vec<_> = accesses
                                        .iter()
                                        .map(|a| {
                                            let name = members
                                                .get(&a.session_id)
                                                .and_then(|m| m.friendly_name.clone());
                                            let timestamp_unix = a
                                                .absolute_time
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_secs())
                                                .unwrap_or(0);
                                            serde_json::json!({
                                                "session_id": a.session_id,
                                                "session_name": name,
                                                "op": a.op.as_str(),
                                                "summary": a.summary,
                                                "age_secs": a.timestamp.elapsed().as_secs(),
                                                "timestamp_unix": timestamp_unix,
                                            })
                                        })
                                        .collect();
                                    out.push(serde_json::json!({
                                        "path": path.to_string_lossy(),
                                        "session_count": unique_sessions.len(),
                                        "accesses": access_history,
                                    }));
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "swarm:proposals" {
                            // List all pending plan proposals across all swarms
                            let ctx = shared_context.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for (swarm_id, swarm_ctx) in ctx.iter() {
                                for (key, context) in swarm_ctx.iter() {
                                    if key.starts_with("plan_proposal:") {
                                        let proposer_id =
                                            key.strip_prefix("plan_proposal:").unwrap_or("");
                                        let proposer_name = members
                                            .get(proposer_id)
                                            .and_then(|m| m.friendly_name.clone());
                                        let item_count =
                                            serde_json::from_str::<Vec<serde_json::Value>>(
                                                &context.value,
                                            )
                                            .map(|v| v.len())
                                            .unwrap_or(0);
                                        out.push(serde_json::json!({
                                            "swarm_id": swarm_id,
                                            "proposer_session": proposer_id,
                                            "proposer_name": proposer_name,
                                            "item_count": item_count,
                                            "age_secs": context.created_at.elapsed().as_secs(),
                                            "status": "pending",
                                        }));
                                    }
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:proposals:") {
                            // Get proposals for specific swarm or specific proposal
                            let arg = cmd.strip_prefix("swarm:proposals:").unwrap_or("").trim();
                            let ctx = shared_context.read().await;
                            let members = swarm_members.read().await;

                            // Check if this is a session ID (get specific proposal details)
                            if arg.starts_with("session_") {
                                // Find proposal from this session across all swarms
                                let proposal_key = format!("plan_proposal:{}", arg);
                                let mut found_proposal: Option<String> = None;
                                for (swarm_id, swarm_ctx) in ctx.iter() {
                                    if let Some(context) = swarm_ctx.get(&proposal_key) {
                                        let proposer_name =
                                            members.get(arg).and_then(|m| m.friendly_name.clone());
                                        let items: Vec<serde_json::Value> =
                                            serde_json::from_str(&context.value)
                                                .unwrap_or_default();
                                        found_proposal = Some(
                                            serde_json::json!({
                                                "swarm_id": swarm_id,
                                                "proposer_session": arg,
                                                "proposer_name": proposer_name,
                                                "status": "pending",
                                                "age_secs": context.created_at.elapsed().as_secs(),
                                                "items": items,
                                            })
                                            .to_string(),
                                        );
                                        break;
                                    }
                                }
                                if let Some(result) = found_proposal {
                                    Ok(result)
                                } else {
                                    Err(anyhow::anyhow!("No proposal found from session '{}'", arg))
                                }
                            } else {
                                // Filter by swarm ID
                                let mut out: Vec<serde_json::Value> = Vec::new();
                                if let Some(swarm_ctx) = ctx.get(arg) {
                                    for (key, context) in swarm_ctx.iter() {
                                        if key.starts_with("plan_proposal:") {
                                            let proposer_id =
                                                key.strip_prefix("plan_proposal:").unwrap_or("");
                                            let proposer_name = members
                                                .get(proposer_id)
                                                .and_then(|m| m.friendly_name.clone());
                                            let items: Vec<serde_json::Value> =
                                                serde_json::from_str(&context.value)
                                                    .unwrap_or_default();
                                            out.push(serde_json::json!({
                                                "proposer_session": proposer_id,
                                                "proposer_name": proposer_name,
                                                "status": "pending",
                                                "age_secs": context.created_at.elapsed().as_secs(),
                                                "items": items,
                                            }));
                                        }
                                    }
                                }
                                Ok(serde_json::to_string_pretty(&out)
                                    .unwrap_or_else(|_| "[]".to_string()))
                            }
                        } else if cmd.starts_with("swarm:info:") {
                            // Get full info for a specific swarm
                            let swarm_id = cmd.strip_prefix("swarm:info:").unwrap_or("").trim();
                            let swarms = swarms_by_id.read().await;
                            let coordinators = swarm_coordinators.read().await;
                            let members = swarm_members.read().await;
                            let plans = swarm_plans.read().await;
                            let ctx = shared_context.read().await;
                            let touches = file_touches.read().await;

                            if let Some(session_ids) = swarms.get(swarm_id) {
                                let coordinator = coordinators.get(swarm_id);
                                let coordinator_name = coordinator.and_then(|cid| {
                                    members.get(cid).and_then(|m| m.friendly_name.clone())
                                });

                                // Get member details
                                let member_details: Vec<_> = session_ids
                                    .iter()
                                    .filter_map(|sid| {
                                        members.get(sid).map(|m| {
                                            serde_json::json!({
                                                "session_id": m.session_id,
                                                "friendly_name": m.friendly_name,
                                                "status": m.status,
                                                "detail": m.detail,
                                                "working_dir": m.working_dir,
                                            })
                                        })
                                    })
                                    .collect();

                                // Get plan
                                let plan = plans
                                    .get(swarm_id)
                                    .map(|vp| &vp.items)
                                    .cloned()
                                    .unwrap_or_default();

                                // Get context keys
                                let context_keys: Vec<_> = ctx
                                    .get(swarm_id)
                                    .map(|entries| entries.keys().cloned().collect())
                                    .unwrap_or_default();

                                // Get files with conflicts in this swarm
                                let conflicts: Vec<_> = touches
                                    .iter()
                                    .filter_map(|(path, accesses)| {
                                        let swarm_accesses: Vec<_> = accesses
                                            .iter()
                                            .filter(|a| session_ids.contains(&a.session_id))
                                            .collect();
                                        let unique: HashSet<_> =
                                            swarm_accesses.iter().map(|a| &a.session_id).collect();
                                        if unique.len() > 1 {
                                            Some(path.to_string_lossy().to_string())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();

                                Ok(serde_json::json!({
                                    "swarm_id": swarm_id,
                                    "member_count": session_ids.len(),
                                    "members": member_details,
                                    "coordinator": coordinator,
                                    "coordinator_name": coordinator_name,
                                    "plan": plan,
                                    "context_keys": context_keys,
                                    "conflict_files": conflicts,
                                })
                                .to_string())
                            } else {
                                Err(anyhow::anyhow!("No swarm with id '{}'", swarm_id))
                            }
                        } else if cmd.starts_with("swarm:broadcast:") {
                            // Broadcast a message to all members of a swarm
                            let rest = cmd.strip_prefix("swarm:broadcast:").unwrap_or("").trim();
                            // Parse: swarm_id message or just message (uses requester's swarm)
                            let (target_swarm_id, message) = if let Some(space_idx) = rest.find(' ')
                            {
                                let potential_id = &rest[..space_idx];
                                let msg = rest[space_idx + 1..].trim();
                                // Check if potential_id looks like a swarm_id (contains /)
                                if potential_id.contains('/') {
                                    (Some(potential_id.to_string()), msg.to_string())
                                } else {
                                    (None, rest.to_string())
                                }
                            } else {
                                (None, rest.to_string())
                            };

                            if message.is_empty() {
                                Err(anyhow::anyhow!("swarm:broadcast requires a message"))
                            } else {
                                // Find the swarm to broadcast to
                                let swarm_id = if let Some(id) = target_swarm_id {
                                    Some(id)
                                } else {
                                    // Try to find requester's swarm
                                    let members = swarm_members.read().await;
                                    let current_session = session_id.read().await;
                                    members
                                        .get(&*current_session)
                                        .and_then(|m| m.swarm_id.clone())
                                };

                                if let Some(swarm_id) = swarm_id {
                                    let swarms = swarms_by_id.read().await;
                                    let members = swarm_members.read().await;
                                    let current_session = session_id.read().await;
                                    let from_name = members
                                        .get(&*current_session)
                                        .and_then(|m| m.friendly_name.clone());

                                    if let Some(member_ids) = swarms.get(&swarm_id) {
                                        let mut sent_count = 0;
                                        for member_id in member_ids {
                                            if let Some(member) = members.get(member_id) {
                                                let notification = ServerEvent::Notification {
                                                    from_session: current_session.clone(),
                                                    from_name: from_name.clone(),
                                                    notification_type: NotificationType::Message {
                                                        scope: Some("broadcast".to_string()),
                                                        channel: None,
                                                    },
                                                    message: message.clone(),
                                                };
                                                if member.event_tx.send(notification).is_ok() {
                                                    sent_count += 1;
                                                }
                                            }
                                        }
                                        Ok(serde_json::json!({
                                            "swarm_id": swarm_id,
                                            "message": message,
                                            "sent_to": sent_count,
                                        })
                                        .to_string())
                                    } else {
                                        Err(anyhow::anyhow!("No members in swarm '{}'", swarm_id))
                                    }
                                } else {
                                    Err(anyhow::anyhow!("No swarm found. Specify swarm_id: swarm:broadcast:<swarm_id> <message>"))
                                }
                            }
                        } else if cmd.starts_with("swarm:notify:") {
                            // Send notification to a specific session
                            let rest = cmd.strip_prefix("swarm:notify:").unwrap_or("").trim();
                            // Parse: session_id message
                            if let Some(space_idx) = rest.find(' ') {
                                let target_session = &rest[..space_idx];
                                let message = rest[space_idx + 1..].trim();

                                if message.is_empty() {
                                    Err(anyhow::anyhow!("swarm:notify requires a message"))
                                } else {
                                    let members = swarm_members.read().await;
                                    let current_session = session_id.read().await;
                                    let from_name = members
                                        .get(&*current_session)
                                        .and_then(|m| m.friendly_name.clone());

                                    if let Some(target) = members.get(target_session) {
                                        let notification = ServerEvent::Notification {
                                            from_session: current_session.clone(),
                                            from_name: from_name.clone(),
                                            notification_type: NotificationType::Message {
                                                scope: Some("dm".to_string()),
                                                channel: None,
                                            },
                                            message: message.to_string(),
                                        };
                                        if target.event_tx.send(notification).is_ok() {
                                            let target_name = target.friendly_name.clone();
                                            Ok(serde_json::json!({
                                                "sent_to": target_session,
                                                "sent_to_name": target_name,
                                                "message": message,
                                            })
                                            .to_string())
                                        } else {
                                            Err(anyhow::anyhow!("Failed to send notification"))
                                        }
                                    } else {
                                        Err(anyhow::anyhow!("Unknown session '{}'", target_session))
                                    }
                                }
                            } else {
                                Err(anyhow::anyhow!(
                                    "Usage: swarm:notify:<session_id> <message>"
                                ))
                            }
                        } else if cmd.starts_with("swarm:session:") {
                            // Get detailed execution state for a specific session
                            let target_session =
                                cmd.strip_prefix("swarm:session:").unwrap_or("").trim();
                            if target_session.is_empty() {
                                Err(anyhow::anyhow!("swarm:session requires a session_id"))
                            } else {
                                let sessions_guard = sessions.read().await;
                                let members = swarm_members.read().await;

                                if let Some(agent_arc) = sessions_guard.get(target_session) {
                                    let member_info = members.get(target_session);

                                    // Try to get agent state (may fail if agent is busy)
                                    let agent_state = if let Ok(agent) = agent_arc.try_lock() {
                                        Some(serde_json::json!({
                                            "provider": agent.provider_name(),
                                            "model": agent.provider_model(),
                                            "message_count": agent.message_count(),
                                            "pending_alert_count": agent.pending_alert_count(),
                                            "pending_alerts": agent.pending_alerts_preview(),
                                            "soft_interrupt_count": agent.soft_interrupt_count(),
                                            "soft_interrupts": agent.soft_interrupts_preview(),
                                            "has_urgent_interrupt": agent.has_urgent_interrupt(),
                                            "last_usage": agent.last_usage(),
                                        }))
                                    } else {
                                        None
                                    };

                                    let is_locked = agent_state.is_none();

                                    Ok(serde_json::json!({
                                        "session_id": target_session,
                                        "friendly_name": member_info.and_then(|m| m.friendly_name.clone()),
                                        "swarm_id": member_info.and_then(|m| m.swarm_id.clone()),
                                        "status": member_info.map(|m| m.status.clone()),
                                        "detail": member_info.and_then(|m| m.detail.clone()),
                                        "joined_secs_ago": member_info.map(|m| m.joined_at.elapsed().as_secs()),
                                        "status_changed_secs_ago": member_info.map(|m| m.last_status_change.elapsed().as_secs()),
                                        "is_processing": is_locked,
                                        "agent_state": agent_state,
                                    }).to_string())
                                } else {
                                    Err(anyhow::anyhow!("Unknown session '{}'", target_session))
                                }
                            }
                        } else if cmd == "swarm:interrupts" {
                            // List all pending interrupts across all sessions
                            let sessions_guard = sessions.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();

                            for (session_id, agent_arc) in sessions_guard.iter() {
                                if let Ok(agent) = agent_arc.try_lock() {
                                    let alert_count = agent.pending_alert_count();
                                    let interrupt_count = agent.soft_interrupt_count();

                                    if alert_count > 0 || interrupt_count > 0 {
                                        let name = members
                                            .get(session_id)
                                            .and_then(|m| m.friendly_name.clone());
                                        out.push(serde_json::json!({
                                            "session_id": session_id,
                                            "session_name": name,
                                            "pending_alert_count": alert_count,
                                            "pending_alerts": agent.pending_alerts_preview(),
                                            "soft_interrupt_count": interrupt_count,
                                            "soft_interrupts": agent.soft_interrupts_preview(),
                                            "has_urgent": agent.has_urgent_interrupt(),
                                        }));
                                    }
                                }
                            }
                            Ok(serde_json::to_string_pretty(&out)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("swarm:id:") {
                            // Compute swarm_id for a path and show provenance
                            let path_str = cmd.strip_prefix("swarm:id:").unwrap_or("").trim();
                            if path_str.is_empty() {
                                Err(anyhow::anyhow!("swarm:id requires a path"))
                            } else {
                                let path = PathBuf::from(path_str);

                                // Check env override first
                                let env_override = std::env::var("JCODE_SWARM_ID")
                                    .ok()
                                    .filter(|s| !s.trim().is_empty());

                                // Try to get git common dir
                                let git_common = git_common_dir_for(&path);

                                // Compute final swarm_id
                                let swarm_id = swarm_id_for_dir(Some(path.clone()));

                                let is_git_repo = git_common.is_some();
                                Ok(serde_json::json!({
                                    "path": path_str,
                                    "swarm_id": swarm_id,
                                    "source": if env_override.is_some() { "env:JCODE_SWARM_ID" }
                                              else if is_git_repo { "git_common_dir" }
                                              else { "none" },
                                    "env_override": env_override,
                                    "git_common_dir": git_common.clone(),
                                    "git_root": git_common,
                                    "is_git_repo": is_git_repo,
                                })
                                .to_string())
                            }
                        } else if cmd.starts_with("swarm:set_context:") {
                            // Set shared context: swarm:set_context:<session_id> <key> <value>
                            let rest = cmd.strip_prefix("swarm:set_context:").unwrap_or("").trim();
                            // Parse: session_id key value
                            let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                            if parts.len() < 3 {
                                Err(anyhow::anyhow!(
                                    "Usage: swarm:set_context:<session_id> <key> <value>"
                                ))
                            } else {
                                let acting_session = parts[0];
                                let key = parts[1].to_string();
                                let value = parts[2].to_string();

                                // Find swarm_id for acting session
                                let (swarm_id, friendly_name) = {
                                    let members = swarm_members.read().await;
                                    let swarm_id = members
                                        .get(acting_session)
                                        .and_then(|m| m.swarm_id.clone());
                                    let name = members
                                        .get(acting_session)
                                        .and_then(|m| m.friendly_name.clone());
                                    (swarm_id, name)
                                };

                                if let Some(swarm_id) = swarm_id {
                                    // Store context
                                    {
                                        let mut ctx = shared_context.write().await;
                                        let swarm_ctx = ctx
                                            .entry(swarm_id.clone())
                                            .or_insert_with(HashMap::new);
                                        let now = Instant::now();
                                        let created_at = swarm_ctx
                                            .get(&key)
                                            .map(|c| c.created_at)
                                            .unwrap_or(now);
                                        swarm_ctx.insert(
                                            key.clone(),
                                            SharedContext {
                                                key: key.clone(),
                                                value: value.clone(),
                                                from_session: acting_session.to_string(),
                                                from_name: friendly_name.clone(),
                                                created_at,
                                                updated_at: now,
                                            },
                                        );
                                    }

                                    // Notify other swarm members
                                    let swarm_session_ids: Vec<String> = {
                                        let swarms = swarms_by_id.read().await;
                                        swarms
                                            .get(&swarm_id)
                                            .map(|s| s.iter().cloned().collect())
                                            .unwrap_or_default()
                                    };
                                    let members = swarm_members.read().await;
                                    for sid in &swarm_session_ids {
                                        if sid != acting_session {
                                            if let Some(member) = members.get(sid) {
                                                let _ = member.event_tx.send(
                                                    ServerEvent::Notification {
                                                        from_session: acting_session.to_string(),
                                                        from_name: friendly_name.clone(),
                                                        notification_type:
                                                            NotificationType::SharedContext {
                                                                key: key.clone(),
                                                                value: value.clone(),
                                                            },
                                                        message: format!(
                                                            "Shared context: {} = {}",
                                                            key, value
                                                        ),
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    Ok(serde_json::json!({
                                        "swarm_id": swarm_id,
                                        "key": key,
                                        "value": value,
                                        "from_session": acting_session,
                                    })
                                    .to_string())
                                } else {
                                    Err(anyhow::anyhow!(
                                        "Session '{}' is not in a swarm",
                                        acting_session
                                    ))
                                }
                            }
                        } else if cmd.starts_with("swarm:approve_plan:") {
                            // Approve plan: swarm:approve_plan:<coordinator_session> <proposer_session>
                            let rest = cmd.strip_prefix("swarm:approve_plan:").unwrap_or("").trim();
                            let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                            if parts.len() < 2 {
                                Err(anyhow::anyhow!("Usage: swarm:approve_plan:<coordinator_session> <proposer_session>"))
                            } else {
                                let coord_session = parts[0];
                                let proposer_session = parts[1];

                                // Check coordinator status
                                let (swarm_id, is_coordinator) = {
                                    let members = swarm_members.read().await;
                                    let swarm_id =
                                        members.get(coord_session).and_then(|m| m.swarm_id.clone());
                                    let is_coord = if let Some(ref sid) = swarm_id {
                                        let coordinators = swarm_coordinators.read().await;
                                        coordinators
                                            .get(sid)
                                            .map(|c| c == coord_session)
                                            .unwrap_or(false)
                                    } else {
                                        false
                                    };
                                    (swarm_id, is_coord)
                                };

                                if !is_coordinator {
                                    Err(anyhow::anyhow!(
                                        "Only the coordinator can approve plan proposals."
                                    ))
                                } else if let Some(swarm_id) = swarm_id {
                                    // Read proposal
                                    let proposal_key =
                                        format!("plan_proposal:{}", proposer_session);
                                    let proposal_value = {
                                        let ctx = shared_context.read().await;
                                        ctx.get(&swarm_id)
                                            .and_then(|sc| sc.get(&proposal_key))
                                            .map(|c| c.value.clone())
                                    };

                                    match proposal_value {
                                        None => Err(anyhow::anyhow!(
                                            "No pending plan proposal from session '{}'",
                                            proposer_session
                                        )),
                                        Some(proposal) => {
                                            if let Ok(items) =
                                                serde_json::from_str::<Vec<PlanItem>>(&proposal)
                                            {
                                                let version = {
                                                    let mut plans = swarm_plans.write().await;
                                                    let vp = plans
                                                        .entry(swarm_id.clone())
                                                        .or_insert_with(VersionedPlan::new);
                                                    vp.items.extend(items.clone());
                                                    vp.version += 1;
                                                    vp.participants
                                                        .insert(coord_session.to_string());
                                                    vp.participants
                                                        .insert(proposer_session.to_string());
                                                    vp.version
                                                };
                                                // Remove proposal
                                                {
                                                    let mut ctx = shared_context.write().await;
                                                    if let Some(swarm_ctx) = ctx.get_mut(&swarm_id)
                                                    {
                                                        swarm_ctx.remove(&proposal_key);
                                                    }
                                                }
                                                Ok(serde_json::json!({
                                                    "approved": true,
                                                    "items_added": items.len(),
                                                    "plan_version": version,
                                                    "swarm_id": swarm_id,
                                                })
                                                .to_string())
                                            } else {
                                                Err(anyhow::anyhow!("Failed to parse plan proposal as Vec<PlanItem>"))
                                            }
                                        }
                                    }
                                } else {
                                    Err(anyhow::anyhow!("Not in a swarm."))
                                }
                            }
                        } else if cmd.starts_with("swarm:reject_plan:") {
                            // Reject plan: swarm:reject_plan:<coordinator_session> <proposer_session> [reason]
                            let rest = cmd.strip_prefix("swarm:reject_plan:").unwrap_or("").trim();
                            let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                            if parts.len() < 2 {
                                Err(anyhow::anyhow!("Usage: swarm:reject_plan:<coordinator_session> <proposer_session> [reason]"))
                            } else {
                                let coord_session = parts[0];
                                let proposer_session = parts[1];
                                let reason = if parts.len() >= 3 {
                                    Some(parts[2].to_string())
                                } else {
                                    None
                                };

                                // Check coordinator status
                                let (swarm_id, is_coordinator) = {
                                    let members = swarm_members.read().await;
                                    let swarm_id =
                                        members.get(coord_session).and_then(|m| m.swarm_id.clone());
                                    let is_coord = if let Some(ref sid) = swarm_id {
                                        let coordinators = swarm_coordinators.read().await;
                                        coordinators
                                            .get(sid)
                                            .map(|c| c == coord_session)
                                            .unwrap_or(false)
                                    } else {
                                        false
                                    };
                                    (swarm_id, is_coord)
                                };

                                if !is_coordinator {
                                    Err(anyhow::anyhow!(
                                        "Only the coordinator can reject plan proposals."
                                    ))
                                } else if let Some(swarm_id) = swarm_id {
                                    let proposal_key =
                                        format!("plan_proposal:{}", proposer_session);
                                    let proposal_exists = {
                                        let ctx = shared_context.read().await;
                                        ctx.get(&swarm_id)
                                            .and_then(|sc| sc.get(&proposal_key))
                                            .is_some()
                                    };
                                    if !proposal_exists {
                                        Err(anyhow::anyhow!(
                                            "No pending plan proposal from session '{}'",
                                            proposer_session
                                        ))
                                    } else {
                                        // Remove proposal
                                        {
                                            let mut ctx = shared_context.write().await;
                                            if let Some(swarm_ctx) = ctx.get_mut(&swarm_id) {
                                                swarm_ctx.remove(&proposal_key);
                                            }
                                        }
                                        let reason_msg = reason
                                            .as_ref()
                                            .map(|r| format!(": {}", r))
                                            .unwrap_or_default();
                                        Ok(serde_json::json!({
                                            "rejected": true,
                                            "proposer_session": proposer_session,
                                            "reason": reason_msg,
                                            "swarm_id": swarm_id,
                                        })
                                        .to_string())
                                    }
                                } else {
                                    Err(anyhow::anyhow!("Not in a swarm."))
                                }
                            }
                        } else if false {
                            // Placeholder (duplicates removed — swarm:roles, swarm:channels,
                            // swarm:plan_version are handled earlier in the chain)
                            Ok("unreachable".to_string())
                        } else if cmd == "ambient:status" {
                            // Get ambient mode status
                            if let Some(ref runner) = ambient_runner {
                                Ok(runner.status_json().await)
                            } else {
                                Ok(serde_json::json!({
                                    "enabled": false,
                                    "status": "disabled",
                                    "message": "Ambient mode is not enabled in config"
                                })
                                .to_string())
                            }
                        } else if cmd == "ambient:queue" {
                            if let Some(ref runner) = ambient_runner {
                                Ok(runner.queue_json().await)
                            } else {
                                Ok("[]".to_string())
                            }
                        } else if cmd == "ambient:trigger" {
                            if let Some(ref runner) = ambient_runner {
                                runner.trigger().await;
                                Ok("Ambient cycle triggered".to_string())
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled"))
                            }
                        } else if cmd == "ambient:log" {
                            if let Some(ref runner) = ambient_runner {
                                Ok(runner.log_json().await)
                            } else {
                                Ok("[]".to_string())
                            }
                        } else if cmd == "ambient:permissions" {
                            if let Some(ref runner) = ambient_runner {
                                let _ = runner
                                    .safety()
                                    .expire_dead_session_requests("debug_socket_gc");
                                let pending = runner.safety().pending_requests();
                                let items: Vec<serde_json::Value> = pending
                                    .iter()
                                    .map(|r| {
                                        let review_summary = r
                                            .context
                                            .as_ref()
                                            .and_then(|ctx| ctx.get("review"))
                                            .and_then(|review| review.get("summary"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or(&r.description);
                                        let review_why = r
                                            .context
                                            .as_ref()
                                            .and_then(|ctx| ctx.get("review"))
                                            .and_then(|review| review.get("why_permission_needed"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or(&r.rationale);
                                        serde_json::json!({
                                            "id": r.id,
                                            "action": r.action,
                                            "description": r.description,
                                            "rationale": r.rationale,
                                            "summary": review_summary,
                                            "why_permission_needed": review_why,
                                            "urgency": format!("{:?}", r.urgency),
                                            "wait": r.wait,
                                            "created_at": r.created_at.to_rfc3339(),
                                            "context": r.context,
                                        })
                                    })
                                    .collect();
                                Ok(serde_json::to_string_pretty(&items)
                                    .unwrap_or_else(|_| "[]".to_string()))
                            } else {
                                Ok("[]".to_string())
                            }
                        } else if cmd.starts_with("ambient:approve:") {
                            let request_id =
                                cmd.strip_prefix("ambient:approve:").unwrap_or("").trim();
                            if request_id.is_empty() {
                                Err(anyhow::anyhow!("Usage: ambient:approve:<request_id>"))
                            } else if let Some(ref runner) = ambient_runner {
                                runner.safety().record_decision(
                                    request_id,
                                    true,
                                    "debug_socket",
                                    None,
                                )?;
                                Ok(format!("Approved: {}", request_id))
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled"))
                            }
                        } else if cmd.starts_with("ambient:deny:") {
                            let rest = cmd.strip_prefix("ambient:deny:").unwrap_or("").trim();
                            if rest.is_empty() {
                                Err(anyhow::anyhow!("Usage: ambient:deny:<request_id> [reason]"))
                            } else if let Some(ref runner) = ambient_runner {
                                let mut parts = rest.splitn(2, char::is_whitespace);
                                let request_id = parts.next().unwrap_or("").trim();
                                let message = parts
                                    .next()
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty());
                                runner.safety().record_decision(
                                    request_id,
                                    false,
                                    "debug_socket",
                                    message,
                                )?;
                                Ok(format!("Denied: {}", request_id))
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled"))
                            }
                        } else if cmd == "ambient:stop" {
                            if let Some(ref runner) = ambient_runner {
                                runner.stop().await;
                                Ok("Ambient mode stopped".to_string())
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled"))
                            }
                        } else if cmd == "ambient:start" {
                            if let Some(ref runner) = ambient_runner {
                                if runner.start(Arc::clone(&provider)).await {
                                    Ok("Ambient mode started".to_string())
                                } else {
                                    Ok("Ambient mode is already running".to_string())
                                }
                            } else {
                                Err(anyhow::anyhow!("Ambient mode is not enabled in config"))
                            }
                        } else if cmd == "ambient:help" {
                            Ok(r#"Ambient mode debug commands (ambient: prefix):
  ambient:status              - Current ambient state, cycle count, last run
  ambient:queue               - Scheduled queue contents
  ambient:trigger             - Manually trigger an ambient cycle
  ambient:log                 - Recent transcript summaries
  ambient:permissions         - List pending permission requests
  ambient:approve:<id>        - Approve a permission request
  ambient:deny:<id> [reason]  - Deny a permission request (optional reason)
  ambient:start               - Start/restart ambient mode
  ambient:stop                - Stop ambient mode"#
                                .to_string())
                        } else if cmd == "events:recent" || cmd.starts_with("events:recent:") {
                            // Get recent events (default 50, or specify count)
                            let count: usize = cmd
                                .strip_prefix("events:recent:")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(50);

                            let history = event_history.read().await;
                            let events: Vec<serde_json::Value> = history
                                .iter()
                                .rev()
                                .take(count)
                                .map(|e| {
                                    let timestamp_unix = e
                                        .absolute_time
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    serde_json::json!({
                                        "id": e.id,
                                        "session_id": e.session_id,
                                        "session_name": e.session_name,
                                        "swarm_id": e.swarm_id,
                                        "event": e.event,
                                        "age_secs": e.timestamp.elapsed().as_secs(),
                                        "timestamp_unix": timestamp_unix,
                                    })
                                })
                                .collect();
                            Ok(serde_json::to_string_pretty(&events)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd.starts_with("events:since:") {
                            // Get events since a specific event ID
                            let since_id: u64 = cmd
                                .strip_prefix("events:since:")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);

                            let history = event_history.read().await;
                            let events: Vec<serde_json::Value> = history
                                .iter()
                                .filter(|e| e.id > since_id)
                                .map(|e| {
                                    let timestamp_unix = e
                                        .absolute_time
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs())
                                        .unwrap_or(0);
                                    serde_json::json!({
                                        "id": e.id,
                                        "session_id": e.session_id,
                                        "session_name": e.session_name,
                                        "swarm_id": e.swarm_id,
                                        "event": e.event,
                                        "age_secs": e.timestamp.elapsed().as_secs(),
                                        "timestamp_unix": timestamp_unix,
                                    })
                                })
                                .collect();
                            Ok(serde_json::to_string_pretty(&events)
                                .unwrap_or_else(|_| "[]".to_string()))
                        } else if cmd == "events:types" {
                            // List available event types
                            Ok(serde_json::json!({
                                "types": [
                                    "file_touch",
                                    "notification",
                                    "plan_update",
                                    "plan_proposal",
                                    "context_update",
                                    "status_change",
                                    "member_change"
                                ],
                                "description": "Use events:recent or events:since:<id> to get events"
                            }).to_string())
                        } else if cmd == "events:count" {
                            // Get current event count and latest ID
                            let history = event_history.read().await;
                            let latest_id = history.last().map(|e| e.id).unwrap_or(0);
                            Ok(serde_json::json!({
                                "count": history.len(),
                                "latest_id": latest_id,
                                "max_history": MAX_EVENT_HISTORY,
                            })
                            .to_string())
                        } else if cmd == "background" || cmd == "background:tasks" {
                            // List background tasks (running + completed on disk)
                            let tasks = crate::background::global().list().await;
                            Ok(serde_json::json!({
                                "count": tasks.len(),
                                "tasks": tasks,
                            })
                            .to_string())
                        } else if cmd == "server:info" {
                            // Return server identity, health, and uptime
                            let uptime_secs = server_start_time.elapsed().as_secs();
                            let session_count = sessions.read().await.len();
                            let member_count = swarm_members.read().await.len();
                            let has_update = server_has_newer_binary();
                            Ok(serde_json::json!({
                                "id": server_identity.id,
                                "name": server_identity.name,
                                "icon": server_identity.icon,
                                "version": server_identity.version,
                                "git_hash": server_identity.git_hash,
                                "uptime_secs": uptime_secs,
                                "session_count": session_count,
                                "swarm_member_count": member_count,
                                "has_update": has_update,
                                "debug_control_enabled": debug_control_allowed(),
                            })
                            .to_string())
                        } else if cmd == "clients:map" || cmd == "clients:mapping" {
                            // Map connected clients to sessions
                            let connections = client_connections.read().await;
                            let members = swarm_members.read().await;
                            let mut out: Vec<serde_json::Value> = Vec::new();
                            for info in connections.values() {
                                let member = members.get(&info.session_id);
                                out.push(serde_json::json!({
                                    "client_id": info.client_id,
                                    "session_id": info.session_id,
                                    "friendly_name": member.and_then(|m| m.friendly_name.clone()),
                                    "working_dir": member.and_then(|m| m.working_dir.clone()),
                                    "swarm_id": member.and_then(|m| m.swarm_id.clone()),
                                    "status": member.map(|m| m.status.clone()),
                                    "detail": member.and_then(|m| m.detail.clone()),
                                    "connected_secs_ago": info.connected_at.elapsed().as_secs(),
                                    "last_seen_secs_ago": info.last_seen.elapsed().as_secs(),
                                }));
                            }
                            Ok(serde_json::json!({
                                "count": out.len(),
                                "clients": out,
                            })
                            .to_string())
                        } else if cmd == "clients" {
                            // List connected TUI clients
                            let debug_state = client_debug_state.read().await;
                            let client_ids: Vec<&String> = debug_state.clients.keys().collect();
                            Ok(serde_json::json!({
                                "count": debug_state.clients.len(),
                                "active_id": debug_state.active_id,
                                "client_ids": client_ids,
                            })
                            .to_string())
                        } else if cmd == "swarm:help" {
                            Ok(swarm_debug_help_text())
                        } else if cmd == "help" {
                            Ok(debug_help_text())
                        } else {
                            match resolve_debug_session(&sessions, &session_id, requested_session)
                                .await
                            {
                                Ok((_session, agent)) => {
                                    execute_debug_command(agent, cmd, Arc::clone(&debug_jobs)).await
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
  tools                    - List available tools (names only)
  tools:full               - List tools with full definitions (input_schema)
  mcp:servers              - List configured + connected MCP servers
  last_response            - Get last assistant response
  message:<text>           - Send message to agent
  message_async:<text>     - Send message async (returns job id)
  swarm_message:<text>     - Plan and run subtasks via swarm workers, then integrate
  swarm_message_async:<text> - Async swarm message (returns job id)
  tool:<name> <json>       - Execute tool directly
  cancel                   - Cancel in-flight generation (urgent interrupt)
  clear                    - Clear conversation history
  agent:info               - Get comprehensive agent internal state
  jobs                     - List async debug jobs
  job_status:<id>          - Get async job status/output
  job_wait:<id>            - Wait for async job to finish
  job_cancel:<id>          - Cancel a running job
  jobs:purge               - Remove completed/failed jobs
  jobs:session:<id>        - List jobs for a session
  background:tasks         - List background tasks
  sessions                 - List all sessions (with full metadata)
  clients                  - List connected TUI clients
  clients:map              - Map connected clients to sessions
  server:info              - Server identity, health, uptime
  swarm                    - List swarm members + status (alias: swarm:members)
  swarm:help               - Full swarm command reference
  create_session           - Create headless session
  create_session:<path>    - Create session with working dir
  destroy_session:<id>     - Destroy a session
  set_model:<model>        - Switch model (may change provider)
  set_provider:<name>      - Switch provider (claude/openai/openrouter/cursor/copilot/antigravity)
  trigger_extraction       - Force end-of-session memory extraction
  available_models         - List all available models
  reload                   - Trigger server reload with current binary

SWARM COMMANDS (swarm: prefix):
  swarm:members            - List all swarm members with details
  swarm:list               - List all swarms with member counts
  swarm:info:<swarm_id>    - Full info for a swarm
  swarm:coordinators       - List all coordinators
  swarm:roles              - List all members with roles
  swarm:plans              - List all swarm plans
  swarm:plan_version:<id>  - Show plan version for a swarm
  swarm:proposals          - List pending plan proposals
  swarm:context            - List all shared context
  swarm:touches            - List all file touches
  swarm:conflicts          - Files touched by multiple sessions
  swarm:channels           - List channel subscriptions
  swarm:broadcast:<msg>    - Broadcast to swarm members
  swarm:notify:<sid> <msg> - Send DM to specific session
  swarm:help               - Full swarm command reference

AMBIENT COMMANDS (ambient: prefix):
  ambient:status              - Current ambient state, cycle count, last run
  ambient:queue               - Scheduled queue contents
  ambient:trigger             - Manually trigger an ambient cycle
  ambient:log                 - Recent transcript summaries
  ambient:permissions         - List pending permission requests
  ambient:approve:<id>        - Approve a permission request
  ambient:deny:<id> [reason]  - Deny a permission request (optional reason)
  ambient:start               - Start/restart ambient mode
  ambient:stop                - Stop ambient mode
  ambient:help                - Ambient command reference

EVENTS COMMANDS (events: prefix):
  events:recent            - Get recent events (default 50)
  events:recent:<N>        - Get recent N events
  events:since:<id>        - Get events since event ID
  events:count             - Event count and latest ID
  events:types             - List available event types

CLIENT COMMANDS (client: prefix):
  client:state             - Get TUI state
  client:frame             - Get latest visual debug frame (JSON)
  client:frame-normalized  - Get normalized frame (for diffs)
  client:screen            - Dump visual debug to file
  client:layout            - Get latest layout JSON
  client:margins           - Get layout margins JSON
  client:widgets           - Get info widget summary/placements
  client:render-stats      - Get render timing + order JSON
  client:render-order      - Get render order list
  client:anomalies         - Get latest visual debug anomalies
  client:theme             - Get palette snapshot
  client:mermaid:stats     - Get mermaid render/cache stats
  client:mermaid:memory    - Mermaid memory profile (RSS + cache estimates)
  client:mermaid:memory-bench [n] - Synthetic Mermaid memory benchmark
  client:mermaid:cache     - List mermaid cache entries
  client:mermaid:state     - Get image state (resize modes)
  client:mermaid:test      - Render test diagram
  client:mermaid:scroll    - Run scroll simulation test
  client:mermaid:render <c> - Render arbitrary mermaid
  client:mermaid:evict     - Clear mermaid cache
  client:markdown:stats    - Get markdown render stats
  client:overlay:on/off    - Toggle overlay boxes
  client:input             - Get current input buffer
  client:set_input:<text>  - Set input buffer
  client:keys:<keyspec>    - Inject key events
  client:message:<text>    - Inject and submit message
  client:inject:<role>:<t> - Inject display message (no send)
  client:scroll:<dir>      - Scroll (up/down/top/bottom)
  client:scroll-test[:<j>] - Run offscreen scroll+diagram test
  client:scroll-suite[:<j>] - Run scroll+diagram test suite
  client:wait              - Check if processing
  client:history           - Get display messages
  client:help              - Client command help

TESTER COMMANDS (tester: prefix):
  tester:spawn             - Spawn new tester instance
  tester:list              - List active testers
  tester:<id>:frame        - Get frame from tester
  tester:<id>:message:<t>  - Send message to tester
  tester:<id>:inject:<t>   - Inject display message (no send)
  tester:<id>:state        - Get tester state
  tester:<id>:scroll-test  - Run offscreen scroll+diagram test
  tester:<id>:scroll-suite - Run scroll+diagram test suite
  tester:<id>:stop         - Stop tester

Examples:
  {"type":"debug_command","id":1,"command":"state"}
  {"type":"debug_command","id":2,"command":"client:frame"}
  {"type":"debug_command","id":3,"command":"tester:list"}
  {"type":"debug_command","id":4,"command":"set_provider:openai","session_id":"..."}
  {"type":"debug_command","id":5,"command":"swarm:info:/home/user/project"}"#
        .to_string()
}

/// Generate help text for swarm debug commands
fn swarm_debug_help_text() -> String {
    r#"Swarm debug commands (swarm: prefix):

MEMBERS & STRUCTURE:
  swarm                    - List all swarm members (alias for swarm:members)
  swarm:members            - List all swarm members with full details
  swarm:list               - List all swarm IDs with member counts and coordinators
  swarm:info:<swarm_id>    - Full info: members, coordinator, plan, context, conflicts

COORDINATORS & ROLES:
  swarm:coordinators       - List all coordinators (swarm_id -> session_id)
  swarm:coordinator:<id>   - Get coordinator for specific swarm
  swarm:roles              - List all members with their roles

PLANS (server-scoped plan items):
  swarm:plans              - List all swarm plans with item counts
  swarm:plan:<swarm_id>    - Get plan items for specific swarm
  swarm:plan_version:<id>  - Show current plan version for a swarm

PLAN PROPOSALS (pending approval):
  swarm:proposals          - List all pending proposals across swarms
  swarm:proposals:<swarm>  - List proposals for a specific swarm (with items)
  swarm:proposals:<sess>   - Get detailed proposal from a session

SHARED CONTEXT (key-value store):
  swarm:context            - List all shared context entries
  swarm:context:<swarm_id> - List context for specific swarm
  swarm:context:<swarm_id>:<key> - Get specific context value

FILE TOUCHES (conflict detection):
  swarm:touches            - List all file touches (path, session, op, age, timestamp)
  swarm:touches:<path>     - Get touches for specific file
  swarm:touches:swarm:<id> - Get touches filtered by swarm members
  swarm:conflicts          - List files touched by multiple sessions

NOTIFICATIONS:
  swarm:broadcast:<msg>    - Broadcast message to all members of your swarm
  swarm:broadcast:<swarm_id> <msg> - Broadcast to specific swarm
  swarm:notify:<session_id> <msg> - Send direct message to specific session

EXECUTION STATE:
  swarm:session:<id>       - Detailed session state (interrupts, provider, usage)
  swarm:interrupts         - List pending interrupts across all sessions

CHANNELS:
  swarm:channels           - List channel subscriptions per swarm

OPERATIONS (debug-only, bypass tool:communicate):
  swarm:set_context:<sess> <key> <value> - Set shared context as session
  swarm:approve_plan:<coord> <proposer>  - Approve plan proposal (coordinator only)
  swarm:reject_plan:<coord> <proposer> [reason] - Reject plan proposal

UTILITIES:
  swarm:id:<path>          - Compute swarm_id for a path and show provenance

REAL-TIME EVENTS:
  events:recent            - Get recent 50 events
  events:recent:<N>        - Get recent N events
  events:since:<id>        - Get events since event ID (for polling)
  events:count             - Get event count and latest ID
  events:types             - List available event types

Examples:
  {"type":"debug_command","id":1,"command":"swarm:list"}
  {"type":"debug_command","id":2,"command":"swarm:info:/home/user/myproject"}
  {"type":"debug_command","id":3,"command":"swarm:plan:/home/user/myproject"}
  {"type":"debug_command","id":4,"command":"swarm:broadcast:Build complete, ready for review"}
  {"type":"debug_command","id":5,"command":"swarm:notify:session_fox_123 Please review PR #42"}"#
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
        if content.trim().is_empty() {
            return Ok(vec![]);
        }
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
        "inject" => format!("inject:{}", arg.unwrap_or("")),
        "keys" => format!("keys:{}", arg.unwrap_or("")),
        "set_input" => format!("set_input:{}", arg.unwrap_or("")),
        "scroll" => format!("scroll:{}", arg.unwrap_or("down")),
        "scroll-test" => match arg {
            Some(raw) => format!("scroll-test:{}", raw),
            None => "scroll-test".to_string(),
        },
        "scroll-suite" => match arg {
            Some(raw) => format!("scroll-suite:{}", raw),
            None => "scroll-suite".to_string(),
        },
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

        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("Server disconnected");
        }
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
    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

/// Server hot-reload with progress streaming to client
/// This just restarts with the existing binary - no rebuild
async fn do_server_reload_with_progress(
    tx: tokio::sync::mpsc::UnboundedSender<ServerEvent>,
    provider_arg: Option<String>,
    model_arg: Option<String>,
    socket_arg: String,
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

    let repo_dir = get_repo_dir();
    if let Some(repo_dir) = &repo_dir {
        send_progress(
            "init",
            &format!("📁 Repository: {}", repo_dir.display()),
            Some(true),
            None,
        );
    } else {
        send_progress("init", "📁 Repository: (not found)", Some(true), None);
    }

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
    if let Some(repo_dir) = &repo_dir {
        let head_output = ProcessCommand::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(repo_dir)
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

    // Exec into new binary with serve command (preserve args)
    let mut cmd = ProcessCommand::new(&exe);
    cmd.arg("serve").arg("--socket").arg(socket_arg);
    if let Some(provider) = provider_arg {
        cmd.arg("--provider").arg(provider);
    }
    if let Some(model) = model_arg {
        cmd.arg("--model").arg(model);
    }
    let err = cmd.exec();

    // exec() only returns on error
    Err(anyhow::anyhow!("Failed to exec {:?}: {}", exe, err))
}

fn provider_cli_arg(provider_name: &str) -> Option<String> {
    let lowered = provider_name.trim().to_lowercase();
    match lowered.as_str() {
        "openai" => Some("openai".to_string()),
        "claude" => Some("claude".to_string()),
        "cursor" => Some("cursor".to_string()),
        "copilot" => Some("copilot".to_string()),
        "antigravity" => Some("antigravity".to_string()),
        _ => None,
    }
}

fn normalize_model_arg(model: String) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Monitor for selfdev signal files and exec into new binary
/// The server directly execs into the new binary instead of exiting
/// NOTE: This should only be called on selfdev servers (guarded at call site)
async fn monitor_selfdev_signals() {
    use std::os::unix::process::CommandExt;
    use std::process::Command as ProcessCommand;
    use tokio::time::{interval, Duration};

    // Double-check: only run on selfdev servers
    if !is_selfdev_env() {
        return;
    }

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
                crate::logging::info("Server: reload signal received, exec'ing into canary binary");

                // Get canary binary path
                if let Ok(binary) = crate::build::canary_binary_path() {
                    if binary.exists() {
                        // Small delay for filesystem sync
                        std::thread::sleep(std::time::Duration::from_millis(200));

                        // Exec into the new binary with serve mode
                        let err = ProcessCommand::new(&binary).arg("serve").exec();

                        // If we get here, exec failed
                        crate::logging::error(&format!("Failed to exec into canary {:?}: {}", binary, err));
                    }
                }
                // Fallback: just exit and let something else restart us
                std::process::exit(42);
            }
        }

        // Check for rollback signal (switch to stable)
        let rollback_path = jcode_dir.join("rollback-signal");
        if rollback_path.exists() {
            if let Ok(_hash) = std::fs::read_to_string(&rollback_path) {
                let _ = std::fs::remove_file(&rollback_path);
                crate::logging::info(
                    "Server: rollback signal received, exec'ing into stable binary",
                );

                // Get stable binary path
                if let Ok(binary) = crate::build::stable_binary_path() {
                    if binary.exists() {
                        std::thread::sleep(std::time::Duration::from_millis(200));

                        let err = ProcessCommand::new(&binary).arg("serve").exec();

                        crate::logging::error(&format!("Failed to exec into stable {:?}: {}", binary, err));
                    }
                }
                std::process::exit(43);
            }
        }
    }
}
