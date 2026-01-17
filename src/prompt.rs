//! System prompt management

use std::path::Path;
use std::process::Command;

/// Default system prompt for jcode (embedded at compile time)
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("prompt/system.txt");

/// Skill info for system prompt
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

/// Build the full system prompt with dynamic context
pub fn build_system_prompt(skill_prompt: Option<&str>, available_skills: &[SkillInfo]) -> String {
    let mut parts = vec![DEFAULT_SYSTEM_PROMPT.to_string()];

    // Add environment context
    if let Some(env_context) = build_env_context() {
        parts.push(env_context);
    }

    // Add CLAUDE.md instructions
    if let Some(claude_md) = load_claude_md_files() {
        parts.push(claude_md);
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
        parts.push(skills_section);
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
