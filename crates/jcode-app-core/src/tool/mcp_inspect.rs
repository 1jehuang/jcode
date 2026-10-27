//! `inspect` action: structured server state for the TUI `/mcp` dialog. It is
//! deliberately absent from the tool schema; the `mcp` runtime command is the
//! only caller, so the model never pays for these schemas.

use super::*;

impl McpManagementTool {
    pub(super) async fn inspect_servers(&self) -> Result<ToolOutput> {
        let manager = self.manager.read().await;
        let connected = manager.connected_servers().await;
        let all_tools = manager.all_tools().await;
        let errors = manager.last_errors();
        let config = manager.config();
        let fallback_names = crate::mcp::dispatch_names(&all_tools);
        let registry = self.registry.as_ref().and_then(|r| r.upgrade());

        let mut names: std::collections::BTreeSet<&String> = config.servers.keys().collect();
        names.extend(connected.iter());

        let servers: Vec<Value> = names
            .into_iter()
            .map(|name| {
                let cfg = config.servers.get(name);
                let is_connected = connected.contains(name);
                // A project server's on/off is its (fresh) approval state.
                let project_state = manager.project_server_state(name);
                let enabled = match project_state {
                    Some(state) => state == crate::mcp::ProjectServerState::Approved,
                    None => cfg.is_none_or(McpServerConfig::is_enabled),
                };
                let status = if is_connected {
                    "connected"
                } else if project_state == Some(crate::mcp::ProjectServerState::Pending) {
                    "needs approval"
                } else if !enabled {
                    "disabled"
                } else if errors.get(name).is_some_and(|error| needs_login(error)) {
                    "needs authentication"
                } else if errors.contains_key(name) {
                    "failed"
                } else {
                    "not connected"
                };
                let tools: Vec<Value> = all_tools
                    .iter()
                    .zip(&fallback_names)
                    .filter(|((owner, _), _)| owner == name)
                    .map(|((_, tool), fallback)| {
                        let callable = registry
                            .as_ref()
                            .and_then(|r| r.mcp_alias(name, &tool.name))
                            .unwrap_or_else(|| fallback.clone());
                        json!({
                            "name": tool.name,
                            "callable": callable,
                            "description": tool.description,
                            "input_schema": tool.input_schema,
                            "annotations": tool.annotations,
                        })
                    })
                    .collect();
                let source = config
                    .sources
                    .get(name)
                    .map_or("added this session", String::as_str);
                let project = manager.project_dir();
                json!({
                    "name": name,
                    "status": status,
                    "enabled": enabled,
                    "error": errors.get(name),
                    "transport": cfg.map(|c| c.remote_transport().map_or("stdio", |t| t.label())),
                    "target": cfg.map(display_endpoint),
                    "source": source,
                    "scope": scope_of(source),
                    "config_location": config_location(source, project),
                    "auth": cfg.and_then(auth_state),
                    "tools": tools,
                })
            })
            .collect();

        Ok(ToolOutput::new(format!("{} MCP server(s)", servers.len()))
            .with_metadata(json!({ "servers": servers })))
    }
}

/// Claude Code's config scope for a server, from the file it came from.
fn scope_of(source: &str) -> &'static str {
    if source == crate::mcp::LOCAL_CLAUDE_SOURCE || source.starts_with("plugin:local:") {
        "local"
    } else if crate::mcp::is_project_mcp_source(source) {
        "project"
    } else if source.starts_with('~') || source.starts_with("plugin:user:") {
        "user"
    } else {
        "dynamic"
    }
}

/// Absolute path of the config file, the way Claude Code shows it. The
/// group header and the server page both use it.
fn config_location(source: &str, project: Option<&std::path::Path>) -> String {
    if source.starts_with("plugin:") {
        // Plugin provenance already includes the installed absolute source path.
        return source.to_string();
    }
    let home = |relative: &str| {
        crate::storage::user_home_path(relative)
            .map_or_else(|_| source.to_string(), |p| p.display().to_string())
    };
    if source == crate::mcp::LOCAL_CLAUDE_SOURCE {
        let project = project.map_or_else(String::new, |p| p.display().to_string());
        return format!("{} [project: {project}]", home(".claude.json"));
    }
    if let Some(relative) = source.strip_prefix("~/") {
        return home(relative);
    }
    if crate::mcp::is_project_mcp_source(source) {
        return project.map_or_else(
            || source.to_string(),
            |p| p.join(source).display().to_string(),
        );
    }
    "Dynamically configured".to_string()
}

