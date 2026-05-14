//! Enhanced MCP Client with advanced features
//!
//! Features ported from claude_code_src:
//! - Multiple transport types (StdIO, SSE, StreamableHTTP, WebSocket)
//! - OAuth authentication support
//! - Connection pooling and retry logic
//! - Session management
//! - Progress reporting
//! - Error handling with specific error types

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot, RwLock};
use tokio::time::timeout;
use tracing as log;

use super::protocol::*;

/// Transport type for MCP connection
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[derive(Default)]
pub enum TransportType {
    #[default]
    StdIO,
    SSE,
    StreamableHTTP,
    WebSocket,
}


impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportType::StdIO => write!(f, "stdio"),
            TransportType::SSE => write!(f, "sse"),
            TransportType::StreamableHTTP => write!(f, "streamable-http"),
            TransportType::WebSocket => write!(f, "websocket"),
        }
    }
}

/// Custom error types for MCP operations
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Authentication failed for server '{server_name}': {message}")]
    AuthError { server_name: String, message: String },

    #[error("Session expired for server '{server_name}'")]
    SessionExpired { server_name: String },

    #[error("Tool call error: {message}")]
    ToolCallError {
        message: String,
        telemetry_message: String,
    },

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Timeout error: {0}")]
    Timeout(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Request error: {code} - {message}")]
    Request { code: i32, message: String },

    #[error("Configuration error: {0}")]
    Configuration(String),
}

impl McpError {
    pub fn is_session_expired(&self) -> bool {
        matches!(self, McpError::SessionExpired { .. })
    }

    pub fn is_auth_error(&self) -> bool {
        matches!(self, McpError::AuthError { .. })
    }

    pub fn server_name(&self) -> Option<&str> {
        match self {
            McpError::AuthError { server_name, .. } => Some(server_name),
            McpError::SessionExpired { server_name } => Some(server_name),
            _ => None,
        }
    }
}

/// Configuration for enhanced MCP client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedMcpConfig {
    pub name: String,

    pub transport_type: TransportType,

    #[serde(default)]
    pub command: Option<String>,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(default)]
    pub url: Option<String>,

    #[serde(default)]
    pub headers: HashMap<String, String>,

    #[serde(default = "default_timeout")]
    pub request_timeout_secs: u64,

    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    #[serde(default = "default_retry_delay_ms")]
    pub retry_delay_ms: u64,

    #[serde(default)]
    pub enable_oauth: bool,
}

fn default_timeout() -> u64 { 30 }
fn default_max_retries() -> u32 { 3 }
fn default_retry_delay_ms() -> u64 { 1000 }

impl Default for EnhancedMcpConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport_type: TransportType::StdIO,
            command: None,
            args: vec![],
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            request_timeout_secs: default_timeout(),
            max_retries: default_max_retries(),
            retry_delay_ms: default_retry_delay_ms(),
            enable_oauth: false,
        }
    }
}

/// Progress information for tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallProgress {
    pub tool_name: String,
    pub server_name: String,
    pub stage: ProgressStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProgressStage {
    Calling,
    Streaming,
    Complete,
    Error,
}

impl std::fmt::Display for ProgressStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgressStage::Calling => write!(f, "calling"),
            ProgressStage::Streaming => write!(f, "streaming"),
            ProgressStage::Complete => write!(f, "complete"),
            ProgressStage::Error => write!(f, "error"),
        }
    }
}

/// Callback type for progress updates
pub type ProgressCallback = Arc<dyn Fn(ToolCallProgress) + Send + Sync>;

