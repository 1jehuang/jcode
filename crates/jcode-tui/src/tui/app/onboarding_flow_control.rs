//! Control logic / phase transitions for the first-run onboarding flow.
//!
//! See [`super::onboarding_flow`] for the phase definitions. This module hangs
//! the driving methods off `App` so the rest of the TUI can advance the flow in
//! response to login, model selection, key presses, and the auto-advance timer.

use super::onboarding_flow::{
    ExternalCli, ImportReview, OnboardingFlow, OnboardingPendingValidation, OnboardingPhase,
};
use super::{App, DisplayMessage, SessionPickerMode};
use crate::import::repo_ranking::{self, SessionLocation};
use crate::tui::session_picker::{SessionPicker, load_sessions};
use crossterm::event::KeyCode;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

impl App {
    /// How long the onboarding flow waits on an in-flight login import before
    /// the tick watchdog assumes the async `LoginCompleted` event is never
    /// coming and recovers into the failure screen. Generous enough that a slow
    /// (network) credential validation completes normally, short enough that a
    /// genuinely wedged import can't trap a first-run user.
    const ONBOARDING_IMPORT_WATCHDOG: Duration = Duration::from_secs(20);

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
            let is_scaffolding = matches!(
                role,
                "system" | "usage" | "overnight" | "todos" | "background_task"
            );
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

    /// Whether this install looks like a brand-new user.
    ///
    /// Primary signal is `launch_count` in `setup_hints.json`, but that file
    /// only counts interactive `jcode` launches (TTY-gated) and can be reset
    /// or lag far behind reality. So before concluding "new user" we also look
    /// for independent evidence of an established install: a meaningful number
    /// of persisted native sessions. A user with a long session history must
    /// never be dragged through first-run onboarding just because their
    /// launch counter looks low.
    fn is_new_user_for_onboarding(&self) -> bool {
        Self::is_new_user_install()
    }

    /// Shared "does this install look brand-new?" check (see
    /// [`Self::is_new_user_for_onboarding`] for the rationale). Also used by
    /// the welcome-screen suggestion prompts.
    ///
    /// Loads via [`crate::setup_hints::SetupHintsState`] so the `.bak`
    /// fallback applies when `setup_hints.json` is missing or corrupt.
    pub(super) fn is_new_user_install() -> bool {
        let Ok(dir) = crate::storage::jcode_dir() else {
            return true;
        };
        if crate::setup_hints::SetupHintsState::load().launch_count > 5 {
            return false;
        }
        !Self::has_established_native_session_history(&dir)
    }

    /// Independent "experienced user" evidence: enough persisted native
    /// sessions on disk. Imported transcripts (`imported_*.json`) don't count;
    /// they exist on fresh installs that imported Codex/Claude history.
    fn has_established_native_session_history(jcode_dir: &std::path::Path) -> bool {
        const ESTABLISHED_SESSION_THRESHOLD: usize = 10;
        let Ok(entries) = std::fs::read_dir(jcode_dir.join("sessions")) else {
            return false;
        };
        let mut native_sessions = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with("session_") && name.ends_with(".json") {
                native_sessions += 1;
                if native_sessions >= ESTABLISHED_SESSION_THRESHOLD {
                    return true;
                }
            }
        }
        false
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
        // yes/no walkthrough rendered by the onboarding welcome screen.
        let import = match crate::external_auth::pending_external_auth_review_candidates() {
            Ok(candidates) => ImportReview::new(candidates),
            Err(err) => {
                crate::logging::error(&format!(
                    "onboarding: failed to inspect external login sources: {err}"
                ));
                None
            }
        };
        let had_imports = import.is_some();
        self.onboarding_flow = Some(OnboardingFlow::begin_at_login(import));
        // The login prompt is rendered by the onboarding welcome screen
        // (`onboarding_welcome_kind`) so it survives in remote mode.
        if had_imports {
            self.set_status_notice(
                "Welcome to jcode: review detected logins (arrows/hl to move, Enter to choose)",
            );
        } else {
            self.set_status_notice(
                "Log in to OpenAI? Yes/No - hl to move, Enter to choose (No skips for now)",
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
        // The import (if any) has resolved; leave the progress state.
        self.onboarding_import_in_progress = None;
        self.onboarding_import_error = None;
        // Prompt/transcript content sharing is opt-in and off by default; we
        // intentionally don't prompt for it during onboarding.
        crate::telemetry::set_content_sharing_enabled(false);
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ModelSelect;
        }
        self.onboarding_after_model_select();
    }

