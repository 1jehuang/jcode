//! Live onboarding simulator.
//!
//! A developer aid for reviewing the first-run onboarding experience without
//! resetting real auth state or installing anything. Pressing the simulator
//! hotkey (Alt+5 resets and opens; Cmd+5 toggles) or running `/onboarding-sim`
//! seeds the onboarding flow with synthetic, fixture-backed phases and lets you
//! step through every screen the way a brand-new user would see them.
//!
//! Unlike the real flow, the simulator:
//!   - never starts a real login, import, or suggested review,
//!   - never auto-advances on a countdown (the tick loop is frozen for the sim),
//!   - can be exited at any point, restoring the prior session state.
//!
//! It reuses the exact rendering path the live flow uses
//! (`onboarding_welcome_kind`), so what you see here is what first-run users see.

use super::App;
use super::SessionPickerMode;
use super::onboarding_flow::{ImportReview, OnboardingFlow, OnboardingPhase, SummaryPill};
use crossterm::event::{KeyCode, KeyModifiers};
use std::time::Instant;

/// One simulated onboarding screen: a human label plus the phase that drives
/// the welcome card.
struct SimScreen {
    title: &'static str,
    phase: OnboardingPhase,
}

impl App {
    /// Whether the onboarding simulator is currently driving the UI.
    pub(super) fn onboarding_sim_active(&self) -> bool {
        self.onboarding_sim.is_some()
    }

    /// Entry point for the `jcode --onboarding-sim` CLI flag: start the
    /// simulator immediately at launch, before the remote session connects.
    pub fn start_onboarding_simulator_on_launch(&mut self) {
        self.start_onboarding_simulator();
    }

    /// Alt+5 is the cross-platform "start over" shortcut. Require exactly Alt so
    /// Windows AltGr chords (reported as Ctrl+Alt) never trigger the simulator.
    pub(super) fn handle_onboarding_sim_reset_shortcut(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        if code != KeyCode::Char('5') || modifiers != KeyModifiers::ALT {
            return false;
        }
        self.start_onboarding_simulator();
        true
    }

    /// Toggle the onboarding simulator on/off (the Cmd+5 hotkey entry point).
    pub(super) fn toggle_onboarding_simulator(&mut self) {
        if self.onboarding_sim.is_some() {
            self.stop_onboarding_simulator();
        } else {
            self.start_onboarding_simulator();
        }
    }

    /// Begin the onboarding simulator at the first screen.
    pub(super) fn start_onboarding_simulator(&mut self) {
        // Don't fight a genuinely-running live guided flow.
        if self.onboarding_flow_active() && self.onboarding_sim.is_none() {
            self.set_status_notice("Onboarding flow already active; can't start the simulator now");
            return;
        }

        // Re-entering the simulator must behave like a pristine first launch,
        // even if the previous simulated screen or an interrupted live attempt
        // left transient progress/error state behind. This only clears
        // onboarding-owned state; the real session transcript, input, auth, and
        // configuration remain untouched.
        self.onboarding_import_in_progress = None;
        self.onboarding_import_error = None;
        self.onboarding_import_failed_provider = None;
        self.onboarding_pending_model_validation = None;
        self.onboarding_auto_model_selection_active
            .store(false, std::sync::atomic::Ordering::Release);

        // The shortcut is intentionally routed ahead of modal handlers. Clear
        // transient overlays too, otherwise a help/picker/copy modal could stay
        // painted above the freshly-reset onboarding card and make Alt+5 appear
        // to do nothing.
        self.changelog_scroll = None;
        self.help_scroll = None;
        self.model_status_scroll = None;
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
        self.onboarding_sim = Some(0);
        // Force the dedicated welcome layout to render regardless of session
        // state, exactly like the static `/onboarding-preview`.
        self.onboarding_preview_mode = true;
        self.apply_onboarding_sim_screen();
        self.force_full_redraw = true;
        self.update_onboarding_sim_status();
    }

    /// Exit the simulator and restore the prior (non-onboarding) state.
    pub(super) fn stop_onboarding_simulator(&mut self) {
        // `/onboarding-sim off` is also callable while a real onboarding flow is
        // active. Never let that command erase genuine onboarding progress.
        if self.onboarding_sim.take().is_none() {
            return;
        }
        self.onboarding_flow = None;
        self.session_picker_overlay = None;
        self.session_picker_mode = SessionPickerMode::Resume;
        self.onboarding_preview_mode = false;
        self.force_full_redraw = true;
        self.set_status_notice("Onboarding simulator: off");
    }

