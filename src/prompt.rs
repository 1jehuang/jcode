//! System prompt management

use std::path::Path;
use std::process::Command;

/// Default system prompt for jcode (embedded at compile time)
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("prompt/system.txt");

/// Split system prompt for efficient caching
/// Static content is cached, dynamic content is not
#[derive(Debug, Clone, Default)]
pub struct SplitSystemPrompt {
    /// Static content that should be cached (CLAUDE.md, base prompt, skills)
    pub static_part: String,
    /// Dynamic content that changes frequently (date, git status, memory)
    pub dynamic_part: String,
}

/// Skill info for system prompt
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

/// Information about what's loaded in the context window
#[derive(Debug, Clone, Default)]
pub struct ContextInfo {
    // === Static (System Prompt) ===
    /// Base system prompt size (chars)
    pub system_prompt_chars: usize,
    /// Environment context size (chars)
    pub env_context_chars: usize,
    /// Whether project AGENTS.md was loaded
    pub has_project_agents_md: bool,
    /// Project AGENTS.md size (chars)
    pub project_agents_md_chars: usize,
    /// Whether project CLAUDE.md was loaded
    pub has_project_claude_md: bool,
    /// Project CLAUDE.md size (chars)
    pub project_claude_md_chars: usize,
    /// Whether global ~/.AGENTS.md was loaded
    pub has_global_agents_md: bool,
    /// Global AGENTS.md size (chars)
    pub global_agents_md_chars: usize,
    /// Whether global ~/.CLAUDE.md was loaded
    pub has_global_claude_md: bool,
    /// Global CLAUDE.md size (chars)
    pub global_claude_md_chars: usize,
    /// Skills section size (chars)
    pub skills_chars: usize,
    /// Self-dev section size (chars)
    pub selfdev_chars: usize,
    /// Memory section size (chars)
    pub memory_chars: usize,

    // === Dynamic (Conversation) ===
    /// Tool definitions sent to API (chars)
    pub tool_defs_chars: usize,
    /// Number of tool definitions
    pub tool_defs_count: usize,
    /// User messages total size (chars)
    pub user_messages_chars: usize,
    /// Number of user messages
    pub user_messages_count: usize,
    /// Assistant messages total size (chars)
    pub assistant_messages_chars: usize,
    /// Number of assistant messages
    pub assistant_messages_count: usize,
    /// Tool calls size (chars)
    pub tool_calls_chars: usize,
    /// Number of tool calls
    pub tool_calls_count: usize,
    /// Tool results size (chars)
    pub tool_results_chars: usize,
    /// Number of tool results
    pub tool_results_count: usize,

    /// Total system prompt size (chars)
    pub total_chars: usize,
}

impl ContextInfo {
    /// Rough estimate of tokens (chars / 4 is a common approximation)
    pub fn estimated_tokens(&self) -> usize {
        self.total_chars / 4
    }

    /// Get breakdown as (label, chars, icon) tuples for display
    pub fn breakdown(&self) -> Vec<(&'static str, usize, &'static str)> {
        let mut parts = vec![
            ("sys", self.system_prompt_chars, "⚙"),
            ("env", self.env_context_chars, "🌍"),
        ];
        if self.has_project_agents_md {
            parts.push(("agents", self.project_agents_md_chars, "📋"));
        }
        if self.has_project_claude_md {
            parts.push(("claude", self.project_claude_md_chars, "📝"));
        }
        if self.has_global_agents_md {
            parts.push(("~agents", self.global_agents_md_chars, "📋"));
        }
        if self.has_global_claude_md {
            parts.push(("~claude", self.global_claude_md_chars, "📝"));
        }
        if self.skills_chars > 0 {
            parts.push(("skills", self.skills_chars, "🔧"));
        }
        if self.selfdev_chars > 0 {
            parts.push(("dev", self.selfdev_chars, "🛠"));
        }
        if self.memory_chars > 0 {
            parts.push(("mem", self.memory_chars, "🧠"));
        }
        parts
    }
}

