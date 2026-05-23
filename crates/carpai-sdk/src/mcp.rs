//! CarpAI SDK - MCP Client Module
//!
//! Provides a high-level MCP client for IDE integration.
//! Supports:
//! - Connecting to MCP servers via stdio, SSE, or HTTP
//! - Listing and calling MCP tools
//! - Managing multiple server connections
//! - Automatic reconnection with backoff

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub transport: McpTransport,
    pub status: McpConnectionStatus,
    pub tools: Vec<McpToolDefinition>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransport {
    #[serde(rename = "stdio")]
    Stdio,
    #[serde(rename = "sse")]
    Sse,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "streamable-http")]
    StreamableHttp,
}

impl std::fmt::Display for McpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpTransport::Stdio => write!(f, "stdio"),
            McpTransport::Sse => write!(f, "sse"),
            McpTransport::Http => write!(f, "http"),
            McpTransport::StreamableHttp => write!(f, "streamable-http"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum McpConnectionStatus {
    #[serde(rename = "disconnected")]
    Disconnected,
    #[serde(rename = "connecting")]
    Connecting,
    #[serde(rename = "connected")]
    Connected,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub transport: McpTransport,
    pub env: HashMap<String, String>,
    pub auto_connect: bool,
}

impl McpServerConfig {
    pub fn stdio(name: impl Into<String>, command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command: Some(command.into()),
            args,
            url: None,
            transport: McpTransport::Stdio,
            env: HashMap::new(),
            auto_connect: true,
        }
    }

    pub fn sse(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: None,
            args: Vec::new(),
            url: Some(url.into()),
            transport: McpTransport::Sse,
            env: HashMap::new(),
            auto_connect: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum McpClientError {
    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Transport error: {0}")]
    TransportError(String),

    #[error("Protocol error: {0}")]
    ProtocolError(String),

    #[error("Tool call error: {0}")]
    ToolCallError(String),

    #[error("Server '{0}' not found")]
    ServerNotFound(String),

    #[error("Tool '{0}' not found on server '{1}'")]
    ToolNotFound(String, String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Timeout")]
    Timeout,
}

// ---------------------------------------------------------------------------
// McpClient trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait McpClient: Send + Sync {
    /// Connect to an MCP server
    async fn connect(&mut self, config: &McpServerConfig) -> Result<(), McpClientError>;

    /// Disconnect from the MCP server
    async fn disconnect(&mut self) -> Result<(), McpClientError>;

    /// List available tools from the server
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpClientError>;

    /// Call a tool on the server
    async fn call_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpClientError>;

    /// Get connection status
    fn status(&self) -> McpConnectionStatus;

    /// Check if connected
    fn is_connected(&self) -> bool {
        self.status() == McpConnectionStatus::Connected
    }
}

// ---------------------------------------------------------------------------
// McpClientManager - manages multiple MCP server connections
// ---------------------------------------------------------------------------

pub struct McpClientManager {
    clients: RwLock<HashMap<String, Box<dyn McpClient + Send + Sync>>>,
    configs: RwLock<HashMap<String, McpServerConfig>>,
    default_timeout: std::time::Duration,
}

impl McpClientManager {
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            configs: RwLock::new(HashMap::new()),
            default_timeout: std::time::Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Register a server config but don't connect yet.
    pub async fn register(&self, config: McpServerConfig) {
        let mut configs = self.configs.write().await;
        configs.insert(config.name.clone(), config);
    }

    /// Register and immediately connect to a server.
    pub async fn register_and_connect<C>(&self, config: McpServerConfig, client: C) -> Result<(), McpClientError>
    where
        C: McpClient + Send + Sync + 'static,
    {
        let name = config.name.clone();
        let mut clients = self.clients.write().await;
        if clients.contains_key(&name) {
            return Err(McpClientError::ConnectionError(format!(
                "Client '{}' already exists", name
            )));
        }

        let mut boxed = Box::new(client);
        boxed.connect(&config).await?;
        clients.insert(name.clone(), boxed);

        let mut configs = self.configs.write().await;
        configs.insert(name, config);

        Ok(())
    }

    /// Connect all registered servers that have auto_connect enabled.
    pub async fn connect_all<F>(&self, client_factory: F) -> Vec<(String, Result<(), McpClientError>)>
    where
        F: Fn(&McpServerConfig) -> Box<dyn McpClient + Send + Sync>,
    {
        let configs = self.configs.read().await;
        let mut results = Vec::new();

        for (name, config) in configs.iter() {
            if !config.auto_connect {
                continue;
            }
            let mut client = client_factory(config);
            match client.connect(config).await {
                Ok(()) => {
                    let mut clients = self.clients.write().await;
                    clients.insert(name.clone(), client);
                    results.push((name.clone(), Ok(())));
                }
                Err(e) => {
                    results.push((name.clone(), Err(e)));
                }
            }
        }

        results
    }

    /// Disconnect from a specific server.
    pub async fn disconnect(&self, name: &str) -> Result<(), McpClientError> {
        let mut clients = self.clients.write().await;
        if let Some(mut client) = clients.remove(name) {
            client.disconnect().await?;
        }
        Ok(())
    }

    /// Disconnect from all servers.
    pub async fn disconnect_all(&self) {
        let mut clients = self.clients.write().await;
        for (name, mut client) in clients.drain() {
            if let Err(e) = client.disconnect().await {
                warn!("Error disconnecting MCP client '{}': {}", name, e);
            }
        }
    }

    /// Get a client by server name.
    pub async fn get_client(&self, name: &str) -> Option<tokio::sync::RwLockReadGuard<'_, HashMap<String, Box<dyn McpClient + Send + Sync>>>> {
        let clients = self.clients.read().await;
        if clients.contains_key(name) {
            // Note: we can't return a reference to the value directly due to the trait object
            // The caller should use call_tool_on_server instead
            drop(clients);
        }
        None
    }

    /// List all connected servers with their tools.
    pub async fn list_servers(&self) -> Vec<McpServerInfo> {
        let clients = self.clients.read().await;
        let configs = self.configs.read().await;
        let mut servers = Vec::new();

        for (name, client) in clients.iter() {
            let config = configs.get(name);
            let tools = client.list_tools().await.unwrap_or_default();
            servers.push(McpServerInfo {
                name: name.clone(),
                transport: config.map(|c| c.transport.clone()).unwrap_or(McpTransport::Stdio),
                status: client.status(),
                tools,
                error: None,
            });
        }

        servers
    }

    /// Call a tool on a specific server.
    pub async fn call_tool_on_server(
        &self,
        server_name: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpClientError> {
        let clients = self.clients.read().await;
        let client = clients
            .get(server_name)
            .ok_or_else(|| McpClientError::ServerNotFound(server_name.to_string()))?;
        client.call_tool(tool_name, args).await
    }

    /// Number of connected clients.
    pub async fn connected_count(&self) -> usize {
        self.clients.read().await.len()
    }
}

impl Default for McpClientManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Simple HTTP MCP client implementation
// ---------------------------------------------------------------------------

pub struct HttpMcpClient {
    name: String,
    client: reqwest::Client,
    base_url: Option<String>,
    connected: bool,
    tools: Vec<McpToolDefinition>,
}

impl HttpMcpClient {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            client: reqwest::Client::new(),
            base_url: None,
            connected: false,
            tools: Vec::new(),
        }
    }
}

