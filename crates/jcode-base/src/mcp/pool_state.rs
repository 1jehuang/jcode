#[derive(Clone)]
struct FailedConnectRecord {
    message: String,
    failed_at: Instant,
}

enum ConnectAttempt {
    Failed(String),
    Connected,
    Leader(Arc<SharedConnectionAttempt>),
    Wait(Arc<SharedConnectionAttempt>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum PoolReferenceOwner {
    Session(String),
    Manager(u64),
}

enum DeferredReferenceRelease {
    Manager(u64),
    Claim(u64, String),
}

/// Global shared pool of MCP server processes.
///
/// Only one pool exists per jcode daemon. It owns the child processes
/// and hands out cheap `McpHandle` clones to sessions.
pub struct SharedMcpPool {
    admission: RwLock<()>,
    cleanup_pending: std::sync::atomic::AtomicBool,
    clients: Mutex<HashMap<String, Arc<Mutex<McpClient>>>>,
    handles: RwLock<HashMap<String, McpHandle>>,
    config: RwLock<McpConfig>,
    /// Directory against which the pool's default config was resolved. Keep it
    /// stable across reloads because the daemon may serve sessions in many dirs.
    config_dir: Option<std::path::PathBuf>,
    ref_counts: Mutex<HashMap<String, usize>>,
    session_refs: std::sync::Mutex<HashMap<PoolReferenceOwner, std::collections::HashSet<String>>>,
    released_references: std::sync::Mutex<Vec<DeferredReferenceRelease>>,
    connecting: SharedConnecting,
    last_errors: RwLock<HashMap<String, FailedConnectRecord>>,
}
