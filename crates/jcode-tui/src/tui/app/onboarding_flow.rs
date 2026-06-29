//! First-run onboarding flow state machine.
//!
//! After the user logs in / imports credentials on a fresh install, we walk
//! them through a short guided flow:
//!
//!   1. `Login` - if we boot without working credentials, ask the
//!      user to log in right inside the TUI (the fresh
//!      install no longer runs a blocking CLI login).
//!      Skipped entirely when credentials already exist.
//!   2. `TranscriptPick` - if we detect external Codex / Claude Code
//!      transcripts, drop the user straight into a
//!      resume-style picker. The picker reserves a top band
//!      for an onboarding prompt and offers a selectable
//!      "Start a new session" row alongside the resumable
//!      sessions. Nothing auto-selects; the user resumes a
//!      session or starts fresh explicitly.
//!   3. `Suggestions` - the existing prompt-suggestion cards. Reached when
//!      they choose "Start a new session", when there is no
//!      external OAuth, or as the terminal resting state.
//!
//!   (`ContinuePrompt` is retained as a legacy phase for replay/test fixtures
//!   but is no longer entered by the live flow.)
//!
//! If anything fails along the continue path (no transcripts, load error,
//! resume failure) we fall back to seeding the input with a prompt that asks
//! the agent to session-search the latest Codex/Claude Code session and
//! continue from there.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long we wait on a yes/no decision phase (login import, telemetry
/// consent) before auto-selecting the highlighted default. We keep this short
/// enough that the user doesn't get stuck deliberating, but long enough to
/// read the prompt.
pub(crate) const DECISION_TIMEOUT: Duration = Duration::from_secs(60);

/// Which external CLI an OAuth login was detected for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalCli {
    Codex,
    ClaudeCode,
    Pi,
    OpenCode,
}

impl ExternalCli {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ExternalCli::Codex => "Codex",
            ExternalCli::ClaudeCode => "Claude Code",
            ExternalCli::Pi => "Pi",
            ExternalCli::OpenCode => "OpenCode",
        }
    }
}

/// Single-screen multi-select review for importing detected external logins.
///
/// On a fresh install we may detect logins left behind by other tools (Codex,
/// Claude Code, Copilot, ...). Rather than walking the user through one yes/no
/// page per login, we show them ALL at once as a checkbox list. Every login is
/// pre-checked (the safe, common default is "import everything"), the user can
/// move a cursor and toggle any row off, and a single "Import" action commits
/// all checked logins together. This collapses N pages into one screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportReview {
    /// All detected importable logins, in display order.
    pub(crate) candidates: Vec<crate::external_auth::ExternalAuthReviewCandidate>,
    /// Per-candidate checked state (parallel to `candidates`). `true` = import.
    /// All start checked so the default action imports everything.
    pub(crate) checked: Vec<bool>,
    /// Index of the row the cursor is currently on (for toggling/highlight).
    pub(crate) cursor: usize,
    /// When `true`, focus is on the "Continue" pill (rendered above and below
    /// the list) rather than on a login row. Moving down past the last row, or
    /// up past the first row, lands here; pressing Enter commits the import.
    /// This lets the user reach the commit action purely by arrowing, instead
    /// of relying on the "Press Enter" instruction text.
    pub(crate) continue_focused: bool,
    /// When the screen was first shown, for the single decision countdown.
    pub(crate) shown_at: Instant,
}

impl ImportReview {
    /// Create a review for the given candidates with every login pre-checked.
    /// Returns `None` if there are no candidates.
    pub(crate) fn new(
        candidates: Vec<crate::external_auth::ExternalAuthReviewCandidate>,
    ) -> Option<Self> {
        if candidates.is_empty() {
            return None;
        }
        let checked = vec![true; candidates.len()];
        Some(Self {
            candidates,
            checked,
            cursor: 0,
            continue_focused: false,
            shown_at: Instant::now(),
        })
    }

    /// The candidate the cursor is currently on, if any. Returns `None` while
    /// the "Continue" pill is focused.
    #[allow(dead_code)] // Accessor kept for the import-review UI; not wired to a caller yet.
    pub(crate) fn current(&self) -> Option<&crate::external_auth::ExternalAuthReviewCandidate> {
        if self.continue_focused {
            return None;
        }
        self.candidates.get(self.cursor)
    }

    /// 1-based position of the cursor row (for "1 of 3" display).
    #[allow(dead_code)] // Accessor kept for the import-review UI; not wired to a caller yet.
    pub(crate) fn position(&self) -> usize {
        self.cursor + 1
    }

