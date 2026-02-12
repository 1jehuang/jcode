//! Shared MCP Server Pool
//!
//! Manages a global pool of MCP server processes that are shared across
//! all jcode sessions. Instead of each session spawning its own set of
//! MCP servers (N sessions × M servers = N×M processes), sessions share
//! a single pool (M processes total).
//!
//! Sessions get lightweight `McpHandle` clones that can send concurrent
//! requests to shared server processes. Request/response correlation by
//! ID ensures no interference between sessions.

use super::client::{McpClient, McpHandle};
use super::protocol::{McpConfig, McpServerConfig, McpToolDef};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Global shared pool of MCP server processes.
///
/// Only one pool exists per jcode daemon. It owns the child processes
/// and hands out cheap `McpHandle` clones to sessions.
pub struct SharedMcpPool {
    clients: Mutex<HashMap<String, McpClient>>,
    handles: RwLock<HashMap<String, McpHandle>>,
    config: RwLock<McpConfig>,
    ref_counts: Mutex<HashMap<String, usize>>,
}

impl SharedMcpPool {
    /// Create a new shared pool with the given config
    pub fn new(config: McpConfig) -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            handles: RwLock::new(HashMap::new()),
            config: RwLock::new(config),
            ref_counts: Mutex::new(HashMap::new()),
        }
    }

    /// Create pool loading config from default locations
    pub fn from_default_config() -> Self {
        Self::new(McpConfig::load())
    }

    /// Connect to all configured servers.
    /// Returns (successes, failures).
    pub async fn connect_all(&self) -> (usize, Vec<(String, String)>) {
        let config = self.config.read().await;
        let mut spawn_tasks = Vec::new();

        for (name, server_config) in &config.servers {
            // Skip if already connected
            let handles = self.handles.read().await;
            if handles.contains_key(name) {
                continue;
            }
            drop(handles);

            let name = name.clone();
            let config = server_config.clone();
            let handle = tokio::spawn(async move {
                let result = McpClient::connect(name.clone(), &config).await;
                (name, result)
            });
            spawn_tasks.push(handle);
        }

        let mut successes = 0;
        let mut failures = Vec::new();

        for task in spawn_tasks {
            match task.await {
                Ok((name, Ok(client))) => {
                    let handle = client.handle();
                    {
                        let mut handles = self.handles.write().await;
                        handles.insert(name.clone(), handle);
                    }
                    {
                        let mut clients = self.clients.lock().await;
                        clients.insert(name, client);
                    }
                    successes += 1;
                }
                Ok((name, Err(e))) => {
                    let error_msg = format!("{:#}", e);
                    crate::logging::error(&format!(
                        "Failed to connect to MCP server '{}': {}",
                        name, error_msg
                    ));
                    failures.push((name, error_msg));
                }
                Err(e) => {
                    crate::logging::error(&format!("MCP connection task panicked: {}", e));
                }
            }
        }

        (successes, failures)
    }

    /// Connect to a specific server by name and config
    pub async fn connect_server(&self, name: &str, config: &McpServerConfig) -> Result<()> {
        let client = McpClient::connect(name.to_string(), config)
            .await
            .with_context(|| format!("Failed to connect to MCP server '{}'", name))?;

        let handle = client.handle();
        {
            let mut handles = self.handles.write().await;
            handles.insert(name.to_string(), handle);
        }
        {
            let mut clients = self.clients.lock().await;
            clients.insert(name.to_string(), client);
        }
        Ok(())
    }

    /// Disconnect a specific server
    pub async fn disconnect_server(&self, name: &str) {
        {
            let mut handles = self.handles.write().await;
            handles.remove(name);
        }
        {
            let mut clients = self.clients.lock().await;
            if let Some(mut client) = clients.remove(name) {
                client.shutdown().await;
            }
        }
        {
            let mut refs = self.ref_counts.lock().await;
            refs.remove(name);
        }
    }

    /// Disconnect all servers
    pub async fn disconnect_all(&self) {
        {
            let mut handles = self.handles.write().await;
            handles.clear();
        }
        {
            let mut clients = self.clients.lock().await;
            for (_, mut client) in clients.drain() {
                client.shutdown().await;
            }
        }
        {
            let mut refs = self.ref_counts.lock().await;
            refs.clear();
        }
    }

    /// Get handles for all connected servers (for a new session).
    /// Increments reference counts.
    pub async fn acquire_handles(&self, session_id: &str) -> HashMap<String, McpHandle> {
        let handles = self.handles.read().await;
        let result = handles.clone();

        let mut refs = self.ref_counts.lock().await;
        for name in result.keys() {
            *refs.entry(name.clone()).or_insert(0) += 1;
        }

        if !result.is_empty() {
            crate::logging::info(&format!(
                "MCP pool: session '{}' acquired {} server handle(s)",
                session_id,
                result.len()
            ));
        }

        result
    }

    /// Release handles when a session disconnects.
    /// Decrements reference counts.
    pub async fn release_handles(&self, session_id: &str, server_names: &[String]) {
        let mut refs = self.ref_counts.lock().await;
        for name in server_names {
            if let Some(count) = refs.get_mut(name) {
                *count = count.saturating_sub(1);
            }
        }

        if !server_names.is_empty() {
            crate::logging::info(&format!(
                "MCP pool: session '{}' released {} server handle(s)",
                session_id,
                server_names.len()
            ));
        }
    }

    /// Get a handle for a specific server
    pub async fn get_handle(&self, name: &str) -> Option<McpHandle> {
        let handles = self.handles.read().await;
        handles.get(name).cloned()
    }

    /// Get all available tools from all connected servers
    pub async fn all_tools(&self) -> Vec<(String, McpToolDef)> {
        let handles = self.handles.read().await;
        let mut tools = Vec::new();
        for (server_name, handle) in handles.iter() {
            for tool in handle.tools() {
                tools.push((server_name.clone(), tool));
            }
        }
        tools
    }

    /// Get list of connected server names
    pub async fn connected_servers(&self) -> Vec<String> {
        let handles = self.handles.read().await;
        handles.keys().cloned().collect()
    }

    /// Call a tool on a specific server
    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) -> Result<super::protocol::ToolCallResult> {
        let handles = self.handles.read().await;
        let handle = handles
            .get(server)
            .with_context(|| format!("MCP server '{}' not connected", server))?;
        handle.call_tool(tool, arguments).await
    }

    /// Reload config and reconnect all servers
    pub async fn reload(&self) -> (usize, Vec<(String, String)>) {
        self.disconnect_all().await;
        *self.config.write().await = McpConfig::load();
        self.connect_all().await
    }

    /// Get current config
    pub async fn config(&self) -> McpConfig {
        self.config.read().await.clone()
    }

    /// Check if any servers are connected
    pub async fn has_connections(&self) -> bool {
        let handles = self.handles.read().await;
        !handles.is_empty()
    }

    /// Get reference counts (for debugging)
    pub async fn ref_counts(&self) -> HashMap<String, usize> {
        self.ref_counts.lock().await.clone()
    }
}

/// Global pool singleton
static SHARED_POOL: tokio::sync::OnceCell<Arc<SharedMcpPool>> = tokio::sync::OnceCell::const_new();

/// Initialize the global shared MCP pool. Call once at daemon startup.
pub async fn init_shared_pool() -> Arc<SharedMcpPool> {
    SHARED_POOL
        .get_or_init(|| async {
            let pool = SharedMcpPool::from_default_config();
            Arc::new(pool)
        })
        .await
        .clone()
}

/// Get the global shared pool, if initialized.
pub fn get_shared_pool() -> Option<Arc<SharedMcpPool>> {
    SHARED_POOL.get().cloned()
}
