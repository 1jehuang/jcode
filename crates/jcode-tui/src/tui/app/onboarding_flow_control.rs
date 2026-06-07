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
    /// the next onboarding step.
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
    ///   - `TelemetryConsent`: Left/h -> No, Right/l -> Yes, toggle with
    ///     Up/Down/k/j/Tab; y/n commit directly, Enter/Space commit the
    ///     highlighted default.
    /// Returns true if the key was consumed.
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
        if self.onboarding_default_model_id_is_concrete() || pending.resolve_timed_out() {
            self.onboarding_pending_model_validation = None;
            self.onboarding_spawn_model_validation();
            return true;
        }
        false
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

        // The transcript/resume picker no longer auto-selects: the user either
        // resumes a session or chooses "Start a new session" explicitly.
        false
    }
}
