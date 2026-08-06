//! First-run onboarding flow state machine.
//!
//! After the user logs in / imports credentials on a fresh install, we walk
//! them through a short guided flow:
//!
//!   1. `Login` - if we boot without working credentials, ask the
//!      user to log in right inside the TUI (the fresh
//!      install no longer runs a blocking CLI login).
//!      Skipped entirely when credentials already exist.
//!   2. `StartChoice` - show two stacked actions: run a suggested Git-based bug
//!      and architecture review, or start a blank new session.
//!   3. `Suggestions` - the existing prompt-suggestion cards. Reached when
//!      they choose "Start a new session" or as the terminal resting state.
//!
//!   (`ContinuePrompt` is retained as a legacy phase for replay/test fixtures
//!   but is no longer entered by the live flow.)
//!
//! Session history is intentionally excluded from onboarding and remains
//! available later through `/resume`.

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
    Cursor,
}

impl ExternalCli {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ExternalCli::Codex => "Codex",
            ExternalCli::ClaudeCode => "Claude Code",
            ExternalCli::Pi => "Pi",
            ExternalCli::OpenCode => "OpenCode",
            ExternalCli::Cursor => "Cursor",
        }
    }
}

/// Which telemetry level the "Telemetry settings" page has highlighted.
///
/// The page is a three-way choice rather than a yes/no so the middle ground
/// (usage stats without prompt content) is reachable without hunting through
/// config. Order matches the on-screen order, most sharing first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TelemetryLevel {
    /// Usage stats plus prompt/transcript content.
    Everything,
    /// Usage stats and crash reports only (no prompts or transcripts).
    NoContent,
    /// Nothing at all.
    Nothing,
}

impl TelemetryLevel {
    /// The on-screen order, most sharing first.
    pub(crate) const ORDER: [TelemetryLevel; 3] = [
        TelemetryLevel::Everything,
        TelemetryLevel::NoContent,
        TelemetryLevel::Nothing,
    ];

    /// The level currently persisted on this machine.
    pub(crate) fn current() -> Self {
        if !crate::telemetry::is_enabled() {
            TelemetryLevel::Nothing
        } else if crate::telemetry::content_sharing_enabled() {
            TelemetryLevel::Everything
        } else {
            TelemetryLevel::NoContent
        }
    }

    /// Persist this level (usage opt-out marker + content-sharing marker).
    pub(crate) fn persist(self) {
        match self {
            TelemetryLevel::Everything => {
                crate::telemetry::set_usage_telemetry_enabled(true);
                crate::telemetry::set_content_sharing_enabled(true);
            }
            TelemetryLevel::NoContent => {
                crate::telemetry::set_usage_telemetry_enabled(true);
                crate::telemetry::set_content_sharing_enabled(false);
            }
            TelemetryLevel::Nothing => {
                crate::telemetry::set_content_sharing_enabled(false);
                crate::telemetry::set_usage_telemetry_enabled(false);
            }
        }
    }

    /// Short status-line label for the chosen level.
    pub(crate) fn status_label(self) -> &'static str {
        match self {
            TelemetryLevel::Everything => "Telemetry: sending everything, thank you",
            TelemetryLevel::NoContent => "Telemetry: usage and crashes only",
            TelemetryLevel::Nothing => "Telemetry: off",
        }
    }
}

