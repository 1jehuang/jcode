#![allow(dead_code)]
#![allow(dead_code)]

pub mod ambient;
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
    pub images: Vec<ToolImage>,
}

#[derive(Debug, Clone)]
pub struct ToolImage {
    pub media_type: String,
    pub data: String,
    pub label: Option<String>,
}

impl ToolOutput {
    pub fn new(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            title: None,
            metadata: None,
            images: Vec::new(),
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

    pub fn with_image(mut self, media_type: impl Into<String>, data: impl Into<String>) -> Self {
        self.images.push(ToolImage {
            media_type: media_type.into(),
            data: data.into(),
            label: None,
        });
        self
    }

    pub fn with_labeled_image(
        mut self,
        media_type: impl Into<String>,
        data: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        self.images.push(ToolImage {
            media_type: media_type.into(),
            data: data.into(),
            label: Some(label.into()),
        });
        self
    }
}

/// A request for stdin input from a running command
pub struct StdinInputRequest {
    pub request_id: String,
    pub prompt: String,
    pub is_password: bool,
    pub response_tx: tokio::sync::oneshot::Sender<String>,
}

#[derive(Clone)]
pub struct ToolContext {
    pub session_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub working_dir: Option<PathBuf>,
    pub stdin_request_tx: Option<tokio::sync::mpsc::UnboundedSender<StdinInputRequest>>,
}

impl ToolContext {
    pub fn for_subcall(&self, tool_call_id: String) -> Self {
        Self {
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            tool_call_id,
            working_dir: self.working_dir.clone(),
            stdin_request_tx: self.stdin_request_tx.clone(),
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
    /// Create a lightweight empty registry (no tools, no skill loading).
    /// Used by remote-mode clients that don't execute tools locally.
    pub fn empty() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            skills: Arc::new(RwLock::new(SkillRegistry::default())),
            compaction: Arc::new(RwLock::new(CompactionManager::new())),
        }
    }

    /// Base tools that are stateless and can be shared across sessions.
    /// Created once and cached in a OnceLock, then cloned (cheap Arc bumps) per session.
    fn base_tools(skills: &Arc<RwLock<SkillRegistry>>) -> HashMap<String, Arc<dyn Tool>> {
        use std::sync::OnceLock;
        static BASE: OnceLock<HashMap<String, Arc<dyn Tool>>> = OnceLock::new();
        let base = BASE.get_or_init(|| {
            let mut m = HashMap::new();
            m.insert(
                "read".into(),
                Arc::new(read::ReadTool::new()) as Arc<dyn Tool>,
            );
            m.insert("write".into(), Arc::new(write::WriteTool::new()) as _);
            m.insert("edit".into(), Arc::new(edit::EditTool::new()) as _);
            m.insert(
                "multiedit".into(),
                Arc::new(multiedit::MultiEditTool::new()) as _,
            );
            m.insert("patch".into(), Arc::new(patch::PatchTool::new()) as _);
            m.insert(
                "apply_patch".into(),
                Arc::new(apply_patch::ApplyPatchTool::new()) as _,
            );
            m.insert("glob".into(), Arc::new(glob::GlobTool::new()) as _);
            m.insert("grep".into(), Arc::new(grep::GrepTool::new()) as _);
            m.insert("ls".into(), Arc::new(ls::LsTool::new()) as _);
            m.insert("bash".into(), Arc::new(bash::BashTool::new()) as _);
            m.insert(
                "webfetch".into(),
                Arc::new(webfetch::WebFetchTool::new()) as _,
            );
            m.insert(
                "websearch".into(),
                Arc::new(websearch::WebSearchTool::new()) as _,
            );
            m.insert(
                "codesearch".into(),
                Arc::new(codesearch::CodeSearchTool::new()) as _,
            );
            m.insert("invalid".into(), Arc::new(invalid::InvalidTool::new()) as _);
            m.insert("lsp".into(), Arc::new(lsp::LspTool::new()) as _);
            m.insert(
                "todowrite".into(),
                Arc::new(todo::TodoWriteTool::new()) as _,
            );
            m.insert("todoread".into(), Arc::new(todo::TodoReadTool::new()) as _);
            m.insert("bg".into(), Arc::new(bg::BgTool::new()) as _);
            m.insert(
                "communicate".into(),
                Arc::new(communicate::CommunicateTool::new()) as _,
            );
            m.insert(
                "session_search".into(),
                Arc::new(session_search::SessionSearchTool::new()) as _,
            );
            m.insert(
                "remember".into(),
                Arc::new(remember::RememberTool::new()) as _,
            );
            m.insert("memory".into(), Arc::new(memory::MemoryTool::new()) as _);
            m.insert(
                "schedule".into(),
                Arc::new(ambient::ScheduleTool::new()) as _,
            );
            m
        });
        // Clone the Arc entries (cheap refcount bumps, not deep copies)
        let mut tools = base.clone();
        // SkillTool needs the skills registry reference (shared across sessions)
        tools.insert(
            "skill_manage".into(),
            Arc::new(skill::SkillTool::new(skills.clone())) as _,
        );
        tools
    }

