//! MCP (Model Context Protocol) Implementation
//!
//! ## Architecture
//! ```text
//! +---------------------------------------------------------+
//! |                    McpBridge                            |
//! |   (bidirectional — Server + Client in one)              |
//! +---------------------------------------------------------+
//! |  +--------------+    +------------------------------+   |
//! |  |  MCP Server  |    |  MCP Client (McpManager)    |   |
//! |  | (server.rs)  |    |  - Basic McpClient          |   |
//! |  | - tools/list |    |  - EnhancedMcpClient        |   |
//! |  | - tools/call |    |  - SharedMcpPool            |   |
//! |  | - resources  |    |  - SSE/HTTP/WS transports   |   |
//! |  | - prompts    |    +------------------------------+   |
//! |  +------+-------+                                       |
//! |         |                                                |
//! |         ▼                                                |
//! |  +--------------------------------------------------+    |
//! |  |           Tool Registry + MCP Tool wrapper       |    |
//! |  +--------------------------------------------------+    |
//! +---------------------------------------------------------+
//! ```
//!
//! ## Server mode (other tools connect TO CarpAI)
//! Run `carpai mcp serve` to start CarpAI as an MCP server.
//! External tools (IDEs, agents) can call CarpAI's tools
//! via the MCP protocol over stdin/stdout JSON-RPC.
//!
//! ## Client mode (connecting TO an MCP server)
//! Connect to MCP servers that provide tools via JSON-RPC over stdio.
//! Supports shared server pools so multiple sessions reuse the same
//! MCP server processes instead of spawning duplicates.
//!
//! ## Bidirectional mode
//! Run `carpai mcp bridge` to start both server and client simultaneously.

use std::collections::HashMap;
use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{info, warn, debug, error};
use uuid::Uuid;
use once_cell::sync::OnceCell;

use crate::config::CoreConfig;
use carpai_internal::{
    ToolDefinition,
    ToolCategory,
    ToolRequest,
    ToolResponse,
    ToolContext,
    ToolSchema,
    ToolExecError,
};

// ========================================================================
// Protocol Types
// ========================================================================

/// JSON-RPC 2.0 Request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<JsonRpcId>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 ID (can be string, number, or null)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Num(i64),
    Str(String),
    Null,
}

/// JSON-RPC 2.0 Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    #[serde(flatten)]
    pub result: JsonRpcResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JsonRpcResult {
    #[serde(rename = "result")]
    Success { result: Value },
    #[serde(rename = "error")]
    Error { error: JsonRpcError },
}

/// JSON-RPC 2.0 Error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self { code: -32700, message: msg.into(), data: None }
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self { code: -32600, message: msg.into(), data: None }
    }

    pub fn method_not_found(msg: impl Into<String>) -> Self {
        Self { code: -32601, message: msg.into(), data: None }
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self { code: -32602, message: msg.into(), data: None }
    }

    pub fn internal_error(msg: impl Into<String>) -> Self {
        Self { code: -32603, message: msg.into(), data: None }
    }
}

// ========================================================================
// MCP Protocol Message Types
// ========================================================================

/// Initialize request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: Implementation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootsCapability {
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingCapability {}

/// Initialize response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: Implementation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsCapability {
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingCapability {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcesCapability {
    pub subscribe: bool,
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptsCapability {
    pub list_changed: bool,
}

/// Implementation info (client/server identification)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

// ========================================================================
// Tool Types
// ========================================================================

/// Tool definition as exposed via MCP
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

/// Tool list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpToolDefinition>,
}

/// Tool call request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolRequest {
    pub name: String,
    #[serde(default)]
    pub arguments: Option<Value>,
}

/// Tool call result content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolCallContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource {
        uri: String,
        mime_type: Option<String>,
        #[serde(rename = "blob")]
        blob: Option<String>,
    },
}

/// Tool call response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<ToolCallContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

// ========================================================================
// Resource Types
// ========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResourcesResult {
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceRequest {
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResourceContent {
    #[serde(rename = "text")]
    Text { text: String, uri: String },
    #[serde(rename = "blob")]
    Blob { blob: String, mime_type: String, uri: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContent>,
}

// ========================================================================
// Prompt Types
// ========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPromptsResult {
    pub prompts: Vec<Prompt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetPromptResult {
    pub description: String,
    pub messages: Vec<PromptMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: String,
    pub content: PromptContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PromptContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "resource")]
    Resource { uri: String, mime_type: Option<String> },
}

// ========================================================================
// Transport Layer
// ========================================================================

