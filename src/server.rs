#![allow(dead_code)]

mod client_actions;
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
mod swarm;

use self::client_lifecycle::handle_client;
use self::debug::{handle_debug_client, ClientConnectionInfo, ClientDebugState};
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
use crate::agent::{Agent, SoftInterruptQueue};
use crate::ambient_runner::AmbientRunnerHandle;
use crate::build;
use crate::bus::{Bus, BusEvent, FileOp};
use crate::plan::PlanItem;
#[allow(unused_imports)]
use crate::protocol::ContextEntry;
use crate::protocol::{HistoryMessage, NotificationType, Request, ServerEvent};
use crate::provider::Provider;
use crate::transport::{Listener, ReadHalf, Stream, WriteHalf};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
    /// Whether this is a headless (spawned) session vs a TUI-connected session.
    /// Headless sessions should not be automatically elected as coordinator.
    pub is_headless: bool,
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
const MAX_EVENT_HISTORY: usize = 5000;

pub(super) type SessionInterruptQueues = Arc<RwLock<HashMap<String, SoftInterruptQueue>>>;

pub(super) fn enqueue_soft_interrupt(
    queue: &SoftInterruptQueue,
    content: String,
    urgent: bool,
) -> bool {
    if let Ok(mut pending) = queue.lock() {
        pending.push(crate::agent::SoftInterruptMessage { content, urgent });
        true
    } else {
        false
    }
}

pub(super) async fn register_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    session_id: &str,
    queue: SoftInterruptQueue,
) {
    let mut guard = queues.write().await;
    guard.insert(session_id.to_string(), queue);
}

pub(super) async fn rename_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    old_session_id: &str,
    new_session_id: &str,
) {
    let mut guard = queues.write().await;
    if let Some(queue) = guard.remove(old_session_id) {
        guard.insert(new_session_id.to_string(), queue);
    }
}

pub(super) async fn remove_session_interrupt_queue(
    queues: &SessionInterruptQueues,
    session_id: &str,
) {
    let mut guard = queues.write().await;
    guard.remove(session_id);
}

