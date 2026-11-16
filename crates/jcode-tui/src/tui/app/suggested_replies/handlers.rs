use super::super::App;
use crossterm::event::KeyCode;

impl App {
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
            "- **Config**: `enabled={}`, `jev_provider=\"{}\"`, `use_open_gf={}`, `confidence_threshold={:.2}`, `max_secondary_suggestions={}` (clamped `{}`), `show_confidence={}`, `custom_prompts={}`, `enable_full_auto_explorer={}`, `enable_finish_category={}`\n",
            cfg.enabled,
            cfg.jev_provider,
            cfg.use_open_gf,
            cfg.clamped_confidence_threshold(),
            cfg.max_secondary_suggestions,
            cfg.clamped_max_secondary(),
            cfg.show_confidence,
            cfg.sanitized_custom_prompts().len(),
            cfg.enable_full_auto_explorer,
            cfg.enable_finish_category,
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
        out.push_str(
            "- **Controls**: `Tab`/`↑`/`↓` cycle & stage · `0`/`1..9` stage on 1st press, send on 2nd press of same digit · `Enter`/`Ctrl+Enter` send · `Esc` clear\n",
        );

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
                    let fin = resp.advances_feature_completion_for_text(&cand.text);
                    out.push_str(&format!(
                        "  {}. `{:.2}` `{}` `fin={}` — {}\n",
                        i + 1,
                        cand.confidence,
                        badge,
                        fin,
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
                self.push_display_message(super::super::DisplayMessage::system(report));
                true
            }
            "test" | "refresh" => {
                self.set_status_notice("Re-evaluating Suggested Replies cascade...");
                self.push_display_message(super::super::DisplayMessage::system(
                    "Triggered Suggested Replies cascade evaluation. Run `/suggestions debug` once complete to inspect telemetry.".to_string(),
                ));
                self.trigger_suggested_replies_evaluation(true);
                true
            }
            _ => {
                self.push_display_message(super::super::DisplayMessage::error(
                    "Usage: `/suggestions [debug|status|test|refresh]`".to_string(),
                ));
                true
            }
        }
    }

    pub(in crate::tui::app) fn try_stage_suggested_reply_text(&mut self, text: &str) -> bool {
        if self.suggested_replies_input_is_empty_or_staged()
            && !self.is_processing
            && self.suggested_replies_visible()
        {
            let mut chars = text.chars();
            if let (Some(c), None) = (chars.next(), chars.next())
                && self.handle_suggested_reply_digit_press(c)
            {
                self.follow_chat_bottom_for_typing();
                return true;
            }
        }
        self.cancel_suggested_replies_interaction(false);
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

    pub(in crate::tui::app) fn cancel_suggested_replies_on_escape(&mut self) {
        let had_explorer_timer = self.suggested_replies.explorer_deadline.is_some();
        let had_pending_remote = self.suggested_replies.pending_remote_submit.is_some();
        if self.cancel_suggested_replies_interaction(true) {
            if had_explorer_timer {
                self.set_status_notice("Full-auto Explorer cancelled");
            } else if had_pending_remote {
                self.set_status_notice("Pending suggested reply cancelled");
            }
        }
    }

    pub(in crate::tui::app) fn try_submit_suggested_reply_on_empty_input(
        &mut self,
        force_default: bool,
    ) -> bool {
        if self.suggested_replies_input_is_empty_or_staged()
            && let Some(prompt) = self.take_focused_or_default_suggested_reply(force_default)
        {
            self.submit_suggested_reply_prompt(prompt);
            return true;
        }
        false
    }

    pub(in crate::tui::app) fn cancel_suggested_replies_for_cursor_key(&mut self, code: KeyCode) {
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

    pub(in crate::tui::app) fn pre_handle_basic_key_for_suggested_replies(
        &mut self,
        code: KeyCode,
    ) -> bool {
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

    pub(in crate::tui::app) fn handle_suggested_reply_mouse_click(
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

    pub(in crate::tui::app) fn poll_and_queue_suggested_replies_tick(&mut self) -> bool {
        let (mut redraw, auto_prompt) = self.poll_suggested_replies_tick();
        if let Some(prompt) = auto_prompt {
            self.submit_suggested_reply_prompt(prompt);
            redraw = true;
        }
        redraw
    }

    pub(in crate::tui::app) fn take_pending_remote_suggested_reply(&mut self) -> Option<String> {
        if self.is_processing {
            return None;
        }
        let prompt = self.suggested_replies.pending_remote_submit.take()?;
        if self.queued_messages.is_empty()
            && self.queued_commands.is_empty()
            && self.hidden_queued_system_messages.is_empty()
        {
            self.pending_queued_dispatch = false;
        }
        if !self.input.is_empty()
            || !self.pending_images.is_empty()
            || self.prompt_history_search.is_some()
            || self.inline_interactive_state.is_some()
            || self.inline_view_state.is_some()
        {
            return None;
        }
        self.input = prompt.clone();
        self.cursor_pos = self.input.len();
        Some(prompt)
    }

    pub(in crate::tui::app) fn on_idle_turn_completed(&mut self, duration_secs: Option<f32>) {
        self.maybe_notify_turn_complete(duration_secs);
        self.maybe_trigger_suggested_replies_at_turn_end();
    }

    pub(in crate::tui::app) fn try_stage_suggested_reply_for_remote_enter(
        &mut self,
        force_default: bool,
    ) {
        if self.suggested_replies_input_is_empty_or_staged()
            && let Some(prompt) = self.take_focused_or_default_suggested_reply(force_default)
        {
            self.input = prompt;
            self.cursor_pos = self.input.len();
        }
    }

    pub(in crate::tui::app) fn handle_suggested_replies_tab_remote(&mut self) -> bool {
        match self.handle_suggested_replies_tab() {
            Some(Some(prompt)) => {
                self.input = prompt;
                self.cursor_pos = self.input.len();
                true
            }
            Some(None) => false,
            None => {
                self.autocomplete();
                false
            }
        }
    }

    pub(in crate::tui::app) fn has_ready_remote_command_or_suggestion(&self) -> bool {
        !self.queued_commands.is_empty() || self.suggested_replies.pending_remote_submit.is_some()
    }

    pub(in crate::tui::app) fn try_handle_local_remote_slash_command(
        &mut self,
        trimmed: &str,
    ) -> bool {
        if trimmed == "/help" || trimmed == "/?" || trimmed == "/commands" {
            self.help_scroll = Some(0);
            return true;
        }
        super::super::commands::handle_dictation_command(self, trimmed)
            || self.handle_suggestions_command(trimmed)
    }
}
