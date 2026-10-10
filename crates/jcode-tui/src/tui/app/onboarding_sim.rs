//! Onboarding rehearsal.
//!
//! A developer aid for experiencing first-run onboarding on a machine where
//! jcode is already set up. Alt+5 (reset), Cmd+5 (toggle), `/onboarding-sim`,
//! or `jcode --onboarding-sim` start it.
//!
//! The rehearsal runs the REAL onboarding flow: the same phases, key handlers,
//! timers, login detection, import, and start choice a brand-new user gets. The
//! only thing it changes is that jcode forgets it already knows this user:
//!   - the flow starts as if this were a first launch,
//!   - login detection lists every importable login from other tools on this
//!     machine, including ones jcode already trusts (a fresh install would
//!     not know about that trust yet).
//!
//! It stays safe because the real flow itself is safe to re-run: importing a
//! login jcode already has keeps the existing login (no re-sign-in, no revoked
//! trust), "Log in to OpenAI" skips the sign-in when OpenAI is already
//! connected, and onboarding never rewrites an existing telemetry choice. The
//! one rehearsal-only guard is that a telemetry choice made here is not saved.

use super::App;
use super::SessionPickerMode;

impl App {
    /// Whether an onboarding rehearsal is currently running.
    pub(super) fn onboarding_sim_active(&self) -> bool {
        self.onboarding_sim
    }

    /// Entry point for the `jcode --onboarding-sim` CLI flag: start the
    /// rehearsal immediately at launch, before the remote session connects.
    pub fn start_onboarding_simulator_on_launch(&mut self) {
        self.start_onboarding_simulator();
    }

    /// Alt+5 is the cross-platform "start over" shortcut. Require exactly Alt so
    /// Windows AltGr chords (reported as Ctrl+Alt) never trigger the rehearsal.
    pub(super) fn handle_onboarding_sim_reset_shortcut(
        &mut self,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        if code != crossterm::event::KeyCode::Char('5')
            || modifiers != crossterm::event::KeyModifiers::ALT
        {
            return false;
        }
        self.start_onboarding_simulator();
        true
    }

    /// Toggle the rehearsal on/off (the Cmd+5 hotkey entry point).
    pub(super) fn toggle_onboarding_simulator(&mut self) {
        if self.onboarding_sim {
            self.stop_onboarding_simulator();
        } else {
            self.start_onboarding_simulator();
        }
    }

    /// Cmd+5 toggles the rehearsal. Routed ahead of modal handlers in both the
    /// local and remote key paths. Every other key goes to the real flow.
    pub(super) fn handle_onboarding_sim_toggle_shortcut(
        &mut self,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        if code != crossterm::event::KeyCode::Char('5')
            || !modifiers.contains(crossterm::event::KeyModifiers::SUPER)
        {
            return false;
        }
        self.toggle_onboarding_simulator();
        true
    }

    /// Start (or restart) the real first-run flow as if this were a new user.
    pub(super) fn start_onboarding_simulator(&mut self) {
        // Don't fight a genuine first-run flow that is already in progress.
        if self.onboarding_flow_active() && !self.onboarding_sim {
            self.set_status_notice("Onboarding flow already active; can't start the rehearsal now");
            return;
        }

        // Restarting must behave like a pristine first launch, even if a
        // previous rehearsal left progress or error state behind. This only
        // clears onboarding-owned state; the session transcript, input, auth,
        // and configuration are untouched.
        self.onboarding_flow = None;
        self.onboarding_import_in_progress = None;
        self.onboarding_import_error = None;
        self.onboarding_import_failed_provider = None;
        self.onboarding_pending_model_validation = None;
        self.onboarding_telemetry_choice_made = false;
        self.onboarding_auto_model_selection_active
            .store(false, std::sync::atomic::Ordering::Release);

        // The shortcut is routed ahead of modal handlers. Clear transient
        // overlays too, otherwise a help/picker/copy modal could stay painted
        // above the onboarding card and make Alt+5 appear to do nothing.
        self.changelog_scroll = None;
        self.help_scroll = None;
        self.model_status_scroll = None;
        self.commit_log_overlay = None;
        self.session_picker_overlay = None;
        self.session_picker_mode = SessionPickerMode::Resume;
        self.pending_session_picker_load = None;
        self.login_picker_overlay = None;
        self.account_picker_overlay = None;
        self.usage_overlay = None;
        self.inline_interactive_state = None;
        self.copy_selection_mode = false;
        self.copy_selection_anchor = None;
        self.copy_selection_cursor = None;
        self.copy_selection_pending_anchor = None;
        self.copy_selection_dragging = false;
        self.copy_selection_goal_column = None;
        self.copy_selection_edge_autoscroll = None;

        self.onboarding_sim = true;
        // Treat this as a new user: render the first-run welcome layout and
        // new-user suggestions even though this install has history.
        self.onboarding_preview_mode = true;
        // The startup check would otherwise decide this is an established
        // install and stay out of the way.
        self.onboarding_startup_checked = true;
        // A brand-new install has no jcode logins yet, so the real flow starts
        // at the login step, where it detects logins from other tools.
        self.begin_onboarding_flow_at_login();
        // Repaint every cell in place. A hard clear (ED2) blanks the whole
        // terminal for a frame before redrawing, which reads as a flash.
        self.request_full_repaint();
    }

    /// Leave the rehearsal and return to the normal session screen.
    pub(super) fn stop_onboarding_simulator(&mut self) {
        // `/onboarding-sim off` is also callable while a real onboarding flow is
        // active. Never let that command erase genuine onboarding progress.
        if !self.onboarding_sim {
            return;
        }
        self.onboarding_sim = false;
        self.onboarding_flow = None;
        self.onboarding_import_in_progress = None;
        self.onboarding_import_error = None;
        self.session_picker_overlay = None;
        self.session_picker_mode = SessionPickerMode::Resume;
        self.onboarding_preview_mode = false;
        // Repaint every cell in place. A hard clear (ED2) blanks the whole
        // terminal for a frame before redrawing, which reads as a flash.
        self.request_full_repaint();
        self.set_status_notice("Onboarding rehearsal: off");
    }
}
