//! Skill tool - load, list, reload, and read skills

use super::{Tool, ToolContext, ToolOutput};
use crate::skill::SkillRegistry;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct SkillTool {
    registry: Arc<RwLock<SkillRegistry>>,
}

impl SkillTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self { registry }
    }
}

#[derive(Deserialize)]
struct SkillInput {
    /// Action to perform: load (default), list, reload, reload_all, read
    #[serde(default = "default_action")]
    action: String,
    /// Skill name (required for load, reload, read)
    #[serde(default)]
    name: Option<String>,
}

fn default_action() -> String {
    "load".to_string()
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill_manage"
    }

    fn description(&self) -> &str {
        "Manage skills: list available skills, reload after editing, or read skill content. \
         Use action='list' to see skills, 'reload' to refresh a skill after editing its SKILL.md, \
         'read' to see full content."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["load", "list", "reload", "reload_all", "read"],
                    "description": "Action: 'load' (default) activates a skill, 'list' shows all skills, 'reload' reloads a skill after editing, 'reload_all' reloads all skills, 'read' shows skill content"
                },
                "name": {
                    "type": "string",
                    "description": "Skill name (required for load, reload, read)"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: SkillInput = serde_json::from_value(input)?;

        match params.action.as_str() {
            "load" => self.load_skill(params.name).await,
            "list" => self.list_skills().await,
            "reload" => self.reload_skill(params.name).await,
            "reload_all" => self.reload_all_skills().await,
            "read" => self.read_skill(params.name).await,
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'load', 'list', 'reload', 'reload_all', or 'read'.",
                params.action
            ))),
        }
    }
}

impl SkillTool {
    async fn load_skill(&self, name: Option<String>) -> Result<ToolOutput> {
        let name = name.ok_or_else(|| anyhow::anyhow!("'name' is required for load action"))?;

        let registry = self.registry.read().await;
        let skill = registry
            .get(&name)
            .ok_or_else(|| anyhow::anyhow!("Skill '{}' not found", name))?;

        let base_dir = skill
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".to_string());

        Ok(ToolOutput::new(format!(
            "## Skill: {}\n\n**Base directory**: {}\n\n{}",
            skill.name,
            base_dir,
            skill.get_prompt()
        ))
        .with_title(format!("skill: {}", skill.name)))
    }

    async fn list_skills(&self) -> Result<ToolOutput> {
        let registry = self.registry.read().await;
        let skills = registry.list();

        if skills.is_empty() {
            return Ok(ToolOutput::new(
                "No skills available.\n\n\
                Skills are loaded from:\n\
                - ~/.claude/skills/<skill-name>/SKILL.md\n\
                - ./.claude/skills/<skill-name>/SKILL.md\n\n\
                Create a SKILL.md file with YAML frontmatter:\n\
                ---\n\
                name: my-skill\n\
                description: What this skill does\n\
                allowed-tools: bash, read, write\n\
                ---\n\n\
                # Skill content here",
            )
            .with_title("Skills: None available"));
        }

        let mut output = format!("Available skills: {}\n\n", skills.len());

        for skill in skills {
            output.push_str(&format!("## /{}\n", skill.name));
            output.push_str(&format!("  {}\n", skill.description));
            output.push_str(&format!("  Path: {}\n", skill.path.display()));
            if let Some(ref tools) = skill.allowed_tools {
                output.push_str(&format!("  Tools: {}\n", tools.join(", ")));
            }
            output.push('\n');
        }

        Ok(ToolOutput::new(output).with_title("Skills: List"))
    }

