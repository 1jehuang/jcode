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

    fn description(&self) -> &str {
        self.tool_def.description.as_deref().unwrap_or("MCP tool")
    }

    fn parameters_schema(&self) -> Value {
        self.tool_def.input_schema.clone()
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
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
                    if let Some(text) = resource.text {
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
        let title = format!("mcp:{}:{}", self.server_name, self.tool_def.name);

        if result.is_error {
            Ok(ToolOutput::new(format!("Error: {}", output)).with_title(title))
        } else {
            Ok(ToolOutput::new(output).with_title(title))
        }
    }
}

pub fn dispatch_name(server_name: &str, tool_name: &str) -> String {
    format!("mcp__{}__{}", server_name, tool_name).replace('-', "_")
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
        let mut candidate = format!("{base}{suffix}");
        let mut counter = 2u32;
        while !used.insert(candidate.clone()) {
            candidate = format!("{base}{suffix}_{counter}");
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
