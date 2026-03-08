use super::Agent;
use crate::logging;
use crate::message::Message;

impl Agent {
    /// Build the system prompt, including skill, memory, self-dev context, and CLAUDE.md files
    pub(super) fn build_system_prompt(&self, memory_prompt: Option<&str>) -> String {
        let split = self.build_system_prompt_split(memory_prompt);
        if split.dynamic_part.is_empty() {
            split.static_part
        } else if split.static_part.is_empty() {
            split.dynamic_part
        } else {
            format!("{}\n\n{}", split.static_part, split.dynamic_part)
        }
    }

    /// Build split system prompt for better caching
    /// Returns static (cacheable) and dynamic (not cached) parts separately
    pub(super) fn build_system_prompt_split(
        &self,
        memory_prompt: Option<&str>,
    ) -> crate::prompt::SplitSystemPrompt {
        if let Some(ref override_prompt) = self.system_prompt_override {
            return crate::prompt::SplitSystemPrompt {
                static_part: override_prompt.clone(),
                dynamic_part: String::new(),
            };
        }

        let skill_prompt = self.active_skill.as_ref().and_then(|name| {
            self.skills
                .get(name)
                .map(|skill| skill.get_prompt().to_string())
        });

        let available_skills: Vec<crate::prompt::SkillInfo> = self
            .skills
            .list()
            .iter()
            .map(|skill| crate::prompt::SkillInfo {
                name: skill.name.clone(),
                description: skill.description.clone(),
            })
            .collect();

        let working_dir = self
            .session
            .working_dir
            .as_ref()
            .map(std::path::PathBuf::from);

        let (split, _context_info) = crate::prompt::build_system_prompt_split(
            skill_prompt.as_deref(),
            &available_skills,
            self.session.is_canary,
            memory_prompt,
            working_dir.as_deref(),
        );

        split
    }

    /// Non-blocking memory prompt - takes pending result and spawns check for next turn
    pub(super) fn build_memory_prompt_nonblocking(
        &self,
        messages: &[Message],
    ) -> Option<crate::memory::PendingMemory> {
        if !self.memory_enabled {
            return None;
        }

        let session_id = &self.session.id;
        let pending = crate::memory::take_pending_memory(session_id);

        let manager = crate::memory::MemoryManager::new();
        manager.spawn_relevance_check(session_id, messages.to_vec());

        crate::memory_agent::update_context_sync(session_id, messages.to_vec());

        pending
    }

    /// Legacy blocking memory prompt - kept for fallback
    #[allow(dead_code)]
    pub(super) async fn build_memory_prompt(&self, messages: &[Message]) -> Option<String> {
        let manager = crate::memory::MemoryManager::new();
        match manager.relevant_prompt_for_messages(messages).await {
            Ok(prompt) => prompt,
            Err(error) => {
                logging::info(&format!("Memory relevance skipped: {}", error));
                None
            }
        }
    }
}
