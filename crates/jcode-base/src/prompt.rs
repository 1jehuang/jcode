//! System prompt management

use std::path::{Path, PathBuf};
use std::process::Command;

/// Default system prompt for jcode (embedded at compile time)
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("prompt/system_prompt.md");
/// Mission-continuation template (embedded at compile time). Consumed by the
/// `mission` module in the upper `jcode-app-core` layer; the asset lives here
/// alongside the other prompt templates.
pub const MISSION_CONTINUATION_TEMPLATE: &str = include_str!("prompt/mission_continuation.md");
const SELFDEV_HINT_PROMPT: &str = include_str!("prompt/selfdev_hint.txt");
const SELFDEV_MODE_PROMPT: &str = include_str!("prompt/selfdev_mode.txt");
const SELFDEV_FOCUS_TUI_PROMPT: &str = include_str!("prompt/selfdev_focus_tui.txt");
const SELFDEV_FOCUS_DESKTOP_PROMPT: &str = include_str!("prompt/selfdev_focus_desktop.txt");
/// Final todo-confidence guidance when validation is still needed.
pub const TODO_CONFIDENCE_NEEDS_VALIDATION_PROMPT: &str =
    include_str!("prompt/todo_confidence_needs_validation.txt");
/// Final todo-confidence guidance when completion confidence is sufficient.
pub const TODO_CONFIDENCE_READY_PROMPT: &str = include_str!("prompt/todo_confidence_ready.txt");

/// Split system prompt for efficient caching
/// Static content is cached, dynamic content is not
#[derive(Debug, Clone, Default)]
pub struct SplitSystemPrompt {
    /// Static content that should be cached (instruction files, base prompt, skills)
    pub static_part: String,
    /// Dynamic turn context that changes per request (memory, active skill, reminders)
    pub dynamic_part: String,
}

impl SplitSystemPrompt {
    pub fn chars(&self) -> usize {
        match (self.static_part.is_empty(), self.dynamic_part.is_empty()) {
            (true, true) => 0,
            (false, true) => self.static_part.len(),
            (true, false) => self.dynamic_part.len(),
            (false, false) => self.static_part.len() + 2 + self.dynamic_part.len(),
        }
    }

    pub fn estimated_tokens(&self) -> usize {
        crate::util::estimate_tokens(&if self.static_part.is_empty() {
            self.dynamic_part.clone()
        } else if self.dynamic_part.is_empty() {
            self.static_part.clone()
        } else {
            format!("{}\n\n{}", self.static_part, self.dynamic_part)
        })
    }
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
    /// Immutable session context size (chars), when persisted in transcript history.
    pub session_context_chars: usize,
    /// Whether project AGENTS.md was loaded
    pub has_project_agents_md: bool,
    /// Project AGENTS.md size (chars)
    pub project_agents_md_chars: usize,
    /// Whether global ~/.AGENTS.md was loaded
    pub has_global_agents_md: bool,
    /// Global AGENTS.md size (chars)
    pub global_agents_md_chars: usize,
    /// Skills section size (chars)
    pub skills_chars: usize,
    /// Self-dev section size (chars)
    pub selfdev_chars: usize,
    /// Memory section size (chars)
    pub memory_chars: usize,
    /// Prompt overlay section size (chars)
    pub prompt_overlay_chars: usize,
    /// Preferred tools section size (chars)
    pub preferred_tools_chars: usize,

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

    pub fn prompt_prefix_chars(&self) -> usize {
        self.system_prompt_chars
            + self.session_context_chars
            + self.project_agents_md_chars
            + self.global_agents_md_chars
            + self.skills_chars
            + self.selfdev_chars
            + self.memory_chars
            + self.prompt_overlay_chars
            + self.preferred_tools_chars
            + self.tool_defs_chars
    }

    pub fn prompt_prefix_tokens(&self) -> usize {
        self.prompt_prefix_chars() / 4
    }

    pub fn tool_definition_tokens(&self) -> usize {
        self.tool_defs_chars / 4
    }

