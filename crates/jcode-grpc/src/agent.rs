//! Agent Workflow Engine for CarpAI
//!
//! Provides autonomous AI agent capabilities with:
//! - **Tool Use**: Function calling and tool execution
//! - **Multi-step Reasoning**: Chain-of-thought, ReAct, Tree-of-Thoughts
//! - **Planning**: Task decomposition and execution planning
//! - **Memory**: Short-term and long-term context management
//! - **Collaboration**: Multi-agent orchestration
//!
//! ## Architecture
//!
//! ```text
//! +---------------------------------------------+
//! |                  Agent Orchestrator          |
//! |  +----------+  +----------+  +----------+    |
//! |  | Planner  |->| Executor |->| Evaluator|    |
//! |  +----------+  +----------+  +----------+    |
//! |         ^              v              ^     |
//! |    +----+----+   +-----+-----+   +----+----+ |
//! |    | Memory  |   |  Tools    |   | LLM Core | |
//! |    | (RAG)   |   | (Code Exec)|  |(Reasoning)| |
//! |    +---------+   +-----------+   +----------+ |
//! +---------------------------------------------+
//! ```

use std::sync::Arc;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};
use tracing::{info, debug, warn, instrument};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Agent workflow configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Maximum number of iterations/steps
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    
    /// Timeout for entire workflow in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    
    /// Enable verbose logging of reasoning steps
    #[serde(default)]
    pub verbose: bool,
    
    /// Memory configuration
    #[serde(default)]
    pub memory: MemoryConfig,
    
    /// Tool configuration
    #[serde(default)]
    pub tools: ToolsConfig,
}

fn default_max_iterations() -> u32 { 20 }
fn default_timeout() -> u64 { 300 } // 5 minutes

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
            timeout_secs: default_timeout(),
            verbose: false,
            memory: Default::default(),
            tools: Default::default(),
        }
    }
}

/// Memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Maximum context window size (in tokens)
    #[serde(default = "default_context_window")]
    pub context_window_tokens: usize,
    
    /// Number of recent messages to keep in working memory
    #[serde(default = "default_working_memory")]
    pub working_memory_size: usize,
    
    /// Enable long-term memory (vector store)
    #[serde(default = "default_true")]
    pub enable_long_term_memory: bool,
    
    /// Enable episodic memory (summarization)
    #[serde(default = "default_true")]
    pub enable_episodic_memory: bool,
}

fn default_context_window() -> usize { 128000 }
fn default_working_memory() -> usize { 50 }
fn default_true() -> bool { true }

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            context_window_tokens: default_context_window(),
            working_memory_size: default_working_memory(),
            enable_long_term_memory: default_true(),
            enable_episodic_memory: default_true(),
        }
    }
}

/// Tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// Available tools for the agent to use
    #[serde(default)]
    pub available_tools: Vec<ToolDefinition>,
    
    /// Allow code execution
    #[serde(default = "default_true")]
    pub allow_code_execution: bool,
    
    /// Allow file system access
    #[serde(default = "default_false")]
    pub allow_file_system_access: bool,
    
    /// Allow network requests
    #[serde(default = "default_true")]
    pub allow_network_requests: bool,
    
    /// Sandbox mode for code execution
    #[serde(default = "default_true")]
    pub sandbox_mode: bool,
}

fn default_false() -> bool { false }

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            available_tools: vec![],
            allow_code_execution: true,
            allow_file_system_access: false,
            allow_network_requests: true,
            sandbox_mode: true,
        }
    }
}

/// Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name
    pub name: String,
    
    /// Human-readable description
    pub description: String,
    
    /// Parameters schema (JSON Schema)
    pub parameters: serde_json::Value,
    
    /// Whether the tool is required
    #[serde(default)]
    pub required: bool,
    
    /// Execution handler (internal use)
    #[serde(skip)]
    pub handler: Option<Arc<dyn ToolHandler>>,
}

/// Trait for tool handlers
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult>;
}

