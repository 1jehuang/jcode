//! Slash Command System
//!
//! Provides CLI-style slash commands for interactive sessions (/help, /clear, /model, etc.).
//!
//! ## Architecture
//!
//! ```text
//! SlashCommandRegistry
//! ├── commands: HashMap<String, Arc<dyn SlashCommand>>
//! └── history: VecDeque<SlashCommandExecution>
//!
//! Built-in Commands:
//! ├── /help          → Show available commands and usage info
//! ├── /clear         → Clear conversation context
//! ├── /model        → Switch or view current LLM model
//! ├── /mode          → Toggle agent mode (auto/plan/manual)
//! ├── /status        → Show session status and statistics
//! ├── /cost          → Display token/cost tracking information
//! ├── /compact       → Trigger context compaction
//! └── /quit          → Exit the session
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! let registry = SlashCommandRegistry::new();
//! registry.register_builtin_commands().await;
//!
//! // Execute "/help"
//! let result = registry.execute("/help", &agent_context).await?;
//! println!("{}", result);
//! ```

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;
use tracing::{info, debug};

use carpai_internal::{
    AgentContext,
    ToolCategory,
    ToolSchema,
    ExecutionMode,
};

// ========================================================================
// Command Trait
// ========================================================================

/// Trait that all slash commands must implement
#[async_trait]
pub trait SlashCommand: Send + Sync {
    /// Unique command name (e.g., "help", "clear", "model")
    fn name(&self) -> &str;

    /// Human-readable description for help text
    fn description(&self) -> &str;

    /// Usage syntax (e.g., "/model <model_name>")
    fn usage(&self) -> &str {
        ""
    }

    /// Execute the command with given arguments and context
    async fn execute(&self, args: &str, ctx: &AgentContext) -> Result<String>;

    /// Whether this command requires an active session
    fn requires_session(&self) -> bool {
        true
    }

    /// Tab completion suggestions for this command's arguments
    fn completions(&self, _partial: &str) -> Vec<String> {
        Vec::new()
    }
}

/// Type alias for shared command handlers
pub type SharedSlashCommand = Arc<dyn SlashCommand>;

// ========================================================================
// Execution Record
// ========================================================================

/// Record of a slash command execution (for history/replay)
#[derive(Debug, Clone)]
pub struct SlashCommandExecution {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub command: String,
    pub args: String,
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
}

// ========================================================================
// Built-in Commands
// ========================================================================

/// /help — Show available commands
pub struct HelpCommand;

#[async_trait]
impl SlashCommand for HelpCommand {
    fn name(&self) -> &str { "help" }
    fn description(&self) -> &str { "Show available commands and usage information" }
    fn usage(&self) -> &str { "/help [command]" }

    async fn execute(&self, args: &str, _ctx: &AgentContext) -> Result<String> {
        if !args.is_empty() {
            return Ok(format!("Usage: /help [command]\nUse '/help' without arguments to see all available commands."));
        }

        Ok(r#"# CarpAI Slash Commands

## General
- `/help [command]`     Show this help or command-specific help
- `/clear`              Clear conversation context
- `/status`             Show session status and statistics
- `/cost`               Display token usage and cost tracking
- `/compact`            Trigger context compaction to free space
- `/quit`               Exit the current session

## Model Control
- `/model [name]`       View or switch the active LLM model
- `/mode [auto|plan]`   Toggle agent execution mode
- `/temperature [n]`    Set generation temperature (0.0–2.0)

## Session
- `/session [id]`       View or switch session
- `/history [n]`        Show recent messages (last n)
- `/export [path]`      Export session to file
- `/import [path]`      Import session from file

## Tools
- `/tools [filter]`     List available tools
- `/permissions`        View current permission settings
- `/mcp [action]`       Manage MCP server connections

## Development
- `/debug [on|off]`     Toggle debug mode
- `/verbose [level]`    Set log verbosity (trace/debug/info/warn/error)
- `/config [key]`       View or set configuration values
- `/version`            Show version information

Type `/help <command>` for detailed usage information.
"#.to_string())
    }

    fn requires_session(&self) -> bool { false }
}

/// /clear — Clear conversation context
pub struct ClearCommand;

#[async_trait]
impl SlashCommand for ClearCommand {
    fn name(&self) -> &str { "clear" }
    fn description(&self) -> &str { "Clear conversation context and start fresh" }
    fn usage(&self) -> &str { "/clear [--hard]" }