    /// Get breakdown as (label, chars, icon) tuples for display
    pub fn breakdown(&self) -> Vec<(&'static str, usize, &'static str)> {
        let mut parts = vec![
            ("sys", self.system_prompt_chars, "⚙"),
            ("session", self.session_context_chars, "🌍"),
        ];
        if self.has_project_agents_md {
            parts.push(("agents", self.project_agents_md_chars, "📋"));
        }
        if self.has_global_agents_md {
            parts.push(("~agents", self.global_agents_md_chars, "📋"));
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
        if self.prompt_overlay_chars > 0 {
            parts.push(("overlay", self.prompt_overlay_chars, "🧩"));
        }
        if self.preferred_tools_chars > 0 {
            parts.push(("tools", self.preferred_tools_chars, "🧰"));
        }
        parts
    }
}

/// Build the full system prompt with static context.
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
    build_system_prompt_full(
        skill_prompt,
        available_skills,
        is_selfdev,
        memory_prompt,
        None,
    )
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
    let mut info = ContextInfo {
        system_prompt_chars: DEFAULT_SYSTEM_PROMPT.len(),
        ..Default::default()
    };

    // Add self-dev guidance. Full workflow instructions are only included for
    // active self-dev sessions; other sessions get a lightweight hint.
    if is_selfdev {
        let selfdev_prompt = build_selfdev_prompt_for_working_dir(working_dir);
        info.selfdev_chars = selfdev_prompt.len();
        parts.push(selfdev_prompt);
    } else {
        parts.push(build_selfdev_hint_prompt());
    }

    // Add AGENTS.md instructions with tracking (from working_dir or cwd)
    let (md_content, md_info) = load_agents_md_files_from_dir(working_dir);
    if let Some(content) = md_content {
        parts.push(content);
    }
    // Merge file info
    info.has_project_agents_md = md_info.has_project_agents_md;
    info.project_agents_md_chars = md_info.project_agents_md_chars;
    info.has_global_agents_md = md_info.has_global_agents_md;
    info.global_agents_md_chars = md_info.global_agents_md_chars;

    // Add optional prompt overlays from ~/.jcode/ and ./.jcode/
    let (overlay_content, overlay_chars) = load_prompt_overlay_files_from_dir(working_dir);
    if let Some(content) = overlay_content {
        info.prompt_overlay_chars = overlay_chars;
        parts.push(content);
    }

    // Add optional preferred-tool guidance from ~/.jcode/ and ./.jcode/
    let (preferred_tools_content, preferred_tools_chars) =
        load_preferred_tools_files_from_dir(working_dir);
    if let Some(content) = preferred_tools_content {
        info.preferred_tools_chars = preferred_tools_chars;
        parts.push(content);
    }

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
    let mut info = ContextInfo {
        system_prompt_chars: DEFAULT_SYSTEM_PROMPT.len(),
        ..Default::default()
    };

    // === STATIC CONTENT (cacheable) ===

    // Add self-dev guidance. Full workflow instructions are only included for
    // active self-dev sessions; other sessions get a lightweight hint.
    if is_selfdev {
        let selfdev_prompt = build_selfdev_prompt_static_for_working_dir(working_dir);
        info.selfdev_chars = selfdev_prompt.len();
        static_parts.push(selfdev_prompt);
    } else {
        static_parts.push(build_selfdev_hint_prompt());
    }

    // Add AGENTS.md instructions (static per project)
    let (md_content, md_info) = load_agents_md_files_from_dir(working_dir);
    if let Some(content) = md_content {
        static_parts.push(content);
    }
    info.has_project_agents_md = md_info.has_project_agents_md;
    info.project_agents_md_chars = md_info.project_agents_md_chars;
    info.has_global_agents_md = md_info.has_global_agents_md;
    info.global_agents_md_chars = md_info.global_agents_md_chars;

    // Add optional prompt overlays from ~/.jcode/ and ./.jcode/
    let (overlay_content, overlay_chars) = load_prompt_overlay_files_from_dir(working_dir);
    if let Some(content) = overlay_content {
        info.prompt_overlay_chars = overlay_chars;
        static_parts.push(content);
    }

    // Add optional preferred-tool guidance (static per project/user)
    let (preferred_tools_content, preferred_tools_chars) =
        load_preferred_tools_files_from_dir(working_dir);
    if let Some(content) = preferred_tools_content {
        info.preferred_tools_chars = preferred_tools_chars;
        static_parts.push(content);
    }

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