/// Trait for MCP transport (stdio, HTTP SSE, WebSocket, etc.)
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and receive response
    async fn send(&mut self, request: JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Receive a notification (server push)
    async fn recv_notification(&mut self) -> Result<JsonRpcRequest>;

    /// Close the transport
    async fn close(&mut self) -> Result<()>;
}

/// Stdio transport (stdin/stdout JSON-RPC lines)
pub struct StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    reader: R,
    writer: W,
    buffer: String,
}

impl<R, W> StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            buffer: String::new(),
        }
    }
}

/// Process-based transport for child process I/O
/// This wraps a spawned child process and communicates via stdin/stdout
pub struct ProcessMcpTransport {
    child: Option<tokio::process::Child>,
}

impl ProcessMcpTransport {
    pub fn spawn(command: &str, args: &[String]) -> Result<Self> {
        let child = tokio::process::Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn '{}': {}", command, e))?;

        Ok(Self { child: Some(child) })
    }

    fn get_stdin(&mut self) -> Result<&mut tokio::process::ChildStdin> {
        self.child.as_mut()
            .and_then(|c| c.stdin.as_mut())
            .ok_or_else(|| anyhow::anyhow!("No stdin available"))
    }

    fn get_stdout(&mut self) -> Result<&mut tokio::process::ChildStdout> {
        self.child.as_mut()
            .and_then(|c| c.stdout.as_mut())
            .ok_or_else(|| anyhow::anyhow!("No stdout available"))
    }
}

#[async_trait]
impl McpTransport for ProcessMcpTransport {
    async fn send(&mut self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        use tokio::io::AsyncWriteExt;

        let line = serde_json::to_string(&request)?;
        debug!(request = %line, "MCP sending (process)");

        let stdin = self.get_stdin()?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        self.read_response().await
    }

    async fn recv_notification(&mut self) -> Result<JsonRpcRequest> {
        let line = self.read_line().await?;
        let request: JsonRpcRequest = serde_json::from_str(&line)?;
        Ok(request)
    }

    async fn close(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            child.kill().await.ok();
        }
        Ok(())
    }
}

impl ProcessMcpTransport {
    async fn read_line(&mut self) -> Result<String> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let stdout = self.get_stdout()?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let line = line.trim_end().to_string();

        if line.is_empty() {
            Err(anyhow::anyhow!("EOF on MCP process transport"))
        } else {
            Ok(line)
        }
    }

    async fn read_response(&mut self) -> Result<JsonRpcResponse> {
        let line = self.read_line().await?;
        let response: JsonRpcResponse = serde_json::from_str(&line)?;
        Ok(response)
    }
}

#[async_trait]
impl<R, W> McpTransport for StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send + Sync,
    W: AsyncWrite + Unpin + Send + Sync,
{
    async fn send(&mut self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        use tokio::io::AsyncWriteExt;

        let line = serde_json::to_string(&request)?;
        debug!(request = %line, "MCP sending");

        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;

        let response_line = self.read_line().await?;
        let response: JsonRpcResponse = serde_json::from_str(&response_line)?;

        Ok(response)
    }

    async fn recv_notification(&mut self) -> Result<JsonRpcRequest> {
        let line = self.read_line().await?;
        let request: JsonRpcRequest = serde_json::from_str(&line)?;
        Ok(request)
    }

    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

impl<R, W> StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    async fn read_line(&mut self) -> Result<String> {
        use tokio::io::AsyncBufReadExt;
        use tokio::io::BufReader;

        let mut reader = BufReader::new(&mut self.reader);
        self.buffer.clear();
        reader.read_line(&mut self.buffer).await?;
        let line = self.buffer.trim_end().to_string();

        if line.is_empty() {
            Err(anyhow::anyhow!("EOF on MCP transport"))
        } else {
            Ok(line)
        }
    }
}

/// HTTP/SSE transport for remote MCP servers
pub struct HttpSseTransport {
    base_url: String,
    client: reqwest::Client,
    session_id: Option<String>,
}

impl HttpSseTransport {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            client: reqwest::Client::new(),
            session_id: None,
        }
    }

    async fn ensure_session(&mut self) -> Result<&str> {
        if self.session_id.is_none() {
            let url = format!("{}/sse", self.base_url);
            let resp = self.client.get(&url).send().await?;
            if resp.status().is_success() {
                let body = resp.text().await?;
                self.session_id = Some(body);
            } else {
                return Err(anyhow::anyhow!("Failed to establish SSE session: {}", resp.status()));
            }
        }
        Ok(self.session_id.as_deref().unwrap())
    }
}

