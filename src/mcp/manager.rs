//! MCP Manager - manages MCP server connections for a single session.
//!
//! In daemon mode with a shared pool, servers marked `shared: true` (the default)
//! are managed by the pool and reused across sessions. Servers marked `shared: false`
//! (e.g., Playwright with browser state) are spawned per-session.

#![allow(dead_code)]

use super::client::{McpClient, McpHandle};
use super::pool::SharedMcpPool;
use super::protocol::{McpConfig, McpServerConfig, McpToolDef, ToolCallResult};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Manages MCP server connections for a session.
///
/// In daemon mode, shared servers delegate to the SharedMcpPool while
/// non-shared (stateful) servers are owned per-session.
pub struct McpManager {
    pool: Option<Arc<SharedMcpPool>>,
    /// Handles from the shared pool (shared servers)
    pool_handles: RwLock<HashMap<String, McpHandle>>,
    /// Per-session owned clients (non-shared / stateful servers)
    owned_clients: RwLock<HashMap<String, McpClient>>,
    config: McpConfig,
    session_id: String,
}

impl McpManager {
    /// Create a new manager in owned mode (standalone/TUI - no sharing)
    pub fn new() -> Self {
        Self {
            pool: None,
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            config: McpConfig::load(),
            session_id: "standalone".to_string(),
        }
    }

    /// Create a manager backed by a shared pool (daemon mode)
    pub fn with_shared_pool(pool: Arc<SharedMcpPool>, session_id: String) -> Self {
        Self {
            pool: Some(pool),
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            config: McpConfig::load(),
            session_id,
        }
    }

    /// Create manager with specific config (no sharing)
    pub fn with_config(config: McpConfig) -> Self {
        Self {
            pool: None,
            pool_handles: RwLock::new(HashMap::new()),
            owned_clients: RwLock::new(HashMap::new()),
            config,
            session_id: "standalone".to_string(),
        }
    }

    /// Whether this manager has a shared pool available
    pub fn is_shared(&self) -> bool {
        self.pool.is_some()
    }

    /// Connect to all configured servers.
    /// Shared servers go to the pool, non-shared are spawned per-session.
    pub async fn connect_all(&self) -> Result<(usize, Vec<(String, String)>)> {
        let mut total_successes = 0;
        let mut total_failures = Vec::new();

        // Split servers into shared vs owned
        let (shared_servers, owned_servers): (Vec<_>, Vec<_>) = self
            .config
            .servers
            .iter()
            .partition(|(_, config)| config.shared && self.pool.is_some());

        // Connect shared servers via pool
        if let Some(pool) = &self.pool {
            if !shared_servers.is_empty() {
                let (successes, failures) = pool.connect_all().await;
                total_successes += successes;
                total_failures.extend(failures);

                // Acquire handles for shared servers only
                let all_handles = pool.acquire_handles(&self.session_id).await;
                let shared_names: std::collections::HashSet<&String> =
                    shared_servers.iter().map(|(name, _)| *name).collect();
                let mut pool_handles = self.pool_handles.write().await;
                for (name, handle) in all_handles {
                    if shared_names.contains(&name) {
                        pool_handles.insert(name, handle);
                    }
                }

                // If pool already had servers connected, count those as successes
                if total_successes == 0 && !pool_handles.is_empty() {
                    total_successes = pool_handles.len();
                }
            }
        }

        // Connect non-shared servers per-session
        if !owned_servers.is_empty() {
            let mut spawn_handles = Vec::new();

            for (name, config) in owned_servers {
                let name = name.clone();
                let config = config.clone();
                let handle = tokio::spawn(async move {
                    let result = McpClient::connect(name.clone(), &config).await;
                    (name, result)
                });
                spawn_handles.push(handle);
            }

            for handle in spawn_handles {
                match handle.await {
                    Ok((name, Ok(client))) => {
                        let mut clients = self.owned_clients.write().await;
                        clients.insert(name, client);
                        total_successes += 1;
                    }
                    Ok((name, Err(e))) => {
                        let error_msg = format!("{:#}", e);
                        crate::logging::error(&format!(
                            "Failed to connect to MCP server '{}': {}",
                            name, error_msg
                        ));
                        total_failures.push((name, error_msg));
                    }
                    Err(e) => {
                        crate::logging::error(&format!("MCP connection task panicked: {}", e));
                    }
                }
            }
        }

        Ok((total_successes, total_failures))
    }

