//! First-run onboarding flow state machine.
//!
//! After the user logs in / imports credentials on a fresh install, we walk
//! them through a short guided flow:
//!
//!   1. `Login` - if we boot without working credentials, ask the user to
//!      log in right inside the TUI (the fresh install no longer runs a
//!      blocking CLI login). Skipped entirely when credentials already exist.
//!   2. `ModelSelect` - let them pick a model.
//!   3. `ContinuePrompt` - if we detect an external Codex or Claude Code OAuth
//!      login, ask whether they want to continue where they last left off.
//!      Auto-selects "Yes" after [`AUTO_ADVANCE`] if they don't choose.
//!   4. `TranscriptPick` - render their recent external transcripts in a
//!      resume-style picker (single select). Auto-selects the most recent
//!      after [`AUTO_ADVANCE`].
//!   5. `Suggestions` - the existing prompt-suggestion cards. Reached when
//!      they answer "No", when there is no external OAuth, or as the terminal
//!      resting state.
//!
//! If anything fails along the continue path (no transcripts, load error,
//! resume failure) we fall back to seeding the input with a prompt that asks
//! the agent to session-search the latest Codex/Claude Code session and
//! continue from there.

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long we wait on an auto-advancing phase before choosing the default.
pub(crate) const AUTO_ADVANCE: Duration = Duration::from_secs(10);

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
}

impl ExternalCli {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ExternalCli::Codex => "Codex",
            ExternalCli::ClaudeCode => "Claude Code",
        }
    }
}

/// Per-candidate yes/no walkthrough for importing detected external logins.
///
/// On a fresh install we may detect logins left behind by other tools (Codex,
/// Claude Code, Copilot, ...). Instead of a single "type 1,3" prompt, we walk
/// the user through each detected login one at a time and ask whether to import
/// it. The highlighted Yes/No option moves with the arrow / vim keys and is
/// committed with Enter or Space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportReview {
    /// All detected importable logins, in display order.
    pub(crate) candidates: Vec<crate::external_auth::ExternalAuthReviewCandidate>,
    /// Index of the candidate currently being reviewed.
    pub(crate) index: usize,
    /// Which option (Yes/No) is highlighted for the current candidate.
    pub(crate) yes_highlighted: bool,
    /// Zero-based indices of candidates the user chose to import so far.
    pub(crate) approved: Vec<usize>,
    /// When the current candidate was first shown, for the decision countdown.
    pub(crate) shown_at: Instant,
}

impl ImportReview {
    /// Create a review for the given candidates, starting on the first with
    /// "Yes" highlighted by default. Returns `None` if there are no candidates.
    pub(crate) fn new(
        candidates: Vec<crate::external_auth::ExternalAuthReviewCandidate>,
    ) -> Option<Self> {
        if candidates.is_empty() {
            return None;
        }
        Some(Self {
            candidates,
            index: 0,
            yes_highlighted: true,
            approved: Vec::new(),
            shown_at: Instant::now(),
        })
    }

    /// The candidate currently under review, if any.
    pub(crate) fn current(&self) -> Option<&crate::external_auth::ExternalAuthReviewCandidate> {
        self.candidates.get(self.index)
    }

    /// 1-based position of the current candidate (for "1 of 3" display).
    pub(crate) fn position(&self) -> usize {
        self.index + 1
    }

    /// Total number of candidates being reviewed.
    pub(crate) fn total(&self) -> usize {
        self.candidates.len()
    }

    /// Move the Yes/No highlight (true = highlight Yes, false = highlight No).
    pub(crate) fn set_yes(&mut self, yes: bool) {
        self.yes_highlighted = yes;
    }

    /// Toggle the Yes/No highlight (used by left/right or tab-style keys).
    pub(crate) fn toggle(&mut self) {
        self.yes_highlighted = !self.yes_highlighted;
    }

    /// Record the current decision and advance to the next candidate.
    /// Returns `true` if the walkthrough is now complete (no more candidates).
    pub(crate) fn commit_current(&mut self) -> bool {
        if self.yes_highlighted && !self.approved.contains(&self.index) {
            self.approved.push(self.index);
        }
        self.index += 1;
        self.yes_highlighted = true;
        // Restart the decision countdown for the next candidate.
        self.shown_at = Instant::now();
        self.index >= self.candidates.len()
    }

    /// Seconds left before the current candidate auto-commits its default.
    pub(crate) fn seconds_remaining(&self) -> u64 {
        DECISION_TIMEOUT
            .saturating_sub(self.shown_at.elapsed())
            .as_secs()
    }

