//! Tool Executor Trait - Unified tool execution with sandboxing
//!
//! This module **extends** (not replaces) the existing `ToolRegistry` trait.
//!
//! ## Architecture
//!
//! ```
//! ToolRegistry (existing)          ToolExecutor (this module)
//! ┌────────────────────┐           ┌──────────────────────────┐
//! │ register()         │           │ execute_sandboxed()      │
//! │ list_tools()       │  ───────> │ execute_local()          │
//! │ get_tool()         │  delegates│ validate_permissions()   │
//! │ validate_params()  │           │ get_tool_schema()        │
//! └────────────────────┘           └──────────────────────────┘
//!                                       │
//!                              ┌────────┼────────┐
//!                              ▼        ▼        ▼
//!                         SandboxTool  LocalTool  RemoteTool
//!                         Executor    Executor   Executor
//! ```
//!
//! ## Key Design Decisions
//!
//! 1. **Execution mode is a runtime choice**, not compile-time.
//!    The same tool can run locally (CLI) or in a sandbox (Server).
//!
//! 2. **Permission check is mandatory** before execution.
//!    Server mode always checks RBAC; CLI mode may skip.
//!
//! 3. **Audit trail** — every execution produces an `ToolExecutionRecord`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

// ========================================================================
// Core Trait
// ========================================================================

/// Unified tool execution interface with security and audit
///
/// Implementations:
/// - **LocalToolExecutor** (`carpai-cli`): Direct process spawn, mirrors `src/tool/mod.rs`
/// - **SandboxToolExecutor** (`carpai-server`): Docker/gVisor/namespace isolation
/// - **RemoteToolExecutor** (`carpai-server` → MCP): Delegate to external MCP server
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool with full context and permission checking
    ///
    /// This is the **primary** entry point. It:
    /// 1. Validates permissions
    /// 2. Resolves the tool handler
    /// 3. Executes in the configured mode (local/sandbox/remote)
    /// 4. Records audit trail
    /// 5. Returns result with timing metadata
    async fn execute(
        &self,
        request: ToolRequest,
    ) -> Result<ToolResponse, ToolExecError>;

    /// List all available tools (with schemas for LLM function calling)
    async fn list_tools(&self) -> Result<Vec<ToolSchema>, ToolExecError>;

    /// Get schema for a single tool
    async fn get_tool_schema(&self, name: &str) -> Result<Option<ToolSchema>, ToolExecError>;

    /// Validate parameters without executing
    async fn validate(
        &self,
        name: &str,
        params: &serde_json::Value,
    ) -> Result<ValidationResult, ToolExecError>;

    /// Check if a user has permission to use a tool
    async fn check_permission(
        &self,
        user_id: &str,
        tool_name: &str,
    ) -> Result<bool, ToolExecError>;

    /// Cancel a running tool execution (by request_id)
    async fn cancel(&self, request_id: &str) -> Result<(), ToolExecError>;
}

// ========================================================================
// Request / Response Types
// ========================================================================

/// Full tool execution request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    /// Tool name (must match registered tool)
    pub tool_name: String,

    /// Parameters as JSON object (must conform to tool's JSON Schema)
    pub parameters: serde_json::Value,

    /// Execution context
    pub context: ToolContext,

    /// Unique request ID for tracking/cancellation
    #[serde(default = "new_request_id")]
    pub request_id: String,

    /// Execution mode override (None = use default)
    pub mode_override: Option<ExecutionMode>,
}

fn new_request_id() -> String {
    format!("req-{}", uuid::Uuid::new_v4().simple())
}

/// Execution context — who, where, how
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolContext {
    /// User/tenant ID (for permission check + audit)
    pub user_id: String,

    /// Session ID this execution belongs to
    pub session_id: String,

    /// Working directory for file operations
    pub working_dir: Option<PathBuf>,

    /// Environment variables to inject
    pub env_vars: HashMap<String, String>,

    /// Timeout (None = tool default)
    pub timeout: Option<Duration>,

    /// Whether this requires confirmation before executing
    pub require_confirmation: bool,

    /// Arbitrary metadata forwarded to the tool
    pub metadata: HashMap<String, String>,
}

