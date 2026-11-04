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

        let mut manager = self.manager.write().await;
        let reload_result = manager.reload().await;

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

        let (successes, failures) = reload_result?;

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
}