    // === TURN CONTEXT (not cached) ===

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
fn build_selfdev_hint_prompt() -> String {
    SELFDEV_HINT_PROMPT.to_string()
}

/// Build self-dev tools prompt section (static version without dynamic socket path)
fn build_selfdev_prompt_static() -> String {
    build_selfdev_prompt_static_for_context(SelfDevProductContext::Tui)
}

/// Build self-dev tools prompt section
fn build_selfdev_prompt() -> String {
    build_selfdev_prompt_for_context(SelfDevProductContext::Tui)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelfDevProductContext {
    Tui,
    Desktop,
}

impl SelfDevProductContext {
    fn from_working_dir(working_dir: Option<&Path>) -> Self {
        let Some(working_dir) = working_dir else {
            return Self::Tui;
        };

        let path = working_dir.to_string_lossy().replace('\\', "/");
        if path.contains("/crates/jcode-desktop") || path.ends_with("crates/jcode-desktop") {
            Self::Desktop
        } else {
            Self::Tui
        }
    }

    fn prompt_block(self) -> &'static str {
        match self {
            Self::Tui => SELFDEV_FOCUS_TUI_PROMPT,
            Self::Desktop => SELFDEV_FOCUS_DESKTOP_PROMPT,
        }
    }
}

fn build_selfdev_prompt_static_for_working_dir(working_dir: Option<&Path>) -> String {
    build_selfdev_prompt_static_for_context(SelfDevProductContext::from_working_dir(working_dir))
}

fn build_selfdev_prompt_for_working_dir(working_dir: Option<&Path>) -> String {
    build_selfdev_prompt_for_context(SelfDevProductContext::from_working_dir(working_dir))
}

fn build_selfdev_prompt_static_for_context(context: SelfDevProductContext) -> String {
    build_selfdev_prompt_for_context(context).replace("__DEBUG_SOCKET_BLOCK__\n\n", "")
}

fn build_selfdev_prompt_for_context(context: SelfDevProductContext) -> String {
    SELFDEV_MODE_PROMPT.replace("__SELFDEV_PRODUCT_FOCUS__", context.prompt_block())
}

/// Build immutable session context captured once per session.
pub fn build_session_context(working_dir: Option<&Path>) -> String {
    let mut lines = vec!["# Session Context".to_string()];

    let now_utc = chrono::Utc::now();
    lines.push(format!("Date: {}", now_utc.format("%Y-%m-%d")));
    lines.push(format!("Time: {} UTC", now_utc.format("%H:%M:%S")));
    lines.push("Timezone: UTC".to_string());
    lines.push(format!("OS: {}", std::env::consts::OS));
    lines.push(format!("Architecture: {}", std::env::consts::ARCH));
    lines.push(format!(
        "Jcode version: {} ({})",
        jcode_build_meta::VERSION,
        jcode_build_meta::GIT_HASH
    ));

    if let Some(hardware) = hardware_context() {
        lines.push(hardware);
    }

    let cwd = working_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    if let Some(cwd) = cwd.as_ref() {
        lines.push(format!("Working directory: {}", cwd.display()));
    }

    if let Some(git_info) = get_git_info(cwd.as_deref()) {
        lines.push(git_info);
    }

    lines.join("\n")
}

/// Get git branch and status summary
fn get_git_info(working_dir: Option<&Path>) -> Option<String> {
    let mut command = Command::new("git");
    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }
    // Check if we're in a git repo
    let in_repo = command
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
    let mut branch_command = Command::new("git");
    if let Some(dir) = working_dir {
        branch_command.current_dir(dir);
    }
    if let Ok(output) = branch_command.args(["branch", "--show-current"]).output()
        && output.status.success()
    {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            info.push(format!("  Branch: {}", branch));
        }
    }

    // Short status (modified files count)
    let mut status_command = Command::new("git");
    if let Some(dir) = working_dir {
        status_command.current_dir(dir);
    }
    if let Ok(output) = status_command.args(["status", "--porcelain"]).output()
        && output.status.success()
    {
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

    if info.len() > 1 {
        Some(info.join("\n"))
    } else {
        None
    }
}

fn hardware_context() -> Option<String> {
    let mut lines = Vec::new();

    if let Some(machine) = machine_model() {
        lines.push(format!("  Machine: {}", machine));
    }
    if let Some(cpu) = cpu_model() {
        lines.push(format!("  CPU: {}", cpu));
    }
    if let Some(gpu) = gpu_summary() {
        lines.push(format!("  GPU: {}", gpu));
    }
    if let Some(memory) = memory_summary() {
        lines.push(format!("  Memory: {}", memory));
    }

    if lines.is_empty() {
        None
    } else {
        let mut out = vec!["Hardware:".to_string()];
        out.extend(lines);
        Some(out.join("\n"))
    }
}