/// Single-screen review for importing detected external logins.
///
/// On a fresh install we may detect logins left behind by other tools (Codex,
/// Claude Code, Copilot, OpenClaw, Hermes, ...). The default screen is a
/// read-only SUMMARY: it lists everything we detected and lands the user on a
/// preselected "Continue" pill that imports all of it with one Enter. A second
/// "Choose what to import" pill switches to the per-login checkbox list
/// (`choosing = true`) for users who want to opt logins out individually.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportReview {
    /// All detected importable logins, in display order.
    pub(crate) candidates: Vec<crate::external_auth::ExternalAuthReviewCandidate>,
    /// Per-candidate checked state (parallel to `candidates`). `true` = import.
    /// All start checked so the default action imports everything.
    pub(crate) checked: Vec<bool>,
    /// Index of the row the cursor is currently on (for toggling/highlight).
    pub(crate) cursor: usize,
    /// Focus flag for the "Continue" pill.
    ///
    /// On the summary screen (`choosing == false`) this is `true` when the
    /// "Continue" pill is focused (the default landing spot) and `false` when
    /// the "Choose what to import" pill is focused.
    ///
    /// On the checkbox list (`choosing == true`) it is `true` while the
    /// "Continue" pill above/below the rows is focused rather than a login row.
    pub(crate) continue_focused: bool,
    /// `false` = the default summary screen (detected logins listed read-only,
    /// Continue preselected). `true` = the per-login checkbox list.
    pub(crate) choosing: bool,
    /// Which summary pill is focused when `choosing == false`.
    pub(crate) summary_pill: SummaryPill,
    /// `Some` while the "Telemetry settings" sub-page is open, holding the
    /// highlighted level. Committing or pressing Esc returns to the summary.
    pub(crate) telemetry: Option<TelemetryLevel>,
    /// When the screen was first shown, for the single decision countdown.
    pub(crate) shown_at: Instant,
}

/// The actions on the import summary screen, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SummaryPill {
    /// Import every detected login and move on.
    Continue,
    /// Skip importing and sign in with a Jcode subscription instead (default).
    Subscription,
    /// Open the per-login checkbox list to import fewer logins.
    ImportLess,
    /// Open the telemetry settings sub-page.
    Telemetry,
}

impl SummaryPill {
    const ORDER: [SummaryPill; 4] = [
        SummaryPill::Continue,
        SummaryPill::Subscription,
        SummaryPill::ImportLess,
        SummaryPill::Telemetry,
    ];

    fn index(self) -> usize {
        Self::ORDER.iter().position(|&p| p == self).unwrap_or(0)
    }

    fn step(self, forward: bool) -> Self {
        let len = Self::ORDER.len();
        let i = self.index();
        let next = if forward {
            (i + 1) % len
        } else {
            (i + len - 1) % len
        };
        Self::ORDER[next]
    }
}

