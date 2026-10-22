//! MCP management tool - stdio/remote connections and noninteractive OAuth management

#[cfg(test)]
#[path = "mcp_remote_management_tests.rs"]
mod remote_management_tests;

use crate::mcp::{ContentBlock, McpManager, McpServerConfig, dispatch_name};
use crate::tool::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct McpSearchInput {
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    query: Option<String>,
}

#[derive(Debug, Serialize)]
struct McpSearchResult {
    name: String,
    server: String,
    tool: String,
    description: String,
    input_schema: Value,
}

/// Upper bound on definitions one `mcp_search` call loads into context via
/// tool references, so an empty or broad query cannot pull in a whole catalog.
const MAX_SEARCH_TOOL_REFERENCES: usize = 32;

/// Token budget for one `mcp_search` result. Descriptions and schemas are
/// fitted to it adaptively: a narrow query keeps everything, a broad one
/// trims long descriptions first, then drops schemas for lower matches.
const SEARCH_TOKEN_BUDGET: usize = 6_000;

/// Shortest a description is clipped to when fitting the search budget.
const MIN_SEARCH_DESCRIPTION_CHARS: usize = 120;

/// Clip `text` to `max` chars on a char boundary, marking the cut.
fn clip_description(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// How well a tool matches a search query. Name matches outrank description
/// matches, so a query like "calendar" does not return every tool whose
/// documentation merely mentions a calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchRank {
    Description,
    Name,
}

fn match_rank(
    query_terms: &[String],
    name: &str,
    server: &str,
    tool: &str,
    description: &str,
) -> Option<MatchRank> {
    if query_terms.is_empty() {
        return Some(MatchRank::Name);
    }
    let names = format!("{name} {server} {tool}").to_ascii_lowercase();
    if query_terms.iter().all(|term| names.contains(term.as_str())) {
        return Some(MatchRank::Name);
    }
    let all = format!("{names} {}", description.to_ascii_lowercase());
    query_terms
        .iter()
        .all(|term| all.contains(term.as_str()))
        .then_some(MatchRank::Description)
}

/// Render search matches within `budget` tokens. Tries the loosest settings
/// first: every match in full, then shorter descriptions, then full detail for
/// only the leading matches with the rest listed by name. The first rendering
/// that fits wins, so small results are never trimmed.
fn fit_search_results(matches: &[McpSearchResult], budget: usize) -> Result<(String, usize)> {
    use jcode_core::util::estimate_tokens;
    let render = |detailed: usize, description_chars: Option<usize>| -> Result<String> {
        let head: Vec<McpSearchResult> = matches[..detailed]
            .iter()
            .map(|m| McpSearchResult {
                name: m.name.clone(),
                server: m.server.clone(),
                tool: m.tool.clone(),
                description: description_chars.map_or_else(
                    || m.description.clone(),
                    |max| clip_description(&m.description, max),
                ),
                input_schema: m.input_schema.clone(),
            })
            .collect();
        let mut out = serde_json::to_string(&head)?;
        if detailed < matches.len() {
            let rest: Vec<&str> = matches[detailed..]
                .iter()
                .map(|m| m.name.as_str())
                .collect();
            out.push_str(&format!(
                "\n\n{} more matches (names only; search for one by name for its schema): {}",
                rest.len(),
                rest.join(", ")
            ));
        }
        Ok(out)
    };
    let fits = |out: &String| estimate_tokens(out) <= budget;

    let full = render(matches.len(), None)?;
    if fits(&full) {
        return Ok((full, matches.len()));
    }
    // Shorten descriptions progressively, keeping every schema.
    let longest = matches
        .iter()
        .map(|m| m.description.chars().count())
        .max()
        .unwrap_or(0);
    let mut chars = longest / 2;
    while chars >= MIN_SEARCH_DESCRIPTION_CHARS {
        let out = render(matches.len(), Some(chars))?;
        if fits(&out) {
            return Ok((out, matches.len()));
        }
        chars /= 2;
    }
    // Keep full detail for as many leading matches as fit.
    let (mut lo, mut hi) = (0usize, matches.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&render(mid, Some(MIN_SEARCH_DESCRIPTION_CHARS))?) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Ok((render(lo, Some(MIN_SEARCH_DESCRIPTION_CHARS))?, lo))
}

/// Fixed MCP discovery surface used when individual server definitions are deferred.
pub struct McpSearchTool {
    manager: Arc<RwLock<McpManager>>,
    registry: Option<super::WeakRegistry>,
}

impl McpSearchTool {
    pub fn new(manager: Arc<RwLock<McpManager>>) -> Self {
        Self {
            manager,
            registry: None,
        }
    }

    pub fn with_registry(mut self, registry: crate::tool::Registry) -> Self {
        self.registry = Some(registry.downgrade());
        self
    }
}

#[async_trait]
impl Tool for McpSearchTool {
    fn model_visible(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "mcp_search"
    }