    /// Connect to a specific server
    pub async fn connect(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        if config.shared {
            if let Some(pool) = &self.pool {
                pool.connect_server(name, config).await?;
                if let Some(handle) = pool.get_handle(name).await {
                    self.pool_handles
                        .write()
                        .await
                        .insert(name.to_string(), handle);
                }
                return Ok(());
            }
        }

        // Owned (non-shared or no pool available)
        let client = McpClient::connect(name.to_string(), config)
            .await
            .with_context(|| format!("Failed to connect to MCP server '{}'", name))?;

        self.owned_clients
            .write()
            .await
            .insert(name.to_string(), client);
        Ok(())
    }

    /// Disconnect from a server
    pub async fn disconnect(&self, name: &str) -> Result<()> {
        // Check if it's a pool handle
        {
            let mut handles = self.pool_handles.write().await;
            if handles.remove(name).is_some() {
                if let Some(pool) = &self.pool {
                    pool.release_handles(&self.session_id, &[name.to_string()])
                        .await;
                }
                return Ok(());
            }
        }

        // Otherwise it's owned
        let mut clients = self.owned_clients.write().await;
        if let Some(mut client) = clients.remove(name) {
            client.shutdown().await;
        }
        Ok(())
    }

    /// Disconnect from all servers
    pub async fn disconnect_all(&self) {
        // Release pool handles
        {
            let mut handles = self.pool_handles.write().await;
            let names: Vec<String> = handles.keys().cloned().collect();
            handles.clear();
            if let Some(pool) = &self.pool {
                pool.release_handles(&self.session_id, &names).await;
            }
        }

        // Shutdown owned clients
        {
            let mut clients = self.owned_clients.write().await;
            for (_, mut client) in clients.drain() {
                client.shutdown().await;
            }
        }
    }

    /// Get list of connected server names
    pub async fn connected_servers(&self) -> Vec<String> {
        let mut names: Vec<String> = self.pool_handles.read().await.keys().cloned().collect();
        names.extend(self.owned_clients.read().await.keys().cloned());
        names
    }

    /// Get all available tools from all connected servers
    pub async fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let mut tools = Vec::new();

        // Pool handles
        for (server_name, handle) in self.pool_handles.read().await.iter() {
            for tool in handle.tools() {
                tools.push((server_name.clone(), tool));
            }
        }

        // Owned clients
        for (server_name, client) in self.owned_clients.read().await.iter() {
            for tool in client.tools() {
                tools.push((server_name.clone(), tool));
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
    ) -> Result<ToolCallResult> {
        // Try pool handles first
        {
            let handles = self.pool_handles.read().await;
            if let Some(handle) = handles.get(server) {
                return handle.call_tool(tool, arguments).await;
            }
        }

        // Try owned clients
        {
            let clients = self.owned_clients.read().await;
            if let Some(client) = clients.get(server) {
                return client.call_tool(tool, arguments).await;
            }
        }

        anyhow::bail!("MCP server '{}' not connected", server)
    }

    /// Reload config and reconnect to servers
    pub async fn reload(&mut self) -> Result<(usize, Vec<(String, String)>)> {
        // Disconnect all (releases pool handles, shuts down owned)
        self.disconnect_all().await;

        // Reload config
        self.config = McpConfig::load();

        // If we have a pool, reload it too (reconnects shared servers)
        if let Some(pool) = &self.pool {
            pool.reload().await;
        }

        // Reconnect everything
        self.connect_all().await
    }

    /// Get config
    pub fn config(&self) -> &McpConfig {
        &self.config
    }

    /// Check if any servers are connected
    pub async fn has_connections(&self) -> bool {
        !self.pool_handles.read().await.is_empty() || !self.owned_clients.read().await.is_empty()
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}