/// Remote servers: `authenticated` with stored OAuth tokens or a static
/// Authorization header, else `not authenticated`. None for stdio.
fn auth_state(cfg: &McpServerConfig) -> Option<&'static str> {
    let url = cfg.url.as_deref().filter(|_| cfg.is_remote())?;
    let static_header = cfg
        .headers
        .keys()
        .any(|key| key.eq_ignore_ascii_case("authorization"));
    let logged_in = crate::mcp::oauth::stored(url).is_some();
    Some(if static_header || logged_in {
        "authenticated"
    } else {
        "not authenticated"
    })
}

/// The connect error jcode's remote client raises on a 401 without tokens.
fn needs_login(error: &str) -> bool {
    error.contains("requires authorization") || error.contains("jcode mcp login")
}

/// Command line for stdio servers, credential-free URL for remote ones.
fn display_endpoint(cfg: &McpServerConfig) -> String {
    if cfg.is_remote() {
        let target = cfg.display_target();
        return target
            .split_once(' ')
            .map_or(target.clone(), |(_, url)| url.to_string());
    }
    std::iter::once(cfg.command.as_str())
        .chain(cfg.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inspect_reports_configured_servers_with_status_and_endpoint() {
        let config: crate::mcp::McpConfig = serde_json::from_value(json!({"servers": {
            "off": {"command": "srv", "args": ["--flag"], "enabled": false},
            "idle": {"url": "https://user:pw@example.com/mcp?token=x"}
        }}))
        .unwrap();
        let mut config = config;
        config.servers.insert(
            "repo".into(),
            serde_json::from_value(json!({"command": "repo-srv", "enabled": false})).unwrap(),
        );
        config.sources.insert("repo".into(), ".mcp.json".into());
        config.pending_approval.insert("repo".into());
        let manager = Arc::new(RwLock::new(McpManager::with_config(config)));
        let output = McpManagementTool::new(manager)
            .inspect_servers()
            .await
            .unwrap();
        let servers = output.metadata.unwrap()["servers"].clone();

        assert_eq!(servers[0]["name"], "idle");
        assert_eq!(servers[0]["status"], "not connected");
        assert_eq!(servers[0]["transport"], "http");
        // Credentials and query strings never reach the dialog.
        assert_eq!(servers[0]["target"], "https://example.com/mcp");
        assert_eq!(servers[1]["name"], "off");
        assert_eq!(servers[1]["status"], "disabled");
        assert_eq!(servers[1]["target"], "srv --flag");
        assert_eq!(servers[1]["source"], "added this session");
        // A project-file server waiting for approval says so, not "disabled".
        assert_eq!(servers[2]["name"], "repo");
        assert_eq!(servers[2]["status"], "needs approval");
        // Grouping and auth facts the `/mcp` dialog needs.
        assert_eq!(servers[0]["scope"], "dynamic");
        assert_eq!(servers[0]["auth"], "not authenticated");
        assert_eq!(servers[1]["auth"], Value::Null, "stdio has no auth row");
        assert_eq!(servers[2]["scope"], "project");
    }

    #[test]
    fn plugin_mcp_scope_and_provenance_location_remain_intact() {
        for scope in ["user", "project", "local"] {
            let source = format!("plugin:{scope}:demo@market [/cache/demo/.mcp.json]");
            assert_eq!(scope_of(&source), scope);
            assert_eq!(
                config_location(&source, Some(std::path::Path::new("/project"))),
                source
            );
        }
    }

    #[test]
    fn scope_and_config_location_follow_claude_code() {
        let project = std::path::Path::new("/work/repo");
        assert_eq!(scope_of("~/.claude.json"), "user");
        assert_eq!(scope_of("~/.jcode/mcp.json"), "user");
        assert_eq!(scope_of(".mcp.json"), "project");
        assert_eq!(scope_of(crate::mcp::LOCAL_CLAUDE_SOURCE), "local");
        assert_eq!(scope_of("added this session"), "dynamic");
        assert_eq!(
            config_location(".mcp.json", Some(project)),
            "/work/repo/.mcp.json"
        );
        assert!(config_location("~/.claude.json", None).ends_with("/.claude.json"));
        let local = config_location(crate::mcp::LOCAL_CLAUDE_SOURCE, Some(project));
        assert!(
            local.ends_with("/.claude.json [project: /work/repo]"),
            "{local}"
        );
        assert_eq!(
            config_location("added this session", None),
            "Dynamically configured"
        );
        assert!(needs_login(
            "MCP server 'x' requires authorization. Run `jcode mcp login x` in a terminal"
        ));
        assert!(!needs_login("connection refused"));
    }
}