/// Tool execution response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    /// Whether execution succeeded
    pub success: bool,

    /// Output content (stdout or structured result)
    pub output: String,

    /// Structured data if available (e.g., file read results)
    pub data: Option<serde_json::Value>,

    /// Exit code (for shell-like tools)
    pub exit_code: Option<i32>,

    /// Execution time in milliseconds
    pub duration_ms: u64,

    /// Request ID (echoes back from request)
    pub request_id: String,

    /// Tool name that was executed
    pub tool_name: String,

    /// Audit record ID
    pub audit_id: Option<String>,
}

// ========================================================================
// Tool Schema (for LLM function calling)
// ========================================================================

/// Tool schema as exposed to LLM providers (OpenAI function calling format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Unique tool identifier
    pub name: String,

    /// Human-readable description (used by LLM to decide when to call)
    pub description: String,

    /// JSON Schema for parameters
    pub parameters_json_schema: serde_json::Value,

    /// Tool category
    pub category: ToolCategory,

    /// Whether this tool requires user confirmation
    pub requires_confirmation: bool,

    /// Default timeout in seconds
    pub timeout_secs: u64,

    /// Execution mode used when no override
    pub default_mode: ExecutionMode,

    /// Required permission scopes
    pub required_permissions: Vec<String>,
}

/// Tool category classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCategory {
    /// File system operations (read, write, edit, delete)
    FileSystem,
    /// Code analysis and editing (AST-based edits, refactoring)
    CodeEdit,
    /// Shell / command execution (bash, powershell)
    Shell,
    /// Web / HTTP requests (curl, fetch)
    Web,
    /// Database operations (SQL queries)
    Database,
    /// AI/ML inference (embedding, classification)
    Inference,
    /// System information (os, cpu, memory)
    SystemInfo,
    /// Version control (git operations)
    VersionControl,
    /// Search (code search, grep, semantic search)
    Search,
    /// Custom / user-defined
    Custom,
}

/// Execution mode — how the tool actually runs
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Direct local execution (CLI mode, trusted environment)
    Local,
    /// Sandboxed execution (Docker, gVisor, namespace isolation)
    Sandboxed,
    /// Delegated to remote MCP server
    Remote { endpoint: String },
    /// Dry-run — validate only, don't execute
    DryRun,
}

// ========================================================================
// Validation
// ========================================================================

/// Result of parameter validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether parameters are valid
    pub valid: bool,

    /// Error message if invalid
    pub error: Option<String>,

    /// Warnings (non-fatal issues)
    pub warnings: Vec<String>,
}

// ========================================================================
// Audit Record
// ========================================================================

/// Immutable audit record for every tool execution
///
/// In server mode, these are written to the audit log.
/// In CLI mode, they are kept in-memory or written to local log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    /// Unique record ID
    pub id: String,

    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,

    /// Who executed
    pub user_id: String,

    /// Which session
    pub session_id: String,

    /// What tool
    pub tool_name: String,

    /// Parameters (may be redacted for sensitive fields)
    pub parameters_redacted: serde_json::Value,

    /// Success or failure
    pub success: bool,

    /// Duration
    pub duration_ms: u64,

    /// Exit code if applicable
    pub exit_code: Option<i32>,

    /// Execution mode used
    pub mode: ExecutionMode,

    /// IP address (server mode)
    pub client_ip: Option<String>,
}

// ========================================================================
// Errors
// ========================================================================

/// Tool execution errors
#[derive(Debug, thiserror::Error)]
pub enum ToolExecError {
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    #[error("Permission denied: user={user}, tool={tool}")]
    PermissionDenied { user: String, tool: String },

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Timeout after {0}s")]
    Timeout(u64),

    #[error("Tool disabled: {0}")]
    Disabled(String),

    #[error("Sandbox error: {0}")]
    Sandbox(String),

    #[error("Cancellation requested")]
    Cancelled,

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_request_serialization() {
        let req = ToolRequest {
            tool_name: "read_file".into(),
            parameters: serde_json::json!({"path": "/tmp/test.rs"}),
            context: ToolContext::default(),
            request_id: "test-req-1".into(),
            mode_override: Some(ExecutionMode::Sandboxed),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("read_file"));
        assert!(json.contains("Sandboxed"));
    }

    #[test]
    fn test_tool_category_hash() {
        // Ensure ToolCategory can be used as HashMap key
        let mut map = HashMap::new();
        map.insert(ToolCategory::FileSystem, vec!["read", "write"]);
        map.insert(ToolCategory::Shell, vec!["bash"]);
        assert_eq!(map.len(), 2);
    }
}
