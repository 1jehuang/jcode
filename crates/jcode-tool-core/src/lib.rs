mod streaming_executor;
mod sub_agent;
mod tool_discovery;
mod macros;
pub mod permissions;
pub mod result_budget;
pub mod file_history;
pub mod debug_log;
pub mod error_types;
pub mod settings_priority;

use anyhow::Result;
use async_trait::async_trait;
use jcode_agent_runtime::InterruptSignal;
use jcode_message_types::ToolDefinition;
use jcode_tool_types::ToolOutput;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const TOOL_INTENT_DESCRIPTION: &str = concat!(
    "Short natural-language label explaining why this tool call is being made. ",
    "Used for compact UI display only. Optional; do not use this instead of required tool parameters."
);

pub fn intent_schema_property() -> Value {
    serde_json::json!({
        "type": "string",
        "description": TOOL_INTENT_DESCRIPTION,
    })
}

/// A request for stdin input from a running command.
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
    pub graceful_shutdown_signal: Option<InterruptSignal>,
    pub execution_mode: ToolExecutionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    AgentTurn,
    Direct,
}

impl ToolContext {
    /// Resolve a path relative to the working directory.
    #[inline]
    pub fn for_subcall(&self, tool_call_id: String) -> Self {
        Self {
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            tool_call_id,
            working_dir: self.working_dir.clone(),
            stdin_request_tx: self.stdin_request_tx.clone(),
            graceful_shutdown_signal: self.graceful_shutdown_signal.clone(),
            execution_mode: self.execution_mode,
        }
    }

    #[inline]
    pub fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(ref base) = self.working_dir {
            base.join(path)
        } else {
            path.to_path_buf()
        }
    }

    /// Create a minimal ToolContext for testing
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self {
            session_id: "test-session".into(),
            message_id: "test-msg".into(),
            tool_call_id: "test-call".into(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: ToolExecutionMode::AgentTurn,
        }
    }
}

/// A tool that can be executed by the agent.
///
/// ## 增强说明
/// - 新增 `aliases()` 支持工具别名查找（源自 Claude Code 的 Tool.aliases[]）
/// - 新增 `is_concurrency_safe()` 支持并发安全标记（源自 Claude Code 的 isConcurrencySafe()）
/// - 新增 `is_enabled()` 支持条件启用/禁用（源自 Claude Code 的 isEnabled()）
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must match what's sent to the API).
    fn name(&self) -> &str;

    /// Optional aliases for backwards compatibility.
    /// A tool can be looked up by any of these names in addition to its primary name.
    /// Default: `&[]` (no aliases).
    /// 源自 Claude Code 的 `aliases?: string[]`.
    fn aliases(&self) -> &[&str] {
        &[]
    }

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON Schema for the input parameters.
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given input.
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput>;

    /// Whether this tool is a read-only operation (safe to parallelize).
    /// Read-only tools can execute concurrently without side effects.
    /// Default: `false` (assume mutating).
    fn is_read_only(&self) -> bool {
        false
    }

    /// Whether this tool is destructive (modifies files/deletes data).
    /// Destructive tools may require user confirmation before execution.
    /// Default: `false` (assume safe).
    fn is_destructive(&self) -> bool {
        false
    }

    /// Whether this tool is concurrency-safe.
    /// True = can run in parallel with other tools.
    /// 源自 Claude Code 的 `isConcurrencySafe(input)`.
    fn is_concurrency_safe(&self) -> bool {
        self.is_read_only() // 默认与 is_read_only 一致
    }

    /// Whether this tool is enabled in the current environment.
    /// 源自 Claude Code 的 `isEnabled()`.
    fn is_enabled(&self) -> bool {
        true
    }

    /// Maximum number of characters in the tool's output.
    /// Returns `None` for unlimited output (subject to global limits).
    fn max_result_size_chars(&self) -> Option<usize> {
        None
    }

    /// Optional MCP server source information for dynamically registered tools.
    fn mcp_source_info(&self) -> Option<&str> {
        None
    }

    /// Convert to API tool definition.
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: self.parameters_schema(),
            read_only: self.is_read_only(),
            destructive: self.is_destructive(),
        }
    }
}

/// Enhanced tool definition with read-only and destructive annotations.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnnotatedToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub is_read_only: bool,
    pub is_destructive: bool,
    pub mcp_source: Option<String>,
}

impl From<&dyn Tool> for AnnotatedToolDefinition {
    fn from(tool: &dyn Tool) -> Self {
        Self {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.parameters_schema(),
            is_read_only: tool.is_read_only(),
            is_destructive: tool.is_destructive(),
            mcp_source: tool.mcp_source_info().map(String::from),
        }
    }
}

// Re-exports from submodules
pub use streaming_executor::{
    StreamingToolExecutor, ExecutorConfig, ToolCallRequest, ExecutionProgress,
    OrderedToolResult,
};
pub use sub_agent::{
    SubAgentPool, SubAgentTask, SubAgentResult, SubAgentConfig, AgentRunner,
    SubAgentProgress, OutputFormat, Artifact, ArtifactType, SubAgentId,
};
pub use tool_discovery::{ToolDiscoveryEngine, ToolEmbeddingIndex, ToolSearchResult};

// Re-exports from Claude Code ported modules
pub use permissions::{
    PermissionMode, PermissionBehavior, PermissionResult, ToolPermissionContext,
    ToolFilterContext, ToolSafetyContext, PermissionRule,
};
pub use result_budget::{
    ToolResultBudgetManager, ToolResultBudgetConfig, ToolResultDecision,
    ContentReplacementState, ReplacedResult, ReplacementReason,
};
pub use file_history::{FileHistory, MessageSnapshot, FileSnapshot, RewindResult, RestoredFile};
pub use debug_log::{DebugLogManager, LogLevel, LogEntry, BufferedWriter};
pub use error_types::{
    ToolError, ConfigError, ShellError, AbortError, PermissionError,
    NetworkError, BudgetError, JcodeError,
    error_message, is_abort_error, short_error_stack, is_fs_not_found, is_timeout_error,
};
pub use settings_priority::{
    SettingsPriorityResolver, SettingSource, SettingValue,
    source_display_name, parse_setting_sources_flag,
};
