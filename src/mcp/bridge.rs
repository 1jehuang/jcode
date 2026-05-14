//! Bidirectional MCP Bridge — connects internal tool system with external MCP servers
//! while simultaneously exposing CarpAI as an MCP server.
//!
//! Architecture:
//! ```text
//! ┌────────────────────────────────────────────────────┐
//! │                  McpBridge                         │
//! ├────────────────────────────────────────────────────┤
//! │  ┌──────────────┐   ┌──────────────────────────┐   │
//! │  │ MCP Server   │   │ MCP Client Manager       │   │
//! │  │ (src/mcp/    │   │ (external servers pool)  │   │
//! │  │  server.rs)  │   │                          │   │
//! │  └──────┬───────┘   └──────────┬───────────────┘   │
//! │         │                      │                    │
//! │         ▼                      ▼                    │
//! │  ┌──────────────────────────────────────────────┐   │
//! │  │          Tool Registry (unified)             │   │
//! │  │  Internal tools + MCP-proxied tools          │   │
//! │  └──────────────────────────────────────────────┘   │
//! │                                                      │
//! │  ┌──────────────────────────────────────────────┐   │
//! │  │          Event Bus (tool calls,              │   │
//! │  │     MCP tool registrations, health)          │   │
//! │  └──────────────────────────────────────────────┘   │
//! └────────────────────────────────────────────────────┘
//! ```

use crate::mcp::pool::SharedMcpPool;
use crate::mcp::protocol::McpConfig;
use crate::mcp::{McpManager, McpServer, McpServerConfig as ServerConfig};
use crate::tool::Registry;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Capabilities that the bridge supports for bidirectional MCP
#[derive(Debug, Clone, Default)]
pub struct BridgeCapabilities {
    /// Whether the bridge can act as an MCP server (expose tools)
    pub server_mode: bool,
    /// Whether the bridge can act as an MCP client (connect to external servers)
    pub client_mode: bool,
    /// Whether the bridge supports tool exchange between server and client realms
    pub tool_exchange: bool,
}

/// Configuration for the bidirectional MCP bridge
#[derive(Debug, Clone)]
pub struct McpBridgeConfig {
    /// Server configuration (for server mode)
    pub server: ServerConfig,
    /// Whether to enable client mode (connect to external MCP servers)
    pub client_enabled: bool,
    /// Whether to auto-connect configured MCP servers on bridge init
    pub auto_connect: bool,
    /// Custom server name for the MCP server
    pub server_name: String,
}

impl Default for McpBridgeConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            client_enabled: true,
            auto_connect: true,
            server_name: "carpai".to_string(),
        }
    }
}

/// Unified bidirectional MCP bridge
pub struct McpBridge {
    /// The MCP server (exposes CarpAI tools to external clients)
    server: Option<McpServer>,
    /// The MCP Manager (connects to external MCP servers)
    manager: Arc<RwLock<McpManager>>,
    /// Shared MCP pool (for daemon mode)
    shared_pool: Option<Arc<SharedMcpPool>>,
    /// Configuration
    config: McpBridgeConfig,
    /// Whether the bridge is initialized
    initialized: bool,
}

impl McpBridge {
    /// Create a new bidirectional MCP bridge
    pub fn new(config: McpBridgeConfig) -> Self {
        let manager = Arc::new(RwLock::new(McpManager::new()));

        Self {
            server: None,
            manager,
            shared_pool: None,
            config,
            initialized: false,
        }
    }

    /// Initialize the bridge with a tool registry
    pub async fn init(&mut self, registry: Registry) -> Result<()> {
        // Build the MCP server
        if self.config.server_mode_enabled() {
            let server_config = ServerConfig {
                expose_resources: self.config.server.expose_resources,
                server_name: self.config.server_name.clone(),
                extra_tools: self.config.server.extra_tools.clone(),
            };
            let mut server = McpServer::new(registry.clone());
            server = server.with_config(server_config);
            self.server = Some(server);
        }

        // Initialize shared pool if client mode is enabled
        if self.config.client_enabled {
            let pool = crate::mcp::pool::init_shared_pool().await;
            self.shared_pool = Some(pool.clone());

            let session_id = format!("bridge-{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0));

            let manager = McpManager::with_shared_pool(pool, session_id);

            // Auto-connect configured MCP servers
            if self.config.auto_connect {
                let config = McpConfig::load();
                if !config.servers.is_empty() {
                    eprintln!(
                        "[MCP] Bridge: found {} configured MCP server(s)",
                        config.servers.len()
                    );
                } else {
                    eprintln!("[MCP] Bridge: no MCP servers configured. Add to ~/.jcode/mcp.json");
                }
            }

            *self.manager.write().await = manager;
        }

        self.initialized = true;
        Ok(())
    }

    /// Start serving as an MCP server (blocking until stdin closes)
    pub async fn serve(&mut self) -> Result<()> {
        if !self.initialized {
            anyhow::bail!("Bridge not initialized. Call init() first.");
        }

        if let Some(ref mut server) = self.server {
            eprintln!("[MCP] Bridge: starting MCP server (Content-Length framing)...");
            server.serve().await
        } else {
            anyhow::bail!("Server mode not enabled in config.");
        }
    }

    /// Get the MCP manager for external client operations
    pub fn manager(&self) -> Arc<RwLock<McpManager>> {
        self.manager.clone()
    }

    /// Get the shared pool reference
    pub fn shared_pool(&self) -> Option<Arc<SharedMcpPool>> {
        self.shared_pool.clone()
    }

    /// Get bridge status info
    pub async fn status(&self) -> BridgeStatus {
        let connected_servers = if self.config.client_enabled {
            self.manager.read().await.connected_servers().await
        } else {
            Vec::new()
        };

        BridgeStatus {
            server_mode: self.config.server_mode_enabled(),
            client_mode: self.config.client_enabled,
            connected_servers: connected_servers.len(),
            server_names: connected_servers,
            initialized: self.initialized,
        }
    }
}

impl McpBridgeConfig {
    fn server_mode_enabled(&self) -> bool {
        self.server.expose_resources || !self.server_name.is_empty()
    }
}

/// Bridge status report
#[derive(Debug, Clone)]
pub struct BridgeStatus {
    pub server_mode: bool,
    pub client_mode: bool,
    pub connected_servers: usize,
    pub server_names: Vec<String>,
    pub initialized: bool,
}

impl std::fmt::Display for BridgeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "MCP Bridge Status:")?;
        writeln!(f, "  Initialized: {}", self.initialized)?;
        writeln!(f, "  Server mode: {}", if self.server_mode { "active" } else { "disabled" })?;
        writeln!(f, "  Client mode: {}", if self.client_mode { "active" } else { "disabled" })?;
        writeln!(f, "  External servers: {}", self.connected_servers)?;
        for name in &self.server_names {
            writeln!(f, "    - {}", name)?;
        }
        Ok(())
    }
}