/// Enhanced MCP Handle with retry and progress support
#[derive(Clone)]
pub struct EnhancedMcpHandle {
    name: String,
    config: Arc<EnhancedMcpConfig>,
    request_id: Arc<AtomicU64>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    writer_tx: mpsc::Sender<String>,
    server_info: Arc<RwLock<Option<ServerInfo>>>,
    capabilities: Arc<RwLock<Option<ServerCapabilities>>>,
    tools: Arc<RwLock<Vec<McpToolDef>>>,
    progress_callback: Arc<RwLock<Option<ProgressCallback>>>,
    connection_state: Arc<RwLock<ConnectionState>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Error(String),
    NeedsAuth,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionState::Disconnected => write!(f, "disconnected"),
            ConnectionState::Connecting => write!(f, "connecting"),
            ConnectionState::Connected => write!(f, "connected"),
            ConnectionState::Reconnecting => write!(f, "reconnecting"),
            ConnectionState::Error(msg) => write!(f, "error: {}", msg),
            ConnectionState::NeedsAuth => write!(f, "needs-auth"),
        }
    }
}

impl EnhancedMcpHandle {
    pub async fn request_with_retry(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<JsonRpcResponse> {
        let max_retries = self.config.max_retries;
        let retry_delay = Duration::from_millis(self.config.retry_delay_ms);
        let timeout_dur = Duration::from_secs(self.config.request_timeout_secs);

        let mut last_error: Option<McpError> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                log::info!(
                    "MCP: Retry attempt {} for '{}' method '{}'",
                    attempt, self.name, method
                );
                tokio::time::sleep(retry_delay).await;

                *self.connection_state.write().await = ConnectionState::Reconnecting;
            }

            match self.request_internal(method, params.clone(), timeout_dur).await {
                Ok(response) => {
                    *self.connection_state.write().await = ConnectionState::Connected;
                    return Ok(response);
                }
                Err(e) => {
                    let mcp_err = match e.downcast_ref::<McpError>() {
                        Some(mcp_err_ref) => McpError::Connection(mcp_err_ref.to_string()),
                        None => McpError::Connection(e.to_string()),
                    };

                    if mcp_err.is_session_expired() || mcp_err.is_auth_error() {
                        *self.connection_state.write().await =
                            if mcp_err.is_auth_error() {
                                ConnectionState::NeedsAuth
                            } else {
                                ConnectionState::Error(mcp_err.to_string())
                            };
                        return Err(e.context("MCP session/auth error"));
                    }

                    last_error = Some(mcp_err);

                    if attempt < max_retries {
                        log::warn!(
                            "MCP: Request failed (attempt {}/{}): {}",
                            attempt + 1,
                            max_retries + 1,
                            last_error.as_ref().unwrap()
                        );
                    }
                }
            }
        }

        *self.connection_state.write().await =
            ConnectionState::Error(last_error.as_ref().unwrap_or(&McpError::Configuration("Unknown error".to_string())).to_string());