#[async_trait]
impl McpTransport for HttpSseTransport {
    async fn send(&mut self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.ensure_session().await?;

        let url = format!("{}/message?sessionId={}", self.base_url, self.session_id.as_deref().unwrap());
        let resp = self.client
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(anyhow::anyhow!("HTTP error: {}", resp.status()));
        }

        let response: JsonRpcResponse = resp.json().await?;
        Ok(response)
    }

    async fn recv_notification(&mut self) -> Result<JsonRpcRequest> {
        Err(anyhow::anyhow!("SSE notification receive not yet implemented"))
    }

    async fn close(&mut self) -> Result<()> {
        self.session_id = None;
        Ok(())
    }
}

// ========================================================================
// MCP Server
// ========================================================================

/// Callback type for tool execution
pub type ToolHandler = Arc<dyn Fn(CallToolRequest) -> std::future::Ready<Result<CallToolResult>> + Send + Sync>;

/// MCP Server configuration
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub server_info: Implementation,
    pub capabilities: ServerCapabilities,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            server_info: Implementation {
                name: "carpai-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: ServerCapabilities {
                tools: Some(ToolsCapability { list_changed: true }),
                logging: Some(LoggingCapability {}),
                resources: Some(ResourcesCapability {
                    subscribe: false,
                    list_changed: true,
                }),
                prompts: Some(PromptsCapability { list_changed: true }),
            },
        }
    }
}

/// MCP Server — exposes CarpAI tools via the MCP protocol
pub struct McpServer {
    config: McpServerConfig,
    tools: RwLock<HashMap<String, (McpToolDefinition, ToolHandler)>>,
    resources: RwLock<HashMap<String, Resource>>,
    prompts: RwLock<HashMap<String, Prompt>>,
    initialized: Mutex<bool>,
}

impl McpServer {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            tools: RwLock::new(HashMap::new()),
            resources: RwLock::new(HashMap::new()),
            prompts: RwLock::new(HashMap::new()),
            initialized: Mutex::new(false),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(McpServerConfig::default())
    }

    /// Register a tool handler
    pub async fn register_tool(
        &self,
        definition: McpToolDefinition,
        handler: ToolHandler,
    ) {
        let tool_name = definition.name.clone();
        let mut tools = self.tools.write().await;
        tools.insert(tool_name.clone(), (definition, handler));
        info!(tool = %tool_name, "MCP tool registered");
    }

    /// Register a resource
    pub async fn register_resource(&self, resource: Resource) {
        let mut resources = self.resources.write().await;
        resources.insert(resource.uri.clone(), resource);
    }

    /// Register a prompt template
    pub async fn register_prompt(&self, prompt: Prompt) {
        let mut prompts = self.prompts.write().await;
        prompts.insert(prompt.name.clone(), prompt);
    }

    /// Handle an incoming JSON-RPC request
    pub async fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id.unwrap_or(JsonRpcId::Null);

        match request.method.as_str() {
            "initialize" => self.handle_initialize(id, request.params).await,
            "notifications/initialized" => {
                *self.initialized.lock().await = true;
                JsonRpcResponse::notification_ok()
            }
            "tools/list" => self.handle_tools_list(id).await,
            "tools/call" => self.handle_tools_call(id, request.params).await,
            "resources/list" => self.handle_resources_list(id).await,
            "resources/read" => self.handle_resources_read(id, request.params).await,
            "prompts/list" => self.handle_prompts_list(id).await,
            "prompts/get" => self.handle_prompts_get(id, request.params).await,
            _ => JsonRpcResponse::error(id, JsonRpcError::method_not_found(format!(
                "Unknown method: {}", request.method
            ))),
        }
    }

    async fn handle_initialize(&self, id: JsonRpcId, params: Option<Value>) -> JsonRpcResponse {
        if let Some(params) = params {
            if let Ok(init_req) = serde_json::from_value::<InitializeRequest>(params) {
                info!(
                    client = %init_req.client_info.name,
                    version = %init_req.client_info.version,
                    protocol = %init_req.protocol_version,
                    "MCP initialize"
                );
            }
        }

        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: self.config.capabilities.clone(),
            server_info: self.config.server_info.clone(),
        };

        JsonRpcResponse::success(id, json!(result))
    }

    async fn handle_tools_list(&self, id: JsonRpcId) -> JsonRpcResponse {
        let tools = self.tools.read().await;
        let definitions: Vec<McpToolDefinition> = tools
            .values()
            .map(|(def, _)| def.clone())
            .collect();

        let result = ListToolsResult { tools: definitions };
        JsonRpcResponse::success(id, json!(result))
    }

    async fn handle_tools_call(&self, id: JsonRpcId, params: Option<Value>) -> JsonRpcResponse {
        let call_req: CallToolRequest = match params.and_then(|p| serde_json::from_value(p).ok()) {
            Some(req) => req,
            None => return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing tool call parameters")),
        };

        let tools = self.tools.read().await;
        match tools.get(&call_req.name) {
            Some((_, handler)) => {
                match handler(call_req).await {
                    Ok(result) => JsonRpcResponse::success(id, json!(result)),
                    Err(e) => JsonRpcResponse::error(id, JsonRpcError::internal_error(e.to_string())),
                }
            }
            None => JsonRpcResponse::error(id, JsonRpcError::method_not_found(format!(
                "Tool not found: {}", call_req.name
            ))),
        }
    }

    async fn handle_resources_list(&self, id: JsonRpcId) -> JsonRpcResponse {
        let resources = self.resources.read().await;
        let result = ListResourcesResult {
            resources: resources.values().cloned().collect(),
        };
        JsonRpcResponse::success(id, json!(result))
    }

    async fn handle_resources_read(&self, id: JsonRpcId, params: Option<Value>) -> JsonRpcResponse {
        let read_req: ReadResourceRequest = match params.and_then(|p| serde_json::from_value(p).ok()) {
            Some(req) => req,
            None => return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing read parameters")),
        };

        let result = ReadResourceResult {
            contents: vec![ResourceContent::Text {
                text: format!("Resource at URI: {}", read_req.uri),
                uri: read_req.uri,
            }],
        };
        JsonRpcResponse::success(id, json!(result))
    }

    async fn handle_prompts_list(&self, id: JsonRpcId) -> JsonRpcResponse {
        let prompts = self.prompts.read().await;
        let result = ListPromptsResult {
            prompts: prompts.values().cloned().collect(),
        };
        JsonRpcResponse::success(id, json!(result))
    }

    async fn handle_prompts_get(&self, id: JsonRpcId, params: Option<Value>) -> JsonRpcResponse {
        let get_req: GetPromptRequest = match params.and_then(|p| serde_json::from_value(p).ok()) {
            Some(req) => req,
            None => return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing prompt get parameters")),
        };

        let prompts = self.prompts.read().await;
        match prompts.get(&get_req.name) {
            Some(prompt) => {
                let result = GetPromptResult {
                    description: prompt.description.clone().unwrap_or_default(),
                    messages: vec![PromptMessage {
                        role: "user".to_string(),
                        content: PromptContent::Text {
                            text: format!("Prompt: {}", prompt.name),
                        },
                    }],
                };
                JsonRpcResponse::success(id, json!(result))
            }
            None => JsonRpcResponse::error(id, JsonRpcError::method_not_found(format!(
                "Prompt not found: {}", get_req.name
            ))),
        }
    }
}

