use super::loader::SkillLoader;
use super::registry::SkillRegistry;

/// Bridge between MCP plugins and the skills system.
/// Provides functions to register MCP tools as skills.
pub struct McpSkillsBridge;

impl McpSkillsBridge {
    /// Register MCP tools from a plugin as skills in the registry
    pub async fn register_mcp_tools(
        registry: &SkillRegistry,
        plugin_name: &str,
        tools: Vec<(String, String)>,
    ) -> Vec<String> {
        let loader = SkillLoader::new();
        let mut registered = vec![];

        for (tool_name, _) in &tools {
            let skill_name = format!("mcp-{}-{}", plugin_name, tool_name);
            registered.push(skill_name);
        }

        loader.register_from_mcp(registry, plugin_name, tools).await;
        registered
    }
}