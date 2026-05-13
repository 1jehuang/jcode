//! `App` glue for the `askUserQuestion` modal overlay: open, dispatch keys,
//! and submit the picked answer back through `crate::ask_user`.

use super::*;
use crate::ask_user::{AskUserAnswer, AskUserAnswerKind, AskUserQuestion, submit_answer};
use crate::tui::ask_user_modal::{AskUserModal, AskUserModalOutcome};
use crossterm::event::{KeyCode, KeyModifiers};
use std::cell::RefCell;

impl App {
    /// Open the ask-user modal for `question`. If a modal is already open for
    /// a different request_id, cancel the previous one so the new one can
    /// proceed; this preserves the invariant that only one ask-user modal is
    /// ever pending at a time and prevents stuck states.
    pub(crate) fn open_ask_user_modal(&mut self, question: AskUserQuestion) {
        if let Some(existing) = self.ask_user_overlay.take() {
            let prev_request_id = existing.borrow().request_id().to_string();
            if prev_request_id != question.request_id {
                submit_answer(AskUserAnswer {
                    request_id: prev_request_id,
                    kind: AskUserAnswerKind::Canceled,
                });
            }
        }
        let modal = AskUserModal::from_question(question);
        self.ask_user_overlay = Some(RefCell::new(modal));
        self.set_status_notice("Agent is asking you a question.");
    }

    /// Dispatch a key while the ask-user modal is visible. Returns true if
    /// the key was consumed.
    pub(crate) fn handle_ask_user_modal_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        let outcome = {
            let Some(cell) = self.ask_user_overlay.as_ref() else {
                return false;
            };
            let mut modal = cell.borrow_mut();
            modal.handle_key(code, modifiers)
        };

        match outcome {
            AskUserModalOutcome::Continue => {}
            AskUserModalOutcome::Done(answer) => {
                self.ask_user_overlay = None;
                // Submit even if the channel was already closed; submit_answer
                // tolerates unknown ids.
                submit_answer(answer);
                self.clear_status_notice();
            }
        }
        true
    }

    /// Render the ask-user modal overlay if visible.
    #[allow(dead_code)] // direct render path; currently driven via TuiState trait
    pub(crate) fn render_ask_user_modal(&self, frame: &mut ratatui::Frame) {
        if let Some(cell) = self.ask_user_overlay.as_ref() {
            cell.borrow().render(frame);
        }
    }

    /// Cancel and dismiss any active modal (used on session reset / cleanup).
    #[allow(dead_code)] // not yet wired into session reset path
    pub(crate) fn cancel_ask_user_modal(&mut self) {
        if let Some(cell) = self.ask_user_overlay.take() {
            let request_id = cell.borrow().request_id().to_string();
            submit_answer(AskUserAnswer {
                request_id,
                kind: AskUserAnswerKind::Canceled,
            });
        }
    }

    pub(crate) fn ask_user_modal_visible(&self) -> bool {
        self.ask_user_overlay.is_some()
    }
}
