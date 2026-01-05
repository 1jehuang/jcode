mod bash;
mod apply_patch;
mod batch;
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
use std::sync::Arc;
use tokio::sync::RwLock;

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

/// Registry of available tools (Arc-wrapped for sharing)
#[derive(Clone)]
pub struct Registry {
    tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
}

impl Registry {
    pub async fn new() -> Self {
        let registry = Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
        };

        let mut tools_map = HashMap::new();

        // File operations
        tools_map.insert("read".to_string(), Arc::new(read::ReadTool::new()) as Arc<dyn Tool>);
        tools_map.insert("write".to_string(), Arc::new(write::WriteTool::new()) as Arc<dyn Tool>);
        tools_map.insert("edit".to_string(), Arc::new(edit::EditTool::new()) as Arc<dyn Tool>);
        tools_map.insert("multiedit".to_string(), Arc::new(multiedit::MultiEditTool::new()) as Arc<dyn Tool>);
        tools_map.insert("patch".to_string(), Arc::new(patch::PatchTool::new()) as Arc<dyn Tool>);
        tools_map.insert(
            "apply_patch".to_string(),
            Arc::new(apply_patch::ApplyPatchTool::new()) as Arc<dyn Tool>,
        );

        // Search and navigation
        tools_map.insert("glob".to_string(), Arc::new(glob::GlobTool::new()) as Arc<dyn Tool>);
        tools_map.insert("grep".to_string(), Arc::new(grep::GrepTool::new()) as Arc<dyn Tool>);
        tools_map.insert("ls".to_string(), Arc::new(ls::LsTool::new()) as Arc<dyn Tool>);

        // Execution
        tools_map.insert("bash".to_string(), Arc::new(bash::BashTool::new()) as Arc<dyn Tool>);

        // Web
        tools_map.insert("webfetch".to_string(), Arc::new(webfetch::WebFetchTool::new()) as Arc<dyn Tool>);
        tools_map.insert("websearch".to_string(), Arc::new(websearch::WebSearchTool::new()) as Arc<dyn Tool>);

        // Add batch with a reference to the registry
        let batch_tool = batch::BatchTool::new(registry.clone());
        tools_map.insert("batch".to_string(), Arc::new(batch_tool) as Arc<dyn Tool>);

        // Populate the registry
        *registry.tools.write().await = tools_map;

        registry
    }

    /// Get all tool definitions for the API
    pub async fn definitions(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.read().await;
        tools.values().map(|t| t.to_definition()).collect()
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, input: Value) -> Result<String> {
        let tools = self.tools.read().await;
        let tool = tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?
            .clone();

        // Drop the lock before executing
        drop(tools);

        tool.execute(input).await
    }
}
