use super::App;
use crate::suggested_replies::SuggestedRepliesResponse;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub const EXPLORER_AUTO_DELAY: Duration = Duration::from_millis(500);

/// Focus state inside the Suggested Replies panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestedReplyFocus {
    DefaultAction,
    Secondary(usize),
}

/// View snapshot exposed through `TuiState` to the renderer (`ui_inline.rs`).
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestedRepliesView {
    pub response: SuggestedRepliesResponse,
    pub focus: Option<SuggestedReplyFocus>,
    pub explorer_remaining_ms: Option<u64>,
}

/// Runtime state owned by `App` for two-stage Suggested Replies and Explorer mode.
#[derive(Default)]
pub struct SuggestedRepliesRuntime {
    pub current: Option<SuggestedRepliesResponse>,
    pub focus: Option<SuggestedReplyFocus>,
    pub explorer_deadline: Option<Instant>,
    pub pending_rx: Option<mpsc::UnboundedReceiver<SuggestedRepliesResponse>>,
    pub pending_six_lens_findings: Option<String>,
    pub pending_remote_submit: Option<String>,
    pub force_enable_in_tests: bool,
    pub last_evaluated_messages_version: u64,
}

impl App {
    /// Whether the Suggested Replies panel should be visible above the input box.
    pub fn suggested_replies_visible(&self) -> bool {
        if !crate::config::config().suggested_replies.enabled {
            return false;
        }
        if self.is_processing
            || !self.input.is_empty()
            || !self.pending_images.is_empty()
            || self.onboarding_welcome_active()
            || self.inline_interactive_state.is_some()
            || self.inline_view_state.is_some()
            || self.prompt_history_search.is_some()
            || !self.command_suggestions().is_empty()
        {
            return false;
        }
        self.suggested_replies
            .current
            .as_ref()
            .is_some_and(|r| !r.is_empty())
    }