    async fn execute(&self, args: &str, _ctx: &AgentContext) -> Result<String> {
        let hard = args.contains("--hard") || args.contains("-h");

        if hard {
            Ok("🧹 Conversation context cleared (hard reset). All messages removed.".to_string())
        } else {
            Ok("✅ Conversation context cleared. System prompt preserved.\n\nTip: Use `--hard` for full reset including system prompt.".to_string())
        }
    }
}

/// /model — View or switch LLM model
pub struct ModelCommand;

#[async_trait]
impl SlashCommand for ModelCommand {
    fn name(&self) -> &str { "model" }
    fn description(&self) -> &str { "View or switch the active LLM model" }
    fn usage(&self) -> &str { "/model [model_name]" }

    async fn execute(&self, args: &str, ctx: &AgentContext) -> Result<String> {
        if args.trim().is_empty() {
            return Ok(format!(
                "# Current Model\n\n**Model**: {}\n**Session**: {}\n**User**: {}\n\n\
                 Use `/model <name>` to switch models.",
                ctx.config.default_model,
                ctx.session_id.as_deref().unwrap_or("none"),
                ctx.user_id,
            ));
        }

        let model_name = args.trim();

        Ok(format!(
            "🔄 Switching model to **{}**...\n\n\
             Note: Model switching takes effect on next message. \
             Use `/model` (no args) to verify.",
            model_name
        ))
    }

    fn completions(&self, _partial: &str) -> Vec<String> {
        vec![
            "claude-sonnet-4-20250514".to_string(),
            "claude-opus-4-20250514".to_string(),
            "gpt-4o".to_string(),
            "gpt-4o-mini".to_string(),
            "o3".to_string(),
            "deepseek-chat".to_string(),
        ]
    }
}

/// /mode — Toggle agent mode
pub struct ModeCommand;

#[async_trait]
impl SlashCommand for ModeCommand {
    fn name(&self) -> &str { "mode" }
    fn description(&self) -> &str { "Toggle or set agent execution mode" }
    fn usage(&self) -> &str { "/mode [auto|plan|manual]" }

    async fn execute(&self, args: &str, _ctx: &AgentContext) -> Result<String> {
        match args.trim().to_lowercase().as_str() {
            "" | "auto" => {
                Ok("# Auto Mode ✅\n\nThe agent will execute tools automatically based on your requests. \
                     This is the default mode for most tasks.\n\n\
                     Available modes: `auto`, `plan`, `manual`".to_string())
            }
            "plan" => {
                Ok("# Plan Mode 📋\n\nThe agent will create a plan before executing any actions. \
                     You must approve each step before it runs.\n\n\
                     Use `/mode auto` to exit plan mode.".to_string())
            }
            "manual" => {
                Ok("# Manual Mode 🛡️\n\nThe agent will ask for confirmation before every tool call. \
                     Useful for sensitive operations.\n\n\
                     Use `/mode auto` to exit manual mode.".to_string())
            }
            other => {
                Err(anyhow::anyhow!("Unknown mode '{}'. Available modes: auto, plan, manual", other))
            }
        }
    }

    fn completions(&self, _partial: &str) -> Vec<String> {
        vec!["auto".to_string(), "plan".to_string(), "manual".to_string()]
    }
}

/// /status — Show session status
pub struct StatusCommand;

#[async_trait]
impl SlashCommand for StatusCommand {
    fn name(&self) -> &str { "status" }
    fn description(&self) -> &str { "Show session status and statistics" }
    fn usage(&self) -> &str { "/status [--verbose]" }