    pub async fn new(provider: Arc<dyn Provider>) -> Self {
        let skills = Arc::new(RwLock::new(SkillRegistry::load().unwrap_or_default()));
        let compaction = Arc::new(RwLock::new(CompactionManager::new()));
        let registry = Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            skills: skills.clone(),
            compaction: compaction.clone(),
        };

        let mut tools_map = Self::base_tools(&skills);

        // Per-session tools that need provider/registry references
        tools_map.insert(
            "subagent".into(),
            Arc::new(task::SubagentTool::new(provider, registry.clone())) as _,
        );
        tools_map.insert(
            "batch".into(),
            Arc::new(batch::BatchTool::new(registry.clone())) as _,
        );
        tools_map.insert(
            "conversation_search".into(),
            Arc::new(conversation_search::ConversationSearchTool::new(compaction)) as _,
        );

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

    /// Resolve tool name aliases.
    ///
    /// When using OAuth, the API presents tools with Claude Code names
    /// (e.g. `file_grep`, `shell_exec`). The model uses those names in
    /// sub-tool calls (e.g. inside `batch`), but our registry uses internal
    /// names (`grep`, `bash`). This mapping ensures both forms resolve
    /// correctly.
    fn resolve_tool_name(name: &str) -> &str {
        match name {
            "task" | "task_runner" => "subagent",
            "shell_exec" => "bash",
            "file_read" => "read",
            "file_write" => "write",
            "file_edit" => "edit",
            "file_glob" => "glob",
            "file_grep" => "grep",
            "todo_read" => "todoread",
            "todo_write" => "todowrite",
            other => other,
        }
    }

    /// Estimate token count for a string (chars / 4, matching compaction heuristic)
    fn estimate_tokens(s: &str) -> usize {
        s.len() / 4
    }

    /// Maximum fraction of context budget a single tool output may consume.
    /// Outputs that would push total context beyond this are truncated.
    const CONTEXT_GUARD_THRESHOLD: f32 = 0.90;

    /// Maximum fraction of context budget a single tool output may occupy.
    /// Even if we have room, a single output shouldn't dominate the context.
    const SINGLE_OUTPUT_MAX_FRACTION: f32 = 0.30;

    /// Execute a tool by name
    pub async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let tools = self.tools.read().await;
        let resolved_name = Self::resolve_tool_name(name);
        let tool = tools
            .get(resolved_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", name))?
            .clone();

        // Drop the lock before executing
        drop(tools);

        let mut output = tool.execute(input, ctx).await?;

        // Context overflow guard: check if this output would push us over the limit
        output = self.guard_context_overflow(name, output).await;

        Ok(output)
    }