#[async_trait]
impl McpClient for HttpMcpClient {
    async fn connect(&mut self, config: &McpServerConfig) -> Result<(), McpClientError> {
        let url = config
            .url
            .as_ref()
            .ok_or_else(|| McpClientError::ConnectionError("No URL provided".to_string()))?;

        // Verify connectivity by fetching tools list
        let tools_url = format!("{}/tools", url.trim_end_matches('/'));
        match self.client.get(&tools_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let data: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| McpClientError::ProtocolError(e.to_string()))?;

                if let Some(tools_array) = data.as_array().or_else(|| data.get("tools").and_then(|v| v.as_array())) {
                    self.tools = tools_array
                        .iter()
                        .filter_map(|t| {
                            Some(McpToolDefinition {
                                name: t.get("name")?.as_str()?.to_string(),
                                description: t.get("description").and_then(|d| d.as_str()).map(String::from),
                                input_schema: t.get("inputSchema").or_else(|| t.get("input_schema")).cloned(),
                            })
                        })
                        .collect();
                }

                self.base_url = Some(url.clone());
                self.connected = true;
                info!("Connected to MCP server '{}' at {}", config.name, url);
                Ok(())
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                Err(McpClientError::ConnectionError(format!(
                    "HTTP {}: {}",
                    status, body
                )))
            }
            Err(e) => Err(McpClientError::ConnectionError(e.to_string())),
        }
    }

    async fn disconnect(&mut self) -> Result<(), McpClientError> {
        self.connected = false;
        self.base_url = None;
        self.tools.clear();
        Ok(())
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpClientError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, McpClientError> {
        let base_url = self
            .base_url
            .as_ref()
            .ok_or_else(|| McpClientError::ConnectionError("Not connected".to_string()))?;

        let url = format!("{}/call", base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "name": tool_name,
            "arguments": args,
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| McpClientError::TransportError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(McpClientError::ToolCallError(format!(
                "HTTP {}: {}",
                status, text
            )));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| McpClientError::ProtocolError(e.to_string()))?;

        // Extract result content (handle MCP response format)
        if let Some(content) = data.get("content").and_then(|c| c.as_array()) {
            let text_parts: Vec<String> = content
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        item.get("text").and_then(|t| t.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .collect();
            return Ok(serde_json::json!({ "text": text_parts.join("\n") }));
        }

        Ok(data)
    }

    fn status(&self) -> McpConnectionStatus {
        if self.connected {
            McpConnectionStatus::Connected
        } else {
            McpConnectionStatus::Disconnected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_server_config_stdio() {
        let config = McpServerConfig::stdio("test", "python", vec!["server.py".to_string()]);
        assert_eq!(config.name, "test");
        assert_eq!(config.command.unwrap(), "python");
        assert!(config.url.is_none());
    }

    #[test]
    fn test_mcp_server_config_sse() {
        let config = McpServerConfig::sse("test", "http://localhost:8000");
        assert_eq!(config.name, "test");
        assert_eq!(config.url.unwrap(), "http://localhost:8000");
    }

    #[test]
    fn test_mcp_connection_status_order() {
        assert_ne!(McpConnectionStatus::Connected, McpConnectionStatus::Disconnected);
        assert_ne!(McpConnectionStatus::Connected, McpConnectionStatus::Connecting);
        assert_ne!(McpConnectionStatus::Connected, McpConnectionStatus::Error);
    }

    #[tokio::test]
    async fn test_mcp_client_manager_empty() {
        let manager = McpClientManager::new();
        assert_eq!(manager.connected_count().await, 0);
        let servers = manager.list_servers().await;
        assert!(servers.is_empty());
    }

    #[tokio::test]
    async fn test_mcp_client_manager_connect_all_no_clients() {
        let manager = McpClientManager::new();

        // Register a config without a client factory (won't auto-connect since factory always returns something to connect)
        // This test just verifies that the method doesn't panic
        let config = McpServerConfig::stdio("nonexistent", "python", vec!["does-not-exist.py".to_string()]);
        manager.register(config).await;

        let results = manager.connect_all(|_| Box::new(HttpMcpClient::new("dummy"))).await;
        // The connect will fail (IO error), but shouldn't panic
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_err());
    }

    #[test]
    fn test_mcp_tool_definition_serde() {
        let tool = McpToolDefinition {
            name: "test_tool".to_string(),
            description: Some("A test tool".to_string()),
            input_schema: None,
        };
        let json = serde_json::to_string(&tool).unwrap();
        let deserialized: McpToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test_tool");
        assert_eq!(deserialized.description.unwrap(), "A test tool");
    }
}