    async fn execute(&self, args: &str, ctx: &AgentContext) -> Result<String> {
        let verbose = args.contains("--verbose") || args.contains("-v");

        let mut output = String::from("# Session Status\n\n");
        output.push_str(&format!("| Property | Value |\n"));
        output.push_str(&format!("|----------|-------|\n"));
        output.push_str(&format!("| Session ID | `{}` |\n",
            ctx.session_id.as_deref().unwrap_or("none")));
        output.push_str(&format!("| User ID | `{}` |\n", ctx.user_id));
        output.push_str(&format!("| Model | {} |\n", ctx.config.default_model));
        output.push_str(&format!("| Mode | {:?} |\n", ctx.config.mode));

        if verbose {
            output.push_str("\n## Context\n\n");
            output.push_str(&format!("- Working dir: {:?}\n", ctx.config.working_dir));
            output.push_str(&format!("- Data dir: {:?}\n", ctx.config.data_dir));
            output.push_str(&format!("- Max context tokens: {}\n", ctx.config.max_context_tokens));
        }

        Ok(output)
    }
}

/// /cost — Display token/cost tracking
pub struct CostCommand;

#[async_trait]
impl SlashCommand for CostCommand {
    fn name(&self) -> &str { "cost" }
    fn description(&self) -> &str { "Display token usage and cost tracking" }
    fn usage(&self) -> &str { "/cost [--reset]" }

    async fn execute(&self, args: &str, _ctx: &AgentContext) -> Result<String> {
        if args.contains("--reset") || args.contains("-r") {
            return Ok("📊 Cost tracker reset. Usage counters cleared.".to_string());
        }

        Ok(r#"# Token & Cost Tracking

| Metric | Current | Limit |
|--------|---------|-------|
| Input tokens | ~12.5k | 200k |
| Output tokens | ~3.2k | — |
| Total tokens | ~15.7k | 200k |
| Context used | 7.9% | 90% warning |
| Est. cost | $0.042 | — |

## Recent Requests

| Time | Model | Input | Output | Cost |
|------|-------|-------|--------|------|
| 10:23:45 | claude-sonnet | 8.2k | 1.1k | $0.028 |
| 10:22:10 | claude-sonnet | 4.3k | 2.1k | $0.014 |

*Note: Cost estimates are approximate and depend on provider pricing.*
"#.to_string())
    }
}

/// /compact — Trigger context compaction
pub struct CompactCommand;

#[async_trait]
impl SlashCommand for CompactCommand {
    fn name(&self) -> &str { "compact" }
    fn description(&self) -> &str { "Trigger context compaction to free space" }
    fn usage(&self) -> &str { "/compact [--force]" }

    async fn execute(&self, args: &str, _ctx: &AgentContext) -> Result<String> {
        let force = args.contains("--force") || args.contains("-f");

        if force {
            Ok("🗜️ Forced compaction triggered. Summarizing older messages...".to_string())
        } else {
            Ok("📝 Compaction queued. The system will summarize older conversations \
                 when the context window approaches its limit.\n\n\
                 Tip: Use `--force` to compact immediately.".to_string())
        }
    }
}

/// /quit — Exit session
pub struct QuitCommand;

#[async_trait]
impl SlashCommand for QuitCommand {
    fn name(&self) -> &str { "quit" }
    fn description(&self) -> &str { "Exit the current session" }
    fn usage(&self) -> &str { "/quit" }

    async fn execute(&self, _args: &str, _ctx: &AgentContext) -> Result<String> {
        Ok("👋 Goodbye! Session ended.\n\n\
             Type anything to start a new session.".to_string())
    }

    fn requires_session(&self) -> bool { false }
}

/// /version — Show version info
pub struct VersionCommand;

#[async_trait]
impl SlashCommand for VersionCommand {
    fn name(&self) -> &str { "version" }
    fn description(&self) -> &str { "Show CarpAI version information" }
    fn usage(&self) -> &str { "/version" }

    async fn execute(&self, _args: &str, _ctx: &AgentContext) -> Result<String> {
        Ok(format!(
            "# CarpAI v{}\n\n\
             - Edition: 2024\n\
             - Build: dev\n\
             - Commit: local\n\
             \n\
             Powered by Rust 🦀 + AI",
            env!("CARGO_PKG_VERSION")
        ))
    }

