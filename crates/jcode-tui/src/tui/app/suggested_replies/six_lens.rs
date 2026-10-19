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

    pub(in crate::tui::app) fn compute_cache_ttl_status(&self) -> Option<crate::tui::CacheTtlInfo> {
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

    pub(in crate::tui::app) fn cancel_suggested_replies_on_escape(&mut self) {
        let had_explorer_timer = self.suggested_replies.explorer_deadline.is_some();
        if self.cancel_suggested_replies_interaction(true) && had_explorer_timer {
            self.set_status_notice("Full-auto Explorer cancelled");
        }
    }

    pub(in crate::tui::app) fn try_submit_suggested_reply_on_empty_input(
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
        self.input = prompt.clone();
        self.cursor_pos = self.input.len();
        Some(prompt)
    }

    pub(in crate::tui::app) fn launch_six_lens_review_local(&mut self) -> anyhow::Result<bool> {
        let (model_override, provider_key_override) =
            super::super::commands_review::current_review_model_override(self);
        let parent_session_id =
            super::super::commands_review::current_feedback_target_session_id(self);
        super::super::commands_review::launch_review_window_local(
            self,
            "review-6lenses",
            "6-Lens Review",
            build_six_lens_review_startup_message(&parent_session_id),
            model_override,
            provider_key_override,
        )
    }

    pub(in crate::tui::app) fn run_six_lens_review_command_local(&mut self, title: &str) -> bool {
        if let Err(error) = self.launch_six_lens_review_local() {
            self.push_display_message(super::super::DisplayMessage::error(format!(
                "Failed to launch {}: {}",
                title.to_ascii_lowercase(),
                error
            )));
            self.set_status_notice(format!("{title} launch failed"));
        }
        true
    }

    fn consume_unreviewed_turn_edits(&mut self) -> bool {
        if self.display_edit_tool_message_count < self.suggested_replies.last_reviewed_edit_count {
            self.suggested_replies.last_reviewed_edit_count = self.display_edit_tool_message_count;
        }
        if crate::config::config()
            .suggested_replies
            .six_lens_auto_review
            && self.display_edit_tool_message_count
                > self.suggested_replies.last_reviewed_edit_count
        {
            self.suggested_replies.last_reviewed_edit_count = self.display_edit_tool_message_count;
            return true;
        }
        false
    }

    pub(in crate::tui::app) fn maybe_trigger_six_lens_autoreview_local(&mut self) -> bool {
        if self.consume_unreviewed_turn_edits() {
            self.run_six_lens_review_command_local("6-Lens Autoreview");
            return true;
        }
        false
    }

    pub(in crate::tui::app) fn maybe_trigger_six_lens_autoreview_remote(&mut self) -> bool {
        if !self.is_remote
            || self.is_replay
            || self.pending_split_request
            || self.pending_split_startup_message.is_some()
        {
            return false;
        }
        if self.consume_unreviewed_turn_edits() {
            let (model_override, provider_key_override) =
                super::super::commands_review::current_review_model_override(self);
            let parent_session_id =
                super::super::commands_review::current_feedback_target_session_id(self);
            super::super::commands_review::queue_review_spawn_remote(
                self,
                "6-Lens Autoreview",
                parent_session_id.clone(),
                build_six_lens_review_startup_message(&parent_session_id),
                model_override,
                provider_key_override,
            );
            return true;
        }
        false
    }

    pub(in crate::tui::app) fn build_remote_review_label_and_startup(
        &mut self,
        trimmed: &str,
        parent_session_id: &str,
    ) -> (&'static str, String) {
        if trimmed != "/review" {
            (
                "6-Lens Review",
                build_six_lens_review_startup_message(parent_session_id),
            )
        } else {
            (
                "Review",
                super::super::commands::build_review_startup_message(parent_session_id),
            )
        }
    }

    pub(in crate::tui::app) fn on_idle_turn_completed(&mut self, duration_secs: Option<f32>) {
        self.maybe_notify_turn_complete(duration_secs);
        self.maybe_trigger_six_lens_autoreview_remote();
        self.maybe_trigger_suggested_replies_at_turn_end();
    }

    pub(in crate::tui::app) fn try_stage_suggested_reply_for_remote_enter(
        &mut self,
        force_default: bool,
    ) {
        if self.input.is_empty()
            && let Some(prompt) = self.take_focused_or_default_suggested_reply(force_default)
        {
            self.input = prompt;
            self.cursor_pos = self.input.len();
        }
    }

    pub(in crate::tui::app) fn clear_failed_remote_split_launch(
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
        self.push_display_message(super::super::DisplayMessage::error(format!(
            "Failed to launch {kind} session: {error}"
        )));
        self.set_status_notice(format!("{label} launch failed"));
    }

    pub(in crate::tui::app) fn reset_synthetic_startup_dispatch(&mut self) {
        crate::logging::info(
            "Dispatching restored startup/queued followup without active remote message id",
        );
        self.is_processing = false;
        self.status = super::super::ProcessingStatus::Idle;
        self.processing_started = None;
        self.clear_visible_turn_started();
        self.replay_processing_started_ms = None;
        self.replay_elapsed_override = None;
    }
}

/// The 6 orthogonal review lenses used by the 6-Lens Subagent Self-Review.
const SIX_REVIEW_LENSES: &[(&str, &str)] = &[
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

pub(in crate::tui::app) fn build_six_lens_review_startup_message(
    parent_session_id: &str,
) -> String {
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
        super::super::commands_review::review_session_read_only_guardrails(),
        lenses_block,
        parent_session_id
    )
}
