use super::App;
use crate::suggested_replies::{SuggestedRepliesContext, SuggestedRepliesResponse};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

mod six_lens;

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
    pub show_confidence: bool,
}

/// Runtime state owned by `App` for two-stage Suggested Replies and Explorer mode.
#[derive(Default)]
pub struct SuggestedRepliesRuntime {
    pub current: Option<SuggestedRepliesResponse>,
    pub focus: Option<SuggestedReplyFocus>,
    pub staged_input: Option<String>,
    pub explorer_deadline: Option<Instant>,
    pub pending_rx: Option<mpsc::UnboundedReceiver<SuggestedRepliesResponse>>,
    pub pending_request_target: Option<(String, u64)>,
    pub active_session_id: Option<String>,
    pub last_reviewed_edit_count: usize,
    pub pending_six_lens_findings: Option<String>,
    pub pending_remote_submit: Option<String>,
    pub force_enable_in_tests: bool,
    pub last_evaluated_messages_version: u64,
    pub last_debug: Option<crate::suggested_replies::SuggestedRepliesDebugInfo>,
    pub last_response: Option<SuggestedRepliesResponse>,
    pub cached_response: Option<(String, u64, SuggestedRepliesResponse)>,
}

fn truncate_preserving_tail(text: &str, max_bytes: usize) -> String {
    crate::suggested_replies::truncate_preserving_tail(text, max_bytes)
}

impl App {
    /// True when `self.input` currently holds an unedited suggestion staged by selecting
    /// a Suggested Reply item.
    pub fn suggested_replies_has_staged_input(&self) -> bool {
        !self.input.is_empty()
            && self.suggested_replies.staged_input.as_deref() == Some(self.input.as_str())
    }

    /// True when `self.input` is either empty or holds an unedited staged suggestion.
    pub fn suggested_replies_input_is_empty_or_staged(&self) -> bool {
        self.input.is_empty() || self.suggested_replies_has_staged_input()
    }

