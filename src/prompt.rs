//! System prompt management

use std::path::Path;
use std::process::Command;

/// Default system prompt for jcode (embedded at compile time)
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("prompt/system.txt");

/// Build the full system prompt with dynamic context
pub fn build_system_prompt(skill_prompt: Option<&str>) -> String {
    let mut parts = vec![DEFAULT_SYSTEM_PROMPT.to_string()];

    // Add environment context
    if let Some(env_context) = build_env_context() {
        parts.push(env_context);
    }

    // Add CLAUDE.md instructions
    if let Some(claude_md) = load_claude_md_files() {
        parts.push(claude_md);
    }

    // Add active skill prompt
    if let Some(skill) = skill_prompt {
        parts.push(format!("# Active Skill\n\n{}", skill));
    }

    parts.join("\n\n")
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
    if let Ok(output) = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
    {
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

/// Load CLAUDE.md files from project and home directory
fn load_claude_md_files() -> Option<String> {
    let mut contents = vec![];

    // Project CLAUDE.md (current directory)
    let project_path = Path::new("CLAUDE.md");
    if project_path.exists() {
        if let Ok(content) = std::fs::read_to_string(project_path) {
            contents.push(format!(
                "# Project Instructions (CLAUDE.md)\n\n{}",
                content.trim()
            ));
        }
    }

    // Home directory CLAUDE.md
    if let Some(home) = dirs::home_dir() {
        let home_path = home.join("CLAUDE.md");
        if home_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&home_path) {
                contents.push(format!(
                    "# Global Instructions (~/.CLAUDE.md)\n\n{}",
                    content.trim()
                ));
            }
        }
    }

    if contents.is_empty() {
        None
    } else {
        Some(contents.join("\n\n"))
    }
}