    /// Total number of candidates being reviewed.
    pub(crate) fn total(&self) -> usize {
        self.candidates.len()
    }

    /// Move focus to the previous item, treating the "Continue" pill as a single
    /// element that sits both above and below the list. The cycle is:
    /// Continue -> last row -> ... -> first row -> Continue.
    pub(crate) fn cursor_up(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        if self.continue_focused {
            // From Continue, step up onto the last row.
            self.continue_focused = false;
            self.cursor = self.candidates.len() - 1;
        } else if self.cursor == 0 {
            // Above the first row sits the Continue pill.
            self.continue_focused = true;
        } else {
            self.cursor -= 1;
        }
    }

    /// Move focus to the next item. The cycle is:
    /// first row -> ... -> last row -> Continue -> first row.
    pub(crate) fn cursor_down(&mut self) {
        if self.candidates.is_empty() {
            return;
        }
        if self.continue_focused {
            // From Continue, step down onto the first row.
            self.continue_focused = false;
            self.cursor = 0;
        } else if self.cursor + 1 >= self.candidates.len() {
            // Below the last row sits the Continue pill.
            self.continue_focused = true;
        } else {
            self.cursor += 1;
        }
    }

    /// Toggle the checked state of the row under the cursor. No-op while the
    /// "Continue" pill is focused.
    pub(crate) fn toggle_current(&mut self) {
        if self.continue_focused {
            return;
        }
        if let Some(slot) = self.checked.get_mut(self.cursor) {
            *slot = !*slot;
        }
    }

    /// Set the checked state of the row under the cursor. No-op while the
    /// "Continue" pill is focused.
    pub(crate) fn set_current(&mut self, checked: bool) {
        if self.continue_focused {
            return;
        }
        if let Some(slot) = self.checked.get_mut(self.cursor) {
            *slot = checked;
        }
    }

    /// Whether the row under the cursor is currently checked. False while the
    /// "Continue" pill is focused.
    #[allow(dead_code)] // Accessor kept for the import-review UI; not wired to a caller yet.
    pub(crate) fn current_checked(&self) -> bool {
        if self.continue_focused {
            return false;
        }
        self.checked.get(self.cursor).copied().unwrap_or(false)
    }

    /// The zero-based indices of all checked (to-be-imported) candidates.
    pub(crate) fn approved_indices(&self) -> Vec<usize> {
        self.checked
            .iter()
            .enumerate()
            .filter_map(|(i, &c)| c.then_some(i))
            .collect()
    }

    /// How many logins are currently checked for import.
    pub(crate) fn checked_count(&self) -> usize {
        self.checked.iter().filter(|&&c| c).count()
    }

    /// Seconds left before the screen auto-commits its default (import all
    /// currently-checked logins).
    pub(crate) fn seconds_remaining(&self) -> u64 {
        DECISION_TIMEOUT
            .saturating_sub(self.shown_at.elapsed())
            .as_secs()
    }

    /// Whether the decision countdown has elapsed.
    pub(crate) fn timed_out(&self) -> bool {
        self.shown_at.elapsed() >= DECISION_TIMEOUT
    }
}

/// The current phase of the onboarding flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OnboardingPhase {
    /// Log in. Entered on a fresh install when no working credentials exist.
    /// The TUI now owns the entire first-run login experience instead of the
    /// old blocking CLI provider prompt.
    ///
    /// When we detect importable external logins, `import` holds a per-candidate
    /// yes/no walkthrough so the user can step through and choose what to import.
    /// When `None`, there was nothing to import and we prompt the user to pick a
    /// provider manually (Enter opens the login picker).
    Login { import: Option<ImportReview> },
    /// Ask the user whether to log in to OpenAI. Shown on a fresh install when
    /// no importable external logins were detected. A highlightable Yes/No
    /// selector (default "Yes") matching the import walkthrough: Yes starts the
    /// OpenAI sign-in, No exits onboarding to the normal new-session screen with
    /// a system message telling the user to run `/login` when ready (we avoid the
    /// inline provider picker here). Unlike the import/telemetry prompts this one
    /// has no auto-timeout: logging in is a meaningful first step, so we wait for
    /// the user rather than opening a browser on a countdown.
    LoginOpenAi {
        /// Which option is highlighted (true = "Yes, log in to OpenAI").
        yes_highlighted: bool,
    },
    /// Legacy phase kept for compatibility with older replay/test fixtures.
    /// New onboarding skips explicit model selection and uses the default route;
    /// users can still run `/model` later.
    ModelSelect,
    /// "Continue where you left off in <cli>?" Yes/No with a
    /// [`DECISION_TIMEOUT`] countdown. Highlightable Yes/No selector to match
    /// the import prompt; the default (and timeout choice) is "Yes" so the
    /// resume menu opens unless the user declines.
    ContinuePrompt {
        cli: ExternalCli,
        /// Which option is highlighted (true = "Yes, continue").
        yes_highlighted: bool,
        /// When the prompt was shown, for the countdown.
        shown_at: Instant,
    },
    /// Single-select transcript picker with a 10s auto-select of the latest.
    TranscriptPick { cli: ExternalCli, shown_at: Instant },
    /// Existing prompt-suggestion cards (resting / "No" state).
    Suggestions,
    /// Flow finished; nothing onboarding-specific to render.
    Done,
}