impl JsonRpcResponse {
    pub fn success(id: JsonRpcId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: JsonRpcResult::Success { result },
        }
    }

    pub fn error(id: JsonRpcId, err: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: JsonRpcResult::Error { error: err },
        }
    }

    pub fn notification_ok() -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Null,
            result: JsonRpcResult::Success { result: json!(null) },
        }
    }
}

// ========================================================================
// MCP Client
// ========================================================================

/// MCP Client configuration
#[derive(Debug, Clone)]
pub struct McpClientConfig {
    pub server_name: String,
    pub client_info: Implementation,
}

impl Default for McpClientConfig {
    fn default() -> Self {
        Self {
            server_name: "unknown-mcp-server".to_string(),
            client_info: Implementation {
                name: "carpai-client".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        }
    }
}

/// MCP Client — connects to an external MCP server to discover/use its tools
pub struct McpClient<T: McpTransport> {
    config: McpClientConfig,
    transport: Mutex<T>,
    server_capabilities: Mutex<Option<ServerCapabilities>>,
    cached_tools: Mutex<Vec<McpToolDefinition>>,
}

impl<T: McpTransport> McpClient<T> {
    pub fn new(config: McpClientConfig, transport: T) -> Self {
        Self {
            config,
            transport: Mutex::new(transport),
            server_capabilities: Mutex::new(None),
            cached_tools: Mutex::new(Vec::new()),
        }
    }

    /// Initialize connection to the MCP server
    pub async fn initialize(&self) -> Result<InitializeResult> {
        let mut transport = self.transport.lock().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Num(1)),
            method: "initialize".to_string(),
            params: Some(json!(InitializeRequest {
                protocol_version: "2024-11-05".to_string(),
                capabilities: ClientCapabilities {
                    roots: Some(RootsCapability { list_changed: false }),
                    sampling: None,
                },
                client_info: self.config.client_info.clone(),
            })),
        };

