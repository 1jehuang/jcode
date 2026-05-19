//! Cluster Configuration Management
//!
//! Provides configuration structures and utilities for distributed cluster setup.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Cluster configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Whether cluster mode is enabled
    pub enabled: bool,

    /// This node's configuration
    pub node: NodeConfig,

    /// Known peer nodes (for initial bootstrap)
    pub peers: Vec<PeerConfig>,

    /// Election configuration
    pub election: ElectionConfig,

    /// Heartbeat configuration
    pub heartbeat: HeartbeatConfig,

    /// Network configuration
    pub network: NetworkConfig,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node: NodeConfig::default(),
            peers: Vec::new(),
            election: ElectionConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

impl ClusterConfig {
    /// Create a new cluster config with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable cluster mode
    pub fn enable(mut self) -> Self {
        self.enabled = true;
        self
    }

    /// Set node configuration
    pub fn with_node(mut self, node: NodeConfig) -> Self {
        self.node = node;
        self
    }

    /// Add a peer node
    pub fn with_peer(mut self, peer: PeerConfig) -> Self {
        self.peers.push(peer);
        self
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        // Validate node config
        self.node.validate()?;

        // Validate peers
        for (i, peer) in self.peers.iter().enumerate() {
            peer.validate().map_err(|e| format!("Peer {} invalid: {}", i, e))?;
        }

        // Check for duplicate peer addresses
        let mut addresses = std::collections::HashSet::new();
        for peer in &self.peers {
            if !addresses.insert(peer.address.clone()) {
                return Err(format!("Duplicate peer address: {}", peer.address));
            }
        }

        Ok(())
    }

    /// Load configuration from file
    pub fn from_file(path: &PathBuf) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;

        let config: ClusterConfig = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config: {}", e))?;

        config.validate()?;
        Ok(config)
    }

    /// Save configuration to file
    pub fn to_file(&self, path: &PathBuf) -> Result<(), String> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;

        std::fs::write(path, content)
            .map_err(|e| format!("Failed to write config file: {}", e))?;

        Ok(())
    }
}

/// Node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// Unique node identifier (auto-generated if empty)
    pub id: Option<String>,

    /// Node host address
    pub host: String,

    /// Node port
    pub port: u16,

    /// Node role preference (optional)
    pub preferred_role: Option<NodeRolePreference>,

    /// Node metadata
    pub metadata: NodeMetadataConfig,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            id: None,
            host: "127.0.0.1".to_string(),
            port: 9000,
            preferred_role: None,
            metadata: NodeMetadataConfig::default(),
        }
    }
}

impl NodeConfig {
    /// Get socket address
    pub fn socket_addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:9000".parse().unwrap())
    }

    /// Validate node configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("Port cannot be 0".to_string());
        }

        if self.host.is_empty() {
            return Err("Host cannot be empty".to_string());
        }

        Ok(())
    }
}

/// Node role preference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeRolePreference {
    Leader,
    Follower,
    Observer,
}

/// Node metadata configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetadataConfig {
    /// Custom labels
    pub labels: std::collections::HashMap<String, String>,

    /// Region/zone information
    pub region: Option<String>,

    /// Availability zone
    pub zone: Option<String>,
}

impl Default for NodeMetadataConfig {
    fn default() -> Self {
        Self {
            labels: std::collections::HashMap::new(),
            region: None,
            zone: None,
        }
    }
}

/// Peer node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    /// Peer address (host:port)
    pub address: String,

    /// Optional peer ID (for verification)
    pub id: Option<String>,

    /// Connection timeout override
    pub timeout_ms: Option<u64>,
}

impl PeerConfig {
    /// Create a new peer config
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            id: None,
            timeout_ms: None,
        }
    }

    /// Validate peer configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.address.is_empty() {
            return Err("Peer address cannot be empty".to_string());
        }

        // Try to parse as socket address
        if self.address.parse::<SocketAddr>().is_err() {
            return Err(format!("Invalid peer address format: {}", self.address));
        }

        Ok(())
    }
}

