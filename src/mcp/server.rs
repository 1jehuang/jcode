//! MCP Server mode — exposes CarpAI's tools via the Model Context Protocol.
//!
//! Start with: carpai mcp serve
//! External tools connect via stdin/stdout JSON-RPC, calling CarpAI's tools
//! through the standard MCP protocol.
//!
//! Features:
//! - MCP protocol version 2024-11-05 (RFC-compliant Content-Length framing)
//! - tools/list, tools/call
//! - resources/list, resources/read (dynamic workspace resources)
//! - prompts/list, prompts/get (built-in prompt templates)
//! - logging/setLevel (no-op for compatibility)
//! - Graceful shutdown notification handling

use crate::tool::{Registry, ToolContext, ToolExecutionMode};
use crate::mcp::dynamic_registry::{DynamicToolRegistry, DynamicTool, RegisterResult, UnregisterResult};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};
use std::sync::Arc;

// --- Configuration ---------------------------------------------

/// MCP server configuration
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Whether to expose workspace resources
    pub expose_resources: bool,
    /// Custom server name (default: "carpai")
    pub server_name: String,
    /// Extra tool definitions to expose alongside registry tools
    pub extra_tools: Vec<ExtraToolDef>,
}

#[derive(Debug, Clone)]
pub struct ExtraToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            expose_resources: false,
            server_name: "carpai".to_string(),
            extra_tools: Vec::new(),
        }
    }
}

// --- Server ----------------------------------------------------

/// MCP Server — reads JSON-RPC from stdin, dispatches, writes to stdout.
/// Supports RFC-compliant Content-Length framing.
pub struct McpServer {
    registry: Arc<Registry>,
    dynamic_registry: Arc<DynamicToolRegistry>,
    initialized: bool,
    config: McpServerConfig,
}

