mod bash;
mod edit;
mod glob;
mod grep;
mod ls;
mod multiedit;
mod patch;
mod read;
mod webfetch;
mod websearch;
mod write;

use crate::message::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

/// A tool that can be executed by the agent
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must match what's sent to the API)
    fn name(&self) -> &str;

    /// Human-readable description
    fn description(&self) -> &str;

    /// JSON Schema for the input parameters
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given input
    async fn execute(&self, input: Value) -> Result<String>;

    /// Convert to API tool definition
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: self.parameters_schema(),
        }
    }
}

/// Registry of available tools
pub struct Registry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
        };

        // File operations
        registry.register(Box::new(read::ReadTool::new()));
        registry.register(Box::new(write::WriteTool::new()));
        registry.register(Box::new(edit::EditTool::new()));
        registry.register(Box::new(multiedit::MultiEditTool::new()));
        registry.register(Box::new(patch::PatchTool::new()));

        // Search and navigation
        registry.register(Box::new(glob::GlobTool::new()));
        registry.register(Box::new(grep::GrepTool::new()));
        registry.register(Box::new(ls::LsTool::new()));

        // Execution
        registry.register(Box::new(bash::BashTool::new()));

        // Web
        registry.register(Box::new(webfetch::WebFetchTool::new()));
        registry.register(Box::new(websearch::WebSearchTool::new()));

        registry
    }

    fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Get all tool definitions for the API
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.to_definition()).collect()
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, input: Value) -> Result<String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?;

        tool.execute(input).await
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
