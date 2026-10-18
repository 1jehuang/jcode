use super::App;
use crate::suggested_replies::{SuggestedRepliesContext, SuggestedRepliesResponse};
use crossterm::event::KeyCode;
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
        SuggestedRepliesContext::from_transcript(&transcript_tail).with_pending_todos(pending_todos)
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
                if tx.send(response).is_err() {}
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
                crate::tui::ui::inline_ui::truncate_to_display_width(&trimmed, 72)
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
        let jev_route =
            match crate::jev::JevClient::for_suggestions_with_selector(&cfg.jev_provider) {
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
                    let badge = resp.badge_for_text(&cand.text);
                    out.push_str(&format!(
                        "  {}. `{:.2}` `{}` — {}\n",
                        i + 1,
                        cand.confidence,
                        badge,
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

    pub(super) fn try_stage_suggested_reply_text(&mut self, text: &str) -> bool {
        if self.input.is_empty() && !self.is_processing && self.suggested_replies_visible() {
            let mut chars = text.chars();
            if let (Some(c), None) = (chars.next(), chars.next())
                && self.stage_suggested_reply_by_digit(c)
            {
                self.follow_chat_bottom_for_typing();
                return true;
            }
        }
        let onboarding_suggestions = matches!(
            self.onboarding_phase(),
            Some(crate::tui::app::onboarding_flow::OnboardingPhase::Suggestions)
        );
        if self.input.is_empty()
            && !self.is_processing
            && (self.display_messages.is_empty() || onboarding_suggestions)
        {
            let mut chars = text.chars();
            if let (Some(c), None) = (chars.next(), chars.next())
                && let Some(digit) = c.to_digit(10)
            {
                let suggestions = self.suggestion_prompts();
                let idx = digit as usize;
                if idx >= 1 && idx <= suggestions.len() {
                    let (_label, prompt) = &suggestions[idx - 1];
                    if !prompt.starts_with('/') {
                        self.remember_input_undo_state();
                        self.input = prompt.clone();
                        self.cursor_pos = self.input.len();
                        self.follow_chat_bottom_for_typing();
                        self.submit_input();
                        return true;
                    }
                }
            }
        }
        false
    }

    pub(super) fn compute_cache_ttl_status(&self) -> Option<crate::tui::CacheTtlInfo> {
        let baseline = self.kv_cache.kv_cache_baseline.as_ref()?;
        if baseline.session_id != self.kv_cache_session_id()
            || baseline.cache_generation != self.kv_cache.cache_generation
            || baseline.provider != self.kv_cache_provider_name()
            || baseline.model != self.kv_cache_provider_model()
        {
            return None;
        }
        let ttl_secs = baseline.cache_ttl_secs?;
        let elapsed = baseline.completed_at.elapsed().as_secs();
        let remaining = ttl_secs.saturating_sub(elapsed);
        Some(crate::tui::CacheTtlInfo {
            remaining_secs: remaining,
            ttl_secs,
            is_cold: remaining == 0,
            cold_for_secs: elapsed.saturating_sub(ttl_secs),
            cached_tokens: Some(baseline.input_tokens),
            is_estimate: crate::provider::cache_ttl_is_estimate(&baseline.provider),
        })
    }

    pub(super) fn cancel_suggested_replies_on_escape(&mut self) {
        let had_explorer_timer = self.suggested_replies.explorer_deadline.is_some();
        if self.cancel_suggested_replies_interaction(true) && had_explorer_timer {
            self.set_status_notice("Full-auto Explorer cancelled");
        }
    }

    pub(super) fn try_submit_suggested_reply_on_empty_input(
        &mut self,
        force_default: bool,
    ) -> bool {
        if self.input.is_empty()
            && let Some(prompt) = self.take_focused_or_default_suggested_reply(force_default)
        {
            self.submit_suggested_reply_prompt(prompt);
            return true;
        }
        false
    }

    pub(super) fn cancel_suggested_replies_for_cursor_key(&mut self, code: KeyCode) {
        if matches!(
            code,
            KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
        ) {
            self.cancel_suggested_replies_interaction(false);
        }
    }

    pub(super) fn pre_handle_basic_key_for_suggested_replies(&mut self, code: KeyCode) -> bool {
        self.cancel_suggested_replies_for_cursor_key(code);
        if code == KeyCode::Tab
            && let Some(maybe_prompt) = self.handle_suggested_replies_tab()
        {
            if let Some(prompt) = maybe_prompt {
                self.submit_suggested_reply_prompt(prompt);
            }
            return true;
        }
        false
    }

    pub(super) fn handle_suggested_reply_mouse_click(
        &mut self,
        mouse: crossterm::event::MouseEvent,
    ) -> Option<&'static str> {
        use crossterm::event::{MouseButton, MouseEventKind};
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && let Some(target) =
                crate::tui::ui::inline_ui::suggested_reply_target_at(mouse.column, mouse.row)
        {
            if self.suggested_replies.focus == Some(target)
                && self.suggested_replies_has_staged_input()
                && let Some(prompt) = self.take_suggested_reply_target(target)
            {
                self.submit_suggested_reply_prompt(prompt);
                return Some("suggested_reply_click_submit");
            } else if self.stage_suggested_reply_target(target) {
                return Some("suggested_reply_click_stage");
            }
        }
        None
    }

    pub(super) fn poll_and_queue_suggested_replies_tick(&mut self) -> bool {
        let (mut redraw, auto_prompt) = self.poll_suggested_replies_tick();
        if let Some(prompt) = auto_prompt {
            self.submit_suggested_reply_prompt(prompt);
            redraw = true;
        }
        redraw
    }

    pub(super) fn take_pending_remote_suggested_reply(&mut self) -> Option<String> {
        if self.is_processing {
            return None;
        }
        let prompt = self.suggested_replies.pending_remote_submit.take()?;
        self.input = prompt.clone();
        self.cursor_pos = self.input.len();
        Some(prompt)
    }

    pub(super) fn launch_six_lens_review_local(&mut self) -> anyhow::Result<bool> {
        let (model_override, provider_key_override) =
            super::commands_review::current_review_model_override(self);
        let parent_session_id = super::commands_review::current_feedback_target_session_id(self);
        self.record_six_lens_review_findings(
            "Verify 6-lens review findings across Correctness, Architecture, Security, TUI/UX, Perf/Async, and Tests".to_string(),
        );
        super::commands_review::launch_review_window_local(
            self,
            "review-6lenses",
            "6-Lens Review",
            build_six_lens_review_startup_message(&parent_session_id),
            model_override,
            provider_key_override,
        )
    }

    pub(super) fn run_six_lens_review_command_local(&mut self, title: &str) -> bool {
        if let Err(error) = self.launch_six_lens_review_local() {
            self.push_display_message(super::DisplayMessage::error(format!(
                "Failed to launch {}: {}",
                title.to_ascii_lowercase(),
                error
            )));
            self.set_status_notice(format!("{title} launch failed"));
        }
        true
    }

    pub(super) fn maybe_trigger_six_lens_autoreview_local(&mut self) -> bool {
        if crate::config::config()
            .suggested_replies
            .six_lens_auto_review
            && self.display_edit_tool_message_count > 0
        {
            self.run_six_lens_review_command_local("6-Lens Autoreview");
            return true;
        }
        false
    }

    pub(super) fn build_remote_review_label_and_startup(
        &mut self,
        trimmed: &str,
        parent_session_id: &str,
    ) -> (&'static str, String) {
        if trimmed != "/review" {
            self.record_six_lens_review_findings(
                "Verify 6-lens review findings across Correctness, Architecture, Security, TUI/UX, Perf/Async, and Tests".to_string(),
            );
            (
                "6-Lens Review",
                build_six_lens_review_startup_message(parent_session_id),
            )
        } else {
            (
                "Review",
                super::commands::build_review_startup_message(parent_session_id),
            )
        }
    }

    pub(super) fn on_idle_turn_completed(&mut self, duration_secs: Option<f32>) {
        self.maybe_notify_turn_complete(duration_secs);
        self.maybe_trigger_suggested_replies_at_turn_end();
    }

    pub(super) fn try_stage_suggested_reply_for_remote_enter(&mut self, force_default: bool) {
        if self.input.is_empty()
            && let Some(prompt) = self.take_focused_or_default_suggested_reply(force_default)
        {
            self.input = prompt;
            self.cursor_pos = self.input.len();
        }
    }

    pub(super) fn clear_failed_remote_split_launch(
        &mut self,
        kind: &str,
        label: &str,
        error: &anyhow::Error,
    ) {
        self.pending_split_startup_message = None;
        self.pending_split_parent_session_id = None;
        self.pending_split_prompt = None;
        self.pending_split_model_override = None;
        self.pending_split_provider_key_override = None;
        self.pending_split_label = None;
        self.push_display_message(super::DisplayMessage::error(format!(
            "Failed to launch {kind} session: {error}"
        )));
        self.set_status_notice(format!("{label} launch failed"));
    }

    pub(super) fn reset_synthetic_startup_dispatch(&mut self) {
        crate::logging::info(
            "Dispatching restored startup/queued followup without active remote message id",
        );
        self.is_processing = false;
        self.status = super::ProcessingStatus::Idle;
        self.processing_started = None;
        self.clear_visible_turn_started();
        self.replay_processing_started_ms = None;
        self.replay_elapsed_override = None;
    }
}

/// The 6 orthogonal review lenses used by the 6-Lens Subagent Self-Review.
pub(crate) const SIX_REVIEW_LENSES: &[(&str, &str)] = &[
    (
        "Lens 1: Correctness & Edge Cases",
        "Logic bugs, NaN/Inf/UTF-8 boundary handling, error propagation, and off-by-one defects",
    ),
    (
        "Lens 2: Architecture & Module Boundaries",
        "Crate separation, config/state coupling, public trait contracts, and cache invalidation",
    ),
    (
        "Lens 3: Security & Safety Invariants",
        "Credential isolation, untrusted input sanitization, HTTP error handling, and read-only safety",
    ),
    (
        "Lens 4: TUI / UX & Keybindings",
        "Layout chunk stability, terminal compatibility, Tab/Ctrl+Enter/Esc parity in local & remote modes, and mouse hit-testing",
    ),
    (
        "Lens 5: Performance, Async & KV-Cache",
        "Non-blocking background tasks, zero KV-cache prefix pollution (store=false), and redraw cadence",
    ),
    (
        "Lens 6: Tests & Go/No-Go Closure",
        "Unit test coverage over main and edge paths, OpenSpec task completion, and concrete Go/No-Go verdict",
    ),
];

pub(super) fn build_six_lens_review_startup_message(parent_session_id: &str) -> String {
    let mut lenses_block = String::new();
    for (idx, (title, focus)) in SIX_REVIEW_LENSES.iter().enumerate() {
        lenses_block.push_str(&format!("{}. **{}** — {}\n", idx + 1, title, focus));
    }
    format!(
        "You are the 6-Lens Subagent Self-Review coordinator for parent session `{}`.\n\
Your job is to audit the recent changes across all 6 orthogonal engineering lenses and report a consolidated Go / Conditional-Go / No-Go verdict back to the parent session.\n\
\n\
First read the recent context:\n\
1. Use `conversation_search` with `stats=true` and `turns` (last 6-12 turns) to understand what was implemented.\n\
2. Inspect `git status --short` and `git diff --stat`.\n\
\n\
{}\
Evaluate the work systematically across these 6 lenses (spawning parallel read-only swarm subagents if `swarm` is available, or evaluating each lens directly):\n\
{}\n\
When finished, send exactly one DM to session `{}` using `swarm` (or `communicate`) with action `dm` containing:\n\
- Overall Verdict: `GO`, `CONDITIONAL GO`, or `NO-GO`\n\
- Per-lens findings with exact `file:line` references\n\
- If any lens is `CONDITIONAL GO` or `NO-GO`, a single concise action line prefixed with `DEFAULT_ACTION:` describing the exact fix to run next.\n\
- After sending the DM, stop.",
        parent_session_id,
        super::commands_review::review_session_read_only_guardrails(),
        lenses_block,
        parent_session_id
    )
}

#[cfg(test)]
mod tests;
