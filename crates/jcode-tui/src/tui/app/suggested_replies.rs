use super::App;
use crate::suggested_replies::{SuggestedRepliesContext, SuggestedRepliesResponse};
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
    pub pending_six_lens_findings: Option<String>,
    pub pending_remote_submit: Option<String>,
    pub force_enable_in_tests: bool,
    pub last_evaluated_messages_version: u64,
    pub last_debug: Option<crate::suggested_replies::SuggestedRepliesDebugInfo>,
    pub last_response: Option<SuggestedRepliesResponse>,
    pub cached_response: Option<(String, u64, SuggestedRepliesResponse)>,
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
        }
        changed
    }

    /// Clear any active or in-flight Suggested Replies when a new turn begins
    /// so stale suggestions from a previous turn never reappear on Esc/cancel
    /// or overwrite the next turn's state.
    pub fn clear_suggested_replies_for_new_turn(&mut self) {
        self.cancel_suggested_replies_interaction(true);
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

    /// Build the structured `SuggestedRepliesContext` from `display_messages` and active session `todos`.
    pub fn build_suggested_replies_context(&self) -> SuggestedRepliesContext {
        let transcript_tail = self.recent_transcript_for_suggestions();
        let pending_todos: Vec<String> = super::commands::incomplete_poke_todos(self)
            .into_iter()
            .map(|t| t.content)
            .collect();
        SuggestedRepliesContext::from_transcript(&transcript_tail)
            .with_pending_todos(pending_todos)
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
        if (cfg!(test)
            || self.is_replay
            || self.runtime_mode == super::AppRuntimeMode::TestHarness)
            && !self.suggested_replies.force_enable_in_tests
        {
            return;
        }

        // Clear any leftover suggestions from a previous turn before evaluating.
        self.clear_suggested_replies_for_new_turn();

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

        if force_refresh {
            self.suggested_replies.cached_response = None;
        }

        let session_id = super::commands::active_session_id(self);
        if !force_refresh
            && self.suggested_replies.pending_six_lens_findings.is_none()
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
        let six_lens_findings = self.suggested_replies.pending_six_lens_findings.take();

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
            self.suggested_replies.pending_rx = Some(rx);
            handle.spawn(async move {
                let response = crate::suggested_replies::evaluate_cascade_with_context(
                    &ctx,
                    &cfg,
                    six_lens_findings.as_deref(),
                )
                .await;
                let _ = tx.send(response);
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

        if let Some(rx) = self.suggested_replies.pending_rx.as_mut() {
            match rx.try_recv() {
                Ok(response) => {
                    self.suggested_replies.pending_rx = None;
                    if !self.is_processing && self.suggested_replies_input_is_empty_or_staged() {
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
            SuggestedReplyFocus::Secondary(idx) => {
                response.secondary_suggestions.get(idx).cloned()
            }
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
                    "Default Action in input (edit or Enter to send · ↑/↓/Tab to switch · Esc to clear)",
                );
            }
            SuggestedReplyFocus::Secondary(idx) => {
                self.set_status_notice(format!(
                    "Suggestion #{} in input (edit or Enter to send · ↑/↓/Tab to switch · Esc to clear)",
                    idx + 1
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

    /// Handle pressing digit `'1'..='9'` (or `'0'` for Default Action) on an empty input
    /// when Suggested Replies are visible: focuses that suggestion and populates `self.input`
    /// so the user can edit it or press `Enter` to send.
    pub fn stage_suggested_reply_by_digit(&mut self, ch: char) -> bool {
        if !self.input.is_empty() || !self.suggested_replies_visible() {
            return false;
        }
        let target = match ch {
            '0' => SuggestedReplyFocus::DefaultAction,
            '1'..='9' => SuggestedReplyFocus::Secondary((ch as usize) - ('1' as usize)),
            _ => return false,
        };
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
                crate::tui::ui::truncate_to_display_width(&trimmed, 72)
            ));
        } else {
            self.submit_input();
        }
    }

    /// Build a human-readable diagnostic summary of `[suggested_replies]` configuration,
    /// live Jev / OpenGF routes, and the most recent cascade evaluation.
    pub fn format_suggested_replies_debug_report(&self) -> String {
        let cfg = &crate::config::config().suggested_replies;
        let sidecar = crate::sidecar::Sidecar::new();
        let jev_route = match crate::jev::JevClient::for_suggestions_with_selector(&cfg.jev_provider)
        {
            Ok(client) => format!(
                "ready (selector=`{}`, supports_choice={})",
                cfg.jev_provider,
                client.supports_choice_questions()
            ),
            Err(err) => format!("unavailable (`{}`: {})", cfg.jev_provider, err),
        };
        let ctx = self.build_suggested_replies_context();

        let mut out = String::new();
        out.push_str("### Suggested Replies Diagnostics\n\n");
        out.push_str(&format!(
            "- **Config**: `enabled={}`, `jev_provider=\"{}\"`, `use_open_gf={}`, `confidence_threshold={:.2}`, `max_secondary_suggestions={}` (clamped `{}`), `show_confidence={}`, `custom_prompts={}`, `enable_full_auto_explorer={}`, `six_lens_auto_review={}`\n",
            cfg.enabled,
            cfg.jev_provider,
            cfg.use_open_gf,
            cfg.clamped_confidence_threshold(),
            cfg.max_secondary_suggestions,
            cfg.clamped_max_secondary(),
            cfg.show_confidence,
            cfg.sanitized_custom_prompts().len(),
            cfg.enable_full_auto_explorer,
            cfg.six_lens_auto_review,
        ));
        out.push_str(&format!(
            "- **OpenGF Primary Sidecar**: `backend={}`, `model={}` (plus fallback to active provider / configured `[providers.*]` profiles)\n",
            sidecar.backend_name(),
            sidecar.model_name(),
        ));
        out.push_str(&format!("- **Jev / SystemOne Route**: {}\n", jev_route));
        out.push_str(&format!(
            "- **Structured Context**: `language={}`, `assistant_options={}`, `pending_todos={}`\n",
            if ctx.language_is_cyrillic { "ru" } else { "en" },
            ctx.assistant_options.len(),
            ctx.pending_todos.len(),
        ));
        out.push_str(&format!(
            "- **UI State**: `visible={}`, `in_flight={}`, `focus={:?}`, `cached={}`\n",
            self.suggested_replies_visible(),
            self.suggested_replies.pending_rx.is_some(),
            self.suggested_replies.focus,
            self.suggested_replies.cached_response.is_some(),
        ));

        if let Some(debug) = self.suggested_replies.last_debug.as_ref() {
            out.push_str("\n#### Last Cascade Telemetry\n");
            out.push_str(&format!(
                "- **Source**: `{}` ({}ms)\n- **OpenGF**: {}\n- **Jev / Laya**: {}\n- **Top Confidence vs Threshold**: `{:?}` vs `{:.2}`\n",
                debug.source,
                debug.elapsed_ms,
                debug.open_gf_status,
                debug.jev_status,
                debug.top_confidence,
                debug.threshold,
            ));
        }

        let resp_ref = self
            .suggested_replies
            .current
            .as_ref()
            .or(self.suggested_replies.last_response.as_ref());
        if let Some(resp) = resp_ref {
            out.push_str(&format!(
                "\n#### Latest Suggestions (`branch = {}`)\n",
                resp.branch
            ));
            out.push_str(&format!(
                "- **Default Action (`★`)**: {}\n",
                resp.default_action
                    .as_deref()
                    .map(|s| format!("`{s}`"))
                    .unwrap_or_else(|| "_None (below threshold)_".to_string())
            ));
            if !resp.scored_candidates.is_empty() {
                out.push_str("- **Scored Candidates**:\n");
                for (i, cand) in resp.scored_candidates.iter().enumerate() {
                    out.push_str(&format!(
                        "  {}. `{:.2}` — {}\n",
                        i + 1,
                        cand.confidence,
                        cand.text
                    ));
                }
            }
        } else {
            out.push_str("\n_No cascade evaluation recorded yet in this session. Run `/suggestions test` to trigger one now._\n");
        }
        out
    }

    /// Handle `/suggestions` (`status` | `debug` | `test` | `refresh`) slash command.
    pub fn handle_suggestions_command(&mut self, trimmed: &str) -> bool {
        let rest = if let Some(r) = trimmed.strip_prefix("/suggestions") {
            if !r.is_empty() && !r.starts_with(char::is_whitespace) {
                return false;
            }
            r.trim()
        } else if let Some(r) = trimmed.strip_prefix("/suggested-replies") {
            if !r.is_empty() && !r.starts_with(char::is_whitespace) {
                return false;
            }
            r.trim()
        } else {
            return false;
        };

        match rest {
            "" | "status" | "debug" => {
                let report = self.format_suggested_replies_debug_report();
                self.push_display_message(super::DisplayMessage::system(report));
                true
            }
            "test" | "refresh" => {
                self.trigger_suggested_replies_evaluation(true);
                self.set_status_notice("Re-evaluating Suggested Replies cascade...");
                self.push_display_message(super::DisplayMessage::system(
                    "Triggered Suggested Replies cascade evaluation. Run `/suggestions debug` once complete to inspect telemetry.".to_string(),
                ));
                true
            }
            _ => {
                self.push_display_message(super::DisplayMessage::error(
                    "Usage: `/suggestions [debug|status|test|refresh]`".to_string(),
                ));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suggested_replies::{
        ScoredSuggestion, SuggestedRepliesContext, distribute_suggestions,
        score_locally_with_context,
    };

    fn render_panel_lines(view: &SuggestedRepliesView, width: u16, height: u16) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                let area = ratatui::layout::Rect::new(0, 0, width, height);
                crate::tui::ui::draw_suggested_replies_panel(frame, view, area);
            })
            .expect("draw suggested replies panel");
        let buf = terminal.backend().buffer();
        let mut rows = Vec::new();
        for y in 0..height {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(buf[(x, y)].symbol());
            }
            let trimmed = line.trim_end().to_string();
            if !trimmed.is_empty() {
                rows.push(trimmed);
            }
        }
        rows
    }

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
            3,
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
            3,
        );
        app.apply_suggested_replies_response(resp.clone());
        assert!(app.suggested_replies_visible());
        assert_eq!(app.suggested_replies.focus, None);

        // First Tab focuses DefaultAction and populates app.input for editing.
        let tab1 = app.handle_suggested_replies_tab();
        assert_eq!(tab1, Some(None));
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::DefaultAction)
        );
        assert_eq!(app.input, "Run unit tests");
        assert_eq!(app.cursor_pos, app.input.len());
        assert!(app.suggested_replies_visible());

        // Second Tab steps focus to Secondary(0) and updates app.input; Up arrow returns to DefaultAction.
        let tab2 = app.handle_suggested_replies_tab();
        assert_eq!(tab2, Some(None));
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::Secondary(0))
        );
        assert_eq!(app.input, "Inspect git diff");
        assert!(app.suggested_replies_visible());
        assert!(app.handle_suggested_replies_arrow_key(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE
        ));
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::DefaultAction)
        );
        assert_eq!(app.input, "Run unit tests");

        // Enter (take_focused_or_default_suggested_reply(false)) submits the focused item.
        assert_eq!(
            app.take_focused_or_default_suggested_reply(false),
            Some("Run unit tests".to_string())
        );
        assert!(!app.suggested_replies_visible());

        // Re-apply and test digit selection populating app.input for editing.
        app.input.clear();
        app.cursor_pos = 0;
        app.apply_suggested_replies_response(resp.clone());
        assert!(app.stage_suggested_reply_by_digit('1'));
        assert_eq!(app.input, "Inspect git diff");
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::Secondary(0))
        );
        assert!(app.suggested_replies_visible());
        assert_eq!(
            app.take_focused_or_default_suggested_reply(false),
            Some("Inspect git diff".to_string())
        );
        assert!(!app.suggested_replies_visible());

        // Re-apply and test Ctrl+Enter (unfocused immediate DefaultAction activation).
        app.input.clear();
        app.cursor_pos = 0;
        app.apply_suggested_replies_response(resp.clone());
        assert_eq!(
            app.take_focused_or_default_suggested_reply(true),
            Some("Run unit tests".to_string())
        );
        assert!(!app.suggested_replies_visible());

        // Re-apply with simulated explorer deadline and test Esc cancellation.
        app.input.clear();
        app.cursor_pos = 0;
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
    fn tui_renders_and_navigates_exact_suggestion_counts_for_1_3_and_20() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.suggested_replies.force_enable_in_tests = true;
        app.push_display_message(super::super::DisplayMessage::user(
            "Проверь работу подсказок при 1, 3 и 20 элементах",
        ));
        app.push_display_message(super::super::DisplayMessage::assistant(
            "Готово! Выберите действие:\n1. Запустить тесты\n2. Проверить рендеринг",
        ));

        let ctx = SuggestedRepliesContext::from_transcript(
            "[user]: Проверь работу подсказок при 1, 3 и 20 элементах\n\n[assistant]: Готово! Выберите действие:\n1. Запустить тесты\n2. Проверить рендеринг",
        );

        for count in [1usize, 3usize, 20usize] {
            app.input.clear();
            app.cursor_pos = 0;
            app.cancel_suggested_replies_interaction(true);

            let resp = score_locally_with_context(
                &ctx,
                None,
                &["Постоянный вопрос пользователя".to_string()],
                None,
                0.55,
                count,
            );
            assert!(resp.default_action.is_some());
            assert_eq!(
                resp.secondary_suggestions.len(),
                count,
                "cascade must return {count} secondary suggestions"
            );

            app.apply_suggested_replies_response(resp);
            let mut view =
                crate::tui::TuiState::suggested_replies_view(&app).expect("view present");
            view.show_confidence = true;

            let expected_total_rows = 1 + count;
            let height = crate::tui::ui::inline_ui_height(&app) as usize;
            assert_eq!(
                height, expected_total_rows,
                "inline_ui_height must equal 1 (Default Action) + {count} secondary rows"
            );

            let rendered_rows = render_panel_lines(&view, 120, height as u16 + 2);
            assert_eq!(
                rendered_rows.len(),
                expected_total_rows,
                "TUI panel must render {expected_total_rows} rows for count={count}"
            );
            assert!(
                rendered_rows[0].contains('★') && rendered_rows[0].contains("%)"),
                "row 0 must be Default Action with confidence badge: {}",
                rendered_rows[0]
            );
            for i in 1..=count {
                let row = &rendered_rows[i];
                let expected_prefix = format!("{i}. ");
                assert!(
                    row.contains(&expected_prefix),
                    "row {i} must contain prefix '{expected_prefix}', got: {row}"
                );
                assert!(
                    row.contains("%)"),
                    "row {i} must display model confidence percentage when show_confidence=true, got: {row}"
                );
            }

            // When show_confidence = false, confidence percentages are omitted from all rows.
            view.show_confidence = false;
            let rows_no_conf = render_panel_lines(&view, 120, height as u16 + 2);
            assert_eq!(rows_no_conf.len(), expected_total_rows);
            for row in &rows_no_conf {
                assert!(
                    !row.contains("%)"),
                    "confidence badge must be hidden when show_confidence=false: {row}"
                );
            }

            // Verify Tab cycling visits all 1 + count items (DefaultAction + Secondary(0..count-1)).
            let targets = app.ordered_suggested_reply_targets();
            assert_eq!(targets.len(), expected_total_rows);
            for expected_target in targets {
                assert!(app.step_suggested_replies_focus(1));
                assert_eq!(app.suggested_replies.focus, Some(expected_target));
                assert!(!app.input.is_empty());
            }
        }
    }

    #[test]
    fn apply_response_preserves_existing_focus_and_version_cache_restores_or_refreshes() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.suggested_replies.force_enable_in_tests = true;
        app.push_display_message(super::super::DisplayMessage::assistant("Done."));

        let initial = distribute_suggestions(
            "implementation",
            vec![
                ScoredSuggestion {
                    text: "Initial default".to_string(),
                    confidence: 0.85,
                },
                ScoredSuggestion {
                    text: "Initial secondary 1".to_string(),
                    confidence: 0.65,
                },
            ],
            0.55,
            3,
        );
        app.apply_suggested_replies_response(initial.clone());
        app.step_suggested_replies_focus(1);
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::DefaultAction)
        );

        // An updated response while the panel is already visible preserves focus instead of resetting to None.
        let updated = distribute_suggestions(
            "verification",
            vec![
                ScoredSuggestion {
                    text: "Updated default".to_string(),
                    confidence: 0.88,
                },
                ScoredSuggestion {
                    text: "Updated secondary 1".to_string(),
                    confidence: 0.62,
                },
            ],
            0.55,
            3,
        );
        app.apply_suggested_replies_response(updated.clone());
        assert_eq!(
            app.suggested_replies.focus,
            Some(SuggestedReplyFocus::DefaultAction)
        );

        // Re-triggering on the same display_messages_version restores the cached response.
        app.clear_suggested_replies_for_new_turn();
        app.input.clear();
        app.cursor_pos = 0;
        app.maybe_trigger_suggested_replies_at_turn_end();
        assert_eq!(
            app.suggested_replies
                .current
                .as_ref()
                .and_then(|r| r.default_action.as_deref()),
            Some("Updated default")
        );

        // /suggestions debug outputs diagnostic summary.
        assert!(app.handle_suggestions_command("/suggestions debug"));
        assert!(
            app.display_messages
                .last()
                .unwrap()
                .content
                .contains("Suggested Replies Diagnostics")
        );

        // Starting a new turn clears both `current` and `pending_rx`.
        let (_tx, rx) = mpsc::unbounded_channel();
        app.suggested_replies.pending_rx = Some(rx);
        app.clear_suggested_replies_for_new_turn();
        assert!(app.suggested_replies.current.is_none());
        assert!(app.suggested_replies.focus.is_none());
        assert!(app.suggested_replies.pending_rx.is_none());
    }

    #[test]
    fn cyrillic_suggestion_uses_display_columns_instead_of_byte_truncation() {
        let ru = "Проверить отладку подсказок через команду /suggestions debug и запустить тесты";
        let display_cols = unicode_width::UnicodeWidthStr::width(ru);
        assert!(
            ru.len() > display_cols,
            "Cyrillic UTF-8 byte length exceeds column width"
        );
        let rendered = crate::tui::ui::truncate_to_display_width(ru, display_cols + 4);
        assert_eq!(rendered, ru);
        let clipped = crate::tui::ui::truncate_to_display_width(ru, 30);
        assert_eq!(unicode_width::UnicodeWidthStr::width(clipped.as_str()), 30);
        assert!(clipped.ends_with('…'));
    }
}