/// Result from tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether the execution was successful
    pub success: bool,
    
    /// Output data (text, JSON, etc.)
    pub output: String,
    
    /// Structured output if applicable
    #[serde(default)]
    pub structured_output: Option<serde_json::Value>,
    
    /// Error message if failed
    #[serde(default)]
    pub error: Option<String>,
    
    /// Execution time in milliseconds
    #[serde(default)]
    pub execution_time_ms: f64,
    
    /// Metadata about the result
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Agent task/request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTask {
    /// Unique task ID
    pub id: String,
    
    /// User's goal/objective
    pub goal: String,
    
    /// Initial context/instructions
    pub context: String,
    
    /// Files or resources to work with
    #[serde(default)]
    pub resources: Vec<ResourceRef>,
    
    /// Constraints and requirements
    #[serde(default)]
    pub constraints: Vec<String>,
    
    /// Expected output format
    #[serde(default)]
    pub expected_format: OutputFormat,
    
    /// Priority level
    #[serde(default)]
    pub priority: TaskPriority,
}

/// Resource reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRef {
    pub resource_type: ResourceType,
    pub path_or_url: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Resource types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResourceType {
    File,
    Url,
    Directory,
    Database,
    ApiEndpoint,
    Custom(String),
}

/// Expected output format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputFormat {
    Text,
    Code,
    Json,
    Markdown,
    DiffPatch,
    TestResults,
    Documentation,
}

/// Task priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum TaskPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

impl Default for TaskPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Agent response/result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Task ID this response belongs to
    pub task_id: String,
    
    /// Final answer/solution
    pub final_answer: String,
    
    /// Step-by-step reasoning trace
    #[serde(default)]
    pub reasoning_trace: Vec<ReasoningStep>,
    
    /// Tools used during execution
    #[serde(default)]
    pub tools_used: Vec<ToolUsage>,
    
    /// Generated artifacts (code, files, etc.)
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    
    /// Metrics about the execution
    pub metrics: ExecutionMetrics,
    
    /// Whether the task was completed successfully
    pub success: bool,
    
    /// Error information if failed
    #[serde(default)]
    pub error: Option<String>,
}

/// Single reasoning step in the trace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningStep {
    /// Step number
    pub step_number: u32,
    
    /// Type of reasoning step
    pub step_type: StepType,
    
    /// The thought/reasoning content
    pub thought: String,
    
    /// Action taken (if any)
    #[serde(default)]
    pub action: Option<Action>,
    
    /// Observation/result of action
    #[serde(default)]
    pub observation: Option<String>,
    
    /// Timestamp
    pub timestamp: i64,
}

/// Types of reasoning steps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StepType {
    /// Initial analysis/planning
    Planning,
    /// Using a tool
    ToolUse,
    /// Observing result
    Observation,
    /// Reflecting on progress
    Reflection,
    /// Making a decision
    Decision,
    /// Asking for clarification
    Clarification,
    /// Summarizing
    Summary,
}

/// Action taken by the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// Action type
    pub action_type: ActionType,
    
    /// Tool name (if tool action)
    #[serde(default)]
    pub tool_name: Option<String>,
    
    /// Parameters passed to tool/action
    pub input: serde_json::Value,
}

/// Action types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionType {
    /// Call a function/tool
    ToolCall,
    /// Write/edit code
    CodeEdit,
    /// Execute command
    CommandExecution,
    /// Make an API request
    ApiRequest,
    /// Search/query
    Search,
    /// Read file
    FileRead,
    /// Think/reflect
    Think,
}

/// Record of tool usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUsage {
    pub tool_name: String,
    pub call_count: u32,
    pub total_execution_time_ms: f64,
    pub success_rate: f64,
    pub example_call: Option<serde_json::Value>,
}

/// Artifact generated during execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_type: ArtifactType,
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub language: Option<String>, // For code artifacts
    #[serde(default)]
    pub path: Option<String>, // For file artifacts
}

/// Artifact types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    Code,
    Documentation,
    Test,
    Configuration,
    Data,
    Analysis,
    Diagram,
    Other(String),
}

/// Execution metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionMetrics {
    /// Total wall-clock time in seconds
    pub total_time_secs: f64,
    
    /// Total number of reasoning steps
    pub total_steps: u32,
    
    /// Number of tool calls made
    pub total_tool_calls: u32,
    
    /// Number of tokens consumed (approximate)
    pub total_tokens_consumed: u32,
    
    /// Cost estimate (if applicable)
    #[serde(default)]
    pub estimated_cost_usd: Option<f64>,
    
    /// Success rate of tool calls
    pub tool_success_rate: f64,
    
    /// Average latency per step
    pub avg_step_latency_ms: f64,
}