    /// Advance out of model selection into the simple first-run choice: run the
    /// suggested Git-based bug review or start with a blank new session.
    pub(super) fn onboarding_after_model_select(&mut self) {
        if !matches!(self.onboarding_phase(), Some(OnboardingPhase::ModelSelect)) {
            return;
        }
        self.onboarding_open_start_choice();
    }

    /// Enter the "Continue where you left off?" phase. Highlightable Yes/No
    /// with a [`DECISION_TIMEOUT`] countdown; the default (and timeout choice)
    /// is "Yes" so the resume menu opens unless the user declines.
    ///
    /// Retained for compatibility with replay/test fixtures and the
    /// `ContinuePrompt` rendering/key/tick paths. The live onboarding flow now
    /// opens the two-action start choice directly.
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
            let _ = cli;
            self.onboarding_open_start_choice();
        } else {
            self.onboarding_show_suggestions();
        }
    }

    /// Intercept keys for the guided onboarding welcome phases:
    /// - `ModelSelect`: we tell the user to run /model; Enter is also a
    ///   shortcut that opens the model picker from the welcome screen.
    /// - `ContinuePrompt`: Y/Enter continues, N/Esc declines.
    /// - `LoginOpenAi`: Left/h -> Yes, Right/l -> No, toggle with
    ///   Up/Down/k/j/Tab; y/n commit directly, Enter/Space commit the
    ///   highlighted default (Yes -> OpenAI sign-in, No -> finish onboarding).
    ///
    /// Returns true if the key was consumed.
    pub(super) fn handle_onboarding_continue_prompt_key(&mut self, code: KeyCode) -> bool {
        // While a provider login is awaiting typed input (an API key, env var
        // value, endpoint, etc.) the onboarding flow is still parked in a
        // `Login`/`LoginOpenAi` phase, but the user is now typing into the
        // pending-login prompt rather than driving the welcome-screen Yes/No.
        // If we kept intercepting keys here, Enter would re-open the provider
        // picker (the reported "pick provider -> enter key -> asks again" loop)
        // and characters like h/l/j/k/y/n would be eaten as navigation. Let the
        // normal input path handle everything until the pending entry resolves.
        if self.pending_login.is_some() || self.pending_account_input.is_some() {
            return false;
        }
        // Universal escape hatch. From any guided pre-ready phase, Esc always
        // leaves onboarding to the normal new-session screen. This is the last
        // line of the liveness guarantee: no matter what state the flow is in
        // (including async-wait states), one key the user always has gets them
        // out. We only handle it on the welcome card itself; when an inline
        // overlay (picker / sign-in) is open we let Esc close that first.
        if code == KeyCode::Esc
            && self.inline_interactive_state.is_none()
            && self.session_picker_overlay.is_none()
            && self.login_picker_overlay.is_none()
            && self.account_picker_overlay.is_none()
            && matches!(
                self.onboarding_phase(),
                Some(
                    OnboardingPhase::Login { .. }
                        | OnboardingPhase::LoginOpenAi { .. }
                        | OnboardingPhase::ModelSelect
                        | OnboardingPhase::ContinuePrompt { .. }
                )
            )
        {
            self.onboarding_import_in_progress = None;
            self.onboarding_import_error = None;
            self.onboarding_finish();
            let login = Self::onboarding_login_suggestion();
            self.set_status_notice(format!(
                "Onboarding skipped - run {login} when you're ready"
            ));
            return true;
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
                            // Clear the failure notice now that the user is acting
                            // on it, so a later retry starts from a clean screen.
                            self.onboarding_import_error = None;
                            self.show_interactive_login();
                            true
                        }
                        // On the failure screen, H hands the fix to a coding
                        // agent the user recently used (prepares a repair brief).
                        KeyCode::Char('h') | KeyCode::Char('H')
                            if self.onboarding_import_error.is_some() =>
                        {
                            self.onboarding_prepare_agent_repair_brief();
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

    /// Handle a key while the single-screen import list is active.
    /// Returns true if the key was consumed.
    ///
    /// The screen has two modes:
    ///   * Summary (default): a read-only list of everything we detected, with
    ///     two pills below it. "Continue" (preselected) imports everything;
    ///     "Choose what to import" switches to the checkbox list. Arrows/Tab
    ///     move between the pills, Enter/Space commit the focused pill.
    ///   * Choose (checkbox list): per-login Yes/No rows.
    ///     - Up / Down / k / j -> move the cursor between logins
    ///     - Left / h / y -> choose "Yes" (import) for the highlighted login
    ///     - Right / l / n -> choose "No" (skip) for the highlighted login
    ///     - Space -> toggle the highlighted login between Yes and No
    ///     - Enter -> import every "Yes" login and finish
    fn handle_onboarding_import_review_key(&mut self, code: KeyCode) -> bool {
        // `finished` means the user committed the import (so we kick it off
        // outside the borrow).
        let mut finished = false;
        {
            let Some(review) = self.onboarding_import_review_mut() else {
                return false;
            };
            if !review.choosing {
                // Summary mode: two pills, "Continue" (preselected) and
                // "Choose what to import". Any directional key moves between
                // them; Enter/Space commit the focused one.
                match code {
                    KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Tab
                    | KeyCode::BackTab
                    | KeyCode::Char('k')
                    | KeyCode::Char('j')
                    | KeyCode::Char('h')
                    | KeyCode::Char('l') => {
                        review.continue_focused = !review.continue_focused;
                    }
                    // c is a direct shortcut into choose mode.
                    KeyCode::Char('c') | KeyCode::Char('C') => review.enter_choose_mode(),
                    // y = "yes, import everything" regardless of pill focus, so
                    // the timeout is never a forced wait.
                    KeyCode::Char('y') | KeyCode::Char('Y') => finished = true,
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if review.continue_focused {
                            finished = true;
                        } else {
                            review.enter_choose_mode();
                        }
                    }
                    _ => return false,
                }
            } else {
                match code {
                    KeyCode::Up | KeyCode::Char('k') => review.cursor_up(),
                    KeyCode::Down | KeyCode::Char('j') => review.cursor_down(),
                    // Left = Yes (import), Right = No (skip), for the highlighted row.
                    KeyCode::Left
                    | KeyCode::Char('h')
                    | KeyCode::Char('y')
                    | KeyCode::Char('Y') => review.set_current(true),
                    KeyCode::Right
                    | KeyCode::Char('l')
                    | KeyCode::Char('n')
                    | KeyCode::Char('N') => review.set_current(false),
                    // Space toggles the highlighted row between Yes and No.
                    KeyCode::Char(' ') => review.toggle_current(),
                    // Enter commits the whole list (import all chosen logins).
                    KeyCode::Enter => finished = true,
                    _ => return false,
                }
            }
        }
        if finished {
            self.onboarding_finish_import_review();
        } else {
            self.update_onboarding_import_review_status();
        }
        true
    }

    /// Handle a key while the "Log in to OpenAI?" prompt is up. Yes/No sit side
    /// by side (default highlight is "Yes"):
    ///   - Left / h  -> highlight "Yes"
    ///   - Right / l -> highlight "No"
    ///   - Up / Down / k / j / Tab -> toggle
    ///   - y / Y -> log in to OpenAI;  n / N -> skip and finish onboarding
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
    /// No exits onboarding and drops the user on the normal new-session screen
    /// with a system message telling them to run `/login` when they're ready.
    ///
    /// We intentionally do NOT open the inline provider picker on "No": that
    /// flow has a flaky input widget (typed characters were not echoed), and
    /// finishing onboarding straight to the usual screen is simpler and more
    /// robust. The user can pick any provider via `/login` at their own pace.
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
            self.onboarding_finish();
            let login = Self::onboarding_login_suggestion();
            let hint = if login == "/login" {
                "No problem. When you're ready to log in, run /login to pick a provider \
                 (OpenAI, Anthropic, Gemini, OpenRouter, and more)."
                    .to_string()
            } else {
                format!(
                    "No problem. When you're ready, run {login} to reuse the credentials \
                     already on this machine, or /login to pick another provider."
                )
            };
            self.push_display_message(DisplayMessage::system(hint));
            self.set_status_notice(format!("Run {login} when you're ready"));
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
            "Log in to OpenAI? [{choice}] - hl to move, Enter to choose (No skips for now)"
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

    /// Refresh the status notice to reflect the current import-list selection.
    fn update_onboarding_import_review_status(&mut self) {
        if let Some(review) = self.onboarding_import_review_mut() {
            let checked = review.checked_count();
            let total = review.total();
            let secs = review.seconds_remaining();
            let notice = if !review.choosing {
                format!(
                    "Found {total} login{} - Enter imports all (auto in {secs}s), or pick \"Choose what to import\"",
                    if total == 1 { "" } else { "s" },
                )
            } else {
                format!(
                    "Import {checked} of {total} login{} - Space toggles, arrows move, Enter imports (auto in {secs}s)",
                    if total == 1 { "" } else { "s" },
                )
            };
            self.set_status_notice(notice);
        }
    }

    /// The walkthrough is complete: run the import for the approved candidates
    /// (if any), then either advance the flow or wait for the import result.
    fn onboarding_finish_import_review(&mut self) {
        // Take the candidates and approved indices out of the phase, then clear
        // the import sub-state so the welcome card stops rendering the prompt.
        let (candidates, approved) = match self.onboarding_import_review_mut() {
            Some(review) => (review.candidates.clone(), review.approved_indices()),
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
            self.onboarding_import_in_progress = None;
            self.set_status_notice("No logins imported. Press Enter to choose a provider.");
            return;
        }

        // Mark the import as in-flight so the welcome card shows an "Importing
        // your logins..." progress state (not the manual-login recovery copy)
        // until the async LoginCompleted event advances or fails the flow.
        self.onboarding_import_in_progress = Some(Instant::now());
        // Remember the first approved login's provider so a later failure can
        // target the agent repair brief at the right `jcode auth-test --provider`.
        self.onboarding_import_failed_provider = approved
            .first()
            .and_then(|&i| candidates.get(i))
            .and_then(|c| {
                c.telemetry_auth_labels()
                    .first()
                    .map(|(p, _)| p.to_string())
            });
        // Kick off the import on the runtime; the LoginCompleted event advances
        // onboarding and activates the provider.
        self.set_status_notice("Login: importing selected logins...");
        // Grab the runtime handle explicitly: this path is reachable straight
        // from a key handler (Enter/y on the summary screen), and a bare
        // `tokio::spawn` would panic if no runtime is running (tests, exotic
        // embeddings). Failing the import gracefully keeps the liveness
        // guarantee: the user lands on the recovery screen, not a crash.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.onboarding_handle_login_failed(Some(
                "The import could not start (no async runtime).".to_string(),
            ));
            return;
        };
        runtime.spawn(async move {
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
            // Preserve which runtime should become the first-run default. The old
            // synthetic `auto-import` provider discarded this information, so the
            // auth-refresh path kept the stale pre-import provider and validation
            // immediately failed. Prefer the exact OAuth/API-key route reported by
            // AuthStatus, then fall back to the deterministic imported-label order.
            let imported_provider = crate::auth::lifecycle::preferred_frontier_auth_provider(
                &crate::auth::AuthStatus::check_fast(),
            )
            .or_else(|| outcome.preferred_activation_provider())
            .unwrap_or("auto-import")
            .to_string();
            crate::bus::Bus::global().publish(crate::bus::BusEvent::LoginCompleted(
                crate::bus::LoginCompleted {
                    provider: imported_provider,
                    success: outcome.imported > 0,
                    message: outcome.render_markdown(),
                },
            ));
        });
    }

    /// Open the action-only onboarding choice. Session history remains available
    /// later through `/resume`, but first run stays focused on two clear paths.
    pub(super) fn onboarding_open_start_choice(&mut self) {
        let mut picker = SessionPicker::new(Vec::new());
        picker.activate_onboarding_banner(Self::onboarding_start_choice_banner_lines());
        self.session_picker_overlay = Some(RefCell::new(picker));
        self.session_picker_mode = SessionPickerMode::Onboarding;
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::StartChoice {
                shown_at: Instant::now(),
            };
        }
        self.set_status_notice("Choose a suggested review or start a new session (↑↓, Enter)");
    }

    /// Formatted copy shown above the two first-run actions.
    fn onboarding_start_choice_banner_lines() -> Vec<ratatui::text::Line<'static>> {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};
        let accent = crate::tui::color_support::rgb(186, 139, 255);
        vec![
            Line::from(vec![Span::styled(
                "Welcome to jcode 🎉",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![Span::styled(
                "How would you like to begin?",
                Style::default().fg(Color::White),
            )]),
        ]
    }

    /// Resolve the project to review before the agent turn starts. The active
    /// session directory wins when it is already inside a Git repository;
    /// otherwise recent native and external session metadata supplies the newest
    /// known repository. This keeps repository discovery out of the model prompt.
    pub(super) fn onboarding_recent_project_path(&self) -> Option<PathBuf> {
        let home = dirs::home_dir();
        let excluded: Vec<PathBuf> = home.iter().cloned().collect();

        if let Some(working_dir) = self.session.working_dir.as_deref() {
            let working_dir = Path::new(working_dir);
            if self.is_remote {
                // The path belongs to the remote runtime and cannot generally be
                // statted by the attached client. Trust the server-provided
                // session directory, but never turn a home-directory launch into
                // a broad home review.
                if !working_dir.as_os_str().is_empty()
                    && home.as_deref().is_none_or(|home| home != working_dir)
                {
                    return Some(working_dir.to_path_buf());
                }
            } else if let Some(root) = repo_ranking::resolve_git_root(working_dir)
                && !excluded.iter().any(|excluded| excluded == &root)
            {
                return Some(root);
            }
        }

        if self.is_remote {
            return None;
        }

        let sessions = load_sessions().unwrap_or_default();
        let locations: Vec<SessionLocation> = sessions
            .into_iter()
            .filter(|session| {
                session.id != self.session.id && !session.is_debug && !session.is_canary
            })
            .filter_map(|session| {
                let working_dir = session.working_dir?;
                Some(SessionLocation::new(
                    working_dir,
                    session.last_active_at.or(Some(session.last_message_time)),
                ))
            })
            .collect();
        repo_ranking::most_recent_repository(&locations, &excluded)
    }

    /// First-turn prompt launched by the onboarding recent-project review action.
    /// Repository discovery is deliberately absent: the path has already been
    /// selected programmatically before this prompt is built.
    pub(super) fn onboarding_recent_project_review_prompt(repository: &Path) -> String {
        let repository = format!("{:?}", repository.to_string_lossy());
        format!(
            "Find the most critical architecture problems in the repository at {repository}. Do not fix them yet, and ask me whether I want them fixed once you find them."
        )
    }

    pub(super) fn onboarding_prepare_recent_project_review(&mut self) -> bool {
        let Some(repository) = self.onboarding_recent_project_path() else {
            self.onboarding_show_suggestions();
            self.set_status_notice(
                "No recent Git repository found. Start jcode inside a project to review it.",
            );
            return false;
        };
        self.onboarding_finish();
        self.input = Self::onboarding_recent_project_review_prompt(&repository);
        self.cursor_pos = self.input.len();
        true
    }

    /// Start the proactive recent-project review on the active runtime.
    ///
    /// Local TUIs consume `pending_turn` in their run loop, while remote-attached
    /// TUIs send queued messages from the remote tick loop. Calling
    /// [`App::submit_input`] in remote mode leaves the client permanently parked
    /// in `Sending` because no local run loop exists to consume that flag.
    pub(super) fn onboarding_start_recent_project_review(&mut self) {
        if !self.onboarding_prepare_recent_project_review() {
            return;
        }
        self.follow_chat_bottom_for_typing();
        if self.is_remote {
            super::input::queue_message(self);
            self.set_status_notice("Architecture review queued");
        } else {
            self.submit_input();
        }
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
        if self.is_remote
            && let Some(model) = self.effective_remote_provider_model()
        {
            return model;
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
        // After a login/import, provider activation and strongest-model selection
        // finish asynchronously in both local and remote mode. The pre-login
        // default is often already a concrete id, so validating immediately would
        // ping the stale provider and make Continue look broken. Defer until the
        // auth catalog refresh completes (or a short timeout).
        if self.recent_authenticated_provider.is_some() && self.auth_catalog_refresh_pending {
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
    /// When waiting for the post-login catalog refresh, hold until the remote
    /// catalog generation advances or the local auth-refresh flag clears, so we
    /// validate the freshly-selected model rather than the stale pre-login
    /// default. The resolve timeout is always a backstop.
    pub(super) fn onboarding_pending_validation_ready_to_fire(&self) -> bool {
        let Some(pending) = self.onboarding_pending_model_validation.as_ref() else {
            return false;
        };
        let timed_out = pending.resolve_timed_out();
        if pending.await_catalog_refresh {
            let refreshed = if self.is_remote {
                self.remote_model_catalog_generation > pending.catalog_generation_at_request
            } else {
                !self.auth_catalog_refresh_pending
            };
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

    /// Suggest a concrete `/login <provider>` command the user can actually
    /// complete, instead of the generic `/login`. Preference order:
    /// 1. A jcode login that exists but expired (they clearly use it).
    /// 2. Credentials detected from another CLI (Codex -> openai, Claude Code
    ///    -> claude, Cursor -> cursor), since that login will succeed instantly.
    ///
    /// Falls back to plain `/login` when nothing is detected.
    pub(super) fn onboarding_login_suggestion() -> String {
        Self::onboarding_login_suggestion_provider()
            .map(|p| format!("/login {p}"))
            .unwrap_or_else(|| "/login".to_string())
    }

    fn onboarding_login_suggestion_provider() -> Option<&'static str> {
        use crate::auth::AuthState;
        let status = crate::auth::AuthStatus::check_fast();
        // An expired login is the strongest signal: the user already picked
        // this provider once, so re-login is the fastest path back to working.
        let states = [
            ("claude", status.anthropic.state),
            ("openai", status.openai),
            ("gemini", status.gemini),
            ("copilot", status.copilot),
            ("cursor", status.cursor),
            ("openrouter", status.openrouter),
        ];
        for (id, state) in states {
            if matches!(state, AuthState::Expired) {
                return Some(id);
            }
        }
        // Otherwise suggest a provider whose credentials another CLI on this
        // machine already holds, so the import/login completes without any
        // new sign-up.
        for cli in super::onboarding_flow::detect_external_cli_oauths() {
            let id = match cli {
                ExternalCli::Codex => Some("openai"),
                ExternalCli::ClaudeCode => Some("claude"),
                ExternalCli::Cursor => Some("cursor"),
                ExternalCli::Pi | ExternalCli::OpenCode => None,
            };
            if id.is_some() {
                return id;
            }
        }
        None
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
            let login = Self::onboarding_login_suggestion();
            let fix_hint = if looks_like_auth || !ready.is_empty() {
                format!("Run {login} to fix a login, or /model to pick another.")
            } else {
                format!("Run {login} to add a login, or /model to pick another.")
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
                format!(
                    "{} to fix credentials, or /model",
                    Self::onboarding_login_suggestion()
                )
            } else {
                "type anything to try, or /model".to_string()
            };
            self.set_status_notice(format!("{} not validated - {hint}", result.model_label));
        }
        true
    }

    /// Mark the flow complete; the normal UI takes over.
    pub(super) fn onboarding_finish(&mut self) {
        self.onboarding_auto_model_selection_active
            .store(false, std::sync::atomic::Ordering::Release);
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
    pub(super) fn onboarding_handle_login_failed(&mut self, reason: Option<String>) {
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
        self.onboarding_import_in_progress = None;
        // Remember a short reason so the recovery screen can explain what went
        // wrong; fall back to a generic line when no detail is available.
        self.onboarding_import_error = Some(
            reason
                .map(|r| Self::summarize_import_error(&r))
                .unwrap_or_else(|| "We couldn't import those logins.".to_string()),
        );
        self.set_status_notice(
            "Import failed. Press Enter to choose a provider and log in manually.",
        );
    }

    /// Condense a (possibly multi-line markdown) import-failure message into a
    /// single short sentence suitable for the recovery screen. Keeps the first
    /// meaningful line and trims markdown/marker noise.
    fn summarize_import_error(raw: &str) -> String {
        let cleaned = raw
            .lines()
            .map(|l| l.trim())
            .find(|l| {
                !l.is_empty() && !l.starts_with("**") && !l.eq_ignore_ascii_case("Logins imported")
            })
            .unwrap_or("We couldn't import those logins.")
            .trim_start_matches(['✕', '✓', '-', ' '])
            .trim();
        let cleaned = if cleaned.is_empty() {
            "We couldn't import those logins."
        } else {
            cleaned
        };
        // Keep it to a single, screen-friendly line, and capitalize the first
        // letter so a lowercase validator message reads as a clean sentence.
        let truncated: String = cleaned.chars().take(120).collect();
        let mut s = String::with_capacity(truncated.len() + 1);
        let mut chars = truncated.chars();
        if let Some(first) = chars.next() {
            s.extend(first.to_uppercase());
            s.push_str(chars.as_str());
        }
        if cleaned.chars().count() > 120 {
            s.push('…');
        }
        s
    }

    /// Prepare the agent-assisted repair brief for the import-failure recovery
    /// screen (triggered by `H`). We detect the coding agent the user used most
    /// recently, build a plain-text brief listing the exact non-interactive
    /// commands the agent can run (`jcode auth-test --json`, `jcode login
    /// --api-key-stdin`, `jcode provider add`), copy it to the clipboard, and
    /// surface it as a system message so the user can paste it into that agent.
    fn onboarding_prepare_agent_repair_brief(&mut self) {
        use crate::tui::app::onboarding_repair;
        let Some(failure) = self.onboarding_import_error.clone() else {
            return;
        };
        let agent = onboarding_repair::detect_preferred_repair_agent();
        let provider = self.onboarding_import_failed_provider.as_deref();
        let mut brief = onboarding_repair::build_repair_brief(agent, &failure, provider);
        // Persist the brief to a stable path so a helper agent launched in this
        // directory can read it directly (cat it) instead of relying on a paste.
        if let Some(path) = onboarding_repair::persist_repair_brief(&brief) {
            brief.push_str(&format!("\nThis brief is saved at: {}\n", path.display()));
        }
        let copied = super::helpers::copy_to_clipboard(&brief);
        // Show the brief in the transcript so it is visible even if the
        // clipboard is unavailable (SSH, no display server).
        self.push_display_message(DisplayMessage::system(format!(
            "Agent repair brief (paste into your coding agent):\n\n{brief}"
        )));
        self.set_status_notice(onboarding_repair::repair_brief_status(agent, copied));
    }

    /// Drive auto-advancing phases. Call once per tick/redraw. Returns true if
    /// the flow state changed (so the caller can request a redraw).
    pub(super) fn onboarding_tick(&mut self) -> bool {
        // The onboarding simulator drives phases manually; never auto-advance
        // while it is walking screens.
        if self.onboarding_sim_active() {
            return false;
        }
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

        // Watchdog: the login-import runs asynchronously and advances the flow
        // only when its `LoginCompleted` event arrives. If that event never
        // lands (a wedged runtime, a dropped bus event, a sandbox with no
        // runtime), the user would otherwise be stranded forever on the
        // "Importing your logins..." screen. After a hard timeout we recover the
        // flow into the failure-aware recovery screen so the user always has a
        // way forward (Enter -> provider picker).
        if let Some(started_at) = self.onboarding_import_in_progress {
            if started_at.elapsed() >= Self::ONBOARDING_IMPORT_WATCHDOG
                && matches!(
                    self.onboarding_phase(),
                    Some(OnboardingPhase::Login { import: None })
                )
            {
                self.onboarding_handle_login_failed(Some(
                    "The import is taking longer than expected.".to_string(),
                ));
                return true;
            }
            // Keep redrawing so the progress screen animates while we wait.
            changed = true;
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
                    // Timeout default: import every currently-checked login.
                    self.onboarding_finish_import_review();
                    return true;
                }
                // Keep the countdown notice fresh.
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
            _ => {}
        }

        // The action picker waits for an explicit choice. Return `changed` so the
        // import-progress watchdog keeps the screen repainting while it waits.
        changed
    }
}
