//! MCP Protocol types (JSON-RPC 2.0)

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC response
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// MCP Initialize params
#[derive(Debug, Clone, Serialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ClientCapabilities {}

#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// MCP Initialize result
#[derive(Debug, Clone, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: Option<ToolsCapability>,
    #[serde(default)]
    pub resources: Option<ResourcesCapability>,
    #[serde(default)]
    pub prompts: Option<PromptsCapability>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged", default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ResourcesCapability {
    #[serde(default)]
    pub subscribe: bool,
    #[serde(rename = "listChanged", default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PromptsCapability {
    #[serde(rename = "listChanged", default)]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// MCP Tool definition from server
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// tools/list result
#[derive(Debug, Clone, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<McpToolDef>,
}

/// tools/call params
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallParams {
    pub name: String,
    pub arguments: Value,
}

/// tools/call result
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallResult {
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

/// Content block in tool result
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource { resource: ResourceContent },
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResourceContent {
    pub uri: String,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob: Option<String>,
}

/// MCP server configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Whether this server can be shared across sessions (default: true).
    /// Stateless API wrappers (Todoist, Canvas) should be shared.
    /// Stateful servers (Playwright browser) should not be shared.
    #[serde(default = "default_shared")]
    pub shared: bool,
}

fn default_shared() -> bool {
    true
}

/// Full MCP configuration file
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: std::collections::HashMap<String, McpServerConfig>,
}

impl McpConfig {
    /// Load config from file
    pub fn load_from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Save config to a JSON file
    pub fn save_to_file(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Import MCP servers from Claude Code and Codex CLI on first run.
    /// Only runs if ~/.jcode/mcp.json doesn't exist yet.
    fn import_from_external() {
        let jcode_mcp = match crate::storage::jcode_dir() {
            Ok(dir) => dir.join("mcp.json"),
            Err(_) => return,
        };

        if jcode_mcp.exists() {
            return; // Not first run
        }

        let mut imported = Self::default();
        let mut sources = Vec::new();

        // Import from Claude Code (~/.claude/mcp.json)
        if let Some(home) = dirs::home_dir() {
            let claude_mcp = home.join(".claude/mcp.json");
            if claude_mcp.exists() {
                if let Ok(config) = Self::load_from_file(&claude_mcp) {
                    let count = config.servers.len();
                    if count > 0 {
                        sources.push(format!("{} from Claude Code", count));
                        imported.servers.extend(config.servers);
                    }
                }
            }

            // Import from Codex CLI (~/.codex/config.toml)
            let codex_config = home.join(".codex/config.toml");
            if codex_config.exists() {
                if let Ok(config) = Self::load_from_codex_toml(&codex_config) {
                    let count = config.servers.len();
                    if count > 0 {
                        sources.push(format!("{} from Codex CLI", count));
                        // Codex overrides Claude for same-named servers
                        imported.servers.extend(config.servers);
                    }
                }
            }
        }

        if !imported.servers.is_empty() {
            if let Err(e) = imported.save_to_file(&jcode_mcp) {
                crate::logging::error(&format!("Failed to save imported MCP config: {}", e));
                return;
            }
            let names: Vec<&str> = imported.servers.keys().map(|s| s.as_str()).collect();
            crate::logging::info(&format!(
                "MCP: Imported {} servers ({}) from {}",
                imported.servers.len(),
                names.join(", "),
                sources.join(" + "),
            ));
        }
    }

    /// Parse MCP servers from Codex CLI's config.toml ([mcp_servers.*] sections)
    fn load_from_codex_toml(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let table: toml::Table = content.parse()?;

        let mut config = Self::default();
        if let Some(toml::Value::Table(mcp_servers)) = table.get("mcp_servers") {
            for (name, value) in mcp_servers {
                if let toml::Value::Table(server) = value {
                    let command = server
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if command.is_empty() {
                        continue;
                    }
                    let args = server
                        .get("args")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let env = server
                        .get("env")
                        .and_then(|v| v.as_table())
                        .map(|t| {
                            t.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect()
                        })
                        .unwrap_or_default();
                    let shared = server
                        .get("shared")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    config.servers.insert(
                        name.clone(),
                        McpServerConfig {
                            command,
                            args,
                            env,
                            shared,
                        },
                    );
                }
            }
        }
        Ok(config)
    }

    /// Load from default locations (merges jcode global + local, local overrides)
    pub fn load() -> Self {
        // First-run import from Claude Code / Codex CLI
        Self::import_from_external();

        let mut merged = Self::default();

        // Load jcode's own global config (~/.jcode/mcp.json)
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let jcode_mcp = jcode_dir.join("mcp.json");
            if jcode_mcp.exists() {
                if let Ok(config) = Self::load_from_file(&jcode_mcp) {
                    merged.servers.extend(config.servers);
                }
            }
        }

        // Load project-local jcode config (.jcode/mcp.json)
        let local_jcode = std::path::Path::new(".jcode/mcp.json");
        if local_jcode.exists() {
            if let Ok(config) = Self::load_from_file(local_jcode) {
                merged.servers.extend(config.servers);
            }
        }

        // Fallback: project-local Claude config (.claude/mcp.json) for compatibility
        let local_claude = std::path::Path::new(".claude/mcp.json");
        if local_claude.exists() {
            if let Ok(config) = Self::load_from_file(local_claude) {
                merged.servers.extend(config.servers);
            }
        }

        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let request = JsonRpcRequest::new(1, "tools/list", None);
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"tools/list\""));
    }

    #[test]
    fn test_json_rpc_response_deserialization() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, Some(1));
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn test_json_rpc_error_response() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid Request"}}"#;
        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(response.error.is_some());
        let err = response.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid Request");
    }

    #[test]
    fn test_mcp_config_deserialization() {
        let json = r#"{
            "servers": {
                "test-server": {
                    "command": "/usr/bin/test-mcp",
                    "args": ["--port", "8080"],
                    "env": {"API_KEY": "secret"}
                }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.servers.len(), 1);
        let server = config.servers.get("test-server").unwrap();
        assert_eq!(server.command, "/usr/bin/test-mcp");
        assert_eq!(server.args, vec!["--port", "8080"]);
        assert_eq!(server.env.get("API_KEY"), Some(&"secret".to_string()));
    }

    #[test]
    fn test_mcp_config_empty() {
        let json = r#"{}"#;
        let config: McpConfig = serde_json::from_str(json).unwrap();
        assert!(config.servers.is_empty());
    }

    #[test]
    fn test_tool_def_deserialization() {
        let json = r#"{
            "name": "read_file",
            "description": "Read a file from disk",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }
        }"#;
        let tool: McpToolDef = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description, Some("Read a file from disk".to_string()));
    }

    #[test]
    fn test_tool_call_result_text() {
        let json = r#"{
            "content": [{"type": "text", "text": "File contents here"}],
            "isError": false
        }"#;
        let result: ToolCallResult = serde_json::from_str(json).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "File contents here"),
            _ => panic!("Expected text block"),
        }
    }

    #[test]
    fn test_tool_call_result_error() {
        let json = r#"{
            "content": [{"type": "text", "text": "File not found"}],
            "isError": true
        }"#;
        let result: ToolCallResult = serde_json::from_str(json).unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn test_initialize_result() {
        let json = r#"{
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {"listChanged": true}
            },
            "serverInfo": {
                "name": "test-server",
                "version": "1.0.0"
            }
        }"#;
        let result: InitializeResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.protocol_version, "2024-11-05");
        assert!(result.server_info.is_some());
    }
}