        let response = transport.send(request).await?;

        match response.result {
            JsonRpcResult::Success { result } => {
                let init_result: InitializeResult = serde_json::from_value(result)?;
                *self.server_capabilities.lock().await = Some(init_result.capabilities.clone());

                let notif = JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    method: "notifications/initialized".to_string(),
                    params: None,
                };
                let _ = transport.send(notif).await;

                info!(
                    server = %self.config.server_name,
                    version = %init_result.server_info.version,
                    "MCP client initialized"
                );

                Ok(init_result)
            }
            JsonRpcResult::Error { error } => {
                Err(anyhow::anyhow!("MCP initialize failed: {} ({})", error.message, error.code))
            }
        }
    }

    /// List available tools from the MCP server
    pub async fn list_tools(&self) -> Result<Vec<McpToolDefinition>> {
        let mut transport = self.transport.lock().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Num(2)),
            method: "tools/list".to_string(),
            params: None,
        };

        let response = transport.send(request).await?;

        match response.result {
            JsonRpcResult::Success { result } => {
                let list_result: ListToolsResult = serde_json::from_value(result)?;
                *self.cached_tools.lock().await = list_result.tools.clone();
                Ok(list_result.tools)
            }
            JsonRpcResult::Error { error } => {
                Err(anyhow::anyhow!("MCP tools/list failed: {}", error.message))
            }
        }
    }

    /// Call a tool on the MCP server
    pub async fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<CallToolResult> {
        let mut transport = self.transport.lock().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Num(Uuid::new_v4().as_u128() as i64)),
            method: "tools/call".to_string(),
            params: Some(json!(CallToolRequest {
                name: name.to_string(),
                arguments,
            })),
        };

        let response = transport.send(request).await?;

        match response.result {
            JsonRpcResult::Success { result } => {
                let call_result: CallToolResult = serde_json::from_value(result)?;
                Ok(call_result)
            }
            JsonRpcResult::Error { error } => {
                Err(anyhow::anyhow!("MCP tools/call '{}' failed: {}", name, error.message))
            }
        }
    }

    /// List available resources
    pub async fn list_resources(&self) -> Result<Vec<Resource>> {
        let mut transport = self.transport.lock().await;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(JsonRpcId::Num(3)),
            method: "resources/list".to_string(),
            params: None,
        };

        let response = transport.send(request).await?;

        match response.result {
            JsonRpcResult::Success { result } => {
                let list_result: ListResourcesResult = serde_json::from_value(result)?;
                Ok(list_result.resources)
            }
            JsonRpcResult::Error { error } => {
                Err(anyhow::anyhow!("MCP resources/list failed: {}", error.message))
            }
        }
    }

    /// Close the connection
    pub async fn close(&self) -> Result<()> {
        let mut transport = self.transport.lock().await;
        transport.close().await
    }
}

// ========================================================================
// Shared MCP Pool (process reuse across sessions)
// ========================================================================

/// A shared pool of MCP server connections that can be reused across multiple sessions.
/// This avoids spawning duplicate MCP server processes.
pub struct SharedMcpPool {
    servers: RwLock<HashMap<String, Arc<dyn McpClientTrait>>>,
}

impl SharedMcpPool {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
        }
    }

    /// Get or create an MCP client for the given server config key
    pub async fn get_or_create(
        &self,
        server_key: &str,
        command: &str,
        args: &[String],
    ) -> Result<Arc<dyn McpClientTrait>> {
        let mut servers = self.servers.write().await;

        if let Some(client) = servers.get(server_key) {
            return Ok(client.clone());
        }

        info!(server = %server_key, cmd = %command, "Creating new MCP client in shared pool");

        let transport = ProcessMcpTransport::spawn(command, args)?;

        let config = McpClientConfig {
            server_name: server_key.to_string(),
            ..Default::default()
        };

        let client: Arc<dyn McpClientTrait> = Arc::new(McpClient::new(config, transport));
        client.initialize().await?;

        servers.insert(server_key.to_string(), client.clone());
        Ok(client)
    }

    /// Remove a server from the pool (disconnects it)
    pub async fn remove(&self, server_key: &str) -> Option<Arc<dyn McpClientTrait>> {
        let mut servers = self.servers.write().await;
        servers.remove(server_key)
    }

    /// Get all connected server names
    pub async fn server_names(&self) -> Vec<String> {
        let servers = self.servers.read().await;
        servers.keys().cloned().collect()
    }
}

impl Default for SharedMcpPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Global singleton for the shared MCP pool
static GLOBAL_MCP_POOL: once_cell::sync::OnceCell<Arc<SharedMcpPool>> = OnceCell::new();

