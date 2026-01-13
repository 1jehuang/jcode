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
    /// Load skills from all standard locations
    pub fn load() -> Result<Self> {
        let mut registry = Self::default();

        // Load from ~/.claude/skills/
        if let Some(home) = dirs::home_dir() {
            let global_skills = home.join(".claude").join("skills");
            if global_skills.exists() {
                registry.load_from_dir(&global_skills)?;
            }
        }

        // Load from ./.claude/skills/ (project-local)
        let local_skills = Path::new(".claude").join("skills");
        if local_skills.exists() {
            registry.load_from_dir(&local_skills)?;
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

        let allowed_tools = frontmatter.allowed_tools.map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .collect()
        });

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
        let end = rest.find("---").ok_or_else(|| anyhow::anyhow!("Unclosed frontmatter"))?;

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

        // Load from ~/.claude/skills/
        if let Some(home) = dirs::home_dir() {
            let global_skills = home.join(".claude").join("skills");
            if global_skills.exists() {
                count += self.load_from_dir_count(&global_skills)?;
            }
        }

        // Load from ./.claude/skills/ (project-local)
        let local_skills = Path::new(".claude").join("skills");
        if local_skills.exists() {
            count += self.load_from_dir_count(&local_skills)?;
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
            self.name,
            self.description,
            self.content
        )
    }

    /// Load additional files from the skill directory
    pub fn load_file(&self, filename: &str) -> Result<String> {
        let skill_dir = self.path.parent().ok_or_else(|| anyhow::anyhow!("No parent dir"))?;
        let file_path = skill_dir.join(filename);
        Ok(std::fs::read_to_string(file_path)?)
    }
}
