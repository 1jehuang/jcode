use std::path::{Path, PathBuf};
use tokio::fs;

use super::skill::{SkillDefinition, SkillCategory, SkillParam, SkillParamType};
use super::registry::SkillRegistry;

/// Handles scanning directories for SKILL.md files and loading them
pub struct SkillLoader {
    scan_dirs: Vec<PathBuf>,
}

impl SkillLoader {
    pub fn new() -> Self {
        SkillLoader {
            scan_dirs: vec![],
        }
    }

    pub fn with_dir(mut self, dir: PathBuf) -> Self {
        self.scan_dirs.push(dir);
        self
    }

    /// Scan configured directories for skill definitions
    pub async fn scan_and_register(&self, registry: &SkillRegistry) -> Vec<String> {
        let mut loaded = vec![];

        for dir in &self.scan_dirs {
            let skills = self.scan_directory(dir).await;
            for skill in skills {
                let name = skill.name.clone();
                registry.register(&name, skill, None).await;
                loaded.push(name);
            }
        }

        loaded
    }

    /// Scan a single directory for skill definitions
    async fn scan_directory(&self, dir: &Path) -> Vec<SkillDefinition> {
        let mut skills = vec![];

        if !dir.exists() || !dir.is_dir() {
            return skills;
        }

        let mut entries = match fs::read_dir(dir).await {
            Ok(entries) => entries,
            Err(_) => return skills,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                // Check for SKILL.md inside subdirectory
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() && skill_file.is_file() {
                    if let Some(skill) = self.parse_skill_file(&path, &skill_file).await {
                        skills.push(skill);
                    }
                }
            }
        }

        skills
    }

    /// Parse a SKILL.md file to extract skill definition
    async fn parse_skill_file(&self, dir: &Path, _file: &Path) -> Option<SkillDefinition> {
        let dir_name = dir.file_name()?.to_str()?.to_string();

        // Extract metadata from directory name
        let name = dir_name.replace('_', "-").to_lowercase();
        let display_name = dir_name.replace('_', " ");

        let skill = SkillDefinition {
            name,
            display_name,
            description: format!("Skill loaded from {:?}", dir),
            category: SkillCategory::Custom("loaded".to_string()),
            params: vec![],
            prompt_template: None,
            source_path: Some(dir.to_string_lossy().to_string()),
            is_builtin: false,
            required_mcp_plugins: vec![],
            tags: vec![],
            executor: None,
        };

        Some(skill)
    }

    pub async fn register_from_mcp(&self, registry: &SkillRegistry, plugin_name: &str, tools: Vec<(String, String)>) {
        for (tool_name, tool_desc) in tools {
            let skill = SkillDefinition {
                name: format!("mcp-{}-{}", plugin_name, tool_name),
                display_name: format!("{}:{}", plugin_name, tool_name),
                description: tool_desc,
                category: SkillCategory::Custom("mcp".to_string()),
                params: vec![SkillParam {
                    name: "input".to_string(),
                    description: "Tool input arguments".to_string(),
                    required: true,
                    param_type: SkillParamType::String,
                    default_value: None,
                }],
                prompt_template: None,
                source_path: None,
                is_builtin: false,
                required_mcp_plugins: vec![plugin_name.to_string()],
                tags: vec!["mcp".to_string(), plugin_name.to_string()],
                executor: Some(format!("mcp:{}:{}", plugin_name, tool_name)),
            };
            registry.register(&skill.name.clone(), skill, None).await;
        }
    }
}

impl Default for SkillLoader {
    fn default() -> Self {
        Self::new()
    }
}