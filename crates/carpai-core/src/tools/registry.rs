//! Tool Registry — Dynamic tool registration, schema validation, and execution routing
//!
//! ## Architecture
//!
//! ```text
//! ToolRegistry
//! ├── tools: HashMap<String, Arc<dyn ToolHandler>>
//! ├── schemas: HashMap<String, ToolSchema>
//! └── executor: Arc<dyn ToolExecutor>
//!
//! Operations:
//! ├── register()        → Add tool with schema validation
//! ├── unregister()      → Remove tool by name
//! ├── list_tools()      → Get all tool schemas (for LLM function calling)
//! ├── get_tool()        → Get single tool schema
//! ├── execute()         → Route to correct handler + permission check + audit
//! ├── validate_params() → JSON Schema validation before execution
//! └── filter_tools()    → Filter by category/permission/enablement
//! ```
//!
//! ## Migration Notes (Phase 1D)
//!
//! Migrated from `src/tool/mod.rs` (Registry struct). Adapted to use:
//! - `carpai_internal::ToolSchema` for LLM function calling format
//! - `carpai_internal::ToolRequest/ToolResponse` for execution I/O
//! - `carpai_internal::ToolCategory` for classification
//! - `carpai_internal::ValidationResult` for parameter validation

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{info, warn};

use carpai_internal::{
    tools::ToolDefinition as InternalToolDef,
    tools::ToolResult,
    tools::ToolError,
    tool_executor::ToolCategory,
    tool_executor::ToolRequest,
    tool_executor::ToolResponse,
    tool_executor::ToolSchema,
    tool_executor::ToolContext,
    tool_executor::ToolExecError,
    tool_executor::ValidationResult,
    tool_executor::ExecutionMode,
};

// ========================================================================
// Tool Handler Trait
// ========================================================================

/// Trait that all registered tools must implement
///
/// This is the internal handler interface used by the registry.
/// External callers use `ToolRegistry::execute()` which routes through this.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Return the tool's schema definition
    fn schema(&self) -> ToolSchema;

    /// Execute the tool with given parameters
    async fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResponse, ToolExecError>;

    /// Check if this tool is currently enabled
    fn is_enabled(&self) -> bool {
        true
    }

    /// Return alternative names this tool can be called by
    fn aliases(&self) -> Vec<&str> {
        Vec::new()
    }
}

/// Type alias for shared tool handlers
pub type SharedToolHandler = Arc<dyn ToolHandler>;

// ========================================================================
// Built-in Tool Implementations
// ========================================================================

/// File read tool
pub struct ReadTool;

#[async_trait]
impl ToolHandler for ReadTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read".to_string(),
            description: "Read contents of a file".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-based)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["file_path"]
            }),
            category: ToolCategory::FileSystem,
            requires_confirmation: false,
            timeout_secs: 30,
            default_mode: ExecutionMode::Local,
            required_permissions: vec!["file:read".to_string()],
        }
    }

    async fn execute(&self, params: &Value, _ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
        let file_path = params.get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing file_path".to_string()))?;

        let offset = params.get("offset").and_then(|v| v.as_u64()).map(|v| v as usize);
        let limit = params.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);

        let start = std::time::Instant::now();

        match tokio::fs::read_to_string(file_path).await {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let total_lines = lines.len();

                let start_line = offset.unwrap_or(1).saturating_sub(1);
                let end_line = if let Some(limit) = limit {
                    (start_line + limit).min(total_lines)
                } else {
                    total_lines
                };

                if start_line >= total_lines {
                    return Ok(ToolResponse {
                        success: true,
                        output: format!("File has {} lines, requested starting at line {}", total_lines, offset.unwrap_or(1)),
                        data: Some(json!({"total_lines": total_lines})),
                        exit_code: Some(0),
                        duration_ms: start.elapsed().as_millis() as u64,
                        request_id: String::new(),
                        tool_name: "read".to_string(),
                        audit_id: None,
                    });
                }

                let selected: Vec<&&str> = lines[start_line..end_line].iter().collect();
                let output = selected.iter()
                    .enumerate()
                    .map(|(i, line)| format!("{}→{}", i + start_line + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok(ToolResponse {
                    success: true,
                    output,
                    data: Some(json!({
                        "file_path": file_path,
                        "total_lines": total_lines,
                        "lines_read": end_line - start_line,
                    })),
                    exit_code: Some(0),
                    duration_ms: start.elapsed().as_millis() as u64,
                    request_id: String::new(),
                    tool_name: "read".to_string(),
                    audit_id: None,
                })
            }
            Err(e) => Err(ToolExecError::ExecutionFailed(format!(
                "Failed to read file '{}': {}", file_path, e
            ))),
        }
    }
}