    /// Check if a tool output would overflow the context window and truncate if needed.
    /// Returns the (possibly truncated) output.
    async fn guard_context_overflow(&self, tool_name: &str, output: ToolOutput) -> ToolOutput {
        let compaction = self.compaction.read().await;
        let budget = compaction.token_budget();
        if budget == 0 {
            return output;
        }

        let current_tokens = compaction.effective_token_count();
        let output_tokens = Self::estimate_tokens(&output.output);

        // Check 1: Would adding this output push us over the safety threshold?
        let projected = current_tokens + output_tokens;
        let threshold_tokens = (budget as f32 * Self::CONTEXT_GUARD_THRESHOLD) as usize;

        // Check 2: Is this single output unreasonably large relative to budget?
        let single_max_tokens = (budget as f32 * Self::SINGLE_OUTPUT_MAX_FRACTION) as usize;

        let needs_truncation = projected > threshold_tokens || output_tokens > single_max_tokens;

        if !needs_truncation {
            return output;
        }

        // Calculate how many tokens we can afford for this output
        let remaining = if current_tokens < threshold_tokens {
            threshold_tokens - current_tokens
        } else {
            // Already over threshold — allow a small amount for the error message
            budget / 50 // ~2% of budget for the truncation notice
        };
        let max_tokens = remaining.min(single_max_tokens);

        // Convert token limit back to approximate character limit
        let max_chars = max_tokens * 4;

        if output.output.len() <= max_chars {
            return output;
        }

        crate::logging::info(&format!(
            "Context guard: truncating {} output from ~{}k to ~{}k tokens \
             (context: {}k/{}k, {:.0}% used)",
            tool_name,
            output_tokens / 1000,
            max_tokens / 1000,
            current_tokens / 1000,
            budget / 1000,
            (current_tokens as f32 / budget as f32) * 100.0,
        ));

        // Truncate the output, keeping the beginning (usually most relevant)
        let truncated = if max_chars > 200 {
            // Keep beginning of output + truncation notice
            let kept = &output.output[..output.output.floor_char_boundary(max_chars - 150)];
            format!(
                "{}\n\n⚠️ OUTPUT TRUNCATED: This tool output was {:.0}k tokens which would \
                 exceed the context window ({:.0}k/{}k tokens used, {}k budget). \
                 Only the first ~{:.0}k tokens are shown. Use more targeted queries \
                 (e.g., smaller line ranges, specific grep patterns) to get the content \
                 you need without exceeding context limits.",
                kept,
                output_tokens as f32 / 1000.0,
                current_tokens as f32 / 1000.0,
                budget / 1000,
                budget / 1000,
                max_tokens as f32 / 1000.0,
            )
        } else {
            // Context is almost completely full — just return error
            format!(
                "⚠️ CONTEXT LIMIT REACHED: Cannot return this tool output (~{:.0}k tokens) \
                 because the context window is nearly full ({:.0}k/{}k tokens). \
                 Consider using /compact to free up space, or use more targeted queries.",
                output_tokens as f32 / 1000.0,
                current_tokens as f32 / 1000.0,
                budget / 1000,
            )
        };

        ToolOutput {
            output: truncated,
            title: output.title,
            metadata: output.metadata,
            images: output.images,
        }
    }

    /// Register a tool dynamically (for MCP tools, etc.)
    pub async fn register(&self, name: String, tool: Arc<dyn Tool>) {
        let mut tools = self.tools.write().await;
        tools.insert(name, tool);
    }

    /// Register MCP tools (MCP management and server tools)
    /// Connections happen in background to avoid blocking startup.
    /// If `event_tx` is provided, sends an McpStatus event when connections complete.
    /// If `shared_pool` is provided, shared servers reuse processes from the pool.
    pub async fn register_mcp_tools(
        &self,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ServerEvent>>,
        shared_pool: Option<std::sync::Arc<crate::mcp::SharedMcpPool>>,
        session_id: Option<String>,
    ) {
        use crate::mcp::McpManager;
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let mcp_manager = if let Some(pool) = shared_pool {
            let sid = session_id.unwrap_or_else(|| "unknown".to_string());
            Arc::new(RwLock::new(McpManager::with_shared_pool(pool, sid)))
        } else {
            Arc::new(RwLock::new(McpManager::new()))
        };

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

    /// Register ambient-mode tools (only for ambient sessions)
    pub async fn register_ambient_tools(&self) {
        self.register(
            "end_ambient_cycle".to_string(),
            Arc::new(ambient::EndAmbientCycleTool::new()) as Arc<dyn Tool>,
        )
        .await;

        self.register(
            "schedule_ambient".to_string(),
            Arc::new(ambient::ScheduleAmbientTool::new()) as Arc<dyn Tool>,
        )
        .await;

        self.register(
            "request_permission".to_string(),
            Arc::new(ambient::RequestPermissionTool::new()) as Arc<dyn Tool>,
        )
        .await;

        self.register(
            "send_message".to_string(),
            Arc::new(ambient::SendChannelMessageTool::new()) as Arc<dyn Tool>,
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

    #[test]
    fn test_resolve_tool_name_oauth_aliases() {
        assert_eq!(Registry::resolve_tool_name("file_grep"), "grep");
        assert_eq!(Registry::resolve_tool_name("file_read"), "read");
        assert_eq!(Registry::resolve_tool_name("file_write"), "write");
        assert_eq!(Registry::resolve_tool_name("file_edit"), "edit");
        assert_eq!(Registry::resolve_tool_name("file_glob"), "glob");
        assert_eq!(Registry::resolve_tool_name("shell_exec"), "bash");
        assert_eq!(Registry::resolve_tool_name("task_runner"), "subagent");
        assert_eq!(Registry::resolve_tool_name("task"), "subagent");
        assert_eq!(Registry::resolve_tool_name("todo_read"), "todoread");
        assert_eq!(Registry::resolve_tool_name("todo_write"), "todowrite");
        assert_eq!(Registry::resolve_tool_name("bash"), "bash");
        assert_eq!(Registry::resolve_tool_name("grep"), "grep");
        assert_eq!(Registry::resolve_tool_name("batch"), "batch");
        assert_eq!(Registry::resolve_tool_name("memory"), "memory");
    }

    #[tokio::test]
    async fn test_batch_resolves_oauth_names() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let registry = Registry::new(provider).await;
        let temp_dir = std::env::temp_dir();
        let temp_dir_str = temp_dir.to_string_lossy().to_string();

        let ctx = ToolContext {
            session_id: "test".to_string(),
            message_id: "test".to_string(),
            tool_call_id: "test".to_string(),
            working_dir: Some(temp_dir),
            stdin_request_tx: None,
        };

        let result = registry
            .execute(
                "file_grep",
                serde_json::json!({"pattern": "nonexistent_xyz", "path": temp_dir_str}),
                ctx,
            )
            .await;
        assert!(result.is_ok(), "file_grep should resolve to grep tool");
    }

    #[tokio::test]
    async fn test_context_guard_small_output_passes_through() {
        let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(200_000)));
        let registry = Registry {
            tools: Arc::new(RwLock::new(HashMap::new())),
            skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
            compaction,
        };

        let output = ToolOutput::new("small output");
        let result = registry.guard_context_overflow("test", output).await;
        assert_eq!(result.output, "small output");
    }

