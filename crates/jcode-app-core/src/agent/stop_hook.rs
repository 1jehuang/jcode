use super::*;
use crate::hooks::{StopInput, StopOutcome};

impl Agent {
    /// [`Self::stop_hook_continues`] for streaming turns: a cap notice goes to
    /// the client as a status detail.
    pub(in crate::agent) async fn stop_hook_continues_mpsc(
        &mut self,
        last_assistant_message: &str,
        event_tx: &mpsc::UnboundedSender<ServerEvent>,
    ) -> Result<bool> {
        self.stop_hook_continues(last_assistant_message, |detail| {
            if event_tx.send(ServerEvent::StatusDetail { detail }).is_err() {
                logging::debug("stop hook notice dropped: client event channel closed");
            }
        })
        .await
    }

    /// Run the `stop` hook for a turn that is about to end. Returns true when
    /// the hook granted a continuation, in which case a hidden reminder was
    /// appended and the caller must ask the model again instead of ending the
    /// turn. The continuation stays inside the same turn, so `turn_end` fires
    /// once and `user_prompt_submit` does not fire again.
    ///
    /// `notice` receives one user-visible line when the continuation cap
    /// overrides a request to continue.
    pub(in crate::agent) async fn stop_hook_continues(
        &mut self,
        last_assistant_message: &str,
        notice: impl FnOnce(String),
    ) -> Result<bool> {
        if self.is_graceful_shutdown() || !crate::hooks::hook_configured("stop") {
            return Ok(false);
        }
        let outcome = crate::hooks::run_stop_hook(&StopInput {
            session_id: &self.session.id,
            cwd: self.working_dir(),
            last_assistant_message,
        })
        .await;
        match outcome {
            Some(StopOutcome::Continue { reminder }) => {
                logging::info(&format!(
                    "stop hook continued session {} (continuation {})",
                    self.session.id,
                    crate::hooks::stop_continuations(&self.session.id)
                ));
                self.append_turn_system_reminder(&format!("[stop hook] {reminder}"))?;
                Ok(true)
            }
            Some(StopOutcome::CapReached { max }) => {
                logging::warn(&format!(
                    "stop hook continuation cap ({max}) reached for session {}; ending the turn",
                    self.session.id
                ));
                notice(format!(
                    "Stop hook asked to continue but the limit of {max} consecutive continuations was reached; ending the turn."
                ));
                Ok(false)
            }
            Some(StopOutcome::Stop) | None => Ok(false),
        }
    }
}