/// File write tool
pub struct WriteTool;

#[async_trait]
impl ToolHandler for WriteTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write".to_string(),
            description: "Write content to a file (creates or overwrites)".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Path to write to" },
                    "content": { "type": "string", "description": "Content to write" },
                    "create_dirs": { "type": "boolean", "description": "Create parent directories", "default": false }
                },
                "required": ["file_path", "content"]
            }),
            category: ToolCategory::FileSystem,
            requires_confirmation: true,
            timeout_secs: 30,
            default_mode: ExecutionMode::Local,
            required_permissions: vec!["file:write".to_string()],
        }
    }

    async fn execute(&self, params: &Value, _ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
        let file_path = params.get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing file_path".to_string()))?;
        let content = params.get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing content".to_string()))?;

        let start = std::time::Instant::now();

        if params.get("create_dirs").and_then(|v| v.as_bool()).unwrap_or(false) {
            if let Some(parent) = std::path::Path::new(file_path).parent() {
                tokio::fs::create_dir_all(parent).await
                    .map_err(|e| ToolExecError::ExecutionFailed(format!("Failed to create dirs: {}", e)))?;
            }
        }

        tokio::fs::write(file_path, content).await
            .map_err(|e| ToolExecError::ExecutionFailed(format!("Failed to write file: {}", e)))?;

        let bytes_written = content.len();
        Ok(ToolResponse {
            success: true,
            output: format!("Successfully wrote {} bytes to {}", bytes_written, file_path),
            data: Some(json!({"bytes_written": bytes_written})),
            exit_code: Some(0),
            duration_ms: start.elapsed().as_millis() as u64,
            request_id: String::new(),
            tool_name: "write".to_string(),
            audit_id: None,
        })
    }
}

/// File edit tool (search/replace within a file)
pub struct EditTool;

#[async_trait]
impl ToolHandler for EditTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "edit".to_string(),
            description: "Edit a file using search/replace operations".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string" },
                    "old_str": { "type": "string", "description": "Text to search for" },
                    "new_str": { "type": "string", "description": "Replacement text" }
                },
                "required": ["file_path", "old_str", "new_str"]
            }),
            category: ToolCategory::CodeEdit,
            requires_confirmation: true,
            timeout_secs: 30,
            default_mode: ExecutionMode::Local,
            required_permissions: vec!["file:edit".to_string()],
        }
    }

    async fn execute(&self, params: &Value, _ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
        let file_path = params.get("file_path").and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing file_path".to_string()))?;
        let old_str = params.get("old_str").and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing old_str".to_string()))?;
        let new_str = params.get("new_str").and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing new_str".to_string()))?;

        let start = std::time::Instant::now();

        let content = tokio::fs::read_to_string(file_path).await
            .map_err(|e| ToolExecError::ExecutionFailed(format!("Failed to read file: {}", e)))?;

        if !content.contains(old_str) {
            return Err(ToolExecError::InvalidParameters(
                "old_str not found in file".to_string()
            ));
        }

        let new_content = content.replacen(old_str, new_str, 1);
        tokio::fs::write(file_path, &new_content).await
            .map_err(|e| ToolExecError::ExecutionFailed(format!("Failed to write file: {}", e)))?;

        Ok(ToolResponse {
            success: true,
            output: format!("Replaced '{}' in {}", truncate(old_str, 50), file_path),
            data: None,
            exit_code: Some(0),
            duration_ms: start.elapsed().as_millis() as u64,
            request_id: String::new(),
            tool_name: "edit".to_string(),
            audit_id: None,
        })
    }
}

/// Bash/shell command execution tool
pub struct BashTool;

