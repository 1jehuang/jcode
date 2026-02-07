#![allow(dead_code)]
#![allow(dead_code)]

mod apply_patch;
mod bash;
mod batch;
mod bg;
mod codesearch;
mod communicate;
mod conversation_search;
mod debug_socket;
mod edit;
mod glob;
mod grep;
mod invalid;
mod ls;
mod lsp;
pub mod mcp;
mod memory;
mod multiedit;
mod patch;
mod read;
mod remember;
pub mod selfdev;
mod session_search;
mod skill;
mod task;
mod todo;
mod webfetch;
mod websearch;
mod write;

use crate::compaction::CompactionManager;
use crate::message::ToolDefinition;
use crate::provider::Provider;
use crate::skill::SkillRegistry;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub output: String,
    pub title: Option<String>,
    pub metadata: Option<Value>,
}

impl ToolOutput {
    pub fn new(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            title: None,
            metadata: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

#[derive(Clone)]
pub struct ToolContext {
    pub session_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub working_dir: Option<PathBuf>,
}

impl ToolContext {
    pub fn for_subcall(&self, tool_call_id: String) -> Self {
        Self {
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            tool_call_id,
            working_dir: self.working_dir.clone(),
        }
    }

    pub fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(ref base) = self.working_dir {
            base.join(path)
        } else {
            path.to_path_buf()
        }
    }
}

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
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput>;

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
///
/// Clone creates a fresh CompactionManager so each subagent gets independent
/// message history tracking. Tools and skills are shared via Arc.
pub struct Registry {
    tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
    skills: Arc<RwLock<SkillRegistry>>,
    compaction: Arc<RwLock<CompactionManager>>,
}

impl Clone for Registry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            skills: self.skills.clone(),
            // Each clone gets a fresh CompactionManager to prevent parallel
            // subagents from corrupting each other's message history
            compaction: Arc::new(RwLock::new(CompactionManager::new())),
        }
    }
}

impl Registry {
    pub async fn new(provider: Arc<dyn Provider>) -> Self {
        let skills = Arc::new(RwLock::new(SkillRegistry::load().unwrap_or_default()));
        let compaction = Arc::new(RwLock::new(CompactionManager::new()));
        let registry = Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            skills: skills.clone(),
            compaction: compaction.clone(),
        };

        let mut tools_map = HashMap::new();

