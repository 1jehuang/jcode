//! Control logic / phase transitions for the first-run onboarding flow.
//!
//! See [`super::onboarding_flow`] for the phase definitions. This module hangs
//! the driving methods off `App` so the rest of the TUI can advance the flow in
//! response to login, model selection, key presses, and the auto-advance timer.

use super::onboarding_flow::{ExternalCli, ImportReview, OnboardingFlow, OnboardingPhase};
use super::{App, DisplayMessage, SessionPickerMode};
use crate::tui::session_picker::{self, SessionFilterMode, SessionPicker};
use crossterm::event::KeyCode;
use std::cell::RefCell;
use std::time::Instant;

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
        if !self.onboarding_preview_mode && !self.is_new_user_for_onboarding() {
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
        if !self.is_new_user_for_onboarding() {
            self.onboarding_startup_checked = true;
            return;
        }
        // Fresh installs no longer log in at the CLI before the TUI launches.
        // If we boot without working credentials, start the flow at the in-TUI
        // `Login` phase. If credentials already exist, start at model select.
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

    /// Begin the guided flow at the model-selection phase. Called once auth
    /// becomes available on a fresh install (login/import completes).
    ///
    /// No-op if a flow is already running or the user is experienced.
    pub(super) fn begin_onboarding_flow(&mut self) {
        if self.onboarding_flow.is_some() {
            return;
        }
        self.onboarding_flow = Some(OnboardingFlow::begin());
        // The model-select prompt is rendered by the onboarding welcome screen
        // (`onboarding_welcome_kind`), not as a transcript message: in remote
        // mode the server owns the transcript and would wipe any pushed message.
        self.set_status_notice("Onboarding: press Enter to choose a model");
    }

    /// Begin the guided flow at the in-TUI `Login` phase. Used on a fresh
    /// install that booted without working credentials (the CLI no longer logs
    /// in before the TUI launches).
    ///
    /// If we detect importable external logins (Codex/Claude/Cursor/etc.), we
    /// arm a per-candidate yes/no walkthrough so the user can step through each
    /// detected login and choose whether to import it. Otherwise we prompt them
    /// to pick a provider manually.
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
            self.set_status_notice("Welcome to jcode: press Enter to log in");
        }
    }

    /// Advance out of the `Login` phase once credentials are available. We then
    /// ask the user whether to share prompt/transcript content with telemetry
    /// before moving on to model selection. No-op unless the flow is in `Login`.
    pub(super) fn onboarding_after_login(&mut self) {
        if !matches!(self.onboarding_phase(), Some(OnboardingPhase::Login { .. })) {
            return;
        }
        self.onboarding_enter_telemetry_consent();
    }

    /// Enter the telemetry content-sharing consent phase. Default highlight is
    /// "No" (privacy-safe), and the prompt auto-declines after the decision
    /// countdown so the user is never stuck on it.
    fn onboarding_enter_telemetry_consent(&mut self) {
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::TelemetryConsent {
                yes_highlighted: false,
                shown_at: Instant::now(),
            };
        }
        self.set_status_notice(
            "Share prompts & transcripts to improve jcode? No/Yes - auto-declines in 60s",
        );
    }

    /// Answer the telemetry consent prompt: persist the choice and advance to
    /// model selection.
    pub(super) fn onboarding_answer_telemetry_consent(&mut self, opt_in: bool) {
        if !matches!(
            self.onboarding_phase(),
            Some(OnboardingPhase::TelemetryConsent { .. })
        ) {
            return;
        }
        crate::telemetry::set_content_sharing_enabled(opt_in);
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ModelSelect;
        }
        let notice = if opt_in {
            "Thanks! Sharing enabled. Onboarding: run /model to pick a model"
        } else {
            "No content shared. Onboarding: run /model to pick a model"
        };
        self.set_status_notice(notice);
    }

    /// Advance out of the model-selection phase once a model has been chosen.
    /// Decides whether to offer "continue where you left off" based on detected
    /// external Codex / Claude Code OAuth logins. When both CLIs are present we
    /// offer whichever one has the most recent transcript, so the user resumes
    /// where they actually last worked rather than always defaulting to Codex.
    pub(super) fn onboarding_after_model_select(&mut self) {
        if !matches!(self.onboarding_phase(), Some(OnboardingPhase::ModelSelect)) {
            return;
        }
        match self.onboarding_most_recent_external_cli() {
            Some(cli) => self.onboarding_enter_continue_prompt(cli),
            None => self.onboarding_show_suggestions(),
        }
    }

    /// Among the external CLIs whose OAuth credentials are present, pick the one
    /// with the most recent transcript. Ties (or a CLI with no transcripts yet)
    /// fall back to detection order (Codex first). Returns `None` when no
    /// external CLI login is present.
    fn onboarding_most_recent_external_cli(&self) -> Option<ExternalCli> {
        let present = crate::tui::app::onboarding_flow::detect_external_cli_oauths();
        match present.as_slice() {
            [] => None,
            [only] => Some(*only),
            _ => {
                // Multiple logins: rank by newest transcript mtime.
                present
                    .iter()
                    .max_by_key(|cli| {
                        session_picker::latest_external_cli_session_secs(**cli).unwrap_or(0)
                    })
                    .copied()
                    .or_else(|| present.first().copied())
            }
        }
    }

    /// Enter the "Continue where you left off?" phase. Highlightable Yes/No
    /// with a [`DECISION_TIMEOUT`] countdown; the default (and timeout choice)
    /// is "Yes" so the resume menu opens unless the user declines.
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
            self.onboarding_open_transcript_picker(cli);
        } else {
            self.onboarding_show_suggestions();
        }
    }

    /// Intercept keys for the guided onboarding welcome phases:
    ///   - `ModelSelect`: we tell the user to run /model; Enter is also a
    ///     shortcut that opens the model picker from the welcome screen.
    ///   - `ContinuePrompt`: Y/Enter continues, N/Esc declines.
    ///   - `TelemetryConsent`: Left/h -> No, Right/l -> Yes, toggle with
    ///     Up/Down/k/j/Tab; y/n commit directly, Enter/Space commit the
    ///     highlighted default.
    ///     Returns true if the key was consumed.
    pub(super) fn handle_onboarding_continue_prompt_key(&mut self, code: KeyCode) -> bool {
        match self.onboarding_phase() {
            Some(OnboardingPhase::Login { import }) => {
                // No detected imports: fall back to "press Enter to choose a
                // provider". Only intercept Enter from the welcome screen; if an
                // overlay is already open let it commit.
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
            Some(OnboardingPhase::TelemetryConsent { .. }) => {
                self.handle_onboarding_telemetry_consent_key(code)
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
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    review.set_yes(false);
                    finished = review.commit_current();
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    finished = review.commit_current();
                }
                _ => return false,
            }
        }
        if finished {
            self.onboarding_finish_import_review();
        } else {
            self.update_onboarding_import_review_status();
        }
        true
    }

    /// Handle a key while the telemetry content-sharing consent prompt is up.
    /// Yes/No sit side by side (default highlight is "No"):
    ///   - Left / h  -> highlight "No"
    ///   - Right / l -> highlight "Yes"
    ///   - Up / Down / k / j / Tab -> toggle
    ///   - y / Y -> opt in;  n / N -> opt out (both commit)
    ///   - Enter / Space -> commit the highlighted choice
    fn handle_onboarding_telemetry_consent_key(&mut self, code: KeyCode) -> bool {
        let Some(flow) = self.onboarding_flow.as_mut() else {
            return false;
        };
        let OnboardingPhase::TelemetryConsent {
            yes_highlighted, ..
        } = &mut flow.phase
        else {
            return false;
        };
        match code {
            KeyCode::Left | KeyCode::Char('h') => {
                *yes_highlighted = false;
                self.update_onboarding_telemetry_consent_status();
                true
            }
            KeyCode::Right | KeyCode::Char('l') => {
                *yes_highlighted = true;
                self.update_onboarding_telemetry_consent_status();
                true
            }
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('k')
            | KeyCode::Char('j')
            | KeyCode::Tab => {
                *yes_highlighted = !*yes_highlighted;
                self.update_onboarding_telemetry_consent_status();
                true
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.onboarding_answer_telemetry_consent(true);
                true
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.onboarding_answer_telemetry_consent(false);
                true
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let opt_in = *yes_highlighted;
                self.onboarding_answer_telemetry_consent(opt_in);
                true
            }
            _ => false,
        }
    }

    /// Refresh the status notice with the telemetry consent countdown.
    fn update_onboarding_telemetry_consent_status(&mut self) {
        let remaining = self
            .onboarding_flow
            .as_ref()
            .and_then(OnboardingFlow::decision_seconds_remaining);
        if let Some(remaining) = remaining {
            self.set_status_notice(format!(
                "Share prompts & transcripts to improve jcode? No/Yes - auto-declines in {remaining}s"
            ));
        }
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
        // onboarding (Login -> ModelSelect) and activates the provider.
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
            crate::bus::Bus::global().publish(crate::bus::BusEvent::LoginCompleted(
                crate::bus::LoginCompleted {
                    provider: "auto-import".to_string(),
                    success: outcome.imported > 0,
                    message: outcome.render_markdown(),
                },
            ));
        });
    }

    /// Open a single-select resume-style picker filtered to the external CLI's
    /// transcripts. Falls back to the session-search prompt if none load.
    pub(super) fn onboarding_open_transcript_picker(&mut self, cli: ExternalCli) {
        let filter = match cli {
            ExternalCli::Codex => SessionFilterMode::Codex,
            ExternalCli::ClaudeCode => SessionFilterMode::ClaudeCode,
        };

        let (server_groups, orphan_sessions) = match session_picker::load_sessions_grouped() {
            Ok(loaded) => loaded,
            Err(err) => {
                crate::logging::error(&format!(
                    "onboarding: failed to load {} sessions: {err}",
                    cli.label()
                ));
                self.onboarding_fallback_to_session_search(cli);
                return;
            }
        };

        let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
        picker.activate_external_cli_filter(filter);

        if picker.visible_session_count() == 0 {
            self.onboarding_fallback_to_session_search(cli);
            return;
        }

        self.session_picker_overlay = Some(RefCell::new(picker));
        self.session_picker_mode = SessionPickerMode::Onboarding { cli };
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::TranscriptPick {
                cli,
                shown_at: Instant::now(),
            };
        }
        self.set_status_notice(format!(
            "Pick a {} session to continue (auto-selects latest in 10s)",
            cli.label()
        ));
    }

    /// Auto-select the most recent transcript in the onboarding picker (called
    /// on the 10s timeout). Falls back to session-search if nothing resolves.
    pub(super) fn onboarding_auto_select_latest_transcript(&mut self, cli: ExternalCli) {
        let target = self
            .session_picker_overlay
            .as_ref()
            .and_then(|cell| cell.borrow().latest_visible_resume_target());

        match target {
            Some(target) => {
                self.session_picker_overlay = None;
                self.handle_session_picker_current_terminal_selection(&[target]);
                self.onboarding_finish();
            }
            None => {
                self.session_picker_overlay = None;
                self.onboarding_fallback_to_session_search(cli);
            }
        }
    }

    /// Fallback: seed the input with a prompt asking the agent to session-search
    /// the latest external session and continue, then submit it.
    pub(super) fn onboarding_fallback_to_session_search(&mut self, cli: ExternalCli) {
        let prompt = format!(
            "Use session search to find my most recent {} session, summarize what we were \
             working on, then continue from exactly where we left off.",
            cli.label()
        );
        self.push_display_message(DisplayMessage::system(format!(
            "No {0} transcripts were found locally, so I'll search for your most recent \
             {0} session and pick up where you left off.",
            cli.label()
        )));
        self.onboarding_finish();
        // Dispatch through the queued-message path rather than `submit_input()`.
        // `submit_input()` sets the local-only `pending_turn`/`is_processing`
        // flags, which the remote run loop never consumes: the prompt would be
        // persisted as a dangling user message and the UI would spin on
        // "sending…" forever. `pending_queued_dispatch` is honored by both the
        // local and remote loops, so the turn actually starts in either mode.
        self.input.clear();
        self.cursor_pos = 0;
        self.queued_messages.push(prompt);
        self.pending_queued_dispatch = true;
    }

    /// Drop into the suggestion-card state (the "No" / no-OAuth path). Prints
    /// the same starter prompts the empty-screen welcome offers, as an inline
    /// numbered list the user can pick by typing the number or anything else.
    pub(super) fn onboarding_show_suggestions(&mut self) {
        if let Some(flow) = self.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Suggestions;
        }
        let suggestions = self.suggestion_prompts();
        if suggestions.is_empty() {
            self.onboarding_finish();
            self.set_status_notice("You're all set, type anything to start");
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
            Some(OnboardingPhase::TelemetryConsent {
                yes_highlighted, ..
            }) => {
                if decision_timed_out {
                    // Timeout default is the highlighted option (No by default).
                    self.onboarding_answer_telemetry_consent(yes_highlighted);
                    return true;
                }
                self.update_onboarding_telemetry_consent_status();
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

        let due = self
            .onboarding_flow
            .as_ref()
            .map(OnboardingFlow::auto_advance_due)
            .unwrap_or(false);
        if !due {
            // Keep the countdown visible on the timed phases.
            if let (Some(remaining), Some(OnboardingPhase::TranscriptPick { .. })) = (
                self.onboarding_flow
                    .as_ref()
                    .and_then(OnboardingFlow::auto_advance_remaining),
                self.onboarding_phase(),
            ) {
                self.set_status_notice(format!(
                    "Pick a session to continue (auto-selects latest in {remaining}s)"
                ));
                return true;
            }
            return false;
        }
        match self.onboarding_phase().cloned() {
            Some(OnboardingPhase::TranscriptPick { cli, .. }) => {
                self.onboarding_auto_select_latest_transcript(cli);
                true
            }
            _ => false,
        }
    }
}