fn read_trimmed_file(path: impl Into<PathBuf>) -> Option<String> {
    std::fs::read_to_string(path.into())
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn machine_model() -> Option<String> {
    let vendor = read_trimmed_file("/sys/devices/virtual/dmi/id/sys_vendor");
    let product = read_trimmed_file("/sys/devices/virtual/dmi/id/product_name");
    match (vendor, product) {
        (Some(vendor), Some(product)) if product.contains(&vendor) => Some(product),
        (Some(vendor), Some(product)) => Some(format!("{} {}", vendor, product)),
        (None, Some(product)) => Some(product),
        (Some(vendor), None) => Some(vendor),
        (None, None) => None,
    }
}

fn cpu_model() -> Option<String> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    cpuinfo.lines().find_map(|line| {
        let (_, value) = line.split_once(':')?;
        if line.trim_start().starts_with("model name") {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        } else {
            None
        }
    })
}

fn memory_summary() -> Option<String> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?.trim();
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })?;
    let gib = kb as f64 / 1024.0 / 1024.0;
    Some(format!("{:.1} GiB", gib))
}

fn gpu_summary() -> Option<String> {
    let output = Command::new("lspci").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut gpus: Vec<String> = text
        .lines()
        .filter(|line| {
            line.contains(" VGA compatible controller")
                || line.contains(" 3D controller")
                || line.contains(" Display controller")
        })
        .filter_map(|line| {
            line.split_once(':')
                .map(|(_, rest)| rest.trim().to_string())
        })
        .collect();
    gpus.dedup();
    if gpus.is_empty() {
        None
    } else {
        Some(gpus.join("; "))
    }
}

/// Load AGENTS.md files from a specific working directory
pub fn load_agents_md_files_from_dir(working_dir: Option<&Path>) -> (Option<String>, ContextInfo) {
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
    let project_dir = working_dir.unwrap_or(Path::new("."));
    if let Some((content, size)) = load_file(
        &project_dir.join("AGENTS.md"),
        "Project Instructions (AGENTS.md)",
    ) {
        info.has_project_agents_md = true;
        info.project_agents_md_chars = size;
        contents.push(content);
    }

    // Home directory files
    if let Ok(global_agents_md) = crate::storage::user_home_path("AGENTS.md")
        && let Some((content, size)) =
            load_file(&global_agents_md, "Global Instructions (~/.AGENTS.md)")
    {
        info.has_global_agents_md = true;
        info.global_agents_md_chars = size;
        contents.push(content);
    }

    if contents.is_empty() {
        (None, info)
    } else {
        (Some(contents.join("\n\n")), info)
    }
}

/// Load optional prompt overlay markdown from ~/.jcode/ and ./.jcode/
fn load_prompt_overlay_files_from_dir(working_dir: Option<&Path>) -> (Option<String>, usize) {
    let mut contents = vec![];
    let mut total_chars = 0usize;

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

    let project_dir = working_dir.unwrap_or(Path::new("."));
    if let Some((content, size)) = load_file(
        &project_dir.join(".jcode").join("prompt-overlay.md"),
        "Project Prompt Overlay (.jcode/prompt-overlay.md)",
    ) {
        total_chars += size;
        contents.push(content);
    }

    if let Ok(global_overlay) = crate::storage::jcode_dir().map(|dir| dir.join("prompt-overlay.md"))
        && let Some((content, size)) = load_file(
            &global_overlay,
            "Global Prompt Overlay (~/.jcode/prompt-overlay.md)",
        )
    {
        total_chars += size;
        contents.push(content);
    }

    if contents.is_empty() {
        (None, 0)
    } else {
        (Some(contents.join("\n\n")), total_chars)
    }
}

/// Load optional preferred-tool guidance from ~/.jcode/ and ./.jcode/
fn load_preferred_tools_files_from_dir(working_dir: Option<&Path>) -> (Option<String>, usize) {
    let mut contents = vec![];
    let mut total_chars = 0usize;

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

    let project_dir = working_dir.unwrap_or(Path::new("."));
    if let Some((content, size)) = load_file(
        &project_dir.join(".jcode").join("preferred-tools.md"),
        "Project Preferred Tools (.jcode/preferred-tools.md)",
    ) {
        total_chars += size;
        contents.push(content);
    }

    if let Ok(global_preferred_tools) =
        crate::storage::jcode_dir().map(|dir| dir.join("preferred-tools.md"))
        && let Some((content, size)) = load_file(
            &global_preferred_tools,
            "Global Preferred Tools (~/.jcode/preferred-tools.md)",
        )
    {
        total_chars += size;
        contents.push(content);
    }

    if contents.is_empty() {
        (None, 0)
    } else {
        (Some(contents.join("\n\n")), total_chars)
    }
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod prompt_tests;