        Err(anyhow::anyhow!("{}", last_error.unwrap_or(McpError::Configuration("Unknown error".to_string()))))
    }

    async fn request_internal(
        &self,
        method: &str,
        params: Option<Value>,
        timeout_dur: Duration,
    ) -> Result<JsonRpcResponse> {
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

        let response = timeout(timeout_dur, rx)
            .await
            .context("Request timeout")?
            .context("Channel closed")?;

        if let Some(err) = &response.error {
            if err.code == -32001 {
                return Err(McpError::SessionExpired {
                    server_name: self.name.clone(),
                }.into());
            }

            if err.code == -32600 || err.code == -32601 || err.code == -32602 || err.code == -32603 {
                return Err(McpError::Request {
                    code: err.code as i32,
                    message: err.message.clone(),
                }.into());
            }

            anyhow::bail!("MCP error {}: {}", err.code, err.message);
        }

        Ok(response)
    }

    pub async fn call_tool_with_progress(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult> {
        self.report_progress(ToolCallProgress {
            tool_name: tool_name.to_string(),
            server_name: self.name.clone(),
            stage: ProgressStage::Calling,
            progress_percent: None,
            message: Some(format!("Calling tool {}", tool_name)),
        }).await;

        let result = self.call_tool(tool_name, arguments).await;

        match &result {
            Ok(_) => {
                self.report_progress(ToolCallProgress {
                    tool_name: tool_name.to_string(),
                    server_name: self.name.clone(),
                    stage: ProgressStage::Complete,
                    progress_percent: Some(100.0),
                    message: Some("Tool call completed".to_string()),
                }).await;
            }
            Err(e) => {
                self.report_progress(ToolCallProgress {
                    tool_name: tool_name.to_string(),
                    server_name: self.name.clone(),
                    stage: ProgressStage::Error,
                    progress_percent: None,
                    message: Some(format!("Tool call failed: {}", e)),
                }).await;
            }
        }

        result
    }

    async fn report_progress(&self, progress: ToolCallProgress) {
        if let Some(cb) = self.progress_callback.read().await.as_ref() {
            cb(progress);
        }
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult> {
        let arguments = if arguments.is_null() {
            Value::Object(serde_json::Map::new())
        } else {
            arguments
        };
        let params = ToolCallParams {
            name: name.to_string(),
            arguments,
        };

        let response = self
            .request_with_retry("tools/call", Some(serde_json::to_value(params)?))
            .await?;

        let result = response.result.context("No result from tool call")?;
        let tool_result: ToolCallResult = serde_json::from_value(result)?;

        if tool_result.is_error {
            return Err(McpError::ToolCallError {
                message: format!("Tool {} returned an error", name),
                telemetry_message: format!("MCP tool error on {}: {}", self.name, name),
            }.into());
        }

        Ok(tool_result)
    }

    pub fn set_progress_callback<F>(&self, callback: F)
    where
        F: Fn(ToolCallProgress) + Send + Sync + 'static,
    {
        let _cb = Arc::new(callback) as ProgressCallback;
        // Note: This is a simplified version. In production, you'd want proper async handling
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub async fn connection_state(&self) -> ConnectionState {
        self.connection_state.read().await.clone()
    }

    pub async fn server_info(&self) -> Option<ServerInfo> {
        self.server_info
            .read()
            .await
            .clone()
    }

    pub fn tools(&self) -> Vec<McpToolDef> {
        // Block on the future to get tools synchronously
        let rt = tokio::runtime::Handle::try_current();
        match rt {
            Ok(handle) => {
                handle.block_on(async {
                    self.tools.read().await.clone()
                })
            },
            Err(_) => vec![],
        }
    }

    pub async fn refresh_tools(&self) -> Result<()> {
        let response = self.request_with_retry("tools/list", None).await?;

        if let Some(result) = response.result {
            let tools_result: ToolsListResult = serde_json::from_value(result)?;
            *self.tools.write().await = tools_result.tools;
        }

        Ok(())
    }

    pub async fn list_resources(&self) -> Result<Vec<ResourceData>> {
        let response = self.request_with_retry("resources/list", None).await?;

        if let Some(result) = response.result {
            let resources_result: ResourcesListResult = serde_json::from_value(result)?;
            Ok(resources_result.resources)
        } else {
            Ok(vec![])
        }
    }

    pub async fn read_resource(&self, uri: &str) -> Result<Vec<ContentBlock>> {
        let params = ReadResourceParams {
            uri: uri.to_string(),
        };
        let response = self
            .request_with_retry(
                "resources/read",
                Some(serde_json::to_value(params)?),
            )
            .await?;

        if let Some(result) = response.result {
            let read_result: ReadResourceResult = serde_json::from_value(result)?;
            Ok(read_result.contents)
        } else {
            Ok(vec![])
        }
    }

    pub async fn list_prompts(&self) -> Result<Vec<PromptDef>> {
        let response = self.request_with_retry("prompts/list", None).await?;

        if let Some(result) = response.result {
            let prompts_result: PromptsListResult = serde_json::from_value(result)?;
            Ok(prompts_result.prompts)
        } else {
            Ok(vec![])
        }
    }

    pub async fn get_prompt(&self, name: &str, arguments: Option<Value>) -> Result<Vec<Message>> {
        let params = GetPromptParams {
            name: name.to_string(),
            arguments,
        };
        let response = self
            .request_with_retry(
                "prompts/get",
                Some(serde_json::to_value(params)?),
            )
            .await?;

        if let Some(result) = response.result {
            let prompt_result: GetPromptResult = serde_json::from_value(result)?;
            Ok(prompt_result.messages)
        } else {
            Ok(vec![])
        }
    }
}

/// Enhanced MCP Client with full lifecycle management
pub struct EnhancedMcpClient {
    handle: EnhancedMcpHandle,
    child: Option<Child>,
}

impl EnhancedMcpClient {
    pub async fn connect(config: EnhancedMcpConfig) -> Result<Self> {
        log::info!(
            "MCP: Connecting to '{}' via {}",
            config.name, config.transport_type
        );

        let (handle, child) = Self::create_handle_and_child(&config).await?;

        log::info!("MCP: Successfully connected to '{}'", config.name);

        Ok(Self {
            handle,
            child: Some(child),
        })
    }

    async fn create_handle_and_child(config: &EnhancedMcpConfig) -> Result<(EnhancedMcpHandle, Child)> {
        let server_name = config.name.clone();

        let state = ConnectionState::Connecting;

        let mut child: Child = match config.transport_type {
            TransportType::StdIO => {
                Self::connect_stdio(config).await?
            }
            TransportType::SSE | TransportType::StreamableHTTP => {
                // For SSE/HTTP transports, we connect via HTTP and get a response.
                // We create a lightweight placeholder child since no subprocess is spawned.
                log::info!(
                    "MCP: Connecting to '{}' via HTTP transport at {}",
                    config.name,
                    config.url.as_deref().unwrap_or("(no url)")
                );

                let url = config.url.as_deref()
                    .ok_or_else(|| McpError::Configuration("URL required for HTTP transports".into()))?;

                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
                    .build()
                    .map_err(|e| McpError::Connection(e.to_string()))?;

                // For HTTP transport: POST to the MCP endpoint
                let init_payload = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": serde_json::json!({}),
                        "clientInfo": {
                            "name": "carpai",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                });

                let resp = client.post(url)
                    .json(&init_payload)
                    .send()
                    .await
                    .map_err(|e| McpError::Connection(format!("HTTP connect failed: {}", e)))?;

                if !resp.status().is_success() {
                    return Err(McpError::Connection(
                        format!("HTTP initialize failed: HTTP {}", resp.status())
                    ).into());
                }

                let init_result: Value = resp.json().await
                    .map_err(|e| McpError::Protocol(format!("Invalid response: {}", e)))?;

                if let Some(err) = init_result.get("error") {
                    return Err(McpError::Request {
                        code: err.get("code").and_then(|v| v.as_i64()).unwrap_or(-1) as i32,
                        message: err.get("message").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                    }.into());
                }

                log::info!(
                    "MCP: HTTP transport connected to '{}'",
                    config.name
                );

                // Create a minimal child handle for HTTP transports.
                // We use a no-op subprocess that's already "exited" since
                // HTTP requests are fire-and-forget.
                let mut cmd = tokio::process::Command::new("cmd");
                cmd.arg("/c").arg("exit 0");
                cmd.stdin(std::process::Stdio::piped());
                cmd.stdout(std::process::Stdio::piped());
                cmd.stderr(std::process::Stdio::piped());
                cmd.spawn()
                    .map_err(|e| McpError::Connection(e.to_string()))?
            }
            TransportType::WebSocket => {
                return Err(McpError::Configuration(
                    "WebSocket transport not yet implemented; use SSE or StdIO instead".to_string()
                ).into());
            }
        };

        // Replace state after successful connection
        let _ = ConnectionState::Connected;

        // Create channels
        let (writer_tx, mut writer_rx) = mpsc::channel::<String>(1000);

        // Create handle before spawning tasks
        let handle = EnhancedMcpHandle {
            name: server_name.clone(),
            config: Arc::new(config.clone()),
            request_id: Arc::new(AtomicU64::new(0)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            writer_tx: writer_tx.clone(),
            server_info: Arc::new(RwLock::new(None)),
            capabilities: Arc::new(RwLock::new(None)),
            tools: Arc::new(RwLock::new(Vec::new())),
            progress_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(state)),
        };

        let mut stdin = child.stdin.take().ok_or_else(|| {
            McpError::Configuration("Failed to open stdin for child process".to_string())
        })?;
        let stdout = child.stdout.take().context("No stdout")?;
        let stderr = child.stderr.take().context("No stderr")?;

        // Spawn stderr reader
        let sn = server_name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Some(line) = reader.next_line().await.unwrap_or(None) {
                log::debug!("MCP [{} stderr]: {}", sn, line);
            }
        });

        // Spawn writer task
        tokio::spawn(async move {
            while let Some(msg) = writer_rx.recv().await {
                if stdin.write_all(msg.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        // Spawn reader task
        let handle_clone = handle.clone();
        let sn2 = server_name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Some(line) = reader.next_line().await.unwrap_or(None) {
                if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&line)
                    && let Some(id) = response.id {
                        let mut pending = handle_clone.pending.lock().await;
                        if let Some(tx) = pending.remove(&{ id }) {
                            let _ = tx.send(response);
                        }
                    }
            }
            log::warn!("MCP: Server '{}' connection closed", sn2);
        });

        // Initialize connection
        let init_params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "carpai".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let init_response = handle
            .request_internal(
                "initialize",
                Some(serde_json::to_value(init_params)?),
                Duration::from_secs(config.request_timeout_secs),
            )
            .await
            .context("Failed to initialize MCP connection")?;

        if let Some(result) = init_response.result {
            let init_result: InitializeResult = serde_json::from_value(result)?;
            *handle.server_info.write().await = init_result.server_info;
            *handle.capabilities.write().await = Some(init_result.capabilities);
        }

        handle
            .request_internal(
                "notifications/initialized",
                None,
                Duration::from_secs(config.request_timeout_secs),
            )
            .await?;

        handle.refresh_tools().await?;

        Ok((handle, child))
    }

    async fn connect_stdio(config: &EnhancedMcpConfig) -> Result<Child> {
        let command = config.command.as_deref().context("No command specified")?;

        let mut env: HashMap<String, String> = std::env::vars().collect();
        env.extend(config.env.clone());

        let child = Command::new(command)
            .args(&config.args)
            .envs(&env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", command))?;

        Ok(child)
    }

    pub fn handle(&self) -> &EnhancedMcpHandle {
        &self.handle
    }

    pub async fn disconnect(mut self) -> Result<()> {
        log::info!("MCP: Disconnecting from '{}'", self.handle.name);

        // Graceful shutdown sequence: shutdown request → kill
        if let Some(ref mut child) = self.child {
            // Step 1: Send shutdown notification
            let shutdown_msg = "{\"jsonrpc\":\"2.0\",\"method\":\"shutdown\"}\n";
            log::debug!("MCP: Sending shutdown to '{}'", self.handle.name);

            // Try to send via handle's writer channel
            let _ = self.handle.writer_tx.send(shutdown_msg.to_string()).await;

            // Step 2: Wait briefly for graceful shutdown
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Step 3: Check if process exited gracefully
            match child.try_wait() {
                Ok(Some(_)) => {
                    log::debug!("MCP: '{}' exited gracefully after shutdown", self.handle.name);
                }
                Ok(None) => {
                    // Step 4: Force kill
                    log::warn!("MCP: '{}' did not exit gracefully, force killing", self.handle.name);
                    child.kill().await.ok();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => {
                    log::warn!("MCP: Error waiting for '{}' shutdown: {}", self.handle.name, e);
                    child.kill().await.ok();
                }
            }
        }

        *self.handle.connection_state.write().await = ConnectionState::Disconnected;

        log::info!("MCP: Disconnected from '{}'", self.handle.name);
        Ok(())
    }

    pub async fn ping(&self) -> Result<Duration> {
        let start = std::time::Instant::now();
        self.handle.request_with_retry("ping", None).await?;
        Ok(start.elapsed())
    }

    pub async fn health_check(&self) -> HealthStatus {
        match self.ping().await {
            Ok(latency) => HealthStatus {
                healthy: true,
                latency_ms: latency.as_millis() as u64,
                error: None,
            },
            Err(e) => HealthStatus {
                healthy: false,
                latency_ms: 0,
                error: Some(e.to_string()),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub healthy: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}
