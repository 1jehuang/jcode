//! Core keyboard event dispatch.

use super::*;

impl App {
    pub(super) fn handle_key_core(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        text_input: Option<String>,
    ) -> Result<()> {
        let mut code = code;
        let mut modifiers = modifiers;
        ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

        if self.handle_ssh_login_key(code, modifiers, text_input.as_deref()) {
            return Ok(());
        }

        // Alt+5 always starts the onboarding rehearsal from a pristine first
        // screen, even when another modal or a previous sim screen is active.
        if self.handle_onboarding_sim_reset_shortcut(code, modifiers) {
            return Ok(());
        }
        if self.handle_update_sim_shortcut(code, modifiers) {
            return Ok(());
        }
        if self.handle_update_rehearsal_shortcut(code, modifiers) {
            return Ok(());
        }

        // Cmd+5 toggles the onboarding rehearsal. Every other key goes to the
        // real onboarding handlers, exactly as on a first run.
        if self.handle_onboarding_sim_toggle_shortcut(code, modifiers) {
            return Ok(());
        }

        if handle_modal_key(self, code, modifiers)? {
            return Ok(());
        }

        if self.handle_onboarding_continue_prompt_key(code) {
            return Ok(());
        }

        // Inline hotkey feedback: when a known-but-rarely-used chord is pressed,
        // show "you just pressed X → does Y". Placed after the modal/overlay
        // handlers so overlay-local keys stay silent. Unknown chords are
        // reported at the fall-through points below.
        self.observe_known_hotkey(code, modifiers, false);

        if code == KeyCode::BackTab {
            self.cycle_model_favorite_hotkey();
            return Ok(());
        }

        // While the model picker preview is visible, route its favorite/default
        // hotkeys (Ctrl+O set default, Ctrl+N toggle favorite) to the focused
        // picker handler before the global control shortcuts can claim them. This
        // makes the hotkeys work directly in the preview list the user always
        // sees, without colliding with the readline/tmux keys (Ctrl+B/Ctrl+F).
        if self.model_picker_preview_hotkey(code, modifiers)? {
            return Ok(());
        }

        if self.handle_key_during_provider_failover(code) {
            return Ok(());
        }

        // Accept an armed post-error fallback offer: switch to the next best
        // model/auth-method and resend the failed turn.
        if self.pending_fallback_offer.is_some()
            && !self.is_processing
            && self.fallback_switch_key_matches(code, modifiers)
        {
            self.apply_pending_fallback_offer();
            return Ok(());
        }

        // Accept an armed "merge the diverged update" offer: spawn a jcode agent
        // to reconcile the branches. Shares the fallback-switch accept key.
        if self.merge_offer_key_matches(code, modifiers) {
            self.accept_update_merge_offer();
            return Ok(());
        }

        if is_next_prompt_new_session_hotkey(code, modifiers) {
            self.toggle_next_prompt_new_session_routing();
            return Ok(());
        }

        if self.handle_command_suggestion_key(code, modifiers) {
            return Ok(());
        }

        if handle_pre_control_shortcuts(self, code, modifiers) {
            return Ok(());
        }

        self.normalize_diagram_state();
        let diagram_available = self.diagram_available();

        // Ctrl / Alt(Option) / Cmd(Super) + Up all recall queued/pending messages
        // for editing and then walk prompt history. We accept any of the three
        // single modifiers so the gesture works regardless of which one a given
        // terminal forwards (some send Option as Alt, some forward Command as
        // Super), without the user having to rebind anything. Alt+Up/Down are
        // claimed earlier by the speed-tier keys by default; unbinding
        // speed_increase/speed_decrease restores the Alt alias here.
        if code == KeyCode::Up && is_prompt_recall_modifier(modifiers) {
            if retrieve_pending_message_for_edit(self) {
                return Ok(());
            }
            // Normalize to CONTROL so handle_prompt_history_navigation takes its
            // explicit-history path (jump straight into history even mid-draft).
            handle_prompt_history_navigation(self, KeyCode::Up, KeyModifiers::CONTROL);
            return Ok(());
        }

        if code == KeyCode::Down && is_prompt_recall_modifier(modifiers) {
            handle_prompt_history_navigation(self, KeyCode::Down, KeyModifiers::CONTROL);
            return Ok(());
        }

        // Handle ctrl combos regardless of processing state
        if modifiers.contains(KeyModifiers::CONTROL)
            && handle_global_control_shortcuts(self, code, diagram_available)
        {
            return Ok(());
        }

        // Ctrl+Enter / Cmd+Enter: does opposite of queue_mode during processing.
        // Terminals may encode Command as either Super or Meta.
        if is_alternate_enter(code, modifiers) {
            handle_alternate_enter(self);
            return Ok(());
        }

        // Shift+Enter, Alt/Option+Enter, and the trailing-backslash fallback all
        // insert a newline in the input box.
        if newline::enter_inserts_newline(self, code, modifiers) {
            return Ok(());
        }

        // When the model picker preview is visible, arrow keys navigate the picker list
        if self
            .inline_interactive_state
            .as_ref()
            .map(|p| p.preview)
            .unwrap_or(false)
        {
            match code {
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                    return self.handle_inline_interactive_key(code, modifiers);
                }
                _ => {}
            }
        }

        if handle_multiline_input_navigation(self, code, modifiers)
            || handle_prompt_history_navigation(self, code, modifiers)
        {
            return Ok(());
        }

        if let Some(text) = text_input.or_else(|| text_input_for_key(code, modifiers)) {
            handle_text_input(self, &text);
            return Ok(());
        }

        // Never fall through and insert literal text for unhandled Ctrl+key chords. This stays
        // after text_input so Ctrl+Alt/AltGr symbols delivered as final printable text still work.
        if modifiers.contains(KeyModifiers::CONTROL) {
            self.note_unrecognized_hotkey(code, modifiers, false);
            return Ok(());
        }

        if code == KeyCode::Enter {
            // Stray post-paste Enter from some terminals is not a submit (#544).
            if paste_guard::consume_paste_trailing_enter() {
                return Ok(());
            }
            // During the onboarding model-selection phase, Enter on an empty
            // prompt opens the model picker instead of submitting nothing.
            if self.input.trim().is_empty()
                && matches!(
                    self.onboarding_phase(),
                    Some(crate::tui::app::onboarding_flow::OnboardingPhase::ModelSelect)
                )
            {
                self.open_model_picker();
                return Ok(());
            }
            handle_enter(self);
            return Ok(());
        }

        // A modified chord (or function key) that reached this point is not
        // bound to anything; tell the user instead of silently swallowing it or
        // inserting a surprise character.
        self.note_unrecognized_hotkey(code, modifiers, false);

        if handle_basic_key(self, code) {
            return Ok(());
        }

        Ok(())
    }
}