/// Build the full system prompt with dynamic context
pub fn build_system_prompt(skill_prompt: Option<&str>, available_skills: &[SkillInfo]) -> String {
    build_system_prompt_with_selfdev(skill_prompt, available_skills, false)
}

/// Build the full system prompt with optional self-dev tools
pub fn build_system_prompt_with_selfdev(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
) -> String {
    let (prompt, _) = build_system_prompt_with_context(skill_prompt, available_skills, is_selfdev);
    prompt
}

/// Build the full system prompt and return context info about what was loaded
pub fn build_system_prompt_with_context(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
) -> (String, ContextInfo) {
    build_system_prompt_with_context_and_memory(skill_prompt, available_skills, is_selfdev, None)
}

/// Build the full system prompt with optional memory section and return context info
pub fn build_system_prompt_with_context_and_memory(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
    memory_prompt: Option<&str>,
) -> (String, ContextInfo) {
    build_system_prompt_full(skill_prompt, available_skills, is_selfdev, memory_prompt, None)
}

/// Build the full system prompt with working directory support for loading context files
pub fn build_system_prompt_full(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
    memory_prompt: Option<&str>,
    working_dir: Option<&Path>,
) -> (String, ContextInfo) {
    let mut parts = vec![DEFAULT_SYSTEM_PROMPT.to_string()];
    let mut info = ContextInfo::default();

    info.system_prompt_chars = DEFAULT_SYSTEM_PROMPT.len();

    // Add environment context
    if let Some(env_context) = build_env_context() {
        info.env_context_chars = env_context.len();
        parts.push(env_context);
    }

    // Add self-dev tools section when in canary mode
    if is_selfdev {
        let selfdev_prompt = build_selfdev_prompt();
        info.selfdev_chars = selfdev_prompt.len();
        parts.push(selfdev_prompt);
    }

    // Add AGENTS.md and CLAUDE.md instructions with tracking (from working_dir or cwd)
    let (md_content, md_info) = load_claude_md_files_from_dir(working_dir);
    if let Some(content) = md_content {
        parts.push(content);
    }
    // Merge file info
    info.has_project_agents_md = md_info.has_project_agents_md;
    info.project_agents_md_chars = md_info.project_agents_md_chars;
    info.has_project_claude_md = md_info.has_project_claude_md;
    info.project_claude_md_chars = md_info.project_claude_md_chars;
    info.has_global_agents_md = md_info.has_global_agents_md;
    info.global_agents_md_chars = md_info.global_agents_md_chars;
    info.has_global_claude_md = md_info.has_global_claude_md;
    info.global_claude_md_chars = md_info.global_claude_md_chars;

    if let Some(memory) = memory_prompt {
        info.memory_chars = memory.len();
        parts.push(memory.to_string());
    }

    // Add available skills list
    if !available_skills.is_empty() {
        let mut skills_section = "# Available Skills\n\nYou have access to the following skills that the user can invoke with `/skillname`:\n".to_string();
        for skill in available_skills {
            skills_section.push_str(&format!("\n- `/{} ` - {}", skill.name, skill.description));
        }
        skills_section.push_str(
            "\n\nWhen a user asks about available skills or capabilities, mention these skills.",
        );
        info.skills_chars = skills_section.len();
        parts.push(skills_section);
    }

    // Add active skill prompt
    if let Some(skill) = skill_prompt {
        parts.push(format!("# Active Skill\n\n{}", skill));
    }

    let prompt = parts.join("\n\n");
    info.total_chars = prompt.len();

    (prompt, info)
}