    /// Build the render view for `TuiState::suggested_replies_view()`.
    pub fn build_suggested_replies_view(&self) -> Option<SuggestedRepliesView> {
        if !self.suggested_replies_visible() {
            return None;
        }
        let response = self.suggested_replies.current.clone()?;
        let explorer_remaining_ms = self.suggested_replies.explorer_deadline.map(|deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64
        });
        Some(SuggestedRepliesView {
            response,
            focus: self.suggested_replies.focus,
            explorer_remaining_ms,
        })
    }

    /// Cancel any armed Full-Auto Explorer countdown and clear focus when the
    /// user interacts with the composer or presses Esc.
    pub fn cancel_suggested_replies_interaction(&mut self, clear_suggestions: bool) -> bool {
        let mut changed = false;
        if self.suggested_replies.explorer_deadline.take().is_some() {
            changed = true;
        }
        if self.suggested_replies.focus.take().is_some() {
            changed = true;
        }
        if clear_suggestions && self.suggested_replies.current.take().is_some() {
            changed = true;
        }
        changed
    }

    /// Apply a freshly computed `SuggestedRepliesResponse`, arming the 500ms
    /// Full-Auto Explorer timer when `enable_full_auto_explorer` is enabled and
    /// a `default_action` cleared the confidence threshold.
    pub fn apply_suggested_replies_response(&mut self, response: SuggestedRepliesResponse) {
        if response.is_empty() {
            self.suggested_replies.current = None;
            self.suggested_replies.focus = None;
            self.suggested_replies.explorer_deadline = None;
            return;
        }
        let cfg = &crate::config::config().suggested_replies;
        let can_arm_explorer = cfg.enable_full_auto_explorer
            && response.default_action.is_some()
            && !self.is_processing
            && self.input.is_empty()
            && self.pending_images.is_empty()
            && self.queued_messages.is_empty()
            && !self.autoreview_enabled
            && !self.autojudge_enabled
            && !self.pending_split_request;

        // Leave `focus` as `None` initially so the first `[Tab]` explicitly focuses
        // `DefaultAction` (or `Secondary(0)`) and the next `[Tab]` or `[Enter]` activates it,
        // while `[Ctrl+Enter]` always activates `DefaultAction` immediately.
        self.suggested_replies.focus = None;
        self.suggested_replies.explorer_deadline =
            can_arm_explorer.then(|| Instant::now() + EXPLORER_AUTO_DELAY);
        self.suggested_replies.current = Some(response);
    }

    /// Build a compact recent-conversation snapshot for the two-stage cascade.
    fn recent_transcript_for_suggestions(&self) -> String {
        let mut buf = String::new();
        for msg in self.display_messages.iter().rev().take(6).rev() {
            if matches!(msg.role.as_str(), "user" | "assistant" | "swarm")
                && !msg.content.trim().is_empty()
            {
                buf.push_str(&format!(
                    "[{}]: {}\n\n",
                    msg.role,
                    crate::util::truncate_str(msg.content.trim(), 900)
                ));
            }
        }
        buf
    }

    /// Trigger the two-stage Suggested Replies cascade at the end of a turn.
    pub fn maybe_trigger_suggested_replies_at_turn_end(&mut self) {
        // In TestHarness / Replay mode, do not auto-populate inline suggestions
        // unless the test explicitly opted in via `force_enable_in_tests`, so
        // existing viewport-height / anchor-stability snapshot tests stay untouched.
        if (self.is_replay || self.runtime_mode == super::AppRuntimeMode::TestHarness)
            && !self.suggested_replies.force_enable_in_tests
        {
            return;
        }

        let cfg = crate::config::config().suggested_replies.clone();
        if !cfg.enabled
            || self.is_processing
            || !self.queued_messages.is_empty()
            || !self.hidden_queued_system_messages.is_empty()
            || self.pending_queued_dispatch
            || self.has_pending_user_followup()
        {
            return;
        }

        let transcript_tail = self.recent_transcript_for_suggestions();
        if transcript_tail.trim().is_empty() {
            return;
        }
        self.suggested_replies.last_evaluated_messages_version = self.display_messages_version;
        let six_lens_findings = self.suggested_replies.pending_six_lens_findings.take();

        // Populate immediate deterministic suggestions first so the UI responds
        // on the very first idle frame after turn completion, then upgrade via
        // the async OpenJev / OpenGF background task if a live route is configured.
        let immediate = crate::suggested_replies::score_locally_from_context(
            &transcript_tail,
            None,
            six_lens_findings.as_deref(),
            cfg.clamped_confidence_threshold(),
            cfg.clamped_max_secondary(),
        );
        self.apply_suggested_replies_response(immediate);

        if !cfg!(test)
            && !self.is_replay
            && self.runtime_mode != super::AppRuntimeMode::TestHarness
            && six_lens_findings.is_none()
            && (cfg.use_open_gf || crate::jev::JevClient::suggestions_available())
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            let (tx, rx) = mpsc::unbounded_channel();
            self.suggested_replies.pending_rx = Some(rx);
            handle.spawn(async move {
                let response = crate::suggested_replies::evaluate_cascade(
                    &transcript_tail,
                    &cfg,
                    six_lens_findings.as_deref(),
                )
                .await;
                let _ = tx.send(response);
            });
        }
    }

    /// Record findings from a 6-Lens Subagent Self-Review and immediately
    /// surface a high-confidence Default Action in Suggested Replies.
    pub fn record_six_lens_review_findings(&mut self, summary: String) {
        let trimmed = summary.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        self.suggested_replies.pending_six_lens_findings = Some(trimmed.clone());
        let cfg = &crate::config::config().suggested_replies;
        let response = crate::suggested_replies::score_locally_from_context(
            &self.recent_transcript_for_suggestions(),
            None,
            Some(&trimmed),
            cfg.clamped_confidence_threshold(),
            cfg.clamped_max_secondary(),
        );
        self.apply_suggested_replies_response(response);
    }

    /// Poll background cascade results and the 500ms Full-Auto Explorer timer.
    /// Returns `(needs_redraw, auto_submit_prompt)`.
    pub fn poll_suggested_replies_tick(&mut self) -> (bool, Option<String>) {
        let mut needs_redraw = false;

        if let Some(rx) = self.suggested_replies.pending_rx.as_mut() {
            match rx.try_recv() {
                Ok(response) => {
                    self.suggested_replies.pending_rx = None;
                    if !self.is_processing && self.input.is_empty() {
                        self.apply_suggested_replies_response(response);
                        needs_redraw = true;
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.suggested_replies.pending_rx = None;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
        }

        if let Some(deadline) = self.suggested_replies.explorer_deadline {
            if self.is_processing
                || !self.input.is_empty()
                || !self.pending_images.is_empty()
                || !crate::config::config()
                    .suggested_replies
                    .enable_full_auto_explorer
            {
                self.suggested_replies.explorer_deadline = None;
                needs_redraw = true;
            } else if Instant::now() >= deadline {
                self.suggested_replies.explorer_deadline = None;
                if let Some(action) = self
                    .suggested_replies
                    .current
                    .as_ref()
                    .and_then(|r| r.default_action.clone())
                {
                    self.suggested_replies.current = None;
                    self.suggested_replies.focus = None;
                    return (true, Some(action));
                }
                needs_redraw = true;
            } else {
                needs_redraw = true;
            }
        }

        (needs_redraw, None)
    }

    /// Handle `[Tab]` when `self.autocomplete()` returned `false` and input is empty.
    /// Returns `Some(Some(prompt))` when `[Tab]` activates a focused Default Action,
    /// `Some(None)` when `[Tab]` moved focus inside the Suggested Replies panel,
    /// or `None` when Suggested Replies are not visible.
    pub fn handle_suggested_replies_tab(&mut self) -> Option<Option<String>> {
        if !self.suggested_replies_visible() {
            return None;
        }
        // Any manual key cancels the auto-explorer countdown.
        self.suggested_replies.explorer_deadline = None;
        let response = self.suggested_replies.current.as_ref()?;

        if let Some(default_action) = response.default_action.clone() {
            match self.suggested_replies.focus {
                None => {
                    self.suggested_replies.focus = Some(SuggestedReplyFocus::DefaultAction);
                    self.set_status_notice("Default Action focused (Tab/Enter to run)");
                    Some(None)
                }
                Some(SuggestedReplyFocus::DefaultAction) => {
                    // Already focused on Default Action -> activate and submit!
                    self.suggested_replies.current = None;
                    self.suggested_replies.focus = None;
                    self.set_status_notice("Default Action activated");
                    Some(Some(default_action))
                }
                Some(SuggestedReplyFocus::Secondary(idx)) => {
                    let len = response.secondary_suggestions.len();
                    if idx + 1 < len {
                        self.suggested_replies.focus =
                            Some(SuggestedReplyFocus::Secondary(idx + 1));
                    } else {
                        self.suggested_replies.focus = Some(SuggestedReplyFocus::DefaultAction);
                    }
                    Some(None)
                }
            }
        } else if !response.secondary_suggestions.is_empty() {
            let len = response.secondary_suggestions.len();
            let next_idx = match self.suggested_replies.focus {
                Some(SuggestedReplyFocus::Secondary(idx)) => (idx + 1) % len,
                _ => 0,
            };
            self.suggested_replies.focus = Some(SuggestedReplyFocus::Secondary(next_idx));
            self.set_status_notice(format!(
                "Suggestion {}/{} focused (Enter to run)",
                next_idx + 1,
                len
            ));
            Some(None)
        } else {
            None
        }
    }

    /// Return the currently focused suggestion (or `default_action` when `Ctrl+Enter` is pressed)
    /// and consume the Suggested Replies panel so the caller can submit it.
    pub fn take_focused_or_default_suggested_reply(
        &mut self,
        allow_unfocused_default: bool,
    ) -> Option<String> {
        if !self.suggested_replies_visible() {
            return None;
        }
        let response = self.suggested_replies.current.as_ref()?;
        let selected = match self.suggested_replies.focus {
            Some(SuggestedReplyFocus::DefaultAction) => response.default_action.clone(),
            Some(SuggestedReplyFocus::Secondary(idx)) => {
                response.secondary_suggestions.get(idx).cloned()
            }
            None if allow_unfocused_default => response.default_action.clone(),
            None => None,
        }?;
        self.suggested_replies.current = None;
        self.suggested_replies.focus = None;
        self.suggested_replies.explorer_deadline = None;
        Some(selected)
    }

    /// Consume a specific suggestion target (clicked by mouse or selected by index).
    pub fn take_suggested_reply_target(&mut self, target: SuggestedReplyFocus) -> Option<String> {
        if !self.suggested_replies_visible() {
            return None;
        }
        let response = self.suggested_replies.current.as_ref()?;
        let text = match target {
            SuggestedReplyFocus::DefaultAction => response.default_action.clone(),
            SuggestedReplyFocus::Secondary(idx) => {
                response.secondary_suggestions.get(idx).cloned()
            }
        }?;
        self.suggested_replies.current = None;
        self.suggested_replies.focus = None;
        self.suggested_replies.explorer_deadline = None;
        Some(text)
    }

    /// Submit a chosen Suggested Reply in local mode (or stage it for remote mode).
    pub fn submit_suggested_reply_prompt(&mut self, prompt: String) {
        let trimmed = prompt.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        self.input = trimmed.clone();
        self.cursor_pos = self.input.len();
        if self.is_remote {
            self.suggested_replies.pending_remote_submit = Some(trimmed.clone());
            self.pending_queued_dispatch = true;
            self.set_status_notice(format!("▸ {}", crate::util::truncate_str(&trimmed, 48)));
        } else {
            self.submit_input();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suggested_replies::{ScoredSuggestion, distribute_suggestions};

    #[test]
    fn distribution_and_focus_state_transitions_work_cleanly() {
        let resp = distribute_suggestions(
            "verification",
            vec![
                ScoredSuggestion {
                    text: "Run cargo test".to_string(),
                    confidence: 0.82,
                },
                ScoredSuggestion {
                    text: "Inspect git diff".to_string(),
                    confidence: 0.61,
                },
            ],
            0.55,
            5,
        );
        assert_eq!(resp.default_action.as_deref(), Some("Run cargo test"));
        assert_eq!(resp.secondary_suggestions, vec!["Inspect git diff"]);
    }

    #[test]
    fn app_tab_focuses_then_activates_default_action_and_esc_cancels_explorer() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.suggested_replies.force_enable_in_tests = true;
        app.push_display_message(super::super::DisplayMessage::assistant(
            "Implemented the feature.",
        ));

        let resp = distribute_suggestions(
            "implementation",
            vec![
                ScoredSuggestion {
                    text: "Run unit tests".to_string(),
                    confidence: 0.85,
                },
                ScoredSuggestion {
                    text: "Inspect git diff".to_string(),
                    confidence: 0.60,
                },
            ],
            0.55,
            5,
        );
        app.apply_suggested_replies_response(resp.clone());
        assert!(app.suggested_replies_visible());
        assert_eq!(app.suggested_replies.focus, None);

        // First Tab focuses DefaultAction without submitting yet.
        let tab1 = app.handle_suggested_replies_tab();
        assert_eq!(tab1, Some(None));
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::DefaultAction)
        );

        // Second Tab activates and returns the DefaultAction prompt.
        let tab2 = app.handle_suggested_replies_tab();
        assert_eq!(tab2, Some(Some("Run unit tests".to_string())));
        assert!(!app.suggested_replies_visible());

        // Re-apply and test Ctrl+Enter (unfocused immediate DefaultAction activation).
        app.apply_suggested_replies_response(resp.clone());
        assert_eq!(
            app.take_focused_or_default_suggested_reply(true),
            Some("Run unit tests".to_string())
        );
        assert!(!app.suggested_replies_visible());

        // Re-apply with simulated explorer deadline and test Esc cancellation.
        app.apply_suggested_replies_response(resp);
        app.suggested_replies.explorer_deadline =
            Some(Instant::now() - Duration::from_millis(10));
        let (_, expired_prompt) = app.poll_suggested_replies_tick();
        if crate::config::config()
            .suggested_replies
            .enable_full_auto_explorer
        {
            assert_eq!(expired_prompt, Some("Run unit tests".to_string()));
        } else {
            assert_eq!(expired_prompt, None);
        }

        // Verify cancel_suggested_replies_interaction clears deadline and suggestions.
        app.suggested_replies.explorer_deadline =
            Some(Instant::now() + Duration::from_millis(500));
        assert!(app.cancel_suggested_replies_interaction(true));
        assert!(app.suggested_replies.explorer_deadline.is_none());
        assert!(app.suggested_replies.current.is_none());
    }
}