    fn description(&self) -> &str {
        "Search available MCP tools by server, name, or description. Returns callable names and input schemas."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Optional exact MCP server name."
                },
                "query": {
                    "type": "string",
                    "description": "Optional case-insensitive name or description search."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input_flags = input.clone();
        let params: McpSearchInput = serde_json::from_value(input)?;
        let server_filter = params
            .server
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        // Tokenize the optional query directly, avoiding a lowercased copy of
        // the entire query and an artificial empty-query sentinel.
        let query_terms: Vec<String> = params
            .query
            .iter()
            .flat_map(|query| query.split_whitespace().map(str::to_ascii_lowercase))
            .collect();
        let manager = self.manager.read().await;
        let catalog = manager.searchable_tools().await;
        drop(manager);

        let names = crate::mcp::dispatch_names(&catalog);
        let ranked: Vec<(MatchRank, McpSearchResult)> = catalog
            .into_iter()
            .zip(names)
            .filter_map(|((server, tool), name)| {
                if server_filter.is_some_and(|wanted| wanted != server) {
                    return None;
                }
                let legacy_name = dispatch_name(&server, &tool.name);
                let allowed = self
                    .registry
                    .as_ref()
                    .and_then(|r| r.upgrade())
                    .map_or_else(
                        || {
                            super::session_mcp_alias_is_allowed(
                                &ctx.session_id,
                                &name,
                                &legacy_name,
                                "mcp_search",
                            )
                        },
                        |r| {
                            r.mcp_dispatch_is_allowed(
                                &ctx.session_id,
                                &server,
                                &tool.name,
                                &name,
                                "mcp_search",
                            )
                        },
                    );
                if !allowed {
                    return None;
                }
                let description = tool.description.as_deref().unwrap_or_default();
                let rank = match_rank(&query_terms, &name, &server, &tool.name, description)?;
                Some((
                    rank,
                    McpSearchResult {
                        name,
                        server,
                        tool: tool.name,
                        description: tool
                            .description
                            .unwrap_or_else(|| "MCP tool".to_string())
                            .trim()
                            .to_string(),
                        input_schema: tool.input_schema,
                    },
                ))
            })
            .collect();

        // Name matches win. Description-only matches are noise once any tool
        // matches by name, so they are dropped rather than ranked below.
        let best = ranked.iter().map(|(rank, _)| *rank).max();
        let matches: Vec<McpSearchResult> = ranked
            .into_iter()
            .filter(|(rank, _)| Some(*rank) == best)
            .map(|(_, result)| result)
            .collect();
        let total = matches.len();
        let budget = if super::accepts_large_output(&input_flags) {
            usize::MAX
        } else {
            SEARCH_TOKEN_BUDGET
        };
        let (output, detailed) = fit_search_results(&matches, budget)?;

        // Ask the agent to load the matched definitions natively. With
        // provider-native deferred loading these become directly callable
        // tools without changing the cached prompt prefix; other providers
        // ignore the references and use `mcp action=call` with these schemas.
        let references: Vec<&str> = matches[..detailed]
            .iter()
            .take(MAX_SEARCH_TOOL_REFERENCES)
            .map(|m| m.name.as_str())
            .collect();
        Ok(ToolOutput::new(output)
            .with_title(format!("MCP tools ({})", total))
            .with_metadata(json!({ "tool_references": references })))
    }
}

#[derive(Debug, Deserialize)]
struct McpCallInput {
    server: String,
    tool: String,
    #[serde(default)]
    arguments: Value,
}

/// Fixed MCP execution surface used when individual server definitions are deferred.
pub struct McpCallTool {
    manager: Arc<RwLock<McpManager>>,
    registry: Option<super::WeakRegistry>,
}

impl McpCallTool {
    pub fn new(manager: Arc<RwLock<McpManager>>) -> Self {
        Self {
            manager,
            registry: None,
        }
    }

    pub fn with_registry(mut self, registry: crate::tool::Registry) -> Self {
        self.registry = Some(registry.downgrade());
        self
    }
}

#[async_trait]
impl Tool for McpCallTool {
    fn model_visible(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "mcp_call"
    }