/// Build system prompt split into static (cacheable) and dynamic parts
/// This improves cache hit rate by keeping frequently-changing content separate
pub fn build_system_prompt_split(
    skill_prompt: Option<&str>,
    available_skills: &[SkillInfo],
    is_selfdev: bool,
    memory_prompt: Option<&str>,
    working_dir: Option<&Path>,
) -> (SplitSystemPrompt, ContextInfo) {
    let mut static_parts = vec![DEFAULT_SYSTEM_PROMPT.to_string()];
    let mut dynamic_parts = Vec::new();
    let mut info = ContextInfo::default();

    info.system_prompt_chars = DEFAULT_SYSTEM_PROMPT.len();

    // === STATIC CONTENT (cacheable) ===

    // Add self-dev tools section (static, without dynamic socket path)
    if is_selfdev {
        let selfdev_prompt = build_selfdev_prompt_static();
        info.selfdev_chars = selfdev_prompt.len();
        static_parts.push(selfdev_prompt);
    }

    // Add AGENTS.md and CLAUDE.md instructions (static per project)
    let (md_content, md_info) = load_claude_md_files_from_dir(working_dir);
    if let Some(content) = md_content {
        static_parts.push(content);
    }
    info.has_project_agents_md = md_info.has_project_agents_md;
    info.project_agents_md_chars = md_info.project_agents_md_chars;
    info.has_project_claude_md = md_info.has_project_claude_md;
    info.project_claude_md_chars = md_info.project_claude_md_chars;
    info.has_global_agents_md = md_info.has_global_agents_md;
    info.global_agents_md_chars = md_info.global_agents_md_chars;
    info.has_global_claude_md = md_info.has_global_claude_md;
    info.global_claude_md_chars = md_info.global_claude_md_chars;

    // Add available skills list (fairly static)
    if !available_skills.is_empty() {
        let mut skills_section = "# Available Skills\n\nYou have access to the following skills that the user can invoke with `/skillname`:\n".to_string();
        for skill in available_skills {
            skills_section.push_str(&format!("\n- `/{} ` - {}", skill.name, skill.description));
        }
        skills_section.push_str(
            "\n\nWhen a user asks about available skills or capabilities, mention these skills.",
        );
        info.skills_chars = skills_section.len();
        static_parts.push(skills_section);
    }

    // === DYNAMIC CONTENT (not cached) ===

    // Environment context (date, cwd, git status) - changes frequently
    if let Some(env_context) = build_env_context() {
        info.env_context_chars = env_context.len();
        dynamic_parts.push(env_context);
    }

    // Memory prompt (changes per conversation)
    if let Some(memory) = memory_prompt {
        info.memory_chars = memory.len();
        dynamic_parts.push(memory.to_string());
    }

    // Active skill prompt (changes per skill invocation)
    if let Some(skill) = skill_prompt {
        dynamic_parts.push(format!("# Active Skill\n\n{}", skill));
    }

    let static_part = static_parts.join("\n\n");
    let dynamic_part = dynamic_parts.join("\n\n");
    info.total_chars = static_part.len() + dynamic_part.len();

    (
        SplitSystemPrompt {
            static_part,
            dynamic_part,
        },
        info,
    )
}

/// Build self-dev tools prompt section (static version without dynamic socket path)
fn build_selfdev_prompt_static() -> String {
    r#"# Self-Development Mode

You are running in self-dev mode, working on the jcode codebase itself. You have access to additional tools:

## selfdev Tool

Use this tool to manage builds and get debug socket info:

```json
{"name": "selfdev", "input": {"action": "reload"}}      // Restart with current binary
{"name": "selfdev", "input": {"action": "status"}}      // Show build versions, debug socket path
{"name": "selfdev", "input": {"action": "promote"}}     // Mark current as stable
{"name": "selfdev", "input": {"action": "rollback"}}    // Switch back to stable build
{"name": "selfdev", "input": {"action": "socket-info"}} // Debug socket connection info
{"name": "selfdev", "input": {"action": "socket-help"}} // Debug socket command reference
```

## debug_socket Tool

Use this tool to send commands to the debug socket for visual debugging, spawning test instances, or inspecting agent state:

```json
{"name": "debug_socket", "input": {"command": "client:frame"}}      // Get visual debug frame
{"name": "debug_socket", "input": {"command": "client:enable"}}     // Enable visual debug
{"name": "debug_socket", "input": {"command": "tester:spawn"}}      // Spawn test instance
{"name": "debug_socket", "input": {"command": "tester:list"}}       // List active testers
{"name": "debug_socket", "input": {"command": "state"}}             // Get agent state
{"name": "debug_socket", "input": {"command": "help"}}              // Full command list
```

## Workflow

When you make code changes to jcode:
1. Build with `cargo build --release`
2. Use `selfdev` with action `reload` to restart with the new binary
3. The session continues automatically after restart

For testing UI changes, use the debug_socket tool to spawn testers and capture visual debug frames."#.to_string()
}

