//! Tool Registry Trait - Unified tool discovery and execution interface
//!
//! Provides:
//! - Tool registration and discovery
//! - Parameter validation
//! - Execution with sandboxing
//! - Result aggregation

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Main tool registry trait
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// Register a new tool
    fn register(&mut self, definition: ToolDefinition) -> Result<(), ToolError>;

    /// Get all registered tools
    fn list_tools(&self) -> Vec<ToolDefinition>;

    /// Get tool by name
    fn get_tool(&self, name: &str) -> Option<ToolDefinition>;

    /// Execute a tool with parameters
    async fn execute(&self, execution: ToolExecution) -> Result<ToolResult, ToolError>;

    /// Validate tool parameters without executing
    fn validate_params(&self, tool_name: &str, params: &serde_json::Value) -> Result<(), ToolError>;

    /// Check if tool is available/enabled
    fn is_tool_available(&self, name: &str) -> bool;
}

/// Tool definition/schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// JSON Schema for parameters
    pub parameters: serde_json::Value,

    /// Whether tool requires confirmation
    pub requires_confirmation: bool,

    /// Tool category
    pub category: ToolCategory,

    /// Optional: timeout in seconds
    pub timeout_secs: Option<u64>,

    /// Optional: required permissions
    pub required_permissions: Vec<String>,
}

/// Tool category classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCategory {
    /// File system operations
    FileSystem,

    /// Code analysis/editing
    CodeEdit,

    /// Shell/command execution
    Shell,

    /// Web/HTTP requests
    Web,

    /// Database operations
    Database,

    /// AI/ML inference
    Inference,

    /// System information
    SystemInfo,

    /// Custom/user-defined
    Custom,
}

/// Tool execution request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    /// Tool name to execute
    pub tool_name: String,

    /// Parameters as JSON object
    pub parameters: serde_json::Value,

    /// Optional: execution context (working directory, env vars)
    pub context: Option<ExecutionContext>,

    /// Optional: unique request ID for tracking
    pub request_id: Option<String>,
}

/// Execution context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Working directory
    pub working_dir: Option<String>,

    /// Environment variables
    pub env_vars: HashMap<String, String>,

    /// Timeout override (seconds)
    pub timeout_secs: Option<u64>,
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether execution succeeded
    pub success: bool,

    /// Output content (stdout or error message)
    pub output: String,

    /// Exit code (for shell commands)
    pub exit_code: Option<i32>,

    /// Optional: structured result data
    pub data: Option<serde_json::Value>,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,
}

/// Tool error types
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Timeout exceeded: {0}s")]
    Timeout(u64),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Tool disabled: {0}")]
    ToolDisabled(String),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

/// Helper: Create a simple file read tool definition
pub fn file_read_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".to_string(),
        description: "Read contents of a file".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum number of lines to read",
                    "default": 1000
                }
            },
            "required": ["file_path"]
        }),
        requires_confirmation: false,
        category: ToolCategory::FileSystem,
        timeout_secs: Some(30),
        required_permissions: vec!["file:read".to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition_serialization() {
        let tool = file_read_tool_definition();
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("read_file"));
        assert!(json.contains("FileSystem"));
    }

    #[test]
    fn test_tool_result_creation() {
        let result = ToolResult {
            success: true,
            output: "File contents".to_string(),
            exit_code: Some(0),
            data: None,
            execution_time_ms: 150,
        };

        assert!(result.success);
        assert_eq!(result.output, "File contents");
    }
}
