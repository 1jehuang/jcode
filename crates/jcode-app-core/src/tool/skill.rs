//! Skill tool - load, list, reload, and read skills

use super::{Tool, ToolContext, ToolOutput};
use crate::skill::SkillRegistry;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct SkillTool {
    registry: Arc<RwLock<SkillRegistry>>,
}

impl SkillTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self { registry }
    }

    /// Effective skill set for this call: shared global registry plus the
    /// session's project-local overlay resolved from the tool context working
    /// dir (issue #457). The overlay is read fresh from disk so edits are
    /// visible without daemon restarts and never enter the shared registry.
    async fn effective_registry(&self, working_dir: Option<&std::path::Path>) -> SkillRegistry {
        let global = self.registry.read().await;
        SkillRegistry::effective_for_working_dir(&global, working_dir)
    }
}

#[derive(Deserialize)]
struct SkillInput {
    /// Action to perform: load (default), list, reload, reload_all, read.
    /// `list` shows both loaded skills and the jcode-endorsed catalog.
    #[serde(default = "default_action")]
    action: String,
    /// Skill name (required for load, reload, read)
    #[serde(alias = "skill")]
    #[serde(default)]
    name: Option<String>,
    /// Optional Claude-compatible Skill wrapper argument. The skill loader only
    /// needs to load the prompt, so args are currently accepted and ignored.
    #[serde(default)]
    args: Option<String>,
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
        "Manage skills."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["load", "list", "reload", "reload_all", "read"],
                    "description": "Action."
                },
                "name": {
                    "type": "string",
                    "description": "Skill name."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: SkillInput = serde_json::from_value(input)?;
        let action_label = params.action.clone();
        let name_label = params.name.clone().unwrap_or_else(|| "<none>".to_string());
        let _args = params.args.as_deref();

        match params.action.as_str() {
            "load" => {
                self.load_skill(params.name, ctx.working_dir.as_deref())
                    .await
            }
            "list" => self.list_skills(ctx.working_dir.as_deref()).await,
            "reload" => self.reload_skill(params.name).await,
            "reload_all" => self.reload_all_skills(ctx.working_dir.as_deref()).await,
            "read" => {
                self.read_skill(params.name, ctx.working_dir.as_deref())
                    .await
            }
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'load', 'list', 'reload', 'reload_all', or 'read'.",
                params.action
            ))),
        }
        .map_err(|err| {
            crate::logging::warn(&format!(
                "[tool:skill_manage] action failed action={} skill={} session_id={} error={}",
                action_label, name_label, ctx.session_id, err
            ));
            err
        })
    }
}

