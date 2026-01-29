//! MCP Manager - manages multiple MCP server connections

#![allow(dead_code)]

use super::client::McpClient;
use super::protocol::{McpConfig, McpServerConfig, McpToolDef};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Manages multiple MCP server connections
pub struct McpManager {
    clients: Arc<RwLock<HashMap<String, McpClient>>>,
    config: McpConfig,
}

impl McpManager {
    /// Create a new manager and load config
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            config: McpConfig::load(),
        }
    }

    /// Create manager with specific config
    pub fn with_config(config: McpConfig) -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Connect to all configured servers
    /// Returns number of successful connections and list of failures
    pub async fn connect_all(&self) -> Result<(usize, Vec<(String, String)>)> {
        let mut successes = 0;
        let mut failures = Vec::new();
        
        for (name, config) in &self.config.servers {
            match self.connect(name, config).await {
                Ok(()) => {
                    successes += 1;
                }
                Err(e) => {
                    let error_msg = format!("{:#}", e);
                    crate::logging::error(&format!(
                        "Failed to connect to MCP server '{}': {}",
                        name, error_msg
                    ));
                    failures.push((name.clone(), error_msg));
                }
            }
        }
        Ok((successes, failures))
    }

    /// Connect to a specific server
    pub async fn connect(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        let client = McpClient::connect(name.to_string(), config)
            .await
            .with_context(|| format!("Failed to connect to MCP server '{}'", name))?;

        let mut clients = self.clients.write().await;
        clients.insert(name.to_string(), client);
        Ok(())
    }

    /// Disconnect from a server
    pub async fn disconnect(&self, name: &str) -> Result<()> {
        let mut clients = self.clients.write().await;
        if let Some(mut client) = clients.remove(name) {
            client.shutdown().await;
        }
        Ok(())
    }

    /// Disconnect from all servers
    pub async fn disconnect_all(&self) {
        let mut clients = self.clients.write().await;
        for (_, mut client) in clients.drain() {
            client.shutdown().await;
        }
    }

    /// Get list of connected server names
    pub async fn connected_servers(&self) -> Vec<String> {
        let clients = self.clients.read().await;
        clients.keys().cloned().collect()
    }

    /// Get all available tools from all connected servers
    pub async fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let clients = self.clients.read().await;
        let mut tools = Vec::new();
        for (server_name, client) in clients.iter() {
            for tool in client.tools() {
                tools.push((server_name.clone(), tool.clone()));
            }
        }
        tools
    }

    /// Call a tool on a specific server
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<super::protocol::ToolCallResult> {
        let clients = self.clients.read().await;
        let client = clients
            .get(server)
            .with_context(|| format!("MCP server '{}' not connected", server))?;
        client.call_tool(tool, arguments).await
    }

    /// Reload config and reconnect to servers
    pub async fn reload(&mut self) -> Result<(usize, Vec<(String, String)>)> {
        // Disconnect all existing
        self.disconnect_all().await;

        // Reload config
        self.config = McpConfig::load();

        // Reconnect
        self.connect_all().await
    }

    /// Get config
    pub fn config(&self) -> &McpConfig {
        &self.config
    }

    /// Check if any servers are connected
    pub async fn has_connections(&self) -> bool {
        let clients = self.clients.read().await;
        !clients.is_empty()
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