/// Main Agent orchestrator
pub struct AgentOrchestrator {
    config: AgentConfig,
    llm_client: Arc<dyn LlmClient>,
    tools: Arc<RwLock<HashMap<String, Arc<dyn ToolHandler>>>>,
    memory: Arc<RwLock<AgentMemory>>,
}

impl AgentOrchestrator {
    pub fn new(
        config: AgentConfig,
        llm_client: Arc<dyn LlmClient>,
    ) -> Self {
        let tools = Arc::new(RwLock::new(HashMap::new()));
        
        // Register built-in tools
        let mut orchestrator = Self {
            config,
            llm_client,
            tools,
            memory: Arc::new(RwLock::new(AgentMemory::new())),
        };
        
        orchestrator.register_builtin_tools();
        orchestrator
    }
    
    fn register_builtin_tools(&mut self) {
        // These would be registered properly in production
        // For now, just note that they exist
    }
    
    /// Execute an agent task
    #[instrument(skip(self), fields(task_id = %task.id))]
    pub async fn execute_task(&self, task: AgentTask) -> Result<AgentResponse> {
        info!(
            goal = %task.goal,
            priority = ?task.priority,
            "Executing agent task"
        );
        
        let start = std::time::Instant::now();
        let mut reasoning_trace = Vec::new();
        let mut tools_used: std::collections::HashMap<String, ToolUsage> = std::collections::HashMap::new();
        let mut artifacts = Vec::new();
        let mut current_context = task.context.clone();
        
        for iteration in 0..self.config.max_iterations {
            debug!(iteration = iteration, "Starting reasoning step");
            
            // Check timeout
            if start.elapsed().as_secs() >= self.config.timeout_secs {
                warn!(elapsed_secs = start.elapsed().as_secs(), "Task timed out");
                break;
            }
            
            // Step 1: Plan/Think
            let planning_prompt = format!(
                "Goal: {}\n\nContext so far:\n{}\n\nAvailable tools:\n{}\n\nWhat should I do next? Think step-by-step.",
                task.goal,
                current_context,
                self.format_available_tools()
            );
            
            let thinking_result = self.llm_client.complete(&planning_prompt).await?;
            
            reasoning_trace.push(ReasoningStep {
                step_number: iteration + 1,
                step_type: StepType::Planning,
                thought: thinking_result.clone(),
                action: None,
                observation: None,
                timestamp: chrono::Utc::now().timestamp(),
            });
            
            // Step 2: Determine if we need to use a tool
            let tool_decision = self.parse_tool_decision(&thinking_result)?;
            
            match tool_decision {
                Some(tool_action) => {
                    debug!(tool = %tool_action.tool_name.as_deref().unwrap_or("unknown"), "Executing tool");
                    
                    // Execute tool
                    let tool_start = Instant::now();
                    let tool_result = self.execute_tool(&tool_action).await?;
                    let tool_elapsed = tool_start.elapsed().as_millis() as f64;
                    
                    // Record tool usage
                    let entry = tools_used
                        .entry(tool_action.tool_name.clone().unwrap_or_default())
                        .or_insert_with(|| ToolUsage {
                            tool_name: tool_action.tool_name.clone().unwrap_or_default(),
                            call_count: 0,
                            total_execution_time_ms: 0.0,
                            success_rate: 1.0,
                            example_call: Some(tool_action.input.clone()),
                        });
                    
                    entry.call_count += 1;
                    entry.total_execution_time_ms += tool_elapsed;
                    if !tool_result.success {
                        entry.success_rate = (entry.success_rate * (entry.call_count - 1) as f64) / entry.call_count as f64;
                    }
                    
                    // Add observation to trace
                    reasoning_trace.push(ReasoningStep {
                        step_number: iteration + 1,
                        step_type: StepType::Observation,
                        thought: format!("Executed tool: {:?}", tool_action),
                        action: Some(tool_action),
                        observation: Some(if tool_result.success {
                            tool_result.output.clone()
                        } else {
                            format!("Error: {}", tool_result.error.unwrap_or("Unknown error".to_string()))
                        }),
                        timestamp: chrono::Utc::now().timestamp(),
                    });
                    
                    // Update context with observation
                    current_context.push_str(&format!("\n\n[Tool Result]:\n{}", 
                        if tool_result.success { &tool_result.output } else { &format!("Error: {}", tool_result.error.unwrap_or_default()) }
                    ));
                    
                    // Collect artifacts
                    if let Some(ref artifact_data) = tool_result.structured_output {
                        artifacts.push(Artifact {
                            artifact_type: ArtifactType::Other("tool-output".to_string()),
                            name: format!("output_{}", iteration),
                            content: artifact_data.to_string(),
                            ..Default::default()
                        });
                    }
                }
                
                None => {
                    // No tool needed, provide final answer
                    debug!("No tool needed, generating final answer");
                    
                    let final_answer_prompt = format!(
                        "Based on all the work done:\n\n{}\n\nProvide the final answer to: {}",
                        current_context,
                        task.goal
                    );
                    
                    let final_answer = self.llm_client.complete(&final_answer_prompt).await?;
                    
                    let elapsed = start.elapsed();
                    
                    return Ok(AgentResponse {
                        task_id: task.id.clone(),
                        final_answer,
                        reasoning_trace,
                        tools_used: tools_used.into_values().collect(),
                        artifacts,
                        metrics: ExecutionMetrics {
                            total_time_secs: elapsed.as_secs_f64(),
                            total_steps: iteration + 1,
                            total_tool_calls: tools_used.values().map(|t| t.call_count).sum(),
                            total_tokens_consumed: 1000, // Placeholder
                            estimated_cost_usd: None,
                            tool_success_rate: if !tools_used.is_empty() {
                                let total_successes: f64 = tools_used.values()
                                    .map(|t| t.success_rate * t.call_count as f64)
                                    .sum();
                                let total_calls: f64 = tools_used.values().map(|t| t.call_count).sum() as f64;
                                total_successes / total_calls.max(1.0)
                            } else {
                                1.0
                            },
                            avg_step_latency_ms: if iteration > 0 {
                                elapsed.as_millis() as f64 / (iteration + 1) as f64
                            } else {
                                0.0
                            },
                        },
                        success: true,
                        error: None,
                    });
                }
            }
        }
        
        // If we exhausted iterations without completing
        Err(anyhow::anyhow!("Max iterations ({}) reached without completion", self.config.max_iterations))
    }
    