pub(super) async fn queue_soft_interrupt_for_session(
    session_id: &str,
    content: String,
    urgent: bool,
    queues: &SessionInterruptQueues,
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
) -> bool {
    if let Some(queue) = queues.read().await.get(session_id).cloned() {
        return enqueue_soft_interrupt(&queue, content, urgent);
    }

    let queue = {
        let guard = sessions.read().await;
        guard.get(session_id).and_then(|agent| {
            agent
                .try_lock()
                .ok()
                .map(|agent_guard| agent_guard.soft_interrupt_queue())
        })
    };

    if let Some(queue) = queue {
        register_session_interrupt_queue(queues, session_id, queue.clone()).await;
        enqueue_soft_interrupt(&queue, content, urgent)
    } else {
        false
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

/// Marker file indicating a server reload handoff is in progress.
///
/// Clients use this to distinguish "server is temporarily restarting" from
/// "no server exists, spawn a new one".
pub fn reload_marker_path() -> PathBuf {
    crate::storage::runtime_dir().join("jcode.reload")
}

pub fn write_reload_marker() {
    ReloadState {
        request_id: "unknown".to_string(),
        hash: "unknown".to_string(),
        phase: ReloadPhase::Starting,
        pid: std::process::id(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail: None,
    }
    .write();
}

pub fn clear_reload_marker() {
    let _ = std::fs::remove_file(reload_marker_path());
}

pub fn reload_marker_exists() -> bool {
    reload_marker_path().exists()
}

pub fn reload_marker_active(max_age: Duration) -> bool {
    matches!(recent_reload_state(max_age), Some(state) if state.phase == ReloadPhase::Starting)
}

pub fn recent_reload_state(max_age: Duration) -> Option<ReloadState> {
    let path = reload_marker_path();
    let state = ReloadState::load()?;
    let Ok(metadata) = std::fs::metadata(&path) else {
        return None;
    };
    let Ok(modified) = metadata.modified() else {
        let _ = std::fs::remove_file(&path);
        return None;
    };
    let Ok(elapsed) = modified.elapsed() else {
        return Some(state);
    };
    if elapsed <= max_age {
        Some(state)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}

pub fn write_reload_state(
    request_id: &str,
    hash: &str,
    phase: ReloadPhase,
    detail: Option<String>,
) {
    ReloadState {
        request_id: request_id.to_string(),
        hash: hash.to_string(),
        phase,
        pid: std::process::id(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        detail,
    }
    .write();
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
pub async fn connect_socket(path: &std::path::Path) -> Result<Stream> {
    match Stream::connect(path).await {
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

async fn socket_has_live_listener(path: &std::path::Path) -> bool {
    crate::transport::is_socket_path(path) && Stream::connect(path).await.is_ok()
}

#[cfg(unix)]
fn mark_close_on_exec<T: std::os::fd::AsRawFd>(io: &T) {
    let fd = io.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags >= 0 {
        let _ = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    }
}

#[cfg(test)]
mod socket_tests {
    use super::{
        cleanup_socket_pair, reload_marker_active, reload_marker_path, sibling_socket_path,
        write_reload_state, ReloadPhase,
    };
    use std::time::Duration;

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

    #[test]
    fn reload_marker_active_expires_stale_marker() {
        let marker = reload_marker_path();
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        write_reload_state("test-request", "test-hash", ReloadPhase::Starting, None);
        assert!(reload_marker_active(Duration::from_secs(30)));
        std::thread::sleep(Duration::from_millis(5));
        assert!(!reload_marker_active(Duration::ZERO));
        assert!(!marker.exists(), "stale reload marker should be cleaned up");
    }
}

#[cfg(test)]
mod startup_tests {
    use super::Server;
    use crate::message::{Message, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use crate::transport::Listener;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct TestProvider;

    #[async_trait]
    impl Provider for TestProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            unimplemented!("test provider")
        }

        fn name(&self) -> &str {
            "test"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(TestProvider)
        }
    }

    #[tokio::test]
    async fn server_run_refuses_to_replace_live_socket() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("jcode.sock");
        let debug_socket_path = temp.path().join("jcode-debug.sock");
        let _listener = Listener::bind(&socket_path).expect("bind existing live socket");
        let provider: Arc<dyn Provider> = Arc::new(TestProvider);
        let server = Server::new_with_paths(provider, socket_path, debug_socket_path);

        let error = server
            .run()
            .await
            .expect_err("should refuse live socket takeover");
        assert!(
            error
                .to_string()
                .contains("Refusing to replace active server socket"),
            "unexpected error: {error:#}"
        );
    }
}

#[cfg(test)]
mod queue_tests {
    use super::{
        queue_soft_interrupt_for_session, register_session_interrupt_queue, SessionInterruptQueues,
    };
    use crate::agent::Agent;
    use crate::message::{Message, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{Mutex, RwLock};

    struct TestProvider;

    #[async_trait]
    impl Provider for TestProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            unimplemented!("test provider")
        }

        fn name(&self) -> &str {
            "test"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(TestProvider)
        }
    }

    async fn test_agent() -> Arc<Mutex<Agent>> {
        let provider: Arc<dyn Provider> = Arc::new(TestProvider);
        let registry = Registry::new(provider.clone()).await;
        Arc::new(Mutex::new(Agent::new(provider, registry)))
    }

    #[tokio::test]
    async fn queue_soft_interrupt_for_session_uses_registered_queue_when_agent_busy() {
        let agent = test_agent().await;
        let session_id = {
            let guard = agent.lock().await;
            guard.session_id().to_string()
        };
        let queue = {
            let guard = agent.lock().await;
            guard.soft_interrupt_queue()
        };
        let queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
        register_session_interrupt_queue(&queues, &session_id, queue.clone()).await;
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));

        let _busy_guard = agent.lock().await;
        let queued = queue_soft_interrupt_for_session(
            &session_id,
            "queued while busy".to_string(),
            false,
            &queues,
            &sessions,
        )
        .await;

        assert!(
            queued,
            "interrupt should queue even while agent lock is held"
        );
        let pending = queue.lock().expect("queue lock");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "queued while busy");
        assert!(!pending[0].urgent);
    }

    #[tokio::test]
    async fn queue_soft_interrupt_for_session_registers_queue_on_fallback_lookup() {
        let agent = test_agent().await;
        let session_id = {
            let guard = agent.lock().await;
            guard.session_id().to_string()
        };
        let queue = {
            let guard = agent.lock().await;
            guard.soft_interrupt_queue()
        };
        let queues: SessionInterruptQueues = Arc::new(RwLock::new(HashMap::new()));
        let sessions = Arc::new(RwLock::new(HashMap::from([(
            session_id.clone(),
            agent.clone(),
        )])));

        let queued = queue_soft_interrupt_for_session(
            &session_id,
            "fallback lookup".to_string(),
            true,
            &queues,
            &sessions,
        )
        .await;

        assert!(queued, "interrupt should queue via session fallback");
        assert!(
            queues.read().await.contains_key(&session_id),
            "fallback should cache the session queue for later busy sends"
        );
        let pending = queue.lock().expect("queue lock");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "fallback lookup");
        assert!(pending[0].urgent);
    }
}

/// Set custom socket path (sets JCODE_SOCKET env var)
pub fn set_socket_path(path: &str) {
    std::env::set_var("JCODE_SOCKET", path);
}

/// Spawn a server child process and wait until it signals readiness (socket bound).
///
/// Creates an anonymous pipe, passes the write-end fd to the child via
/// `JCODE_READY_FD`, and awaits a single byte on the read end.  The server
/// calls `signal_ready_fd()` after `Listener::bind()`, so the future resolves
/// the instant the socket is accepting connections -- no polling needed.
///
/// Falls back to a short poll loop if the pipe read times out (e.g. server
/// built without ready-fd support, or crash before bind).
#[cfg(unix)]
pub async fn spawn_server_notify(cmd: &mut std::process::Command) -> Result<std::process::Child> {
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::CommandExt;

    // Create a pipe: fds[0] = read end, fds[1] = write end.
    // Use pipe2 with O_CLOEXEC on the read end (parent keeps it).
    // The write end needs CLOEXEC cleared so it survives exec in the child.
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        anyhow::bail!("pipe() failed: {}", std::io::Error::last_os_error());
    }
    let read_fd = fds[0];
    let write_fd = fds[1];

    // Set CLOEXEC on the read end (parent only)
    unsafe {
        let flags = libc::fcntl(read_fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(read_fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }

    // Pass the write-end fd to the child and tell it the fd number.
    unsafe {
        cmd.pre_exec(move || {
            // Clear CLOEXEC on the write end so it survives exec
            let flags = libc::fcntl(write_fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(write_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
            libc::setsid();
            Ok(())
        });
    }
    cmd.env("JCODE_READY_FD", write_fd.to_string());

    let mut child = cmd.spawn()?;

    // Close our copy of the write end so we get EOF if the child dies.
    unsafe { libc::close(write_fd) };

    // Wait for the ready signal (or timeout / child death).
    let read_file = unsafe { std::fs::File::from_raw_fd(read_fd) };
    let mut async_file = tokio::fs::File::from_std(read_file);
    let mut buf = [0u8; 1];
    match tokio::time::timeout(
        Duration::from_secs(10),
        tokio::io::AsyncReadExt::read(&mut async_file, &mut buf),
    )
    .await
    {
        Ok(Ok(1)) => {
            crate::logging::info("Server signalled ready via pipe");
        }
        Ok(Ok(_)) => {
            if let Some(status) = child.try_wait()? {
                anyhow::bail!(server_start_error(&mut child, status));
            }
            crate::logging::info(
                "Server closed ready pipe without signalling; falling back to poll",
            );
            poll_for_socket(&socket_path(), Duration::from_secs(5)).await?;
        }
        Ok(Err(e)) => {
            crate::logging::info(&format!(
                "Ready pipe read error: {}; falling back to poll",
                e
            ));
            poll_for_socket(&socket_path(), Duration::from_secs(5)).await?;
        }
        Err(_) => {
            crate::logging::info("Timed out waiting for server ready signal; falling back to poll");
            poll_for_socket(&socket_path(), Duration::from_secs(5)).await?;
        }
    }

    if let Some(mut stderr) = child.stderr.take() {
        // The shared daemon outlives the spawning client. Keep draining the
        // stderr pipe after startup so later reloads cannot die on SIGPIPE
        // when they emit provider/model selection notices during boot.
        std::thread::spawn(move || {
            let mut sink = std::io::sink();
            let _ = std::io::copy(&mut stderr, &mut sink);
        });
    }

    Ok(child)
}

/// Simple poll loop waiting for the socket file to become connectable.
async fn poll_for_socket(path: &std::path::Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if crate::transport::is_socket_path(path) && Stream::connect(path).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    anyhow::bail!("Timed out waiting for socket {}", path.display());
}

#[cfg(unix)]
fn server_start_error(child: &mut std::process::Child, status: std::process::ExitStatus) -> String {
    use std::io::Read;

    let stderr_output = child
        .stderr
        .take()
        .and_then(|mut stderr| {
            let mut buf = String::new();
            stderr.read_to_string(&mut buf).ok()?;
            Some(buf)
        })
        .unwrap_or_default();
    if stderr_output.trim().is_empty() {
        format!(
            "Server exited before signalling ready ({}). Check logs at ~/.jcode/logs/",
            status
        )
    } else {
        format!(
            "Server exited before signalling ready ({}):\n{}",
            status,
            stderr_output.trim()
        )
    }
}

/// Idle timeout for the shared server when no clients are connected (5 minutes)
const IDLE_TIMEOUT_SECS: u64 = 300;

/// How often to check whether the embedding model can be unloaded.
const EMBEDDING_IDLE_CHECK_SECS: u64 = 30;

/// Default embedding idle unload threshold (15 minutes).
const EMBEDDING_IDLE_UNLOAD_DEFAULT_SECS: u64 = 15 * 60;

/// Write a single byte to the fd in `JCODE_READY_FD` and close it.
/// Called after socket bind so the parent process knows the server is
/// accepting connections.  The env var is cleared afterwards so child
/// processes (e.g. tool subprocesses) don't inherit a stale fd.
fn signal_ready_fd() {
    #[cfg(unix)]
    {
        use std::os::unix::io::FromRawFd;

        if let Ok(fd_str) = std::env::var("JCODE_READY_FD") {
            std::env::remove_var("JCODE_READY_FD");
            if let Ok(fd) = fd_str.parse::<i32>() {
                let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
                let _ = std::io::Write::write_all(&mut file, b"R");
                // file is dropped here which closes the fd
            }
        }
    }
}

/// Reload signal payload sent via in-process channel (replaces filesystem-based rebuild-signal)
#[derive(Clone, Debug)]
pub struct ReloadSignal {
    pub hash: String,
    pub triggering_session: Option<String>,
    pub prefer_selfdev_binary: bool,
    pub request_id: String,
}

#[derive(Clone, Debug)]
pub struct ReloadAck {
    pub hash: String,
    pub request_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReloadPhase {
    Starting,
    SocketReady,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReloadState {
    pub request_id: String,
    pub hash: String,
    pub phase: ReloadPhase,
    pub pid: u32,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ReloadState {
    fn path() -> PathBuf {
        reload_marker_path()
    }

    fn write(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = crate::storage::ensure_dir(parent);
        }
        let _ = crate::storage::write_json(&path, self);
    }

    pub fn load() -> Option<Self> {
        let path = Self::path();
        if !path.exists() {
            return None;
        }
        crate::storage::read_json(&path).ok()
    }
}

/// Global reload signal channel. The selfdev tool and debug commands fire this;
/// the server awaits it instead of polling the filesystem.
static RELOAD_SIGNAL: std::sync::OnceLock<(
    tokio::sync::watch::Sender<Option<ReloadSignal>>,
    tokio::sync::watch::Receiver<Option<ReloadSignal>>,
)> = std::sync::OnceLock::new();

static RELOAD_ACK: std::sync::OnceLock<(
    tokio::sync::watch::Sender<Option<ReloadAck>>,
    tokio::sync::watch::Receiver<Option<ReloadAck>>,
)> = std::sync::OnceLock::new();

fn reload_signal() -> &'static (
    tokio::sync::watch::Sender<Option<ReloadSignal>>,
    tokio::sync::watch::Receiver<Option<ReloadSignal>>,
) {
    RELOAD_SIGNAL.get_or_init(|| tokio::sync::watch::channel(None))
}

#[cfg(test)]
pub(crate) fn subscribe_reload_signal_for_tests(
) -> tokio::sync::watch::Receiver<Option<ReloadSignal>> {
    reload_signal().1.clone()
}

fn reload_ack() -> &'static (
    tokio::sync::watch::Sender<Option<ReloadAck>>,
    tokio::sync::watch::Receiver<Option<ReloadAck>>,
) {
    RELOAD_ACK.get_or_init(|| tokio::sync::watch::channel(None))
}

/// Send a reload signal to the server (called by selfdev tool / debug commands).
pub fn send_reload_signal(
    hash: String,
    triggering_session: Option<String>,
    prefer_selfdev_binary: bool,
) -> String {
    let request_id = crate::id::new_id("reload");
    let (tx, _) = reload_signal();
    let _ = tx.send(Some(ReloadSignal {
        hash,
        triggering_session,
        prefer_selfdev_binary,
        request_id: request_id.clone(),
    }));
    request_id
}

pub fn acknowledge_reload_signal(signal: &ReloadSignal) {
    let (tx, _) = reload_ack();
    let _ = tx.send(Some(ReloadAck {
        hash: signal.hash.clone(),
        request_id: signal.request_id.clone(),
    }));
}

pub async fn wait_for_reload_ack(
    request_id: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<ReloadAck> {
    let mut rx = reload_ack().1.clone();

    if let Some(ack) = rx.borrow_and_update().clone() {
        if ack.request_id == request_id {
            return Ok(ack);
        }
    }

    let request_id = request_id.to_string();
    tokio::time::timeout(timeout, async move {
        loop {
            rx.changed()
                .await
                .map_err(|_| anyhow::anyhow!("reload acknowledgement channel closed"))?;
            if let Some(ack) = rx.borrow_and_update().clone() {
                if ack.request_id == request_id {
                    return Ok(ack);
                }
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for reload acknowledgement"))?
}

fn debug_control_allowed() -> bool {
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

fn embedding_idle_unload_secs() -> u64 {
    std::env::var("JCODE_EMBEDDING_IDLE_UNLOAD_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(EMBEDDING_IDLE_UNLOAD_DEFAULT_SECS)
}

fn server_update_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    build::client_update_candidate(is_selfdev_session)
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
    let current_mtime = current_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    let current_canonical = current_exe
        .as_ref()
        .map(|path| canonicalize_or(path.clone()));

    let mut candidates = HashSet::new();
    for is_selfdev_session in [false, true] {
        if let Some((candidate, _label)) = server_update_candidate(is_selfdev_session) {
            candidates.insert(canonicalize_or(candidate));
        }
    }

    candidates.into_iter().any(|candidate| {
        let candidate_mtime = std::fs::metadata(&candidate)
            .ok()
            .and_then(|m| m.modified().ok());

        match (current_mtime, candidate_mtime) {
            (Some(current), Some(candidate_time)) => candidate_time > current,
            _ => current_canonical
                .as_ref()
                .map(|current| current != &candidate)
                .unwrap_or(false),
        }
    })
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
    /// Shared MCP server pool (processes shared across sessions)
    mcp_pool: Arc<crate::mcp::SharedMcpPool>,
    /// Graceful shutdown signals by session_id (stored outside agent mutex so they
    /// can be signaled without locking the agent during active tool execution)
    shutdown_signals: Arc<RwLock<HashMap<String, crate::agent::InterruptSignal>>>,
    /// Soft interrupt queues by session_id (stored outside agent mutex so swarm/debug
    /// notifications can be enqueued while an agent is actively processing)
    soft_interrupt_queues: SessionInterruptQueues,
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
            mcp_pool: Arc::new(crate::mcp::SharedMcpPool::from_default_config()),
            shutdown_signals: Arc::new(RwLock::new(HashMap::new())),
            soft_interrupt_queues: Arc::new(RwLock::new(HashMap::new())),
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

                    // Only check for conflicts when someone writes/edits (reads never conflict)
                    let is_write = matches!(touch.op, FileOp::Write | FileOp::Edit);
                    if is_write {
                        crate::logging::info(&format!(
                            "[conflict-check] WRITE by {} on {}, swarm_peers: {:?}",
                            &session_id[..8.min(session_id.len())],
                            path.display(),
                            swarm_session_ids
                                .iter()
                                .map(|s| &s[..8.min(s.len())])
                                .collect::<Vec<_>>()
                        ));
                    }
                    let previous_touches: Vec<FileAccess> = if is_write {
                        let touches = file_touches.read().await;
                        if let Some(accesses) = touches.get(&path) {
                            let result: Vec<FileAccess> = accesses
                                .iter()
                                .filter(|a| {
                                    a.session_id != session_id
                                        && swarm_session_ids.contains(&a.session_id)
                                        && matches!(a.op, FileOp::Write | FileOp::Edit)
                                })
                                .cloned()
                                .collect();
                            crate::logging::info(&format!(
                                "[conflict-check] {} prev write-touches from peers ({} total accesses)",
                                result.len(), accesses.len()
                            ));
                            result
                        } else {
                            crate::logging::info("[conflict-check] no touches for this path yet");
                            vec![]
                        }
                    } else {
                        vec![]
                    };

                    // If there are previous write conflicts from swarm members, send alerts
                    if !previous_touches.is_empty() {
                        crate::logging::info(&format!(
                            "[conflict-check] CONFLICT on {} — sending alerts",
                            path.display()
                        ));
                        let members = swarm_members.read().await;
                        let current_member = members.get(&session_id);
                        let current_name = current_member.and_then(|m| m.friendly_name.clone());
                        // Deduplicate previous touches by session (keep latest per agent)
                        let mut unique_by_session: std::collections::HashMap<&str, &FileAccess> =
                            std::collections::HashMap::new();
                        for prev in &previous_touches {
                            unique_by_session
                                .entry(&prev.session_id)
                                .and_modify(|existing| {
                                    if prev.timestamp > existing.timestamp {
                                        *existing = prev;
                                    }
                                })
                                .or_insert(prev);
                        }

                        // Alert the current agent about previous writers (one per agent)
                        if let Some(member) = current_member {
                            for prev in unique_by_session.values() {
                                let prev_member = members.get(&prev.session_id);
                                let prev_name = prev_member.and_then(|m| m.friendly_name.clone());
                                let alert_msg = format!(
                                    "⚠️ File conflict: {} — {} previously {} this file{}",
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
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-conflict soft interrupt for session {}",
                                        session_id
                                    ));
                                }
                            }
                        }

                        // Alert previous agents about the current touch (one per agent)
                        let mut notified_sessions: std::collections::HashSet<&str> =
                            std::collections::HashSet::new();
                        for prev in &previous_touches {
                            if !notified_sessions.insert(&prev.session_id) {
                                continue;
                            }
                            if let Some(prev_member) = members.get(&prev.session_id) {
                                let alert_msg = format!(
                                    "⚠️ File conflict: {} — {} just {} this file you previously worked with{}",
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

                                if !queue_soft_interrupt_for_session(
                                    &prev.session_id,
                                    alert_msg.clone(),
                                    false,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-conflict soft interrupt for session {}",
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

        if socket_has_live_listener(&self.socket_path).await {
            anyhow::bail!(
                "Refusing to replace active server socket at {}",
                self.socket_path.display()
            );
        }

        // Remove existing sockets (uses transport abstraction for cross-platform cleanup)
        crate::transport::remove_socket(&self.socket_path);
        crate::transport::remove_socket(&self.debug_socket_path);

        let main_listener = Listener::bind(&self.socket_path)?;
        let debug_listener = Listener::bind(&self.debug_socket_path)?;

        #[cfg(unix)]
        {
            // Server reload uses exec. Force the published listener fds to close
            // across exec so the replacement daemon can safely rebind them.
            mark_close_on_exec(&main_listener);
            mark_close_on_exec(&debug_listener);
        }

        // We successfully rebound the socket pair, so any reload handoff is complete.
        clear_reload_marker();

        // Restrict socket files to owner-only so other local users cannot connect.
        let _ = crate::platform::set_permissions_owner_only(&self.socket_path);
        let _ = crate::platform::set_permissions_owner_only(&self.debug_socket_path);

        // Signal readiness to parent process via JCODE_READY_FD (if set).
        // The parent creates a pipe and passes the write-end fd number so we
        // can notify it the moment the socket is bound, avoiding poll-based
        // startup detection.
        signal_ready_fd();

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

        // Register server in the registry so session picker and clients can discover it.
        // Do registration inline (fast) but defer cleanup to background (probes sockets).
        {
            let mut registry = crate::registry::ServerRegistry::load()
                .await
                .unwrap_or_default();
            let info = crate::registry::ServerInfo {
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
            registry.register(info);
            let _ = registry.save().await;
            crate::logging::info(&format!(
                "Registered as {} in server registry",
                self.identity.display_name(),
            ));
        }

        // Cleanup stale registry entries in background (probes sockets, can be slow)
        tokio::spawn(async {
            if let Ok(mut registry) = crate::registry::ServerRegistry::load().await {
                let _ = registry.cleanup_stale().await;
                let _ = registry.save().await;
            }
        });

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
        tokio::spawn(async move {
            await_reload_signal(
                signal_sessions,
                signal_swarm_members,
                signal_shutdown_signals,
            )
            .await;
        });

        // Log when we receive SIGTERM for debugging
        #[cfg(unix)]
        {
            let sigterm_server_name = self.identity.name.clone();
            tokio::spawn(async move {
                use tokio::signal::unix::{signal, SignalKind};
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

        // Spawn ambient mode background loop if enabled
        if let Some(ref runner) = self.ambient_runner {
            let ambient_handle = runner.clone();
            let ambient_provider = Arc::clone(&self.provider);
            crate::logging::info("Ambient mode enabled - starting background loop");
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
                                swarm_event_tx,
                                server_name,
                                server_icon,
                                mcp_pool,
                                shutdown_signals,
                                soft_interrupt_queues,
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
                        let mcp_pool = Some(debug_mcp_pool.clone());
                        let soft_interrupt_queues = Arc::clone(&debug_soft_interrupt_queues);

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

                *client_count.write().await += 1;

                crate::logging::info(&format!(
                    "Gateway client connected: {} ({})",
                    gw_client.device_name, gw_client.device_id
                ));

                tokio::spawn(async move {
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

/// Client for connecting to a running server
pub struct Client {
    reader: BufReader<ReadHalf>,
    writer: WriteHalf,
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
            system_reminder: None,
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
            retry_after_secs: None,
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