/// Election configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElectionConfig {
    /// Base election timeout in milliseconds
    pub election_timeout_ms: u64,

    /// Random jitter range in milliseconds
    pub election_jitter_ms: u64,

    /// Minimum number of nodes for quorum
    pub min_quorum_size: usize,
}

impl Default for ElectionConfig {
    fn default() -> Self {
        Self {
            election_timeout_ms: 150,
            election_jitter_ms: 150,
            min_quorum_size: 2,
        }
    }
}

impl ElectionConfig {
    /// Get election timeout duration
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.election_timeout_ms)
    }

    /// Get maximum jitter duration
    pub fn max_jitter(&self) -> Duration {
        Duration::from_millis(self.election_jitter_ms)
    }
}

/// Heartbeat configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// Heartbeat interval in milliseconds
    pub interval_ms: u64,

    /// Heartbeat timeout in milliseconds (when to consider node dead)
    pub timeout_ms: u64,

    /// Maximum missed heartbeats before marking unhealthy
    pub max_missed: u32,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_ms: 50,
            timeout_ms: 150,
            max_missed: 3,
        }
    }
}

impl HeartbeatConfig {
    /// Get heartbeat interval duration
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms)
    }

    /// Get heartbeat timeout duration
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Connection timeout in milliseconds
    pub connect_timeout_ms: u64,

    /// Request timeout in milliseconds
    pub request_timeout_ms: u64,

    /// Maximum retry attempts
    pub max_retries: u32,

    /// Retry backoff base in milliseconds
    pub retry_backoff_ms: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 5000,
            request_timeout_ms: 10000,
            max_retries: 3,
            retry_backoff_ms: 1000,
        }
    }
}

impl NetworkConfig {
    /// Get connection timeout duration
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }

    /// Get request timeout duration
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ClusterConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.node.port, 9000);
    }

    #[test]
    fn test_enable_cluster() {
        let config = ClusterConfig::new().enable();
        assert!(config.enabled);
    }

    #[test]
    fn test_add_peer() {
        let config = ClusterConfig::new()
            .with_peer(PeerConfig::new("127.0.0.1:9001"));
        assert_eq!(config.peers.len(), 1);
    }

    #[test]
    fn test_validate_disabled_cluster() {
        let config = ClusterConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_port() {
        let config = ClusterConfig {
            enabled: true,
            node: NodeConfig {
                port: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_duplicate_peers() {
        let config = ClusterConfig {
            enabled: true,
            peers: vec![
                PeerConfig::new("127.0.0.1:9001"),
                PeerConfig::new("127.0.0.1:9001"),
            ],
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_peer_validation() {
        let valid_peer = PeerConfig::new("127.0.0.1:9001");
        assert!(valid_peer.validate().is_ok());

        let invalid_peer = PeerConfig::new("invalid-address");
        assert!(invalid_peer.validate().is_err());
    }

    #[test]
    fn test_election_config_durations() {
        let config = ElectionConfig {
            election_timeout_ms: 200,
            election_jitter_ms: 100,
            ..Default::default()
        };
        assert_eq!(config.timeout(), Duration::from_millis(200));
        assert_eq!(config.max_jitter(), Duration::from_millis(100));
    }

    #[test]
    fn test_heartbeat_config_durations() {
        let config = HeartbeatConfig {
            interval_ms: 100,
            timeout_ms: 300,
            ..Default::default()
        };
        assert_eq!(config.interval(), Duration::from_millis(100));
        assert_eq!(config.timeout(), Duration::from_millis(300));
    }

    #[test]
    fn test_network_config_durations() {
        let config = NetworkConfig {
            connect_timeout_ms: 3000,
            request_timeout_ms: 5000,
            ..Default::default()
        };
        assert_eq!(config.connect_timeout(), Duration::from_millis(3000));
        assert_eq!(config.request_timeout(), Duration::from_millis(5000));
    }
}