    async fn reload_skill(&self, name: Option<String>) -> Result<ToolOutput> {
        let name = name.ok_or_else(|| anyhow::anyhow!("'name' is required for reload action"))?;

        let mut registry = self.registry.write().await;

        match registry.reload(&name) {
            Ok(true) => {
                // Re-read to get updated info
                if let Some(skill) = registry.get(&name) {
                    Ok(ToolOutput::new(format!(
                        "Reloaded skill '{}'\n\nDescription: {}\nPath: {}",
                        name,
                        skill.description,
                        skill.path.display()
                    ))
                    .with_title(format!("Skills: Reloaded {}", name)))
                } else {
                    Ok(ToolOutput::new(format!("Reloaded skill '{}'", name))
                        .with_title(format!("Skills: Reloaded {}", name)))
                }
            }
            Ok(false) => Ok(ToolOutput::new(format!(
                "Skill '{}' not found or was deleted.\n\nUse 'list' to see available skills.",
                name
            ))
            .with_title("Skills: Not found")),
            Err(e) => Ok(
                ToolOutput::new(format!("Failed to reload skill '{}': {}", name, e))
                    .with_title("Skills: Reload failed"),
            ),
        }
    }

    async fn reload_all_skills(&self) -> Result<ToolOutput> {
        let mut registry = self.registry.write().await;

        match registry.reload_all() {
            Ok(count) => {
                let skills = registry.list();
                let mut output = format!("Reloaded {} skills\n\n", count);

                for skill in skills {
                    output.push_str(&format!("- /{}: {}\n", skill.name, skill.description));
                }

                Ok(ToolOutput::new(output).with_title(format!("Skills: Reloaded {}", count)))
            }
            Err(e) => Ok(ToolOutput::new(format!("Failed to reload skills: {}", e))
                .with_title("Skills: Reload failed")),
        }
    }

    async fn read_skill(&self, name: Option<String>) -> Result<ToolOutput> {
        let name = name.ok_or_else(|| anyhow::anyhow!("'name' is required for read action"))?;

        let registry = self.registry.read().await;

        if let Some(skill) = registry.get(&name) {
            let mut output = format!("# Skill: {}\n\n", skill.name);
            output.push_str(&format!("**Description:** {}\n", skill.description));
            output.push_str(&format!("**Path:** {}\n", skill.path.display()));
            if let Some(ref tools) = skill.allowed_tools {
                output.push_str(&format!("**Allowed tools:** {}\n", tools.join(", ")));
            }
            output.push_str("\n---\n\n");
            output.push_str(&skill.content);

            Ok(ToolOutput::new(output).with_title(format!("Skills: {}", name)))
        } else {
            Ok(ToolOutput::new(format!(
                "Skill '{}' not found.\n\nUse 'list' to see available skills.",
                name
            ))
            .with_title("Skills: Not found"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tool() -> SkillTool {
        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        SkillTool::new(registry)
    }

    fn create_test_context() -> ToolContext {
        ToolContext {
            session_id: "test-session".to_string(),
            message_id: "test-message".to_string(),
            tool_call_id: "test-tool-call".to_string(),
            working_dir: None,
        }
    }

    #[test]
    fn test_tool_name() {
        let tool = create_test_tool();
        assert_eq!(tool.name(), "skill_manage");
    }

    #[test]
    fn test_tool_description() {
        let tool = create_test_tool();
        assert!(tool.description().contains("skill"));
    }

    #[test]
    fn test_parameters_schema() {
        let tool = create_test_tool();
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["action"].is_object());
        assert!(schema["properties"]["name"].is_object());
    }

    #[tokio::test]
    async fn test_list_empty() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "list"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("No skills available"));
    }

    #[tokio::test]
    async fn test_load_missing_name() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "load"});

        let result = tool.execute(input, ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[tokio::test]
    async fn test_reload_missing_name() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "reload"});

        let result = tool.execute(input, ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[tokio::test]
    async fn test_read_missing_name() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "read"});

        let result = tool.execute(input, ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[tokio::test]
    async fn test_reload_nonexistent() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "reload", "name": "nonexistent"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("not found"));
    }

    #[tokio::test]
    async fn test_unknown_action() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "invalid"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("Unknown action"));
    }

    #[tokio::test]
    async fn test_reload_all() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "reload_all"});

        let result = tool.execute(input, ctx).await.unwrap();
        // The output format is "Reloaded N skills" where N is any number
        // (depends on what skills exist on the system)
        assert!(
            result.output.contains("Reloaded"),
            "Expected 'Reloaded' in output, got: {}",
            result.output
        );
        assert!(
            result.output.contains("skills"),
            "Expected 'skills' in output, got: {}",
            result.output
        );
    }
}