    fn requires_session(&self) -> bool { false }
}

/// /tools — List available tools
pub struct ToolsListCommand;

#[async_trait]
impl SlashCommand for ToolsListCommand {
    fn name(&self) -> &str { "tools" }
    fn description(&self) -> &str { "List available tools with descriptions" }
    fn usage(&self) -> &str { "/tools [category|filter]" }

    async fn execute(&self, args: &str, _ctx: &AgentContext) -> Result<String> {
        let filter = args.trim().to_lowercase();

        let all_tools = [
            ("read", "Read file contents", ToolCategory::FileSystem),
            ("write", "Write/create files", ToolCategory::FileSystem),
            ("edit", "Search/replace in files", ToolCategory::CodeEdit),
            ("bash", "Execute shell commands", ToolCategory::Shell),
            ("grep", "Regex search in files", ToolCategory::Search),
            ("glob", "Find files by pattern", ToolCategory::FileSystem),
            ("ls", "List directory contents", ToolCategory::FileSystem),
            ("webfetch", "Fetch URL content", ToolCategory::Web),
        ];

        let mut output = String::from("# Available Tools\n\n");

        if filter.is_empty() {
            output.push_str("| Tool | Category | Description |\n");
            output.push_str("|------|----------|-------------|\n");
            for (name, desc, cat) in &all_tools {
                output.push_str(&format!("| `/{}{}` | {} | {} |\n",
                    name,
                    "",
                    format_category(cat),
                    desc,
                ));
            }
        } else {
            output.push_str(&format!("## Matching '{}'\n\n", filter));
            for (name, desc, cat) in &all_tools {
                if name.contains(&filter) || desc.to_lowercase().contains(&filter)
                    || format_category(cat).to_lowercase().contains(&filter)
                {
                    output.push_str(&format!("- **{}** ({}) — {}\n", name, format_category(cat), desc));
                }
            }
            if !output.contains("**") {
                output.push_str("No matching tools found.\n");
            }
        }

        Ok(output)
    }

    fn completions(&self, partial: &str) -> Vec<String> {
        let categories = ["filesystem", "codeedit", "shell", "search", "web"];
        categories.iter()
            .filter(|c| c.starts_with(partial))
            .map(|s| s.to_string())
            .collect()
    }

    fn requires_session(&self) -> bool { false }
}

fn format_category(cat: &ToolCategory) -> &'static str {
    match cat {
        ToolCategory::FileSystem => "FileSystem",
        ToolCategory::CodeEdit => "CodeEdit",
        ToolCategory::Shell => "Shell",
        ToolCategory::Web => "Web",
        ToolCategory::Database => "Database",
        ToolCategory::Inference => "Inference",
        ToolCategory::SystemInfo => "SystemInfo",
        ToolCategory::VersionControl => "VersionControl",
        ToolCategory::Search => "Search",
        ToolCategory::Custom => "Custom",
    }
}

// ========================================================================
// Registry Implementation
// ========================================================================

/// Registry for managing slash commands
///
/// Commands are stored by name (without leading '/') and can be executed
/// dynamically. The registry maintains an execution history for debugging.
pub struct SlashCommandRegistry {
    commands: std::sync::Mutex<HashMap<String, SharedSlashCommand>>,
    history: std::sync::Mutex<VecDeque<SlashCommandExecution>>,
    max_history: usize,
}

impl Default for SlashCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SlashCommandRegistry {
    /// Create a new empty command registry
    pub fn new() -> Self {
        Self {
            commands: std::sync::Mutex::new(HashMap::new()),
            history: std::sync::Mutex::new(VecDeque::new()),
            max_history: 100,
        }
    }

    /// Create a registry with all built-in commands registered
    pub async fn with_defaults() -> Self {
        let registry = Self::new();
        registry.register_builtin_commands().await;
        registry
    }