/// A first-run new-session model-validation request that is waiting for a
/// concrete default-model id to be known before it fires. In remote/client
/// mode the live model is reported by the server asynchronously, so the
/// onboarding tick polls until a real id (not "unknown") is available, then
/// runs the lightweight validation ping.
///
/// When the validation is requested right after a login (remote mode), the
/// server also pushes a fresh model catalog a moment later (e.g. switching the
/// route to gpt-5.5 after an OpenAI login). We capture the catalog "generation"
/// at request time and wait for it to advance so the readiness line reports the
/// freshly-selected model rather than the stale pre-login default.
#[derive(Clone, Debug)]
pub(crate) struct OnboardingPendingValidation {
    /// Session the validation belongs to; stale requests are ignored.
    pub(crate) session_id: String,
    /// When the request was created, so we can give up after a short wait
    /// (and validate whatever default we have) rather than spinning forever.
    pub(crate) requested_at: Instant,
    /// Whether to wait for the server's post-login catalog refresh to land
    /// before firing (remote mode after a login).
    pub(crate) await_catalog_refresh: bool,
    /// Remote catalog generation observed when the request was created. The
    /// post-login refresh has landed once the live generation moves past this.
    pub(crate) catalog_generation_at_request: u64,
}

impl OnboardingPendingValidation {
    /// How long we will wait for the server to report a concrete model id
    /// before validating with the best default we currently have.
    const RESOLVE_TIMEOUT: Duration = Duration::from_secs(8);

    pub(crate) fn new(session_id: String) -> Self {
        Self {
            session_id,
            requested_at: Instant::now(),
            await_catalog_refresh: false,
            catalog_generation_at_request: 0,
        }
    }

    /// Variant that also waits for the remote catalog generation to advance
    /// past `catalog_generation` (the post-login refresh) before firing.
    pub(crate) fn awaiting_catalog_refresh(session_id: String, catalog_generation: u64) -> Self {
        Self {
            session_id,
            requested_at: Instant::now(),
            await_catalog_refresh: true,
            catalog_generation_at_request: catalog_generation,
        }
    }

    /// Whether we have waited long enough that we should validate now even if
    /// the model id has not been reported yet.
    pub(crate) fn resolve_timed_out(&self) -> bool {
        self.requested_at.elapsed() >= Self::RESOLVE_TIMEOUT
    }
}

/// Runtime state for the onboarding flow. `None`/`Done` means inactive.
#[derive(Clone, Debug)]
pub(crate) struct OnboardingFlow {
    pub(crate) phase: OnboardingPhase,
}

impl OnboardingFlow {
    /// Start the post-login flow. The app immediately advances this legacy
    /// phase to continue/suggestions so first-run onboarding no longer blocks on
    /// choosing a model.
    pub(crate) fn begin() -> Self {
        Self {
            phase: OnboardingPhase::ModelSelect,
        }
    }

    /// Start the flow at the login phase (no working credentials yet).
    /// `import` is the per-candidate import walkthrough when external logins
    /// were detected. When no logins were detected (`import` is `None`) we ask a
    /// simple "Log in to OpenAI?" Yes/No instead of dropping straight to the
    /// provider picker.
    pub(crate) fn begin_at_login(import: Option<ImportReview>) -> Self {
        let phase = match import {
            Some(review) => OnboardingPhase::Login {
                import: Some(review),
            },
            None => OnboardingPhase::LoginOpenAi {
                yes_highlighted: true,
            },
        };
        Self { phase }
    }

