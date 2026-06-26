//! Control logic / phase transitions for the first-run onboarding flow.
//!
//! See [`super::onboarding_flow`] for the phase definitions. This module hangs
//! the driving methods off `App` so the rest of the TUI can advance the flow in
//! response to login, model selection, key presses, and the auto-advance timer.

use super::onboarding_flow::{
    ExternalCli, ImportReview, OnboardingFlow, OnboardingPendingValidation, OnboardingPhase,
};
use super::{App, DisplayMessage, SessionPickerMode};
use crate::tui::session_picker::{self, SessionFilterMode, SessionPicker};
use crossterm::event::KeyCode;
use std::cell::RefCell;
use std::time::Instant;

/// Lines scrolled per PgUp/PgDn in the "Searched, not found" onboarding panel.
const NOT_FOUND_SCROLL_STEP: i32 = 3;

impl App {
    /// Whether the guided onboarding flow is currently driving the UI.
    pub(super) fn onboarding_flow_active(&self) -> bool {
        self.onboarding_flow
            .as_ref()
            .map(OnboardingFlow::is_active)
            .unwrap_or(false)
    }

    /// The current onboarding phase, if the flow is active.
    pub(super) fn onboarding_phase(&self) -> Option<&OnboardingPhase> {
        self.onboarding_flow
            .as_ref()
            .filter(|flow| flow.is_active())
            .map(|flow| &flow.phase)
    }

    /// Gate + start the flow after a successful login. Only fires for brand-new
    /// users (no prior onboarding flow this session) so returning users who
    /// re-auth aren't dragged through onboarding.
    pub(super) fn maybe_begin_onboarding_flow_after_login(&mut self) {
        // If the flow is already running, a successful login means we should
        // leave the in-TUI `Login` phase and continue into model selection.
        if self.onboarding_flow.is_some() {
            self.onboarding_after_login();
            return;
        }
        if !self.onboarding_preview_mode
            && (self.is_selfdev_canary_session() || !self.is_new_user_for_onboarding())
        {
            return;
        }
        self.begin_onboarding_flow();
    }

    /// One-shot startup check: the fresh-install path logs the user in at the CLI
    /// *before* the TUI launches, so no in-TUI login event ever fires. If we boot
    /// already authenticated as a brand-new user, kick the guided flow here.
    ///
    /// Returns without committing the one-shot guard until auth is actually
    /// resolved (the server may still be bootstrapping on the first ticks), so a
    /// momentary "not yet authenticated" reading doesn't permanently skip the
    /// flow. Once we either start the flow or conclude it shouldn't run, the
    /// guard is set and this becomes a no-op for the rest of the session.
    pub(super) fn maybe_begin_onboarding_flow_on_startup(&mut self) {
        if self.onboarding_startup_checked {
            return;
        }
        if self.onboarding_flow.is_some() {
            self.onboarding_startup_checked = true;
            return;
        }
        // Don't hijack a session that already has real activity (resume,
        // restored input, or a genuine conversation already on screen). These
        // are settled states, so we can commit the guard.
        //
        // A brand-new session still carries one synthetic `<system-reminder>`
        // "Session Context" message (role=user) plus assorted system scaffolding.
        // Those are not real activity, so we ignore them when deciding whether
        // the session is already in use.
        let has_real_conversation = self.display_messages.iter().any(|m| {
            let role = m.role.as_str();
            let is_system_reminder =
                role == "user" && m.content.trim_start().starts_with("<system-reminder>");
            let is_scaffolding =
                matches!(role, "system" | "usage" | "overnight" | "background_task");
            !is_system_reminder && !is_scaffolding
        });
        if has_real_conversation || self.is_processing || !self.input.is_empty() {
            self.onboarding_startup_checked = true;
            return;
        }
        // Self-dev / canary sessions are explicitly not first-run users: they are
        // spawned by developers (e.g. the niri `jcode self-dev` hotkey) and that
        // launch path never increments `launch_count`, so the new-user heuristic
        // would otherwise re-onboard on every spawn. Skip onboarding for them.
        if self.is_selfdev_canary_session() {
            self.onboarding_startup_checked = true;
            return;
        }
        if !self.is_new_user_for_onboarding() {
            self.onboarding_startup_checked = true;
            return;
        }
        // Fresh installs no longer log in at the CLI before the TUI launches.
        // If we boot without working credentials, start the flow at the in-TUI
        // `Login` phase. If credentials already exist, start the post-login
        // onboarding path directly; we no longer ask first-run users to choose a
        // model before they can get started.
        self.onboarding_startup_checked = true;
        if crate::auth::AuthStatus::check_fast().has_any_available() {
            self.begin_onboarding_flow();
        } else {
            self.begin_onboarding_flow_at_login();
        }
    }

