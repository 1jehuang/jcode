//! MCP Protocol types (JSON-RPC 2.0)

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Command for stdio servers. Empty for HTTP/SSE servers, which jcode does
    /// not yet support (such entries are skipped at load time).
    #[serde(default)]
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
    /// Transport type from Claude Code configs ("stdio", "http", "sse"). Used
    /// only to recognize and skip non-stdio servers; defaults to stdio.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// URL for HTTP/SSE servers (Claude Code compat). Unused by jcode today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl McpServerConfig {
    /// jcode currently only supports stdio (command-based) MCP servers. A config
    /// entry is stdio when it has a command and is not explicitly an http/sse
    /// transport.
    pub fn is_stdio(&self) -> bool {
        if let Some(t) = &self.transport {
            let t = t.to_ascii_lowercase();
            if t == "http" || t == "sse" || t == "streamable-http" {
                return false;
            }
        }
        !self.command.trim().is_empty()
    }
}

fn default_shared() -> bool {
    true
}

/// Full MCP configuration file
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    /// Server map. Accepts the canonical Claude Code key `mcpServers` as well as
    /// jcode's historical `servers` key.
    #[serde(default, alias = "mcpServers")]
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
    #[expect(
        clippy::collapsible_if,
        reason = "Import logic keeps source-specific MCP config handling explicit"
    )]
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

        // Import from Claude Code. The canonical user config is `~/.claude.json`
        // (top-level `mcpServers` + per-project entries); fall back to the older
        // `~/.claude/mcp.json` layout for users who still have it.
        if let Ok(claude_json) = crate::storage::user_home_path(".claude.json") {
            if claude_json.exists() {
                let cwd = std::env::current_dir().ok();
                let config = Self::load_claude_json(&claude_json, cwd.as_deref());
                let count = config.servers.len();
                if count > 0 {
                    sources.push(format!("{} from Claude Code", count));
                    imported.servers.extend(config.servers);
                }
            }
        }
        if let Ok(claude_mcp) = crate::storage::user_home_path(".claude/mcp.json") {
            if claude_mcp.exists() {
                if let Ok(config) = Self::load_from_file(&claude_mcp) {
                    let count = config.servers.len();
                    if count > 0 {
                        sources.push(format!("{} from Claude Code (legacy)", count));
                        imported.servers.extend(config.servers);
                    }
                }
            }
        }

        // Import from Codex CLI (~/.codex/config.toml)
        if let Ok(codex_config) = crate::storage::user_home_path(".codex/config.toml") {
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
                            transport: None,
                            url: None,
                        },
                    );
                }
            }
        }
        Ok(config)
    }

    /// Parse MCP servers from Claude Code's `~/.claude.json`.
    ///
    /// Claude Code stores a global set under the top-level `mcpServers` key, and
    /// per-project sets under `projects.<abs_path>.mcpServers`. We merge the
    /// global set first, then overlay the entry for `cwd` (if any) so a
    /// project-specific server wins for the active directory.
    fn load_claude_json(path: &std::path::Path, cwd: Option<&std::path::Path>) -> Self {
        let mut config = Self::default();
        let Ok(content) = std::fs::read_to_string(path) else {
            return config;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
            return config;
        };

        // Global servers under top-level `mcpServers`.
        if let Some(map) = value.get("mcpServers")
            && let Ok(servers) = serde_json::from_value::<
                std::collections::HashMap<String, McpServerConfig>,
            >(map.clone())
        {
            config.servers.extend(servers);
        }

        // Per-project servers under `projects.<abs_path>.mcpServers`.
        if let (Some(cwd), Some(projects)) =
            (cwd, value.get("projects").and_then(|p| p.as_object()))
        {
            let cwd_str = cwd.to_string_lossy();
            if let Some(project) = projects.get(cwd_str.as_ref())
                && let Some(map) = project.get("mcpServers")
                && let Ok(servers) = serde_json::from_value::<
                    std::collections::HashMap<String, McpServerConfig>,
                >(map.clone())
            {
                config.servers.extend(servers);
            }
        }

        config
    }

    /// Load from default locations (merges jcode global + local, local overrides)
    #[expect(
        clippy::collapsible_if,
        reason = "Import logic keeps source-specific MCP config merge order explicit"
    )]
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

        // Claude Code user/global config (~/.claude.json): top-level mcpServers
        // plus per-project entries for the current working directory.
        if let Ok(claude_json) = crate::storage::user_home_path(".claude.json") {
            if claude_json.exists() {
                let cwd = std::env::current_dir().ok();
                let config = Self::load_claude_json(&claude_json, cwd.as_deref());
                merged.servers.extend(config.servers);
            }
        }

        // Load project-local jcode config (.jcode/mcp.json)
        let local_jcode = std::path::Path::new(".jcode/mcp.json");
        if local_jcode.exists() {
            if let Ok(config) = Self::load_from_file(local_jcode) {
                merged.servers.extend(config.servers);
            }
        }

        // Claude Code project config: `.mcp.json` at the repo root.
        let local_mcp = std::path::Path::new(".mcp.json");
        if local_mcp.exists() {
            if let Ok(config) = Self::load_from_file(local_mcp) {
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

        // jcode only supports stdio servers today. Drop HTTP/SSE entries (common
        // in Claude Code configs) so they don't fail to spawn, but log them so
        // the omission is visible.
        merged.servers.retain(|name, cfg| {
            let keep = cfg.is_stdio();
            if !keep {
                crate::logging::info(&format!(
                    "MCP: Skipping non-stdio server '{}' ({}); HTTP/SSE transports are not yet supported",
                    name,
                    cfg.transport.as_deref().unwrap_or("http")
                ));
            }
            keep
        });

        merged
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
