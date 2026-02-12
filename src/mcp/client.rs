//! MCP Client - handles communication with a single MCP server

#![allow(dead_code)]

use super::protocol::*;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

/// Shared communication handle for an MCP server.
/// Multiple sessions can hold clones of this and send concurrent requests.
/// Request/response correlation by ID ensures no interference.
#[derive(Clone)]
pub struct McpHandle {
    pub(crate) name: String,
    request_id: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    writer_tx: mpsc::Sender<String>,
    server_info: Arc<std::sync::RwLock<Option<ServerInfo>>>,
    capabilities: Arc<std::sync::RwLock<ServerCapabilities>>,
    tools: Arc<std::sync::RwLock<Vec<McpToolDef>>>,
}

impl McpHandle {
    /// Send a request and wait for response
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<JsonRpcResponse> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let msg = serde_json::to_string(&request)? + "\n";
        self.writer_tx
            .send(msg)
            .await
            .context("Failed to send request")?;

        let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .context("Request timeout")?
            .context("Channel closed")?;

        if let Some(err) = &response.error {
            anyhow::bail!("MCP error {}: {}", err.code, err.message);
        }

        Ok(response)
    }

    /// Call a tool
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        let params = ToolCallParams {
            name: name.to_string(),
            arguments,
        };

        let response = self
            .request("tools/call", Some(serde_json::to_value(params)?))
            .await?;

        let result = response.result.context("No result from tool call")?;
        let tool_result: ToolCallResult = serde_json::from_value(result)?;

        Ok(tool_result)
    }

    /// Get the server name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get server info
    pub fn server_info(&self) -> Option<ServerInfo> {
        self.server_info.read().unwrap().clone()
    }

    /// Get available tools
    pub fn tools(&self) -> Vec<McpToolDef> {
        self.tools.read().unwrap().clone()
    }

    /// Refresh the list of available tools
    pub async fn refresh_tools(&self) -> Result<()> {
        let response = self.request("tools/list", None).await?;

        if let Some(result) = response.result {
            let tools_result: ToolsListResult = serde_json::from_value(result)?;
            *self.tools.write().unwrap() = tools_result.tools;
        }

        Ok(())
    }
}

/// MCP Client - owns the child process and provides shared handles.
/// Only one McpClient exists per MCP server process, but many McpHandle
/// clones can be distributed to different sessions.
pub struct McpClient {
    handle: McpHandle,
    child: Child,
}

impl McpClient {
    /// Connect to an MCP server
    pub async fn connect(name: String, config: &McpServerConfig) -> Result<Self> {
        crate::logging::info(&format!(
            "MCP: Connecting to '{}' ({} {:?})",
            name, config.command, config.args
        ));

        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.extend(config.env.clone());

        let mut child = Command::new(&config.command)
            .args(&config.args)
            .envs(&env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", config.command))?;

        let stdin = child.stdin.take().context("No stdin")?;
        let stdout = child.stdout.take().context("No stdout")?;
        let stderr = child.stderr.take().context("No stderr")?;

        // Spawn stderr reader
        let server_name = name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            crate::logging::warn(&format!(
                                "MCP [{}] stderr: {}",
                                server_name, trimmed
                            ));
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Setup channels
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (writer_tx, mut writer_rx) = mpsc::channel::<String>(32);

        // Spawn writer task
        let mut stdin = stdin;
        tokio::spawn(async move {
            while let Some(msg) = writer_rx.recv().await {
                if stdin.write_all(msg.as_bytes()).await.is_err() {
                    break;
                }
                if stdin.flush().await.is_err() {
                    break;
                }
            }
        });

        // Spawn reader task
        let pending_clone = Arc::clone(&pending);
        let reader_name = name.clone();
        let mut reader = BufReader::new(stdout);
        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        crate::logging::debug(&format!("MCP [{}]: stdout EOF", reader_name));
                        break;
                    }
                    Ok(_) => {
                        if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&line) {
                            if let Some(id) = response.id {
                                let mut pending = pending_clone.lock().await;
                                if let Some(tx) = pending.remove(&id) {
                                    let _ = tx.send(response);
                                }
                            }
                        } else {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() {
                                crate::logging::debug(&format!(
                                    "MCP [{}] non-JSON output: {}",
                                    reader_name, trimmed
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        crate::logging::warn(&format!("MCP [{}] read error: {}", reader_name, e));
                        break;
                    }
                }
            }
        });

        let handle = McpHandle {
            name: name.clone(),
            request_id: Arc::new(AtomicU64::new(1)),
            pending,
            writer_tx,
            server_info: Arc::new(std::sync::RwLock::new(None)),
            capabilities: Arc::new(std::sync::RwLock::new(ServerCapabilities::default())),
            tools: Arc::new(std::sync::RwLock::new(Vec::new())),
        };

        let mut client = Self { handle, child };

        client
            .initialize()
            .await
            .with_context(|| format!("MCP server '{}' failed to initialize", name))?;

        client
            .handle
            .refresh_tools()
            .await
            .with_context(|| format!("MCP server '{}' failed to list tools", name))?;

        crate::logging::info(&format!(
            "MCP: Connected to '{}' with {} tools",
            name,
            client.handle.tools().len()
        ));

        Ok(client)
    }

    /// Get a shareable handle to this client
    pub fn handle(&self) -> McpHandle {
        self.handle.clone()
    }

    /// Initialize the MCP connection
    async fn initialize(&mut self) -> Result<()> {
        let params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "jcode".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let response = self
            .handle
            .request("initialize", Some(serde_json::to_value(params)?))
            .await?;

        if let Some(result) = response.result {
            let init_result: InitializeResult = serde_json::from_value(result)?;
            *self.handle.server_info.write().unwrap() = init_result.server_info;
            *self.handle.capabilities.write().unwrap() = init_result.capabilities;
        }

        // Send initialized notification
        let notif = JsonRpcRequest::new(0, "notifications/initialized", None);
        let msg = serde_json::to_string(&notif)? + "\n";
        self.handle.writer_tx.send(msg).await?;

        Ok(())
    }

    /// Check if server is still running
    pub fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) => false,
            Err(_) => false,
        }
    }

    /// Shutdown the server
    pub async fn shutdown(&mut self) {
        let _ = self
            .handle
            .writer_tx
            .send("{\"jsonrpc\":\"2.0\",\"method\":\"shutdown\"}\n".to_string())
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let _ = self.child.kill().await;
    }

    // === Legacy compatibility methods that delegate to handle ===

    pub fn name(&self) -> &str {
        &self.handle.name
    }

    pub fn server_info(&self) -> Option<ServerInfo> {
        self.handle.server_info()
    }

    pub fn tools(&self) -> Vec<McpToolDef> {
        self.handle.tools()
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        self.handle.call_tool(name, arguments).await
    }

    pub async fn refresh_tools(&self) -> Result<()> {
        self.handle.refresh_tools().await
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
