use super::*;
use crate::hooks::{PromptOrigin, PromptSubmit, PromptSubmitOutcome};

/// What the prompt hook decided for a message about to be appended.
pub(in crate::agent) struct PromptHookDecision {
    /// Hidden context to append after the user message.
    pub context: Option<String>,
    /// `systemMessage` notices for the user. Never added to model context.
    pub notices: Vec<String>,
}

/// Wire event that shows a hook notice to a client without touching the transcript.
pub(crate) fn hook_notice_event(session_id: &str, notice: &str) -> crate::protocol::ServerEvent {
    crate::protocol::ServerEvent::Notification {
        from_session: session_id.to_string(),
        from_name: Some("hook".to_string()),
        notification_type: crate::protocol::NotificationType::Message {
            scope: Some("hook".to_string()),
            channel: None,
            tldr: None,
        },
        message: notice.to_string(),
    }
}

impl Agent {
    /// Deliver notices on the CLI output path (stderr keeps `--json` stdout clean).
    pub(in crate::agent) fn print_hook_notices(notices: &[String]) {
        for notice in notices {
            eprintln!("[hook] {notice}");
        }
    }

    /// Like `run_once` for a prompt whose origin the caller declares.
    pub async fn run_once_as(&mut self, origin: PromptOrigin, msg: &str) -> Result<()> {
        crate::hooks::set_pending_prompt_origin(&self.session.id, origin);
        self.run_once(msg).await
    }

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
        let origin = crate::hooks::begin_turn_origin(&self.session.id, default_origin);
        let mut decision = PromptHookDecision {
            context: None,
            notices: Vec::new(),
        };
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
                decision.notices = notices;
                Ok(decision)
            }
            None => Ok(decision),
        }
    }
}