/// Build self-dev tools prompt section
fn build_selfdev_prompt() -> String {
    let debug_socket_path = crate::server::debug_socket_path();

    format!(
        r#"# Self-Development Mode

You are running in self-dev mode, working on the jcode codebase itself. You have access to additional tools:

## selfdev Tool

Use this tool to manage builds and get debug socket info:

```json
{{"name": "selfdev", "input": {{"action": "reload"}}}}      // Restart with current binary
{{"name": "selfdev", "input": {{"action": "status"}}}}      // Show build versions, debug socket path
{{"name": "selfdev", "input": {{"action": "promote"}}}}     // Mark current as stable
{{"name": "selfdev", "input": {{"action": "rollback"}}}}    // Switch back to stable build
{{"name": "selfdev", "input": {{"action": "socket-info"}}}} // Debug socket connection info
{{"name": "selfdev", "input": {{"action": "socket-help"}}}} // Debug socket command reference
```

## debug_socket Tool

Use this tool to send commands to the debug socket for visual debugging, spawning test instances, or inspecting agent state:

```json
{{"name": "debug_socket", "input": {{"command": "client:frame"}}}}      // Get visual debug frame
{{"name": "debug_socket", "input": {{"command": "client:enable"}}}}     // Enable visual debug
{{"name": "debug_socket", "input": {{"command": "tester:spawn"}}}}      // Spawn test instance
{{"name": "debug_socket", "input": {{"command": "tester:list"}}}}       // List active testers
{{"name": "debug_socket", "input": {{"command": "state"}}}}             // Get agent state
{{"name": "debug_socket", "input": {{"command": "help"}}}}              // Full command list
```

Debug socket path: {}

## Workflow

When you make code changes to jcode:
1. Build with `cargo build --release`
2. Use `selfdev` with action `reload` to restart with the new binary
3. The session continues automatically after restart

For testing UI changes, use the debug_socket tool to spawn testers and capture visual debug frames."#,
        debug_socket_path.display()
    )
}

/// Build environment context (date, cwd, git status)
fn build_env_context() -> Option<String> {
    let mut lines = vec!["# Environment".to_string()];

    // Current date
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    lines.push(format!("Date: {}", date));

    // Working directory
    if let Ok(cwd) = std::env::current_dir() {
        lines.push(format!("Working directory: {}", cwd.display()));
    }

    // Git info
    if let Some(git_info) = get_git_info() {
        lines.push(git_info);
    }

    Some(lines.join("\n"))
}

/// Get git branch and status summary
fn get_git_info() -> Option<String> {
    // Check if we're in a git repo
    let in_repo = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .ok()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !in_repo {
        return None;
    }

    let mut info = vec!["Git:".to_string()];

    // Current branch
    if let Ok(output) = Command::new("git")
        .args(["branch", "--show-current"])
        .output()
    {
        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !branch.is_empty() {
                info.push(format!("  Branch: {}", branch));
            }
        }
    }

    // Short status (modified files count)
    if let Ok(output) = Command::new("git").args(["status", "--porcelain"]).output() {
        if output.status.success() {
            let status = String::from_utf8_lossy(&output.stdout);
            let modified: Vec<&str> = status.lines().take(5).collect();
            if !modified.is_empty() {
                info.push(format!("  Modified: {} files", status.lines().count()));
                for file in modified {
                    info.push(format!("    {}", file));
                }
                if status.lines().count() > 5 {
                    info.push("    ...".to_string());
                }
            }
        }
    }

    if info.len() > 1 {
        Some(info.join("\n"))
    } else {
        None
    }
}

/// Load AGENTS.md and CLAUDE.md files from project and home directory
/// Order: AGENTS.md (generic) first, then CLAUDE.md (Claude-specific overrides)
fn load_claude_md_files() -> Option<String> {
    let (content, _) = load_claude_md_files_with_info();
    content
}

