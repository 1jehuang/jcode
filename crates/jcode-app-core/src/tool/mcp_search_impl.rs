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