    /// Parse LLM output to determine if tool use is needed
    fn parse_tool_decision(&self, text: &str) -> Result<Option<Action>> {
        // In production, this would use structured output/function calling
        // For now, simple heuristic: look for patterns like "I'll use [tool_name]"
        
        let tool_patterns = [
            ("read_file", r"(?:I'll|Let me|I need to|I should)\s+(?:use|call|invoke|run|execute)\s+.*(?:the\s+)?(?:read_file|file_read|file_reader)"),
            ("write_file", r"(?:I'll|Let me|I need to|I should)\s+(?:use|call|invoke|run|execute)\s+.*(?:write_file|file_write|file_writer)"),
            ("search_code", r"(?:I'll|Let me|I need to|I should)\s+(?:use|call|invoke|run|execute)\s+.*(?:search_code|code_search|grep)"),
            ("execute_code", r"(?:I'll|Let me|I need to|I should)\s+(?:use|call|invoke|run|execute)\s+.*(?:execute_code|run_code|bash|shell)"),
            ("web_search", r"(?:I'll|Let me|I need to|I should)\s+(?:use|call|invoke|run|execute)\s+.*(?:web_search|search|google|lookup)"),
        ];
        
        for (tool_name, pattern) in &tool_patterns {
            let regex = regex::Regex::new(pattern)?;
            if regex.is_match(text) {
                return Ok(Some(Action {
                    action_type: ActionType::ToolCall,
                    tool_name: Some(tool_name.to_string()),
                    input: serde_json::json!({}),
                }));
            }
        }
        
        Ok(None)
    }
    
    /// Execute a tool action
    async fn execute_tool(&self, action: &Action) -> Result<ToolResult> {
        let tool_name = action.tool_name.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No tool name specified"))?;
        
        let tools = self.tools.read().await;
        let handler = tools.get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", tool_name))?;
        
        handler.execute(action.input.clone()).await
    }
    
