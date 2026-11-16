use super::*;
use crate::protocol::DecisionChoice;
use crate::tui::backend::RemoteConnection;

impl App {
    /// Handle a key while the blocking decision chooser is visible.
    /// Returns true when the key was consumed by the chooser. The chooser
    /// owns the keyboard entirely while it is shown: the main input box is
    /// disabled, and free-form answers are typed into the chooser's own
    /// "Your answer" row instead.
    pub(in crate::tui::app) async fn handle_decision_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: &mut RemoteConnection,
    ) -> bool {
        let Some(mut decision) = self.pending_decision.take() else {
            return false;
        };
        let option_count = decision.options.len();
        let row_count = decision.row_count();

        // Arrow navigation moves between the numbered options and the
        // free-form "Your answer" row. Vim-style j/k are deliberately NOT
        // bound here: typing letters edits the answer draft when the answer
        // row is selected (and falls through as a no-op otherwise, since the
        // main input box is disabled while the chooser is visible).
        match (code, modifiers) {
            (KeyCode::Up, KeyModifiers::NONE) => {
                if decision.selected == PendingDecision::ANSWER_ROW {
                    // Up from the free-form row returns to the last option.
                    decision.selected = option_count.saturating_sub(1);
                } else {
                    decision.selected = decision.selected.saturating_sub(1);
                }
                self.pending_decision = Some(decision);
                true
            }
            (KeyCode::Down, KeyModifiers::NONE) => {
                if decision.selected == PendingDecision::ANSWER_ROW {
                    // Already on the last row; clamp.
                    self.pending_decision = Some(decision);
                } else if decision.selected + 1 == row_count - 1 {
                    decision.selected = PendingDecision::ANSWER_ROW;
                    self.pending_decision = Some(decision);
                } else if decision.selected + 1 < row_count {
                    decision.selected += 1;
                    self.pending_decision = Some(decision);
                } else {
                    self.pending_decision = Some(decision);
                }
                true
            }
            (KeyCode::Esc, KeyModifiers::NONE) => {
                // `dismiss_pending_decision` re-takes `pending_decision`, which
                // is already `None` here (we took it at the top of this
                // function), so it would silently skip sending the Dismissed
                // answer and the server-side ask_user would block forever.
                // Send directly from the decision we already hold.
                self.send_decision_answer(&decision.request_id, DecisionChoice::Dismissed, remote)
                    .await;
                self.set_status_notice("Decision dismissed");
                true
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                if decision.selected_is_answer_row() {
                    let text = decision.answer_draft.trim().to_string();
                    if text.is_empty() {
                        // Enter on an empty answer draft does nothing; the
                        // user has not written an answer yet.
                        self.pending_decision = Some(decision);
                    } else if self
                        .send_decision_answer(
                            &decision.request_id,
                            DecisionChoice::Text { text },
                            remote,
                        )
                        .await
                    {
                        self.record_decision_answer(&decision, "answered");
                    } else {
                        // Send failed: keep the chooser and draft open so the
                        // user can retry; do not record a phantom answer.
                        self.pending_decision = Some(decision);
                    }
                } else {
                    let index = (decision.selected as u32) + 1;
                    if self
                        .send_decision_answer(
                            &decision.request_id,
                            DecisionChoice::Option { index },
                            remote,
                        )
                        .await
                    {
                        self.record_decision_answer(&decision, "picked");
                    } else {
                        self.pending_decision = Some(decision);
                    }
                }
                true
            }
            // Shift/Alt+Enter inserts a newline in the answer draft, like
            // the composer's enter_inserts_newline: both chords must stay
            // live while typing an answer, and on the composer they work
            // through the kitty protocol (Shift) or ESC-prefix (Alt).
            // Ctrl/Cmd+Enter stays reserved (alternate-enter), and the
            // backslash continuation fallback is composer-only: an answer
            // draft is one-line text unless the user breaks it explicitly.
            (KeyCode::Enter, KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                if decision.selected_is_answer_row() {
                    decision.answer_draft.push('\n');
                }
                self.pending_decision = Some(decision);
                true
            }
            (KeyCode::Char(c), KeyModifiers::NONE) if c.is_ascii_digit() && c != '0' => {
                let index = c.to_digit(10).unwrap_or(1) as usize;
                if decision.selected_is_answer_row() {
                    // The answer row is a text entry: digits are typed text,
                    // not option picks. Picking option N while typing an
                    // answer would send a choice the user never made.
                    decision.answer_draft.push(c);
                    self.pending_decision = Some(decision);
                    true
                } else if index == option_count + 1 {
                    // The chooser numbers the answer row like the other
                    // choices: pressing that number selects it, instead of
                    // being silently discarded.
                    decision.selected = crate::tui::app::PendingDecision::ANSWER_ROW;
                    self.pending_decision = Some(decision);
                    true
                } else if index <= option_count {
                    // Reflect the digit pick in `selected` so the recorded
                    // answer names the option the user actually picked, not
                    // the row that happened to be highlighted.
                    decision.selected = index - 1;
                    if self
                        .send_decision_answer(
                            &decision.request_id,
                            DecisionChoice::Option {
                                index: index as u32,
                            },
                            remote,
                        )
                        .await
                    {
                        self.record_decision_answer(&decision, "picked");
                    } else {
                        self.pending_decision = Some(decision);
                    }
                    true
                } else {
                    self.pending_decision = Some(decision);
                    false
                }
            }
            // Printable text for the answer draft. Mirrors the main input's
            // text handling so Shift (uppercase, shifted symbols) and
            // AltGr/layout symbols work the same on the answer row. Digit
            // shortcuts are only reachable with an option row selected, so
            // typed digits can never trigger a pick while answering.
            (code, modifiers)
                if decision.selected_is_answer_row()
                    && matches!(code, KeyCode::Char(_))
                    && super::input::text_input_for_key(code, modifiers).is_some() =>
            {
                let typed = super::input::text_input_for_key(code, modifiers).unwrap();
                decision.answer_draft.push_str(&typed);
                self.pending_decision = Some(decision);
                true
            }
            // Backspace edits the draft one codepoint at a time.
            (KeyCode::Backspace, KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                if decision.selected_is_answer_row() {
                    decision.answer_draft.pop();
                    self.pending_decision = Some(decision);
                    true
                } else {
                    self.pending_decision = Some(decision);
                    false
                }
            }
            _ => {
                self.pending_decision = Some(decision);
                false
            }
        }
    }

    async fn send_decision_answer(
        &mut self,
        request_id: &str,
        choice: DecisionChoice,
        remote: &mut RemoteConnection,
    ) -> bool {
        match remote.send_decision_response(request_id, choice).await {
            Ok(()) => {
                // The chooser was resolved: the turn resumes, so publish
                // `working` over OSC 9 for herdr/herdr-webui. If another
                // question was queued behind this one, show it next.
                self.pending_decision = self.pending_decision_queue.pop_front();
                if self.pending_decision.is_some() {
                    self.set_status_notice(
                        "Decision needed: pick a number, or type your own answer",
                    );
                }
                crate::tui::app::decision_osc9::emit_decision_osc9(self.pending_decision.is_some());
                true
            }
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to send decision answer: {}",
                    e
                )));
                self.set_status_notice("Decision answer failed");
                false
            }
        }
    }

    /// Retire a decision chooser that resolved server-side, by request id.
    ///
    /// A nested `ask_user` (e.g. inside a `batch`) never gets its own
    /// `ToolDone`: the parent `batch` call does, and its `BatchProgress`
    /// events only carry the sub-call's tool call id. When such a sub-call
    /// finishes, its chooser is dead — the server already auto-picked an
    /// answer on timeout — so drop it instead of leaving a locked input box
    /// that swallows keys until the whole turn ends. When the retired chooser
    /// was the visible one, promote the next queued question, if any.
    pub(in crate::tui::app) fn retire_decision_by_request_id(&mut self, request_id: &str) {
        let retired_visible = self
            .pending_decision
            .as_ref()
            .is_some_and(|decision| decision.request_id == request_id);
        self.pending_decision_queue
            .retain(|decision| decision.request_id != request_id);
        if retired_visible {
            self.pending_decision = self.pending_decision_queue.pop_front();
            if self.pending_decision.is_some() {
                self.set_status_notice("Decision needed: pick a number, or type your own answer");
            }
            crate::tui::app::decision_osc9::emit_decision_osc9(self.pending_decision.is_some());
        }
    }

    /// Drop the visible chooser and every queued question, and reset the
    /// published OSC 9 status.
    ///
    /// Called when the turn that asked the question dies (cancel, finish,
    /// or fatal error): the server-side reply channels are gone with it, so
    /// any visible chooser or queued question is dead. The multiplexers
    /// (herdr/herdr-webui) must also stop seeing the pane as blocked, which
    /// means emitting `working` even if no chooser was ever shown, because
    /// the last published state may still say `blocked` from an earlier
    /// question in the same turn.
    pub(in crate::tui::app) fn clear_decisions_and_unblock(&mut self) {
        self.pending_decision = None;
        self.pending_decision_queue.clear();
        crate::tui::app::decision_osc9::emit_decision_osc9(false);
    }

    fn record_decision_answer(&mut self, decision: &crate::tui::app::PendingDecision, verb: &str) {
        let choice_text = if decision.selected_is_answer_row() {
            decision.answer_draft.trim().to_string()
        } else {
            decision
                .options
                .get(decision.selected)
                .map(|(label, detail)| {
                    if let Some(detail) = detail {
                        format!("{} ({})", label, detail)
                    } else {
                        label.clone()
                    }
                })
                .unwrap_or_else(|| decision.selected.to_string())
        };
        self.push_display_message(DisplayMessage::system(format!(
            "❓ {} — you {}: {}",
            decision.question, verb, choice_text
        )));
        self.set_status_notice("Answered");
    }
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod tests;