    fn description(&self) -> &str {
        "Internal compatibility alias for mcp action=call."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {"type": "string", "description": "MCP server name."},
                "tool": {"type": "string", "description": "Raw MCP tool name."},
                "arguments": {
                    "type": "object",
                    "additionalProperties": true,
                    "description": "Arguments matching the input schema returned by MCP search."
                }
            },
            "required": ["server", "tool", "arguments"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input_flags = input.clone();
        let mut params: McpCallInput = serde_json::from_value(input)?;
        let dispatched_name = dispatch_name(&params.server, &params.tool);
        // Check the current alias too: a per-alias deny must not be bypassed
        // by spelling the original server/tool pair through mcp_call.
        let catalog = self.manager.read().await.searchable_tools().await;
        let names = crate::mcp::dispatch_names(&catalog);
        let alias = catalog
            .iter()
            .zip(&names)
            .find(|((server, tool), _)| server == &params.server && tool.name == params.tool)
            .map(|(_, alias)| alias.as_str())
            .unwrap_or(&dispatched_name);
        let allowed = self
            .registry
            .as_ref()
            .and_then(|r| r.upgrade())
            .map_or_else(
                || {
                    super::session_mcp_alias_is_allowed(
                        &ctx.session_id,
                        alias,
                        &dispatched_name,
                        "mcp_call",
                    )
                },
                |r| {
                    r.mcp_dispatch_is_allowed(
                        &ctx.session_id,
                        &params.server,
                        &params.tool,
                        alias,
                        "mcp_call",
                    )
                },
            );
        if !allowed {
            anyhow::bail!("MCP tool '{}' is not allowed", alias);
        }
        if params.arguments.is_null() {
            params.arguments = Value::Object(serde_json::Map::new());
        }
        // The escape hatch may arrive at either level; never forward it.
        let accept_large_output = super::accepts_large_output(&input_flags)
            | crate::mcp::take_accept_large_output(&mut params.arguments);

        // Deferred dispatch must honor the same session-local replacement as
        // eager and batched dispatch. Never fall through to the real MCP server
        // when the SDK owner has replaced this identity.
        let custom_name = if super::sdk::custom(&ctx.session_id, alias) {
            Some(alias)
        } else if super::sdk::custom(&ctx.session_id, &dispatched_name) {
            Some(dispatched_name.as_str())
        } else {
            None
        };
        if let Some(custom_name) = custom_name {
            let registry = self
                .registry
                .as_ref()
                .and_then(|r| r.upgrade())
                .ok_or_else(|| anyhow::anyhow!("SDK MCP override requires a live registry"))?;
            return registry.execute(custom_name, params.arguments, ctx).await;
        }

        let manager = self.manager.read().await;
        let result = manager
            .call_tool(&params.server, &params.tool, params.arguments)
            .await?;
        drop(manager);

        let mut output_parts = Vec::new();
        for block in result.content {
            match block {
                ContentBlock::Text { text } => output_parts.push(text),
                ContentBlock::Image { data, mime_type } => {
                    output_parts.push(format!("[Image: {} ({} bytes)]", mime_type, data.len()));
                }
                ContentBlock::Resource { resource } => {
                    if let Some(rendered) = crate::applets::mount_mcp_resource(
                        &ctx.session_id,
                        &ctx.tool_call_id,
                        &resource.uri,
                        resource.mime_type.as_deref(),
                        resource.text.as_deref(),
                    ) {
                        output_parts.push(rendered);
                    } else if let Some(text) = resource.text {
                        output_parts.push(text);
                    } else if let Some(blob) = resource.blob {
                        output_parts.push(format!(
                            "[Resource: {} ({} bytes)]",
                            resource.uri,
                            blob.len()
                        ));
                    } else {
                        output_parts.push(format!("[Resource: {}]", resource.uri));
                    }
                }
            }
        }
        let output = output_parts.join("\n");
        let output = if accept_large_output {
            output
        } else {
            crate::mcp::budget::fit_to_budget(&output, crate::mcp::budget::MCP_RESULT_TOKEN_BUDGET)
        };
        let title = format!("mcp:{}:{}", params.server, params.tool);
        if result.is_error {
            Ok(ToolOutput::new(format!("Error: {}", output)).with_title(title))
        } else {
            Ok(ToolOutput::new(output).with_title(title))
        }
    }
}

#[derive(Debug, Deserialize)]
struct McpToolInput {
    action: String,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    transport: Option<String>,
}

fn connection_config(
    params: &McpToolInput,
    configured: Option<McpServerConfig>,
) -> Result<McpServerConfig> {
    if params.command.is_some() && params.url.is_some() {
        anyhow::bail!("Supply either 'command' for stdio or 'url' for remote MCP, not both");
    }
    if params.command.is_none() && params.url.is_none() {
        if params.args.is_some()
            || params.env.is_some()
            || params.headers.is_some()
            || params.transport.is_some()
        {
            anyhow::bail!(
                "Connection overrides require an explicit 'command' or 'url'; omit them to use the configured server"
            );
        }
        return configured.ok_or_else(|| anyhow::anyhow!("'command' or 'url' is required for connect action (server is not in the MCP config)"));
    }
    if params.url.is_some() {
        if params.args.is_some() || params.env.is_some() {
            anyhow::bail!("'args' and 'env' are only supported for stdio MCP servers");
        }
        let transport = params.transport.as_deref().unwrap_or("http");
        if !matches!(transport, "http" | "streamable-http" | "sse") {
            anyhow::bail!("Remote 'transport' must be 'http', 'streamable-http', or 'sse'");
        }
        validate_remote_url(params.url.as_deref().expect("remote URL"))?;
    } else {
        if params
            .command
            .as_deref()
            .is_none_or(|command| command.trim().is_empty())
        {
            anyhow::bail!("'command' must not be empty");
        }
        if params.headers.is_some()
            || params
                .transport
                .as_deref()
                .is_some_and(|transport| transport != "stdio")
        {
            anyhow::bail!("'headers' and remote transports require 'url'");
        }
    }
    Ok(McpServerConfig {
        command: params.command.clone().unwrap_or_default(),
        args: params.args.clone().unwrap_or_default(),
        env: params.env.clone().unwrap_or_default(),
        shared: true,
        transport: params.transport.clone(),
        url: params.url.clone(),
        headers: params.headers.clone().unwrap_or_default(),
        enabled: None,
        disabled: None,
        timeout_secs: None,
    })
}

fn validate_remote_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).map_err(|_| anyhow::anyhow!("Invalid remote MCP URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        anyhow::bail!(
            "Remote MCP URL must be HTTP(S), without embedded user credentials or a fragment"
        );
    }
    Ok(())
}

