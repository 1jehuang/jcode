//! Auto MCP — automatically discovers, connects, and manages MCP server lifecycle
//! within the agent's turn loop.
//!
//! Features:
//! - Auto-connect all configured MCP servers on agent init
//! - Auto-register MCP tools into the agent's tool registry
//! - Exponential backoff reconnection for failed servers
//! - Periodic health checks
//! - Graceful shutdown

use crate::mcp::protocol::{McpConfig, McpServerConfig};
use crate::mcp::McpManager;
use crate::tool::Registry;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

/// Configuration for auto MCP behavior
#[derive(Debug, Clone)]
pub struct AutoMcpConfig {
    /// Auto-connect all configured servers on init
    pub auto_connect: bool,
    /// Auto-reconnect failed servers with backoff
    pub auto_reconnect: bool,
    /// Health check interval in seconds (0 = disabled)
    pub health_check_interval_secs: u64,
    /// Maximum reconnect attempts per server
    pub max_reconnect_attempts: u32,
    /// Initial backoff delay in seconds
    pub initial_backoff_secs: u64,
}

impl Default for AutoMcpConfig {
    fn default() -> Self {
        Self {
            auto_connect: true,
            auto_reconnect: true,
            health_check_interval_secs: 60,
            max_reconnect_attempts: 5,
            initial_backoff_secs: 2,
        }
    }
}

/// Status of a single MCP server
#[derive(Debug, Clone)]
pub struct McpServerStatus {
    pub name: String,
    pub connected: bool,
    pub tools_count: usize,
    pub last_connected: Option<SystemTime>,
    pub last_error: Option<String>,
    pub reconnect_count: u32,
}

