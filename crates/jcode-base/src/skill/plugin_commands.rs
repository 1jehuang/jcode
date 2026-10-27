//! Named manifest commands. Loading is inert; model/tool declarations are metadata.
use super::*;

impl SkillRegistry {
    pub(super) fn load_declared_commands(&mut self, plugin: &str, root: &Path) {
        let Ok(Some(declarations)) = crate::plugin_components::command_declarations(root) else {
            return;
        };
        for (name, declaration) in declarations {
            let path = declaration
                .source
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join(".claude-plugin/plugin.json"));
            let source = match declaration.content {
                Some(content) if !content.is_empty() => content,
                _ => match std::fs::read_to_string(&path) {
                    Ok(source) => source,
                    Err(_) => continue,
                },
            };
            let (description, content, allowed_tools, disable_model_invocation) =
                if source.trim_start().starts_with("---") {
                    let Ok((frontmatter, body)) = Self::parse_frontmatter(&source) else {
                        continue;
                    };
                    let tools = frontmatter.allowed_tools.map(|tools| match tools {
                        AllowedTools::CommaDelimited(tools) => {
                            tools.split(',').map(|s| s.trim().to_owned()).collect()
                        }
                        AllowedTools::Sequence(tools) => tools,
                    });
                    (
                        frontmatter
                            .description
                            .unwrap_or_else(|| description_from_body(&body)),
                        body,
                        tools,
                        frontmatter.disable_model_invocation,
                    )
                } else {
                    (description_from_body(&source), source, None, false)
                };
            // Skill currently has no argument-hint or model field. Do not inject
            // those into the prompt or claim a model switch. Tool metadata uses
            // the existing advisory Skill policy, never execution at discovery.
            let _unsupported_metadata = (&declaration.argument_hint, &declaration.model);
            let key = format!("{plugin}:{name}");
            let skill = Skill {
                name: key.clone(),
                description: declaration
                    .description
                    .filter(|text| !text.is_empty())
                    .unwrap_or(description),
                content,
                allowed_tools: declaration.allowed_tools.or(allowed_tools),
                disable_model_invocation,
                path,
                search_text: std::sync::OnceLock::new(),
            };
            self.command_origins
                .insert(key.clone(), (plugin.to_owned(), root.to_owned()));
            self.plugin_keys.insert(key.clone());
            self.skills.insert(key, skill);
        }
    }
}

#[cfg(test)]
#[path = "plugin_commands_tests.rs"]
mod tests;