impl SkillTool {
    async fn load_skill(
        &self,
        name: Option<String>,
        working_dir: Option<&std::path::Path>,
    ) -> Result<ToolOutput> {
        let name = normalize_skill_name(name, "load")?;

        let registry = self.effective_registry(working_dir).await;
        let skill = registry.get(&name).ok_or_else(|| {
            // Endorsed skills are advertised in `list` but are not bundled;
            // a bare "not found" here reads like a bug (issue #445). Point at
            // the actual install command instead.
            if let Some(endorsed) = crate::skill::endorsed_skills()
                .iter()
                .find(|endorsed| endorsed.name == name)
            {
                match endorsed.install {
                    Some(install) => anyhow::anyhow!(
                        "Skill '{}' is endorsed but not installed. Install it with `{}`, then run skill_manage reload_all.",
                        name,
                        install
                    ),
                    None => anyhow::anyhow!(
                        "Skill '{}' is endorsed but not installed (source: {}). Install it into ~/.jcode/skills/{}/SKILL.md, then run skill_manage reload_all.",
                        name,
                        endorsed.source,
                        name
                    ),
                }
            } else {
                anyhow::anyhow!("Skill '{}' not found", name)
            }
        })?;

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

    async fn list_skills(&self, working_dir: Option<&std::path::Path>) -> Result<ToolOutput> {
        let registry = self.effective_registry(working_dir).await;
        let mut skills = registry.list();
        skills.sort_by(|a, b| a.name.cmp(&b.name));

        let installed: std::collections::HashSet<&str> =
            skills.iter().map(|s| s.name.as_str()).collect();

        let mut output = if skills.is_empty() {
            "No skills loaded.\n\n\
            Skills are loaded from:\n\
            - ~/.jcode/skills/<skill-name>/SKILL.md (global)\n\
            - ./.jcode/skills/<skill-name>/SKILL.md (project-local)\n\
            - ./.claude/skills/<skill-name>/SKILL.md (compatibility)\n\n\
            Create a SKILL.md file with YAML frontmatter:\n\
            ---\n\
            name: my-skill\n\
            description: What this skill does\n\
            allowed-tools: bash, read, write\n\
            ---\n\n\
            # Skill content here\n"
                .to_string()
        } else {
            // With hundreds of installed skills a full listing (multi-line
            // descriptions, paths, tool lists) is ~170KB of tool result that
            // permanently enters the transcript and never compacts out — one
            // observed `list` call added ~42k tokens to every subsequent turn
            // of the session. Keep the browsing view to one clipped line per
            // skill; `read`/`load` still expose full details for one skill.
            let mut output = format!(
                "Loaded skills: {} (one line each; use action=read for full details)\n\n",
                skills.len()
            );
            for skill in &skills {
                output.push_str(&format!(
                    "- /{} — {}\n",
                    skill.name,
                    jcode_base::prompt::clip_skill_description(&skill.description)
                ));
            }
            output
        };

        append_endorsed_skills(&mut output, &installed);

        Ok(ToolOutput::new(output).with_title("Skills: List"))
    }

    async fn reload_skill(&self, name: Option<String>) -> Result<ToolOutput> {
        let name = normalize_skill_name(name, "reload")?;

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
            Err(e) => {
                crate::logging::warn(&format!(
                    "[tool:skill_manage] reload failed skill={} error={}",
                    name, e
                ));
                Ok(
                    ToolOutput::new(format!("Failed to reload skill '{}': {}", name, e))
                        .with_title("Skills: Reload failed"),
                )
            }
        }
    }

    async fn reload_all_skills(&self, working_dir: Option<&std::path::Path>) -> Result<ToolOutput> {
        // Reload the shared GLOBAL registry only; the project-local overlay is
        // session-scoped and re-read from disk on every access, so reloading
        // it here would leak this session's project skills to other sessions
        // (issue #457).
        let reloaded = {
            let mut registry = self.registry.write().await;
            registry.reload_global()
        };

        match reloaded {
            Ok(global_count) => {
                let effective = self.effective_registry(working_dir).await;
                let skills = effective.list();
                let mut output = format!(
                    "Reloaded {} global skills ({} effective for this session)\n\n",
                    global_count,
                    skills.len()
                );

                for skill in skills {
                    output.push_str(&format!("- /{}: {}\n", skill.name, skill.description));
                }

                Ok(
                    ToolOutput::new(output)
                        .with_title(format!("Skills: Reloaded {}", global_count)),
                )
            }
            Err(e) => {
                crate::logging::warn(&format!(
                    "[tool:skill_manage] reload_all failed error={}",
                    e
                ));
                Ok(ToolOutput::new(format!("Failed to reload skills: {}", e))
                    .with_title("Skills: Reload failed"))
            }
        }
    }

