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

/// JSON-RPC notification (a request without an `id`).
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
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
    /// Headers for HTTP/SSE servers (Claude Code compat). Unused by jcode today,
    /// but retained so environment expansion is ready when those transports are.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub headers: std::collections::HashMap<String, String>,
    /// Whether this server is enabled (default: true). Disabled servers stay
    /// registered in config but are not spawned or connected at load time
    /// until re-enabled (issue #436). opencode-style `"enabled": false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Claude Code style alias: `"disabled": true`. Wins over `enabled` when
    /// both are present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
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

    /// Whether this server should be spawned/connected automatically.
    /// Defaults to true. `"disabled": true` (Claude Code style) wins over
    /// `"enabled"` (opencode style) when both are present. Disabled servers
    /// stay in config and can still be connected on demand by name.
    pub fn is_enabled(&self) -> bool {
        if let Some(disabled) = self.disabled {
            return !disabled;
        }
        self.enabled.unwrap_or(true)
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct UnresolvedEnvironmentVariable {
    server: String,
    variable: String,
}

fn valid_environment_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z' | 'a'..='z'))
        && chars.all(|ch| matches!(ch, '_' | 'A'..='Z' | 'a'..='z' | '0'..='9'))
}

/// Expand Claude Code's documented `${VAR}` and `${VAR:-default}` syntax in a
/// single config string. Unsupported/malformed expressions are preserved.
fn expand_environment_string<F>(
    value: &str,
    lookup: &F,
    unresolved: &mut std::collections::BTreeSet<String>,
) -> String
where
    F: Fn(&str) -> Option<String>,
{
    let mut output = String::with_capacity(value.len());
    let mut remainder = value;

    while let Some(start) = remainder.find("${") {
        output.push_str(&remainder[..start]);
        let expression_start = start + 2;
        let Some(relative_end) = remainder[expression_start..].find('}') else {
            output.push_str(&remainder[start..]);
            return output;
        };
        let end = expression_start + relative_end;
        let expression = &remainder[expression_start..end];
        let (variable, default) = match expression.split_once(":-") {
            Some((variable, default)) => (variable, Some(default)),
            None => (expression, None),
        };
        let literal = &remainder[start..=end];

        if !valid_environment_variable_name(variable) {
            output.push_str(literal);
        } else if let Some(expanded) = lookup(variable) {
            output.push_str(&expanded);
        } else if let Some(default) = default {
            output.push_str(default);
        } else {
            unresolved.insert(variable.to_string());
            output.push_str(literal);
        }

        remainder = &remainder[end + 1..];
    }

    output.push_str(remainder);
    output
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

    /// Expand environment references only after all config sources have been
    /// merged. This avoids warning about shadowed definitions and ensures every
    /// downstream consumer, including the tool-schema cache, sees the exact
    /// values that will be passed to the MCP process.
    fn expand_environment_variables_with<F>(
        &mut self,
        lookup: F,
    ) -> Vec<UnresolvedEnvironmentVariable>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut warnings = Vec::new();

        for (server_name, config) in &mut self.servers {
            let mut unresolved = std::collections::BTreeSet::new();
            config.command = expand_environment_string(&config.command, &lookup, &mut unresolved);
            for arg in &mut config.args {
                *arg = expand_environment_string(arg, &lookup, &mut unresolved);
            }
            for value in config.env.values_mut() {
                *value = expand_environment_string(value, &lookup, &mut unresolved);
            }
            if let Some(url) = &mut config.url {
                *url = expand_environment_string(url, &lookup, &mut unresolved);
            }
            for value in config.headers.values_mut() {
                *value = expand_environment_string(value, &lookup, &mut unresolved);
            }

            warnings.extend(
                unresolved
                    .into_iter()
                    .map(|variable| UnresolvedEnvironmentVariable {
                        server: server_name.clone(),
                        variable,
                    }),
            );
        }

        warnings.sort();
        warnings
    }

    fn expand_environment_variables(&mut self) {
        let warnings =
            self.expand_environment_variables_with(|variable| std::env::var(variable).ok());
        for warning in warnings {
            crate::logging::warn(&format!(
                "MCP: Server '{}' references unset environment variable '{}'; leaving '${{{}}}' unexpanded",
                warning.server, warning.variable, warning.variable
            ));
        }
    }

    /// Import MCP servers from Codex CLI on first run.
    ///
    /// Claude Code configuration is intentionally not imported here. It is a
    /// live source read by `load_for_dir`, so persisting it would make deleted
    /// servers survive in jcode's snapshot and would duplicate inline secrets.
    /// This only runs while ~/.jcode/mcp.json does not exist.
    fn import_from_codex_once() {
        let jcode_mcp = match crate::storage::jcode_dir() {
            Ok(dir) => dir.join("mcp.json"),
            Err(_) => return,
        };

        if jcode_mcp.exists() {
            return; // Not first run
        }

        let Ok(codex_config) = crate::storage::user_home_path(".codex/config.toml") else {
            return;
        };
        if !codex_config.exists() {
            return;
        }
        let Ok(imported) = Self::load_from_codex_toml(&codex_config) else {
            return;
        };
        if imported.servers.is_empty() {
            return;
        }

        let server_count = imported.servers.len();
        let environment_value_count = imported
            .servers
            .values()
            .map(|server| server.env.len())
            .sum();
        if let Err(e) = imported.save_to_file(&jcode_mcp) {
            crate::logging::error(&format!("Failed to save imported MCP config: {}", e));
            return;
        }
        crate::logging::info(&Self::codex_import_log_message(
            server_count,
            environment_value_count,
            &jcode_mcp,
        ));
    }

    fn codex_import_log_message(
        server_count: usize,
        environment_value_count: usize,
        destination: &std::path::Path,
    ) -> String {
        let environment_note = if environment_value_count == 0 {
            "no configured environment values were copied".to_string()
        } else {
            format!(
                "copied {} configured environment value(s), which may contain secrets",
                environment_value_count
            )
        };
        format!(
            "MCP: One-time imported {} server(s) from Codex CLI (~/.codex/config.toml) into {}; {}. Claude Code MCP configuration remains live and was not copied",
            server_count,
            destination.display(),
            environment_note,
        )
    }

    fn live_claude_log_message(server_count: usize, source: &str) -> String {
        format!(
            "MCP: Loaded {} server(s) live from Claude Code ({}); source values were not copied into jcode config",
            server_count, source
        )
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
                            headers: std::collections::HashMap::new(),
                            enabled: None,
                            disabled: None,
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
                Self::merge_servers_preferring_runnable(&mut config.servers, servers);
            }
        }

        config
    }

    /// Load project-local MCP config files from `project_root`, in override
    /// order: `.jcode/mcp.json`, then `.mcp.json` (Claude Code project config),
    /// then `.claude/mcp.json` (legacy compatibility). Later files override
    /// same-named servers from earlier ones.
    fn load_project_locals(project_root: &std::path::Path) -> Self {
        let mut merged = Self::default();
        for relative in [".jcode/mcp.json", ".mcp.json", ".claude/mcp.json"] {
            let path = project_root.join(relative);
            if path.exists()
                && let Ok(config) = Self::load_from_file(&path)
            {
                Self::merge_servers_preferring_runnable(&mut merged.servers, config.servers);
            }
        }
        merged
    }

    /// Load from default locations (merges jcode global + local, local overrides),
    /// resolving project-local config against the process working directory.
    pub fn load() -> Self {
        let cwd = std::env::current_dir().ok();
        Self::load_for_dir(cwd.as_deref())
    }

    /// Load from default locations, resolving project-local config
    /// (`.jcode/mcp.json`, `.mcp.json`, `.claude/mcp.json`, and the per-project
    /// entries in `~/.claude.json`) against `project_dir` instead of the
    /// process working directory when provided.
    ///
    /// Remote/client sessions run inside a long-lived server whose cwd is
    /// unrelated to the session's project, so the session working directory
    /// must be threaded through explicitly (issue #420).
    #[expect(
        clippy::collapsible_if,
        reason = "Import logic keeps source-specific MCP config merge order explicit"
    )]
    pub fn load_for_dir(project_dir: Option<&std::path::Path>) -> Self {
        // Codex CLI is a one-time migration. Claude Code remains a live source.
        Self::import_from_codex_once();

        let mut merged = Self::default();
        let claude_mcp_enabled = std::env::var_os("JCODE_DISABLE_CLAUDE_MCP").is_none();

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
        // plus per-project entries for the project directory.
        if claude_mcp_enabled
            && let Ok(claude_json) = crate::storage::user_home_path(".claude.json")
        {
            if claude_json.exists() {
                let cwd = project_dir.map(std::path::Path::to_path_buf);
                let config = Self::load_claude_json(&claude_json, cwd.as_deref());
                if !config.servers.is_empty() {
                    crate::logging::info(&Self::live_claude_log_message(
                        config.servers.len(),
                        "~/.claude.json",
                    ));
                }
                Self::merge_servers_preferring_runnable(&mut merged.servers, config.servers);
            }
        }

        // Older Claude Code global config is also a live source. Reading it on
        // every load preserves compatibility without copying any inline env
        // values into ~/.jcode/mcp.json.
        if claude_mcp_enabled
            && let Ok(claude_mcp) = crate::storage::user_home_path(".claude/mcp.json")
        {
            if claude_mcp.exists()
                && let Ok(config) = Self::load_from_file(&claude_mcp)
            {
                if !config.servers.is_empty() {
                    crate::logging::info(&Self::live_claude_log_message(
                        config.servers.len(),
                        "~/.claude/mcp.json (legacy)",
                    ));
                }
                Self::merge_servers_preferring_runnable(&mut merged.servers, config.servers);
            }
        }

        // Project-local config files, resolved against the project directory.
        if let Some(project_root) = project_dir {
            Self::merge_servers_preferring_runnable(
                &mut merged.servers,
                Self::load_project_locals(project_root).servers,
            );
        }

        // Claude Code expands environment references after source precedence is
        // resolved. Keep this before transport filtering so future HTTP/SSE
        // support receives already-expanded URLs and headers as well.
        merged.expand_environment_variables();

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

    /// Merge `incoming` over `existing`, except that an entry jcode cannot run
    /// (HTTP/SSE) never displaces a working stdio entry for the same name.
    ///
    /// Without this, a `type: http` entry in `~/.claude.json` would overwrite a
    /// working stdio server from `~/.jcode/mcp.json` and then be dropped by the
    /// non-stdio filter, silently losing the server (issue #653).
    fn merge_servers_preferring_runnable(
        existing: &mut std::collections::HashMap<String, McpServerConfig>,
        incoming: std::collections::HashMap<String, McpServerConfig>,
    ) {
        for (name, cfg) in incoming {
            if let Some(current) = existing.get(&name)
                && current.is_stdio()
                && !cfg.is_stdio()
            {
                crate::logging::info(&format!(
                    "MCP: Keeping existing stdio server '{}'; ignoring {} definition from a lower-precedence config",
                    name,
                    cfg.transport.as_deref().unwrap_or("http")
                ));
                continue;
            }
            existing.insert(name, cfg);
        }
    }
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod protocol_tests;