    /// Whether this install looks like a brand-new user (few launches).
    fn is_new_user_for_onboarding(&self) -> bool {
        crate::storage::jcode_dir()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join("setup_hints.json")).ok())
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .and_then(|v| v.get("launch_count")?.as_u64())
            .map(|count| count <= 5)
            .unwrap_or(true)
    }

    /// Whether this is a self-dev / canary session.
    ///
    /// These are launched by developers working on jcode itself (for example the
    /// niri `jcode self-dev` hotkey). That launch path bypasses
    /// `maybe_show_setup_hints`, so `launch_count` never advances and the
    /// new-user heuristic above would otherwise treat every spawn as a first run.
    /// Such sessions should never auto-start the guided onboarding flow.
    fn is_selfdev_canary_session(&self) -> bool {
        if self.is_remote {
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        }
    }

    /// Begin the guided post-login flow. Called once auth becomes available on a
    /// fresh install (login/import completes). New users are not forced through a
    /// model picker; the default route is used and `/model` remains available.
    ///
    /// No-op if a flow is already running or the user is experienced.
    pub(super) fn begin_onboarding_flow(&mut self) {
        if self.onboarding_flow.is_some() {
            return;
        }
        self.onboarding_flow = Some(OnboardingFlow::begin());
        self.onboarding_after_model_select();
    }

    /// Begin the guided flow at the in-TUI `Login` phase. Used on a fresh
    /// install that booted without working credentials (the CLI no longer logs
    /// in before the TUI launches).
    ///
    /// If we detect importable external logins (Codex/Claude/Cursor/etc.), we
    /// arm a per-candidate yes/no walkthrough so the user can step through each
    /// detected login and choose whether to import it. Otherwise we ask a simple
    /// "Log in to OpenAI?" Yes/No.
    ///
    /// No-op if a flow is already running.
    pub(super) fn begin_onboarding_flow_at_login(&mut self) {
        if self.onboarding_flow.is_some() {
            return;
        }
        // Detect importable external logins and, if any, build a per-candidate
        // yes/no walkthrough rendered by the onboarding welcome screen. The
        // search report also carries the families we probed but did not find, so
        // the card can show a "Searched, not found" panel beneath the decision.
        let (import, login_not_found) = match crate::external_auth::external_auth_search_report() {
            Ok(report) => {
                let not_found = report
                    .not_found()
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>();
                (ImportReview::new(report.found), not_found)
            }
            Err(err) => {
                crate::logging::error(&format!(
                    "onboarding: failed to inspect external login sources: {err}"
                ));
                (None, Vec::new())
            }
        };
        let had_imports = import.is_some();
        self.onboarding_flow = Some(OnboardingFlow::begin_at_login_with_not_found(
            import,
            login_not_found,
        ));
        // The login prompt is rendered by the onboarding welcome screen
        // (`onboarding_welcome_kind`) so it survives in remote mode.
        if had_imports {
            self.set_status_notice(
                "Welcome to jcode: review detected logins (arrows/hl to move, Enter to choose)",
            );
        } else {
            self.set_status_notice(
                "Log in to OpenAI? Yes/No - hl to move, Enter to choose (No picks another provider)",
            );
        }
    }

    /// Start the default first-run login when no external logins were detected.
    /// We point brand-new users straight at OpenAI (ChatGPT) rather than the full
    /// provider picker, since that is the most common first login. The provider
    /// picker is still reachable via `/login`.
    pub(super) fn onboarding_start_default_login(&mut self) {
        crate::telemetry::record_setup_step_once("login_picker_opened");
        self.start_login_provider(crate::provider_catalog::OPENAI_LOGIN_PROVIDER);
        self.set_status_notice("Login: opening OpenAI sign-in (or type /login for others)");
    }

    /// Advance out of a login phase once credentials are available. We no longer
    /// ask the user about prompt/transcript telemetry here: content sharing
    /// stays off by default (the separate anonymous-usage telemetry is still
    /// disclosed on the welcome screen). Advance straight to model selection.
    /// No-op unless the flow is in a login phase.
    pub(super) fn onboarding_after_login(&mut self) {
        if !matches!(
            self.onboarding_phase(),
            Some(OnboardingPhase::Login { .. }) | Some(OnboardingPhase::LoginOpenAi { .. })
        ) {
            return;
        }
        // Prompt/transcript content sharing is opt-in and off by default; we
        // intentionally don't prompt for it during onboarding.
        crate::telemetry::set_content_sharing_enabled(false);
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ModelSelect;
        }
        self.onboarding_after_model_select();
    }

    /// Advance out of the model-selection phase once a model has been chosen.
    /// When we detect external Codex / Claude Code transcripts, drop the user
    /// straight into the resume picker (with an onboarding banner + a
    /// "Start a new session" option) instead of asking a separate Yes/No
    /// "continue where you left off" question. When both CLIs are present we
    /// show *both* their transcripts together in one combined, recency-sorted
    /// list rather than hiding one behind the other.
    pub(super) fn onboarding_after_model_select(&mut self) {
        if !matches!(self.onboarding_phase(), Some(OnboardingPhase::ModelSelect)) {
            return;
        }
        let present = crate::tui::app::onboarding_flow::detect_external_cli_oauths();
        if present.is_empty() {
            self.onboarding_show_suggestions();
        } else {
            self.onboarding_open_transcript_picker(&present);
        }
    }

    /// Enter the "Continue where you left off?" phase. Highlightable Yes/No
    /// with a [`DECISION_TIMEOUT`] countdown; the default (and timeout choice)
    /// is "Yes" so the resume menu opens unless the user declines.
    ///
    /// Retained for compatibility with replay/test fixtures and the
    /// `ContinuePrompt` rendering/key/tick paths. The live onboarding flow now
    /// opens the resume picker directly instead of asking this Yes/No question.
    #[allow(dead_code)]
    fn onboarding_enter_continue_prompt(&mut self, cli: ExternalCli) {
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ContinuePrompt {
                cli,
                yes_highlighted: true,
                shown_at: Instant::now(),
            };
        }
        // The continue prompt is rendered by the onboarding welcome screen
        // (`onboarding_welcome_kind`) so it survives in remote mode.
        self.update_onboarding_continue_prompt_status(cli);
    }

    /// Refresh the status notice with the continue-prompt countdown.
    fn update_onboarding_continue_prompt_status(&mut self, cli: ExternalCli) {
        let remaining = self
            .onboarding_flow
            .as_ref()
            .and_then(OnboardingFlow::decision_seconds_remaining)
            .unwrap_or(0);
        self.set_status_notice(format!(
            "Continue a session where you left off in {}? Opens the resume menu in {remaining}s (Yes/No)",
            cli.label()
        ));
    }

    /// Answer the continue prompt. `true` -> open the transcript picker;
    /// `false` -> fall through to the suggestion cards.
    pub(super) fn onboarding_answer_continue(&mut self, wants_continue: bool) {
        let cli = match self.onboarding_phase() {
            Some(OnboardingPhase::ContinuePrompt { cli, .. }) => *cli,
            _ => return,
        };
        if wants_continue {
            self.onboarding_open_transcript_picker(std::slice::from_ref(&cli));
        } else {
            self.onboarding_show_suggestions();
        }
    }

    /// Intercept keys for the guided onboarding welcome phases:
    ///   - `ModelSelect`: we tell the user to run /model; Enter is also a
    ///     shortcut that opens the model picker from the welcome screen.
    ///   - `ContinuePrompt`: Y/Enter continues, N/Esc declines.
    ///   - `LoginOpenAi`: Left/h -> Yes, Right/l -> No, toggle with
    ///     Up/Down/k/j/Tab; y/n commit directly, Enter/Space commit the
    ///     highlighted default (Yes -> OpenAI sign-in, No -> provider picker).
    /// Returns true if the key was consumed.
    pub(super) fn handle_onboarding_continue_prompt_key(&mut self, code: KeyCode) -> bool {
        // The login decision cards can show a scrollable "Searched, not found"
        // panel beneath the Yes/No row. PgUp/PgDn scroll it without disturbing
        // the highlighted choice (which uses h/l/j/k/arrows). These keys are a
        // no-op (and fall through) when no overflowing not-found list is shown.
        if matches!(
            self.onboarding_phase(),
            Some(OnboardingPhase::Login { .. }) | Some(OnboardingPhase::LoginOpenAi { .. })
        ) && self.inline_interactive_state.is_none()
        {
            match code {
                KeyCode::PageDown => {
                    if self.onboarding_scroll_not_found(NOT_FOUND_SCROLL_STEP) {
                        return true;
                    }
                }
                KeyCode::PageUp => {
                    if self.onboarding_scroll_not_found(-NOT_FOUND_SCROLL_STEP) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        match self.onboarding_phase() {
            Some(OnboardingPhase::Login { import }) => {
                // No detected imports remaining: this is the recovery fallback
                // (an import failed or the user declined every detected login).
                // Point them at the provider picker. Only intercept Enter from
                // the welcome screen; if an overlay is already open let it commit.
                if import.is_none() {
                    return match code {
                        KeyCode::Enter if self.inline_interactive_state.is_none() => {
                            self.show_interactive_login();
                            true
                        }
                        _ => false,
                    };
                }
                // A per-candidate import walkthrough is active. Drive it with the
                // arrow / vim keys; Enter or Space commits the highlighted Yes/No
                // and advances. Don't intercept once an inline overlay is open.
                if self.inline_interactive_state.is_some() {
                    return false;
                }
                self.handle_onboarding_import_review_key(code)
            }
            Some(OnboardingPhase::LoginOpenAi { .. }) => {
                // Don't intercept once an inline overlay (the OpenAI sign-in or
                // the provider picker) is already open.
                if self.inline_interactive_state.is_some() {
                    return false;
                }
                self.handle_onboarding_login_openai_key(code)
            }
            Some(OnboardingPhase::ModelSelect) => match code {
                // Enter opens the model picker, but only from the welcome
                // screen. If a picker (or any inline overlay) is already open,
                // let it handle Enter so the selection can commit.
                KeyCode::Enter if self.inline_interactive_state.is_none() => {
                    self.open_model_picker();
                    true
                }
                _ => false,
            },
            Some(OnboardingPhase::ContinuePrompt { .. }) => {
                self.handle_onboarding_continue_choice_key(code)
            }
            Some(OnboardingPhase::ScrollWmOptIn { .. }) => {
                // Ignore keys while an install is running (the card shows a
                // progress line, not the Yes/No row).
                if self.scrollwm_install_progress.is_some() {
                    return false;
                }
                self.handle_onboarding_scrollwm_optin_key(code)
            }
            _ => false,
        }
    }

    /// Handle a key while the "continue where you left off?" prompt is up.
    /// Yes/No sit side by side (default highlight is "Yes"), matching the
    /// import and telemetry-consent prompts:
    ///   - Left / h  -> highlight "Yes"
    ///   - Right / l -> highlight "No"
    ///   - Up / Down / k / j / Tab -> toggle
    ///   - y / Y -> continue;  n / N / Esc -> decline (both commit)
    ///   - Enter / Space -> commit the highlighted choice
    fn handle_onboarding_continue_choice_key(&mut self, code: KeyCode) -> bool {
        let cli = match self.onboarding_phase() {
            Some(OnboardingPhase::ContinuePrompt { cli, .. }) => *cli,
            _ => return false,
        };
        let Some(flow) = self.onboarding_flow.as_mut() else {
            return false;
        };
        let OnboardingPhase::ContinuePrompt {
            yes_highlighted, ..
        } = &mut flow.phase
        else {
            return false;
        };
        match code {
            KeyCode::Left | KeyCode::Char('h') => {
                *yes_highlighted = true;
                self.update_onboarding_continue_prompt_status(cli);
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                *yes_highlighted = false;
                self.update_onboarding_continue_prompt_status(cli);
                true
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('k')
            | KeyCode::Char('j')
            | KeyCode::Tab => {
                *yes_highlighted = !*yes_highlighted;
                self.update_onboarding_continue_prompt_status(cli);
                true
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.onboarding_answer_continue(true);
                true
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.onboarding_answer_continue(false);
                true
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let wants_continue = *yes_highlighted;
                self.onboarding_answer_continue(wants_continue);
                true
            }
            _ => false,
        }
    }

    /// Handle a key while the per-candidate import walkthrough is active.
    /// Returns true if the key was consumed.
    ///
    /// The Yes / No options sit side by side, so any movement key simply moves
    /// the highlight between them:
    ///   - Left / h  -> highlight "Yes"
    ///   - Right / l -> highlight "No"
    ///   - Up / Down / k / j / Tab -> toggle between Yes and No
    ///   - y / Y     -> choose "Yes" and commit
    ///   - n / N     -> choose "No" and commit
    ///   - Enter / Space -> commit the highlighted choice, advance
    fn handle_onboarding_import_review_key(&mut self, code: KeyCode) -> bool {
        // Mutate the live review in place, and report whether the walkthrough
        // finished so we can kick off the import outside the borrow.
        let mut finished = false;
        // Whether this key advanced to the next candidate (committed a row), so
        // we can reset the not-found scroll only on a real advance.
        let mut advanced = false;
        {
            let Some(review) = self.onboarding_import_review_mut() else {
                return false;
            };
            match code {
                KeyCode::Left | KeyCode::Char('h') => review.set_yes(true),
                KeyCode::Right | KeyCode::Char('l') => review.set_yes(false),
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char('k')
                | KeyCode::Char('j')
                | KeyCode::Tab => review.toggle(),
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    review.set_yes(true);
                    finished = review.commit_current();
                    advanced = true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    review.set_yes(false);
                    finished = review.commit_current();
                    advanced = true;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    finished = review.commit_current();
                    advanced = true;
                }
                _ => return false,
            }
        }
        if finished {
            self.onboarding_finish_import_review();
        } else {
            if advanced {
                // Advancing to the next candidate restarts the not-found panel
                // at the top so each decision row begins from the same place.
                self.reset_onboarding_not_found_scroll();
            }
            self.update_onboarding_import_review_status();
        }
        true
    }

    /// Number of "Searched, not found" rows for the active login flow (0 when
    /// there is no flow or nothing was probed/absent).
    fn onboarding_not_found_len(&self) -> usize {
        self.onboarding_flow
            .as_ref()
            .map(|flow| flow.login_not_found.len())
            .unwrap_or(0)
    }

    /// Scroll the not-found panel by `delta` lines (positive = down), clamped to
    /// the panel's scrollable range. Returns `true` if the offset changed (the
    /// key was consumed), `false` when the list doesn't overflow or is already
    /// at the requested edge (so the key can fall through to other handlers).
    fn onboarding_scroll_not_found(&mut self, delta: i32) -> bool {
        let total = self.onboarding_not_found_len();
        let current = self.onboarding_notfound_scroll;
        let raw = (current as i32 + delta).max(0);
        let clamped = crate::tui::ui::onboarding::clamp_not_found_scroll(
            total,
            raw.min(u16::MAX as i32) as u16,
        );
        if clamped == current {
            return false;
        }
        self.onboarding_notfound_scroll = clamped;
        true
    }

    /// Reset the not-found scroll offset to the top. Called whenever the login
    /// decision row advances or the login phase (re)starts so each decision
    /// begins at the top of the shared not-found list.
    pub(super) fn reset_onboarding_not_found_scroll(&mut self) {
        self.onboarding_notfound_scroll = 0;
    }

    /// Handle a key while the "Set up ScrollWM?" opt-in is up. Yes/No sit side
    /// by side (default highlight is "No"):
    ///   - Left / h  -> highlight "Yes"
    ///   - Right / l -> highlight "No"
    ///   - Up / Down / k / j / Tab -> toggle
    ///   - y / Y -> install ScrollWM;  n / N / Esc -> skip
    ///   - Enter / Space -> commit the highlighted choice
    fn handle_onboarding_scrollwm_optin_key(&mut self, code: KeyCode) -> bool {
        {
            let Some(flow) = self.onboarding_flow.as_mut() else {
                return false;
            };
            let OnboardingPhase::ScrollWmOptIn { yes_highlighted, .. } = &mut flow.phase else {
                return false;
            };
            match code {
                KeyCode::Left | KeyCode::Char('h') => {
                    *yes_highlighted = true;
                    self.update_onboarding_scrollwm_optin_status();
                    return true;
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    *yes_highlighted = false;
                    self.update_onboarding_scrollwm_optin_status();
                    return true;
                }
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char('k')
                | KeyCode::Char('j')
                | KeyCode::Tab => {
                    *yes_highlighted = !*yes_highlighted;
                    self.update_onboarding_scrollwm_optin_status();
                    return true;
                }
                _ => {}
            }
        }
        // Commit paths re-borrow `self`, so handle them after the mutable phase
        // borrow above is released.
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.onboarding_answer_scrollwm_optin(true);
                true
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.onboarding_answer_scrollwm_optin(false);
                true
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let wants_install = matches!(
                    self.onboarding_phase(),
                    Some(OnboardingPhase::ScrollWmOptIn {
                        yes_highlighted: true,
                        ..
                    })
                );
                self.onboarding_answer_scrollwm_optin(wants_install);
                true
            }
            _ => false,
        }
    }

    /// Handle a key while the "Log in to OpenAI?" prompt is up. Yes/No sit side
    /// by side (default highlight is "Yes"):
    ///   - Left / h  -> highlight "Yes"
    ///   - Right / l -> highlight "No"
    ///   - Up / Down / k / j / Tab -> toggle
    ///   - y / Y -> log in to OpenAI;  n / N -> open the provider picker
    ///   - Enter / Space -> commit the highlighted choice
    fn handle_onboarding_login_openai_key(&mut self, code: KeyCode) -> bool {
        let Some(flow) = self.onboarding_flow.as_mut() else {
            return false;
        };
        let OnboardingPhase::LoginOpenAi { yes_highlighted } = &mut flow.phase else {
            return false;
        };
        match code {
            KeyCode::Left | KeyCode::Char('h') => {
                *yes_highlighted = true;
                self.update_onboarding_login_openai_status();
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                *yes_highlighted = false;
                self.update_onboarding_login_openai_status();
                true
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('k')
            | KeyCode::Char('j')
            | KeyCode::Tab => {
                *yes_highlighted = !*yes_highlighted;
                self.update_onboarding_login_openai_status();
                true
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.onboarding_answer_login_openai(true);
                true
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.onboarding_answer_login_openai(false);
                true
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let wants_openai = *yes_highlighted;
                self.onboarding_answer_login_openai(wants_openai);
                true
            }
            _ => false,
        }
    }

    /// Answer the "Log in to OpenAI?" prompt. Yes starts the OpenAI sign-in;
    /// No opens the full provider picker so the user can pick another provider.
    pub(super) fn onboarding_answer_login_openai(&mut self, wants_openai: bool) {
        if !matches!(
            self.onboarding_phase(),
            Some(OnboardingPhase::LoginOpenAi { .. })
        ) {
            return;
        }
        if wants_openai {
            self.onboarding_start_default_login();
        } else {
            self.show_interactive_login();
        }
    }

    /// Refresh the status notice for the "Log in to OpenAI?" prompt.
    fn update_onboarding_login_openai_status(&mut self) {
        let yes = matches!(
            self.onboarding_phase(),
            Some(OnboardingPhase::LoginOpenAi {
                yes_highlighted: true
            })
        );
        let choice = if yes { "Yes" } else { "No" };
        self.set_status_notice(format!(
            "Log in to OpenAI? [{choice}] - hl to move, Enter to choose (No picks another provider)"
        ));
    }

    /// Mutable access to the active import walkthrough, if any.
    fn onboarding_import_review_mut(&mut self) -> Option<&mut ImportReview> {
        match self.onboarding_flow.as_mut()?.phase {
            OnboardingPhase::Login {
                import: Some(ref mut review),
            } => Some(review),
            _ => None,
        }
    }

    /// Refresh the status notice to reflect the current import-review position.
    fn update_onboarding_import_review_status(&mut self) {
        if let Some(review) = self.onboarding_import_review_mut()
            && let Some(candidate) = review.current()
        {
            let notice = format!(
                "Import {} ({} of {})? Yes/No - hl to move, Enter to choose, auto in {}s",
                candidate.provider_summary(),
                review.position(),
                review.total(),
                review.seconds_remaining(),
            );
            self.set_status_notice(notice);
        }
    }

    /// The walkthrough is complete: run the import for the approved candidates
    /// (if any), then either advance the flow or wait for the import result.
    fn onboarding_finish_import_review(&mut self) {
        // Take the candidates and approved indices out of the phase, then clear
        // the import sub-state so the welcome card stops rendering the prompt.
        let (candidates, approved) = match self.onboarding_import_review_mut() {
            Some(review) => (review.candidates.clone(), review.approved.clone()),
            None => return,
        };
        if let Some(flow) = self.onboarding_flow.as_mut()
            && let OnboardingPhase::Login { ref mut import } = flow.phase
        {
            *import = None;
        }

        if approved.is_empty() {
            // The user declined every detected login. Fall back to manual login
            // so they can still authenticate.
            self.set_status_notice("No logins imported. Press Enter to choose a provider.");
            return;
        }

        // Kick off the import on the runtime; the LoginCompleted event advances
        // onboarding and activates the provider.
        self.set_status_notice("Login: importing selected logins...");
        tokio::spawn(async move {
            let outcome = match crate::external_auth::run_external_auth_auto_import_candidates(
                &candidates,
                &approved,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(err) => {
                    crate::bus::Bus::global().publish(crate::bus::BusEvent::LoginCompleted(
                        crate::bus::LoginCompleted {
                            provider: "auto-import".to_string(),
                            success: false,
                            message: format!("Auto import failed: {}", err),
                        },
                    ));
                    return;
                }
            };
            // Auto-import bypasses the manual `pending_login` path, so record
            // `auth_success` here for each imported provider. Without this the
            // onboarding activation funnel undercounts every imported login
            // (the happy path of the guided first-run flow).
            for (provider, method) in &outcome.imported_auth_labels {
                crate::telemetry::record_auth_success(provider, method);
            }
            crate::bus::Bus::global().publish(crate::bus::BusEvent::LoginCompleted(
                crate::bus::LoginCompleted {
                    provider: "auto-import".to_string(),
                    success: outcome.imported > 0,
                    message: outcome.render_markdown(),
                },
            ));
        });
    }

    /// Open a single-select resume-style picker showing the transcripts of every
    /// detected external CLI together (Codex and/or Claude Code), sorted by
    /// recency. Falls back to the session-search prompt if none load.
    ///
    /// `clis` is the set of external CLIs the user is logged into. When more than
    /// one is present we still show them in one combined list so the user never
    /// has a CLI's history hidden behind the other.
    pub(super) fn onboarding_open_transcript_picker(&mut self, clis: &[ExternalCli]) {
        // Choose a representative CLI for the banner/mode headline: the one with
        // the most recent transcript (falling back to detection order).
        let headline_cli = clis
            .iter()
            .copied()
            .max_by_key(|cli| session_picker::latest_external_cli_session_secs(*cli).unwrap_or(0))
            .or_else(|| clis.first().copied())
            .unwrap_or(ExternalCli::Codex);

        let multi = clis.len() > 1;
        let filter = if multi {
            SessionFilterMode::ExternalClis
        } else {
            match headline_cli {
                ExternalCli::Codex => SessionFilterMode::Codex,
                ExternalCli::ClaudeCode => SessionFilterMode::ClaudeCode,
            }
        };

        // The onboarding picker only shows external CLI transcripts, so load just
        // those instead of paying the full `load_sessions_grouped` cost (parsing
        // every jcode snapshot and listing servers). This keeps first-run
        // onboarding snappy while still surfacing every logged-in CLI.
        let (server_groups, orphan_sessions) =
            session_picker::load_external_cli_sessions_grouped_multi(clis);

        let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
        picker.activate_external_cli_filter(filter);

        if picker.visible_session_count() == 0 {
            self.onboarding_fallback_to_session_search(headline_cli);
            return;
        }

        picker.activate_onboarding_banner(Self::onboarding_resume_banner_lines(clis));

        self.session_picker_overlay = Some(RefCell::new(picker));
        self.session_picker_mode = SessionPickerMode::Onboarding { cli: headline_cli };
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::TranscriptPick {
                cli: headline_cli,
                shown_at: Instant::now(),
            };
        }
        let resume_label = if multi {
            "Resume a Codex or Claude Code session".to_string()
        } else {
            format!("Resume a {} session", headline_cli.label())
        };
        self.set_status_notice(format!(
            "{resume_label} (↑↓ to choose, Enter to resume) or pick \"Start a new session\""
        ));
    }

    /// Formatted onboarding prompt shown in the reserved top band of the
    /// resume picker on first run.
    fn onboarding_resume_banner_lines(clis: &[ExternalCli]) -> Vec<ratatui::text::Line<'static>> {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};
        let accent = crate::tui::color_support::rgb(186, 139, 255);
        // Describe whichever CLIs were detected: "Codex", "Claude Code", or
        // "Codex and Claude Code" when both are present.
        let mut labels: Vec<&'static str> = Vec::new();
        for cli in clis {
            let label = cli.label();
            if !labels.contains(&label) {
                labels.push(label);
            }
        }
        let found = match labels.as_slice() {
            [] => "external".to_string(),
            [only] => (*only).to_string(),
            [first, second] => format!("{first} and {second}"),
            _ => labels.join(", "),
        };
        vec![
            Line::from(vec![Span::styled(
                "Welcome to jcode 🎉",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![Span::styled(
                format!(
                    "We found your {found} sessions. Pick one below to pick up right where you left off,"
                ),
                Style::default().fg(Color::White),
            )]),
            Line::from(vec![Span::styled(
                "or start fresh with a brand-new session.",
                Style::default().fg(Color::White),
            )]),
        ]
    }

    /// Fallback when an external CLI login is present but no resumable
    /// transcripts load: just land the user on the clean new-session screen with
    /// the prompt-suggestion cards. We intentionally do NOT auto-submit a
    /// "search for my last session" turn here; firing an agent turn the user
    /// never asked for on first run is surprising. They can resume later via
    /// `/resume` if they want.
    pub(super) fn onboarding_fallback_to_session_search(&mut self, _cli: ExternalCli) {
        self.onboarding_show_suggestions();
    }

    /// Drop into the suggestion-card state (the "No" / no-OAuth path). Prints
    /// the same starter prompts the empty-screen welcome offers, as an inline
    /// numbered list the user can pick by typing the number or anything else.
    ///
    /// This is also the "Start a new session" landing screen on first run. We
    /// intentionally keep it clean: the usual login/import system chatter is
    /// suppressed while onboarding drives the UI, and instead of that noise we
    /// kick off a single lightweight live validation of the auto-selected
    /// default model and report it as one tidy "ready"/"failed" line.
    pub(super) fn onboarding_show_suggestions(&mut self) {
        // Before landing on the suggestion cards, offer the one-time ScrollWM
        // opt-in (macOS, not installed, not yet answered). On any other
        // platform / state this is a no-op and we fall through immediately.
        if self.should_offer_scrollwm_optin() {
            self.onboarding_enter_scrollwm_optin();
            return;
        }
        self.onboarding_finish_to_suggestions();
    }

    /// Render the starter suggestion cards and kick off the new-session model
    /// validation. This is the real terminal step of onboarding; the ScrollWM
    /// opt-in (when shown) calls this once the user answers.
    pub(super) fn onboarding_finish_to_suggestions(&mut self) {
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Suggestions;
        }
        let suggestions = self.suggestion_prompts();
        if suggestions.is_empty() {
            self.onboarding_finish();
            self.set_status_notice("You're all set, type anything to start");
            self.onboarding_validate_default_model();
            return;
        }
        let mut body = String::from("Here are a few things you can try:\n");
        for (i, (label, _prompt)) in suggestions.iter().enumerate() {
            body.push_str(&format!("  [{}] {}\n", i + 1, label));
        }
        body.push_str(&format!(
            "Press 1-{} to use one, or just type anything to start.",
            suggestions.len()
        ));
        self.push_display_message(DisplayMessage::system(body));
        self.set_status_notice("Try a suggestion, or type anything to start");
        self.onboarding_validate_default_model();
    }

    /// Whether to offer the one-time "Set up ScrollWM?" opt-in. macOS only,
    /// local (non-remote) sessions only, only when ScrollWM is not already
    /// installed and the user hasn't answered the prompt before.
    ///
    /// Disabled under `cfg!(test)` so unit tests that walk to the suggestion
    /// cards stay deterministic regardless of the host's real `~/Applications`
    /// / setup-hints state; the decision logic itself is covered by
    /// [`scrollwm_optin_should_offer`] and the wiring by direct-phase tests.
    fn should_offer_scrollwm_optin(&self) -> bool {
        if cfg!(test) {
            return false;
        }
        scrollwm_optin_should_offer(
            cfg!(target_os = "macos"),
            self.is_remote,
            scrollwm_app_installed(),
            crate::setup_hints::SetupHintsState::load().scrollwm_optin_answered,
        )
    }

    /// Enter the ScrollWM opt-in phase. Default highlight is **No** so a
    /// timeout / accidental Enter never installs third-party software.
    pub(super) fn onboarding_enter_scrollwm_optin(&mut self) {
        self.scrollwm_install_progress = None;
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ScrollWmOptIn {
                yes_highlighted: false,
                shown_at: Instant::now(),
            };
        }
        self.update_onboarding_scrollwm_optin_status();
    }

    /// Refresh the status notice with the ScrollWM opt-in countdown.
    fn update_onboarding_scrollwm_optin_status(&mut self) {
        let remaining = self
            .onboarding_flow
            .as_ref()
            .and_then(OnboardingFlow::decision_seconds_remaining)
            .unwrap_or(0);
        self.set_status_notice(format!(
            "Set up ScrollWM? [No] - hl to move, Enter to choose, skips in {remaining}s"
        ));
    }

    /// Record the opt-in answer and either kick off the install (Yes) or fall
    /// through to the suggestion cards (No). The answer is persisted before any
    /// async work so we never re-ask, even if the install crashes.
    pub(super) fn onboarding_answer_scrollwm_optin(&mut self, wants_install: bool) {
        let mut state = crate::setup_hints::SetupHintsState::load();
        state.scrollwm_optin_answered = true;
        if wants_install {
            state.scrollwm_install_started = true;
        }
        let _ = state.save();

        if wants_install {
            self.scrollwm_install_progress = Some(crate::tui::ScrollWmInstallProgress::Running);
            self.update_onboarding_scrollwm_optin_status();
            self.spawn_scrollwm_install();
            // Stay in the ScrollWmOptIn phase so the card shows the Running
            // line; the Bus completion handler advances to the suggestions.
        } else {
            self.scrollwm_install_progress = None;
            self.onboarding_finish_to_suggestions();
        }
    }

    /// Spawn the background ScrollWM install (web installer). Fire-and-forget;
    /// the result is delivered via [`crate::bus::BusEvent::ScrollWmInstallCompleted`]
    /// and handled on the UI thread. Mirrors the async model-validation pattern.
    fn spawn_scrollwm_install(&mut self) {
        let session_id = self.session.id.clone();
        self.set_status_notice("Installing ScrollWM…");
        // The TUI always runs inside a tokio runtime in production; guard the
        // spawn so unit tests (which build an App outside an async context) can
        // exercise the opt-in answer path without panicking on a missing runtime.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            crate::logging::warn(
                "scrollwm: no tokio runtime; skipping background install spawn",
            );
            return;
        };
        handle.spawn(async move {
            let result = run_scrollwm_install().await;
            crate::bus::Bus::global().publish(crate::bus::BusEvent::ScrollWmInstallCompleted(
                crate::bus::ScrollWmInstallCompleted {
                    session_id,
                    ok: result.is_ok(),
                    detail: result.err(),
                },
            ));
        });
    }

    /// Handle the background ScrollWM install result on the UI thread: show a
    /// one-line outcome and advance onboarding to the suggestion cards.
    /// Returns true if the event was for this session (consumed).
    pub(super) fn handle_scrollwm_install_completed(
        &mut self,
        ev: crate::bus::ScrollWmInstallCompleted,
    ) -> bool {
        if ev.session_id != self.session.id {
            return false;
        }
        let message = if ev.ok {
            "ScrollWM installed. Grant **Accessibility** when System Settings opens; \
             it then arranges your windows automatically."
                .to_string()
        } else {
            let detail = ev
                .detail
                .unwrap_or_else(|| "the installer did not finish".to_string());
            format!(
                "ScrollWM install didn't finish ({detail}). You can install it later:\n  \
                 curl -fsSL https://raw.githubusercontent.com/1jehuang/scrollwm/main/scripts/web-install.sh | bash"
            )
        };
        self.scrollwm_install_progress = None;
        // Only drive the flow forward if we are still parked on the opt-in
        // card; otherwise just surface the message.
        let in_optin = matches!(
            self.onboarding_phase(),
            Some(OnboardingPhase::ScrollWmOptIn { .. })
        );
        self.push_display_message(DisplayMessage::system(message));
        if in_optin {
            self.onboarding_finish_to_suggestions();
        }
        true
    }

    /// Friendly label for the active default model, including the reasoning
    /// effort tier when one applies (e.g. "GPT-5.5 (low)"). Used by the
    /// onboarding new-session validation line.
    fn onboarding_default_model_label(&self) -> String {
        let model = self.onboarding_default_model_id();
        let pretty = super::helpers::pretty_model_display_name(&model);
        match self.provider.reasoning_effort() {
            Some(effort) if !effort.trim().is_empty() && effort != "none" => {
                let effort_label = super::helpers::effort_display_label(&effort);
                format!("{} ({})", pretty, effort_label.to_ascii_lowercase())
            }
            _ => pretty,
        }
    }

    /// Resolve the raw id of the default model the new-session screen is about
    /// to use. In remote/client mode the live model is reported by the server,
    /// so prefer the same resolution the header uses; fall back to the session
    /// model and finally the local provider's model.
    fn onboarding_default_model_id(&self) -> String {
        if self.is_remote {
            if let Some(model) = self.effective_remote_provider_model() {
                return model;
            }
        }
        self.session
            .model
            .clone()
            .filter(|m| !m.trim().is_empty() && !m.eq_ignore_ascii_case("unknown"))
            .unwrap_or_else(|| self.provider.model())
    }

    /// Request a one-shot, lightweight live validation of the auto-selected
    /// default model for the clean new-session screen. We want a single line
    /// that tells the user their default model is actually working, rather than
    /// the usual login/import status spam.
    ///
    /// In remote/client mode the live default model is reported by the server
    /// asynchronously, so firing immediately can race ahead of the model id
    /// being known (resolving to "unknown" and validating the wrong provider).
    /// Instead we record a pending request and let `onboarding_tick` fire it
    /// once a concrete model id is available (or a short timeout elapses).
    pub(super) fn onboarding_validate_default_model(&mut self) {
        if !crate::auth::AuthStatus::check_fast().has_any_available() {
            return;
        }
        // Remote mode after a login: the server pushes a fresh model catalog a
        // moment later (e.g. switching the route to gpt-5.5 after an OpenAI
        // login). The pre-login default (e.g. Claude Opus) is already a concrete
        // id, so validating immediately would report the *stale* model. Defer
        // until the catalog generation advances (or a short timeout) so the
        // readiness line names the freshly-selected model.
        if self.is_remote && self.recent_authenticated_provider.is_some() {
            self.onboarding_pending_model_validation =
                Some(OnboardingPendingValidation::awaiting_catalog_refresh(
                    self.session.id.clone(),
                    self.remote_model_catalog_generation,
                ));
            return;
        }
        // If we already know a concrete model (typically local mode), run it
        // right away; otherwise defer to the tick loop until the server reports
        // the live model id.
        if self.onboarding_default_model_id_is_concrete() {
            self.onboarding_spawn_model_validation();
        } else {
            self.onboarding_pending_model_validation =
                Some(OnboardingPendingValidation::new(self.session.id.clone()));
        }
    }

    /// Whether we currently have a concrete (non-"unknown") default model id to
    /// validate. In remote mode this becomes true once the server reports the
    /// live model.
    fn onboarding_default_model_id_is_concrete(&self) -> bool {
        let model = self.onboarding_default_model_id();
        let trimmed = model.trim();
        !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("unknown")
    }

    /// Spawn the background validation ping for the current default model.
    fn onboarding_spawn_model_validation(&mut self) {
        let Some(provider) = self.onboarding_validation_provider() else {
            return;
        };
        let model_label = self.onboarding_default_model_label();
        let provider_key = crate::session::derive_session_provider_key(provider.name());
        let session_id = self.session.id.clone();
        // Whether to also run a definitive live Copilot auth check. Copilot is
        // unusual: a local GitHub token can exist while the account is banned or
        // not entitled, so the presence-only probe used by the readiness summary
        // would otherwise show a banned account as "Ready to use". We skip it
        // when Copilot is the default provider (the model ping already covers
        // it) or when no Copilot credentials are present locally.
        let verify_copilot = provider_key.as_deref() != Some("copilot")
            && crate::auth::copilot::has_copilot_credentials_fast();
        self.set_status_notice(format!("Checking {model_label}..."));
        tokio::spawn(async move {
            // Run the definitive Copilot auth check first so its validation
            // record is persisted (and the auth cache invalidated) before the
            // readiness summary reads `check_fast()` below.
            if verify_copilot {
                let _ = crate::auth::copilot::verify_copilot_credentials_live_default().await;
            }
            let (ok, detail) = match Self::onboarding_run_model_validation(provider).await {
                Ok(()) => (true, None),
                Err(err) => (false, Some(Self::onboarding_trim_validation_error(&err))),
            };
            crate::bus::Bus::global().publish(crate::bus::BusEvent::OnboardingModelValidated(
                crate::bus::OnboardingModelValidated {
                    session_id,
                    model_label,
                    provider_key,
                    ok,
                    detail,
                },
            ));
        });
    }

    /// Drive a pending (deferred) model validation from the onboarding tick.
    /// Returns true if it fired this tick. Fires once a concrete model id is
    /// known, or after a short resolve timeout so the line always appears.
    pub(super) fn onboarding_tick_model_validation(&mut self) -> bool {
        let Some(pending) = self.onboarding_pending_model_validation.as_ref() else {
            return false;
        };
        if pending.session_id != self.session.id {
            // Session changed out from under us; drop the stale request.
            self.onboarding_pending_model_validation = None;
            return false;
        }
        if !self.onboarding_pending_validation_ready_to_fire() {
            return false;
        }
        self.onboarding_pending_model_validation = None;
        self.onboarding_spawn_model_validation();
        true
    }

    /// Whether the currently-pending validation should fire this tick. Pure
    /// decision logic (no side effects) so it can be unit-tested without the
    /// `tokio::spawn` in `onboarding_spawn_model_validation`.
    ///
    /// When waiting for the post-login catalog refresh (remote mode), hold until
    /// the catalog generation advances past the value captured at request time,
    /// so we validate the freshly-selected model rather than the stale pre-login
    /// default. The resolve timeout is always a backstop so the line eventually
    /// appears even if no refresh arrives.
    pub(super) fn onboarding_pending_validation_ready_to_fire(&self) -> bool {
        let Some(pending) = self.onboarding_pending_model_validation.as_ref() else {
            return false;
        };
        let timed_out = pending.resolve_timed_out();
        if pending.await_catalog_refresh {
            let refreshed =
                self.remote_model_catalog_generation > pending.catalog_generation_at_request;
            return refreshed || timed_out;
        }
        self.onboarding_default_model_id_is_concrete() || timed_out
    }

    /// Build the provider used for the onboarding model-validation ping.
    ///
    /// In local mode we fork the live provider. In remote/client mode the app's
    /// `self.provider` is a `NullProvider` (real turns run in the backend), so
    /// we spin up a real local provider and pin it to the displayed session
    /// model so the ping exercises the same model the user is about to use.
    fn onboarding_validation_provider(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::provider::Provider>> {
        if !self.is_remote {
            return Some(self.provider.fork());
        }
        let provider: std::sync::Arc<dyn crate::provider::Provider> =
            std::sync::Arc::new(crate::provider::MultiProvider::new_fast());
        let model = self.onboarding_default_model_id();
        if !model.trim().is_empty() && !model.eq_ignore_ascii_case("unknown") {
            // Best-effort: if the model can't be set locally we still ping the
            // provider default, which is enough to confirm credentials work.
            let _ = provider.set_model(&model);
        }
        Some(provider)
    }

    /// Run the lightweight live validation ping against the active provider.
    /// Succeeds as long as the provider returns any non-empty completion.
    async fn onboarding_run_model_validation(
        provider: std::sync::Arc<dyn crate::provider::Provider>,
    ) -> anyhow::Result<()> {
        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            provider.complete_simple(
                "Reply with exactly: OK",
                "You are validating connectivity. Reply with exactly: OK",
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timed out after 30s"))??;
        if reply.trim().is_empty() {
            anyhow::bail!("empty response");
        }
        Ok(())
    }

    /// Condense a validation error into a short, user-facing detail string.
    ///
    /// Provider errors are often a full JSON blob on a single line; we map the
    /// common cases to a tidy phrase so the onboarding summary stays readable,
    /// and otherwise fall back to a clipped first line.
    fn onboarding_trim_validation_error(err: &anyhow::Error) -> String {
        let msg = err.to_string();
        let lower = msg.to_ascii_lowercase();
        // Common, recognizable failures get a short canonical phrase.
        if lower.contains("401")
            || lower.contains("unauthorized")
            || lower.contains("invalid authentication")
            || lower.contains("invalid api key")
            || lower.contains("invalid x-api-key")
        {
            return "login expired or invalid".to_string();
        }
        if lower.contains("timed out") || lower.contains("timeout") {
            return "timed out".to_string();
        }
        if lower.contains("429") || lower.contains("rate limit") {
            return "rate limited".to_string();
        }
        if lower.contains("empty response") {
            return "no response".to_string();
        }
        let first_line = msg.lines().next().unwrap_or(&msg).trim();
        let trimmed: String = first_line.chars().take(100).collect();
        if trimmed.is_empty() {
            "unknown error".to_string()
        } else {
            trimmed
        }
    }

    /// Whether a validation detail string looks like an authentication failure
    /// (expired/invalid credentials), which is fixed by logging in again rather
    /// than by switching models.
    fn onboarding_detail_looks_like_auth(detail: &str) -> bool {
        let lower = detail.to_ascii_lowercase();
        lower.contains("401")
            || lower.contains("unauthorized")
            || lower.contains("authentication")
            || lower.contains("invalid api key")
            || lower.contains("invalid x-api-key")
            || lower.contains("credentials")
            || lower.contains("login expired")
            || lower.contains("expired or invalid")
    }

    /// Build the "other providers" rows for the onboarding readiness summary.
    ///
    /// We already ran a live ping for the default model; for the remaining
    /// configured providers we trust the cached auth probe (Available -> ready,
    /// Expired -> needs attention). `skip` is the provider key backing the
    /// default model so we don't list it twice.
    fn onboarding_other_provider_rows(skip: Option<&str>) -> (Vec<String>, Vec<String>) {
        use crate::auth::AuthState;
        let status = crate::auth::AuthStatus::check_fast();
        // (display name, provider-key, state)
        let providers: [(&str, &str, AuthState); 8] = [
            ("Anthropic (Claude)", "anthropic", status.anthropic.state),
            ("OpenAI", "openai", status.openai),
            ("Jcode subscription", "jcode", status.jcode),
            ("Gemini", "gemini", status.gemini),
            ("GitHub Copilot", "copilot", status.copilot),
            ("Cursor", "cursor", status.cursor),
            ("OpenRouter", "openrouter", status.openrouter),
            ("Antigravity", "antigravity", status.antigravity),
        ];
        // Normalize the default provider's key so aliases like "claude" map to
        // the canonical "anthropic" bucket we list below.
        let skip = skip.map(|s| {
            let s = s.trim().to_ascii_lowercase();
            match s.as_str() {
                "claude" | "claude cli" => "anthropic".to_string(),
                other => other.to_string(),
            }
        });
        let mut ready = Vec::new();
        let mut attention = Vec::new();
        for (name, key, state) in providers {
            if skip.as_deref() == Some(key) {
                continue;
            }
            match state {
                AuthState::Available => ready.push(name.to_string()),
                AuthState::Expired => attention.push(format!("{name} - login expired")),
                AuthState::NotConfigured => {}
            }
        }
        (ready, attention)
    }

    /// Handle the result of the onboarding default-model validation: render one
    /// clean readiness summary listing the logins that work and the ones that
    /// need attention. Stale results (from a previous session) are ignored.
    pub(super) fn handle_onboarding_model_validated(
        &mut self,
        result: crate::bus::OnboardingModelValidated,
    ) -> bool {
        if result.session_id != self.session.id {
            return false;
        }

        let detail_text = result.detail.clone().unwrap_or_default();
        let looks_like_auth = !result.ok && Self::onboarding_detail_looks_like_auth(&detail_text);

        // Gather the other configured providers so the summary shows the full
        // picture, not just the default model. We skip the default model's own
        // provider since it gets the live-ping line below.
        let (mut ready, mut attention) =
            Self::onboarding_other_provider_rows(result.provider_key.as_deref());

        // Place the freshly-pinged default model at the top of whichever list it
        // belongs in, so it always reads first.
        if result.ok {
            ready.insert(0, format!("{} (default)", result.model_label));
        } else {
            let reason = if detail_text.is_empty() {
                "could not be validated".to_string()
            } else {
                detail_text.clone()
            };
            attention.insert(0, format!("{} (default) - {reason}", result.model_label));
        }

        // Render a single tidy block with the two sections.
        let mut body = String::new();
        if !ready.is_empty() {
            body.push_str("**Ready to use**\n");
            for row in &ready {
                body.push_str(&format!("- ✓ {row}\n"));
            }
        }
        if !attention.is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str("**Needs attention**\n");
            for row in &attention {
                body.push_str(&format!("- ✕ {row}\n"));
            }
            let fix_hint = if looks_like_auth || !ready.is_empty() {
                "Run /login to fix a login, or /model to pick another."
            } else {
                "Run /login to add a login, or /model to pick another."
            };
            body.push_str(&format!("\n{fix_hint}"));
        }
        if body.is_empty() {
            // Defensive: should not happen because the default model always
            // lands in one of the lists.
            body.push_str("Type anything to start.");
        }
        self.push_display_message(DisplayMessage::system(body.trim_end().to_string()));

        // Status-bar notice: concise, action-oriented.
        if attention.is_empty() {
            self.set_status_notice(format!(
                "{} ready - type anything to start",
                result.model_label
            ));
        } else if result.ok {
            self.set_status_notice(format!(
                "{} ready - type anything to start ({} login{} need attention)",
                result.model_label,
                attention.len(),
                if attention.len() == 1 { "" } else { "s" }
            ));
        } else {
            let hint = if looks_like_auth {
                "/login to fix credentials, or /model"
            } else {
                "type anything to try, or /model"
            };
            self.set_status_notice(format!("{} not validated - {hint}", result.model_label));
        }
        true
    }

    /// Mark the flow complete; the normal UI takes over.
    pub(super) fn onboarding_finish(&mut self) {
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Done;
        }
    }

    /// A login/import attempt failed while onboarding was driving the Login
    /// phase. Without this, the welcome card stays up (still spinning the donut)
    /// while a red error message renders behind it, which looks broken. Reset
    /// the Login phase to the clean manual-login prompt so the user can pick a
    /// provider and try again; the pushed error message tells them what went
    /// wrong.
    pub(super) fn onboarding_handle_login_failed(&mut self) {
        let in_login_phase = matches!(
            self.onboarding_flow.as_ref().map(|f| &f.phase),
            Some(OnboardingPhase::Login { .. })
        );
        if !in_login_phase {
            return;
        }
        if let Some(flow) = self.onboarding_flow.as_mut()
            && let OnboardingPhase::Login { ref mut import } = flow.phase
        {
            *import = None;
        }
        self.set_status_notice(
            "Import failed. Press Enter to choose a provider and log in manually.",
        );
    }

    /// Drive auto-advancing phases. Call once per tick/redraw. Returns true if
    /// the flow state changed (so the caller can request a redraw).
    pub(super) fn onboarding_tick(&mut self) -> bool {
        // Fresh-install bootstrap: if we were already logged in at the CLI before
        // the TUI launched, no in-TUI login event fired, so evaluate (once)
        // whether to begin the guided flow now that the TUI is up.
        let mut changed = false;
        if !self.onboarding_startup_checked {
            self.maybe_begin_onboarding_flow_on_startup();
            // If startup just kicked the flow on, request a redraw.
            changed = self.onboarding_flow_active();
        }
        // Drive the deferred new-session model validation independently of the
        // flow phase: it may be requested right as the flow finishes (the
        // no-transcripts path calls `onboarding_finish()` before validating), so
        // gating it on `onboarding_flow_active()` would strand it forever.
        if self.onboarding_tick_model_validation() {
            changed = true;
        }
        if !self.onboarding_flow_active() {
            return changed;
        }

        // Drive the longer (60s) yes/no decision phases: the login-import
        // walkthrough and the telemetry consent prompt. On timeout we pick the
        // highlighted default; otherwise we keep the countdown notice fresh.
        let decision_timed_out = self
            .onboarding_flow
            .as_ref()
            .map(OnboardingFlow::decision_timed_out)
            .unwrap_or(false);
        match self.onboarding_phase().cloned() {
            Some(OnboardingPhase::Login {
                import: Some(_), ..
            }) => {
                if decision_timed_out {
                    // Auto-commit the currently highlighted choice and advance.
                    let mut finished = false;
                    if let Some(review) = self.onboarding_import_review_mut() {
                        finished = review.commit_current();
                    }
                    if finished {
                        self.onboarding_finish_import_review();
                    } else {
                        self.update_onboarding_import_review_status();
                    }
                    return true;
                }
                // Keep the per-candidate countdown notice fresh.
                self.update_onboarding_import_review_status();
                return true;
            }
            Some(OnboardingPhase::ContinuePrompt {
                yes_highlighted,
                cli,
                ..
            }) => {
                if decision_timed_out {
                    // Timeout default is the highlighted option (Yes by default).
                    self.onboarding_answer_continue(yes_highlighted);
                    return true;
                }
                self.update_onboarding_continue_prompt_status(cli);
                return true;
            }
            Some(OnboardingPhase::ScrollWmOptIn {
                yes_highlighted, ..
            }) => {
                // While an install is running, let the Bus completion handler
                // drive the flow; don't auto-skip out from under it.
                if self.scrollwm_install_progress.is_some() {
                    return false;
                }
                if decision_timed_out {
                    // Timeout default is "No": never install on a silent skip,
                    // regardless of which option is currently highlighted.
                    let _ = yes_highlighted;
                    self.onboarding_answer_scrollwm_optin(false);
                    return true;
                }
                self.update_onboarding_scrollwm_optin_status();
                return true;
            }
            _ => {}
        }

        // The transcript/resume picker no longer auto-selects: the user either
        // resumes a session or chooses "Start a new session" explicitly.
        false
    }
}

