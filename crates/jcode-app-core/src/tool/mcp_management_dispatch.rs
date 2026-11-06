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
                    "enum": ["search", "call", "list", "connect", "disconnect", "reload", "status", "logout"],
                    "description": "Action."
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
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'search', 'call', 'list', 'connect', 'disconnect', 'reload', 'status', or 'logout'.",
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

// Helper for tests to update cached server names
impl McpManagementTool {
    pub fn manager(&self) -> &Arc<RwLock<McpManager>> {
        &self.manager
    }
}