    /// Format available tools for prompt
    fn format_available_tools(&self) -> String {
        // This would list actual registered tools
        "- read_file(path): Read contents of a file\n\
         - write_file(path, content): Write content to a file\n\
         - search_code(query): Search codebase for pattern\n\
         - execute_code(code): Run code snippet\n\
         - web_search(query): Search the web".to_string()
    }
}

/// Agent memory system
struct AgentMemory {
    working_memory: Vec<MemoryEntry>,
    long_term_memory: Vec<MemoryEntry>,
    episodic_summaries: Vec<String>,
}

impl AgentMemory {
    fn new() -> Self {
        Self {
            working_memory: Vec::new(),
            long_term_memory: Vec::new(),
            episodic_summaries: Vec::new(),
        }
    }
    
    fn add_to_working(&mut self, entry: MemoryEntry) {
        self.working_memory.push(entry);
    }
    
    fn get_relevant_context(&self, query: &str) -> String {
        // Would implement similarity search here
        // For now, just concatenate recent entries
        self.working_memory.iter()
            .rev()
            .take(10)
            .map(|e| e.content.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Memory entry
struct MemoryEntry {
    id: Uuid,
    timestamp: i64,
    content: String,
    embedding: Option<Vec<f32>>, // For semantic search
    metadata: HashMap<String, String>,
}

/// Trait for LLM client interface
#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String>;
    async fn complete_stream(&self, prompt: &str) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send + '_>>>;
}

// Simple stream trait
trait Stream<Item> {
    type Item;
}

impl<T> Stream<T> for futures::stream::BoxStream<'static, T> where T: Unpin {}

/// Built-in tool implementations

/// File reading tool
pub struct ReadFileTool {
    base_path: std::path::PathBuf,
}

impl ReadFileTool {
    pub fn new(base_path: impl Into<std::path::PathBuf>) -> Self {
        Self { base_path: base_path.into() }
    }
}

#[async_trait::async_trait]
impl ToolHandler for ReadFileTool {
    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let path = params["path"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;
        
        let full_path = self.base_path.join(path);
        
        let content = tokio::fs::read_to_string(&full_path).await
            .with_context(|| format!("Failed to read file: {}", full_path.display()))?;
        
        Ok(ToolResult {
            success: true,
            output: content,
            structured_output: Some(serde_json::json!({
                "path": path,
                "size": content.len(),
                "lines": content.lines().count(),
            })),
            error: None,
            execution_time_ms: 10.0, // Placeholder
            metadata: [
                ("full_path".to_string(), full_path.to_string_lossy().to_string()),
            ].into_iter().collect(),
        })
    }
}

/// Code execution tool (sandboxed)
pub struct ExecuteCodeTool;

#[async_trait::async_trait]
impl ToolHandler for ExecuteCodeTool {
    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let code = params["code"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'code' parameter"))?;
        
        let language = params["language"].as_str().unwrap_or("python");
        
        // In production, this would use proper sandboxing (Docker, gVisor, etc.)
        // For now, just validate syntax
        
        let output = match language {
            "python" | "python3" => {
                // Could use rust-python or similar
                format!("Would execute Python:\n{}", code)
            }
            "javascript" | "js" => {
                format!("Would execute JavaScript:\n{}", code)
            }
            _ => {
                format!("Unsupported language: {} (code not executed)", language)
            }
        };
        
        Ok(ToolResult {
            success: true,
            output,
            structured_output: None,
            error: None,
            execution_time_ms: 50.0,
            metadata: [
                ("language".to_string(), language.to_string()),
                ("sandboxed".to_string(), "true".to_string()),
            ].into_iter().collect(),
        })
    }
}

/// Web search tool
pub struct WebSearchTool {
    api_key: Option<String>,
}

impl WebSearchTool {
    pub fn new(api_key: Option<String>) -> Self {
        Self { api_key }
    }
}

#[async_trait::async_trait]
impl ToolHandler for WebSearchTool {
    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let query = params["query"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'query' parameter"))?;
        
        // Would call search API (Google, Bing, DuckDuckGo, etc.)
        let results = format!("Search results for '{}': [placeholder results]", query);
        
        Ok(ToolResult {
            success: true,
            output: results,
            structured_output: Some(serde_json::json!({
                "query": query,
                "results": [],
                "total_results": 0,
            })),
            error: None,
            execution_time_ms: 200.0, // Typical web search latency
            metadata: [
                ("engine".to_string(), "placeholder".to_string()),
            ].into_iter().collect(),
        })
    }
}

/// Predefined agent workflows/patterns
pub mod workflows {
    use super::*;
    
    /// Code review workflow
    pub async fn code_review_workflow(
        orchestrator: &AgentOrchestrator,
        code: &str,
        file_path: &str,
    ) -> Result<AgentResponse> {
        let task = AgentTask {
            id: Uuid::new_v4().to_string(),
            goal: format!("Review the following code for bugs, security issues, performance problems, and best practices violations:\n\n```{}``\n\nFile: {}", 
                detect_language(file_path), code, file_path),
            context: "You are an expert code reviewer. Provide specific, actionable feedback.".to_string(),
            resources: vec![ResourceRef {
                resource_type: ResourceType::File,
                path_or_url: file_path.to_string(),
                ..Default::default()
            }],
            constraints: vec![
                "Focus on real issues, not style preferences".to_string(),
                "Suggest concrete improvements".to_string(),
            ],
            expected_format: OutputFormat::Markdown,
            priority: TaskPriority::Normal,
        };
        
        orchestrator.execute_task(task).await
    }
    
    /// Refactoring workflow
    pub async fn refactoring_workflow(
        orchestrator: &AgentOrchestrator,
        code: &str,
        goals: &[String],
    ) -> Result<AgentResponse> {
        let task = AgentTask {
            id: Uuid::new_v4().to_string(),
            goal: format!("Refactor the following code to achieve these goals:\n- {}\n\n```{}```",
                goals.join("\n- "), code),
            context: "You are a refactoring expert. Preserve functionality while improving quality.".to_string(),
            resources: vec![],
            constraints: vec![
                "Maintain backward compatibility".to_string(),
                "Add comments explaining changes".to_string(),
            ],
            expected_format: OutputFormat::DiffPatch,
            priority: TaskPriority::High,
        };
        
        orchestrator.execute_task(task).await
    }
    
    /// Debugging workflow
    pub async fn debugging_workflow(
        orchestrator: &AgentOrchestrator,
        error_message: &str,
        code_snippet: &str,
        stack_trace: Option<&str>,
    ) -> Result<AgentResponse> {
        let mut context = format!("Error: {}\n\nCode:\n```\n{}\n```", error_message, code_snippet);
        
        if let Some(trace) = stack_trace {
            context.push_str(&format!("\n\nStack Trace:\n```\n{}\n```", trace));
        }
        
        let task = AgentTask {
            id: Uuid::new_v4().to_string(),
            goal: "Debug this error and provide a fix. Identify root cause, explain why it happens, and show how to fix it.".to_string(),
            context,
            resources: vec![],
            constraints: vec![
                "Explain the root cause clearly".to_string(),
                "Provide a minimal reproduction case if possible".to_string(),
            ],
            expected_format: OutputFormat::Text,
            priority: TaskPriority::Critical,
        };
        
        orchestrator.execute_task(task).await
    }
    
    /// Detect programming language from file extension
    fn detect_language(file_path: &str) -> &'static str {
        match file_path.rsplit('.').next().unwrap_or("") {
            "rs" => "rust",
            "py" | "pyi" => "python",
            "ts" | "tsx" => "typescript",
            "js" | "jsx" | "mjs" => "javascript",
            "go" => "go",
            "java" => "java",
            "kt" | "kts" => "kotlin",
            "c" => "c",
            "cpp" | "cc" | "cxx" => "cpp",
            "h" | "hpp" => "cpp-header",
            "cs" => "csharp",
            "rb" => "ruby",
            "php" => "php",
            "swift" => "swift",
            "scala" => "scala",
            "sh" | "bash" => "bash",
            "sql" => "sql",
            "html" | "htm" => "html",
            "css" | "scss" | "less" => "css",
            "md" => "markdown",
            "toml" => "toml",
            "yaml" | "yml" => "yaml",
            "json" => "json",
            _ => "unknown",
        }
    }
}
