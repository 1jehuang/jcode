use super::*;

impl Agent {
    /// Synchronize the remote client's selected skill, accepting only names
    /// present in the daemon's own registry snapshot. `arguments` are the words
    /// typed after `/skill` on the invoking request, if any.
    pub(crate) fn set_remote_active_skill(
        &mut self,
        active_skill: Option<String>,
        arguments: Option<String>,
    ) -> bool {
        let skills = self.current_skills_snapshot();
        let recognized = active_skill
            .as_ref()
            .is_none_or(|name| skills.get(name).is_some());
        let next = active_skill.filter(|name| skills.get(name).is_some());
        let changed = next != self.active_skill;
        self.active_skill = next;
        if self.active_skill.is_none() {
            self.active_skill_render = None;
        } else if changed || arguments.is_some() {
            self.announce_active_skill_with(arguments.as_deref());
        }
        recognized
    }

    /// Activate `name` for a locally driven session, rendering `arguments`.
    pub(crate) fn activate_skill(&mut self, name: &str, arguments: Option<&str>) {
        self.active_skill = Some(name.to_string());
        self.announce_active_skill_with(arguments);
    }

    /// Announce the active skill with `$ARGUMENTS` substituted. The same
    /// rendered text is never announced twice in a row, so re-invoking a skill
    /// with identical arguments adds nothing while different arguments append
    /// the new rendering.
    fn announce_active_skill_with(&mut self, arguments: Option<&str>) {
        let Some(name) = self.active_skill.clone() else {
            return;
        };
        let skills = self.current_skills_snapshot();
        let Some(skill) = skills.get(&name) else {
            return;
        };
        let project_dir = self
            .session
            .working_dir
            .as_deref()
            .map(std::path::Path::new);
        let body = skill.render_prompt(arguments.unwrap_or(""), project_dir);
        let text = format!(
            "<system-reminder>\n# Active Skill\n\n{}\n</system-reminder>",
            body.trim()
        );
        if self.active_skill_render.as_deref() == Some(text.as_str()) {
            return;
        }
        self.active_skill_render = Some(text.clone());
        self.add_message_with_display_role(
            Role::User,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
            Some(StoredDisplayRole::System),
        );
        self.persist_session_best_effort("active skill announcement");
    }
}