/// Load AGENTS.md and CLAUDE.md files with tracking info
fn load_claude_md_files_with_info() -> (Option<String>, ContextInfo) {
    load_claude_md_files_from_dir(None)
}

/// Load AGENTS.md and CLAUDE.md files from a specific working directory
pub fn load_claude_md_files_from_dir(working_dir: Option<&Path>) -> (Option<String>, ContextInfo) {
    let mut contents = vec![];
    let mut info = ContextInfo::default();

    // Helper to load a file if it exists, returns (formatted_content, raw_size)
    let load_file = |path: &Path, label: &str| -> Option<(String, usize)> {
        if path.exists() {
            std::fs::read_to_string(path).ok().map(|content| {
                let raw_size = content.len();
                let formatted = format!("# {}\n\n{}", label, content.trim());
                (formatted, raw_size)
            })
        } else {
            None
        }
    };

    // Project-level files (from specified working directory or current directory)
    // AGENTS.md first (generic), then CLAUDE.md (Claude-specific)
    let project_dir = working_dir.unwrap_or(Path::new("."));
    if let Some((content, size)) =
        load_file(&project_dir.join("AGENTS.md"), "Project Instructions (AGENTS.md)")
    {
        info.has_project_agents_md = true;
        info.project_agents_md_chars = size;
        contents.push(content);
    }
    if let Some((content, size)) =
        load_file(&project_dir.join("CLAUDE.md"), "Project Instructions (CLAUDE.md)")
    {
        info.has_project_claude_md = true;
        info.project_claude_md_chars = size;
        contents.push(content);
    }

    // Home directory files
    if let Some(home) = dirs::home_dir() {
        if let Some((content, size)) = load_file(
            &home.join("AGENTS.md"),
            "Global Instructions (~/.AGENTS.md)",
        ) {
            info.has_global_agents_md = true;
            info.global_agents_md_chars = size;
            contents.push(content);
        }
        if let Some((content, size)) = load_file(
            &home.join("CLAUDE.md"),
            "Global Instructions (~/.CLAUDE.md)",
        ) {
            info.has_global_claude_md = true;
            info.global_claude_md_chars = size;
            contents.push(content);
        }
    }

    if contents.is_empty() {
        (None, info)
    } else {
        (Some(contents.join("\n\n")), info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the default system prompt does NOT identify as "Claude Code"
    /// It's fine to say "powered by Claude" but not "Claude Code" (Anthropic's product)
    #[test]
    fn test_default_system_prompt_no_claude_code_identity() {
        let prompt = DEFAULT_SYSTEM_PROMPT.to_lowercase();

        assert!(
            !prompt.contains("claude code"),
            "DEFAULT_SYSTEM_PROMPT should NOT identify as 'Claude Code'. Found in system.txt"
        );
        assert!(
            !prompt.contains("claude-code"),
            "DEFAULT_SYSTEM_PROMPT should NOT contain 'claude-code'. Found in system.txt"
        );
    }

    /// Verify skill prompts don't accidentally introduce "Claude Code" identity
    #[test]
    fn test_skill_prompt_integration() {
        // Test that a skill prompt is properly appended and doesn't break anything
        let skill_prompt = "You are helping with a debugging task.";
        let prompt = build_system_prompt(Some(skill_prompt), &[]);

        // The prompt should contain our default system prompt
        assert!(prompt.contains("jcode, an independent AI coding agent"));

        // The prompt should contain the skill prompt
        assert!(prompt.contains(skill_prompt));

        // The base prompt parts (excluding user CLAUDE.md files) should NOT contain "Claude Code"
        // We check DEFAULT_SYSTEM_PROMPT separately since user files may legitimately contain it
        let default_lower = DEFAULT_SYSTEM_PROMPT.to_lowercase();
        assert!(
            !default_lower.contains("claude code"),
            "DEFAULT_SYSTEM_PROMPT should NOT identify as 'Claude Code'"
        );
    }
}
