# CarpAI Enhanced Features - API Documentation

## 📚 Table of Contents

1. [MCP Enhanced Client](#1-mcp-enhanced-client)
2. [LSP Enhanced Client](#2-lsp-enhanced-client)
3. [Extended Commands System](#3-extended-commands-system)
4. [Skills System](#4-skills-system)
5. [App State Management](#5-app-state-management)

---

## 1. MCP Enhanced Client

### Overview
Enhanced MCP (Model Context Protocol) client with advanced features ported from claude_code_src.

**File**: `src/mcp/enhanced_client.rs`

### Key Components

#### `EnhancedMcpConfig`
Configuration for enhanced MCP client connections.

```rust
pub struct EnhancedMcpConfig {
    pub name: String,                    // Server name
    pub transport_type: TransportType,   // StdIO, SSE, StreamableHTTP, WebSocket
    pub command: Option<String>,         // Command to spawn
    pub args: Vec<String>,               // Command arguments
    pub env: HashMap<String, String>,    // Environment variables
    pub request_timeout_secs: u64,       // Request timeout
    pub max_retries: u32,                // Max retry attempts
    pub retry_delay_ms: u64,             // Delay between retries
    pub enable_oauth: bool,              // Enable OAuth authentication
}
```

**Usage Example**:
```rust
let config = EnhancedMcpConfig {
    name: "filesystem".to_string(),
    transport_type: TransportType::StdIO,
    command: Some("npx".to_string()),
    args: vec!["@modelcontextprotocol/server-filesystem".to_string()],
    request_timeout_secs: 30,
    max_retries: 3,
    ..Default::default()
};

let client = EnhancedMcpClient::connect(config).await?;
```

#### `TransportType`
Supported transport types:
- `StdIO` - Standard input/output (subprocess)
- `SSE` - Server-Sent Events (HTTP)
- `StreamableHTTP` - Newer MCP protocol
- `WebSocket` - WebSocket transport

#### `McpError`
Custom error types for better error handling:

```rust
pub enum McpError {
    AuthError { server_name, message },
    SessionExpired { server_name },
    ToolCallError { message, telemetry_message },
    Connection(String),
    Timeout(String),
    Protocol(String),
    Request { code, message },
    Configuration(String),
}
```

**Key Methods**:
- `is_session_expired()` - Check if error is session expiry
- `is_auth_error()` - Check if error is auth-related
- `server_name()` - Get server name from error

#### `EnhancedMcpHandle`
Handle for interacting with MCP server.

**Methods**:
```rust
// With automatic retry
pub async fn request_with_retry(&self, method: &str, params: Option<Value>) -> Result<JsonRpcResponse>

// Tool calls with progress reporting
pub async fn call_tool_with_progress(&self, tool_name: &str, arguments: Value) -> Result<ToolCallResult>

// Basic tool call
pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolCallResult>

// Resource management
pub async fn list_resources(&self) -> Result<Vec<ResourceData>>
pub async fn read_resource(&self, uri: &str) -> Result<Vec<ContentBlock>>

// Prompt management
pub async fn list_prompts(&self) -> Result<Vec<PromptDef>>
pub async fn get_prompt(&self, name: &str, arguments: Option<Value>) -> Result<Vec<Message>>

// State queries
pub fn name(&self) -> &str
pub async fn connection_state(&self) -> ConnectionState
pub fn tools(&self) -> Vec<McpToolDef>
```

#### `ConnectionState`
Server connection state machine:
- `Disconnected`
- `Connecting`
- `Connected`
- `Reconnecting`
- `Error(String)`
- `NeedsAuth`

#### `EnhancedMcpClient`
Full lifecycle management client.

**Methods**:
```rust
// Connect to server
pub async fn connect(config: EnhancedMcpConfig) -> Result<Self>

// Get handle
pub fn handle(&self) -> &EnhancedMcpHandle

// Disconnect cleanly
pub async fn disconnect(self) -> Result<()>

// Health check
pub async fn ping(&self) -> Result<Duration>
pub async fn health_check(&self) -> HealthStatus
```

---

## 2. LSP Enhanced Client

### Overview
Enhanced LSP (Language Server Protocol) client with lifecycle management and performance monitoring.

**File**: `src/lsp_enhanced.rs`

### Key Components

#### `EnhancedLspConfig`
LSP server configuration:

```rust
pub struct EnhancedLspConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub language_ids: HashMap<String, String>,
    pub root_path: Option<PathBuf>,
    pub initialization_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub auto_restart: bool,
    pub max_restarts: u32,
}
```

#### `EnhancedLspServerState`
Server state machine:
- `Stopped`, `Starting`, `Running`, `Stopping`, `Error`, `Crashed`

**Methods**:
- `label()` - Get string representation
- `is_operational()` - Check if server is running

#### `EnhancedLspHandle`
Handle for LSP operations with timing information.

**Core Methods**:
```rust
// Navigation
pub async fn goto_definition(&self, uri: &Url, position: Position) -> Result<LspOperationResult<Option<GotoDefinitionResponse>>>
pub async fn find_references(&self, uri: &Url, position: Position, context: ReferenceContext) -> Result<LspOperationResult<Vec<Location>>>

// Information
pub async fn hover(&self, uri: &Url, position: Position) -> Result<LspOperationResult<Option<Hover>>>
pub async fn document_symbol(&self, uri: &Url) -> Result<LspOperationResult<Vec<DocumentSymbol>>>
pub async fn workspace_symbol(&self, query: &str) -> Result<LspOperationResult<Vec<SymbolInformation>>>

// Code actions
pub async fn completion(&self, uri: &Url, position: Position, context: Option<CompletionContext>) -> Result<LspOperationResult<CompletionResponse>>
pub async fn code_action(&self, uri: &Url, range: Range, context: CodeActionContext) -> Result<LspOperationResult<Vec<CodeActionOrCommand>>>

// Notifications
pub async fn publish_diagnostics(&self, uri: &Url, version: Option<i32>, diagnostics: Vec<Diagnostic>) -> Result<()>

// Event handling
pub async fn on_notification<F>(&self, method: &str, handler: F)
```

#### `LspOperationResult<T>`
Wrapper with timing info:
```rust
pub struct LspOperationResult<T> {
    pub result: T,
    pub latency_ms: u128,
    pub cached: bool,
}
```

#### `LspMetrics`
Performance metrics:
```rust
pub struct LspMetrics {
    pub total_requests: u64,
    pub total_notifications: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub average_latency_ms: f64,
    pub last_request_latency_ms: Option<f64>,
    pub uptime_seconds: u64,
    pub restart_count: u32,
}
```

#### `EnhancedDiagnosticRegistry`
Diagnostic caching and history:

```rust
let registry = Arc::new(EnhancedDiagnosticRegistry::new(100));

// Update diagnostics
registry.update(&uri, Some(version), diagnostics);

// Query
let errors = registry.get_errors_count();
let warnings = registry.get_warnings_count();
let file_diags = registry.get_diagnostics_for_file(&uri_string);

// Management
registry.clear_uri(&uri_string);
registry.clear_all();
```

#### `EnhancedLspServer`
Full server lifecycle management:

```rust
// Start server
let server = EnhancedLspServer::connect(config).await?;

// Get components
let handle = server.handle();
let diag_registry = server.diagnostic_registry();

// Restart
let new_server = server.restart().await?;

// Shutdown
server.shutdown().await?;
```

---

## 3. Extended Commands System

### Overview
Extended command system with /btw, /fast, /rewind commands.

**File**: `src/cli/extended_commands.rs`

### Commands

#### `/btw` - Context-Aware Hints
Shows contextual hints based on current work.

**Usage**: `/btw [context]`

**Features**:
- Context-aware suggestions
- Task-specific tips
- Mode recommendations

**Example Output**:
```
🤔 By the way...

1. 💡 Tip: Consider breaking this task into smaller steps
2. 📝 You can use /fast mode for quicker iterations
3. 🔄 Use /rewind if you want to undo recent changes
4. 🎯 Focus on the most impactful changes first
```

#### `/fast` - Fast Mode Toggle
Toggle between speed modes.

**Usage**: `/fast [normal|fast|turbo]`

**Modes**:
| Mode | Description | Settings |
|------|-------------|----------|
| Normal | Full reasoning | thinking_budget=full, response_detail=high |
| Fast | Reduced thinking | thinking_budget=reduced, response_detail=medium |
| Turbo | Maximum speed | thinking_budget=minimal, response_detail=low |

**Example**:
```rust
// Toggle modes
registry.execute_command("fast", &ctx, None).await?;  // Normal → Fast
registry.execute_command("fast", &ctx, Some("turbo")).await?;  // Set to Turbo
```

#### `/rewind` - Session Rollback
Rollback session to a previous snapshot.

**Usage**: `/rewind [list|<snapshot_id>]`

**Features**:
- Automatic snapshots
- Snapshot listing with metadata
- Selective rollback
- Max 10 snapshots by default

**API**:
```rust
// Create snapshot
let snap_id = rewind_cmd.create_snapshot("Before refactoring", msg_count, tool_calls).await;

// List snapshots
let snaps = rewind_cmd.list_snapshots().await;

// Rewind
rewind_cmd.rewind_to("snap_1234567890").await?;
```

### Command Registry

```rust
// Initialize all commands
let registry = init_extended_commands().await;

// Register custom command
registry.register(Arc::new(MyCustomCommand)).await;

// Execute command
let result = registry.execute_command("btw", &ctx, None).await?;

// List available commands
let commands = registry.list_commands().await;
```

### Creating Custom Commands

Implement `ExtendedCommand` trait:

```rust
#[async_trait]
impl ExtendedCommand for MyCommand {
    fn name(&self) -> &str { "mycommand" }
    fn description(&self) -> &str { "My custom command" }
    fn usage(&self) -> &str { "/mycommand <args>" }

    async fn validate_args(&self, args: Option<&str>) -> Result<()> {
        // Validate arguments
        Ok(())
    }

    async fn execute(&self, ctx: &CommandContext, args: Option<&str>) -> Result<CommandResult> {
        Ok(CommandResult {
            success: true,
            message: "Done!".to_string(),
            data: None,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }
}
```

---

## 4. Skills System

### Overview
Advanced agent skills for iterative execution, validation, and optimization.

**File**: `src/skill_system.rs`

### Skills

#### `loop` Skill
Iterative execution with automatic improvement.

**Use Cases**:
- Tasks requiring multiple attempts
- Optimization problems
- Refactoring iterations

**Configuration**:
```rust
let ctx = SkillContext {
    task_description: "Optimize database query".to_string(),
    constraints: SkillConstraints {
        max_iterations: 10,
        quality_threshold: 0.8,
        timeout_secs: 300,
        ..Default::default()
    },
    ..Default::default()
};

let result = skills.execute_skill("loop", &ctx).await?;
```

**Output Metrics**:
- `success` - Whether quality threshold was met
- `quality_score` - Final quality score (0.0-1.0)
- `iterations_used` - Number of iterations executed
- `duration_ms` - Total execution time

#### `verify` Skill
Comprehensive result validation.

**Built-in Checks**:
1. **syntax_check** - Basic syntax validation
2. **content_validation** - Content completeness check
3. **error_detection** - Common error pattern detection

**Output Format**:
```
🔍 Verification Results (2/3)

✅ syntax_check: Syntax looks valid
   Details: Input length: 150
❌ content_validation: Content seems incomplete
   Details: Character count: 30
✅ error_detection: No error patterns detected
```

#### `simplify` Skill
Code/text simplification and optimization.

**Features**:
- Remove unnecessary complexity
- Collapse whitespace
- Optimize structure
- Report reduction statistics

**Output Example**:
```
✨ Simplification Results

Original size: 500 characters
Simplified size: 350 characters
Reduction: 30.0%

Simplified output:
```
[optimized code]
```
```

### Skill Cost Estimation

Each skill provides cost estimates before execution:

```rust
let estimate = skill.estimate_cost(&ctx).await;
println!("Estimated time: {}ms", estimate.estimated_time_ms);
println!("Token usage: ~{}", estimate.token_usage_estimate);
println!("Complexity: {:?}", estimate.complexity); // Low, Medium, High
```

### Skills Registry

```rust
// Initialize system
let skills = init_skills_system().await;

// Register custom skill
skills.register(Arc::new(MyCustomSkill)).await;

// Execute skill
let result = skills.execute_skill("verify", &ctx).await?;

// Get best skill for task
if let Some((name, cost)) = skills.get_best_skill_for_task("optimize code").await {
    println!("Recommended skill: {}", name);
}

// View history
let history = skills.get_history().await;
```

### Creating Custom Skills

Implement `Skill` trait:

```rust
#[async_trait]
impl Skill for MyCustomSkill {
    fn name(&self) -> &str { "myskill" }
    fn description(&self) -> &str { "My custom skill" }

    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        Ok(SkillResult {
            success: true,
            output: "Processed!".to_string(),
            quality_score: Some(0.9),
            iterations_used: 1,
            duration_ms: 100,
            metadata: HashMap::new(),
        })
    }

    async fn can_execute(&self, ctx: &SkillContext) -> bool { true }

    async fn estimate_cost(&self, ctx: &SkillContext) -> SkillCostEstimate {
        SkillCostEstimate {
            estimated_time_ms: 200,
            token_usage_estimate: 50,
            complexity: SkillComplexity::Low,
        }
    }
}
```

---

## 5. App State Management

### Overview
Centralized application state management with observer pattern and selectors.

**File**: `src/app_state.rs`

### Core Concepts

#### `AppState`
Main application state structure:

```rust
pub struct AppState {
    pub version: u64,
    pub timestamp: DateTime<Utc>,
    pub session: SessionState,
    pub ui: UiState,
    pub config: ConfigState,
    pub tools: ToolsState,
    pub custom: HashMap<String, Value>,
}
```

#### `AppStateManager`
State manager with full lifecycle support.

**Initialization**:
```rust
// Basic initialization
let manager = AppStateManager::new(50);

// With default subscriptions
let manager = create_state_manager_with_defaults().await;
```

**State Updates**:
```rust
// Atomic update
manager.update(|state| {
    state.config.model_name = "gpt-4".to_string();
    state.ui.theme = "dark".to_string();
}).await?;

// Batch updates
batch_update(&manager, vec![
    Box::new(|state| { state.version += 1; }),
    Box::new(|state| { state.session.id = "new".to_string(); }),
]).await?;
```

**Querying State**:

Using **Selector Pattern**:
```rust
// Built-in selectors
let model = manager.select::<String, _>(&ModelNameSelector).await;
let theme = manager.select::<String, _>(&ThemeSelector).await;
let count = manager.select::<u64, _>(&MessageCountSelector).await;

// Custom selector
struct CustomSelector;
impl StateSelector<MyDataType> for CustomSelector {
    fn select(&self, state: &AppState) -> MyDataType {
        // Extract and return specific data
    }
}
```

**Undo/Redo**:
```rust
// Make changes...
manager.update(|state| { /* ... */ }).await?;

// Undo
while manager.undo().await? {
    println!("Undone!");
}

// History length
let len = manager.history_length().await;
```

**Persistence**:
```rust
// Save to disk
manager.persist(Path::new("state.json")).await?;

// Load from disk
manager.load(Path::new("state.json")).await?;

// Reset to defaults
manager.reset().await?;
```

**Observer Pattern**:
```rust
// Subscribe to all changes
manager.subscribe(|old, new| {
    println!("Changed v{} → v{}", old.version, new.version);
}).await;

// Broadcast channel subscription
let mut rx = manager.subscribe_channel();
tokio::spawn(async move {
    while let Ok(change) = rx.recv().await {
        println!("Broadcast: v{}", change.version);
    }
});
```

**Custom Data**:
```rust
// Merge custom data
manager.merge_custom_data([
    ("key1".to_string(), json!(value1)),
    ("key2".to_string(), json!(value2)),
].into_iter().collect()).await?;

// Get custom value
let value = manager.get_custom_value("key1").await;
```

**Statistics**:
```rust
// Increment counters
manager.increment_message_count().await?;
manager.increment_tool_call_count().await?;
manager.set_current_task(Some("Task X".to_string())).await?;

// Summary
println!("{}", manager.summary().await);
```

### Built-in Selectors

| Selector | Type | Description |
|----------|------|-------------|
| `SessionIdSelector` | `String` | Current session ID |
| `MessageCountSelector` | `u64` | Total messages in session |
| `ThemeSelector` | `String` | UI theme name |
| `ModelNameSelector` | `String` | Active model name |

### Sub-State Structures

#### SessionState
```rust
pub struct SessionState {
    pub id: String,
    pub started_at: DateTime<Utc>,
    pub message_count: u64,
    pub tool_call_count: u64,
    pub current_task: Option<String>,
}
```

#### UiState
```rust
pub struct UiState {
    pub theme: String,
    pub font_size: u8,
    pub show_line_numbers: bool,
    pub sidebar_visible: bool,
}
```

#### ConfigState
```rust
pub struct ConfigState {
    pub model_name: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub auto_save: bool,
}
```

#### ToolsState
```rust
pub struct ToolsState {
    pub enabled_tools: Vec<String>,
    pub recent_tools: Vec<String>,
    pub tool_configs: HashMap<String, Value>,
}
```

---

## 🚀 Quick Start Guide

### 1. Setup MCP Connection

```rust
use carpai::mcp::enhanced_client::*;

#[tokio::main]
async fn main() -> Result<()> {
    let config = EnhancedMcpConfig {
        name: "filesystem".to_string(),
        transport_type: TransportType::StdIO,
        command: Some("npx".to_string()),
        args: vec!["@modelcontextprotocol/server-filesystem".to_string(), "/tmp".to_string()],
        ..Default::default()
    };

    let client = EnhancedMcpClient::connect(config).await?;
    
    let tools = client.handle().tools();
    println!("Available tools: {:?}", tools);

    client.disconnect().await?;
    Ok(())
}
```

### 2. Use Extended Commands

```rust
use carpai::cli::extended_commands::*;

#[tokio::main]
async fn main() -> Result<()> {
    let registry = init_extended_commands().await;
    let ctx = CommandContext::default();

    // Show hints
    let result = registry.execute_command("btw", &ctx, None).await?;
    println!("{}", result.message);

    // Switch to fast mode
    let result = registry.execute_command("fast", &ctx, Some("fast")).await?;
    println!("{}", result.message);

    Ok(())
}
```

### 3. Run Skills

```rust
use carpai::skill_system::*;

#[tokio::main]
async fn main() -> Result<()> {
    let skills = init_skills_system().await;

    let ctx = SkillContext {
        task_description: "Refactor this code".to_string(),
        ..Default::default()
    };

    // Verify code
    let result = skills.execute_skill("verify", &ctx).await?;
    println!("{}", result.output);

    // Simplify code
    let result = skills.execute_skill("simplify", &ctx).await?;
    println!("{}", result.output);

    Ok(())
}
```

### 4. Manage App State

```rust
use carpai::app_state::*;

#[tokio::main]
async fn main() -> Result<()> {
    let manager = create_state_manager_with_defaults().await;

    // Update state
    manager.update(|state| {
        state.config.model_name = "gpt-4".to_string();
    }).await?;

    // Query using selector
    let model = manager.select::<String, _>(&ModelNameSelector).await;
    println!("Current model: {}", model);

    // Persist
    manager.persist(Path::new("app_state.json")).await?;

    Ok(())
}
```

---

## 🔧 Configuration Reference

### MCP Client Configuration Options

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | `String` | Required | Server identifier |
| `transport_type` | `TransportType` | `StdIO` | Connection type |
| `command` | `Option<String>` | None | Spawn command |
| `args` | `Vec<String>` | `[]` | Command arguments |
| `request_timeout_secs` | `u64` | 30 | Request timeout |
| `max_retries` | `u32` | 3 | Retry attempts |
| `retry_delay_ms` | `u64` | 1000 | Delay between retries |
| `enable_oauth` | `bool` | false | OAuth support |

### LSP Client Configuration Options

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | `String` | Required | Server identifier |
| `command` | `String` | Required | Server command |
| `root_path` | `Option<PathBuf>` | None | Workspace root |
| `initialization_timeout_secs` | `u64` | 30 | Init timeout |
| `request_timeout_secs` | `u64` | 10 | Request timeout |
| `auto_restart` | `bool` | false | Auto-restart on crash |
| `max_restarts` | `u32` | 3 | Max restart attempts |

### Skills Constraints

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `max_iterations` | `u32` | 10 | Max loop iterations |
| `timeout_secs` | `u64` | 300 | Skill timeout |
| `quality_threshold` | `f64` | 0.8 | Minimum quality score |
| `allowed_tools` | `Vec<String>` | `[]` | Allowed tool names |

---

## 📖 Best Practices

### Error Handling

Always use proper error handling with McpError:

```rust
match client.handle().call_tool("tool_name", json!({})).await {
    Ok(result) => { /* success */ }
    Err(e) => {
        if e.is_session_expired() {
            // Re-authenticate
        } else if e.is_auth_error() {
            // Handle auth failure
        } else {
            // Generic error
        }
    }
}
```

### Performance Monitoring

Use LSP metrics for monitoring:

```rust
let metrics = lsp_handle.metrics();
if metrics.failed_requests > 0 {
    warn!("{} requests failed", metrics.failed_requests);
}

if metrics.average_latency_ms > 1000.0 {
    warn!("High average latency: {:.0}ms", metrics.average_latency_ms);
}
```

### State Management Best Practices

1. **Use Selectors**: Always use selectors for querying state
2. **Batch Updates**: Group related updates together
3. **Subscribe Wisely**: Only subscribe to changes you need
4. **Persist Regularly**: Save state periodically
5. **Limit History**: Keep history size reasonable (50-100)

---

## ❓ FAQ

**Q: How do I add OAuth to my MCP server?**
A: Set `enable_oauth: true` in `EnhancedMcpConfig` and provide OAuth configuration.

**Q: Can I run multiple LSP servers?**
A: Yes, create multiple `EnhancedLspServer` instances with different configs.

**Q: How do I create custom commands/skills?**
A: Implement the `ExtendedCommand` or `Skill` trait and register them.

**Q: Is thread-safe?**
A: Yes, all components use Arc/RwLock/Mutex for safe concurrent access.

**Q: How do I debug state issues?**
A: Use `manager.summary().await` for debugging, or subscribe to changes.

---

## 📝 Version History

- **v1.0.0** (2025): Initial release with MCP/LSP enhancements, extended commands, skills system, and AppState management

---

*Generated automatically from source code documentation.*
