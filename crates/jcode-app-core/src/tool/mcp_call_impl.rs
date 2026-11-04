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
