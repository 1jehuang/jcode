use serde::{Serialize, Deserialize};
use std::time::{Instant, Duration};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterNode {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub role: NodeRole,
    pub status: NodeStatus,
    pub capabilities: Vec<String>,
    pub metadata: NodeMetadata,
    pub joined_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip)]
    pub last_heartbeat: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeRole {
    Leader,
    Follower,
    Candidate,
    Observer,
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeRole::Leader => write!(f, "👑 Leader"),
            NodeRole::Follower => write!(f, "🔄 Follower"),
            NodeRole::Candidate => write!(f, "⚡ Candidate"),
            NodeRole::Observer => write!(f, "👁 Observer"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeStatus {
    Online,
    Offline,
    Degraded,
    Starting,
    Stopping,
}

impl Default for NodeStatus {
    fn default() -> Self { NodeStatus::Online }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetadata {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub cpu_cores: usize,
    pub memory_mb: u64,
    pub load_average: f64,
    pub custom: std::collections::HashMap<String, String>,
}

impl Default for NodeMetadata {
    fn default() -> Self {
        NodeMetadata {
            version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
            memory_mb: 0,
            load_average: 0.0,
            custom: std::collections::HashMap::new(),
        }
    }
}

impl ClusterNode {
    pub fn new(host: &str, port: u16) -> Self {
        ClusterNode {
            id: Uuid::new_v4().to_string(),
            host: host.to_string(),
            port,
            role: NodeRole::Follower,
            status: NodeStatus::Online,
            capabilities: vec![],
            metadata: NodeMetadata::default(),
            joined_at: chrono::Utc::now(),
            last_heartbeat: Some(Instant::now()),
        }
    }

    pub fn address(&self) -> String { format!("{}:{}", self.host, self.port) }

    pub fn is_healthy(&self) -> bool {
        self.status == NodeStatus::Online
            && self.last_heartbeat
                .map(|h| h.elapsed() < Duration::from_secs(30))
                .unwrap_or(false)
    }

    pub fn with_capabilities(mut self, caps: Vec<&str>) -> Self {
        self.capabilities = caps.into_iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn heartbeat(&mut self) { self.last_heartbeat = Some(Instant::now()); }
}