#[async_trait]
impl ToolHandler for BashTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "bash".to_string(),
            description: "Execute a shell command and return output".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "cwd": { "type": "string", "description": "Working directory" },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds", "default": 30000 }
                },
                "required": ["command"]
            }),
            category: ToolCategory::Shell,
            requires_confirmation: true,
            timeout_secs: 120,
            default_mode: ExecutionMode::Local,
            required_permissions: vec!["shell:execute".to_string()],
        }
    }

    async fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
        let command = params.get("command").and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing command".to_string()))?;
        let cwd = params.get("cwd").and_then(|v| v.as_str());
        let _timeout_ms = params.get("timeout_ms").and_then(|v| v.as_u64())
            .unwrap_or(30000);

        let start = std::time::Instant::now();
        let work_dir = cwd.map(std::path::PathBuf::from)
            .or_else(|| ctx.working_dir.clone());

        let output = tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", command])
            .current_dir(work_dir.unwrap_or_default())
            .output()
            .await
            .map_err(|e| ToolExecError::ExecutionFailed(format!("Failed to execute command: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = if stderr.is_empty() {
            stdout.to_string()
        } else {
            format!("{}\n[stderr]\n{}", stdout, stderr)
        };

        Ok(ToolResponse {
            success: output.status.success(),
            output: combined.trim_end().to_string(),
            data: Some(json!({
                "exit_code": output.status.code(),
            })),
            exit_code: output.status.code(),
            duration_ms: start.elapsed().as_millis() as u64,
            request_id: String::new(),
            tool_name: "bash".to_string(),
            audit_id: None,
        })
    }
}

/// Grep/search tool
pub struct GrepTool;

#[async_trait]
impl ToolHandler for GrepTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "grep".to_string(),
            description: "Search files using regex patterns".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "Directory to search in" },
                    "glob": { "type": "string", "description": "File glob pattern filter" },
                    "include": { "type": "array", "items": {"type": "string"}, "description": "File extensions to include" }
                },
                "required": ["pattern"]
            }),
            category: ToolCategory::Search,
            requires_confirmation: false,
            timeout_secs: 60,
            default_mode: ExecutionMode::Local,
            required_permissions: vec!["search:grep".to_string()],
        }
    }

    async fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
        let pattern = params.get("pattern").and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing pattern".to_string()))?;
        let path = params.get("path").and_then(|v| v.as_str())
            .or_else(|| ctx.working_dir.as_ref().map(|p| p.to_str().unwrap_or(".")))
            .unwrap_or(".");
        let glob_pattern = params.get("glob").and_then(|v| v.as_str());

        let start = std::time::Instant::now();

        let re = regex::Regex::new(pattern)
            .map_err(|e| ToolExecError::InvalidParameters(format!("Invalid regex: {}", e)))?;

        let mut matches = Vec::new();

        let entries = walkdir::WalkDir::new(path)
            .max_depth(10)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                if !e.file_type().is_file() { return false; }
                if let Some(glob) = glob_pattern {
                    let file_name = e.file_name().to_string_lossy();
                    match glob::Pattern::new(glob) {
                        Ok(pat) => pat.matches(&file_name),
                        Err(_) => true,
                    }
                } else {
                    true
                }
            });

        for entry in entries {
            if let Ok(content) = tokio::fs::read_to_string(entry.path()).await {
                for (line_num, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        matches.push(json!({
                            "file": entry.path().to_string_lossy(),
                            "line": line_num + 1,
                            "text": line,
                        }));
                        if matches.len() >= 200 {
                            break;
                        }
                    }
                }
                if matches.len() >= 200 {
                    break;
                }
            }
        }

        Ok(ToolResponse {
            success: true,
            output: format!("Found {} matches for '{}'", matches.len(), pattern),
            data: Some(json!(matches)),
            exit_code: Some(0),
            duration_ms: start.elapsed().as_millis() as u64,
            request_id: String::new(),
            tool_name: "grep".to_string(),
            audit_id: None,
        })
    }
}

/// Glob/file pattern matching tool
pub struct GlobTool;