pub struct McpManagementTool {
    manager: Arc<RwLock<McpManager>>,
    registry: Option<crate::tool::WeakRegistry>,
    /// Optional channel to emit ServerEvent::McpStatus after a toggle so the
    /// remote/client UI refreshes immediately. Set by the server path; the
    /// local TUI path manages its own mcp_server_names from the manager.
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ServerEvent>>,
}

impl McpManagementTool {
    pub fn new(manager: Arc<RwLock<McpManager>>) -> Self {
        Self {
            manager,
            registry: None,
            event_tx: None,
        }
    }

    pub fn with_registry(mut self, registry: crate::tool::Registry) -> Self {
        self.registry = Some(registry.downgrade());
        self
    }

    /// Set the event channel so the tool can emit McpStatus updates after a
    /// toggle. Used by the server/remote path; the local TUI path passes None
    /// and refreshes from the manager directly. Accepts an Option so callers
    /// that already hold one pass it through without an extra match.
    pub fn with_event_tx(
        mut self,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ServerEvent>>,
    ) -> Self {
        self.event_tx = event_tx;
        self
    }

    /// Emit an McpStatus event reflecting the currently connected servers and
    /// their tool counts, so the UI refreshes after a toggle.
    async fn emit_mcp_status(&self) {
        let Some(tx) = &self.event_tx else {
            return;
        };
        let manager = self.manager.read().await;
        let servers = manager.connected_servers().await;
        let all_tools = manager.all_tools().await;
        let status: Vec<String> = servers
            .iter()
            .map(|name| {
                let count = all_tools.iter().filter(|(s, _)| s == name).count();
                format!("{}:{}", name, count)
            })
            .collect();
        let server_count = status.len();
        let servers_status = crate::protocol::ServerEvent::McpStatus { servers: status };
        if tx.send(servers_status).is_err() {
            // Client channel closed: the status refresh is best-effort.
            crate::logging::event_info(
                "mcp_status_send_dropped",
                vec![("servers", server_count.to_string())],
            );
        }
    }
}

#[async_trait]
impl Tool for McpManagementTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        "Search and call MCP (Model Context Protocol) tools, or manage servers. Search returns tool names and input schemas. Interactive OAuth login is CLI-only: `jcode mcp login <configured-server>`."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["search", "call", "list", "connect", "disconnect", "reload", "status", "logout", "enable", "disable"],
                    "description": "Action. 'enable'/'disable' toggles whether a server's tools are injected into the prompt (persists to config)."
                },
                "server": {
                    "type": "string",
                    "description": "Server name. Optional exact filter for search, required for call."
                },
                "query": {"type": "string", "description": "Search: optional name or description keywords."},
                "tool": {"type": "string", "description": "Call: required raw tool name returned by search."},
                "arguments": {"type": "object", "additionalProperties": true, "description": "Call: arguments matching the discovered input schema."},
                "command": {
                    "type": "string",
                    "description": "Stdio server command. Mutually exclusive with url."
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Command args."
                },
                "env": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "description": "Server env."
                },
                "url": {
                    "type": "string",
                    "description": "HTTP(S) MCP endpoint for an ad hoc remote connect, or OAuth status/logout. Omit command/url to connect a configured server by name."
                },
                "headers": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "description": "HTTP headers for an ad hoc remote connect. A static Authorization header overrides stored OAuth credentials."
                },
                "transport": {
                    "type": "string",
                    "enum": ["stdio", "http", "streamable-http", "sse"],
                    "description": "Ad hoc transport. Remote URLs default to Streamable HTTP with legacy SSE fallback."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        // Reuse the internal surfaces, including their identity, SDK override,
        // output-budget and per-tool policy checks. Do not route back through
        // Registry::execute: this call already owns the in-flight marker.
        let surface = match input.get("action").and_then(Value::as_str) {
            Some("search") => Some("mcp_search"),
            Some("call") => Some("mcp_call"),
            _ => None,
        };
        if let Some(surface) = surface {
            super::ensure_mcp_action_allowed(&ctx.session_id, surface)?;
            return if surface == "mcp_search" {
                McpSearchTool {
                    manager: self.manager.clone(),
                    registry: self.registry.clone(),
                }
                .execute(input, ctx)
                .await
            } else {
                McpCallTool {
                    manager: self.manager.clone(),
                    registry: self.registry.clone(),
                }
                .execute(input, ctx)
                .await
            };
        }
        super::ensure_mcp_action_allowed(&ctx.session_id, "mcp")?;
        let params: McpToolInput = serde_json::from_value(input)?;
        let started = std::time::Instant::now();
        let action = params.action.clone();
        let server = params.server.clone().unwrap_or_else(|| "none".to_string());
        crate::logging::event_info(
            "MCP_LIFECYCLE",
            vec![
                ("phase", "management_start".to_string()),
                ("action", action.clone()),
                ("server", server.clone()),
                ("session_id", ctx.session_id.clone()),
                ("tool_call_id", ctx.tool_call_id.clone()),
            ],
        );

        let result = match params.action.as_str() {
            "list" => self.list_servers().await,
            "connect" => self.connect_server(params, &ctx.session_id).await,
            "disconnect" => self.disconnect_server(params).await,
            "reload" => self.reload_config(&ctx.session_id).await,
            "status" | "logout" => self.oauth_action(params).await,
            "login" => anyhow::bail!(
                "Interactive MCP OAuth login is CLI-only. Run `jcode mcp login <configured-server>` in a terminal (add --no-browser to print the authorization URL). No browser was opened."
            ),
            "enable" => self.toggle_server(params, true, &ctx.session_id).await,
            "disable" => self.toggle_server(params, false, &ctx.session_id).await,
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'search', 'call', 'list', 'connect', 'disconnect', 'reload', 'status', 'logout', 'enable', or 'disable'.",
                params.action
            ))),
        };

        match &result {
            Ok(_) => crate::logging::event_info(
                "MCP_LIFECYCLE",
                vec![
                    ("phase", "management_done".to_string()),
                    ("action", action),
                    ("server", server),
                    ("session_id", ctx.session_id),
                    ("tool_call_id", ctx.tool_call_id),
                    ("status", "ok".to_string()),
                    ("elapsed_ms", started.elapsed().as_millis().to_string()),
                ],
            ),
            Err(error) => crate::logging::event_warn(
                "MCP_LIFECYCLE",
                vec![
                    ("phase", "management_done".to_string()),
                    ("action", action),
                    ("server", server),
                    ("session_id", ctx.session_id),
                    ("tool_call_id", ctx.tool_call_id),
                    ("status", "error".to_string()),
                    ("error", error.to_string()),
                    ("elapsed_ms", started.elapsed().as_millis().to_string()),
                ],
            ),
        }

        result
    }
}