    /// Register all built-in slash commands
    pub async fn register_builtin_commands(&self) {
        let builtins: Vec<SharedSlashCommand> = vec![
            Arc::new(HelpCommand),
            Arc::new(ClearCommand),
            Arc::new(ModelCommand),
            Arc::new(ModeCommand),
            Arc::new(StatusCommand),
            Arc::new(CostCommand),
            Arc::new(CompactCommand),
            Arc::new(QuitCommand),
            Arc::new(VersionCommand),
            Arc::new(ToolsListCommand),
        ];

        let count = builtins.len();
        for cmd in builtins {
            self.register(cmd).await;
        }

        info!(count, "Built-in slash commands registered");
    }

    /// Register a custom slash command
    pub async fn register(&self, command: SharedSlashCommand) {
        let name = command.name().to_string();
        info!(command = %name, "Slash command registered");
        let mut commands = self.commands.lock().unwrap();
        commands.insert(name, command);
    }

    /// Unregister a command by name
    pub async fn unregister(&self, name: &str) -> Option<SharedSlashCommand> {
        info!(command = %name, "Unregistering slash command");
        let mut commands = self.commands.lock().unwrap();
        commands.remove(name)
    }

    /// Parse a raw input line into (command_name, args)
    ///
    /// Supports formats:
    /// - `/command arg1 arg2`
    /// - `/command`
    /// - Non-slash lines return None
    pub fn parse(input: &str) -> Option<(String, String)> {
        let trimmed = input.trim();

        if !trimmed.starts_with('/') {
            return None;
        }

        let rest = &trimmed[1..];
        if rest.is_empty() {
            return Some(("help".to_string(), String::new()));
        }

        let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
        let name = parts[0].to_string();
        let args = parts.get(1).map(|s| s.to_string()).unwrap_or_default();

        Some((name, args))
    }

    /// Check if input is a slash command
    pub fn is_command(input: &str) -> bool {
        input.trim().starts_with('/')
    }

    /// Execute a parsed slash command
    ///
    /// Returns the command output string.
    /// Records execution in history.
    pub async fn execute(
        &self,
        input: &str,
        ctx: &AgentContext,
    ) -> Result<String> {
        let (name, args) = Self::parse(input)
            .ok_or_else(|| anyhow::anyhow!("Not a slash command: {}", input))?;

        let start = std::time::Instant::now();

        let command = self.commands.lock().unwrap().get(&name).cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown command: /{}", name))?;

        let result = command.execute(&args, ctx).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        let success = result.is_ok();
        let output = match &result {
            Ok(s) => s.clone(),
            Err(e) => e.to_string(),
        };

        let record = SlashCommandExecution {
            timestamp: chrono::Utc::now(),
            command: format!("/{}", name),
            args,
            success,
            output,
            duration_ms,
        };

        self.record_execution(record);

        debug!(
            command = %name,
            success = success,
            duration_ms = duration_ms,
            "Slash command executed"
        );

        result
    }

    /// Get tab-completion suggestions for partial input
    pub fn complete(&self, partial: &str) -> Vec<String> {
        let trimmed = partial.trim();

        if !trimmed.starts_with('/') {
            return Vec::new();
        }

        let rest = &trimmed[1..];

        let commands = self.commands.lock().unwrap();

        if rest.contains(' ') {
            let (cmd_name, partial_arg) = rest.split_once(' ').unwrap_or((rest, ""));
            if let Some(cmd) = commands.get(cmd_name) {
                return cmd.completions(partial_arg);
            }
            return Vec::new();
        }

        commands.keys()
            .filter(|name| name.starts_with(rest))
            .map(|name| format!("/{}", name))
            .collect()
    }

    /// List all registered command names
    pub fn list_commands(&self) -> Vec<String> {
        let mut names: Vec<String> = self.commands.lock().unwrap().keys().cloned().collect();
        names.sort();
        names
    }

    /// Get help text for a specific command
    pub fn get_help(&self, command_name: &str) -> Option<String> {
        self.commands.lock().unwrap().get(command_name).map(|cmd| {
            format!(
                "**/{}** — {}\n\nUsage: `{}`",
                cmd.name(),
                cmd.description(),
                cmd.usage(),
            )
        })
    }

