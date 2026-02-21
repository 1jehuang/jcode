#![allow(dead_code)]
#![allow(dead_code)]

use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A skill definition from SKILL.md
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub allowed_tools: Option<Vec<String>>,
    pub content: String,
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(rename = "allowed-tools")]
    allowed_tools: Option<String>,
}

/// Registry of available skills
#[derive(Debug, Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    /// Import skills from Claude Code and Codex CLI on first run.
    /// Only runs if ~/.jcode/skills/ doesn't exist yet.
    fn import_from_external() {
        let jcode_skills = match crate::storage::jcode_dir() {
            Ok(dir) => dir.join("skills"),
            Err(_) => return,
        };

        if jcode_skills.exists() {
            return; // Not first run
        }

        let mut sources = Vec::new();
        let mut copied = Vec::new();

        if let Some(home) = dirs::home_dir() {
            // Import from Claude Code (~/.claude/skills/)
            let claude_skills = home.join(".claude").join("skills");
            if claude_skills.is_dir() {
                let count = Self::copy_skills_dir(&claude_skills, &jcode_skills);
                if count > 0 {
                    sources.push(format!("{} from Claude Code", count));
                    copied.extend(Self::list_skill_names(&jcode_skills));
                }
            }

            // Import from Codex CLI (~/.codex/skills/)
            let codex_skills = home.join(".codex").join("skills");
            if codex_skills.is_dir() {
                let count = Self::copy_skills_dir(&codex_skills, &jcode_skills);
                if count > 0 {
                    sources.push(format!("{} from Codex CLI", count));
                    copied.extend(Self::list_skill_names(&jcode_skills));
                }
            }
        }

        if !sources.is_empty() {
            // Deduplicate names
            copied.sort();
            copied.dedup();
            crate::logging::info(&format!(
                "Skills: Imported {} ({}) from {}",
                copied.len(),
                copied.join(", "),
                sources.join(" + "),
            ));
        }
    }

    /// Copy skill directories from src to dst. Returns count of skills copied.
    fn copy_skills_dir(src: &Path, dst: &Path) -> usize {
        let entries = match std::fs::read_dir(src) {
            Ok(e) => e,
            Err(_) => return 0,
        };

        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip Codex system skills
            if name.starts_with('.') {
                continue;
            }

            // Only copy if SKILL.md exists
            if !path.join("SKILL.md").exists() {
                continue;
            }

            let dest = dst.join(&name);
            if let Err(e) = Self::copy_dir_recursive(&path, &dest) {
                crate::logging::error(&format!("Failed to copy skill '{}': {}", name, e));
                continue;
            }
            count += 1;
        }
        count
    }

    /// Recursively copy a directory
    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else if src_path.is_symlink() {
                // Resolve symlink and copy the target
                let target = std::fs::read_link(&src_path)?;
                // Try to create symlink, fall back to copying the file
                if crate::platform::symlink_or_copy(&target, &dst_path).is_err() {
                    if let Ok(resolved) = std::fs::canonicalize(&src_path) {
                        std::fs::copy(&resolved, &dst_path)?;
                    }
                }
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    /// List skill directory names
    fn list_skill_names(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().to_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Load skills from all standard locations
    pub fn load() -> Result<Self> {
        // First-run import from Claude Code / Codex CLI
        Self::import_from_external();

        let mut registry = Self::default();

        // Load from ~/.jcode/skills/ (jcode's own global skills)
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let jcode_skills = jcode_dir.join("skills");
            if jcode_skills.exists() {
                registry.load_from_dir(&jcode_skills)?;
            }
        }

        // Load from ./.jcode/skills/ (project-local jcode skills)
        let local_jcode = Path::new(".jcode").join("skills");
        if local_jcode.exists() {
            registry.load_from_dir(&local_jcode)?;
        }

        // Fallback: ./.claude/skills/ (project-local Claude skills for compatibility)
        let local_claude = Path::new(".claude").join("skills");
        if local_claude.exists() {
            registry.load_from_dir(&local_claude)?;
        }

        Ok(registry)
    }

    /// Load skills from a directory
    fn load_from_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    if let Ok(skill) = Self::parse_skill(&skill_file) {
                        self.skills.insert(skill.name.clone(), skill);
                    }
                }
            }
        }

        Ok(())
    }

    /// Parse a SKILL.md file
    fn parse_skill(path: &Path) -> Result<Skill> {
        let content = std::fs::read_to_string(path)?;

        // Parse YAML frontmatter
        let (frontmatter, body) = Self::parse_frontmatter(&content)?;

        let allowed_tools = frontmatter
            .allowed_tools
            .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());

        Ok(Skill {
            name: frontmatter.name,
            description: frontmatter.description,
            allowed_tools,
            content: body,
            path: path.to_path_buf(),
        })
    }

    /// Parse YAML frontmatter from markdown
    fn parse_frontmatter(content: &str) -> Result<(SkillFrontmatter, String)> {
        let content = content.trim();

        if !content.starts_with("---") {
            anyhow::bail!("Missing YAML frontmatter");
        }

        let rest = &content[3..];
        let end = rest
            .find("---")
            .ok_or_else(|| anyhow::anyhow!("Unclosed frontmatter"))?;

        let yaml = &rest[..end];
        let body = rest[end + 3..].trim().to_string();

        let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml)?;

        Ok((frontmatter, body))
    }

    /// Get a skill by name
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// List all available skills
    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    /// Reload a specific skill by name
    pub fn reload(&mut self, name: &str) -> Result<bool> {
        // Find the skill's path first
        let path = self.skills.get(name).map(|s| s.path.clone());

        if let Some(path) = path {
            if path.exists() {
                let skill = Self::parse_skill(&path)?;
                self.skills.insert(skill.name.clone(), skill);
                Ok(true)
            } else {
                // Skill file was deleted
                self.skills.remove(name);
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    /// Reload all skills from all locations
    pub fn reload_all(&mut self) -> Result<usize> {
        self.skills.clear();

        let mut count = 0;

        // Load from ~/.jcode/skills/ (jcode's own global skills)
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let jcode_skills = jcode_dir.join("skills");
            if jcode_skills.exists() {
                count += self.load_from_dir_count(&jcode_skills)?;
            }
        }

        // Load from ./.jcode/skills/ (project-local jcode skills)
        let local_jcode = Path::new(".jcode").join("skills");
        if local_jcode.exists() {
            count += self.load_from_dir_count(&local_jcode)?;
        }

        // Fallback: ./.claude/skills/ (project-local Claude skills for compatibility)
        let local_claude = Path::new(".claude").join("skills");
        if local_claude.exists() {
            count += self.load_from_dir_count(&local_claude)?;
        }

        Ok(count)
    }

    /// Load skills from a directory and return count
    fn load_from_dir_count(&mut self, dir: &Path) -> Result<usize> {
        if !dir.is_dir() {
            return Ok(0);
        }

        let mut count = 0;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    if let Ok(skill) = Self::parse_skill(&skill_file) {
                        self.skills.insert(skill.name.clone(), skill);
                        count += 1;
                    }
                }
            }
        }

        Ok(count)
    }

    /// Check if a message is a skill invocation (starts with /)
    pub fn parse_invocation(input: &str) -> Option<&str> {
        let trimmed = input.trim();
        if trimmed.starts_with('/') && !trimmed.contains(' ') {
            Some(&trimmed[1..])
        } else {
            None
        }
    }
}

impl Skill {
    /// Get the full prompt content for this skill
    pub fn get_prompt(&self) -> String {
        format!(
            "# Skill: {}\n\n{}\n\n{}",
            self.name, self.description, self.content
        )
    }

    /// Load additional files from the skill directory
    pub fn load_file(&self, filename: &str) -> Result<String> {
        let skill_dir = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("No parent dir"))?;
        let file_path = skill_dir.join(filename);
        Ok(std::fs::read_to_string(file_path)?)
    }
}