/// Whether ScrollWM.app is already installed. Checks the README install
/// locations: `~/Applications` (default web-install / Homebrew target) and the
/// system `/Applications`. Always false off macOS.
fn scrollwm_app_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join("Applications/ScrollWM.app"));
        }
        candidates.push(std::path::PathBuf::from("/Applications/ScrollWM.app"));
        candidates.into_iter().any(|p| p.is_dir())
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Pure decision for the ScrollWM opt-in gate, extracted for unit testing.
/// Offer the opt-in only on macOS, in a local (non-remote) session, when
/// ScrollWM is not already installed and the user hasn't answered before.
pub(crate) fn scrollwm_optin_should_offer(
    is_macos: bool,
    is_remote: bool,
    already_installed: bool,
    already_answered: bool,
) -> bool {
    is_macos && !is_remote && !already_installed && !already_answered
}

/// URL of the ScrollWM web installer (the README "recommended" path).
const SCROLLWM_WEB_INSTALL_URL: &str =
    "https://raw.githubusercontent.com/1jehuang/scrollwm/main/scripts/web-install.sh";

/// Run the ScrollWM web installer non-interactively. Downloads the installer
/// script to a temp file (so it is inspectable/loggable rather than a blind
/// `curl | bash`), then executes it with `bash`. Leaves `SCROLLWM_DEST` unset so
/// the app lands in `~/Applications`, where the Accessibility grant persists
/// (avoiding App Translocation). Returns a short error string on failure.
///
/// macOS only; on other platforms this is an immediate error (the opt-in is
/// never shown there, so this is just a safety net).
async fn run_scrollwm_install() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tokio::process::Command;
        use tokio::time::{Duration, timeout};

        let run = async {
            // 1. Download the installer script to a temp file.
            let tmp = std::env::temp_dir().join(format!(
                "scrollwm-web-install-{}.sh",
                std::process::id()
            ));
            let curl = Command::new("curl")
                .args(["-fsSL", SCROLLWM_WEB_INSTALL_URL, "-o"])
                .arg(&tmp)
                .output()
                .await
                .map_err(|e| format!("could not run curl: {e}"))?;
            if !curl.status.success() {
                let err = String::from_utf8_lossy(&curl.stderr);
                return Err(format!(
                    "download failed: {}",
                    err.trim().lines().last().unwrap_or("unknown error")
                ));
            }

            // 2. Execute the downloaded installer with bash.
            let out = Command::new("bash")
                .arg(&tmp)
                .output()
                .await
                .map_err(|e| format!("could not run installer: {e}"))?;
            let _ = tokio::fs::remove_file(&tmp).await;
            if out.status.success() {
                Ok(())
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(err
                    .trim()
                    .lines()
                    .last()
                    .unwrap_or("installer exited with an error")
                    .to_string())
            }
        };

        // Generous cap so a stuck download never wedges the flow forever.
        match timeout(Duration::from_secs(180), run).await {
            Ok(result) => result,
            Err(_) => Err("timed out after 180s".to_string()),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("ScrollWM is macOS-only".to_string())
    }
}