impl McpManagementTool {
    async fn oauth_action(&self, params: McpToolInput) -> Result<ToolOutput> {
        let configured =
            if let Some(name) = params.server.as_deref().filter(|_| params.url.is_none()) {
                self.manager
                    .read()
                    .await
                    .config()
                    .servers
                    .get(name)
                    .cloned()
            } else {
                None
            };
        if params.url.is_none()
            && configured
                .as_ref()
                .is_some_and(|server| !server.is_remote())
        {
            anyhow::bail!("OAuth status/logout is only supported for remote MCP servers");
        }
        let url = params
            .url
            .as_deref()
            .or_else(|| configured.as_ref().and_then(|server| server.url.as_deref()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Supply a configured remote 'server' or its 'url' for OAuth status/logout"
                )
            })?;
        validate_remote_url(url)?;
        if params.action == "logout" {
            let removed = crate::mcp::oauth::logout(url)?;
            return Ok(ToolOutput::new(format!("{}. Static headers and existing connections are unchanged. This only forgets local OAuth credentials, not server-side authorization. Disconnect/reconnect active servers to stop using the old token.",
                if removed { "Removed local OAuth credentials" } else { "No local OAuth credentials stored" }))
                .with_title("MCP: OAuth logout"));
        }
        let static_auth = params
            .headers
            .as_ref()
            .or_else(|| configured.as_ref().map(|server| &server.headers))
            .is_some_and(|headers| {
                headers
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case("authorization"))
            });
        let status = if static_auth {
            "Static Authorization header configured (takes precedence over OAuth)"
        } else if crate::mcp::oauth::status(url)?.logged_in {
            "OAuth credentials stored (not a connection or token-validity check)"
        } else {
            "No OAuth credentials stored. To sign in, run `jcode mcp login <configured-server>` in a terminal."
        };
        Ok(ToolOutput::new(status).with_title("MCP: OAuth status"))
    }

    async fn list_servers(&self) -> Result<ToolOutput> {
        let manager = self.manager.read().await;
        let servers = manager.connected_servers().await;
        let all_tools = manager.all_tools().await;
        // Configured-but-not-connected servers, including disabled ones
        // (issue #436), so the full config state is visible.
        let mut configured: Vec<(String, bool)> = manager
            .config()
            .servers
            .iter()
            .filter(|(name, _)| !servers.contains(name))
            .map(|(name, cfg)| (name.clone(), cfg.is_enabled()))
            .collect();
        configured.sort();

        if servers.is_empty() && configured.is_empty() {
            return Ok(ToolOutput::new(
                "No MCP servers connected.\n\n\
                To connect a server, use:\n\
                {\"action\": \"connect\", \"server\": \"name\", \"command\": \"/path/to/server\", \"args\": []}\n\n\
                Or add servers to ~/.jcode/mcp.json or .jcode/mcp.json and use {\"action\": \"reload\"}.\n\
                .claude/mcp.json is also supported for compatibility."
            ).with_title("MCP: No servers"));
        }

        // Names only: full descriptions for every tool of every server cost
        // tens of thousands of tokens. `mcp action=search` returns descriptions and
        // schemas for the tools the agent actually needs.
        let mut output = String::new();
        output.push_str(&format!(
            "Connected MCP servers: {} ({} tools)\n\n",
            servers.len(),
            all_tools
                .iter()
                .filter(|(owner, _)| servers.contains(owner))
                .count()
        ));

        let names = crate::mcp::dispatch_names(&all_tools);
        let registry = self.registry.as_ref().and_then(|r| r.upgrade());
        for server in &servers {
            // Callable aliases, which differ from raw names when servers collide.
            let server_tools: Vec<String> = all_tools
                .iter()
                .zip(&names)
                .filter(|((owner, _), _)| owner == server)
                .map(|((_, tool), fallback)| {
                    registry
                        .as_ref()
                        .and_then(|r| r.mcp_alias(server, &tool.name))
                        .unwrap_or_else(|| fallback.clone())
                })
                .collect();
            output.push_str(&format!(
                "## {} ({} tool{})\n",
                server,
                server_tools.len(),
                if server_tools.len() == 1 { "" } else { "s" }
            ));
            if server_tools.is_empty() {
                output.push_str("(no tools)\n");
            } else {
                output.push_str(&server_tools.join(", "));
                output.push('\n');
            }
            output.push('\n');
        }
        if !servers.is_empty() {
            output.push_str(
                "Use mcp action=search with a server or query for descriptions and input schemas.\n\n",
            );
        }

        if !configured.is_empty() {
            output.push_str("Configured but not connected:\n");
            for (name, enabled) in &configured {
                if *enabled {
                    output.push_str(&format!(
                        "  - {} (enabled; connect with {{\"action\": \"connect\", \"server\": \"{}\"}})\n",
                        name, name
                    ));
                } else {
                    output.push_str(&format!(
                        "  - {} (disabled in config; connect on demand with {{\"action\": \"connect\", \"server\": \"{}\"}})\n",
                        name, name
                    ));
                }
            }
        }

        Ok(ToolOutput::new(output).with_title("MCP: Server list"))
    }

    async fn connect_server(&self, params: McpToolInput, session_id: &str) -> Result<ToolOutput> {
        let server_name = params
            .server
            .clone()
            .ok_or_else(|| anyhow::anyhow!("'server' is required for connect action"))?;

        // Explicit command/URL is ad hoc. Otherwise use the named config,
        // including disabled entries, without changing persistent configuration.
        let configured = self
            .manager
            .read()
            .await
            .config()
            .servers
            .get(&server_name)
            .cloned();
        let config = connection_config(&params, configured)?;

        let manager = self.manager.read().await;

        // Check if already connected
        let connected = manager.connected_servers().await;
        if connected.contains(&server_name) {
            return Ok(ToolOutput::new(format!(
                "Server '{}' is already connected. Use 'disconnect' first to reconnect.",
                server_name
            ))
            .with_title("MCP: Already connected"));
        }
        drop(manager);

        // Connect
        let manager = self.manager.read().await;
        match manager.connect(&server_name, &config).await {
            Ok(()) => {
                let tools = manager.all_tools().await;
                let connected = manager.connected_servers().await;
                drop(manager);
                let registry = self.registry.as_ref().and_then(|r| r.upgrade());
                if let Some(registry) = &registry {
                    registry
                        .refresh_mcp_tools(
                            crate::mcp::create_mcp_tools_from_cached_many(
                                &tools,
                                Arc::clone(&self.manager),
                            ),
                            &connected,
                        )
                        .await;
                }
                let names = crate::mcp::dispatch_names(&tools);
                let server_tools: Vec<_> = tools
                    .iter()
                    .zip(&names)
                    .filter(|((server, _), _)| server == &server_name)
                    .collect();
                let mut output = format!(
                    "Connected to MCP server '{}'\n\nAvailable tools ({}):\n",
                    server_name,
                    server_tools.len()
                );
                let mut references = Vec::new();
                for ((_, tool), fallback) in server_tools {
                    let name = registry
                        .as_ref()
                        .and_then(|r| r.mcp_alias(&server_name, &tool.name))
                        .unwrap_or_else(|| fallback.clone());
                    // The schema is part of the result on purpose: on providers
                    // without native deferred loading the cached tool list never
                    // changes, so this transcript entry is the only place the
                    // model learns how to call the new tool (via `mcp action=call`).
                    output.push_str(&format!(
                        "  - {}: {}\n    tool: {}  input_schema: {}\n",
                        name,
                        tool.description.as_deref().unwrap_or("(no description)"),
                        tool.name,
                        serde_json::to_string(&tool.input_schema)
                            .unwrap_or_else(|_| "{}".to_string()),
                    ));
                    references.push(name);
                }
                references.truncate(MAX_SEARCH_TOOL_REFERENCES);
                output.push_str(&format!(
                    "\nCall these tools directly by name if they appear in your tool list; \
                     otherwise use mcp action=call with server=\"{}\", tool=<tool>, and arguments \
                     matching input_schema.\n",
                    server_name
                ));

                // The new server's tools load as provider-native deferred
                // definitions (no prompt-cache miss) where supported.
                Ok(ToolOutput::new(output)
                    .with_title(format!("MCP: Connected {}", server_name))
                    .with_metadata(json!({ "tool_references": references })))
            }
            Err(e) => {
                let auth_hint = if config.is_remote()
                    && !config
                        .headers
                        .keys()
                        .any(|key| key.eq_ignore_ascii_case("authorization"))
                {
                    format!(
                        "\nIf authentication is required, configure this server and run `jcode mcp login {server_name}` in a terminal, then reconnect. Login never runs automatically."
                    )
                } else {
                    String::new()
                };
                crate::logging::event_warn(
                    "MCP_LIFECYCLE",
                    vec![
                        ("phase", "connect_failed".to_string()),
                        ("server", server_name.clone()),
                        ("session_id", session_id.to_string()),
                        ("error", e.to_string()),
                    ],
                );
                Ok(ToolOutput::new(format!(
                    "Failed to connect to '{}': {}{}",
                    server_name, e, auth_hint
                ))
                .with_title("MCP: Connection failed"))
            }
        }
    }

    async fn disconnect_server(&self, params: McpToolInput) -> Result<ToolOutput> {
        let server_name = params
            .server
            .ok_or_else(|| anyhow::anyhow!("'server' is required for disconnect action"))?;

        let manager = self.manager.read().await;
        let connected = manager.connected_servers().await;

        if !connected.contains(&server_name) {
            return Ok(ToolOutput::new(format!(
                "Server '{}' is not connected.\n\nConnected servers: {}",
                server_name,
                if connected.is_empty() {
                    "(none)".to_string()
                } else {
                    connected.join(", ")
                }
            ))
            .with_title("MCP: Not connected"));
        }
        drop(manager);

        let manager = self.manager.read().await;
        manager.disconnect(&server_name).await?;
        drop(manager);

        // Unregister tools for this server
        if let Some(registry) = self
            .registry
            .as_ref()
            .and_then(|registry| registry.upgrade())
        {
            let removed = registry.unregister_mcp_server(&server_name).await;
            let connected = self.manager.read().await.connected_servers().await;
            registry
                .refresh_mcp_tools(
                    crate::mcp::create_mcp_tools(Arc::clone(&self.manager)).await,
                    &connected,
                )
                .await;
            crate::logging::event_info(
                "MCP_LIFECYCLE",
                vec![
                    ("phase", "tools_unregistered".to_string()),
                    ("server", server_name.clone()),
                    ("removed_tool_count", removed.len().to_string()),
                ],
            );
        }

        Ok(
            ToolOutput::new(format!("Disconnected from MCP server '{}'", server_name))
                .with_title(format!("MCP: Disconnected {}", server_name)),
        )
    }

    async fn reload_config(&self, session_id: &str) -> Result<ToolOutput> {
        // Load fresh config, resolved against the session's project directory
        // rather than the server process cwd (issue #420).
        let config = self.manager.read().await.load_fresh_config();

        if config.servers.is_empty() {
            // Unregister all existing MCP tools before reporting empty
            if let Some(registry) = self
                .registry
                .as_ref()
                .and_then(|registry| registry.upgrade())
            {
                registry.unregister_prefix("mcp__").await;
            }
            return Ok(ToolOutput::new(
                "No servers found in config.\n\n\
                Add servers to ~/.jcode/mcp.json (global) or .jcode/mcp.json (project):\n\
                {\n  \"servers\": {\n    \"server-name\": {\n      \"command\": \"/path/to/server\",\n      \"args\": [],\n      \"env\": {},\n      \"shared\": true\n    }\n  }\n}\n\n\
                .claude/mcp.json is also supported for compatibility."
            ).with_title("MCP: Empty config"));
        }

        // Unregister all existing MCP server tools before reload
        if let Some(registry) = self
            .registry
            .as_ref()
            .and_then(|registry| registry.upgrade())
        {
            registry.unregister_prefix("mcp__").await;
        }

        let mut manager = self.manager.write().await;
        let (successes, failures) = manager.reload().await?;

        let servers = manager.connected_servers().await;
        let all_tools = manager.all_tools().await;
        drop(manager);

        // Re-register tools from fresh connections
        if let Some(registry) = self
            .registry
            .as_ref()
            .and_then(|registry| registry.upgrade())
        {
            let mcp_tools = crate::mcp::create_mcp_tools(Arc::clone(&self.manager)).await;
            registry.reconcile_mcp_tools(mcp_tools).await;
        }

        let enabled_count = config
            .servers
            .values()
            .filter(|cfg| cfg.is_enabled())
            .count();
        let disabled_count = config.servers.len() - enabled_count;
        let mut output = format!(
            "Reloaded MCP config. Connected: {}/{}\n\n",
            successes, enabled_count
        );
        if disabled_count > 0 {
            output.push_str(&format!(
                "{} server(s) disabled in config (kept, not spawned).\n\n",
                disabled_count
            ));
        }

        // Show failures first
        if !failures.is_empty() {
            crate::logging::event_warn(
                "MCP_LIFECYCLE",
                vec![
                    ("phase", "reload_connect_failures".to_string()),
                    ("session_id", session_id.to_string()),
                    ("failure_count", failures.len().to_string()),
                    (
                        "servers",
                        failures
                            .iter()
                            .map(|(name, _)| name.clone())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                ],
            );
            output.push_str("## Connection Failures\n");
            for (name, error) in &failures {
                output.push_str(&format!("  - {}: {}\n", name, error));
            }
            output.push('\n');
        }

        let names = crate::mcp::dispatch_names(&all_tools);
        for server in &servers {
            output.push_str(&format!("## {}\n", server));
            let server_tools: Vec<_> = all_tools
                .iter()
                .zip(&names)
                .filter(|((owner, _), _)| owner == server)
                .collect();

            for (_, name) in server_tools {
                output.push_str(&format!("  - {}\n", name));
            }
            output.push('\n');
        }

        Ok(ToolOutput::new(output).with_title("MCP: Reloaded"))
    }

    /// Enable or disable an MCP server in config. When enabling, the server is
    /// connected and its tools registered. When disabling, the server is
    /// disconnected and its tools unregistered. The change persists to
    /// ~/.jcode/mcp.json so it survives restarts (opencode-style toggle).
    ///
    /// Persist, flip, reload, and (un)register work live in the shared
    /// `mcp_toggle` service (jcode-base) so the tool and the TUI command
    /// behave identically. A failed config save stops the toggle on both
    /// paths: nothing is (dis)connected or (un)registered, and the tool
    /// reports the failure instead of a toggle that does not hold.
    async fn toggle_server(
        &self,
        params: McpToolInput,
        enable: bool,
        session_id: &str,
    ) -> Result<ToolOutput> {
        let server_name = params
            .server
            .ok_or_else(|| anyhow::anyhow!("'server' is required for enable/disable action"))?;

        // Load current config, flip the enabled flag, persist to ~/.jcode/mcp.json.
        let config = self.manager.read().await.load_fresh_config();
        let (server_config, persistence) =
            crate::mcp::mcp_toggle::persist_toggle(&config, &server_name, enable)?;

        // A failed save means the toggle did not happen. Nothing below may
        // mutate manager state: reloading the in-memory config from the
        // unchanged file would keep the server enabled, `searchable_tools`
        // would re-advertise its cached tools, and `mcp_call` could reconnect
        // it, while the tool claimed the toggle applied. Stop here and report
        // the failure, like the TUI path already does. (Greptile 4307539754)
        if !persistence.save_ok {
            let reason = persistence
                .save_error
                .clone()
                .unwrap_or_else(|| "unknown error".to_string());
            let action = if enable { "enable" } else { "disable" };
            return Ok(ToolOutput::new(format!(
                "Failed to {action} MCP server '{server_name}': config not saved ({reason}). \
                 The server's enabled state is unchanged; fix the write error and retry.",
            ))
            .with_title(format!(
                "MCP: {} {} failed (config not saved)",
                server_name, action
            )));
        }

        // Refresh the manager's in-memory config so subsequent operations
        // (connect, list, connect_all) see the new enabled/disabled state
        // instead of the stale cached copy from process start.
        self.manager.write().await.reload_config();

        let registry = self
            .registry
            .as_ref()
            .and_then(|registry| registry.upgrade());

        if enable {
            // Connect the server and register its tools
            let manager = self.manager.read().await;
            let connected = manager.connected_servers().await;
            if connected.contains(&server_name) {
                drop(manager);
                self.emit_mcp_status().await;
                return Ok(ToolOutput::new(format!(
                    "Server '{}' is already connected and enabled. {}",
                    server_name,
                    persistence.describe()
                ))
                .with_title(format!("MCP: {} already enabled", server_name)));
            }
            drop(manager);

            let (tool_count, connect_error) = crate::mcp::mcp_toggle::connect_and_register(
                &self.manager,
                &server_name,
                &server_config,
                registry.as_ref().map(|r| r as _),
            )
            .await;

            if let Some(error) = connect_error {
                self.emit_mcp_status().await;
                return Ok(ToolOutput::new(format!(
                    "Server '{}' enabled in config but failed to connect: {}.\nUse 'reload' to retry. {}",
                    server_name,
                    error,
                    persistence.describe()
                ))
                .with_title(format!("MCP: {} enabled (connect failed)", server_name)));
            }

            crate::logging::event_info(
                "MCP_LIFECYCLE",
                vec![
                    ("phase", "server_enabled".to_string()),
                    ("server", server_name.clone()),
                    ("session_id", session_id.to_string()),
                    ("tool_count", tool_count.to_string()),
                ],
            );

            self.emit_mcp_status().await;

            Ok(ToolOutput::new(format!(
                "Enabled MCP server '{}'. Connected with {} tool(s). {}",
                server_name,
                tool_count,
                persistence.describe()
            ))
            .with_title(format!("MCP: Enabled {}", server_name)))
        } else {
            // Disconnect the server and unregister its tools
            crate::mcp::mcp_toggle::disconnect_and_unregister(
                &self.manager,
                &server_name,
                registry.as_ref().map(|r| r as _),
            )
            .await;

            crate::logging::event_info(
                "MCP_LIFECYCLE",
                vec![
                    ("phase", "server_disabled".to_string()),
                    ("server", server_name.clone()),
                    ("session_id", session_id.to_string()),
                ],
            );

            self.emit_mcp_status().await;

            Ok(ToolOutput::new(format!(
                "Disabled MCP server '{}'. Tools removed from prompt. {}",
                server_name,
                persistence.describe()
            ))
            .with_title(format!("MCP: Disabled {}", server_name)))
        }
    }
}

// Helper for tests to update cached server names
impl McpManagementTool {
    pub fn manager(&self) -> &Arc<RwLock<McpManager>> {
        &self.manager
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