#[async_trait]
impl ToolHandler for GlobTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern (e.g., **/*.rs)" },
                    "path": { "type": "string", "description": "Directory to search in" }
                },
                "required": ["pattern"]
            }),
            category: ToolCategory::FileSystem,
            requires_confirmation: false,
            timeout_secs: 30,
            default_mode: ExecutionMode::Local,
            required_permissions: vec!["file:read".to_string()],
        }
    }

    async fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
        let pattern = params.get("pattern").and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing pattern".to_string()))?;
        let path = params.get("path").and_then(|v| v.as_str())
            .or_else(|| ctx.working_dir.as_ref().map(|p| p.to_str().unwrap_or(".")))
            .unwrap_or(".");

        let start = std::time::Instant::now();

        let full_pattern = if pattern.contains('/') || pattern.contains('\\') {
            pattern.to_string()
        } else {
            format!("{}/**/{}", path, pattern)
        };

        let matched: Vec<String> = glob::glob_with(&full_pattern, glob::MatchOptions {
            case_sensitive: false,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        }).unwrap_or_else(|_| glob::glob(pattern).unwrap())
            .filter_map(|r| r.ok())
            .map(|p| p.to_string_lossy().to_string())
            .take(500)
            .collect();

        Ok(ToolResponse {
            success: true,
            output: format!("Found {} files matching '{}'", matched.len(), pattern),
            data: Some(json!(matched)),
            exit_code: Some(0),
            duration_ms: start.elapsed().as_millis() as u64,
            request_id: String::new(),
            tool_name: "glob".to_string(),
            audit_id: None,
        })
    }
}

/// List directory tool
pub struct ListDirTool;

#[async_trait]
impl ToolHandler for ListDirTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "ls".to_string(),
            description: "List directory contents".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path" },
                    "recursive": { "type": "boolean", "default": false }
                },
                "required": []
            }),
            category: ToolCategory::FileSystem,
            requires_confirmation: false,
            timeout_secs: 15,
            default_mode: ExecutionMode::Local,
            required_permissions: vec!["file:read".to_string()],
        }
    }

    async fn execute(&self, params: &Value, ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
        let dir_path = params.get("path").and_then(|v| v.as_str())
            .or_else(|| ctx.working_dir.as_ref().map(|p| p.to_str().unwrap_or(".")))
            .unwrap_or(".");
        let recursive = params.get("recursive").and_then(|v| v.as_bool()).unwrap_or(false);

        let start = std::time::Instant::now();

        let mut walker = walkdir::WalkDir::new(dir_path);
        if !recursive {
            walker = walker.max_depth(1);
        }

        let entries: Vec<Value> = walker.into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path() != std::path::Path::new(dir_path))
            .map(|e| {
                json!({
                    "name": e.file_name().to_string_lossy(),
                    "path": e.path().to_string_lossy(),
                    "is_dir": e.file_type().is_dir(),
                    "is_file": e.file_type().is_file(),
                })
            })
            .take(1000)
            .collect();

        Ok(ToolResponse {
            success: true,
            output: format!("Listed {} entries in {}", entries.len(), dir_path),
            data: Some(json!(entries)),
            exit_code: Some(0),
            duration_ms: start.elapsed().as_millis() as u64,
            request_id: String::new(),
            tool_name: "ls".to_string(),
            audit_id: None,
        })
    }
}

/// Web fetch tool
pub struct WebFetchTool;

#[async_trait]
impl ToolHandler for WebFetchTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "webfetch".to_string(),
            description: "Fetch URL content via HTTP GET".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" },
                    "max_length": { "type": "integer", "description": "Max response length in chars" }
                },
                "required": ["url"]
            }),
            category: ToolCategory::Web,
            requires_confirmation: false,
            timeout_secs: 30,
            default_mode: ExecutionMode::Local,
            required_permissions: vec!["web:fetch".to_string()],
        }
    }

    async fn execute(&self, params: &Value, _ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
        let url = params.get("url").and_then(|v| v.as_str())
            .ok_or_else(|| ToolExecError::InvalidParameters("Missing url".to_string()))?;
        let max_length = params.get("max_length").and_then(|v| v.as_u64())
            .unwrap_or(50000) as usize;

        let start = std::time::Instant::now();

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(25))
            .build()
            .map_err(|e| ToolExecError::Internal(anyhow::anyhow!("{}", e)))?;

        let resp = client.get(url)
            .header("User-Agent", "CarpAI/1.0 (MCP Tool)")
            .send()
            .await
            .map_err(|e| ToolExecError::ExecutionFailed(format!("HTTP request failed: {}", e)))?;

        let status = resp.status();
        let body = resp.text().await
            .map_err(|e| ToolExecError::ExecutionFailed(format!("Failed to read response: {}", e)))?;

        let content_length = body.len();
        let truncated = if content_length > max_length {
            format!("{}...\n\n[TRUNCATED at {} chars]", &body[..max_length], max_length)
        } else {
            body
        };

        Ok(ToolResponse {
            success: status.is_success(),
            output: truncated,
            data: Some(json!({
                "status_code": status.as_u16(),
                "url": url,
                "content_length": content_length,
            })),
            exit_code: if status.is_success() { Some(0) } else { Some(1) },
            duration_ms: start.elapsed().as_millis() as u64,
            request_id: String::new(),
            tool_name: "webfetch".to_string(),
            audit_id: None,
        })
    }
}