impl ImportReview {
    /// Create a review for the given candidates with every login pre-checked,
    /// starting on the summary screen with the Jcode subscription preselected.
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
            choosing: false,
            summary_pill: SummaryPill::Subscription,
            telemetry: None,
            shown_at: Instant::now(),
        })
    }

    /// Switch from the summary screen to the per-login checkbox list, with the
    /// cursor on the first login row.
    pub(crate) fn enter_choose_mode(&mut self) {
        self.choosing = true;
        self.continue_focused = false;
        self.cursor = 0;
        self.telemetry = None;
    }

    /// Open the telemetry settings sub-page, highlighting "Send everything" so
    /// the most helpful option is the default commit.
    pub(crate) fn open_telemetry(&mut self) {
        self.telemetry = Some(TelemetryLevel::Everything);
    }

    /// Close the telemetry sub-page and return to the summary screen with the
    /// "Telemetry settings" pill still focused.
    pub(crate) fn close_telemetry(&mut self) {
        self.telemetry = None;
    }

    /// Move the telemetry highlight. No-op unless the sub-page is open.
    pub(crate) fn telemetry_step(&mut self, forward: bool) {
        let Some(level) = self.telemetry else { return };
        let order = TelemetryLevel::ORDER;
        let i = order.iter().position(|&l| l == level).unwrap_or(0);
        let next = if forward {
            (i + 1) % order.len()
        } else {
            (i + order.len() - 1) % order.len()
        };
        self.telemetry = Some(order[next]);
    }

    /// Move the summary pill focus. Keeps `continue_focused` in sync so the
    /// existing rendering/commit paths stay correct.
    pub(crate) fn summary_step(&mut self, forward: bool) {
        self.summary_pill = self.summary_pill.step(forward);
        self.continue_focused = self.summary_pill == SummaryPill::Continue;
    }

    /// Focus a specific summary pill (used by the onboarding simulator to land
    /// directly on a given screen state).
    pub(crate) fn focus_summary_pill(&mut self, pill: SummaryPill) {
        self.summary_pill = pill;
        self.continue_focused = pill == SummaryPill::Continue;
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

    /// Whether the decision countdown has elapsed. Paused while the telemetry
    /// settings sub-page is open so the screen never commits the import out
    /// from under a user who is reading it.
    pub(crate) fn timed_out(&self) -> bool {
        self.telemetry.is_none() && self.shown_at.elapsed() >= DECISION_TIMEOUT
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
    /// Action-only picker offering the suggested review or a blank new session.
    StartChoice { shown_at: Instant },
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
    // Detection drives the first-run "continue where you left off" picker, whose
    // only requirement is that resumable transcripts exist. We therefore treat a
    // CLI as present when EITHER its OAuth login file exists OR it has written
    // transcripts. The transcript fallback matters because some tools store
    // credentials outside a plain JSON file (Claude Code and Cursor use the
    // macOS keychain / a vscdb), so an auth-file-only check would silently hide
    // sessions the user clearly has.
    if external_oauth_present(&external_home_path(".codex/auth.json"))
        || external_transcripts_present(&external_home_path(".codex/sessions"), "jsonl")
    {
        found.push(ExternalCli::Codex);
    }
    if external_oauth_present(&external_home_path(".claude/.credentials.json"))
        || external_transcripts_present(&external_home_path(".claude/projects"), "jsonl")
    {
        found.push(ExternalCli::ClaudeCode);
    }
    if external_oauth_present(&external_home_path(".pi/agent/auth.json"))
        || external_transcripts_present(&external_home_path(".pi/agent/sessions"), "jsonl")
    {
        found.push(ExternalCli::Pi);
    }
    if external_oauth_present(&external_home_path(".local/share/opencode/auth.json"))
        || external_transcripts_present(
            &external_home_path(".local/share/opencode/storage/session"),
            "json",
        )
    {
        found.push(ExternalCli::OpenCode);
    }
    // Cursor agent stores its credentials in a vscdb/keychain rather than a
    // plain JSON file, so the reliable "can we resume?" signal is the presence
    // of agent transcripts under ~/.cursor/projects. Fall back to the optional
    // auth.json when transcripts have not been written yet.
    if external_transcripts_present(&external_home_path(".cursor/projects"), "jsonl")
        || external_oauth_present(&external_home_path(".cursor/auth.json"))
        || external_oauth_present(&external_home_path(".config/cursor/auth.json"))
    {
        found.push(ExternalCli::Cursor);
    }
    found
}

/// Whether `root` contains at least one file with the given extension, searched
/// shallowly-recursively. Cheap directory walk used for resume detection.
fn external_transcripts_present(root: &std::path::Path, ext: &str) -> bool {
    fn walk(dir: &std::path::Path, ext: &str, budget: &mut u32) -> bool {
        if *budget == 0 {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            if *budget == 0 {
                return false;
            }
            *budget -= 1;
            let path = entry.path();
            if path.is_dir() {
                if walk(&path, ext, budget) {
                    return true;
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                return true;
            }
        }
        false
    }
    if !root.exists() {
        return false;
    }
    // Bound the walk so a pathological tree cannot stall onboarding.
    let mut budget = 20_000u32;
    walk(root, ext, &mut budget)
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

    #[test]
    fn external_transcripts_present_finds_nested_files() {
        let dir = std::env::temp_dir().join(format!(
            "jcode-onb-transcripts-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = dir.join("projects/demo/agent-transcripts/uuid");
        std::fs::create_dir_all(&nested).unwrap();
        // No matching files yet.
        assert!(!external_transcripts_present(&dir, "jsonl"));
        std::fs::write(nested.join("uuid.jsonl"), b"{}\n").unwrap();
        assert!(external_transcripts_present(&dir, "jsonl"));
        // A different extension should not match.
        assert!(!external_transcripts_present(&dir, "json"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