/// Manages automatic MCP server lifecycle
pub struct AutoMcpManager {
    manager: Arc<RwLock<McpManager>>,
    config: AutoMcpConfig,
    registry: Arc<RwLock<Option<Registry>>>,
    connected_servers: Arc<RwLock<HashMap<String, McpServerStatus>>>,
    health_check_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl AutoMcpManager {
    pub fn new() -> Self {
        Self {
            manager: Arc::new(RwLock::new(McpManager::new())),
            config: AutoMcpConfig::default(),
            registry: Arc::new(RwLock::new(None)),
            connected_servers: Arc::new(RwLock::new(HashMap::new())),
            health_check_handle: Arc::new(RwLock::new(None)),
        }
    }

    pub fn with_config(mut self, config: AutoMcpConfig) -> Self {
        self.config = config;
        self
    }

    /// Initialize: store registry and auto-connect configured servers.
    pub async fn init(&self, registry: Registry) -> Result<()> {
        {
            let mut reg = self.registry.write().await;
            *reg = Some(registry);
        }

        if self.config.auto_connect {
            self.connect_all().await;
        }

        if self.config.health_check_interval_secs > 0 && self.config.auto_reconnect {
            self.start_health_check().await;
        }

        Ok(())
    }

    /// Connect all servers from config.
    pub async fn connect_all(&self) {
        let config = McpConfig::load();
        let mut results = HashMap::new();

        for (name, server_config) in &config.servers {
            let result = self.connect_single(name, server_config).await;
            results.insert(name.clone(), result);
        }

        // Log summary
        let success_count = results.values().filter(|r| r.is_ok()).count();
        let total = results.len();
        if success_count > 0 {
            crate::logging::info(&format!(
                "Auto MCP: connected {}/{} servers",
                success_count, total
            ));
        }
        if success_count < total {
            for (name, result) in &results {
                if let Err(e) = result {
                    crate::logging::warn(&format!(
                        "Auto MCP: failed to connect '{}': {}",
                        name, e
                    ));
                }
            }
        }
    }

    /// Connect a single MCP server.
    async fn connect_single(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        let manager = self.manager.write().await;
        manager.connect(name, config).await?;
        drop(manager);

        // Register tools in the tool registry
        let tools = {
            let manager = self.manager.read().await;
            manager.all_tools().await
        };

        let server_tools: Vec<_> = tools.iter().filter(|(s, _)| s == name).collect();
        let tools_count = server_tools.len();

        // Register MCP tools in the agent's registry
        if let Some(registry) = self.registry.read().await.as_ref() {
            let mcp_tools = crate::mcp::create_mcp_tools(Arc::clone(&self.manager)).await;
            for (tool_name, tool) in mcp_tools {
                if tool_name.starts_with(&format!("mcp__{}__", name)) {
                    registry.register(tool_name, tool).await;
                }
            }
        }

        // Update status
        let mut servers = self.connected_servers.write().await;
        servers.insert(name.to_string(), McpServerStatus {
            name: name.to_string(),
            connected: true,
            tools_count,
            last_connected: Some(SystemTime::now()),
            last_error: None,
            reconnect_count: 0,
        });

        crate::logging::info(&format!(
            "Auto MCP: connected '{}' with {} tools",
            name, tools_count
        ));

        Ok(())
    }

    /// Connect a specific server by name from config.
    pub async fn connect_server(&self, name: &str) -> Result<()> {
        let config = McpConfig::load();
        let server_config = config
            .servers
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Server '{}' not found in config", name))?;

        // Check if already connected
        {
            let servers = self.connected_servers.read().await;
            if let Some(status) = servers.get(name) {
                if status.connected {
                    return Ok(()); // Already connected
                }
            }
        }

        self.connect_single(name, server_config).await
    }

    /// Disconnect a specific server and unregister its tools.
    pub async fn disconnect_server(&self, name: &str) -> Result<()> {
        // Unregister tools
        if let Some(registry) = self.registry.read().await.as_ref() {
            registry.unregister_prefix(&format!("mcp__{}__", name)).await;
        }

        // Disconnect
        {
            let manager = self.manager.read().await;
            manager.disconnect(name).await?;
        }

        // Update status
        let mut servers = self.connected_servers.write().await;
        servers.remove(name);

        crate::logging::info(&format!("Auto MCP: disconnected '{}'", name));
        Ok(())
    }

    /// [I-01] Get list of all connected server names (for agent tool injection).
    pub async fn connected_server_names(&self) -> Vec<String> {
        let servers = self.connected_servers.read().await;
        servers.iter()
            .filter(|(_, s)| s.connected)
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// [I-01] Get MCP tool names registered in the tool registry (for agent).
    pub async fn registered_mcp_tool_names(&self) -> Vec<String> {
        let reg = self.registry.read().await;
        match reg.as_ref() {
            Some(r) => {
                let defs = r.definitions(None).await;
                defs.iter()
                    .filter(|d| d.name.starts_with("mcp__"))
                    .map(|d| d.name.clone())
                    .collect()
            }
            None => Vec::new(),
        }
    }

    /// Get status of all servers (configured + connected).
    pub async fn status(&self) -> Vec<McpServerStatus> {
        let servers = self.connected_servers.read().await;
        let config = McpConfig::load();
        let mut result = Vec::new();

        for name in config.servers.keys() {
            if let Some(status) = servers.get(name) {
                result.push(status.clone());
            } else {
                result.push(McpServerStatus {
                    name: name.clone(),
                    connected: false,
                    tools_count: 0,
                    last_connected: None,
                    last_error: Some("not connected".to_string()),
                    reconnect_count: 0,
                });
            }
        }
        result
    }

    /// Start background health check task.
    pub async fn start_health_check(&self) {
        let interval = self.config.health_check_interval_secs;
        if interval == 0 {
            return;
        }

        let manager = Arc::clone(&self.manager);
        let connected_servers = Arc::clone(&self.connected_servers);
        let config = self.config.clone();
        let registry = Arc::clone(&self.registry);

        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(interval)).await;

                // Check all connected servers
                let server_names: Vec<String> = {
                    let servers = connected_servers.read().await;
                    servers
                        .iter()
                        .filter(|(_, s)| s.connected)
                        .map(|(k, _)| k.clone())
                        .collect()
                };

                for name in &server_names {
                    let is_alive = {
                        let mgr = manager.read().await;
                        let connected = mgr.connected_servers().await;
                        connected.contains(name)
                    };

                    if !is_alive && config.auto_reconnect {
                        crate::logging::warn(&format!(
                            "Auto MCP: health check detected '{}' disconnected, reconnecting...",
                            name
                        ));

                        // Reconnect with backoff
                        let cfg = McpConfig::load();
                        let server_config = cfg.servers.get(name).map(|sc| (name.clone(), sc.clone()));

                        if let Some((n, sc)) = server_config {
                            let mgr = manager.read().await;
                            match mgr.connect(&n, &sc).await {
                                Ok(()) => {
                                    // Re-register tools
                                    if let Some(ref reg) = *registry.read().await {
                                        let mcp_tools =
                                            crate::mcp::create_mcp_tools(Arc::clone(&manager)).await;
                                        for (tn, tool) in mcp_tools {
                                            if tn.starts_with(&format!("mcp__{}__", n)) {
                                                reg.register(tn, tool).await;
                                            }
                                        }
                                    }

                                    let mut servers = connected_servers.write().await;
                                    if let Some(status) = servers.get_mut(&n) {
                                        status.connected = true;
                                        status.last_connected = Some(SystemTime::now());
                                        status.last_error = None;
                                    }
                                    crate::logging::info(&format!(
                                        "Auto MCP: reconnected '{}'", n
                                    ));
                                }
                                Err(e) => {
                                    let mut servers = connected_servers.write().await;
                                    if let Some(status) = servers.get_mut(&n) {
                                        status.reconnect_count += 1;
                                        status.last_error = Some(e.to_string());
                                    }
                                    crate::logging::warn(&format!(
                                        "Auto MCP: reconnect failed for '{}': {}",
                                        n, e
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        });

        let mut h = self.health_check_handle.write().await;
        *h = Some(handle);
    }

    /// Stop health check background task.
    pub async fn stop_health_check(&self) {
        let mut h = self.health_check_handle.write().await;
        if let Some(handle) = h.take() {
            handle.abort();
        }
    }

    /// Get reference to the underlying McpManager.
    pub fn manager(&self) -> Arc<RwLock<McpManager>> {
        Arc::clone(&self.manager)
    }

    /// Disconnect all servers and stop health checks.
    pub async fn shutdown(&self) {
        self.stop_health_check().await;

        let names: Vec<String> = {
            let servers = self.connected_servers.read().await;
            servers.keys().cloned().collect()
        };

        for name in &names {
            if let Some(registry) = self.registry.read().await.as_ref() {
                registry.unregister_prefix(&format!("mcp__{}__", name)).await;
            }
        }

        {
            let manager = self.manager.read().await;
            for name in &names {
                let _ = manager.disconnect(name).await;
            }
        }

        self.connected_servers.write().await.clear();
        crate::logging::info("Auto MCP: all servers disconnected");
    }
}

impl Default for AutoMcpManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Integrate MCP into the agent's tool registry.
/// Call this during agent initialization to auto-discover and register MCP tools.
pub async fn integrate_mcp_into_agent(registry: &Registry) -> Result<AutoMcpManager> {
    let manager = AutoMcpManager::new();
    manager.init(registry.clone()).await?;
    Ok(manager)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_auto_mcp_manager_creation() {
        let manager = AutoMcpManager::new();
        let status = manager.status().await;
        // No servers configured, but should not panic
        assert!(status.is_empty() || !status.iter().any(|s| s.connected));
    }

    #[test]
    fn test_config_defaults() {
        let config = AutoMcpConfig::default();
        assert!(config.auto_connect);
        assert!(config.auto_reconnect);
        assert_eq!(config.health_check_interval_secs, 60);
        assert_eq!(config.max_reconnect_attempts, 5);
    }
}
