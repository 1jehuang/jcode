//! MCP Tool - wraps MCP server tools for jcode's tool system

use super::manager::McpManager;
use super::protocol::{ContentBlock, McpToolDef};
use anyhow::Result;
use async_trait::async_trait;
use jcode_tool_core::{Tool, ToolContext};
use jcode_tool_types::ToolOutput;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A tool that proxies to an MCP server
pub struct McpTool {
    server_name: String,
    tool_def: McpToolDef,
    manager: Arc<RwLock<McpManager>>,
}

impl McpTool {
    pub fn new(
        server_name: String,
        tool_def: McpToolDef,
        manager: Arc<RwLock<McpManager>>,
    ) -> Self {
        Self {
            server_name,
            tool_def,
            manager,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        // This will be overridden in registration with prefixed name
        &self.tool_def.name
    }

    fn mcp_identity(&self) -> Option<(&str, &str)> {
        Some((&self.server_name, &self.tool_def.name))
    }

    fn description(&self) -> &str {
        self.tool_def.description.as_deref().unwrap_or("MCP tool")
    }

    fn parameters_schema(&self) -> Value {
        self.tool_def.input_schema.clone()
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let mut input = if input.is_null() {
            Value::Object(serde_json::Map::new())
        } else {
            input
        };
        // `intent` is a jcode-injected display-only parameter (see
        // ensure_intent_in_schema). Strip it before forwarding unless the
        // MCP server's own schema declares an `intent` property.
        let server_declares_intent = self
            .tool_def
            .input_schema
            .get("properties")
            .and_then(|p| p.as_object())
            .is_some_and(|p| p.contains_key("intent"));
        if !server_declares_intent && let Some(object) = input.as_object_mut() {
            object.remove("intent");
        }
        // The escape hatch is jcode's, not the server's: read it, never forward it.
        let accept_large_output = take_accept_large_output(&mut input);
        let manager = self.manager.read().await;
        let result = manager
            .call_tool(&self.server_name, &self.tool_def.name, input)
            .await?;

        // Convert MCP content blocks to output string
        let mut output_parts = Vec::new();
        for block in result.content {
            match block {
                ContentBlock::Text { text } => {
                    output_parts.push(text);
                }
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
            super::budget::fit_to_budget(&output, super::budget::MCP_RESULT_TOKEN_BUDGET)
        };
        let title = format!("mcp:{}:{}", self.server_name, self.tool_def.name);

        if result.is_error {
            Ok(ToolOutput::new(format!("Error: {}", output)).with_title(title))
        } else {
            Ok(ToolOutput::new(output).with_title(title))
        }
    }
}

/// Remove jcode's `accept_large_output` flag from MCP arguments, returning
/// whether the caller opted in. Accepts `true` or the string `"true"`.
pub fn take_accept_large_output(input: &mut Value) -> bool {
    let Some(object) = input.as_object_mut() else {
        return false;
    };
    match object.remove(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY) {
        Some(Value::Bool(accepted)) => accepted,
        Some(Value::String(raw)) => raw.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Longest tool name every supported provider accepts (OpenAI caps at 64,
/// Anthropic at 128).
const MAX_DISPATCH_NAME_LEN: usize = 64;

/// Model-facing name for an MCP tool.
///
/// Providers require tool names to match `^[a-zA-Z0-9_-]{1,64}$` (OpenAI) or
/// `{1,128}` (Anthropic). MCP servers are free to use dots, slashes, spaces,
/// or other characters (e.g. YC's `hiring.create_job`), so every character
/// outside `[A-Za-z0-9_]` becomes `_`. Hyphens are normalized too, preserving
/// the historical spelling. Over-long names are truncated with a stable hash
/// suffix so they stay unique.
pub fn dispatch_name(server_name: &str, tool_name: &str) -> String {
    let raw = format!("mcp__{}__{}", server_name, tool_name);
    let mut name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.len() > MAX_DISPATCH_NAME_LEN {
        let suffix = format!("_{:08x}", stable_dispatch_hash(server_name, tool_name));
        name.truncate(MAX_DISPATCH_NAME_LEN - suffix.len());
        name.push_str(&suffix);
    }
    name
}

/// Build deterministic registry keys for a complete MCP tool surface.
///
/// `dispatch_name` predates multi-server tool registration and intentionally
/// normalizes hyphens for model compatibility. That normalization is lossy,
/// so two distinct `(server, tool)` pairs can otherwise overwrite one another
/// in the registry. Keep the historical spelling when it is unique, and add a
/// stable suffix only to colliding entries.
pub fn dispatch_names(tools: &[(String, McpToolDef)]) -> Vec<String> {
    let bases: Vec<String> = tools
        .iter()
        .map(|(server, tool)| dispatch_name(server, &tool.name))
        .collect();
    let mut counts = std::collections::HashMap::<&str, usize>::new();
    for base in &bases {
        *counts.entry(base).or_default() += 1;
    }

    let mut ordered_indices: Vec<usize> = (0..tools.len()).collect();
    ordered_indices.sort_by(|&left, &right| {
        tools[left]
            .0
            .cmp(&tools[right].0)
            .then_with(|| tools[left].1.name.cmp(&tools[right].1.name))
    });

    let mut names = vec![String::new(); tools.len()];
    let mut used = std::collections::HashSet::with_capacity(tools.len());
    for index in ordered_indices {
        let (server, tool) = &tools[index];
        let base = &bases[index];
        if counts[base.as_str()] == 1 && used.insert(base.clone()) {
            names[index] = base.clone();
            continue;
        }

        let suffix = format!("__{:08x}", stable_dispatch_hash(server, &tool.name));
        let fit = |extra: &str| {
            let mut head = base.clone();
            head.truncate(MAX_DISPATCH_NAME_LEN.saturating_sub(extra.len()));
            format!("{head}{extra}")
        };
        let mut candidate = fit(&suffix);
        let mut counter = 2u32;
        while !used.insert(candidate.clone()) {
            candidate = fit(&format!("{suffix}_{counter}"));
            counter = counter.saturating_add(1);
        }
        names[index] = candidate;
    }
    names
}

fn stable_dispatch_hash(server_name: &str, tool_name: &str) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in server_name
        .as_bytes()
        .iter()
        .chain(std::iter::once(&0))
        .chain(tool_name.as_bytes())
    {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Create tools from an MCP manager
pub async fn create_mcp_tools(manager: Arc<RwLock<McpManager>>) -> Vec<(String, Arc<dyn Tool>)> {
    let mgr = manager.read().await;
    let all_tools = mgr.all_tools().await;
    drop(mgr);

    let names = dispatch_names(&all_tools);
    let mut tools = Vec::new();
    for ((server_name, tool_def), prefixed_name) in all_tools.into_iter().zip(names) {
        let mcp_tool = McpTool::new(server_name, tool_def, Arc::clone(&manager));
        tools.push((prefixed_name, Arc::new(mcp_tool) as Arc<dyn Tool>));
    }
    tools
}

/// Build proxy tools for a single server from cached schemas, without requiring
/// a live connection. Used to advertise a server's tools immediately at spawn
/// (the proxy connects on first call). The returned tools are functionally
/// identical to live ones; only their definitions come from the disk cache.
pub fn create_mcp_tools_from_cached(
    server_name: &str,
    tool_defs: &[McpToolDef],
    manager: Arc<RwLock<McpManager>>,
) -> Vec<(String, Arc<dyn Tool>)> {
    let all_tools: Vec<(String, McpToolDef)> = tool_defs
        .iter()
        .cloned()
        .map(|tool_def| (server_name.to_string(), tool_def))
        .collect();
    create_mcp_tools_from_cached_many(&all_tools, manager)
}

/// Build proxy tools from cached schemas across all configured servers so the
/// same collision handling is applied before registry insertion.
pub fn create_mcp_tools_from_cached_many(
    all_tools: &[(String, McpToolDef)],
    manager: Arc<RwLock<McpManager>>,
) -> Vec<(String, Arc<dyn Tool>)> {
    let names = dispatch_names(all_tools);
    all_tools
        .iter()
        .zip(names)
        .map(|((server_name, tool_def), prefixed_name)| {
            let mcp_tool = McpTool::new(
                server_name.to_string(),
                tool_def.clone(),
                Arc::clone(&manager),
            );
            (prefixed_name, Arc::new(mcp_tool) as Arc<dyn Tool>)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{dispatch_name, dispatch_names};
    use crate::mcp::protocol::McpToolDef;
    use serde_json::json;

    #[test]
    fn hyphenated_mcp_names_are_safe_for_the_standard_dispatcher() {
        assert_eq!(
            dispatch_name("context7", "resolve-library-id"),
            "mcp__context7__resolve_library_id"
        );
        assert_eq!(
            dispatch_name("hyphenated-server", "query-docs"),
            "mcp__hyphenated_server__query_docs"
        );
    }

    /// The strictest tool-name rule across providers: OpenAI/Gemini/OpenRouter
    /// accept `^[a-zA-Z0-9_-]{1,64}$`, Anthropic the same set up to 128.
    fn is_provider_safe(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    fn def(name: &str) -> McpToolDef {
        McpToolDef {
            name: name.to_string(),
            description: None,
            input_schema: json!({"type": "object"}),
        }
    }

    #[test]
    fn edge_case_mcp_names_are_provider_safe() {
        assert_eq!(
            dispatch_name("yc", "hiring.create_job"),
            "mcp__yc__hiring_create_job"
        );
        let long = "x".repeat(200);
        let cases: Vec<(&str, &str)> = vec![
            ("yc", "hiring.create_job"),
            ("yc", "company_documents"),
            ("my server", "a/b:c"),
            ("srv", "tool with spaces"),
            ("srv", "ünïcode.tööl"),
            ("srv", "emoji🚀tool"),
            ("srv", "dots...and--dashes"),
            ("srv", "$pecial@chars!#%"),
            ("srv", ""),
            ("", "tool"),
            ("server.with.dots", "t"),
            ("srv", &long),
            (&long, "tool"),
        ];
        for (server, tool) in cases {
            let name = dispatch_name(server, tool);
            assert!(is_provider_safe(&name), "{server:?}/{tool:?} -> {name:?}");
            assert_eq!(name, dispatch_name(server, tool), "stable");
        }
    }

    #[test]
    fn truncated_names_stay_distinct() {
        let a = format!("{}a", "x".repeat(100));
        let b = format!("{}b", "x".repeat(100));
        assert_ne!(dispatch_name("s", &a), dispatch_name("s", &b));
        let exact = "x".repeat(64 - "mcp__s__".len());
        assert_eq!(dispatch_name("s", &exact).len(), 64);
        assert!(
            dispatch_name("s", &exact).ends_with('x'),
            "no hash when it fits"
        );
    }

    #[test]
    fn sanitization_collisions_get_unique_safe_aliases() {
        let long = "y".repeat(120);
        let tools = vec![
            ("yc".to_string(), def("hiring.status")),
            ("yc".to_string(), def("hiring_status")),
            ("yc".to_string(), def("hiring/status")),
            ("yc".to_string(), def("hiring-status")),
            ("yc".to_string(), def(&long)),
            ("yc".to_string(), def(&long)),
        ];
        let names = dispatch_names(&tools);
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        for name in &names {
            assert!(is_provider_safe(name), "{name:?}");
        }
    }

    #[test]
    fn colliding_dispatch_names_are_unique_and_stable() {
        let tools = vec![
            (
                "server-a".to_string(),
                McpToolDef {
                    name: "query-docs".to_string(),
                    description: None,
                    input_schema: json!({"type": "object"}),
                },
            ),
            (
                "server_a".to_string(),
                McpToolDef {
                    name: "query_docs".to_string(),
                    description: None,
                    input_schema: json!({"type": "object"}),
                },
            ),
        ];
        let first = dispatch_names(&tools);
        let second = dispatch_names(&tools);

        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        assert_ne!(first[0], first[1]);
        assert!(first.iter().all(|name| name.starts_with("mcp__")));
    }
}