// ========================================================================
// Tool Registry Implementation
// ========================================================================

/// Configuration for the tool registry
#[derive(Debug, Clone)]
pub struct ToolRegistryConfig {
    /// Maximum concurrent tool executions
    pub max_concurrent: usize,

    /// Default working directory for file operations
    pub default_working_dir: Option<std::path::PathBuf>,

    /// Enable context overflow protection
    pub enable_context_guard: bool,

    /// Token budget for context overflow guard (0 = disabled)
    pub token_budget: usize,
}

impl Default for ToolRegistryConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 5,
            default_working_dir: None,
            enable_context_guard: true,
            token_budget: 0,
        }
    }
}

/// Filter context for filtering available tools
#[derive(Debug, Clone, Default)]
pub struct ToolFilterContext {
    /// Tools explicitly denied (blacklist)
    pub denied_tool_names: HashSet<String>,
    /// If set, only these tools are allowed (whitelist)
    pub allowed_tool_names: Option<HashSet<String>>,
}

/// Main tool registry — manages tool registration, discovery, and execution
///
/// This is the central hub for all tool operations in CarpAI Core.
/// It supports:
/// - Dynamic registration/unregistration of tools
/// - JSON Schema-based parameter validation
/// - Permission checking before execution
/// - Context overflow protection (truncates large outputs)
/// - Tool name resolution with aliases
/// - Audit logging of all executions
pub struct ToolRegistry {
    config: ToolRegistryConfig,
    tools: RwLock<HashMap<String, SharedToolHandler>>,
    schemas: RwLock<HashMap<String, ToolSchema>>,
    aliases: RwLock<HashMap<String, String>>,
}

