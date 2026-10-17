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

    /// Ordered list of selectable focus targets in the current Suggested Replies panel.
    fn ordered_suggested_reply_targets(&self) -> Vec<SuggestedReplyFocus> {
        let Some(response) = self.suggested_replies.current.as_ref() else {
            return Vec::new();
        };
        let mut targets = Vec::new();
        if response.default_action.is_some() {
            targets.push(SuggestedReplyFocus::DefaultAction);
        }
        for idx in 0..response.secondary_suggestions.len().min(5) {
            targets.push(SuggestedReplyFocus::Secondary(idx));
        }
        targets
    }

    /// Move focus forward (`delta = 1`) or backward (`delta = -1`) across
    /// `DefaultAction` and `Secondary(0..N)`.
    pub fn step_suggested_replies_focus(&mut self, delta: isize) -> bool {
        if !self.suggested_replies_visible() {
            return false;
        }
        let targets = self.ordered_suggested_reply_targets();
        if targets.is_empty() {
            return false;
        }
        self.suggested_replies.explorer_deadline = None;
        let next_pos = match self
            .suggested_replies
            .focus
            .and_then(|cur| targets.iter().position(|t| *t == cur))
        {
            Some(pos) => (pos as isize + delta).rem_euclid(targets.len() as isize) as usize,
            None => {
                if delta >= 0 {
                    0
                } else {
                    targets.len() - 1
                }
            }
        };
        let next_focus = targets[next_pos];
        self.suggested_replies.focus = Some(next_focus);
        match next_focus {
            SuggestedReplyFocus::DefaultAction => {
                self.set_status_notice(
                    "Default Action selected (Enter to send · ↑/↓/Tab to choose · 1-5 quick pick)",
                );
            }
            SuggestedReplyFocus::Secondary(idx) => {
                self.set_status_notice(format!(
                    "Suggestion #{} selected (Enter to send · ↑/↓/Tab to choose)",
                    idx + 1
                ));
            }
        }
        true
    }

    /// Handle `Up` / `Down` arrow keys when Suggested Replies are visible on an empty input.
    /// - `Down` always steps down through the vertical suggestion list (focusing the first item if unfocused).
    /// - `Up` steps up when focus is already inside the suggestion panel (leaving plain `Up` for prompt history when unfocused).
    pub fn handle_suggested_replies_arrow_key(
        &mut self,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        if !modifiers.is_empty() || !self.suggested_replies_visible() {
            return false;
        }
        match code {
            crossterm::event::KeyCode::Down => self.step_suggested_replies_focus(1),
            crossterm::event::KeyCode::Up if self.suggested_replies.focus.is_some() => {
                self.step_suggested_replies_focus(-1)
            }
            _ => false,
        }
    }

    /// Handle pressing digit `'1'..='5'` (or `'0'` for Default Action) on an empty input
    /// when Suggested Replies are visible. Returns `Some(prompt)` to submit immediately.
    pub fn take_suggested_reply_by_digit(&mut self, ch: char) -> Option<String> {
        if !self.suggested_replies_visible() {
            return None;
        }
        let target = match ch {
            '0' => SuggestedReplyFocus::DefaultAction,
            '1'..='5' => SuggestedReplyFocus::Secondary((ch as usize) - ('1' as usize)),
            _ => return None,
        };
        self.take_suggested_reply_target(target)
    }

    /// Handle `[Tab]` when `self.autocomplete()` returned `false` and input is empty.
    /// Cycles focus across `DefaultAction` and `Secondary(0..N)` so the user can pick any
    /// vertical suggestion with `[Tab]` and press `[Enter]` to send it.
    pub fn handle_suggested_replies_tab(&mut self) -> Option<Option<String>> {
        if self.step_suggested_replies_focus(1) {
            Some(None)
        } else {
            None
        }
    }

    /// Return the currently focused suggestion (or `default_action` when `allow_unfocused_default`
    /// is true) and consume the Suggested Replies panel so the caller can submit it.
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
            None if allow_unfocused_default => response
                .default_action
                .clone()
                .or_else(|| response.secondary_suggestions.first().cloned()),
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

        // First Tab focuses DefaultAction.
        let tab1 = app.handle_suggested_replies_tab();
        assert_eq!(tab1, Some(None));
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::DefaultAction)
        );

        // Second Tab steps focus to Secondary(0), and Up arrow returns to DefaultAction.
        let tab2 = app.handle_suggested_replies_tab();
        assert_eq!(tab2, Some(None));
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::Secondary(0))
        );
        assert!(app.handle_suggested_replies_arrow_key(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE
        ));
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::DefaultAction)
        );

        // Enter (take_focused_or_default_suggested_reply(false)) submits the focused item.
        assert_eq!(
            app.take_focused_or_default_suggested_reply(false),
            Some("Run unit tests".to_string())
        );
        assert!(!app.suggested_replies_visible());

        // Re-apply and test 1..5 digit quick selection.
        app.apply_suggested_replies_response(resp.clone());
        assert_eq!(
            app.take_suggested_reply_by_digit('1'),
            Some("Inspect git diff".to_string())
        );
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

    #[test]
    fn suggested_replies_panel_renders_vertically_one_row_per_item() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.suggested_replies.force_enable_in_tests = true;
        app.push_display_message(super::super::DisplayMessage::assistant(
            "Implemented the feature.",
        ));
        let resp = distribute_suggestions(
            "implementation",
            vec![
                ScoredSuggestion {
                    text: "Default action top".to_string(),
                    confidence: 0.90,
                },
                ScoredSuggestion {
                    text: "Secondary option 1".to_string(),
                    confidence: 0.70,
                },
                ScoredSuggestion {
                    text: "Secondary option 2".to_string(),
                    confidence: 0.65,
                },
                ScoredSuggestion {
                    text: "Secondary option 3".to_string(),
                    confidence: 0.60,
                },
            ],
            0.55,
            5,
        );
        app.apply_suggested_replies_response(resp);
        let view = crate::tui::TuiState::suggested_replies_view(&app).expect("view present");
        let expected_rows =
            u16::from(view.response.default_action.is_some()) + view.response.secondary_suggestions.len() as u16;
        assert_eq!(expected_rows, 4);
    }
}
