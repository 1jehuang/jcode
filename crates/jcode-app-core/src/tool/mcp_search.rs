//! Deferred MCP discovery with keyword search and exact canonical selection.
use super::*;
use std::collections::HashSet;

#[async_trait]
impl Tool for McpSearchTool {
    fn model_visible(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "mcp_search"
    }

    fn description(&self) -> &str {
        "Find MCP tools, or load exact advertised names with query select:a,b. Returns schemas."
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
                    "description": "Optional keywords, or select:<name>[,<name>...] for exact advertised tool names."
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
        let selected: Option<HashSet<&str>> = params
            .query
            .as_deref()
            .and_then(|query| query.trim().strip_prefix("select:"))
            .map(|names| {
                names
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .collect()
            });
        // Tokenize the optional query directly, avoiding a lowercased copy of
        // the entire query and an artificial empty-query sentinel.
        let query_terms: Vec<String> = params
            .query
            .iter()
            .filter(|_| selected.is_none())
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
                            super::super::session_mcp_alias_is_allowed(
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
                let description = tool.description.as_deref().unwrap_or("");
                let rank = match &selected {
                    Some(selected) => selected
                        .contains(name.as_str())
                        .then_some(MatchRank::Name)?,
                    None => match_rank(&query_terms, &name, &server, &tool.name, description)?,
                };
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
                        server_instructions: None,
                    },
                ))
            })
            .collect();

        // Name matches win. Description-only matches are noise once any tool
        // matches by name, so they are dropped rather than ranked below.
        let best = ranked.iter().map(|(rank, _)| *rank).max();
        let mut matches: Vec<McpSearchResult> = ranked
            .into_iter()
            .filter(|(rank, _)| Some(*rank) == best)
            .map(|(_, result)| result)
            .collect();
        if !matches.is_empty() {
            let manager = self.manager.read().await;
            let mut servers = HashSet::new();
            for tool in &mut matches {
                if servers.insert(tool.server.clone()) {
                    tool.server_instructions = manager.server_instructions(&tool.server).await;
                }
            }
        }
        let total = matches.len();
        let budget = if super::super::accepts_large_output(&input_flags) {
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