impl McpServer {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry: Arc::new(registry),
            dynamic_registry: Arc::new(DynamicToolRegistry::with_defaults()),
            initialized: false,
            config: McpServerConfig::default(),
        }
    }

    pub fn with_config(mut self, config: McpServerConfig) -> Self {
        self.config = config;
        self
    }

    /// Get access to the dynamic tool registry (for programmatic registration)
    pub fn dynamic_registry(&self) -> Arc<DynamicToolRegistry> {
        Arc::clone(&self.dynamic_registry)
    }

    /// Start the MCP server loop — reads JSON-RPC from stdin, dispatches, writes to stdout.
    /// Uses Content-Length framing per MCP specification.
    pub async fn serve(&mut self) -> Result<()> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut header_buf = String::new();

        loop {
            header_buf.clear();

            // Read Content-Length header
            let content_length = match read_content_length(&mut stdin.lock(), &mut header_buf) {
                Ok(Some(len)) => len,
                Ok(None) => break, // EOF
                Err(e) => {
                    eprintln!("[MCP] Header parse error: {}", e);
                    continue;
                }
            };

            // Read empty line separator after headers
            header_buf.clear();
            if stdin.lock().read_line(&mut header_buf)? == 0 {
                break;
            }

            // Read the JSON body
            let mut body = vec![0u8; content_length];
            read_exact(&mut stdin.lock(), &mut body)?;

            let line = String::from_utf8_lossy(&body);
            let response = self.handle(line.trim()).await;

            if !response.is_empty() {
                write_content_length(&mut stdout.lock(), &response)?;
                stdout.lock().flush()?;
            }
        }

        Ok(())
    }

    async fn handle(&mut self, line: &str) -> String {
        let req: Value = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => return error_json(None, -32700, format!("Parse error: {}", e)),
        };
        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = req.get("id");
        let params = req.get("params");

        match method {
            "initialize" => self.handle_initialize(id, params),
            "notifications/initialized" => {
                self.initialized = true;
                String::new()
            }
            "notifications/cancelled" => String::new(),
            "ping" => json_result(id, &json!({})),
            "tools/list" => self.handle_tools_list(id).await,
            "tools/call" => self.handle_tools_call(id, params).await,
            // Dynamic tool registration methods
            "tools/register" => self.handle_tool_register(id, params).await,
            "tools/unregister" => self.handle_tool_unregister(id, params).await,
            "tools/search" => self.handle_tool_search(id, params).await,
            "tools/stats" => self.handle_tool_stats(id).await,
            // Standard MCP methods
            "resources/list" => self.handle_resources_list(id),
            "resources/read" => self.handle_resource_read(id, params),
            "prompts/list" => self.handle_prompts_list(id),
            "prompts/get" => self.handle_prompts_get(id, params),
            "logging/setLevel" => json_result(id, &json!({})),
            _ => error_json(id, -32601, format!("Method not found: {}", method)),
        }
    }

    fn handle_initialize(&self, id: Option<&Value>, params: Option<&Value>) -> String {
        if let Some(p) = params {
            if let Some(client_info) = p.get("clientInfo") {
                let name = client_info.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                let version = client_info.get("version").and_then(|v| v.as_str()).unwrap_or("?");
                eprintln!("[MCP] Client connected: {} v{}", name, version);
            }
        }

        let mut capabilities = json!({
            "tools": { "listChanged": false }
        });

        if self.config.expose_resources {
            capabilities["resources"] = json!({
                "subscribe": false,
                "listChanged": false
            });
        }

        json_result(id, &json!({
            "protocolVersion": "2024-11-05",
            "capabilities": capabilities,
            "serverInfo": {
                "name": self.config.server_name,
                "version": env!("CARGO_PKG_VERSION")
            }
        }))
    }

    async fn handle_tools_list(&self, id: Option<&Value>) -> String {
        let defs = self.registry.definitions(None).await;

        let tool_list: Vec<Value> = defs.iter().map(|d| {
            json!({
                "name": d.name,
                "description": d.description,
                "inputSchema": d.input_schema
            })
        }).collect();

        json_result(id, &json!({ "tools": tool_list }))
    }

    async fn handle_tools_call(&self, id: Option<&Value>, params: Option<&Value>) -> String {
        let name = params
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let arguments = params
            .and_then(|p| p.get("arguments"))
            .cloned()
            .unwrap_or(Value::Null);

        let tool_call_id = format!(
            "mcp-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos().to_string())
                .unwrap_or_default()
        );

        let ctx = ToolContext {
            session_id: "mcp-server".into(),
            message_id: tool_call_id.clone(),
            tool_call_id,
            working_dir: std::env::current_dir().ok(),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::Direct,
        };

        match self.registry.execute(name, arguments, ctx).await {
            Ok(output) => json_result(id, &json!({
                "content": [{ "type": "text", "text": output.output }],
                "isError": false
            })),
            Err(e) => json_result(id, &json!({
                "content": [{ "type": "text", "text": format!("Error: {}", e) }],
                "isError": true
            })),
        }
    }

    // --- Dynamic Tool Registration Methods -------------

    /// Handle tools/register - Register a new dynamic tool
    async fn handle_tool_register(&self, id: Option<&Value>, params: Option<&Value>) -> String {
        let tool_def = match params.and_then(|p| p.get("tool")) {
            Some(t) => t,
            None => return error_json(id, -32602, "Missing 'tool' parameter".to_string()),
        };

        let dynamic_tool = match serde_json::from_value::<DynamicTool>(tool_def.clone()) {
            Ok(t) => t,
            Err(e) => return error_json(id, -32602, format!("Invalid tool definition: {}", e)),
        };

        match self.dynamic_registry.register_tool(dynamic_tool).await {
            Ok(result) => json_result(id, &json!({ "result": result })),
            Err(e) => error_json(id, -32603, format!("Registration failed: {}", e)),
        }
    }

    /// Handle tools/unregister - Unregister a dynamic tool
    async fn handle_tool_unregister(&self, id: Option<&Value>, params: Option<&Value>) -> String {
        let name = match params.and_then(|p| p.get("name")).and_then(|v| v.as_str()) {
            Some(n) => n,
            None => return error_json(id, -32602, "Missing 'name' parameter".to_string()),
        };

        match self.dynamic_registry.unregister_tool(name).await {
            Ok(result) => json_result(id, &json!({ "result": result })),
            Err(e) => error_json(id, -32603, format!("Unregistration failed: {}", e)),
        }
    }

    /// Handle tools/search - Search tools by query
    async fn handle_tool_search(&self, id: Option<&Value>, params: Option<&Value>) -> String {
        let query = params
            .and_then(|p| p.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if query.is_empty() {
            return error_json(id, -32602, "Missing 'query' parameter".to_string());
        }

        let results = self.dynamic_registry.search_fuzzy(query).await;
        
        let tools: Vec<Value> = results.iter().map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "category": format!("{}", t.category),
                "version": t.version,
                "tags": t.tags,
                "enabled": t.enabled
            })
        }).collect();

        json_result(id, &json!({ 
            "tools": tools,
            "count": tools.len()
        }))
    }

    /// Handle tools/stats - Get registry statistics
    async fn handle_tool_stats(&self, id: Option<&Value>) -> String {
        let stats = self.dynamic_registry.get_stats().await;
        json_result(id, &json!({ "stats": stats }))
    }

    fn handle_resources_list(&self, id: Option<&Value>) -> String {
        if !self.config.expose_resources {
            return json_result(id, &json!({ "resources": [] }));
        }

        let mut resources = Vec::new();

        if let Ok(cwd) = std::env::current_dir() {
            resources.push(json!({
                "uri": "workspace://root",
                "name": "Workspace Root",
                "description": "Current workspace root directory",
                "mimeType": "text/plain"
            }));
            resources.push(json!({
                "uri": "workspace://git-status",
                "name": "Git Status",
                "description": "Current git status of the workspace",
                "mimeType": "text/plain"
            }));
        }

        json_result(id, &json!({ "resources": resources }))
    }

    fn handle_resource_read(&self, id: Option<&Value>, params: Option<&Value>) -> String {
        let uri = params
            .and_then(|p| p.get("uri"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let text = match uri {
            "workspace://root" => {
                std::env::current_dir()
                    .map(|p| format!("Workspace: {}", p.display()))
                    .unwrap_or_else(|_| "Workspace: unknown".to_string())
            }
            "workspace://git-status" => get_git_status(),
            _ => return error_json(id, -32602, format!("Resource not found: {}", uri)),
        };

        json_result(id, &json!({
            "contents": [{
                "uri": uri,
                "mimeType": "text/plain",
                "text": text
            }]
        }))
    }

    fn handle_prompts_list(&self, id: Option<&Value>) -> String {
        let prompts = vec![
            json!({
                "name": "summarize-workspace",
                "description": "Summarize the current workspace context",
                "arguments": []
            }),
            json!({
                "name": "analyze-changes",
                "description": "Analyze recent git changes",
                "arguments": [
                    { "name": "detail", "description": "Detail level: brief/normal/detailed", "required": false }
                ]
            }),
        ];

        json_result(id, &json!({ "prompts": prompts }))
    }

    fn handle_prompts_get(&self, id: Option<&Value>, params: Option<&Value>) -> String {
        let name = params
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match name {
            "summarize-workspace" => json_result(id, &json!({
                "description": "Summarize workspace context",
                "messages": [{
                    "role": "user",
                    "content": { "type": "text", "text": "Please summarize the current workspace." }
                }]
            })),
            "analyze-changes" => json_result(id, &json!({
                "description": "Analyze recent workspace changes",
                "messages": [{
                    "role": "user",
                    "content": { "type": "text", "text": "Analyze the recent git changes in this workspace." }
                }]
            })),
            _ => error_json(id, -32602, format!("Prompt not found: {}", name)),
        }
    }
}

// --- NoopProvider (for MCP Server mode without LLM) -------------

struct NoopProvider;

#[async_trait::async_trait]
impl crate::provider::Provider for NoopProvider {
    async fn complete(&self, _: &[crate::message::Message], _: &[crate::message::ToolDefinition], _: &str, _: Option<&str>) -> Result<crate::provider::EventStream> {
        anyhow::bail!("MCP Server mode - no LLM provider")
    }
    fn name(&self) -> &str { "mcp-server" }
    fn fork(&self) -> Arc<dyn crate::provider::Provider> { Arc::new(NoopProvider) }
    fn available_models_display(&self) -> Vec<String> { vec![] }
    async fn prefetch_models(&self) -> Result<()> { Ok(()) }
}

/// Create a noop provider for MCP server mode (no LLM).
pub fn noop_provider() -> Arc<dyn crate::provider::Provider> {
    Arc::new(NoopProvider)
}

// --- Legacy API (backward compatible) --------------------------

/// Start the MCP server: read JSON-RPC from stdin with Content-Length framing.
/// This is the entry point used by the CLI: `carpai mcp serve`
pub async fn serve() -> Result<()> {
    let provider = noop_provider();
    let registry = Registry::new(provider).await;
    let mut server = McpServer::new(registry);
    server.serve().await
}

// --- Protocol helpers ------------------------------------------

/// Read Content-Length header from stdin.
/// Returns `Ok(Some(length))` on success, `Ok(None)` on EOF.
fn read_content_length(reader: &mut impl BufRead, buf: &mut String) -> Result<Option<usize>> {
    loop {
        buf.clear();
        let bytes_read = reader.read_line(buf)?;
        if bytes_read == 0 {
            return Ok(None);
        }
        let line = buf.trim_end_matches(&['\r', '\n'][..]);
        if line.is_empty() {
            continue;
        }
        if line.to_ascii_lowercase().starts_with("content-length:") {
            let len_str = line.split(':').nth(1).unwrap_or("0").trim();
            let len: usize = len_str.parse().map_err(|e| {
                anyhow::anyhow!("Invalid Content-Length '{}': {}", len_str, e)
            })?;
            return Ok(Some(len));
        }
        // Skip other headers (Content-Type, etc.)
    }
}

/// Read exactly `len` bytes from reader.
fn read_exact(reader: &mut impl Read, buf: &mut [u8]) -> Result<()> {
    let mut offset = 0;
    while offset < buf.len() {
        let n = reader.read(&mut buf[offset..])?;
        if n == 0 {
            return Err(anyhow::anyhow!("Unexpected EOF while reading body"));
        }
        offset += n;
    }
    Ok(())
}

/// Write a JSON-RPC response with Content-Length framing.
fn write_content_length(writer: &mut impl Write, response: &str) -> Result<()> {
    let bytes = response.as_bytes();
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(bytes)?;
    Ok(())
}

fn json_result(id: Option<&Value>, result: &impl serde::Serialize) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

fn error_json(id: Option<&Value>, code: i32, message: String) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

fn get_git_status() -> String {
    let output = std::process::Command::new("git")
        .args(["status", "--short", "--branch"])
        .output()
        .ok();

    match output {
        Some(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => "Git status not available".to_string(),
    }
}