pub fn get_shared_pool() -> Arc<SharedMcpPool> {
    GLOBAL_MCP_POOL
        .get_or_init(|| Arc::new(SharedMcpPool::new()))
        .clone()
}

pub fn init_shared_pool() -> Arc<SharedMcpPool> {
    let pool = Arc::new(SharedMcpPool::new());
    let _ = GLOBAL_MCP_POOL.set(pool.clone());
    pool
}

// ========================================================================
// MCP Manager (manages multiple MCP server connections)
// ========================================================================

/// Configuration for a single MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfigEntry {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Manages connections to multiple MCP servers
pub struct McpManager {
    config: McpManagerConfig,
    clients: RwLock<HashMap<String, Arc<dyn McpClientTrait>>>,
    shared_pool: Option<Arc<SharedMcpPool>>,
    session_id: String,
}

/// Trait object for MCP clients to allow different transport types
#[async_trait]
pub trait McpClientTrait: Send + Sync {
    async fn initialize(&self) -> Result<InitializeResult>;
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>>;
    async fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<CallToolResult>;
    async fn close(&self) -> Result<()>;
}

#[async_trait]
impl<T: McpTransport + 'static> McpClientTrait for McpClient<T> {
    async fn initialize(&self) -> Result<InitializeResult> {
        self.initialize().await
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>> {
        self.list_tools().await
    }

    async fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<CallToolResult> {
        self.call_tool(name, arguments).await
    }

    async fn close(&self) -> Result<()> {
        self.close().await
    }
}

#[derive(Debug, Clone, Default)]
pub struct McpManagerConfig {
    pub servers: Vec<McpServerConfigEntry>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            config: McpManagerConfig::default(),
            clients: RwLock::new(HashMap::new()),
            shared_pool: None,
            session_id: Uuid::new_v4().to_string(),
        }
    }

    pub fn with_shared_pool(pool: Arc<SharedMcpPool>, session_id: String) -> Self {
        Self {
            config: McpManagerConfig::default(),
            clients: RwLock::new(HashMap::new()),
            shared_pool: Some(pool),
            session_id,
        }
    }

    pub fn with_config(config: McpManagerConfig) -> Self {
        Self {
            config,
            clients: RwLock::new(HashMap::new()),
            shared_pool: None,
            session_id: Uuid::new_v4().to_string(),
        }
    }

    pub fn config(&self) -> &McpManagerConfig {
        &self.config
    }

    /// Connect to all configured MCP servers
    pub async fn connect_all(&self) -> Result<(usize, Vec<(String, String)>)> {
        let mut successes = 0;
        let mut failures = Vec::new();

        for entry in &self.config.servers {
            if entry.enabled == Some(false) {
                continue;
            }

            let result = self.connect_server(entry).await;
            match result {
                Ok(_) => successes += 1,
                Err(e) => failures.push((entry.name.clone(), e.to_string())),
            }
        }

        Ok((successes, failures))
    }

    async fn connect_server(&self, entry: &McpServerConfigEntry) -> Result<()> {
        let client = if let Some(ref pool) = self.shared_pool {
            pool.get_or_create(&entry.name, &entry.command, &entry.args).await?
        } else {
            let transport = ProcessMcpTransport::spawn(&entry.command, &entry.args)?;

            let config = McpClientConfig {
                server_name: entry.name.clone(),
                ..Default::default()
            };

            let client = McpClient::new(config, transport);
            client.initialize().await?;
            Arc::new(client)
        };

        let mut clients = self.clients.write().await;
        clients.insert(entry.name.clone(), client);

        info!(server = %entry.name, "MCP server connected");
        Ok(())
    }

    /// Get all tools from all connected MCP servers
    pub async fn get_all_tools(&self) -> Result<Vec<(String, McpToolDefinition)>> {
        let clients = self.clients.read().await;
        let mut all_tools = Vec::new();

        for (server_name, client) in clients.iter() {
            match client.list_tools().await {
                Ok(tools) => {
                    for tool in tools {
                        let full_name = format!("mcp__{}__{}", server_name, tool.name);
                        all_tools.push((full_name, tool));
                    }
                }
                Err(e) => {
                    warn!(server = %server_name, error = %e, "Failed to list tools from MCP server");
                }
            }
        }

        Ok(all_tools)
    }

    /// Call a tool on a specific MCP server
    pub async fn call_tool(&self, mcp_tool_name: &str, arguments: Option<Value>) -> Result<CallToolResult> {
        let parts: Vec<&str> = mcp_tool_name.splitn(3, "__").collect();
        if parts.len() != 3 || parts[0] != "mcp" {
            return Err(anyhow::anyhow!("Invalid MCP tool name format: {}", mcp_tool_name));
        }

        let server_name = parts[1];
        let tool_name = parts[2];

        let clients = self.clients.read().await;
        let client = clients.get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server not connected: {}", server_name))?;

        client.call_tool(tool_name, arguments).await
    }
}