    async fn read_skill(
        &self,
        name: Option<String>,
        working_dir: Option<&std::path::Path>,
    ) -> Result<ToolOutput> {
        let name = normalize_skill_name(name, "read")?;

        let registry = self.effective_registry(working_dir).await;

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

/// Append the curated jcode-endorsed skill catalog to `output`, grouped by
/// category and marked with installed/not-installed status. `installed` is the
/// set of skill names currently loaded in the registry.
fn append_endorsed_skills(output: &mut String, installed: &std::collections::HashSet<&str>) {
    let endorsed = crate::skill::endorsed_skills();
    if endorsed.is_empty() {
        return;
    }

    output.push_str("\nEndorsed skills (recommended by jcode)\n");

    // Group by category, preserving first-seen order.
    let mut category_order: Vec<&str> = Vec::new();
    for skill in endorsed {
        if !category_order.contains(&skill.category) {
            category_order.push(skill.category);
        }
    }

    for category in category_order {
        let in_category: Vec<_> = endorsed.iter().filter(|e| e.category == category).collect();
        let installed_count = in_category
            .iter()
            .filter(|e| installed.contains(e.name))
            .count();
        output.push_str(&format!(
            "\n  {} ({}/{} installed)\n",
            category,
            installed_count,
            in_category.len()
        ));
        for skill in in_category {
            let is_installed = installed.contains(skill.name);
            let status = if is_installed {
                "installed"
            } else {
                "not installed"
            };
            output.push_str(&format!("  - /{} [{}]\n", skill.name, status));
            output.push_str(&format!("      {}\n", skill.description));
            output.push_str(&format!("      source: {}\n", skill.source));
            if !is_installed && let Some(install) = skill.install {
                output.push_str(&format!("      install: {}\n", install));
            }
        }
    }

    output.push_str(
        "\nActivate a loaded skill by loading it with skill_manage (action=load) or typing its slash command.\n",
    );
    output.push_str(
        "NVIDIA CUDA-X skills come from the official catalog at https://github.com/NVIDIA/skills.\n",
    );
}

fn normalize_skill_name(name: Option<String>, action: &str) -> Result<String> {
    let name = name.ok_or_else(|| anyhow::anyhow!("'name' is required for {} action", action))?;
    let trimmed = name.trim().trim_start_matches('/').to_string();
    if trimmed.is_empty() {
        anyhow::bail!("'name' is required for {} action", action);
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tool() -> SkillTool {
        let registry = Arc::new(RwLock::new(SkillRegistry::default()));
        SkillTool::new(registry)
    }

    fn create_test_tool_with_skill(name: &str) -> (SkillTool, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let skill_dir = temp_dir.path().join(".jcode").join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Test skill\n---\n\n# Test Skill\n\nUse this test skill."
            ),
        )
        .unwrap();

        let registry = SkillRegistry::load_for_working_dir(Some(temp_dir.path())).unwrap();
        let tool = SkillTool::new(Arc::new(RwLock::new(registry)));
        (tool, temp_dir)
    }

    fn create_test_context() -> ToolContext {
        ToolContext {
            session_id: "test-session".to_string(),
            message_id: "test-message".to_string(),
            tool_call_id: "test-tool-call".to_string(),
            working_dir: None,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
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
        assert!(result.output.contains("No skills loaded"));
        // Even with no skills loaded, the endorsed catalog should be listed.
        assert!(result.output.contains("Endorsed skills"));
    }

    #[tokio::test]
    async fn test_list_includes_endorsed_skills() {
        let tool = create_test_tool();
        let ctx = create_test_context();
        let input = json!({"action": "list"});

        let result = tool.execute(input, ctx).await.unwrap();
        // Every endorsed skill should appear with an install-status marker.
        for endorsed in crate::skill::endorsed_skills() {
            assert!(
                result.output.contains(&format!("/{}", endorsed.name)),
                "expected endorsed skill /{} in:\n{}",
                endorsed.name,
                result.output
            );
        }
        // No skills are loaded in this tool, so they should be "not installed".
        assert!(result.output.contains("[not installed]"));
    }

    #[tokio::test]
    async fn test_list_is_compact_one_line_per_skill() {
        // Regression: with hundreds of skills the old multi-line listing
        // (description + Path + Tools per skill) was a ~170KB tool result
        // that bloated every subsequent turn. The browsing view must stay
        // one clipped line per skill with no path dumps.
        let long_description = "word ".repeat(200);
        let temp_dir = tempfile::tempdir().unwrap();
        let skill_dir = temp_dir.path().join(".jcode").join("skills").join("wordy");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: wordy\ndescription: \"{long_description}\"\n---\n\n# Wordy\n"),
        )
        .unwrap();
        let registry = SkillRegistry::load_for_working_dir(Some(temp_dir.path())).unwrap();
        let tool = SkillTool::new(Arc::new(RwLock::new(registry)));
        let ctx = create_test_context();

        let result = tool
            .execute(json!({"action": "list"}), ctx)
            .await
            .unwrap();
        let line = result
            .output
            .lines()
            .find(|line| line.starts_with("- /wordy"))
            .expect("skill line present");
        assert!(line.chars().count() < 160, "line too long: {line}");
        assert!(
            !result.output.contains(&temp_dir.path().display().to_string()),
            "listing must not dump per-skill paths"
        );
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
    async fn test_load_accepts_skill_alias_and_args() {
        let (tool, _temp_dir) = create_test_tool_with_skill("optimization");
        let ctx = create_test_context();
        let input = json!({"skill": "optimization", "args": "optimize this"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("## Skill: optimization"));
        assert_eq!(result.title.as_deref(), Some("skill: optimization"));
    }

    #[tokio::test]
    async fn test_load_strips_leading_slash_from_name() {
        let (tool, _temp_dir) = create_test_tool_with_skill("optimization");
        let ctx = create_test_context();
        let input = json!({"action": "load", "name": "/optimization"});

        let result = tool.execute(input, ctx).await.unwrap();
        assert!(result.output.contains("## Skill: optimization"));
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

    fn context_with_working_dir(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            working_dir: Some(dir.to_path_buf()),
            ..create_test_context()
        }
    }

    fn write_project_skill(root: &std::path::Path, name: &str) {
        let skill_dir = root.join(".agents").join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Project skill {name}\n---\n\nBody."),
        )
        .unwrap();
    }

    /// Issue #457: project-local skills must be session-scoped. Two contexts
    /// with different working dirs share one registry but must each see only
    /// their own project skills, immediately and without reload_all.
    #[tokio::test]
    async fn test_project_skills_are_scoped_to_tool_context_working_dir() {
        let tool = create_test_tool();
        let repo_a = tempfile::tempdir().unwrap();
        let repo_b = tempfile::tempdir().unwrap();
        write_project_skill(repo_a.path(), "repo-a-skill");
        write_project_skill(repo_b.path(), "repo-b-skill");

        // Immediately visible in each session without any reload.
        let list_a = tool
            .execute(
                json!({"action": "list"}),
                context_with_working_dir(repo_a.path()),
            )
            .await
            .unwrap();
        assert!(list_a.output.contains("repo-a-skill"));
        assert!(
            !list_a.output.contains("repo-b-skill"),
            "session A must not see session B's project skills"
        );

        let list_b = tool
            .execute(
                json!({"action": "list"}),
                context_with_working_dir(repo_b.path()),
            )
            .await
            .unwrap();
        assert!(list_b.output.contains("repo-b-skill"));
        assert!(!list_b.output.contains("repo-a-skill"));

        // reload_all in session A must not leak A's project skills into the
        // shared registry that session B reads.
        tool.execute(
            json!({"action": "reload_all"}),
            context_with_working_dir(repo_a.path()),
        )
        .await
        .unwrap();
        let shared = tool.registry.read().await;
        assert!(
            shared.get("repo-a-skill").is_none(),
            "shared registry must stay free of project-local skills"
        );
        drop(shared);

        // Skill file edits are visible without any reload/restart.
        let skill_md = repo_a.path().join(".agents/skills/repo-a-skill/SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: repo-a-skill\ndescription: Updated description\n---\n\nNew body.",
        )
        .unwrap();
        let read = tool
            .execute(
                json!({"action": "read", "name": "repo-a-skill"}),
                context_with_working_dir(repo_a.path()),
            )
            .await
            .unwrap();
        assert!(
            read.output.contains("Updated description"),
            "skill edits must be visible without daemon restart, got: {}",
            read.output
        );
    }
}
