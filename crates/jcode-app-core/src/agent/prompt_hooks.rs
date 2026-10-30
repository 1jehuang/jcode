use super::*;
use crate::hooks::{PromptOrigin, PromptSubmit, PromptSubmitOutcome};

/// What the prompt hook decided for a message about to be appended.
pub(in crate::agent) struct PromptHookDecision {
    /// Hidden context to append after the user message.
    pub context: Option<String>,
}

impl Agent {
    /// Like `run_once_capture` for a prompt the host generated.
    pub async fn run_once_capture_as(&mut self, origin: PromptOrigin, msg: &str) -> Result<String> {
        crate::hooks::set_pending_prompt_origin(&self.session.id, origin);
        self.run_once_capture(msg).await
    }

    /// Run the `user_prompt_submit` hook for a new user message.
    ///
    /// `default_origin` applies when no caller declared one with
    /// `set_pending_prompt_origin`. Returns an error when a hook blocks the
    /// message, in which case nothing may be appended. Hook failures fail
    /// open. Costs one config lookup when no hook is configured.
    pub(in crate::agent) async fn run_user_prompt_submit_hook(
        &self,
        prompt: &str,
        default_origin: PromptOrigin,
        source: &str,
    ) -> Result<PromptHookDecision> {
        crate::hooks::reset_stop_continuations(&self.session.id);
        let origin = crate::hooks::begin_turn_origin(&self.session.id, default_origin);
        let mut decision = PromptHookDecision { context: None };
        if !crate::hooks::hook_configured("user_prompt_submit") {
            return Ok(decision);
        }
        let outcome = crate::hooks::run_user_prompt_submit(&PromptSubmit {
            session_id: &self.session.id,
            cwd: self.working_dir(),
            prompt,
            origin,
            active_skill: self.active_skill.as_deref(),
            source,
        })
        .await;
        match outcome {
            Some(PromptSubmitOutcome::Block { reason }) => {
                crate::logging::info(&format!(
                    "user_prompt_submit hook blocked a message for session {}",
                    self.session.id
                ));
                Err(crate::hooks::PromptBlocked(reason).into())
            }
            Some(PromptSubmitOutcome::Allow { context, notices }) => {
                for notice in notices {
                    crate::logging::info(&format!("user_prompt_submit notice: {notice}"));
                }
                decision.context = context;
                Ok(decision)
            }
            None => Ok(decision),
        }
    }
}