    /// The catalog of simulated screens, rebuilt fresh each call (fixtures carry
    /// `Instant`s, so they can't be `'static`).
    fn onboarding_sim_screens() -> Vec<SimScreen> {
        vec![
            SimScreen {
                title: "Log in to OpenAI (nothing detected)",
                phase: OnboardingPhase::LoginOpenAi {
                    yes_highlighted: true,
                },
            },
            SimScreen {
                title: "Import summary (detected logins, Continue preselected)",
                phase: OnboardingPhase::Login {
                    import: ImportReview::new(vec![
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "OpenAI/Codex",
                            "Codex auth.json",
                        ),
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "Claude",
                            "Claude Code",
                        ),
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "GitHub Copilot",
                            "github-copilot/hosts.json",
                        ),
                    ]),
                },
            },
            SimScreen {
                title: "Import detected logins (multiple, checkbox list)",
                phase: OnboardingPhase::Login {
                    import: ImportReview::new(vec![
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "OpenAI/Codex",
                            "Codex auth.json",
                        ),
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "Claude",
                            "Claude Code",
                        ),
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "GitHub Copilot",
                            "github-copilot/hosts.json",
                        ),
                    ])
                    .map(|mut review| {
                        review.enter_choose_mode();
                        review
                    }),
                },
            },
            SimScreen {
                title: "Telemetry settings (from the summary's third pill)",
                phase: OnboardingPhase::Login {
                    import: ImportReview::new(vec![
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "OpenAI/Codex",
                            "Codex auth.json",
                        ),
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "Claude",
                            "Claude Code",
                        ),
                    ])
                    .map(|mut review| {
                        review.focus_summary_pill(SummaryPill::Telemetry);
                        review.open_telemetry();
                        review
                    }),
                },
            },
            SimScreen {
                title: "Import detected logins (single)",
                phase: OnboardingPhase::Login {
                    import: ImportReview::new(vec![
                        crate::external_auth::ExternalAuthReviewCandidate::fixture(
                            "Cursor", "Cursor",
                        ),
                    ])
                    .map(|mut review| {
                        review.enter_choose_mode();
                        review
                    }),
                },
            },
            SimScreen {
                title: "Login recovery (import declined / failed)",
                phase: OnboardingPhase::Login { import: None },
            },
            SimScreen {
                title: "Choose a suggested review or blank session",
                phase: OnboardingPhase::StartChoice {
                    shown_at: Instant::now(),
                },
            },
            SimScreen {
                title: "Suggestions (resting / all set)",
                phase: OnboardingPhase::Suggestions,
            },
        ]
    }

    /// Total number of simulated screens.
    fn onboarding_sim_screen_count() -> usize {
        Self::onboarding_sim_screens().len()
    }

    /// Install the flow state for the current simulator screen.
    fn apply_onboarding_sim_screen(&mut self) {
        let Some(index) = self.onboarding_sim else {
            return;
        };
        let mut screens = Self::onboarding_sim_screens();
        if index >= screens.len() {
            self.stop_onboarding_simulator();
            return;
        }
        let screen = screens.remove(index);
        let is_start_choice = matches!(&screen.phase, OnboardingPhase::StartChoice { .. });
        self.session_picker_overlay = None;
        self.session_picker_mode = SessionPickerMode::Resume;
        self.onboarding_flow = Some(OnboardingFlow {
            phase: screen.phase,
        });
        if is_start_choice {
            self.onboarding_open_start_choice();
        }
    }

    /// Refresh the status line with the current screen position and controls.
    fn update_onboarding_sim_status(&mut self) {
        let Some(index) = self.onboarding_sim else {
            return;
        };
        let screens = Self::onboarding_sim_screens();
        let total = screens.len();
        let title = screens.get(index).map(|s| s.title).unwrap_or("Onboarding");
        self.set_status_notice(format!(
            "Onboarding sim {}/{}: {} — Tab/→ next, Shift+Tab/← prev, h/l highlight, Alt+5 restart, Esc exit",
            index + 1,
            total,
            title,
        ));
    }

    /// Move to the next/previous simulated screen (delta of +1 / -1). Advancing
    /// past the last screen exits the simulator.
    fn onboarding_sim_step(&mut self, delta: i32) {
        let Some(index) = self.onboarding_sim else {
            return;
        };
        let total = Self::onboarding_sim_screen_count() as i32;
        let next = index as i32 + delta;
        if next < 0 {
            // Stepping back from the first screen just stays put.
            self.update_onboarding_sim_status();
            return;
        }
        if next >= total {
            self.stop_onboarding_simulator();
            return;
        }
        self.onboarding_sim = Some(next as usize);
        self.apply_onboarding_sim_screen();
        self.force_full_redraw = true;
        self.update_onboarding_sim_status();
    }

    /// Visually move the Yes/No highlight (or the import checkbox cursor) on the
    /// current screen. No real action is taken; the simulator never logs in or
    /// imports anything.
    fn onboarding_sim_set_highlight(&mut self, yes: bool) {
        if let Some(flow) = self.onboarding_flow.as_mut() {
            match &mut flow.phase {
                OnboardingPhase::LoginOpenAi { yes_highlighted }
                | OnboardingPhase::ContinuePrompt {
                    yes_highlighted, ..
                } => {
                    *yes_highlighted = yes;
                }
                OnboardingPhase::Login {
                    import: Some(review),
                } => {
                    // On the checkbox import screen, preview check/uncheck of the
                    // current row instead of a Yes/No pill.
                    review.set_current(yes);
                }
                _ => {}
            }
        }
    }

    /// Move the import checkbox cursor, or the telemetry option highlight while
    /// the telemetry settings screen is showing. Only meaningful on the import
    /// family of screens.
    fn onboarding_sim_move_cursor(&mut self, down: bool) -> bool {
        if let Some(flow) = self.onboarding_flow.as_mut()
            && let OnboardingPhase::Login {
                import: Some(review),
            } = &mut flow.phase
        {
            if review.telemetry.is_some() {
                review.telemetry_step(down);
            } else if down {
                review.cursor_down();
            } else {
                review.cursor_up();
            }
            return true;
        }
        false
    }

    /// Move the import summary pill focus (Continue / Import less / Telemetry
    /// settings) so the simulator can preview each pill's highlighted state.
    /// Returns true when the current screen is the read-only summary.
    fn onboarding_sim_step_summary_pill(&mut self, forward: bool) -> bool {
        if let Some(flow) = self.onboarding_flow.as_mut()
            && let OnboardingPhase::Login {
                import: Some(review),
            } = &mut flow.phase
            && !review.choosing
            && review.telemetry.is_none()
        {
            review.summary_step(forward);
            return true;
        }
        false
    }

    /// Intercept keys while the simulator is active. Returns true when consumed.
    ///
    /// The simulator owns key handling so the real onboarding key handlers never
    /// fire (which would attempt actual logins/imports). Tab/Shift+Tab step
    /// between screens; on the import screen Up/Down move the checkbox cursor;
    /// h/l preview the highlight; Esc/q exits.
    pub(super) fn handle_onboarding_sim_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        if self.onboarding_sim.is_none() {
            return false;
        }
        // Let the toggle hotkey (Cmd+5) also exit while active.
        if modifiers.contains(KeyModifiers::SUPER) && matches!(code, KeyCode::Char('5')) {
            self.stop_onboarding_simulator();
            return true;
        }
        let on_import_screen = matches!(
            self.onboarding_flow.as_ref().map(|f| &f.phase),
            Some(OnboardingPhase::Login { import: Some(_) })
        );
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.stop_onboarding_simulator();
                true
            }
            // On the import summary, Left/Right preview the three action pills
            // instead of stepping screens (Tab still steps).
            KeyCode::Right if self.onboarding_sim_step_summary_pill(true) => {
                self.force_full_redraw = true;
                true
            }
            KeyCode::Left if self.onboarding_sim_step_summary_pill(false) => {
                self.force_full_redraw = true;
                true
            }
            KeyCode::Tab | KeyCode::Right => {
                self.onboarding_sim_step(1);
                true
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.onboarding_sim_step(-1);
                true
            }
            // Enter/Space advance between screens (don't commit a real import).
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.onboarding_sim_step(1);
                true
            }
            // On the import screen, Up/Down move the checkbox cursor; elsewhere
            // they have no effect (already consumed so nothing leaks through).
            KeyCode::Up if on_import_screen => {
                self.onboarding_sim_move_cursor(false);
                self.force_full_redraw = true;
                true
            }
            KeyCode::Down if on_import_screen => {
                self.onboarding_sim_move_cursor(true);
                self.force_full_redraw = true;
                true
            }
            KeyCode::Char('h') | KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.onboarding_sim_set_highlight(true);
                self.force_full_redraw = true;
                true
            }
            KeyCode::Char('l') | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.onboarding_sim_set_highlight(false);
                self.force_full_redraw = true;
                true
            }
            _ => true,
        }
    }
}
