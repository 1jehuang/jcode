//! Agent-side glue for `crate::harness`: shared by the blocking and the
//! streaming turn loops so both enforce the same policy.

use super::*;

impl Agent {
    /// Latest real user prompt (text, not a tool result) in the transcript.
    fn latest_user_prompt(&self) -> String {
        self.session
            .messages
            .iter()
            .rev()
            .find(|m| {
                m.role == Role::User
                    && !m
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            })
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    /// Pre-turn router: runs once per user turn, before the first request.
    pub(super) fn harness_begin_turn(&mut self) {
        if !crate::harness::enabled() {
            return;
        }
        let prompt = self.latest_user_prompt();
        let skills = self.current_skills_snapshot();
        let turn = crate::harness::begin_turn(&self.session.id, &prompt, &skills);
        if !turn.auto_routed_skills.is_empty() {
            logging::info(&format!(
                "[harness] auto-routed skills: {}",
                turn.auto_routed_skills.join(", ")
            ));
        }
    }

    /// The one request-only policy message. Never persisted.
    pub(super) fn harness_policy_message(&self) -> Option<Message> {
        crate::harness::enabled()
            .then(|| Message::user(&crate::harness::policy_block(&self.session.id)))
    }

    /// Turn-completion gate. On a premature text-only stop, drops the
    /// unverified answer from history, appends the correction and returns true.
    pub(super) fn harness_intercept_stop(&mut self) -> Result<bool> {
        if !crate::harness::enabled() || self.is_graceful_shutdown() {
            return Ok(false);
        }
        let Some(correction) = crate::harness::stop_intervention(&self.session.id) else {
            return Ok(false);
        };
        logging::warn("[harness] intercepted text-only stop on an actionable turn");
        if self
            .session
            .messages
            .last()
            .is_some_and(|m| m.role == Role::Assistant)
        {
            let len = self.session.messages.len() - 1;
            self.session.truncate_messages(len);
        }
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: correction,
                cache_control: None,
            }],
        );
        self.session.save()?;
        Ok(true)
    }

    pub(super) fn harness_end_turn(&self) {
        if crate::harness::enabled() {
            crate::harness::log_turn(&self.session.id);
        }
    }
}