    /// Whether the flow is actively driving the UI.
    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.phase, OnboardingPhase::Done)
    }

    /// Seconds remaining on the longer [`DECISION_TIMEOUT`] yes/no phases
    /// (login import walkthrough, continue prompt), if one is active.
    pub(crate) fn decision_seconds_remaining(&self) -> Option<u64> {
        match &self.phase {
            OnboardingPhase::Login {
                import: Some(review),
            } => Some(review.seconds_remaining()),
            OnboardingPhase::ContinuePrompt { shown_at, .. } => Some(
                DECISION_TIMEOUT
                    .saturating_sub(shown_at.elapsed())
                    .as_secs(),
            ),
            _ => None,
        }
    }

    /// Whether a [`DECISION_TIMEOUT`] yes/no phase has elapsed and should
    /// auto-select its default.
    pub(crate) fn decision_timed_out(&self) -> bool {
        match &self.phase {
            OnboardingPhase::Login {
                import: Some(review),
            } => review.timed_out(),
            OnboardingPhase::ContinuePrompt { shown_at, .. } => {
                shown_at.elapsed() >= DECISION_TIMEOUT
            }
            _ => false,
        }
    }
}

/// Detect whether an external Codex, Claude Code, Pi, or OpenCode OAuth login
/// is present.
///
/// Returns every detected CLI (sandbox-aware), so the caller can choose which
/// one to offer (e.g. by most-recent activity). The order is Codex, Claude, Pi,
/// then OpenCode, but callers should not treat that as a preference.
pub(crate) fn detect_external_cli_oauths() -> Vec<ExternalCli> {
    let mut found = Vec::new();
    if external_oauth_present(&external_home_path(".codex/auth.json")) {
        found.push(ExternalCli::Codex);
    }
    if external_oauth_present(&external_home_path(".claude/.credentials.json")) {
        found.push(ExternalCli::ClaudeCode);
    }
    if external_oauth_present(&external_home_path(".pi/agent/auth.json")) {
        found.push(ExternalCli::Pi);
    }
    if external_oauth_present(&external_home_path(".local/share/opencode/auth.json")) {
        found.push(ExternalCli::OpenCode);
    }
    found
}

/// Resolve a path under the (sandbox-aware) external home so onboarding honors
/// `JCODE_HOME`/external isolation, matching the import detectors.
fn external_home_path(rel: &str) -> PathBuf {
    crate::storage::user_home_path(rel)
        .ok()
        .or_else(|| home_dir().map(|home| home.join(rel)))
        .unwrap_or_else(|| PathBuf::from(rel))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// A credentials file counts as an OAuth login when it exists and is non-empty.
fn external_oauth_present(path: &PathBuf) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() > 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_starts_at_model_select_and_is_active() {
        let flow = OnboardingFlow::begin();
        assert_eq!(flow.phase, OnboardingPhase::ModelSelect);
        assert!(flow.is_active());
    }

    #[test]
    fn done_phase_is_inactive() {
        let flow = OnboardingFlow {
            phase: OnboardingPhase::Done,
        };
        assert!(!flow.is_active());
    }

    #[test]
    fn continue_prompt_counts_down_and_times_out() {
        let past = Instant::now() - (DECISION_TIMEOUT + Duration::from_secs(1));
        let flow = OnboardingFlow {
            phase: OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: past,
            },
        };
        // The continue prompt now shares the longer DECISION_TIMEOUT with the
        // import and telemetry prompts (not the short AUTO_ADVANCE).
        assert_eq!(flow.decision_seconds_remaining(), Some(0));
        assert!(flow.decision_timed_out());
    }

    #[test]
    fn fresh_continue_prompt_has_remaining_time() {
        let flow = OnboardingFlow {
            phase: OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::ClaudeCode,
                yes_highlighted: true,
                shown_at: Instant::now(),
            },
        };
        let remaining = flow.decision_seconds_remaining().unwrap();
        assert!(
            remaining >= DECISION_TIMEOUT.as_secs() - 2 && remaining <= DECISION_TIMEOUT.as_secs()
        );
        assert!(!flow.decision_timed_out());
    }

    #[test]
    fn external_oauth_present_requires_nonempty_file() {
        let dir = std::env::temp_dir().join(format!("jcode-onb-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let empty = dir.join("empty.json");
        let full = dir.join("full.json");
        std::fs::write(&empty, b"").unwrap();
        std::fs::write(&full, b"{\"token\":\"x\"}").unwrap();
        assert!(!external_oauth_present(&empty));
        assert!(external_oauth_present(&full));
        assert!(!external_oauth_present(&dir.join("missing.json")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