        // File operations
        tools_map.insert(
            "read".to_string(),
            Arc::new(read::ReadTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "write".to_string(),
            Arc::new(write::WriteTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "edit".to_string(),
            Arc::new(edit::EditTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "multiedit".to_string(),
            Arc::new(multiedit::MultiEditTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "patch".to_string(),
            Arc::new(patch::PatchTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "apply_patch".to_string(),
            Arc::new(apply_patch::ApplyPatchTool::new()) as Arc<dyn Tool>,
        );

        // Search and navigation
        tools_map.insert(
            "glob".to_string(),
            Arc::new(glob::GlobTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "grep".to_string(),
            Arc::new(grep::GrepTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "ls".to_string(),
            Arc::new(ls::LsTool::new()) as Arc<dyn Tool>,
        );

        // Execution
        tools_map.insert(
            "bash".to_string(),
            Arc::new(bash::BashTool::new()) as Arc<dyn Tool>,
        );

        // Web
        tools_map.insert(
            "webfetch".to_string(),
            Arc::new(webfetch::WebFetchTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "websearch".to_string(),
            Arc::new(websearch::WebSearchTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "codesearch".to_string(),
            Arc::new(codesearch::CodeSearchTool::new()) as Arc<dyn Tool>,
        );

        // Meta tools
        tools_map.insert(
            "invalid".to_string(),
            Arc::new(invalid::InvalidTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "skill_manage".to_string(),
            Arc::new(skill::SkillTool::new(skills)) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "lsp".to_string(),
            Arc::new(lsp::LspTool::new()) as Arc<dyn Tool>,
        );
        let task_tool = task::TaskTool::new(provider, registry.clone());
        tools_map.insert("task".to_string(), Arc::new(task_tool) as Arc<dyn Tool>);
        tools_map.insert(
            "todowrite".to_string(),
            Arc::new(todo::TodoWriteTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "todoread".to_string(),
            Arc::new(todo::TodoReadTool::new()) as Arc<dyn Tool>,
        );
        tools_map.insert(
            "bg".to_string(),
            Arc::new(bg::BgTool::new()) as Arc<dyn Tool>,
        );

        // Add batch with a reference to the registry
        let batch_tool = batch::BatchTool::new(registry.clone());
        tools_map.insert("batch".to_string(), Arc::new(batch_tool) as Arc<dyn Tool>);

        // Conversation search for RAG over compacted history
        let search_tool = conversation_search::ConversationSearchTool::new(compaction);
        tools_map.insert(
            "conversation_search".to_string(),
            Arc::new(search_tool) as Arc<dyn Tool>,
        );

        // Agent communication tool
        tools_map.insert(
            "communicate".to_string(),
            Arc::new(communicate::CommunicateTool::new()) as Arc<dyn Tool>,
        );

        // Cross-session search (RAG over past sessions)
        tools_map.insert(
            "session_search".to_string(),
            Arc::new(session_search::SessionSearchTool::new()) as Arc<dyn Tool>,
        );

        // Simple remember tool for persisting learnings
        tools_map.insert(
            "remember".to_string(),
            Arc::new(remember::RememberTool::new()) as Arc<dyn Tool>,
        );

        // Full memory tool with categories and lifecycle management
        tools_map.insert(
            "memory".to_string(),
            Arc::new(memory::MemoryTool::new()) as Arc<dyn Tool>,
        );

        // Populate the registry
        *registry.tools.write().await = tools_map;

        registry
    }

    /// Get all tool definitions for the API
    pub async fn definitions(
        &self,
        allowed_tools: Option<&HashSet<String>>,
    ) -> Vec<ToolDefinition> {
        let tools = self.tools.read().await;
        let mut defs: Vec<ToolDefinition> = tools
            .iter()
            .filter(|(name, _)| allowed_tools.map(|set| set.contains(*name)).unwrap_or(true))
            .map(|(name, tool)| {
                let mut def = tool.to_definition();
                // Use registry key as the tool name (important for MCP tools where
                // the registry key is "mcp__server__tool" but Tool::name() returns
                // just the raw tool name)
                if def.name != *name {
                    def.name = name.clone();
                }
                def
            })
            .collect();
        // Sort by name for deterministic ordering - critical for prompt cache hits
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    pub async fn tool_names(&self) -> Vec<String> {
        let tools = self.tools.read().await;
        tools.keys().cloned().collect()
    }

    /// Enable test mode for memory tools (isolated storage)
    /// Called when session is marked as debug
    pub async fn enable_memory_test_mode(&self) {
        let mut tools = self.tools.write().await;

        // Replace memory tool with test version
        tools.insert(
            "memory".to_string(),
            Arc::new(memory::MemoryTool::new_test()) as Arc<dyn Tool>,
        );

        // Replace remember tool with test version
        tools.insert(
            "remember".to_string(),
            Arc::new(remember::RememberTool::new_test()) as Arc<dyn Tool>,
        );

        crate::logging::info("Memory test mode enabled - using isolated storage");
    }

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let tools = self.tools.read().await;
        let tool = tools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?
            .clone();

        // Drop the lock before executing
        drop(tools);

        tool.execute(input, ctx).await
    }

    /// Register a tool dynamically (for MCP tools, etc.)
    pub async fn register(&self, name: String, tool: Arc<dyn Tool>) {
        let mut tools = self.tools.write().await;
        tools.insert(name, tool);
    }

    /// Register MCP tools (MCP management and server tools)
    /// Connections happen in background to avoid blocking startup.
    /// If `event_tx` is provided, sends an McpStatus event when connections complete.
    pub async fn register_mcp_tools(
        &self,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ServerEvent>>,
    ) {
        use crate::mcp::McpManager;
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let mcp_manager = Arc::new(RwLock::new(McpManager::new()));

        // Register MCP management tool immediately (with registry for dynamic tool registration)
        let mcp_tool =
            mcp::McpManagementTool::new(Arc::clone(&mcp_manager)).with_registry(self.clone());
        self.register("mcp".to_string(), Arc::new(mcp_tool) as Arc<dyn Tool>)
            .await;

        // Check if we have servers to connect to
        let server_count = {
            let manager = mcp_manager.read().await;
            manager.config().servers.len()
        };

        if server_count > 0 {
            crate::logging::info(&format!("MCP: Found {} server(s) in config", server_count));

            // Send immediate "connecting" status so the TUI shows loading state
            // Server names with count 0 means "connecting..."
            if let Some(ref tx) = event_tx {
                let server_names: Vec<String> = {
                    let manager = mcp_manager.read().await;
                    manager
                        .config()
                        .servers
                        .keys()
                        .map(|name| format!("{}:0", name))
                        .collect()
                };
                let _ = tx.send(crate::protocol::ServerEvent::McpStatus {
                    servers: server_names,
                });
            }

            // Spawn connection and tool registration in background
            let registry = self.clone();
            tokio::spawn(async move {
                let (successes, failures) = {
                    let manager = mcp_manager.write().await;
                    manager.connect_all().await.unwrap_or((0, Vec::new()))
                };

                if successes > 0 {
                    crate::logging::info(&format!("MCP: Connected to {} server(s)", successes));
                }
                if !failures.is_empty() {
                    for (name, error) in &failures {
                        crate::logging::error(&format!("MCP '{}' failed: {}", name, error));
                    }
                }

                // Register MCP server tools and collect server info
                let tools = crate::mcp::create_mcp_tools(Arc::clone(&mcp_manager)).await;
                let mut server_counts: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                for (name, tool) in &tools {
                    if let Some(rest) = name.strip_prefix("mcp__") {
                        if let Some((server, _)) = rest.split_once("__") {
                            *server_counts.entry(server.to_string()).or_default() += 1;
                        }
                    }
                    registry.register(name.clone(), tool.clone()).await;
                }

                // Notify client of MCP status
                if let Some(tx) = event_tx {
                    let servers: Vec<String> = server_counts
                        .into_iter()
                        .map(|(name, count)| format!("{}:{}", name, count))
                        .collect();
                    let _ = tx.send(crate::protocol::ServerEvent::McpStatus { servers });
                }
            });
        }
    }

    /// Register self-dev tools (only for canary/self-dev sessions)
    pub async fn register_selfdev_tools(&self) {
        // Self-dev management tool
        let selfdev_tool = selfdev::SelfDevTool::new();
        self.register(
            "selfdev".to_string(),
            Arc::new(selfdev_tool) as Arc<dyn Tool>,
        )
        .await;

        // Debug socket tool for direct debug socket access
        let debug_socket_tool = debug_socket::DebugSocketTool::new();
        self.register(
            "debug_socket".to_string(),
            Arc::new(debug_socket_tool) as Arc<dyn Tool>,
        )
        .await;
    }

    /// Unregister a tool
    pub async fn unregister(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let mut tools = self.tools.write().await;
        tools.remove(name)
    }

    /// Unregister all tools matching a prefix
    pub async fn unregister_prefix(&self, prefix: &str) -> Vec<String> {
        let mut tools = self.tools.write().await;
        let to_remove: Vec<String> = tools
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        for name in &to_remove {
            tools.remove(name);
        }
        to_remove
    }

    /// Get shared access to the skill registry
    pub fn skills(&self) -> Arc<RwLock<SkillRegistry>> {
        self.skills.clone()
    }

    /// Get shared access to the compaction manager
    pub fn compaction(&self) -> Arc<RwLock<CompactionManager>> {
        self.compaction.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use async_trait::async_trait;

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            unimplemented!("Mock provider")
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MockProvider)
        }
    }

    #[tokio::test]
    async fn test_tool_definitions_are_sorted() {
        // Create registry with mock provider
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let registry = Registry::new(provider).await;

        // Get definitions multiple times and verify they're always in the same order
        let defs1 = registry.definitions(None).await;
        let defs2 = registry.definitions(None).await;

        // Should have the same order
        assert_eq!(defs1.len(), defs2.len());
        for (d1, d2) in defs1.iter().zip(defs2.iter()) {
            assert_eq!(d1.name, d2.name);
        }

        // Verify they're sorted alphabetically
        let names: Vec<&str> = defs1.iter().map(|d| d.name.as_str()).collect();
        let mut sorted_names = names.clone();
        sorted_names.sort();
        assert_eq!(
            names, sorted_names,
            "Tool definitions should be sorted alphabetically"
        );
    }
}