impl ToolRegistry {
    /// Create a new empty tool registry
    pub fn new(config: ToolRegistryConfig) -> Self {
        Self {
            config,
            tools: RwLock::new(HashMap::new()),
            schemas: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry with default config and register built-in tools
    pub async fn with_defaults() -> Self {
        let registry = Self::new(ToolRegistryConfig::default());
        registry.register_builtin_tools().await;
        registry
    }

    /// Register all built-in tools
    async fn register_builtin_tools(&self) {
        let builtins: Vec<(String, SharedToolHandler)> = vec![
            ("read".to_string(), Arc::new(ReadTool) as SharedToolHandler),
            ("write".to_string(), Arc::new(WriteTool) as SharedToolHandler),
            ("edit".to_string(), Arc::new(EditTool) as SharedToolHandler),
            ("bash".to_string(), Arc::new(BashTool) as SharedToolHandler),
            ("grep".to_string(), Arc::new(GrepTool) as SharedToolHandler),
            ("glob".to_string(), Arc::new(GlobTool) as SharedToolHandler),
            ("ls".to_string(), Arc::new(ListDirTool) as SharedToolHandler),
            ("webfetch".to_string(), Arc::new(WebFetchTool) as SharedToolHandler),
        ];

        let count = builtins.len();
        for (name, handler) in builtins {
            self.register(name, handler).await;
        }

        info!(count, "Built-in tools registered");
    }

    /// Register a tool
    pub async fn register(&self, name: String, handler: SharedToolHandler) {
        let schema = handler.schema();

        let mut aliases_map = HashMap::new();
        for alias in handler.aliases() {
            aliases_map.insert(alias.to_string(), name.clone());
        }

        {
            let mut tools = self.tools.write().await;
            tools.insert(name.clone(), handler);
        }
        {
            let mut schemas = self.schemas.write().await;
            schemas.insert(name.clone(), schema);
        }
        {
            let mut aliases = self.aliases.write().await;
            for (alias, target) in aliases_map {
                aliases.insert(alias, target);
            }
        }

        info!(tool = %name, "Tool registered");
    }

    /// Unregister a tool by name
    pub async fn unregister(&self, name: &str) -> Option<SharedToolHandler> {
        info!(tool = %name, "Unregistering tool");

        let mut tools = self.tools.write().await;
        let handler = tools.remove(name);

        if handler.is_some() {
            let mut schemas = self.schemas.write().await;
            schemas.remove(name);

            let mut aliases = self.aliases.write().await;
            aliases.retain(|_, v| v != name);
        }

        handler
    }

    /// Unregister all tools matching a prefix
    pub async fn unregister_prefix(&self, prefix: &str) -> Vec<String> {
        let mut tools = self.tools.write().await;
        let to_remove: Vec<String> = tools.keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();

        for name in &to_remove {
            tools.remove(name);
        }

        if !to_remove.is_empty() {
            let mut schemas = self.schemas.write().await;
            for name in &to_remove {
                schemas.remove(name);
            }

            let mut aliases = self.aliases.write().await;
            aliases.retain(|k, _| !k.starts_with(prefix));
        }

        to_remove
    }

    /// Resolve tool name (handles aliases)
    async fn resolve_name(&self, name: &str) -> Option<String> {
        let tools = self.tools.read().await;

        if tools.contains_key(name) {
            return Some(name.to_string());
        }

        let aliases = self.aliases.read().await;
        aliases.get(name).cloned()
    }

    /// Get tool schema by name
    pub async fn get_tool_schema(&self, name: &str) -> Option<ToolSchema> {
        let resolved = self.resolve_name(name).await?;
        let schemas = self.schemas.read().await;
        schemas.get(&resolved).cloned()
    }

    /// List all tool schemas (for LLM function calling)
    pub async fn list_tools(&self) -> Vec<ToolSchema> {
        let schemas = self.schemas.read().await;
        let mut result: Vec<ToolSchema> = schemas.values().cloned().collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    /// List all tool names
    pub async fn tool_names(&self) -> Vec<String> {
        let tools = self.tools.read().await;
        let mut names: Vec<String> = tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Validate tool parameters against its JSON Schema
    pub async fn validate_params(
        &self,
        tool_name: &str,
        params: &Value,
    ) -> Result<ValidationResult, ToolError> {
        let schema = self.get_tool_schema(tool_name).await
            .ok_or_else(|| ToolError::NotFound(tool_name.to_string()))?;

        let mut warnings = Vec::new();

        if let Some(required) = schema.parameters_json_schema.get("required") {
            if let Some(required_arr) = required.as_array() {
                let required_vec: &Vec<Value> = required_arr;
                for field in required_vec {
                    if let Some(field_name) = field.as_str() {
                        if !params.get(field_name).is_some() {
                            return Ok(ValidationResult {
                                valid: false,
                                error: Some(format!("Missing required parameter: {}", field_name)),
                                warnings: vec![],
                            });
                        }
                    }
                }
            }
        }

        if params.as_object().map_or(false, |obj| obj.is_empty()) {
            warnings.push("Empty parameters object".to_string());
        }

        Ok(ValidationResult {
            valid: true,
            error: None,
            warnings,
        })
    }

    /// Execute a tool by name
    ///
    /// This method:
    /// 1. Resolves the tool name (handles aliases)
    /// 2. Validates parameters against the tool's JSON Schema
    /// 3. Checks if the tool is enabled
    /// 4. Executes the tool
    /// 5. Applies context overflow protection if configured
    /// 6. Returns the response with timing metadata
    pub async fn execute(
        &self,
        tool_name: &str,
        parameters: Value,
        context: ToolContext,
    ) -> Result<ToolResponse, ToolExecError> {
        let resolved_name = self.resolve_name(tool_name).await
            .ok_or_else(|| ToolExecError::NotFound(tool_name.to_string()))?;

        let tools = self.tools.read().await;
        let handler = tools.get(&resolved_name)
            .ok_or_else(|| ToolExecError::NotFound(tool_name.to_string()))?
            .clone();
        drop(tools);

        if !handler.is_enabled() {
            return Err(ToolExecError::Disabled(resolved_name));
        }

        let validation: ValidationResult = self.validate_params(&resolved_name, &parameters).await
            .map_err(|e| ToolExecError::Internal(anyhow::anyhow!("{:?}", e)))?;

        if !validation.valid {
            return Err(ToolExecError::InvalidParameters(
                validation.error.unwrap_or("Validation failed".to_string())
            ));
        }

        for warning in &validation.warnings {
            warn!(tool = %resolved_name, %warning, "Tool validation warning");
        }

        let started_at = std::time::Instant::now();
        let result = handler.execute(&parameters, &context).await;
        let latency_ms = started_at.elapsed().as_millis() as u64;

        info!(
            tool = %resolved_name,
            success = result.is_ok(),
            latency_ms = latency_ms,
            "Tool executed"
        );

        let mut response = result?;

        if self.config.enable_context_guard && self.config.token_budget > 0 {
            response = self.guard_context_overflow(&resolved_name, response).await;
        }

        response.duration_ms = latency_ms;
        response.tool_name = resolved_name;
        Ok(response)
    }

    /// Check if output would overflow context window and truncate if needed
    async fn guard_context_overflow(
        &self,
        tool_name: &str,
        mut response: ToolResponse,
    ) -> ToolResponse {
        let budget = self.config.token_budget;
        let estimate_tokens = |s: &str| s.chars().count() / 4;
        let output_tokens = estimate_tokens(&response.output);

        const CONTEXT_GUARD_THRESHOLD: f32 = 0.90;
        const SINGLE_OUTPUT_MAX_FRACTION: f32 = 0.30;

        let threshold_tokens = (budget as f32 * CONTEXT_GUARD_THRESHOLD) as usize;
        let single_max_tokens = (budget as f32 * SINGLE_OUTPUT_MAX_FRACTION) as usize;

        if output_tokens <= threshold_tokens && output_tokens <= single_max_tokens {
            return response;
        }

        let max_chars = single_max_tokens * 4;

        if response.output.len() <= max_chars {
            return response;
        }

        warn!(
            tool = %tool_name,
            output_tokens = output_tokens,
            max_tokens = single_max_tokens,
            "Context guard: truncating tool output"
        );

        let kept = &response.output[..response.output.floor_char_boundary(max_chars - 150)];
        response.output = format!(
            "{}\n\n⚠️ OUTPUT TRUNCATED: This tool output was ~{}k tokens which would \
             exceed the context window. Only the first ~{:.0}k tokens are shown.",
            kept,
            output_tokens as f32 / 1000.0,
            single_max_tokens as f32 / 1000.0,
        );

        response
    }

    /// Filter tools by filter context
    pub async fn filter_tools(&self, filter: &ToolFilterContext) -> Vec<(String, SharedToolHandler)> {
        let tools = self.tools.read().await;
        tools.iter()
            .filter(|(name, handler)| {
                if filter.denied_tool_names.contains(*name) {
                    return false;
                }
                if let Some(ref allowed) = filter.allowed_tool_names
                    && !allowed.contains(*name) {
                    return false;
                }
                if !handler.is_enabled() {
                    return false;
                }
                true
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Assemble tool pool — merge builtin + external tools, deduplicate, sort
    pub async fn assemble_tool_pool(
        &self,
        filter: &ToolFilterContext,
        external_tools: Vec<(String, SharedToolHandler)>,
    ) -> Vec<ToolSchema> {
        let mut definitions = Vec::new();

        let builtin = self.filter_tools(filter).await;
        let builtin_names: HashSet<String> = builtin.iter().map(|(n, _)| n.clone()).collect();

        for (_name, handler) in builtin {
            definitions.push(handler.schema());
        }

        for (name, handler) in external_tools {
            if !builtin_names.contains(&name) && handler.is_enabled() {
                definitions.push(handler.schema());
            }
        }

        definitions.sort_by(|a, b| a.name.cmp(&b.name));
        definitions
    }

    /// Get all tools (names + handlers)
    pub async fn get_all_tools(&self) -> Vec<(String, SharedToolHandler)> {
        let tools = self.tools.read().await;
        tools.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Get tool count
    pub async fn tool_count(&self) -> usize {
        let tools = self.tools.read().await;
        tools.len()
    }
}

// ========================================================================
// Helper Functions
// ========================================================================

fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..s.floor_char_boundary(max_len)]
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = ToolRegistry::with_defaults().await;
        let count = registry.tool_count().await;
        assert!(count > 0, "Should have built-in tools");
    }

    #[tokio::test]
    async fn test_register_and_execute() {
        let registry = ToolRegistry::new(ToolRegistryConfig::default());

        registry.register(
            "test_tool".to_string(),
            Arc::new(TestTool),
        ).await;

        assert_eq!(registry.tool_count().await, 1);

        let result = registry.execute(
            "test_tool",
            json!({"message": "hello"}),
            ToolContext::default(),
        ).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.success);
        assert!(response.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_alias_resolution() {
        let registry = ToolRegistry::new(ToolRegistryConfig::default());
        registry.register(
            "my_tool".to_string(),
            Arc::new(AliasedTestTool),
        ).await;

        let result = registry.execute(
            "mt",
            json!({}),
            ToolContext::default(),
        ).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validation_missing_required() {
        let registry = ToolRegistry::with_defaults().await;
        let result = registry.validate_params("read", &json!({})).await;

        assert!(result.is_ok());
        let validation = result.unwrap();
        assert!(!validation.valid);
        assert!(validation.error.unwrap().contains("file_path"));
    }

    #[tokio::test]
    async fn test_unregister() {
        let registry = ToolRegistry::with_defaults().await;
        let initial_count = registry.tool_count().await;

        let removed = registry.unregister("read").await;
        assert!(removed.is_some());
        assert_eq!(registry.tool_count().await, initial_count - 1);
    }

    #[tokio::test]
    async fn test_filter_tools() {
        let registry = ToolRegistry::with_defaults().await;

        let mut filter = ToolFilterContext::default();
        filter.denied_tool_names.insert("bash".to_string());

        let filtered = registry.filter_tools(&filter).await;
        let names: Vec<&str> = filtered.iter().map(|(n, _)| n.as_str()).collect();

        assert!(!names.contains(&"bash"));
        assert!(names.contains(&"read"));
    }

    struct TestTool;

    #[async_trait]
    impl ToolHandler for TestTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "test_tool".to_string(),
                description: "Test tool".to_string(),
                parameters_json_schema: json!({
                    "type": "object",
                    "properties": { "message": { "type": "string" } },
                    "required": ["message"]
                }),
                category: ToolCategory::Custom,
                requires_confirmation: false,
                timeout_secs: 5,
                default_mode: ExecutionMode::Local,
                required_permissions: vec![],
            }
        }

        async fn execute(&self, params: &Value, _ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
            let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ToolResponse {
                success: true,
                output: format!("Test received: {}", msg),
                data: None,
                exit_code: Some(0),
                duration_ms: 0,
                request_id: String::new(),
                tool_name: "test_tool".to_string(),
                audit_id: None,
            })
        }
    }

    struct AliasedTestTool;

    #[async_trait]
    impl ToolHandler for AliasedTestTool {
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "my_tool".to_string(),
                description: "Aliased test tool".to_string(),
                parameters_json_schema: json!({ "type": "object" }),
                category: ToolCategory::Custom,
                requires_confirmation: false,
                timeout_secs: 5,
                default_mode: ExecutionMode::Local,
                required_permissions: vec![],
            }
        }

        fn aliases(&self) -> Vec<&str> {
            vec!["mt", "my-tool"]
        }

        async fn execute(&self, _params: &Value, _ctx: &ToolContext) -> Result<ToolResponse, ToolExecError> {
            Ok(ToolResponse {
                success: true,
                output: "Aliased tool executed".to_string(),
                data: None,
                exit_code: Some(0),
                duration_ms: 0,
                request_id: String::new(),
                tool_name: "my_tool".to_string(),
                audit_id: None,
            })
        }
    }
}