    /// Whether the Suggested Replies panel should be visible above the input box.
    pub fn suggested_replies_visible(&self) -> bool {
        if !crate::config::config().suggested_replies.enabled {
            return false;
        }
        let is_staged = self.suggested_replies_has_staged_input();
        if self.is_processing
            || self.pending_turn
            || !self.streaming.streaming_text.is_empty()
            || !self.stream_buffer.is_empty()
            || self.deferred_stream_done_id.is_some()
            || !self.streaming_tool_calls.is_empty()
            || (!self.input.is_empty() && !is_staged)
            || !self.pending_images.is_empty()
            || self.onboarding_welcome_active()
            || self.inline_interactive_state.is_some()
            || self.inline_view_state.is_some()
            || self.prompt_history_search.is_some()
            || (!is_staged && !self.command_suggestions().is_empty())
        {
            return false;
        }
        if self
            .suggested_replies
            .active_session_id
            .as_deref()
            .is_some_and(|sid| sid != super::commands::active_session_id(self))
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
        let show_confidence = crate::config::config().suggested_replies.show_confidence;
        Some(SuggestedRepliesView {
            response,
            focus: self.suggested_replies.focus,
            explorer_remaining_ms,
            show_confidence,
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
        if self.suggested_replies.staged_input.take().is_some() {
            changed = true;
        }
        if clear_suggestions {
            if self.suggested_replies.current.take().is_some() {
                changed = true;
            }
            if self.suggested_replies.pending_rx.take().is_some() {
                changed = true;
            }
            self.suggested_replies.pending_request_target = None;
        }
        changed
    }

    /// Clear any active or in-flight Suggested Replies when a new turn begins
    /// so stale suggestions from a previous turn never reappear on Esc/cancel
    /// or overwrite the next turn's state.
    pub fn clear_suggested_replies_for_new_turn(&mut self) {
        self.cancel_suggested_replies_interaction(true);
    }

    /// Invalidate all active, staged, or in-flight suggestions when switching sessions
    /// so suggestions or Explorer timers from the previous session never leak across sessions.
    pub fn invalidate_suggested_replies_on_session_switch(&mut self) {
        if self.suggested_replies_has_staged_input() {
            self.input.clear();
            self.cursor_pos = 0;
        }
        self.cancel_suggested_replies_interaction(true);
        self.suggested_replies.active_session_id = None;
        self.suggested_replies.pending_six_lens_findings = None;
        self.suggested_replies.pending_remote_submit = None;
        self.suggested_replies.last_reviewed_edit_count = self.display_edit_tool_message_count;
    }

    /// Apply a freshly computed `SuggestedRepliesResponse`, arming the 500ms
    /// Full-Auto Explorer timer when `enable_full_auto_explorer` is enabled and
    /// a `default_action` cleared the confidence threshold.
    /// If the user already focused a suggestion (`self.suggested_replies.focus.is_some()`),
    /// preserve a valid focus target instead of resetting focus to `None`.
    pub fn apply_suggested_replies_response(&mut self, response: SuggestedRepliesResponse) {
        if let Some(debug) = response.debug.clone() {
            self.suggested_replies.last_debug = Some(debug);
        }
        if response.is_empty() {
            self.suggested_replies.current = None;
            self.suggested_replies.focus = None;
            self.suggested_replies.explorer_deadline = None;
            return;
        }
        self.suggested_replies.last_response = Some(response.clone());
        let session_id = super::commands::active_session_id(self);
        self.suggested_replies.active_session_id = Some(session_id.clone());
        self.suggested_replies.cached_response =
            Some((session_id, self.display_messages_version, response.clone()));

        let preserved_focus = if self.suggested_replies.current.is_some() {
            match self.suggested_replies.focus {
                Some(SuggestedReplyFocus::DefaultAction) => {
                    if response.default_action.is_some() {
                        Some(SuggestedReplyFocus::DefaultAction)
                    } else if !response.secondary_suggestions.is_empty() {
                        Some(SuggestedReplyFocus::Secondary(0))
                    } else {
                        None
                    }
                }
                Some(SuggestedReplyFocus::Secondary(idx)) => {
                    let count = response.secondary_suggestions.len();
                    if count > 0 {
                        Some(SuggestedReplyFocus::Secondary(idx.min(count - 1)))
                    } else if response.default_action.is_some() {
                        Some(SuggestedReplyFocus::DefaultAction)
                    } else {
                        None
                    }
                }
                None => None,
            }
        } else {
            None
        };

        let had_staged_or_empty = self.suggested_replies_input_is_empty_or_staged();
        let cfg = &crate::config::config().suggested_replies;
        let can_arm_explorer = cfg.enable_full_auto_explorer
            && preserved_focus.is_none()
            && response.default_action.is_some()
            && !self.is_processing
            && self.input.is_empty()
            && self.pending_images.is_empty()
            && self.queued_messages.is_empty()
            && !self.autoreview_enabled
            && !self.autojudge_enabled
            && !self.pending_split_request;

        self.suggested_replies.focus = preserved_focus;
        self.suggested_replies.explorer_deadline =
            can_arm_explorer.then(|| Instant::now() + EXPLORER_AUTO_DELAY);
        self.suggested_replies.current = Some(response);
        if had_staged_or_empty {
            if let Some(target) = preserved_focus
                && let Some(text) = self.suggested_reply_text_for_target(target)
            {
                self.input = text.clone();
                self.cursor_pos = self.input.len();
                self.suggested_replies.staged_input = Some(text);
            } else if self.suggested_replies.staged_input.take().is_some() {
                self.input.clear();
                self.cursor_pos = 0;
            }
        }
    }

    /// Build a compact recent-conversation snapshot for the two-stage cascade.
    fn recent_transcript_for_suggestions(&self) -> String {
        let conversational: Vec<&super::DisplayMessage> = self
            .display_messages
            .iter()
            .filter(|msg| {
                matches!(msg.role.as_str(), "user" | "assistant" | "swarm")
                    && !msg.content.trim().is_empty()
            })
            .collect();
        if conversational.is_empty() {
            return String::new();
        }
        let window_start = conversational.len().saturating_sub(6);
        let mut selected: Vec<&super::DisplayMessage> = Vec::with_capacity(7);
        if let Some(&latest_user) = conversational[..window_start]
            .iter()
            .rev()
            .find(|msg| msg.role == "user")
            && !conversational[window_start..]
                .iter()
                .any(|msg| msg.role == "user")
        {
            selected.push(latest_user);
        }
        selected.extend_from_slice(&conversational[window_start..]);

        let mut buf = String::new();
        let total = selected.len();
        for (idx, msg) in selected.into_iter().enumerate() {
            let max_bytes = if idx + 1 == total { 2400 } else { 1200 };
            buf.push_str(&format!(
                "[{}]: {}\n\n",
                msg.role,
                truncate_preserving_tail(&msg.content, max_bytes)
            ));
        }
        buf
    }

    /// Build the structured `SuggestedRepliesContext` from `display_messages` and active session `todos`.
    pub fn build_suggested_replies_context(&self) -> SuggestedRepliesContext {
        let transcript_tail = self.recent_transcript_for_suggestions();
        let pending_todos: Vec<String> = super::commands::incomplete_poke_todos(self)
            .into_iter()
            .map(|t| t.content)
            .collect();
        let mut ctx = SuggestedRepliesContext::from_transcript(&transcript_tail)
            .with_pending_todos(pending_todos);
        if let Some(last_assistant) = self
            .display_messages
            .iter()
            .rev()
            .find(|msg| msg.role == "assistant" && !msg.content.trim().is_empty())
        {
            ctx.last_assistant_message =
                Some(truncate_preserving_tail(&last_assistant.content, 2400));
            ctx.assistant_options =
                crate::suggested_replies::extract_assistant_options(&last_assistant.content);
        }
        if let Some(last_user) = self
            .display_messages
            .iter()
            .rev()
            .find(|msg| msg.role == "user" && !msg.content.trim().is_empty())
        {
            ctx.language_is_cyrillic =
                crate::suggested_replies::detect_cyrillic_language(&last_user.content);
            if ctx.last_user_message.is_none() {
                ctx.last_user_message = Some(truncate_preserving_tail(&last_user.content, 900));
            }
        }
        ctx
    }

    pub(super) fn latest_six_lens_findings_from_display(&self) -> Option<String> {
        for msg in self.display_messages.iter().rev() {
            if msg.role == "user" {
                break;
            }
            if matches!(msg.role.as_str(), "swarm" | "assistant" | "system")
                && let Some(findings) =
                    crate::suggested_replies::extract_six_lens_review_findings(&msg.content)
            {
                return Some(findings);
            }
        }
        None
    }

    pub(super) fn maybe_ingest_six_lens_review_message(&mut self, message: &super::DisplayMessage) {
        if message.role == "user" {
            self.suggested_replies.pending_six_lens_findings = None;
            return;
        }
        if matches!(message.role.as_str(), "swarm" | "assistant" | "system")
            && let Some(findings) =
                crate::suggested_replies::extract_six_lens_review_findings(&message.content)
        {
            self.record_six_lens_review_findings(findings);
        }
    }

    pub(super) fn maybe_ingest_latest_six_lens_review_from_history(&mut self) {
        if let Some(findings) = self.latest_six_lens_findings_from_display() {
            self.record_six_lens_review_findings(findings);
        }
    }

    /// Trigger the two-stage Suggested Replies cascade at the end of a turn.
    pub fn maybe_trigger_suggested_replies_at_turn_end(&mut self) {
        self.trigger_suggested_replies_evaluation(false);
    }

    /// Trigger the two-stage Suggested Replies cascade, optionally bypassing the
    /// `(session_id, display_messages_version)` response cache (`force_refresh = true`).
    pub fn trigger_suggested_replies_evaluation(&mut self, force_refresh: bool) {
        // In TestHarness / Replay mode, do not auto-populate inline suggestions
        // unless the test explicitly opted in via `force_enable_in_tests`, so
        // existing viewport-height / anchor-stability snapshot tests stay untouched.
        if (cfg!(test) || self.is_replay || self.runtime_mode == super::AppRuntimeMode::TestHarness)
            && !self.suggested_replies.force_enable_in_tests
        {
            return;
        }

        // Clear any leftover suggestions from a previous turn before evaluating.
        self.clear_suggested_replies_for_new_turn();

        let cfg = crate::config::config().suggested_replies.clone();
        if !cfg.enabled
            || self.is_processing
            || self.pending_turn
            || !self.streaming.streaming_text.is_empty()
            || !self.stream_buffer.is_empty()
            || self.deferred_stream_done_id.is_some()
            || !self.streaming_tool_calls.is_empty()
            || !self.queued_messages.is_empty()
            || !self.hidden_queued_system_messages.is_empty()
            || self.pending_queued_dispatch
            || self.has_pending_user_followup()
        {
            return;
        }
        if !force_refresh
            && self
                .display_messages
                .last()
                .is_some_and(|m| m.role == "system" && m.content.trim() == "Interrupted")
        {
            return;
        }

        if force_refresh {
            self.suggested_replies.cached_response = None;
        }

        let session_id = super::commands::active_session_id(self);
        let six_lens_findings = self
            .suggested_replies
            .pending_six_lens_findings
            .clone()
            .or_else(|| self.latest_six_lens_findings_from_display());
        if !force_refresh
            && six_lens_findings.is_none()
            && let Some((ref cached_sid, cached_ver, ref cached_resp)) =
                self.suggested_replies.cached_response
            && *cached_sid == session_id
            && cached_ver == self.display_messages_version
        {
            let restored = cached_resp.clone();
            self.apply_suggested_replies_response(restored);
            return;
        }

        let ctx = self.build_suggested_replies_context();
        if ctx.transcript_tail.trim().is_empty() {
            return;
        }
        self.suggested_replies.last_evaluated_messages_version = self.display_messages_version;
        self.suggested_replies.active_session_id = Some(session_id.clone());

        // When an async OpenGF / Jev (Laya) cascade route is active in a live session,
        // wait for the real cascade result instead of flashing canned local English
        // templates for 150ms and then replacing/demoting `default_action` under the user's cursor.
        if !cfg!(test)
            && !self.is_replay
            && self.runtime_mode != super::AppRuntimeMode::TestHarness
            && six_lens_findings.is_none()
            && (cfg.use_open_gf || crate::jev::JevClient::suggestions_available())
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            let (tx, rx) = mpsc::unbounded_channel();
            self.suggested_replies.pending_request_target =
                Some((session_id, self.display_messages_version));
            self.suggested_replies.pending_rx = Some(rx);
            handle.spawn(async move {
                let response = crate::suggested_replies::evaluate_cascade_with_context(
                    &ctx,
                    &cfg,
                    six_lens_findings.as_deref(),
                )
                .await;
                drop(tx.send(response));
            });
            return;
        }

        let immediate = crate::suggested_replies::score_locally_with_context(
            &ctx,
            None,
            &cfg.sanitized_custom_prompts(),
            six_lens_findings.as_deref(),
            cfg.clamped_confidence_threshold(),
            cfg.clamped_max_secondary(),
        );
        self.apply_suggested_replies_response(immediate);
    }

    /// Record findings from a 6-Lens Subagent Self-Review and immediately
    /// surface a high-confidence Default Action in Suggested Replies.
    pub fn record_six_lens_review_findings(&mut self, summary: String) {
        let trimmed = summary.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        self.suggested_replies.cached_response = None;
        self.suggested_replies.pending_six_lens_findings = Some(trimmed.clone());
        let cfg = &crate::config::config().suggested_replies;
        let ctx = self.build_suggested_replies_context();
        let response = crate::suggested_replies::score_locally_with_context(
            &ctx,
            None,
            &cfg.sanitized_custom_prompts(),
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
        let current_sid = super::commands::active_session_id(self);

        if self
            .suggested_replies
            .active_session_id
            .as_deref()
            .is_some_and(|sid| sid != current_sid)
        {
            self.invalidate_suggested_replies_on_session_switch();
            needs_redraw = true;
        }

        if let Some(rx) = self.suggested_replies.pending_rx.as_mut() {
            match rx.try_recv() {
                Ok(response) => {
                    self.suggested_replies.pending_rx = None;
                    let target_matches = match self.suggested_replies.pending_request_target.take()
                    {
                        Some((req_sid, req_ver)) => {
                            req_sid == current_sid && req_ver == self.display_messages_version
                        }
                        None => {
                            self.suggested_replies.last_evaluated_messages_version
                                == self.display_messages_version
                                && self
                                    .suggested_replies
                                    .active_session_id
                                    .as_deref()
                                    .is_none_or(|sid| sid == current_sid)
                        }
                    };
                    if target_matches
                        && !self.is_processing
                        && !self.pending_turn
                        && self.streaming.streaming_text.is_empty()
                        && self.stream_buffer.is_empty()
                        && self.deferred_stream_done_id.is_none()
                        && self.streaming_tool_calls.is_empty()
                        && self.suggested_replies_input_is_empty_or_staged()
                    {
                        self.apply_suggested_replies_response(response);
                        needs_redraw = true;
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.suggested_replies.pending_rx = None;
                    self.suggested_replies.pending_request_target = None;
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
                    self.suggested_replies.staged_input = None;
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
        for idx in 0..response.secondary_suggestions.len() {
            targets.push(SuggestedReplyFocus::Secondary(idx));
        }
        targets
    }

    /// Look up the suggestion text for `target` in the current response.
    pub fn suggested_reply_text_for_target(&self, target: SuggestedReplyFocus) -> Option<String> {
        let response = self.suggested_replies.current.as_ref()?;
        match target {
            SuggestedReplyFocus::DefaultAction => response.default_action.clone(),
            SuggestedReplyFocus::Secondary(idx) => response.secondary_suggestions.get(idx).cloned(),
        }
    }

    /// Focus `target` and populate `self.input` (composer) with its text so the user
    /// can immediately edit it, press `Enter` to send, or keep cycling with `Tab`/`↑`/`↓`.
    pub fn stage_suggested_reply_target(&mut self, target: SuggestedReplyFocus) -> bool {
        if !self.suggested_replies_visible() {
            return false;
        }
        let Some(text) = self.suggested_reply_text_for_target(target) else {
            return false;
        };
        self.suggested_replies.explorer_deadline = None;
        self.suggested_replies.focus = Some(target);
        if self.input != text {
            self.remember_input_undo_state();
            self.input = text.clone();
            self.cursor_pos = self.input.len();
        }
        self.suggested_replies.staged_input = Some(text);
        match target {
            SuggestedReplyFocus::DefaultAction => {
                self.set_status_notice(
                    "Default Action in input (press 0 or Enter to send · ↑/↓/Tab to switch · Esc to clear)",
                );
            }
            SuggestedReplyFocus::Secondary(idx) => {
                let repeat_hint = if idx < 9 {
                    format!("press {} again or Enter to send", idx + 1)
                } else {
                    "edit or Enter to send".to_string()
                };
                self.set_status_notice(format!(
                    "Suggestion #{} in input ({} · ↑/↓/Tab to switch · Esc to clear)",
                    idx + 1,
                    repeat_hint
                ));
            }
        }
        true
    }

    /// Move focus forward (`delta = 1`) or backward (`delta = -1`) across
    /// `DefaultAction` and `Secondary(0..N)` and populate `self.input` with the
    /// selected suggestion text for editing.
    pub fn step_suggested_replies_focus(&mut self, delta: isize) -> bool {
        if !self.suggested_replies_visible() {
            return false;
        }
        let targets = self.ordered_suggested_reply_targets();
        if targets.is_empty() {
            return false;
        }
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
        self.stage_suggested_reply_target(next_focus)
    }

    /// Handle `Up` / `Down` arrow keys when Suggested Replies are visible.
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

    /// Handle pressing digit `'1'..='9'` (or `'0'` for Default Action) on an empty or staged input
    /// when Suggested Replies are visible: focuses that suggestion and populates `self.input`
    /// so the user can edit it or press the same digit / `Enter` to send.
    pub fn stage_suggested_reply_by_digit(&mut self, ch: char) -> bool {
        if !self.suggested_replies_input_is_empty_or_staged() || !self.suggested_replies_visible() {
            return false;
        }
        let target = match ch {
            '0' => SuggestedReplyFocus::DefaultAction,
            '1'..='9' => SuggestedReplyFocus::Secondary((ch as usize) - ('1' as usize)),
            _ => return false,
        };
        self.stage_suggested_reply_target(target)
    }

    /// Handle pressing digit `'1'..='9'` (or `'0'` for Default Action) when Suggested Replies
    /// are visible and the input is empty or holds an unedited staged suggestion:
    /// - First press on `target`: focuses `target` and stages its text into `self.input`.
    /// - Second press of the same digit while `target` is already focused and staged:
    ///   consumes and submits the suggestion (`submit_suggested_reply_prompt`).
    pub fn handle_suggested_reply_digit_press(&mut self, ch: char) -> bool {
        if !self.suggested_replies_input_is_empty_or_staged() || !self.suggested_replies_visible() {
            return false;
        }
        let target = match ch {
            '0' => SuggestedReplyFocus::DefaultAction,
            '1'..='9' => SuggestedReplyFocus::Secondary((ch as usize) - ('1' as usize)),
            _ => return false,
        };
        if self.suggested_replies.focus == Some(target)
            && self.suggested_replies_has_staged_input()
            && let Some(prompt) = self.take_suggested_reply_target(target)
        {
            self.submit_suggested_reply_prompt(prompt);
            return true;
        }
        self.stage_suggested_reply_target(target)
    }

    /// Handle pressing digit `'1'..='9'` (or `'0'` for Default Action) to consume and return
    /// the suggestion text directly.
    pub fn take_suggested_reply_by_digit(&mut self, ch: char) -> Option<String> {
        if !self.suggested_replies_visible() {
            return None;
        }
        let target = match ch {
            '0' => SuggestedReplyFocus::DefaultAction,
            '1'..='9' => SuggestedReplyFocus::Secondary((ch as usize) - ('1' as usize)),
            _ => return None,
        };
        self.take_suggested_reply_target(target)
    }

    /// Handle `[Tab]` when Suggested Replies are visible.
    /// Cycles focus across `DefaultAction` and `Secondary(0..N)` and fills `self.input`
    /// with the selected suggestion so the user can edit it or press `[Enter]` to send.
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
        self.suggested_replies.staged_input = None;
        self.suggested_replies.explorer_deadline = None;
        Some(selected)
    }

    /// Consume a specific suggestion target (clicked by mouse or selected by index).
    pub fn take_suggested_reply_target(&mut self, target: SuggestedReplyFocus) -> Option<String> {
        if !self.suggested_replies_visible() {
            return None;
        }
        let text = self.suggested_reply_text_for_target(target)?;
        self.suggested_replies.current = None;
        self.suggested_replies.focus = None;
        self.suggested_replies.staged_input = None;
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
            self.set_status_notice(format!(
                "▸ {}",
                crate::tui::ui::inline_ui::truncate_to_display_width(&trimmed, 72)
            ));
        } else {
            self.submit_input();
        }
    }
}

#[cfg(test)]
mod tests;