    /// Whether the current candidate's decision countdown has elapsed.
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
    /// Ask whether to share prompt/transcript content with telemetry. Shown
    /// right after a successful login/import. Yes/No with a [`DECISION_TIMEOUT`]
    /// countdown; the default (and timeout choice) is "No" since sharing
    /// content is sensitive and opt-in.
    TelemetryConsent {
        /// Which option is highlighted (true = "Yes, share").
        yes_highlighted: bool,
        /// When the prompt was shown, for the countdown.
        shown_at: Instant,
    },
    /// Pick a model. Entered right after login/import.
    ModelSelect,
    /// "Continue where you left off in <cli>?" Yes/No with a
    /// [`DECISION_TIMEOUT`] countdown. Highlightable Yes/No selector to match
    /// the import and telemetry-consent prompts; the default (and timeout
    /// choice) is "Yes" so the resume menu opens unless the user declines.
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

/// Runtime state for the onboarding flow. `None`/`Done` means inactive.
#[derive(Clone, Debug)]
pub(crate) struct OnboardingFlow {
    pub(crate) phase: OnboardingPhase,
}

impl OnboardingFlow {
    /// Start the flow at the model-selection phase (credentials already exist).
    pub(crate) fn begin() -> Self {
        Self {
            phase: OnboardingPhase::ModelSelect,
        }
    }

    /// Start the flow at the login phase (no working credentials yet).
    /// `import` is the per-candidate import walkthrough when external logins
    /// were detected, or `None` to prompt for a manual provider login.
    pub(crate) fn begin_at_login(import: Option<ImportReview>) -> Self {
        Self {
            phase: OnboardingPhase::Login { import },
        }
    }

    /// Whether the flow is actively driving the UI.
    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.phase, OnboardingPhase::Done)
    }

    /// Seconds remaining before the current auto-advancing phase fires, if any.
    pub(crate) fn auto_advance_remaining(&self) -> Option<u64> {
        let shown_at = match &self.phase {
            OnboardingPhase::TranscriptPick { shown_at, .. } => *shown_at,
            _ => return None,
        };
        let elapsed = shown_at.elapsed();
        Some(AUTO_ADVANCE.saturating_sub(elapsed).as_secs())
    }

    /// Whether the current auto-advancing phase has timed out.
    pub(crate) fn auto_advance_due(&self) -> bool {
        let shown_at = match &self.phase {
            OnboardingPhase::TranscriptPick { shown_at, .. } => *shown_at,
            _ => return false,
        };
        shown_at.elapsed() >= AUTO_ADVANCE
    }

    /// Seconds remaining on the longer [`DECISION_TIMEOUT`] yes/no phases
    /// (login import walkthrough, telemetry consent), if one is active.
    pub(crate) fn decision_seconds_remaining(&self) -> Option<u64> {
        match &self.phase {
            OnboardingPhase::Login {
                import: Some(review),
            } => Some(review.seconds_remaining()),
            OnboardingPhase::TelemetryConsent { shown_at, .. } => Some(
                DECISION_TIMEOUT
                    .saturating_sub(shown_at.elapsed())
                    .as_secs(),
            ),
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
            OnboardingPhase::TelemetryConsent { shown_at, .. } => {
                shown_at.elapsed() >= DECISION_TIMEOUT
            }
            OnboardingPhase::ContinuePrompt { shown_at, .. } => {
                shown_at.elapsed() >= DECISION_TIMEOUT
            }
            _ => false,
        }
    }
}

/// Detect whether an external Codex or Claude Code OAuth login is present.
///
/// Returns every detected CLI (sandbox-aware), so the caller can choose which
/// one to offer (e.g. by most-recent activity). The order is Codex first,
/// Claude second, but callers should not treat that as a preference.
pub(crate) fn detect_external_cli_oauths() -> Vec<ExternalCli> {
    let mut found = Vec::new();
    if external_oauth_present(&external_home_path(".codex/auth.json")) {
        found.push(ExternalCli::Codex);
    }
    if external_oauth_present(&external_home_path(".claude/.credentials.json")) {
        found.push(ExternalCli::ClaudeCode);
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
    fn auto_advance_none_outside_timed_phases() {
        let flow = OnboardingFlow::begin();
        assert_eq!(flow.auto_advance_remaining(), None);
        assert!(!flow.auto_advance_due());
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