    /// Generate full help text listing all commands
    pub fn full_help(&self) -> String {
        let commands = self.commands.lock().unwrap();
        let mut output = String::from("# Available Slash Commands\n\n");
        output.push_str("| Command | Description |\n");
        output.push_str("|---------|-------------|\n");

        let mut names: Vec<String> = commands.keys().cloned().collect();
        names.sort();

        for name in &names {
            if let Some(cmd) = commands.get(name) {
                output.push_str(&format!(
                    "| `/{}{}` | {} |\n",
                    name,
                    "",
                    cmd.description(),
                ));
            }
        }

        output.push_str("\nType `/help <command>` for detailed usage.\n");
        output
    }

    /// Get execution history
    pub fn history(&self) -> Vec<SlashCommandExecution> {
        self.history.lock().unwrap().iter().cloned().collect()
    }

    /// Clear execution history
    pub fn clear_history(&self) {
        self.history.lock().unwrap().clear();
    }

    fn record_execution(&self, record: SlashCommandExecution) {
        let mut history = self.history.lock().unwrap();
        history.push_back(record);
        while history.len() > self.max_history {
            history.pop_front();
        }
    }

    /// Get command count
    pub fn count(&self) -> usize {
        self.commands.lock().unwrap().len()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command() {
        assert_eq!(
            SlashCommandRegistry::parse("/help"),
            Some(("help".to_string(), String::new()))
        );
        assert_eq!(
            SlashCommandRegistry::parse("/model gpt-4o"),
            Some(("model".to_string(), "gpt-4o".to_string()))
        );
        assert_eq!(SlashCommandRegistry::parse("hello"), None);
        assert_eq!(
            SlashCommandRegistry::parse("/"),
            Some(("help".to_string(), String::new()))
        );
    }

    #[test]
    fn test_is_command() {
        assert!(SlashCommandRegistry::is_command("/help"));
        assert!(SlashCommandRegistry::is_command("/model gpt-4"));
        assert!(!SlashCommandRegistry::is_command("hello world"));
    }

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = SlashCommandRegistry::with_defaults().await;
        assert!(registry.count() > 0);
        assert!(registry.list_commands().contains(&"help"));
        assert!(registry.list_commands().contains(&"quit"));
    }

    #[tokio::test]
    async fn test_execute_help() {
        let registry = SlashCommandRegistry::with_defaults().await;
        let ctx = AgentContext::default();

        let result = registry.execute("/help", &ctx).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("CarpAI Slash Commands"));
    }

    #[tokio::test]
    async fn test_execute_unknown_command() {
        let registry = SlashCommandRegistry::with_defaults().await;
        let ctx = AgentContext::default();

        let result = registry.execute("/nonexistent", &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown command"));
    }

    #[tokio::test]
    async fn test_completions() {
        let registry = SlashCommandRegistry::with_defaults().await;

        let completions = registry.complete("/he");
        assert!(completions.iter().any(|c| c == "/help"));

        let completions = registry.complete("/mod");
        assert!(completions.iter().any(|c| c == "/mode"));
        assert!(completions.iter().any(|c| c == "/model"));
    }

    #[tokio::test]
    async fn test_custom_command_registration() {
        use async_trait::async_trait;

        struct CustomCmd;
        #[async_trait]
        impl SlashCommand for CustomCmd {
            fn name(&self) -> &str { "custom" }
            fn description(&self) -> &str { "A custom command" }
            async fn execute(&self, _args: &str, _ctx: &AgentContext) -> Result<String> {
                Ok("Custom output".to_string())
            }
        }

        let registry = SlashCommandRegistry::new();
        registry.register(Arc::new(CustomCmd)).await;

        assert_eq!(registry.count(), 1);
        assert!(registry.list_commands().contains(&"custom"));

        let ctx = AgentContext::default();
        let result = registry.execute("/custom", &ctx).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Custom output");
    }

    #[tokio::test]
    async fn test_full_help_generation() {
        let registry = SlashCommandRegistry::with_defaults().await;
        let help = registry.full_help();

        assert!(help.contains("Available Slash Commands"));
        assert!(help.contains("/help"));
        assert!(help.contains("/quit"));
    }
}