// ========================================================================
// MCP Bridge (Bidirectional: Server + Client in one process)
// ========================================================================

/// Bridge capabilities configuration
#[derive(Debug, Clone)]
pub struct BridgeCapabilities {
    pub server_enabled: bool,
    pub client_enabled: bool,
}

impl Default for BridgeCapabilities {
    fn default() -> Self {
        Self {
            server_enabled: true,
            client_enabled: true,
        }
    }
}

/// Bridge status
#[derive(Debug, Clone)]
pub struct BridgeStatus {
    pub server_running: bool,
    pub client_connected_servers: usize,
    pub total_tools_available: usize,
}

/// Bidirectional MCP bridge — runs as both server and client
pub struct McpBridge {
    server: Option<Arc<McpServer>>,
    manager: McpManager,
    config: McpBridgeConfig,
}

#[derive(Debug, Clone)]
pub struct McpBridgeConfig {
    pub server_config: Option<McpServerConfig>,
    pub mcp_servers: Vec<McpServerConfigEntry>,
    pub capabilities: BridgeCapabilities,
}

impl Default for McpBridgeConfig {
    fn default() -> Self {
        Self {
            server_config: None,
            mcp_servers: Vec::new(),
            capabilities: BridgeCapabilities::default(),
        }
    }
}

impl McpBridge {
    pub fn new(config: McpBridgeConfig) -> Self {
        let server = config.server_config.as_ref().map(|sc| Arc::new(McpServer::new(sc.clone())));

        let manager_config = McpManagerConfig {
            servers: config.mcp_servers.clone(),
        };

        Self {
            server,
            manager: McpManager::with_config(manager_config),
            config,
        }
    }

    /// Start the bridge (connect clients, optionally start server)
    pub async fn start(&self) -> Result<BridgeStatus> {
        let mut status = BridgeStatus {
            server_running: false,
            client_connected_servers: 0,
            total_tools_available: 0,
        };

        if self.config.capabilities.client_enabled && !self.manager.config.servers.is_empty() {
            let (connected, _) = self.manager.connect_all().await?;
            status.client_connected_servers = connected;
        }

        if self.config.capabilities.server_enabled && self.server.is_some() {
            status.server_running = true;
        }

        let tools = self.manager.get_all_tools().await.unwrap_or_default();
        status.total_tools_available = tools.len();

        Ok(status)
    }

    /// Get reference to the server (for serving over stdio)
    pub fn server(&self) -> Option<&Arc<McpServer>> {
        self.server.as_ref()
    }

    /// Get reference to the manager (for calling external tools)
    pub fn manager(&self) -> &McpManager {
        &self.manager
    }

    /// Stop the bridge and disconnect all clients
    pub async fn stop(&self) -> Result<()> {
        let clients = self.manager.clients.read().await;
        for (_, client) in clients.iter() {
            let _ = client.close().await;
        }
        Ok(())
    }
}

// ========================================================================
// MCP Tool Wrapper (wraps MCP server tools into local Tool interface)
// ========================================================================

/// Wraps an MCP tool so it can be used through the local ToolExecutor
pub struct McpTool {
    pub name: String,
    pub definition: McpToolDefinition,
    pub manager: Arc<McpManager>,
}

impl McpTool {
    pub fn new(name: String, definition: McpToolDefinition, manager: Arc<McpManager>) -> Self {
        Self {
            name,
            definition,
            manager,
        }
    }

    /// Execute this MCP tool by forwarding to the MCP server
    pub async fn execute(&self, arguments: Option<Value>) -> Result<ToolResponse> {
        let start = std::time::Instant::now();
        let result = self.manager.call_tool(&self.name, arguments).await?;
        let duration_ms = start.elapsed().as_millis() as u64;

        let output_text: Vec<String> = result.content.iter().map(|c| match c {
            ToolCallContent::Text { text } => text.clone(),
            ToolCallContent::Image { .. } => "[image]".to_string(),
            ToolCallContent::Resource { uri, .. } => format!("[resource: {}]", uri),
        }).collect();

        Ok(ToolResponse {
            success: result.is_error != Some(true),
            output: output_text.join("\n"),
            data: None,
            exit_code: if result.is_error == Some(true) { Some(1) } else { Some(0) },
            duration_ms,
            request_id: Uuid::new_v4().to_string(),
            tool_name: self.name.clone(),
            audit_id: None,
        })
    }