    #[tokio::test]
    async fn test_context_guard_truncates_huge_single_output() {
        let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(1000)));
        let registry = Registry {
            tools: Arc::new(RwLock::new(HashMap::new())),
            skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
            compaction,
        };

        // 30% of 1000 = 300 tokens = 1200 chars max for a single output
        // Create output that's way larger
        let big_output = "x".repeat(8000); // 2000 tokens, well over 30% of 1000
        let output = ToolOutput::new(big_output.clone());
        let result = registry.guard_context_overflow("test", output).await;
        assert!(
            result.output.len() < big_output.len(),
            "Output should be truncated"
        );
        assert!(
            result.output.contains("TRUNCATED"),
            "Should contain truncation warning"
        );
    }

    #[tokio::test]
    async fn test_context_guard_truncates_when_context_nearly_full() {
        let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(10_000)));
        {
            let mut mgr = compaction.write().await;
            mgr.update_observed_input_tokens(9500); // 95% full
        }
        let registry = Registry {
            tools: Arc::new(RwLock::new(HashMap::new())),
            skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
            compaction,
        };

        // Even a modest output should get truncated when context is 95% full
        let output = ToolOutput::new("x".repeat(4000)); // 1000 tokens
        let result = registry.guard_context_overflow("test", output).await;
        assert!(
            result.output.contains("TRUNCATED") || result.output.contains("CONTEXT LIMIT"),
            "Should warn about context limits when nearly full"
        );
    }

    #[tokio::test]
    async fn test_context_guard_zero_budget_passes_through() {
        let compaction = Arc::new(RwLock::new(CompactionManager::new().with_budget(0)));
        let registry = Registry {
            tools: Arc::new(RwLock::new(HashMap::new())),
            skills: Arc::new(RwLock::new(crate::skill::SkillRegistry::default())),
            compaction,
        };

        let output = ToolOutput::new("x".repeat(100_000));
        let result = registry.guard_context_overflow("test", output).await;
        assert_eq!(
            result.output.len(),
            100_000,
            "Zero budget should pass through"
        );
    }

    #[tokio::test]
    async fn test_request_permission_is_ambient_only() {
        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let registry = Registry::new(provider).await;

        let defs = registry.definitions(None).await;
        assert!(
            !defs.iter().any(|d| d.name == "request_permission"),
            "request_permission should not be available in normal sessions"
        );

        registry.register_ambient_tools().await;
        let defs_after = registry.definitions(None).await;
        assert!(
            defs_after.iter().any(|d| d.name == "request_permission"),
            "request_permission should be available after ambient tool registration"
        );
    }
}
