use super::SkillRegistry;

/// A slash-command skill invocation with an optional trailing prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillInvocation<'a> {
    pub name: &'a str,
    pub prompt: Option<&'a str>,
}

impl SkillRegistry {
    /// Resolve a slash invocation against registered names, including names
    /// containing spaces. The longest registered name wins; unknown names fall
    /// back to the identifier-shaped result from [`Self::parse_invocation`].
    pub fn resolve_invocation<'a>(&self, input: &'a str) -> Option<SkillInvocation<'a>> {
        let fallback = Self::parse_invocation(input)?;
        let invocation = input.trim().strip_prefix('/')?;
        let boundaries = std::iter::once(invocation.len()).chain(
            invocation
                .char_indices()
                .rev()
                .filter_map(|(index, ch)| ch.is_whitespace().then_some(index)),
        );

        for end in boundaries {
            let name = &invocation[..end];
            if name.is_empty() || !self.skills.contains_key(name) {
                continue;
            }

            let prompt = invocation[end..].trim();
            let prompt = match prompt.as_bytes() {
                [b'"', .., b'"'] | [b'\'', .., b'\''] if prompt.len() >= 2 => {
                    &prompt[1..prompt.len() - 1]
                }
                _ => prompt,
            };
            return Some(SkillInvocation {
                name,
                prompt: (!prompt.is_empty()).then_some(prompt),
            });
        }

        Some(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::{Skill, build_skill_search_text};
    use std::path::PathBuf;

    fn registry(names: &[&str]) -> SkillRegistry {
        let mut registry = SkillRegistry::default();
        for name in names {
            registry.skills.insert(
                (*name).to_string(),
                Skill {
                    name: (*name).to_string(),
                    description: "Test skill".to_string(),
                    allowed_tools: None,
                    content: "content".to_string(),
                    path: PathBuf::from(format!("/tmp/{name}/SKILL.md")),
                    search_text: build_skill_search_text(name, "Test skill", "content"),
                },
            );
        }
        registry
    }

    #[test]
    fn matches_multi_word_skill_names() {
        let registry = registry(&["My Custom Skill"]);
        assert_eq!(
            registry.resolve_invocation("/My Custom Skill"),
            Some(SkillInvocation {
                name: "My Custom Skill",
                prompt: None,
            })
        );
    }

    #[test]
    fn keeps_trailing_prompt_after_multi_word_name() {
        let registry = registry(&["My Custom Skill"]);
        assert_eq!(
            registry.resolve_invocation("/My Custom Skill only the staged diff"),
            Some(SkillInvocation {
                name: "My Custom Skill",
                prompt: Some("only the staged diff"),
            })
        );
        assert_eq!(
            registry.resolve_invocation("/My Custom Skill 'only the staged diff'"),
            registry.resolve_invocation("/My Custom Skill only the staged diff")
        );
    }

    #[test]
    fn prefers_longest_registered_match() {
        let registry = registry(&["My", "My Custom Skill"]);
        assert_eq!(
            registry.resolve_invocation("/My Custom Skill please"),
            Some(SkillInvocation {
                name: "My Custom Skill",
                prompt: Some("please"),
            })
        );
    }

    #[test]
    fn falls_back_to_single_token_when_no_registry_match_exists() {
        let registry = SkillRegistry::default();
        assert_eq!(
            registry.resolve_invocation("/not-a-real-skill with a prompt"),
            Some(SkillInvocation {
                name: "not-a-real-skill",
                prompt: Some("with a prompt"),
            })
        );
    }

    #[test]
    fn does_not_bypass_terminal_file_drop_guard() {
        let registry = registry(&["tmp/screenshot.png"]);
        assert_eq!(registry.resolve_invocation("/tmp/screenshot.png"), None);
        assert_eq!(
            registry.resolve_invocation("/tmp/screenshot.png describe this"),
            None
        );
    }

    #[test]
    fn preserves_single_word_skill_invocations() {
        let registry = registry(&["frontend-design"]);
        assert_eq!(
            registry.resolve_invocation("/frontend-design build a settings page"),
            Some(SkillInvocation {
                name: "frontend-design",
                prompt: Some("build a settings page"),
            })
        );
    }
}