    /// Convert to carpai_internal ToolSchema
    pub fn to_schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.clone(),
            description: self.definition.description.clone().unwrap_or_default(),
            parameters_json_schema: self.definition.input_schema.clone(),
            category: ToolCategory::Custom,
            requires_confirmation: false,
            timeout_secs: 30,
            default_mode: carpai_internal::ExecutionMode::Remote {
                endpoint: format!("mcp://{}", self.name),
            },
            required_permissions: vec![],
        }
    }
}

/// Create McpTool wrappers for all tools from an McpManager
pub async fn create_mcp_tools(manager: Arc<McpManager>) -> Vec<(String, Arc<McpTool>)> {
    match manager.get_all_tools().await {
        Ok(tools) => tools
            .into_iter()
            .map(|(name, def)| {
                let tool = Arc::new(McpTool::new(name.clone(), def, manager.clone()));
                (name, tool)
            })
            .collect(),
        Err(e) => {
            error!(error = %e, "Failed to create MCP tools");
            Vec::new()
        }
    }
}

// ========================================================================
// Audit Log (lightweight implementation)
// ========================================================================

/// Audit log entry for tool invocations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub tool_name: String,
    pub parameters: Option<Value>,
    pub success: bool,
    pub error_message: Option<String>,
    pub duration_ms: u64,
}

/// Filter for audit log queries
#[derive(Debug, Clone, Default)]
pub struct AuditLogFilter {
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub success: Option<bool>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    pub limit: Option<usize>,
}

/// Audit log statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogStats {
    pub total_entries: u64,
    pub successful: u64,
    pub failed: u64,
    pub avg_duration_ms: f64,
    pub unique_tools: usize,
    pub unique_users: usize,
}

/// Simple in-memory audit logger
pub struct AuditLogger {
    entries: Mutex<Vec<AuditLogEntry>>,
}

impl AuditLogger {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    pub async fn record_invocation(
        &self,
        user_id: Option<String>,
        session_id: Option<String>,
        tool_name: String,
        params: Option<Value>,
        _result: Option<Value>,
        success: bool,
        error_message: Option<String>,
        duration_ms: u64,
    ) -> Result<()> {
        let entry = AuditLogEntry {
            id: Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            user_id,
            session_id,
            tool_name,
            parameters: params,
            success,
            error_message,
            duration_ms,
        };

        let mut entries = self.entries.lock().await;
        entries.push(entry);
        Ok(())
    }

    pub async fn query(&self, filter: &AuditLogFilter) -> Result<Vec<AuditLogEntry>> {
        let entries = self.entries.lock().await;
        let filtered: Vec<AuditLogEntry> = entries
            .iter()
            .filter(|e| {
                if let Some(ref uid) = filter.user_id {
                    if e.user_id.as_ref() != Some(uid) { return false; }
                }
                if let Some(ref sid) = filter.session_id {
                    if e.session_id.as_ref() != Some(sid) { return false; }
                }
                if let Some(ref tn) = filter.tool_name {
                    if &e.tool_name != tn { return false; }
                }
                if let Some(s) = filter.success {
                    if e.success != s { return false; }
                }
                if let Some(since) = filter.since {
                    if e.timestamp < since { return false; }
                }
                if let Some(until) = filter.until {
                    if e.timestamp > until { return false; }
                }
                true
            })
            .cloned()
            .collect();

        let limited = if let Some(limit) = filter.limit {
            filtered.into_iter().take(limit).collect()
        } else {
            filtered
        };

        Ok(limited)
    }

    pub async fn stats(&self) -> Result<AuditLogStats> {
        let entries = self.entries.lock().await;
        let total = entries.len() as u64;
        let successful = entries.iter().filter(|e| e.success).count() as u64;
        let failed = total - successful;
        let avg_duration = if total > 0 {
            entries.iter().map(|e| e.duration_ms as f64).sum::<f64>() / total as f64
        } else {
            0.0
        };
        let unique_tools = entries.iter().map(|e| e.tool_name.clone()).collect::<std::collections::HashSet<_>>().len();
        let unique_users = entries.iter().filter_map(|e| e.user_id.clone()).collect::<std::collections::HashSet<_>>().len();

        Ok(AuditLogStats {
            total_entries: total,
            successful,
            failed,
            avg_duration_ms: avg_duration,
            unique_tools,
            unique_users,
        })
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Re-exports for convenience
// ========================================================================

pub type McpHandle<T> = McpClient<T>;
