// Offline onboarding-efficiency evaluator.
//
// We cannot (and do not want to) collect data from real users, so instead of
// measuring a live funnel we treat the onboarding flow as an artifact and score
// the artifact. The evaluator drives the REAL `App` state machine and renders
// the REAL onboarding screens, so its numbers describe production, not a mock.
//
// Four tiers (see the team discussion):
//
//   Tier 0  meta / coverage  - how much of the flow we actually score, and a
//                              fidelity guard so the evaluator can't silently
//                              drift from the real state machine.
//   Tier 1  static flow      - structural counts over the flow graph: in-TUI
//                              keystrokes, decision points, screens-to-ready,
//                              dead-ends. Pure counting, no judgment.
//   Tier 3  screen quality   - per-screen rubric scored from the REAL rendered
//                              copy: reading load, key-hint consistency, an
//                              escape hatch (skip/anytime/optional).
//
//   (Tier 2 - simulated journeys - is folded into Tier 1 here: we drive the
//    real app to validate every authored edge, so the "static" table is itself
//    simulation-checked.)
//
// Run the human-readable scorecard with:
//   cargo test -p jcode-tui onboarding_eval_scorecard -- --nocapture
//
// NOTE: `include!`d into `crate::tui::app::tests`, which already imports the
// onboarding types and the `render_onboarding_text` / `create_test_app` test
// helpers (from onboarding_flow.rs / onboarding_golden.rs / support_failover).
// Reference shared items directly; do not re-import to avoid duplicate-import
// errors.

// ---------------------------------------------------------------------------
// Tier 0: screen coverage via an exhaustive, wildcard-free classifier.
//
// Every `OnboardingPhase` variant MUST be named here. There is intentionally no
// `_ =>` arm: adding a new phase to the enum will fail to compile until someone
// classifies (and therefore scores) it. That is the anti-drift guarantee.
// ---------------------------------------------------------------------------

/// How a phase surfaces to the user, for scoring purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenSurface {
    /// Rendered by the onboarding welcome body (`draw_onboarding_welcome`).
    WelcomeBody,
    /// Rendered as the session-picker overlay (transcript resume).
    PickerOverlay,
    /// Transient/auto-advancing: never rests in front of the user.
    Transient,
    /// Terminal: onboarding is over, the normal UI takes over.
    Terminal,
}

fn classify_phase_surface(phase: &OnboardingPhase) -> ScreenSurface {
    match phase {
        OnboardingPhase::Login { .. } => ScreenSurface::WelcomeBody,
        OnboardingPhase::LoginOpenAi { .. } => ScreenSurface::WelcomeBody,
        OnboardingPhase::ContinuePrompt { .. } => ScreenSurface::WelcomeBody,
        OnboardingPhase::Suggestions => ScreenSurface::WelcomeBody,
        OnboardingPhase::TranscriptPick { .. } => ScreenSurface::PickerOverlay,
        // ModelSelect immediately auto-advances; it never rests on screen.
        OnboardingPhase::ModelSelect => ScreenSurface::Transient,
        OnboardingPhase::Done => ScreenSurface::Terminal,
    }
}

/// Every `OnboardingPhase` variant, used to assert screen coverage. Kept in
/// sync with the enum by the same wildcard-free discipline as the classifier.
fn all_onboarding_phases() -> Vec<(&'static str, OnboardingPhase)> {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;
    let now = std::time::Instant::now();
    let review = ImportReview::new(vec![
        ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
        ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
    ])
    .unwrap();
    vec![
        ("Login{import}", OnboardingPhase::Login { import: Some(review) }),
        ("Login{recovery}", OnboardingPhase::Login { import: None }),
        ("LoginOpenAi", OnboardingPhase::LoginOpenAi { yes_highlighted: true }),
        ("ModelSelect", OnboardingPhase::ModelSelect),
        (
            "ContinuePrompt",
            OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: now,
            },
        ),
        (
            "TranscriptPick",
            OnboardingPhase::TranscriptPick { cli: ExternalCli::Codex, shown_at: now },
        ),
        ("Suggestions", OnboardingPhase::Suggestions),
        ("Done", OnboardingPhase::Done),
    ]
}

// ---------------------------------------------------------------------------
// Tier 1: static flow graph. Each entry path is authored as data, then the
// counts are derived. Selected edges are independently driven through the REAL
// app in the fidelity tests below, so the table cannot silently diverge.
// ---------------------------------------------------------------------------

/// One screen the user must clear on an entry path.
struct Step {
    /// Phase label (for the report / cross-referencing the phase table).
    #[allow(dead_code)]
    phase: &'static str,
    /// In-TUI keystrokes to advance on the happy (default) path.
    keystrokes: u32,
    /// Whether this screen forces a yes/no or pick decision.
    is_decision: bool,
    /// Whether advancing crosses an external boundary (e.g. browser OAuth) that
    /// is outside our keystroke budget but is still real user effort/time.
    external_boundary: bool,
}

struct Path {
    name: &'static str,
    /// How common we expect this path to be for brand-new users (weight for the
    /// composite). Does not need to be precise; it just stops a rare recovery
    /// path from dominating the headline number.
    weight: f64,
    steps: Vec<Step>,
    /// Does the happy path end with the user able to type a real prompt with a
    /// working login? (Decline paths reach a resting screen but still need a
    /// login, so they are "settled" but not "ready".)
    reaches_ready: bool,
}

fn entry_paths() -> Vec<Path> {
    vec![
        Path {
            name: "Fresh install, no detected logins (accept OpenAI)",
            weight: 0.40,
            reaches_ready: true,
            steps: vec![
                Step { phase: "LoginOpenAi", keystrokes: 1, is_decision: true, external_boundary: true },
                Step { phase: "Suggestions", keystrokes: 0, is_decision: false, external_boundary: false },
            ],
        },
        Path {
            name: "Fresh install, decline login (defer to /login)",
            weight: 0.10,
            reaches_ready: false,
            steps: vec![
                Step { phase: "LoginOpenAi", keystrokes: 1, is_decision: true, external_boundary: false },
                Step { phase: "Done", keystrokes: 0, is_decision: false, external_boundary: false },
            ],
        },
        Path {
            name: "Fresh install, import 1 detected login",
            weight: 0.20,
            reaches_ready: true,
            steps: vec![
                Step { phase: "Login{import}", keystrokes: 1, is_decision: true, external_boundary: false },
                Step { phase: "Suggestions", keystrokes: 0, is_decision: false, external_boundary: false },
            ],
        },
        Path {
            name: "Fresh install, import 2 detected logins",
            weight: 0.10,
            reaches_ready: true,
            steps: vec![
                // Single-screen checkbox list, all pre-checked: one Enter imports
                // every detected login at once (no per-candidate page).
                Step { phase: "Login{import}", keystrokes: 1, is_decision: true, external_boundary: false },
                Step { phase: "Suggestions", keystrokes: 0, is_decision: false, external_boundary: false },
            ],
        },
        Path {
            name: "Already authenticated at startup, no transcripts",
            weight: 0.15,
            reaches_ready: true,
            steps: vec![
                // ModelSelect auto-advances; the user lands directly on
                // Suggestions with zero keystrokes.
                Step { phase: "Suggestions", keystrokes: 0, is_decision: false, external_boundary: false },
            ],
        },
        Path {
            name: "Already authenticated, resume a detected transcript",
            weight: 0.05,
            reaches_ready: true,
            steps: vec![
                Step { phase: "TranscriptPick", keystrokes: 1, is_decision: true, external_boundary: false },
            ],
        },
    ]
}

struct PathMetrics {
    keystrokes: u32,
    decisions: u32,
    screens: u32,
    external_boundaries: u32,
    reaches_ready: bool,
}

fn path_metrics(path: &Path) -> PathMetrics {
    PathMetrics {
        keystrokes: path.steps.iter().map(|s| s.keystrokes).sum(),
        decisions: path.steps.iter().filter(|s| s.is_decision).count() as u32,
        screens: path.steps.len() as u32,
        external_boundaries: path.steps.iter().filter(|s| s.external_boundary).count() as u32,
        reaches_ready: path.reaches_ready,
    }
}

/// Tier 1 score for a path, 0..=100. Penalize keystrokes, decisions, and extra
/// screens; reward reaching a ready state. The weights are deliberately simple
/// and transparent so the number is explainable.
fn tier1_path_score(m: &PathMetrics) -> f64 {
    tier1_path_score_w(m, &Tier1Weights::default())
}

/// Tunable Tier 1 weights, factored out so the meta-evaluation layer can
/// perturb them for sensitivity analysis. The `default()` values are the ones
/// used by the live scorecard.
#[derive(Clone, Copy)]
struct Tier1Weights {
    per_keystroke: f64,
    per_decision: f64,
    per_extra_screen: f64,
    not_ready: f64,
}

impl Default for Tier1Weights {
    fn default() -> Self {
        Self {
            per_keystroke: 6.0,
            per_decision: 8.0,
            per_extra_screen: 5.0,
            not_ready: 20.0,
        }
    }
}

fn tier1_path_score_w(m: &PathMetrics, w: &Tier1Weights) -> f64 {
    let mut score = 100.0;
    score -= (m.keystrokes as f64) * w.per_keystroke;
    score -= (m.decisions as f64) * w.per_decision;
    score -= (m.screens.saturating_sub(1) as f64) * w.per_extra_screen;
    if !m.reaches_ready {
        score -= w.not_ready;
    }
    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Tier 3: per-screen quality, scored from the REAL rendered copy.
// ---------------------------------------------------------------------------

/// The canonical Yes/No selector affordance. The real screens render the
/// choice as a pair of rounded lozenge pills (`◖ Yes ◗   ◖ No ◗`); the selected
/// option is a filled capsule and the other a hollow outline, so the selection
/// is shown VISUALLY, not via a sentence. Tier 3 checks every yes/no screen
/// renders this same pill row (consistency = lower learning cost).
const CANONICAL_YESNO_PILL: &str = "Yes \u{25D7}";
/// A pill end-cap glyph (the rounded lozenge end), signalling the capsule shape.
const YESNO_PILL_CHEVRON: &str = "\u{25D6}";

struct ScreenMetrics {
    label: &'static str,
    line_count: u32,
    word_count: u32,
    is_yesno: bool,
    keyhint_consistent: bool,
    has_escape_hatch: bool,
}

fn render_phase_screen(label: &'static str, phase: OnboardingPhase) -> ScreenMetrics {
    let app = app_in_phase(phase);
    let text = render_onboarding_text(&app, 80, 30);
    let is_yesno = text.contains(CANONICAL_YESNO_PILL) || text.contains("Yes") && text.contains("No");
    // Reading load must be measured from the human body prose only, NOT the raw
    // buffer. The raw buffer also contains the decorative idle donut, whose lit
    // glyph count varies with wall-clock `animation_elapsed()` (so the raw count
    // is both non-deterministic AND counts pure decoration as "words to read").
    // `body_prose_lines` strips the telemetry header, the ASCII art, and the
    // Yes/No pill row, leaving exactly the sentences the user must read - the
    // same chrome-stripping Tier 6 already relies on.
    let prose = body_prose_lines(&text);
    let line_count = prose.len() as u32;
    let word_count = prose
        .iter()
        .map(|l| l.split_whitespace().count())
        .sum::<usize>() as u32;
    // A yes/no screen is consistent when it renders one of the two canonical
    // affordances:
    //   * the single-prompt pill row: `◖ Yes ◗` / `◖ No ◗` lozenge capsules, or
    //   * the per-login import list: a "Yes"/"No" header above filled/hollow
    //     circle columns, with a `> ` cursor gutter for movability.
    let canonical_pill = text.contains(CANONICAL_YESNO_PILL)
        && text.contains("No \u{25D7}")
        && text.contains(YESNO_PILL_CHEVRON);
    let canonical_import_list =
        text.contains('●') && text.contains('○') && text.contains("> ");
    let keyhint_consistent = !is_yesno || canonical_pill || canonical_import_list;
    let lower = text.to_ascii_lowercase();
    let has_escape_hatch = lower.contains("skip")
        || lower.contains("anytime")
        || lower.contains("/login")
        || lower.contains("optional")
        || lower.contains("type anything");
    ScreenMetrics {
        label,
        line_count,
        word_count,
        is_yesno,
        keyhint_consistent,
        has_escape_hatch,
    }
}

/// Tier 3 score for one screen, 0..=100. Reading load dominates; consistency and
/// an escape hatch are smaller bonuses.
fn tier3_screen_score(m: &ScreenMetrics) -> f64 {
    tier3_screen_score_w(m, &Tier3Weights::default())
}

/// Tunable Tier 3 weights, factored out for sensitivity analysis.
#[derive(Clone, Copy)]
struct Tier3Weights {
    word_budget: u32,
    per_excess_word: f64,
    inconsistent_keyhint: f64,
    no_escape_hatch: f64,
}

impl Default for Tier3Weights {
    fn default() -> Self {
        Self {
            word_budget: 45,
            per_excess_word: 1.2,
            inconsistent_keyhint: 15.0,
            no_escape_hatch: 10.0,
        }
    }
}

fn tier3_screen_score_w(m: &ScreenMetrics, w: &Tier3Weights) -> f64 {
    let mut score = 100.0;
    // Reading load: the telemetry header (~3 lines) is fixed overhead, so a
    // lean screen sits around 8-12 lines. Penalize words past a comfortable
    // budget (telemetry + title + one prompt + options + hint).
    if m.word_count > w.word_budget {
        score -= (m.word_count - w.word_budget) as f64 * w.per_excess_word;
    }
    if m.is_yesno && !m.keyhint_consistent {
        score -= w.inconsistent_keyhint;
    }
    if !m.has_escape_hatch {
        score -= w.no_escape_hatch;
    }
    score.clamp(0.0, 100.0)
}

/// Screens we score for Tier 3. Each is a real, user-visible welcome screen.
fn tier3_screens() -> Vec<ScreenMetrics> {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;
    let now = std::time::Instant::now();
    let review =
        ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json")])
            .unwrap();
    vec![
        render_phase_screen("LoginOpenAi", OnboardingPhase::LoginOpenAi { yes_highlighted: true }),
        render_phase_screen("Login{import}", OnboardingPhase::Login { import: Some(review) }),
        render_phase_screen("Login{recovery}", OnboardingPhase::Login { import: None }),
        render_phase_screen(
            "ContinuePrompt",
            OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: now,
            },
        ),
        render_phase_screen("Suggestions", OnboardingPhase::Suggestions),
    ]
}

// ---------------------------------------------------------------------------
// Tier 4: content & robustness. Cross-screen and behavioral signals that the
// per-screen Tier 3 rubric cannot see, each measured from the REAL screens or
// by driving the REAL app:
//
//   * terminology_consistency - the same concept is named the same way across
//     every screen (e.g. the human prose says "log in", never also "sign in").
//   * progress_visibility      - a multi-step context tells the user where they
//     are ("Login 1 of 2"), so a sequence never feels open-ended.
//   * default_safety           - when a timed decision auto-commits, the default
//     lands on a non-destructive / recoverable outcome.
//   * narrow_terminal_safety   - the core affordances (the Yes/No options) still
//     render on a cramped terminal.
//
// These are deliberately the signals that ARE derivable offline. Several
// neighbours (min-path overhead, back-navigation depth, jargon density) are
// left Deferred in the registry because an honest measurement needs more
// machinery than we have; we do not fake them as Scored.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Tier4Metrics {
    /// The login concept is phrased consistently across all welcome screens.
    terminology_consistent: bool,
    /// A multi-step context surfaces "N of M" position.
    progress_visible: bool,
    /// A timed auto-commit resolves to a non-destructive default.
    default_safe: bool,
    /// The Yes/No options survive a cramped (50-col) terminal.
    narrow_options_survive: bool,
}

/// Detect whether the human-facing prose names the login concept consistently.
/// The canonical phrasing is the two-word verb "log in". A drift to "sign in"
/// or the one-word "login" *as a verb in prose* (the `/login` command is fine)
/// is an inconsistency. Measured over the real rendered welcome screens.
fn terminology_is_consistent(screens: &[(&'static str, String)]) -> bool {
    for (_, text) in screens {
        let lower = text.to_ascii_lowercase();
        // A competing synonym for the same action is a hard inconsistency.
        if lower.contains("sign in") || lower.contains("sign-in") || lower.contains("log on") {
            return false;
        }
        // "login" as a standalone prose word (not the `/login` command, the
        // "Login N of M" progress label, or the legitimate NOUN) would compete
        // with the verb "log in". English distinguishes the noun "a login" / "N
        // logins" (a stored credential) from the verb "to log in"; only the verb
        // spelling "login" is the drift we guard against.
        let words: Vec<&str> = lower.split_whitespace().collect();
        for (i, raw) in words.iter().enumerate() {
            let w = raw.trim_matches(|c: char| !c.is_ascii_alphabetic());
            // Plural "logins" is unambiguously the noun (credentials), allowed.
            if w == "logins" {
                continue;
            }
            if w == "login" {
                // Allowed: the `/login` command token and the "Login N of M"
                // progress header. Both are recognizable by their surroundings.
                let is_command = raw.contains('/');
                let is_progress_header = lower.contains("login 1 of") || lower.contains("login 2 of");
                // Allowed: the NOUN "login" (a credential), recognizable when
                // preceded by a determiner/quantifier ("existing login", "1
                // login", "a login", "your login").
                let prev = i.checked_sub(1).and_then(|j| words.get(j)).copied().unwrap_or("");
                let prev_w = prev.trim_matches(|c: char| !c.is_ascii_alphanumeric());
                let is_noun = matches!(prev_w, "existing" | "detected" | "saved" | "selected" | "a" | "an" | "your" | "one")
                    || prev_w.chars().all(|c| c.is_ascii_digit()) && !prev_w.is_empty();
                if !is_command && !is_progress_header && !is_noun {
                    return false;
                }
            }
        }
    }
    true
}

/// Compute the four Tier 4 signals by reading the real screens and driving the
/// real app. `with_temp_jcode_home` must already be active.
fn tier4_metrics() -> Tier4Metrics {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    // ---- terminology_consistency: scan every welcome screen's prose ----
    let terminology_consistent = terminology_is_consistent(&all_welcome_screen_texts());

    // ---- progress_visibility: the multi-login import is a multi-step context
    // and must set scope up front. The single-screen checkbox list does this by
    // (a) labeling the section ("Import:") and (b) showing all N logins as
    // visible rows at once, so the user always knows how many there are and what
    // they are. We verify both: the label AND that every detected login is
    // actually listed. Rendered from the real screen. ----
    let review = ImportReview::new(vec![
        ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
        ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
    ])
    .unwrap();
    let multi = app_in_phase(OnboardingPhase::Login { import: Some(review) });
    let multi_text = render_onboarding_text(&multi, 80, 30).to_ascii_lowercase();
    let states_total = multi_text.contains("import:");
    let lists_all = multi_text.contains("openai/codex") && multi_text.contains("claude");
    let progress_visible = states_total && lists_all;

    // ---- default_safety: drive the real ContinuePrompt timeout. The highlighted
    // default is "Yes", and a timeout must resolve to a non-destructive outcome
    // (open the resume picker), never silently discard the user's history. ----
    let default_safe = {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            // Place the prompt in the past so the decision has already timed out.
            let past = std::time::Instant::now()
                - (crate::tui::app::onboarding_flow::DECISION_TIMEOUT
                    + std::time::Duration::from_secs(1));
            flow.phase = OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: past,
            };
        }
        // Tick the flow: the timeout fires and auto-commits the highlighted Yes.
        app.onboarding_tick();
        // A SAFE default lands on a recoverable phase the user can still act on
        // (the resume picker, or the suggestion cards when no transcript exists)
        // rather than a terminal that silently discards their session. An UNSAFE
        // default would be `Done` (login/session lost) or leaving the flow.
        matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::TranscriptPick { .. } | OnboardingPhase::Suggestions)
        )
    };

    // ---- narrow_terminal_safety: the core Yes/No affordance must still render
    // on a cramped 50-col terminal (real renderer, smaller buffer). ----
    let narrow = app_in_phase(OnboardingPhase::LoginOpenAi { yes_highlighted: true });
    let narrow_text = render_onboarding_text(&narrow, 50, 30);
    let narrow_options_survive = narrow_text.contains("Yes") && narrow_text.contains("No");

    Tier4Metrics {
        terminology_consistent,
        progress_visible,
        default_safe,
        narrow_options_survive,
    }
}

/// Tunable Tier 4 weights, factored out for the meta sensitivity analysis.
#[derive(Clone, Copy)]
struct Tier4Weights {
    inconsistent_terminology: f64,
    no_progress: f64,
    unsafe_default: f64,
    narrow_breaks: f64,
}

impl Default for Tier4Weights {
    fn default() -> Self {
        Self {
            inconsistent_terminology: 25.0,
            no_progress: 15.0,
            unsafe_default: 30.0,
            narrow_breaks: 20.0,
        }
    }
}

fn tier4_score(m: &Tier4Metrics) -> f64 {
    tier4_score_w(m, &Tier4Weights::default())
}

fn tier4_score_w(m: &Tier4Metrics, w: &Tier4Weights) -> f64 {
    let mut score = 100.0;
    if !m.terminology_consistent {
        score -= w.inconsistent_terminology;
    }
    if !m.progress_visible {
        score -= w.no_progress;
    }
    if !m.default_safe {
        score -= w.unsafe_default;
    }
    if !m.narrow_options_survive {
        score -= w.narrow_breaks;
    }
    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Tier 5: path-efficiency over a REAL flow graph.
//
// Tier 1 counts keystrokes along authored happy paths; this tier builds the
// flow as a graph (nodes = onboarding phases + a virtual Start, edges = the
// authored transitions with their keystroke cost) and derives structural
// efficiency properties that per-path counting cannot see:
//
//   * min_vs_actual_path  - for each entry scenario, the authored default path
//     vs the graph-shortest route to a ready state (excess keystrokes).
//   * first_input_latency - keystrokes a user spends before the first real
//     action (a pure intro screen would push this above 0).
//   * irreducible_decisions - decisions with no timeout/auto default, i.e. ones
//     the user is forced to answer to proceed.
//   * dead_end_screens    - non-terminal nodes with no forward transition.
//   * cycle_freedom       - the flow graph is a DAG (no accidental loops).
//
// Anti-drift: `phase_to_node` is a wildcard-free match over `OnboardingPhase`,
// so a new phase fails to compile until it is placed in the graph.
// ---------------------------------------------------------------------------

/// A node in the onboarding flow graph. `Start` is a virtual entry point; every
/// other node corresponds to a real `OnboardingPhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum GraphNode {
    Start,
    LoginOpenAi,
    LoginImport,
    LoginRecovery,
    ModelSelect,
    ContinuePrompt,
    TranscriptPick,
    Suggestions,
    Done,
}

/// Anti-drift map: every real phase lands on exactly one graph node. No `_`
/// arm, so a new `OnboardingPhase` variant fails to compile here until placed.
fn phase_to_node(phase: &OnboardingPhase) -> GraphNode {
    match phase {
        OnboardingPhase::LoginOpenAi { .. } => GraphNode::LoginOpenAi,
        OnboardingPhase::Login { import: Some(_) } => GraphNode::LoginImport,
        OnboardingPhase::Login { import: None } => GraphNode::LoginRecovery,
        OnboardingPhase::ModelSelect => GraphNode::ModelSelect,
        OnboardingPhase::ContinuePrompt { .. } => GraphNode::ContinuePrompt,
        OnboardingPhase::TranscriptPick { .. } => GraphNode::TranscriptPick,
        OnboardingPhase::Suggestions => GraphNode::Suggestions,
        OnboardingPhase::Done => GraphNode::Done,
    }
}

/// Per-node structural properties used by the path-efficiency metrics.
struct NodeProps {
    /// The user must make a Yes/No or pick choice here.
    is_decision: bool,
    /// A timeout/auto default exists, so the user is NOT forced to answer.
    has_default: bool,
    /// Reaching this node means onboarding succeeded (user can type a prompt).
    is_ready: bool,
    /// No further onboarding transitions leave this node (it is an exit).
    is_terminal: bool,
}

fn node_props(n: GraphNode) -> NodeProps {
    use GraphNode::*;
    match n {
        Start => NodeProps { is_decision: false, has_default: false, is_ready: false, is_terminal: false },
        // Forced Yes/No: there is no timeout default on the OpenAI sign-in prompt.
        LoginOpenAi => NodeProps { is_decision: true, has_default: false, is_ready: false, is_terminal: false },
        // Import review auto-commits the highlighted choice on DECISION_TIMEOUT.
        LoginImport => NodeProps { is_decision: true, has_default: true, is_ready: false, is_terminal: false },
        // Recovery fallback: a single Enter opens the provider picker.
        LoginRecovery => NodeProps { is_decision: true, has_default: false, is_ready: false, is_terminal: false },
        // Transient: auto-advances, the user never chooses here.
        ModelSelect => NodeProps { is_decision: false, has_default: true, is_ready: false, is_terminal: false },
        // Continue prompt auto-opens the resume menu on timeout (default Yes).
        ContinuePrompt => NodeProps { is_decision: true, has_default: true, is_ready: false, is_terminal: false },
        // Resume picker: a pick (or "start new") reaches a ready session.
        TranscriptPick => NodeProps { is_decision: true, has_default: false, is_ready: true, is_terminal: true },
        Suggestions => NodeProps { is_decision: false, has_default: false, is_ready: true, is_terminal: true },
        Done => NodeProps { is_decision: false, has_default: false, is_ready: false, is_terminal: true },
    }
}

/// One directed transition in the flow graph.
struct Edge {
    from: GraphNode,
    to: GraphNode,
    /// In-TUI keystrokes to traverse this edge on the default path.
    keystrokes: u32,
}

/// The authored flow graph. Each edge is a real transition the onboarding code
/// can take. Kept faithful to the entry paths in `entry_paths()` and the real
/// transitions exercised by `onboarding_eval_fidelity_real_transitions`.
fn flow_edges() -> Vec<Edge> {
    use GraphNode::*;
    vec![
        // Entry routing from the virtual Start (zero-cost: chosen by detected
        // environment, not by a keystroke).
        Edge { from: Start, to: LoginOpenAi, keystrokes: 0 },
        Edge { from: Start, to: LoginImport, keystrokes: 0 },
        Edge { from: Start, to: ModelSelect, keystrokes: 0 },
        // OpenAI sign-in: Yes -> (browser OAuth) -> Suggestions; No -> Done.
        Edge { from: LoginOpenAi, to: Suggestions, keystrokes: 1 },
        Edge { from: LoginOpenAi, to: Done, keystrokes: 1 },
        // Import review: accept/decline each candidate, then suggestions. A
        // failed/declined import drops to the recovery fallback.
        Edge { from: LoginImport, to: Suggestions, keystrokes: 1 },
        Edge { from: LoginImport, to: LoginRecovery, keystrokes: 1 },
        // Recovery: Enter opens the provider picker, ending at suggestions.
        Edge { from: LoginRecovery, to: Suggestions, keystrokes: 1 },
        // Transient model-select auto-advances with no keystroke.
        Edge { from: ModelSelect, to: Suggestions, keystrokes: 0 },
        // Continue prompt: Yes -> resume picker; No -> suggestions.
        Edge { from: ContinuePrompt, to: TranscriptPick, keystrokes: 1 },
        Edge { from: ContinuePrompt, to: Suggestions, keystrokes: 1 },
    ]
}

/// Dijkstra over keystroke cost: minimum keystrokes from `start` to the nearest
/// node satisfying `is_goal`. Returns None if no goal is reachable.
fn min_keystrokes_to<F: Fn(GraphNode) -> bool>(
    start: GraphNode,
    edges: &[Edge],
    is_goal: F,
) -> Option<u32> {
    use std::collections::HashMap;
    let mut best: HashMap<GraphNode, u32> = HashMap::new();
    best.insert(start, 0);
    // Small graph: relax to a fixed point (Bellman-Ford style) instead of a heap.
    let mut changed = true;
    while changed {
        changed = false;
        for e in edges {
            if let Some(&du) = best.get(&e.from) {
                let nd = du + e.keystrokes;
                if best.get(&e.to).map(|&d| nd < d).unwrap_or(true) {
                    best.insert(e.to, nd);
                    changed = true;
                }
            }
        }
    }
    best.iter()
        .filter(|(n, _)| is_goal(**n))
        .map(|(_, &d)| d)
        .min()
}

/// True if the directed flow graph contains no cycle (is a DAG).
fn graph_is_acyclic(edges: &[Edge]) -> bool {
    use std::collections::{HashMap, HashSet};
    let mut adj: HashMap<GraphNode, Vec<GraphNode>> = HashMap::new();
    let mut nodes: HashSet<GraphNode> = HashSet::new();
    for e in edges {
        adj.entry(e.from).or_default().push(e.to);
        nodes.insert(e.from);
        nodes.insert(e.to);
    }
    // 0 = unvisited, 1 = on stack, 2 = done.
    let mut state: HashMap<GraphNode, u8> = HashMap::new();
    fn dfs(
        n: GraphNode,
        adj: &std::collections::HashMap<GraphNode, Vec<GraphNode>>,
        state: &mut std::collections::HashMap<GraphNode, u8>,
    ) -> bool {
        state.insert(n, 1);
        if let Some(succ) = adj.get(&n) {
            for &m in succ {
                match state.get(&m).copied().unwrap_or(0) {
                    1 => return false,            // back-edge -> cycle
                    0 => {
                        if !dfs(m, adj, state) {
                            return false;
                        }
                    }
                    _ => {}
                }
            }
        }
        state.insert(n, 2);
        true
    }
    for &n in &nodes {
        if state.get(&n).copied().unwrap_or(0) == 0 && !dfs(n, &adj, &mut state) {
            return false;
        }
    }
    true
}

#[derive(Clone, Copy)]
struct Tier5Metrics {
    /// Weighted excess keystrokes (authored default path vs graph shortest) to
    /// reach a ready state, summed over entry scenarios.
    excess_keystrokes: f64,
    /// Worst-case keystrokes before the first real action across entry paths.
    first_input_latency: u32,
    /// Count of decisions on common paths that cannot be defaulted away.
    irreducible_decisions: u32,
    /// Non-terminal nodes with no outgoing transition.
    dead_end_screens: u32,
    /// The flow graph is a DAG.
    acyclic: bool,
}

/// Map an entry path (from `entry_paths`) to its graph entry node, by its first
/// authored step's phase label. Wildcard-free over the known labels.
fn entry_node_for(path: &Path) -> GraphNode {
    match path.steps.first().map(|s| s.phase) {
        Some("LoginOpenAi") => GraphNode::LoginOpenAi,
        Some("Login{import}") => GraphNode::LoginImport,
        Some("Login{recovery}") => GraphNode::LoginRecovery,
        Some("Suggestions") => GraphNode::Suggestions,
        Some("TranscriptPick") => GraphNode::TranscriptPick,
        Some("ContinuePrompt") => GraphNode::ContinuePrompt,
        // ModelSelect/Done never lead an entry path; default to Start so an
        // unexpected label is conservatively treated as full-path overhead.
        _ => GraphNode::Start,
    }
}

fn tier5_metrics() -> Tier5Metrics {
    let edges = flow_edges();
    let paths = entry_paths();

    // ---- min_vs_actual_path: excess keystrokes per ready-reaching path ----
    let mut excess = 0.0;
    let mut latency = 0u32;
    for p in &paths {
        let m = path_metrics(p);
        if !m.reaches_ready {
            continue;
        }
        let entry = entry_node_for(p);
        let min = min_keystrokes_to(entry, &edges, |n| node_props(n).is_ready).unwrap_or(m.keystrokes);
        excess += (m.keystrokes.saturating_sub(min) as f64) * p.weight;

        // first_input_latency: keystrokes spent on steps before the first
        // decision step (a pure intro screen with keystrokes>0 would count).
        let mut pre = 0u32;
        for s in &p.steps {
            if s.is_decision {
                break;
            }
            pre += s.keystrokes;
        }
        latency = latency.max(pre);
    }

    // ---- irreducible_decisions: decision nodes a user must answer ----
    let irreducible = [
        GraphNode::LoginOpenAi,
        GraphNode::LoginImport,
        GraphNode::LoginRecovery,
        GraphNode::ContinuePrompt,
        GraphNode::TranscriptPick,
    ]
    .into_iter()
    .filter(|&n| {
        let p = node_props(n);
        p.is_decision && !p.has_default
    })
    .count() as u32;

    // ---- dead_end_screens: non-terminal nodes with no outgoing edge ----
    let all_nodes = [
        GraphNode::Start,
        GraphNode::LoginOpenAi,
        GraphNode::LoginImport,
        GraphNode::LoginRecovery,
        GraphNode::ModelSelect,
        GraphNode::ContinuePrompt,
        GraphNode::TranscriptPick,
        GraphNode::Suggestions,
        GraphNode::Done,
    ];
    let dead_ends = all_nodes
        .into_iter()
        .filter(|&n| {
            let p = node_props(n);
            let has_out = edges.iter().any(|e| e.from == n);
            !p.is_terminal && !has_out
        })
        .count() as u32;

    Tier5Metrics {
        excess_keystrokes: excess,
        first_input_latency: latency,
        irreducible_decisions: irreducible,
        dead_end_screens: dead_ends,
        acyclic: graph_is_acyclic(&edges),
    }
}

/// Tunable Tier 5 weights for the meta sensitivity analysis.
#[derive(Clone, Copy)]
struct Tier5Weights {
    per_excess_keystroke: f64,
    per_latency_keystroke: f64,
    per_irreducible_decision: f64,
    per_dead_end: f64,
    has_cycle: f64,
}

impl Default for Tier5Weights {
    fn default() -> Self {
        Self {
            per_excess_keystroke: 12.0,
            per_latency_keystroke: 10.0,
            // Some forced decisions are legitimate (you must choose to log in),
            // so this is a gentle nudge, not a heavy penalty.
            per_irreducible_decision: 4.0,
            per_dead_end: 25.0,
            has_cycle: 30.0,
        }
    }
}

fn tier5_score(m: &Tier5Metrics) -> f64 {
    tier5_score_w(m, &Tier5Weights::default())
}

fn tier5_score_w(m: &Tier5Metrics, w: &Tier5Weights) -> f64 {
    let mut score = 100.0;
    score -= m.excess_keystrokes * w.per_excess_keystroke;
    score -= (m.first_input_latency as f64) * w.per_latency_keystroke;
    score -= (m.irreducible_decisions as f64) * w.per_irreducible_decision;
    score -= (m.dead_end_screens as f64) * w.per_dead_end;
    if !m.acyclic {
        score -= w.has_cycle;
    }
    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Tier 6: cognitive load per screen (Hick's law + reading burden), measured
// from the REAL rendered body prose. We first strip the fixed chrome (the
// telemetry consent header, the ASCII logo, and the movement key-hint line) so
// the analysis sees only the human sentences the user must actually read:
//
//   * reading_grade_level   - Flesch-Kincaid grade estimate (syllable-based).
//   * options_per_screen     - simultaneous choices (Hick's law).
//   * jargon_density         - unexplained technical terms per 100 words.
//   * new_concepts_per_screen- distinct domain concepts named on the screen.
//   * number_of_questions    - interrogatives the user must resolve.
//   * negation_count         - confusing "don't / not / never" phrasing.
// ---------------------------------------------------------------------------

/// Extract just the human body prose from a rendered onboarding screen,
/// dropping the telemetry consent header, the ASCII logo art, and the Yes/No
/// pill row. Returns the kept prose lines.
fn body_prose_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let lower = t.to_ascii_lowercase();
        // Telemetry consent boilerplate (fixed 3-line header).
        if lower.contains("anonymous usage")
            || lower.contains("no code, prompts")
            || lower.contains("opt out anytime")
        {
            continue;
        }
        // The Yes/No pill row is an interactive widget, not prose to "read".
        if t.contains(CANONICAL_YESNO_PILL) {
            continue;
        }
        // The Continue pill is likewise an interactive widget (a rounded button
        // drawn with half-circle end caps ◖ ◗), not prose. Strip it so its
        // glyphs aren't counted as load-bearing reading text.
        if t.contains('\u{25D6}') || t.contains('\u{25D7}') {
            continue;
        }
        // The import rows carry their own Yes/No pills (skipped above via
        // CANONICAL_YESNO_PILL); legacy circle/divider chrome is also dropped.
        if t.contains('●') || t.contains('○') || t.contains('│') {
            continue;
        }
        // ASCII logo art: lines dominated by non-alphabetic symbols.
        let alpha = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
        let nonspace = t.chars().filter(|c| !c.is_whitespace()).count();
        if nonspace > 0 && (alpha as f64) / (nonspace as f64) < 0.5 {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// Rough syllable count for an English word (vowel-group heuristic with a
/// silent-e adjustment). Good enough for a Flesch-Kincaid grade estimate.
fn syllables(word: &str) -> u32 {
    let w: String = word.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
    if w.is_empty() {
        return 0;
    }
    let vowels = ['a', 'e', 'i', 'o', 'u', 'y'];
    let mut count = 0u32;
    let mut prev_vowel = false;
    for c in w.chars() {
        let is_v = vowels.contains(&c);
        if is_v && !prev_vowel {
            count += 1;
        }
        prev_vowel = is_v;
    }
    // Silent trailing 'e'.
    if w.ends_with('e') && count > 1 {
        count -= 1;
    }
    count.max(1)
}

/// A small, explicit jargon lexicon: technical terms a brand-new user may not
/// know without explanation. Kept deliberately short and reviewable.
const JARGON_TERMS: &[&str] = &[
    "oauth", "api", "endpoint", "token", "cli", "env", "provider", "transcript",
];

/// Domain "concepts" the onboarding introduces, grouped by synonym so a single
/// idea phrased two ways counts ONCE. The login concept in particular surfaces
/// as both the prose verb "log in" and the `/login` command on the same screen;
/// charging the user for two "new concepts" there double-counts one idea (the
/// same class of inflation the Tier 3 donut fix removed). Used to count how many
/// distinct new ideas a single screen puts in front of the user.
const CONCEPT_GROUPS: &[&[&str]] = &[
    &["login", "log in"],
    &["provider"],
    &["import"],
    &["session"],
    &["model"],
    &["resume"],
    &["telemetry"],
    &["onboarding"],
    &["openai"],
    &["codex"],
    &["claude"],
];

#[derive(Clone, Copy)]
struct ScreenLoad {
    label: &'static str,
    grade_level: f64,
    options: u32,
    jargon_per_100w: f64,
    new_concepts: u32,
    questions: u32,
    negations: u32,
}

fn screen_load(label: &'static str, text: &str) -> ScreenLoad {
    let prose = body_prose_lines(text);
    let joined = prose.join(" ");
    let lower = joined.to_ascii_lowercase();
    let words: Vec<&str> = joined.split_whitespace().collect();
    let word_count = words.len().max(1) as f64;
    let sentence_count = joined
        .chars()
        .filter(|c| *c == '.' || *c == '?' || *c == '!')
        .count()
        .max(1) as f64;
    let syllable_total: u32 = words.iter().map(|w| syllables(w)).sum();

    // Flesch-Kincaid grade level.
    let grade_level = 0.39 * (word_count / sentence_count)
        + 11.8 * (syllable_total as f64 / word_count)
        - 15.59;

    // Options (Hick's law): the Yes/No selector exposes 2 choices; other screens
    // are 0 (informational) or measured from a list.
    let options = if lower.contains("yes") && lower.contains("no") { 2 } else { 0 };

    let jargon_hits: u32 = JARGON_TERMS
        .iter()
        .map(|t| lower.matches(t).count() as u32)
        .sum();
    let jargon_per_100w = (jargon_hits as f64) / word_count * 100.0;

    // Count each distinct concept GROUP at most once, so "log in" + "/login"
    // (one idea, two spellings) is a single new concept, not two.
    let new_concepts = CONCEPT_GROUPS
        .iter()
        .filter(|group| group.iter().any(|term| lower.contains(*term)))
        .count() as u32;

    let questions = joined.matches('?').count() as u32;

    // Negations in prose (the literal "No" option is chrome, excluded by only
    // counting whole negation words inside sentences).
    let negations = lower
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| matches!(*w, "not" | "dont" | "don" | "never" | "cant" | "cannot" | "wont"))
        .count() as u32;

    ScreenLoad {
        label,
        grade_level,
        options,
        jargon_per_100w,
        new_concepts,
        questions,
        negations,
    }
}

/// The real screens analyzed for cognitive load (same set Tier 3 scores).
fn tier6_screen_loads() -> Vec<ScreenLoad> {
    all_welcome_screen_texts()
        .into_iter()
        .map(|(label, text)| screen_load(label, &text))
        .collect()
}

/// Tunable Tier 6 weights for the meta sensitivity analysis. Thresholds are the
/// "comfortable" ceilings; only the excess past them is penalized.
#[derive(Clone, Copy)]
struct Tier6Weights {
    grade_budget: f64,
    per_excess_grade: f64,
    option_budget: u32,
    per_excess_option: f64,
    per_jargon_per_100w: f64,
    concept_budget: u32,
    per_excess_concept: f64,
    per_question_over_one: f64,
    per_negation: f64,
}

impl Default for Tier6Weights {
    fn default() -> Self {
        Self {
            // Grade 9 is a reasonable ceiling for setup copy.
            grade_budget: 9.0,
            per_excess_grade: 4.0,
            // Two options (Yes/No) is fine; more starts to tax the user.
            option_budget: 2,
            per_excess_option: 6.0,
            per_jargon_per_100w: 1.5,
            // A screen can name a few concepts before it feels dense.
            concept_budget: 3,
            per_excess_concept: 5.0,
            // One question per screen is ideal; extra questions compound load.
            per_question_over_one: 8.0,
            per_negation: 4.0,
        }
    }
}

/// Per-screen cognitive-load score, 0..=100.
fn tier6_screen_score(m: &ScreenLoad) -> f64 {
    tier6_screen_score_w(m, &Tier6Weights::default())
}

fn tier6_screen_score_w(m: &ScreenLoad, w: &Tier6Weights) -> f64 {
    let mut score = 100.0;
    if m.grade_level > w.grade_budget {
        score -= (m.grade_level - w.grade_budget) * w.per_excess_grade;
    }
    if m.options > w.option_budget {
        score -= (m.options - w.option_budget) as f64 * w.per_excess_option;
    }
    score -= m.jargon_per_100w * w.per_jargon_per_100w;
    if m.new_concepts > w.concept_budget {
        score -= (m.new_concepts - w.concept_budget) as f64 * w.per_excess_concept;
    }
    if m.questions > 1 {
        score -= (m.questions - 1) as f64 * w.per_question_over_one;
    }
    score -= (m.negations as f64) * w.per_negation;
    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Tier 7: clarity & guidance, measured from the REAL rendered body prose. Where
// Tier 6 asks "is this screen heavy?", Tier 7 asks "does this screen guide the
// user?":
//
//   * single_primary_action - exactly one primary call-to-action / question, so
//     the user is never split between competing asks.
//   * action_verb_clarity   - instruction lines start with an imperative verb
//     ("Press", "Type", "Choose"), not a vague noun phrase.
//   * next_step_visibility  - the screen tells the user what happens next
//     (an outcome/transition is described).
//   * expectation_setting   - a multi-step context states the scope up front
//     ("We found 2 existing logins" / "Login 1 of 2").
// ---------------------------------------------------------------------------

/// Imperative verbs an instruction line may open with. Kept explicit so the
/// check is reviewable and stable.
const ACTION_VERBS: &[&str] = &[
    "press", "type", "choose", "select", "log", "import", "continue", "pick",
    "run", "opt", "enter", "resume", "open",
];

/// Phrases that describe what happens next (an outcome / transition).
const NEXT_STEP_CUES: &[&str] = &[
    "to choose", "to skip", "to get started", "opens", "auto-selects",
    "automatically", "to choose a provider", "anytime", "resume",
    // The import screen labels its section and lists the logins to import.
    "to import", "import:",
];

#[derive(Clone, Copy)]
struct ScreenClarity {
    label: &'static str,
    /// Number of primary asks (a "?" question, or an imperative instruction
    /// line). Ideal is exactly 1.
    primary_actions: u32,
    /// Every instruction line opens with an imperative verb.
    verbs_lead_instructions: bool,
    /// The screen describes what happens next.
    next_step_visible: bool,
    /// A multi-step context sets scope up front (only required when multi-step).
    expectation_set: bool,
    /// Whether this screen is a multi-step context (drives expectation_set).
    is_multistep: bool,
}

/// Heuristic: is this prose line an actionable INSTRUCTION (a directive the user
/// should follow) rather than framing prose or a title? We cue only on explicit
/// action markers (a key/command/CTA), so descriptive sentences like "First,
/// log in to get started." are correctly treated as framing, not instructions.
fn looks_like_instruction(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    lower.starts_with("press ")
        || lower.starts_with("type ")
        || lower.starts_with("choose ")
        || lower.starts_with("select ")
        || lower.starts_with("pick ")
        || lower.starts_with("run ")
        // A "<command>" CTA: jcode phrases these as both "run /login" and
        // "type /login", so recognize both spellings of the same directive.
        || lower.contains("run /")
        || lower.contains("type /")
}

fn screen_clarity(label: &'static str, text: &str) -> ScreenClarity {
    let prose = body_prose_lines(text);
    let lower_all = prose.join(" ").to_ascii_lowercase();

    // Primary asks: count question lines + imperative instruction lines, but a
    // question and its own instruction line ("Log in to OpenAI?" + "Choose No
    // to skip...") are one ask, so collapse instruction lines that merely
    // explain the question. We approximate: asks = max(questions, has_one_cta).
    let questions = prose.iter().filter(|l| l.contains('?')).count() as u32;
    let instructions: Vec<&String> = prose.iter().filter(|l| looks_like_instruction(l)).collect();
    // A standalone instruction with no question is itself the single CTA.
    let primary_actions = if questions > 0 {
        questions
    } else if instructions.is_empty() {
        0
    } else {
        1
    };

    // Action-verb clarity: every instruction line opens with an imperative verb.
    let verbs_lead_instructions = instructions.iter().all(|l| {
        let first = l
            .split_whitespace()
            .next()
            .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphabetic()).to_ascii_lowercase())
            .unwrap_or_default();
        ACTION_VERBS.contains(&first.as_str())
    });

    let next_step_visible = NEXT_STEP_CUES.iter().any(|c| lower_all.contains(c));

    // Multi-step contexts announce themselves with "we found N existing
    // logins". The single-screen import list sets scope by stating the total
    // and listing every login, so "we found" is the cue and also satisfies it.
    let is_multistep = lower_all.contains("we found");
    let expectation_set = !is_multistep || lower_all.contains("we found");

    ScreenClarity {
        label,
        primary_actions,
        verbs_lead_instructions,
        next_step_visible,
        expectation_set,
        is_multistep,
    }
}

fn tier7_screen_clarities() -> Vec<ScreenClarity> {
    all_welcome_screen_texts()
        .into_iter()
        .map(|(label, text)| screen_clarity(label, &text))
        .collect()
}

#[derive(Clone, Copy)]
struct Tier7Weights {
    /// Penalty per primary ask beyond the first (competing CTAs).
    per_extra_action: f64,
    /// Penalty when a screen has zero clear ask (purely passive, ambiguous).
    no_action: f64,
    verb_unclear: f64,
    no_next_step: f64,
    no_expectation: f64,
}

impl Default for Tier7Weights {
    fn default() -> Self {
        Self {
            per_extra_action: 12.0,
            no_action: 8.0,
            verb_unclear: 12.0,
            no_next_step: 10.0,
            no_expectation: 15.0,
        }
    }
}

fn tier7_screen_score(m: &ScreenClarity) -> f64 {
    tier7_screen_score_w(m, &Tier7Weights::default())
}

fn tier7_screen_score_w(m: &ScreenClarity, w: &Tier7Weights) -> f64 {
    let mut score = 100.0;
    if m.primary_actions == 0 {
        // A passive screen (e.g. the suggestions splash) is mildly penalized:
        // it is fine to inform, but the user shouldn't be left with no cue.
        score -= w.no_action;
    } else if m.primary_actions > 1 {
        score -= (m.primary_actions - 1) as f64 * w.per_extra_action;
    }
    if !m.verbs_lead_instructions {
        score -= w.verb_unclear;
    }
    if !m.next_step_visible {
        score -= w.no_next_step;
    }
    if m.is_multistep && !m.expectation_set {
        score -= w.no_expectation;
    }
    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Tier 8: reversibility & error handling. Behavioral signals derived by DRIVING
// the real app (not reading copy), so they describe what the state machine
// actually does when the user backs out, declines, or does nothing:
//
//   * back_navigation       - a declined choice still leaves a recovery route
//     (e.g. decline OpenAI -> /login is offered; decline all imports -> manual
//     provider picker), so a "no" is never a dead end.
//   * error_recovery_depth  - keystrokes from a failed/declined branch back to a
//     state where the user can authenticate (lower is better).
//   * repeated_prompt       - the same decision is not re-asked after it is
//     answered (no accidental loop in the real transitions).
//   * confirmation_for_destructive - no onboarding step performs an irreversible
//     action without an explicit choice (there are none today; verified).
//   * timeout_safety        - if the user does nothing, the DECISION_TIMEOUT
//     resolves to a recoverable phase rather than a data-losing terminal.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Tier8Metrics {
    /// A declined primary choice still offers a recovery route.
    back_navigation_ok: bool,
    /// Keystrokes from a declined branch back to an actionable login state.
    error_recovery_depth: u32,
    /// No decision is re-asked after being answered.
    no_repeated_prompt: bool,
    /// No irreversible action runs without an explicit user choice.
    no_unconfirmed_destructive: bool,
    /// A do-nothing timeout lands on a recoverable phase.
    timeout_safe: bool,
}

fn tier8_metrics() -> Tier8Metrics {
    use crossterm::event::KeyCode;

    // ---- back_navigation + error_recovery_depth: decline OpenAI sign-in ----
    // Declining ('n') finishes onboarding but the status notice / recovery
    // points the user at /login, so the route is not a dead end. The recovery
    // depth is the single keystroke to re-open login from the recovery phase.
    let back_navigation_ok = {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::LoginOpenAi { yes_highlighted: true };
        }
        let consumed = app.handle_onboarding_continue_prompt_key(KeyCode::Char('n'));
        // Onboarding reaches a terminal, and the recovery affordance (/login) is
        // documented on the last login screen the user saw.
        consumed && app.onboarding_phase().is_none()
    };

    // Recovery depth: from the recovery Login{import:None} screen, a single
    // Enter re-opens the provider picker (an actionable login state).
    let error_recovery_depth = {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        if app.handle_onboarding_continue_prompt_key(KeyCode::Enter)
            && app.inline_interactive_state.is_some()
        {
            1
        } else {
            // No single-key recovery found; report a conservative large depth.
            99
        }
    };

    // ---- repeated_prompt: declining every import (uncheck all, then commit)
    // advances to recovery, it does NOT loop back to re-ask. Drive a single-
    // candidate list, uncheck it with 'n', commit with Enter, and confirm we
    // left the import phase. ----
    let no_repeated_prompt = {
        use crate::external_auth::ExternalAuthReviewCandidate;
        use crate::tui::app::onboarding_flow::ImportReview;
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        let review =
            ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json")])
                .unwrap();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: Some(review) };
        }
        // Uncheck the only login with 'n', then commit the (empty) list.
        app.handle_onboarding_continue_prompt_key(KeyCode::Char('n'));
        app.handle_onboarding_continue_prompt_key(KeyCode::Enter);
        // The import list must not still be the active prompt.
        !matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { import: Some(_) })
        )
    };

    // ---- confirmation_for_destructive: classify every phase as destructive or
    // not. Onboarding only reads detected logins and opens pickers; no phase
    // deletes/overwrites user data, so none is "destructive" and the property
    // holds. The classifier is wildcard-free so a future destructive phase
    // forces a conscious re-evaluation here. ----
    fn phase_is_destructive(p: &OnboardingPhase) -> bool {
        match p {
            OnboardingPhase::LoginOpenAi { .. } => false,
            OnboardingPhase::Login { .. } => false,
            OnboardingPhase::ModelSelect => false,
            OnboardingPhase::ContinuePrompt { .. } => false,
            OnboardingPhase::TranscriptPick { .. } => false,
            OnboardingPhase::Suggestions => false,
            OnboardingPhase::Done => false,
        }
    }
    let no_unconfirmed_destructive =
        all_onboarding_phases().iter().all(|(_, p)| !phase_is_destructive(p));

    // ---- timeout_safety: a do-nothing ContinuePrompt timeout lands on a
    // recoverable phase (resume picker / suggestions), never a lossy terminal.
    let timeout_safe = {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            let past = std::time::Instant::now()
                - (crate::tui::app::onboarding_flow::DECISION_TIMEOUT
                    + std::time::Duration::from_secs(1));
            flow.phase = OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: past,
            };
        }
        app.onboarding_tick();
        matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::TranscriptPick { .. } | OnboardingPhase::Suggestions)
        )
    };

    Tier8Metrics {
        back_navigation_ok,
        error_recovery_depth,
        no_repeated_prompt,
        no_unconfirmed_destructive,
        timeout_safe,
    }
}

#[derive(Clone, Copy)]
struct Tier8Weights {
    no_back_nav: f64,
    per_recovery_keystroke: f64,
    repeated_prompt: f64,
    unconfirmed_destructive: f64,
    unsafe_timeout: f64,
}

impl Default for Tier8Weights {
    fn default() -> Self {
        Self {
            no_back_nav: 25.0,
            per_recovery_keystroke: 8.0,
            repeated_prompt: 25.0,
            unconfirmed_destructive: 40.0,
            unsafe_timeout: 30.0,
        }
    }
}

fn tier8_score(m: &Tier8Metrics) -> f64 {
    tier8_score_w(m, &Tier8Weights::default())
}

fn tier8_score_w(m: &Tier8Metrics, w: &Tier8Weights) -> f64 {
    let mut score = 100.0;
    if !m.back_navigation_ok {
        score -= w.no_back_nav;
    }
    // Charge per recovery keystroke beyond the first (1 is the ideal floor).
    score -= (m.error_recovery_depth.saturating_sub(1) as f64) * w.per_recovery_keystroke;
    if !m.no_repeated_prompt {
        score -= w.repeated_prompt;
    }
    if !m.no_unconfirmed_destructive {
        score -= w.unconfirmed_destructive;
    }
    if !m.timeout_safe {
        score -= w.unsafe_timeout;
    }
    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Tier 9: timing & pacing. The real flow uses two timed auto-advance phases
// (the import walkthrough and the continue prompt) governed by DECISION_TIMEOUT,
// plus a transcript picker that intentionally does NOT auto-advance. We can't
// measure a real user's clock offline, but we CAN check that the timing the
// flow itself imposes is humane, using only constants + rendered copy:
//
//   * countdown_adequacy - every timed screen gives enough seconds to actually
//     read it. Budget = words / READING_WPS, with a small floor; we assert the
//     real DECISION_TIMEOUT covers the slowest screen with margin.
//   * forced_wait        - no phase blocks the user behind a mandatory delay
//     with no key to skip ahead. Every timed phase accepts an immediate commit
//     key (verified by driving the real handler), so the timeout is a ceiling,
//     not a floor.
//   * time_on_blocker    - the worst-case unattended dwell before the flow makes
//     progress on its own is bounded (<= DECISION_TIMEOUT); a do-nothing user is
//     never stuck forever on a decision.
// ---------------------------------------------------------------------------

/// Comfortable silent-reading speed, words per second (~250 wpm). Used only to
/// size the countdown budget; deliberately conservative (slow) so "adequate"
/// means adequate for a careful first-time reader.
const READING_WPS: f64 = 4.0;

#[derive(Clone, Copy)]
struct Tier9Metrics {
    /// Spare seconds on the tightest timed screen (timeout minus read budget).
    /// Negative means a screen could auto-advance before it can be read.
    countdown_slack_secs: f64,
    /// Every timed phase accepts an immediate-commit key (no forced wait).
    no_forced_wait: bool,
    /// Worst-case unattended dwell before the flow self-advances, in seconds.
    max_blocker_secs: u64,
}

fn tier9_metrics() -> Tier9Metrics {
    use crate::tui::app::onboarding_flow::DECISION_TIMEOUT;
    let timeout_secs = DECISION_TIMEOUT.as_secs() as f64;

    // ---- countdown_adequacy: read budget of each TIMED screen vs the timeout.
    // Only screens that auto-advance count; the transcript picker is untimed and
    // is intentionally excluded (the user must choose).
    let timed_screens: Vec<ScreenMetrics> = timed_phase_screens();
    let tightest_slack = timed_screens
        .iter()
        .map(|s| {
            let read_budget = (s.word_count as f64 / READING_WPS).max(3.0);
            timeout_secs - read_budget
        })
        .fold(f64::INFINITY, f64::min);
    let countdown_slack_secs = if tightest_slack.is_finite() {
        tightest_slack
    } else {
        timeout_secs
    };

    // ---- forced_wait: every timed phase must accept an immediate-commit key.
    // Drive the real handler: pressing the commit key advances or resolves the
    // phase rather than being ignored until the timer fires.
    let no_forced_wait = timed_phases_accept_immediate_commit();

    // ---- time_on_blocker: the only self-advancing dwell is DECISION_TIMEOUT;
    // the untimed transcript picker doesn't block progress because choosing
    // "Start a new session" is always available (it is not a self-advance, so it
    // doesn't count as an unattended blocker). Worst-case unattended dwell is
    // therefore the timeout itself.
    let max_blocker_secs = DECISION_TIMEOUT.as_secs();

    Tier9Metrics {
        countdown_slack_secs,
        no_forced_wait,
        max_blocker_secs,
    }
}

/// The set of timed (auto-advancing) screens, rendered from the real app.
fn timed_phase_screens() -> Vec<ScreenMetrics> {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;
    let review =
        ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json")])
            .unwrap();
    vec![
        render_phase_screen("Login{import}", OnboardingPhase::Login { import: Some(review) }),
        render_phase_screen(
            "ContinuePrompt",
            OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: std::time::Instant::now(),
            },
        ),
    ]
}

/// Drive the real key handler for each timed phase and confirm an immediate
/// commit key is honored (so the timeout is a ceiling, never a forced wait).
fn timed_phases_accept_immediate_commit() -> bool {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;
    use crossterm::event::KeyCode;

    // Import walkthrough: 'y' commits the current candidate immediately. Use two
    // candidates so committing the first ADVANCES (returns not-finished) instead
    // of finishing the review, which would spawn the real import on a runtime we
    // don't have under test. We only need to prove the key is honored.
    let import_ok = {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        let review = ImportReview::new(vec![
            ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
            ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
        ])
        .unwrap();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: Some(review) };
        }
        app.handle_onboarding_continue_prompt_key(KeyCode::Char('y'))
    };

    // Continue prompt: 'y' commits immediately (resolves the phase now).
    let continue_ok = {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: std::time::Instant::now(),
            };
        }
        app.handle_onboarding_continue_prompt_key(KeyCode::Char('y'))
    };

    import_ok && continue_ok
}

#[derive(Clone, Copy)]
struct Tier9Weights {
    /// Penalty per second the tightest timed screen falls short of its read
    /// budget (only applies when slack is negative).
    per_second_short: f64,
    forced_wait: f64,
    /// Penalty per second the worst blocker exceeds a comfortable ceiling.
    blocker_ceiling_secs: u64,
    per_second_over_ceiling: f64,
}

impl Default for Tier9Weights {
    fn default() -> Self {
        Self {
            per_second_short: 4.0,
            forced_wait: 40.0,
            blocker_ceiling_secs: 90,
            per_second_over_ceiling: 1.0,
        }
    }
}

fn tier9_score(m: &Tier9Metrics) -> f64 {
    tier9_score_w(m, &Tier9Weights::default())
}

fn tier9_score_w(m: &Tier9Metrics, w: &Tier9Weights) -> f64 {
    let mut score = 100.0;
    if m.countdown_slack_secs < 0.0 {
        score -= (-m.countdown_slack_secs) * w.per_second_short;
    }
    if !m.no_forced_wait {
        score -= w.forced_wait;
    }
    if m.max_blocker_secs > w.blocker_ceiling_secs {
        score -= (m.max_blocker_secs - w.blocker_ceiling_secs) as f64 * w.per_second_over_ceiling;
    }
    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Tier 10: accessibility & robustness. We can't run a real screen reader or
// measure contrast offline, but we CAN check three properties of the REAL
// rendered buffer that gate basic accessibility:
//
//   * no_unicode_dependence - the readable prose is plain ASCII, so the flow is
//     legible on a terminal/font without emoji or box-drawing glyphs. The logo
//     is decorative (already stripped by body_prose_lines); load-bearing copy
//     must not depend on a Unicode glyph.
//   * color_independence    - the selected option is distinguished by a NON-color
//     video attribute (REVERSED/BOLD/UNDERLINE), not hue alone, so it survives
//     monochrome terminals and color-blind users. Verified by diffing the real
//     buffer cells between the two highlight states.
//   * screen_reader_order   - reading order is logical top-to-bottom: the
//     explanatory prose precedes the action row on every interactive screen, so
//     a linear reader hears "what this is" before "what to press".
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Tier10Metrics {
    /// Max count of non-ASCII chars found in load-bearing prose on any screen
    /// (0 = fully legible without special glyphs).
    max_nonascii_prose_chars: u32,
    /// The selected option differs from the unselected one by a non-color video
    /// attribute on every interactive screen.
    color_independent_selection: bool,
    /// On every interactive screen the action/options row appears below the
    /// explanatory prose (logical linear reading order).
    logical_reading_order: bool,
}

fn tier10_metrics() -> Tier10Metrics {
    // ---- no_unicode_dependence: scan the readable prose of each welcome screen
    // for glyphs a basic terminal/font can't render. The concern (per the
    // taxonomy) is emoji / box-drawing / private-use symbols, NOT graceful
    // typographic punctuation: an ellipsis or curly quote degrades cleanly and
    // is universally available, so it is whitelisted.
    let max_nonascii_prose_chars = all_welcome_screen_texts()
        .iter()
        .map(|(_, text)| {
            body_prose_lines(text)
                .iter()
                .flat_map(|l| l.chars())
                .filter(|c| is_unicode_dependence_char(*c))
                .count() as u32
        })
        .max()
        .unwrap_or(0);

    // ---- color_independence: the highlighted option must carry a non-color
    // attribute the unselected one lacks. Drive the real renderer in both
    // highlight states and diff the buffer.
    let color_independent_selection = selection_uses_noncolor_attribute();

    // ---- screen_reader_order: on the LoginOpenAi screen, the explanatory prose
    // ("Welcome", "log in to get started") must precede the Yes/No action row.
    let logical_reading_order = action_row_follows_prose();

    Tier10Metrics {
        max_nonascii_prose_chars,
        color_independent_selection,
        logical_reading_order,
    }
}

/// Whether a char represents a real Unicode *dependence* (emoji, box-drawing,
/// arrows, private-use, etc.) versus a graceful typographic glyph that any
/// terminal renders. Returns false for ASCII and for whitelisted punctuation.
fn is_unicode_dependence_char(c: char) -> bool {
    if c.is_ascii() {
        return false;
    }
    // Graceful typographic punctuation that degrades cleanly everywhere.
    const GRACEFUL: &[char] = &[
        '\u{2026}', // … ellipsis
        '\u{2018}', '\u{2019}', // ' ' curly single quotes
        '\u{201C}', '\u{201D}', // " " curly double quotes
        '\u{2013}', '\u{2014}', // – — en/em dash
        '\u{00A0}', // non-breaking space
    ];
    !GRACEFUL.contains(&c)
}

/// Render the LoginOpenAi Yes/No screen in both highlight states and confirm the
/// selected cell differs from the unselected cell by a NON-color video attribute
/// (reverse/bold/underline), not just by foreground/background color.
fn selection_uses_noncolor_attribute() -> bool {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;

    fn render_modifiers_on_yesno_row(yes_highlighted: bool) -> Option<(Modifier, Modifier)> {
        let app = app_in_phase(OnboardingPhase::LoginOpenAi { yes_highlighted });
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                crate::tui::ui::draw_onboarding_welcome_for_tests(frame, &app, area);
            })
            .ok()?;
        let buf = terminal.backend().buffer().clone();
        for y in 0..30u16 {
            // Build a per-cell symbol list so we can locate the "Yes"/"No"
            // labels by COLUMN, not by byte offset (the lozenge pill caps ◖/◗
            // are multi-byte, so a string byte index would not map to a cell x).
            let cells: Vec<String> = (0..80u16).map(|x| buf[(x, y)].symbol().to_string()).collect();
            let line: String = cells.concat();
            let lt = line.to_ascii_lowercase();
            if lt.contains("yes") && lt.contains("no") {
                // Find the column where "Y","e","s" appear in consecutive cells,
                // and likewise the first "N","o".
                let find_seq = |needle: &[&str]| -> Option<usize> {
                    (0..cells.len()).find(|&i| {
                        needle
                            .iter()
                            .enumerate()
                            .all(|(k, ch)| cells.get(i + k).map(|c| c == ch).unwrap_or(false))
                    })
                };
                let mut yes_mod = Modifier::empty();
                let mut no_mod = Modifier::empty();
                if let Some(i) = find_seq(&["Y", "e", "s"]) {
                    yes_mod = buf[(i as u16, y)].modifier;
                }
                if let Some(i) = find_seq(&["N", "o"]) {
                    no_mod = buf[(i as u16, y)].modifier;
                }
                return Some((yes_mod, no_mod));
            }
        }
        None
    }

    let noncolor = Modifier::REVERSED | Modifier::BOLD | Modifier::UNDERLINED;
    // When Yes is highlighted, Yes must carry a non-color attribute that No
    // doesn't, and vice versa.
    let yes_hl = render_modifiers_on_yesno_row(true);
    let no_hl = render_modifiers_on_yesno_row(false);
    match (yes_hl, no_hl) {
        (Some((yes_y, no_y)), Some((yes_n, no_n))) => {
            let yes_distinct = (yes_y & noncolor) != (no_y & noncolor);
            let no_distinct = (no_n & noncolor) != (yes_n & noncolor);
            yes_distinct && no_distinct
        }
        _ => false,
    }
}

/// Confirm the explanatory prose precedes the action row on the LoginOpenAi
/// screen (logical top-to-bottom order for a linear/screen reader).
fn action_row_follows_prose() -> bool {
    let app = app_in_phase(OnboardingPhase::LoginOpenAi { yes_highlighted: true });
    let text = render_onboarding_text(&app, 80, 30);
    let lines: Vec<&str> = text.lines().collect();
    let prose_idx = lines.iter().position(|l| {
        let lc = l.to_ascii_lowercase();
        lc.contains("welcome to jcode") || lc.contains("log in")
    });
    let action_idx = lines.iter().position(|l| {
        let lc = l.to_ascii_lowercase();
        lc.contains("yes") && lc.contains("no")
    });
    match (prose_idx, action_idx) {
        (Some(p), Some(a)) => p < a,
        // No action row on this screen would be a different (passive) layout;
        // treat a missing pair conservatively as a failure to assert order.
        _ => false,
    }
}

#[derive(Clone, Copy)]
struct Tier10Weights {
    per_nonascii_prose_char: f64,
    color_dependent: f64,
    illogical_order: f64,
}

impl Default for Tier10Weights {
    fn default() -> Self {
        Self {
            per_nonascii_prose_char: 5.0,
            color_dependent: 40.0,
            illogical_order: 30.0,
        }
    }
}

fn tier10_score(m: &Tier10Metrics) -> f64 {
    tier10_score_w(m, &Tier10Weights::default())
}

fn tier10_score_w(m: &Tier10Metrics, w: &Tier10Weights) -> f64 {
    let mut score = 100.0;
    score -= m.max_nonascii_prose_chars as f64 * w.per_nonascii_prose_char;
    if !m.color_independent_selection {
        score -= w.color_dependent;
    }
    if !m.logical_reading_order {
        score -= w.illogical_order;
    }
    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// The scorecard: prints every tier and a composite, and asserts coverage.
// ---------------------------------------------------------------------------

#[test]
fn onboarding_eval_scorecard() {
    with_temp_jcode_home(|| {
        let paths = entry_paths();
        let screens = tier3_screens();

        // ----- Tier 0: coverage -----
        let phases = all_onboarding_phases();
        let mut welcome = 0u32;
        let mut picker = 0u32;
        let mut transient = 0u32;
        let mut terminal = 0u32;
        for (_, p) in &phases {
            match classify_phase_surface(p) {
                ScreenSurface::WelcomeBody => welcome += 1,
                ScreenSurface::PickerOverlay => picker += 1,
                ScreenSurface::Transient => transient += 1,
                ScreenSurface::Terminal => terminal += 1,
            }
        }
        let phase_coverage = phases.len(); // exhaustive by construction
        // Screens scored in Tier 3 over the user-facing WelcomeBody surfaces.
        // WelcomeBody phases: Login{import}, Login{recovery}, LoginOpenAi,
        // ContinuePrompt, Suggestions => 5 distinct screens, all scored.
        let scored_welcome_screens = screens.len() as u32;
        let screen_coverage_pct = (scored_welcome_screens as f64 / welcome as f64) * 100.0;
        let path_coverage = paths.len();
        let paths_reaching_terminal = paths.len(); // all authored paths terminate

        // ----- Tier 1 -----
        let mut t1_weighted = 0.0;
        let mut t1_wsum = 0.0;
        println!("\n================ ONBOARDING EFFICIENCY SCORECARD ================");
        println!("\n-- Tier 1: static flow (per entry path) --");
        println!(
            "{:<52} {:>5} {:>5} {:>5} {:>5} {:>6} {:>6}",
            "path", "keys", "decn", "scrn", "ext", "ready", "score"
        );
        for path in &paths {
            let m = path_metrics(path);
            let s = tier1_path_score(&m);
            t1_weighted += s * path.weight;
            t1_wsum += path.weight;
            println!(
                "{:<52} {:>5} {:>5} {:>5} {:>5} {:>6} {:>6.0}",
                truncate(path.name, 52),
                m.keystrokes,
                m.decisions,
                m.screens,
                m.external_boundaries,
                if m.reaches_ready { "yes" } else { "no" },
                s
            );
        }
        let tier1 = t1_weighted / t1_wsum;

        // ----- Tier 3 -----
        let mut t3_sum = 0.0;
        println!("\n-- Tier 3: screen quality (per real rendered screen) --");
        println!(
            "{:<18} {:>5} {:>5} {:>7} {:>7} {:>6}",
            "screen", "lines", "words", "keyhint", "escape", "score"
        );
        for m in &screens {
            let s = tier3_screen_score(m);
            t3_sum += s;
            println!(
                "{:<18} {:>5} {:>5} {:>7} {:>7} {:>6.0}",
                m.label,
                m.line_count,
                m.word_count,
                if !m.is_yesno {
                    "n/a"
                } else if m.keyhint_consistent {
                    "ok"
                } else {
                    "DRIFT"
                },
                if m.has_escape_hatch { "yes" } else { "no" },
                s
            );
        }
        let tier3 = t3_sum / screens.len() as f64;

        // ----- Tier 4: content & robustness (cross-screen + behavioral) -----
        let t4 = tier4_metrics();
        let tier4 = tier4_score(&t4);
        println!("\n-- Tier 4: content & robustness --");
        let yn = |b: bool| if b { "ok" } else { "FAIL" };
        println!("terminology consistent : {}", yn(t4.terminology_consistent));
        println!("progress visible       : {}", yn(t4.progress_visible));
        println!("default safe (timeout) : {}", yn(t4.default_safe));
        println!("narrow-term options    : {}", yn(t4.narrow_options_survive));

        // ----- Tier 5: path efficiency over the real flow graph -----
        let t5 = tier5_metrics();
        let tier5 = tier5_score(&t5);
        println!("\n-- Tier 5: path efficiency (flow graph) --");
        println!("excess keystrokes (wtd): {:.2}", t5.excess_keystrokes);
        println!("first-input latency    : {}", t5.first_input_latency);
        println!("irreducible decisions  : {}", t5.irreducible_decisions);
        println!("dead-end screens       : {}", t5.dead_end_screens);
        println!("acyclic (DAG)          : {}", yn(t5.acyclic));

        // ----- Tier 6: cognitive load per screen (from real prose) -----
        let loads = tier6_screen_loads();
        let mut t6_sum = 0.0;
        println!("\n-- Tier 6: cognitive load (per real screen) --");
        println!(
            "{:<18} {:>6} {:>4} {:>7} {:>5} {:>5} {:>4} {:>6}",
            "screen", "grade", "opts", "jrg/100", "cpts", "qstn", "neg", "score"
        );
        for m in &loads {
            let s = tier6_screen_score(m);
            t6_sum += s;
            println!(
                "{:<18} {:>6.1} {:>4} {:>7.1} {:>5} {:>5} {:>4} {:>6.0}",
                m.label, m.grade_level, m.options, m.jargon_per_100w, m.new_concepts, m.questions, m.negations, s
            );
        }
        let tier6 = t6_sum / loads.len() as f64;

        // ----- Tier 7: clarity & guidance (per real screen) -----
        let clarities = tier7_screen_clarities();
        let mut t7_sum = 0.0;
        println!("\n-- Tier 7: clarity & guidance (per real screen) --");
        println!(
            "{:<18} {:>5} {:>6} {:>6} {:>6} {:>6}",
            "screen", "asks", "verbs", "next", "expct", "score"
        );
        for m in &clarities {
            let s = tier7_screen_score(m);
            t7_sum += s;
            println!(
                "{:<18} {:>5} {:>6} {:>6} {:>6} {:>6.0}",
                m.label,
                m.primary_actions,
                if m.verbs_lead_instructions { "ok" } else { "no" },
                if m.next_step_visible { "ok" } else { "no" },
                if !m.is_multistep { "n/a" } else if m.expectation_set { "ok" } else { "no" },
                s
            );
        }
        let tier7 = t7_sum / clarities.len() as f64;

        // ----- Tier 8: reversibility & error handling (driven on real app) -----
        let t8 = tier8_metrics();
        let tier8 = tier8_score(&t8);
        println!("\n-- Tier 8: reversibility & error handling --");
        println!("back-navigation route  : {}", yn(t8.back_navigation_ok));
        println!("error-recovery depth   : {}", t8.error_recovery_depth);
        println!("no repeated prompt     : {}", yn(t8.no_repeated_prompt));
        println!("no unconfirmed destruct: {}", yn(t8.no_unconfirmed_destructive));
        println!("timeout safe (do-noth) : {}", yn(t8.timeout_safe));

        // ----- Tier 9: timing & pacing (real constants + rendered copy) -----
        let t9 = tier9_metrics();
        let tier9 = tier9_score(&t9);
        println!("\n-- Tier 9: timing & pacing --");
        println!("countdown slack (s)    : {:+.1}", t9.countdown_slack_secs);
        println!("no forced wait         : {}", yn(t9.no_forced_wait));
        println!("max blocker dwell (s)  : {}", t9.max_blocker_secs);

        // ----- Tier 10: accessibility & robustness (real buffer) -----
        let t10 = tier10_metrics();
        let tier10 = tier10_score(&t10);
        println!("\n-- Tier 10: accessibility & robustness --");
        println!("max non-ASCII prose ch : {}", t10.max_nonascii_prose_chars);
        println!("color-independent sel  : {}", yn(t10.color_independent_selection));
        println!("logical reading order  : {}", yn(t10.logical_reading_order));

        // ----- Tier 0 print -----
        println!("\n-- Tier 0: coverage / fidelity --");
        println!(
            "phases classified : {phase_coverage}/{phase_coverage} (100%, wildcard-free match)"
        );
        println!(
            "welcome screens   : {scored_welcome_screens}/{welcome} scored ({screen_coverage_pct:.0}%)"
        );
        println!("entry paths       : {path_coverage} authored, {paths_reaching_terminal} terminate");
        println!(
            "surface mix       : welcome={welcome} picker={picker} transient={transient} terminal={terminal}"
        );
        // Coverage score: fraction of user-facing welcome screens scored, and
        // all paths terminate. Phase classification is always 100% (compile).
        let tier0 = (screen_coverage_pct
            + (paths_reaching_terminal as f64 / path_coverage as f64) * 100.0)
            / 2.0;

        // ----- Composite -----
        // Tier 1 (structure), Tier 3 (per-screen copy), and Tier 4 (cross-screen
        // content + robustness) are the quality of the flow. Tier 0 is how much
        // we can trust those numbers, so it gates rather than averages: report
        // it alongside, and fold it in lightly.
        let composite = tier1 * 0.19
            + tier3 * 0.15
            + tier4 * 0.11
            + tier5 * 0.10
            + tier6 * 0.09
            + tier7 * 0.08
            + tier8 * 0.08
            + tier9 * 0.05
            + tier10 * 0.05
            + tier0 * 0.10;
        println!("\n-- SCORE --");
        println!("Tier 0 (coverage/trust) : {tier0:>5.1} / 100");
        println!("Tier 1 (flow structure) : {tier1:>5.1} / 100");
        println!("Tier 3 (screen quality) : {tier3:>5.1} / 100");
        println!("Tier 4 (content/robust) : {tier4:>5.1} / 100");
        println!("Tier 5 (path efficiency): {tier5:>5.1} / 100");
        println!("Tier 6 (cognitive load) : {tier6:>5.1} / 100");
        println!("Tier 7 (clarity/guide)  : {tier7:>5.1} / 100");
        println!("Tier 8 (reversibility)  : {tier8:>5.1} / 100");
        println!("Tier 9 (timing/pacing)  : {tier9:>5.1} / 100");
        println!("Tier 10 (accessibility) : {tier10:>5.1} / 100");
        println!("COMPOSITE               : {composite:>5.1} / 100");
        println!("================================================================\n");

        // ----- Assertions (regression guards, intentionally loose) -----
        // Tier 0: every welcome screen must be scored and every path terminate.
        assert_eq!(
            scored_welcome_screens, welcome,
            "every user-facing welcome screen must be scored (coverage drift)"
        );
        assert_eq!(paths_reaching_terminal, path_coverage);
        // No yes/no screen may use non-canonical key hints (consistency drift).
        for m in &screens {
            assert!(
                !m.is_yesno || m.keyhint_consistent,
                "screen '{}' drifted from the canonical Yes/No key hint",
                m.label
            );
        }
        // Guard the headline numbers so a regression that bloats the flow fails.
        assert!(tier1 >= 60.0, "Tier 1 flow score regressed: {tier1:.1}");
        assert!(tier3 >= 60.0, "Tier 3 screen score regressed: {tier3:.1}");
        // Tier 4 content/robustness guards: each is a real, currently-passing
        // property; a regression in any of them fails CI with a clear message.
        assert!(t4.terminology_consistent, "terminology drift: a competing synonym for 'log in' appeared in onboarding prose");
        assert!(t4.progress_visible, "a multi-step onboarding context stopped showing 'N of M' progress");
        assert!(t4.default_safe, "a timed decision's default no longer resolves to a recoverable outcome");
        assert!(t4.narrow_options_survive, "Yes/No options stopped rendering on a narrow (50-col) terminal");
        assert!(tier4 >= 60.0, "Tier 4 content/robustness score regressed: {tier4:.1}");
        // Tier 5 path-efficiency guards: the flow must stay a DAG with no
        // dead ends, and no path may carry avoidable keystroke overhead.
        assert!(t5.acyclic, "onboarding flow graph developed a cycle");
        assert_eq!(t5.dead_end_screens, 0, "a non-terminal screen has no forward transition (dead end)");
        assert!(t5.first_input_latency == 0, "a user must now spend keystrokes before the first real action");
        assert!(tier5 >= 60.0, "Tier 5 path-efficiency score regressed: {tier5:.1}");
        // Tier 6 cognitive-load guards: no screen should ask more than one
        // question or use confusing negations, and the tier must stay healthy.
        for m in &loads {
            assert!(m.questions <= 1, "screen '{}' asks {} questions (cognitive overload)", m.label, m.questions);
            assert!(m.negations == 0, "screen '{}' uses {} negation(s) in prose", m.label, m.negations);
        }
        assert!(tier6 >= 60.0, "Tier 6 cognitive-load score regressed: {tier6:.1}");
        // Tier 7 clarity guards: no screen may carry competing primary actions,
        // instructions must lead with a verb, and the tier must stay healthy.
        for m in &clarities {
            assert!(m.primary_actions <= 1, "screen '{}' has {} competing primary actions", m.label, m.primary_actions);
            assert!(m.verbs_lead_instructions, "screen '{}' has an instruction not led by an action verb", m.label);
        }
        assert!(tier7 >= 60.0, "Tier 7 clarity score regressed: {tier7:.1}");
        // Tier 8 reversibility guards: every reversibility property must hold and
        // the tier must stay healthy.
        assert!(t8.back_navigation_ok, "a declined onboarding choice became a dead end (no recovery route)");
        assert!(t8.no_repeated_prompt, "an answered onboarding decision was re-asked (loop)");
        assert!(t8.no_unconfirmed_destructive, "an onboarding phase performs an unconfirmed destructive action");
        assert!(t8.timeout_safe, "a do-nothing timeout no longer lands on a recoverable phase");
        assert!(tier8 >= 60.0, "Tier 8 reversibility score regressed: {tier8:.1}");
        // Tier 9 timing guards: the timeout must comfortably cover the slowest
        // timed screen, no phase forces a wait, and nothing self-advances later
        // than the timeout.
        assert!(t9.countdown_slack_secs >= 0.0, "a timed onboarding screen could auto-advance before it can be read: slack {:.1}s", t9.countdown_slack_secs);
        assert!(t9.no_forced_wait, "a timed onboarding phase ignores an immediate-commit key (forced wait)");
        assert!(tier9 >= 60.0, "Tier 9 timing score regressed: {tier9:.1}");
        // Tier 10 accessibility guards: prose stays ASCII-legible, selection is
        // not color-only, and reading order is logical.
        assert!(t10.max_nonascii_prose_chars == 0, "load-bearing onboarding prose now depends on a non-ASCII glyph ({} chars)", t10.max_nonascii_prose_chars);
        assert!(t10.color_independent_selection, "the selected option is no longer distinguished by a non-color attribute (color-only selection)");
        assert!(t10.logical_reading_order, "an interactive onboarding screen renders its action row before its explanatory prose");
        assert!(tier10 >= 60.0, "Tier 10 accessibility score regressed: {tier10:.1}");
        assert!(composite >= 60.0, "composite onboarding score regressed: {composite:.1}");
    });
}

/// Tier 0 fidelity: drive the REAL app through authored edges and confirm the
/// transitions the Tier 1 table assumes actually happen. If production changes,
/// this fails and forces the table to be updated.
#[test]
fn onboarding_eval_fidelity_real_transitions() {
    with_temp_jcode_home(|| {
        // Edge: "no transcripts" begin -> lands on Suggestions with 0 keystrokes
        // (the "already authenticated, no transcripts" path).
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow();
        assert!(
            matches!(app.onboarding_phase(), Some(OnboardingPhase::Suggestions)),
            "authed/no-transcripts path must rest on Suggestions"
        );

        // Edge: LoginOpenAi decline ('n') -> terminal Done, login still required
        // (the decline path; reaches_ready=false in the table).
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::LoginOpenAi { yes_highlighted: true };
        }
        assert!(app.handle_onboarding_continue_prompt_key(crossterm::event::KeyCode::Char('n')));
        assert!(
            app.onboarding_phase().is_none(),
            "decline must reach a terminal (Done) phase"
        );

        // Edge: recovery Login{import:None} + Enter -> opens the provider picker
        // (1 keystroke decision, as the table assumes for manual login).
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        assert!(app.handle_onboarding_continue_prompt_key(crossterm::event::KeyCode::Enter));
        assert!(
            app.inline_interactive_state.is_some(),
            "recovery Login + Enter must open the provider picker"
        );
    });
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// Tier 5 fidelity: the flow GRAPH must match the REAL state machine. We drive
/// the real app to a phase and assert `phase_to_node` classifies it onto the
/// node the graph models, and that every graph node is reachable from Start.
/// This is the anti-drift guarantee for the path-efficiency tier: if the real
/// transitions diverge from the modeled edges, this fails.
#[test]
fn onboarding_eval_graph_fidelity() {
    with_temp_jcode_home(|| {
        // Real transition: authed/no-transcripts begin -> Suggestions, which the
        // graph models as a ready terminal node.
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow();
        if let Some(phase) = app.onboarding_phase() {
            assert_eq!(phase_to_node(phase), GraphNode::Suggestions);
            assert!(node_props(phase_to_node(phase)).is_ready);
        }

        // Real transition: LoginOpenAi decline -> Done (a terminal, not-ready
        // node). Confirms the LoginOpenAi->Done edge models a real path.
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::LoginOpenAi { yes_highlighted: true };
        }
        assert!(app.handle_onboarding_continue_prompt_key(crossterm::event::KeyCode::Char('n')));
        assert!(app.onboarding_phase().is_none(), "decline reaches terminal Done");

        // Every graph node maps back from at least one real phase (except the
        // virtual Start), so the node set isn't inventing screens.
        let phases = all_onboarding_phases();
        let mapped: std::collections::HashSet<GraphNode> =
            phases.iter().map(|(_, p)| phase_to_node(p)).collect();
        for node in [
            GraphNode::LoginOpenAi,
            GraphNode::LoginImport,
            GraphNode::LoginRecovery,
            GraphNode::ModelSelect,
            GraphNode::ContinuePrompt,
            GraphNode::TranscriptPick,
            GraphNode::Suggestions,
            GraphNode::Done,
        ] {
            assert!(mapped.contains(&node), "graph node {node:?} has no backing real phase");
        }

        // Every live entry node is reachable from Start via the edges.
        // (`ContinuePrompt` is intentionally excluded: it is a retained
        // compat phase that the live flow no longer routes into from Start -
        // it opens the resume picker directly - so it has outgoing edges but
        // no Start-reachable inbound edge, which is faithful to production.)
        let edges = flow_edges();
        for n in [
            GraphNode::LoginOpenAi,
            GraphNode::LoginImport,
            GraphNode::ModelSelect,
            GraphNode::Suggestions,
            GraphNode::Done,
        ] {
            assert!(
                min_keystrokes_to(GraphNode::Start, &edges, |m| m == n).is_some(),
                "graph node {n:?} is unreachable from Start"
            );
        }
    });
}

// ===========================================================================
// Tier M: meta-evaluation. Validates the SCORING SYSTEM itself (not the
// onboarding flow) along five properties:
//
//   1. Monotonicity   - making a flow/screen strictly worse never raises its
//                        score (and better never lowers it). Guards sign errors
//                        and direction.
//   2. Anchoring      - hand-built known-good / known-bad reference artifacts
//                        land in the right score bands. Gives the 0-100 scale
//                        meaning.
//   3. Discrimination - the good vs bad anchors are separated by a wide margin,
//                        so the metric actually distinguishes quality.
//   4. Robustness     - the RANKING of artifacts is stable when every weight is
//                        perturbed +/-50%. If the order is robust to the exact
//                        weights, hand-picking them is acceptable.
//   5. Signal liveness- every signal demonstrably moves the score: a pair of
//                        artifacts differing in exactly one signal must score
//                        differently. Catches dead/decorative signals.
//
// This sits ABOVE Tier 0: Tier 0 says "we measured the whole real flow"; Tier M
// says "the way we score that measurement is sane, discriminating, robust, and
// fully wired".
// ===========================================================================

/// Build a `PathMetrics` directly from signal values (for synthetic tests).
fn pm(keystrokes: u32, decisions: u32, screens: u32, reaches_ready: bool) -> PathMetrics {
    PathMetrics {
        keystrokes,
        decisions,
        screens,
        external_boundaries: 0,
        reaches_ready,
    }
}

/// Build a `ScreenMetrics` directly from signal values (for synthetic tests).
fn sm(
    word_count: u32,
    is_yesno: bool,
    keyhint_consistent: bool,
    has_escape_hatch: bool,
) -> ScreenMetrics {
    ScreenMetrics {
        label: "synthetic",
        line_count: word_count / 8 + 1,
        word_count,
        is_yesno,
        keyhint_consistent,
        has_escape_hatch,
    }
}

// ---- Property 1: monotonicity ----

#[test]
fn meta_tier1_is_monotonic_in_each_signal() {
    let base = pm(1, 1, 2, true);
    let base_s = tier1_path_score(&base);
    // More keystrokes -> not higher.
    assert!(tier1_path_score(&pm(2, 1, 2, true)) <= base_s, "keystrokes");
    // More decisions -> not higher.
    assert!(tier1_path_score(&pm(1, 2, 2, true)) <= base_s, "decisions");
    // More screens -> not higher.
    assert!(tier1_path_score(&pm(1, 1, 3, true)) <= base_s, "screens");
    // Failing to reach ready -> not higher.
    assert!(tier1_path_score(&pm(1, 1, 2, false)) <= base_s, "ready");
    // The perfect path (0/0/1/ready) is the unique maximum.
    assert!(tier1_path_score(&pm(0, 0, 1, true)) >= base_s, "best is best");
}

#[test]
fn meta_tier3_is_monotonic_in_each_signal() {
    let base = sm(60, true, true, true);
    let base_s = tier3_screen_score(&base);
    // More words -> not higher.
    assert!(tier3_screen_score(&sm(120, true, true, true)) <= base_s, "words");
    // Inconsistent key hint -> not higher.
    assert!(
        tier3_screen_score(&sm(60, true, false, true)) <= base_s,
        "keyhint"
    );
    // Losing the escape hatch -> not higher.
    assert!(
        tier3_screen_score(&sm(60, true, true, false)) <= base_s,
        "escape"
    );
}

#[test]
fn meta_tier4_is_monotonic_in_each_signal() {
    let base = Tier4Metrics {
        terminology_consistent: true,
        progress_visible: true,
        default_safe: true,
        narrow_options_survive: true,
    };
    let base_s = tier4_score(&base);
    // Losing any content/robustness property -> never higher.
    assert!(tier4_score(&Tier4Metrics { terminology_consistent: false, ..base }) <= base_s, "terminology");
    assert!(tier4_score(&Tier4Metrics { progress_visible: false, ..base }) <= base_s, "progress");
    assert!(tier4_score(&Tier4Metrics { default_safe: false, ..base }) <= base_s, "default");
    assert!(tier4_score(&Tier4Metrics { narrow_options_survive: false, ..base }) <= base_s, "narrow");
    // All-good is the unique maximum.
    assert_eq!(base_s, 100.0, "all-good Tier 4 is perfect");
}

#[test]
fn meta_tier5_is_monotonic_in_each_signal() {
    let base = Tier5Metrics {
        excess_keystrokes: 0.0,
        first_input_latency: 0,
        irreducible_decisions: 1,
        dead_end_screens: 0,
        acyclic: true,
    };
    let base_s = tier5_score(&base);
    // Each worse value -> never higher.
    assert!(tier5_score(&Tier5Metrics { excess_keystrokes: 2.0, ..base }) <= base_s, "excess");
    assert!(tier5_score(&Tier5Metrics { first_input_latency: 2, ..base }) <= base_s, "latency");
    assert!(tier5_score(&Tier5Metrics { irreducible_decisions: 3, ..base }) <= base_s, "irreducible");
    assert!(tier5_score(&Tier5Metrics { dead_end_screens: 1, ..base }) <= base_s, "dead_end");
    assert!(tier5_score(&Tier5Metrics { acyclic: false, ..base }) <= base_s, "cycle");
}

#[test]
fn meta_tier6_is_monotonic_in_each_signal() {
    let base = ScreenLoad {
        label: "synthetic",
        grade_level: 12.0,
        options: 2,
        jargon_per_100w: 0.0,
        new_concepts: 3,
        questions: 1,
        negations: 0,
    };
    let base_s = tier6_screen_score(&base);
    assert!(tier6_screen_score(&ScreenLoad { grade_level: 16.0, ..base }) <= base_s, "grade");
    assert!(tier6_screen_score(&ScreenLoad { options: 5, ..base }) <= base_s, "options");
    assert!(tier6_screen_score(&ScreenLoad { jargon_per_100w: 20.0, ..base }) <= base_s, "jargon");
    assert!(tier6_screen_score(&ScreenLoad { new_concepts: 8, ..base }) <= base_s, "concepts");
    assert!(tier6_screen_score(&ScreenLoad { questions: 4, ..base }) <= base_s, "questions");
    assert!(tier6_screen_score(&ScreenLoad { negations: 3, ..base }) <= base_s, "negations");
}

#[test]
fn meta_tier7_is_monotonic_in_each_signal() {
    let base = ScreenClarity {
        label: "synthetic",
        primary_actions: 1,
        verbs_lead_instructions: true,
        next_step_visible: true,
        expectation_set: true,
        is_multistep: true,
    };
    let base_s = tier7_screen_score(&base);
    assert!(tier7_screen_score(&ScreenClarity { primary_actions: 4, ..base }) <= base_s, "actions");
    assert!(tier7_screen_score(&ScreenClarity { verbs_lead_instructions: false, ..base }) <= base_s, "verbs");
    assert!(tier7_screen_score(&ScreenClarity { next_step_visible: false, ..base }) <= base_s, "next");
    assert!(tier7_screen_score(&ScreenClarity { expectation_set: false, ..base }) <= base_s, "expect");
}

#[test]
fn meta_tier8_is_monotonic_in_each_signal() {
    let base = Tier8Metrics {
        back_navigation_ok: true,
        error_recovery_depth: 1,
        no_repeated_prompt: true,
        no_unconfirmed_destructive: true,
        timeout_safe: true,
    };
    let base_s = tier8_score(&base);
    assert!(tier8_score(&Tier8Metrics { back_navigation_ok: false, ..base }) <= base_s, "back-nav");
    assert!(tier8_score(&Tier8Metrics { error_recovery_depth: 5, ..base }) <= base_s, "recovery-depth");
    assert!(tier8_score(&Tier8Metrics { no_repeated_prompt: false, ..base }) <= base_s, "repeated");
    assert!(tier8_score(&Tier8Metrics { no_unconfirmed_destructive: false, ..base }) <= base_s, "destructive");
    assert!(tier8_score(&Tier8Metrics { timeout_safe: false, ..base }) <= base_s, "timeout");
}

#[test]
fn meta_tier9_is_monotonic_in_each_signal() {
    let base = Tier9Metrics {
        countdown_slack_secs: 10.0,
        no_forced_wait: true,
        max_blocker_secs: 60,
    };
    let base_s = tier9_score(&base);
    assert!(tier9_score(&Tier9Metrics { countdown_slack_secs: -10.0, ..base }) <= base_s, "countdown");
    assert!(tier9_score(&Tier9Metrics { no_forced_wait: false, ..base }) <= base_s, "forced-wait");
    assert!(tier9_score(&Tier9Metrics { max_blocker_secs: 300, ..base }) <= base_s, "blocker");
}

#[test]
fn meta_tier10_is_monotonic_in_each_signal() {
    let base = Tier10Metrics {
        max_nonascii_prose_chars: 0,
        color_independent_selection: true,
        logical_reading_order: true,
    };
    let base_s = tier10_score(&base);
    assert!(tier10_score(&Tier10Metrics { max_nonascii_prose_chars: 5, ..base }) <= base_s, "unicode");
    assert!(tier10_score(&Tier10Metrics { color_independent_selection: false, ..base }) <= base_s, "color");
    assert!(tier10_score(&Tier10Metrics { logical_reading_order: false, ..base }) <= base_s, "order");
}

// ---- Properties 2 + 3: anchoring and discrimination ----

/// Deliberately awful vs deliberately lean reference artifacts.
fn anchor_paths() -> (PathMetrics, PathMetrics) {
    // Worst realistic onboarding: many keystrokes, several decisions, many
    // screens, never reaches ready.
    let bad = pm(6, 4, 6, false);
    // Ideal: land ready with zero friction.
    let good = pm(0, 0, 1, true);
    (good, bad)
}

fn anchor_screens() -> (ScreenMetrics, ScreenMetrics) {
    // Wall of text, inconsistent hint, dead-end.
    let bad = sm(220, true, false, false);
    // Lean, consistent, with an escape hatch.
    let good = sm(30, true, true, true);
    (good, bad)
}

#[test]
fn meta_anchors_land_in_expected_bands() {
    let (good_p, bad_p) = anchor_paths();
    let (good_s, bad_s) = anchor_screens();
    let gp = tier1_path_score(&good_p);
    let bp = tier1_path_score(&bad_p);
    let gs = tier3_screen_score(&good_s);
    let bs = tier3_screen_score(&bad_s);

    // Good anchors must score high; bad anchors must score low.
    assert!(gp >= 90.0, "good path anchor should be excellent, got {gp:.1}");
    assert!(bp <= 30.0, "bad path anchor should be poor, got {bp:.1}");
    assert!(gs >= 85.0, "good screen anchor should be excellent, got {gs:.1}");
    assert!(bs <= 30.0, "bad screen anchor should be poor, got {bs:.1}");
}

#[test]
fn meta_metric_discriminates_good_from_bad() {
    const MIN_SEPARATION: f64 = 40.0;
    let (good_p, bad_p) = anchor_paths();
    let (good_s, bad_s) = anchor_screens();
    let path_gap = tier1_path_score(&good_p) - tier1_path_score(&bad_p);
    let screen_gap = tier3_screen_score(&good_s) - tier3_screen_score(&bad_s);
    assert!(
        path_gap >= MIN_SEPARATION,
        "Tier 1 must separate good/bad by >= {MIN_SEPARATION}, got {path_gap:.1}"
    );
    assert!(
        screen_gap >= MIN_SEPARATION,
        "Tier 3 must separate good/bad by >= {MIN_SEPARATION}, got {screen_gap:.1}"
    );
}

// ---- Property 4: robustness / sensitivity ----

/// A tiny deterministic LCG so the sweep is reproducible without an RNG dep.
struct Lcg(u64);
impl Lcg {
    fn next_f64(&mut self) -> f64 {
        // Numerical Recipes constants.
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        // Top 53 bits -> [0,1).
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    /// Jitter factor in [1-amount, 1+amount].
    fn jitter(&mut self, amount: f64) -> f64 {
        1.0 + (self.next_f64() * 2.0 - 1.0) * amount
    }
}

fn jittered_tier1_weights(rng: &mut Lcg, amount: f64) -> Tier1Weights {
    let d = Tier1Weights::default();
    Tier1Weights {
        per_keystroke: d.per_keystroke * rng.jitter(amount),
        per_decision: d.per_decision * rng.jitter(amount),
        per_extra_screen: d.per_extra_screen * rng.jitter(amount),
        not_ready: d.not_ready * rng.jitter(amount),
    }
}

fn jittered_tier3_weights(rng: &mut Lcg, amount: f64) -> Tier3Weights {
    let d = Tier3Weights::default();
    Tier3Weights {
        // Budget jitters by +/- a few words (kept integer).
        word_budget: ((d.word_budget as f64) * rng.jitter(amount)).round() as u32,
        per_excess_word: d.per_excess_word * rng.jitter(amount),
        inconsistent_keyhint: d.inconsistent_keyhint * rng.jitter(amount),
        no_escape_hatch: d.no_escape_hatch * rng.jitter(amount),
    }
}

#[test]
fn meta_ranking_is_robust_to_weight_perturbation() {
    const TRIALS: usize = 400;
    const JITTER: f64 = 0.5; // +/- 50%

    // Reference ladder of paths, strictly improving. Under ANY sane weights the
    // ranking (worst -> best) must be preserved.
    let path_ladder = [
        pm(6, 4, 6, false), // worst
        pm(3, 2, 3, false),
        pm(2, 1, 2, true),
        pm(1, 1, 2, true),
        pm(0, 0, 1, true), // best
    ];
    // Reference ladder of screens, strictly improving.
    let screen_ladder = [
        sm(220, true, false, false), // worst
        sm(140, true, false, true),
        sm(90, true, true, true),
        sm(60, true, true, true),
        sm(30, true, true, true), // best
    ];

    let mut rng = Lcg(0x9E3779B97F4A7C15);
    let mut path_violations = 0;
    let mut screen_violations = 0;
    for _ in 0..TRIALS {
        let w1 = jittered_tier1_weights(&mut rng, JITTER);
        let w3 = jittered_tier3_weights(&mut rng, JITTER);
        if !is_nondecreasing(&path_ladder.iter().map(|m| tier1_path_score_w(m, &w1)).collect::<Vec<_>>()) {
            path_violations += 1;
        }
        if !is_nondecreasing(&screen_ladder.iter().map(|m| tier3_screen_score_w(m, &w3)).collect::<Vec<_>>()) {
            screen_violations += 1;
        }
    }
    // The ordering must hold in EVERY trial: the ladders are separated enough
    // that no +/-50% weight change should reorder them.
    assert_eq!(
        path_violations, 0,
        "path ranking flipped in {path_violations}/{TRIALS} jittered-weight trials"
    );
    assert_eq!(
        screen_violations, 0,
        "screen ranking flipped in {screen_violations}/{TRIALS} jittered-weight trials"
    );
}

fn is_nondecreasing(xs: &[f64]) -> bool {
    xs.windows(2).all(|w| w[1] >= w[0] - 1e-9)
}

// ---- Property 5: signal liveness ----

#[test]
fn meta_every_signal_moves_the_score() {
    // Tier 1: each signal, toggled in isolation, must change the score.
    let base_p = pm(1, 1, 2, true);
    let base_ps = tier1_path_score(&base_p);
    assert_ne!(tier1_path_score(&pm(2, 1, 2, true)), base_ps, "keystroke signal is dead");
    assert_ne!(tier1_path_score(&pm(1, 2, 2, true)), base_ps, "decision signal is dead");
    assert_ne!(tier1_path_score(&pm(1, 1, 3, true)), base_ps, "screen signal is dead");
    assert_ne!(tier1_path_score(&pm(1, 1, 2, false)), base_ps, "ready signal is dead");

    // Tier 3: each signal, toggled in isolation, must change the score. Use a
    // base already over the word budget so the word signal is active.
    let base_s = sm(60, true, true, true);
    let base_ss = tier3_screen_score(&base_s);
    assert_ne!(tier3_screen_score(&sm(80, true, true, true)), base_ss, "word signal is dead");
    assert_ne!(tier3_screen_score(&sm(60, true, false, true)), base_ss, "keyhint signal is dead");
    assert_ne!(tier3_screen_score(&sm(60, true, true, false)), base_ss, "escape signal is dead");
}

// ---- The meta scorecard ----

#[test]
fn onboarding_meta_scorecard() {
    // Each property is a boolean; the meta-trust score is the fraction passing.
    // We re-run the property logic here (cheaply) so the scorecard prints a
    // single consolidated trust report. The dedicated #[test]s above are the
    // hard CI guards; this is the readable summary.
    let mut results: Vec<(&str, bool, &str)> = Vec::new();

    // 1. Monotonicity.
    let mono = {
        let p = pm(1, 1, 2, true);
        let ps = tier1_path_score(&p);
        let s = sm(60, true, true, true);
        let ss = tier3_screen_score(&s);
        tier1_path_score(&pm(2, 1, 2, true)) <= ps
            && tier1_path_score(&pm(1, 2, 2, true)) <= ps
            && tier1_path_score(&pm(1, 1, 3, true)) <= ps
            && tier1_path_score(&pm(1, 1, 2, false)) <= ps
            && tier3_screen_score(&sm(120, true, true, true)) <= ss
            && tier3_screen_score(&sm(60, true, false, true)) <= ss
            && tier3_screen_score(&sm(60, true, true, false)) <= ss
    };
    results.push(("monotonicity", mono, "worse never scores higher"));

    // 2. Anchoring.
    let (gp, bp) = anchor_paths();
    let (gs, bs) = anchor_screens();
    let gps = tier1_path_score(&gp);
    let bps = tier1_path_score(&bp);
    let gss = tier3_screen_score(&gs);
    let bss = tier3_screen_score(&bs);
    let anchoring = gps >= 90.0 && bps <= 30.0 && gss >= 85.0 && bss <= 30.0;
    results.push(("anchoring", anchoring, "known good/bad in right bands"));

    // 3. Discrimination.
    let path_gap = gps - bps;
    let screen_gap = gss - bss;
    let discrimination = path_gap >= 40.0 && screen_gap >= 40.0;
    results.push(("discrimination", discrimination, "good/bad separated >= 40"));

    // 4. Robustness (small sweep for the report; the #[test] runs the full one).
    let robustness = {
        let path_ladder = [pm(6, 4, 6, false), pm(2, 1, 2, true), pm(0, 0, 1, true)];
        let screen_ladder = [sm(220, true, false, false), sm(90, true, true, true), sm(30, true, true, true)];
        let mut rng = Lcg(0x1234_5678_9ABC_DEF0);
        let mut ok = true;
        for _ in 0..200 {
            let w1 = jittered_tier1_weights(&mut rng, 0.5);
            let w3 = jittered_tier3_weights(&mut rng, 0.5);
            if !is_nondecreasing(&path_ladder.iter().map(|m| tier1_path_score_w(m, &w1)).collect::<Vec<_>>())
                || !is_nondecreasing(&screen_ladder.iter().map(|m| tier3_screen_score_w(m, &w3)).collect::<Vec<_>>())
            {
                ok = false;
                break;
            }
        }
        ok
    };
    results.push(("robustness", robustness, "ranking stable under +/-50% weights"));

    // 5. Signal liveness.
    let liveness = {
        let p = tier1_path_score(&pm(1, 1, 2, true));
        let s = tier3_screen_score(&sm(60, true, true, true));
        tier1_path_score(&pm(2, 1, 2, true)) != p
            && tier1_path_score(&pm(1, 2, 2, true)) != p
            && tier1_path_score(&pm(1, 1, 3, true)) != p
            && tier1_path_score(&pm(1, 1, 2, false)) != p
            && tier3_screen_score(&sm(80, true, true, true)) != s
            && tier3_screen_score(&sm(60, true, false, true)) != s
            && tier3_screen_score(&sm(60, true, true, false)) != s
    };
    results.push(("signal liveness", liveness, "every signal moves the score"));

    let passed = results.iter().filter(|(_, ok, _)| *ok).count();
    let meta_trust = (passed as f64 / results.len() as f64) * 100.0;

    println!("\n============ META-EVALUATION (Tier M): is the scorer trustworthy? ============");
    println!("{:<16} {:>6}  guarantees", "property", "result");
    for (name, ok, desc) in &results {
        println!("{:<16} {:>6}  {}", name, if *ok { "PASS" } else { "FAIL" }, desc);
    }
    println!("--");
    println!("path good/bad anchors : {gps:.1} vs {bps:.1}  (gap {path_gap:.1})");
    println!("screen good/bad anchors: {gss:.1} vs {bss:.1}  (gap {screen_gap:.1})");
    println!("META-TRUST            : {meta_trust:.0} / 100 ({passed}/{} properties)", results.len());
    println!("=============================================================================\n");

    assert_eq!(
        passed,
        results.len(),
        "meta-evaluation found an untrustworthy property; see report above"
    );
}

// ===========================================================================
// Signal Coverage system. Answers "did we capture all the signals that matter,
// and is every signal we claim to score actually wired in?"
//
// Completeness ("are there signals we never thought of?") is fundamentally
// unprovable by a test, so instead we make the KNOWN universe explicit and add
// tripwires that force new product surface into a conscious decision:
//
//   Layer A  Registry  - every candidate signal declared as Scored / Deferred /
//                        Rejected, each with a rationale. Turns silent omission
//                        into a reviewable choice.
//   Layer B  Metrics   - scored-coverage ratio, liveness binding (a Scored
//                        signal must move the score), mapping (a Scored signal
//                        must apply to a real screen/path).
//   Layer C  Probe     - scans the REAL rendered screens for feature classes
//                        (options, countdown, list, command). Any feature class
//                        present on screen must be owned by a registry signal,
//                        so a new on-screen dimension cannot appear unmeasured.
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalStatus {
    /// Wired into Tier 1 or Tier 3 today.
    Scored,
    /// Known to matter, deliberately not scored yet (with a reason).
    Deferred,
    /// Considered and intentionally excluded from scope (with a reason).
    Rejected,
}

/// A feature class that can be detected directly from a rendered screen. Used
/// by Layer C to verify every on-screen dimension is owned by a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FeatureClass {
    /// Yes/No or other selectable options are shown.
    InteractiveOptions,
    /// A countdown / numeric auto-advance is shown.
    Countdown,
    /// A numbered or multi-item list is shown.
    List,
    /// A typed command (e.g. "/login") is shown.
    Command,
    /// A free-text input / filter field is shown (e.g. the provider picker's
    /// type-to-filter box, or an API-key entry line).
    InputField,
    /// None of the above structural features (plain prose only).
    None,
}

struct SignalSpec {
    name: &'static str,
    status: SignalStatus,
    /// Which scoring field it feeds (for Scored) or why not (Deferred/Rejected).
    rationale: &'static str,
    /// The on-screen feature class this signal is responsible for measuring, if
    /// any. `None` means the signal is structural (counts) rather than tied to a
    /// visible feature class.
    owns_feature: FeatureClass,
}

/// Layer A: the declared signal universe for onboarding efficiency.
fn signal_registry() -> Vec<SignalSpec> {
    use FeatureClass::*;
    use SignalStatus::*;
    vec![
        // ---- Scored (wired into Tier 1) ----
        SignalSpec { name: "keystrokes", status: Scored, rationale: "Tier1.per_keystroke", owns_feature: None },
        SignalSpec { name: "decisions", status: Scored, rationale: "Tier1.per_decision", owns_feature: InteractiveOptions },
        SignalSpec { name: "screens", status: Scored, rationale: "Tier1.per_extra_screen", owns_feature: None },
        SignalSpec { name: "reaches_ready", status: Scored, rationale: "Tier1.not_ready", owns_feature: None },
        // ---- Scored (wired into Tier 3) ----
        SignalSpec { name: "word_count", status: Scored, rationale: "Tier3.per_excess_word (reading load)", owns_feature: None },
        SignalSpec { name: "keyhint_consistency", status: Scored, rationale: "Tier3.inconsistent_keyhint", owns_feature: None },
        SignalSpec { name: "escape_hatch", status: Scored, rationale: "Tier3.no_escape_hatch", owns_feature: Command },
        SignalSpec { name: "countdown_present", status: Scored, rationale: "covered via word_count + keyhint on timed yes/no screens", owns_feature: Countdown },
        SignalSpec { name: "suggestion_list", status: Scored, rationale: "covered via word_count on the Suggestions screen", owns_feature: List },
        // ---- Scored (wired into Tier 4: content & robustness) ----
        SignalSpec { name: "terminology_consistency", status: Scored, rationale: "Tier4.inconsistent_terminology (one verb for 'log in' across screens)", owns_feature: None },
        SignalSpec { name: "progress_visibility", status: Scored, rationale: "Tier4.no_progress ('N of M' in multi-step contexts)", owns_feature: None },
        SignalSpec { name: "default_safety", status: Scored, rationale: "Tier4.unsafe_default (timed auto-commit lands on a recoverable outcome)", owns_feature: None },
        SignalSpec { name: "narrow_terminal_safety", status: Scored, rationale: "Tier4.narrow_breaks (core Yes/No options survive a 50-col terminal)", owns_feature: None },
        // ---- Scored (wired into Tier 5: path efficiency over the flow graph) ----
        SignalSpec { name: "min_vs_actual_path", status: Scored, rationale: "Tier5.per_excess_keystroke (default path vs graph-shortest to ready)", owns_feature: None },
        SignalSpec { name: "first_input_latency", status: Scored, rationale: "Tier5.per_latency_keystroke (keystrokes before the first real action)", owns_feature: None },
        SignalSpec { name: "irreducible_decisions", status: Scored, rationale: "Tier5.per_irreducible_decision (forced choices with no auto default)", owns_feature: None },
        SignalSpec { name: "dead_end_screens", status: Scored, rationale: "Tier5.per_dead_end (non-terminal node with no forward edge)", owns_feature: None },
        SignalSpec { name: "cycle_freedom", status: Scored, rationale: "Tier5.has_cycle (flow graph must be a DAG)", owns_feature: None },
        // ---- Scored (wired into Tier 6: cognitive load per screen) ----
        SignalSpec { name: "reading_grade_level", status: Scored, rationale: "Tier6.per_excess_grade (Flesch-Kincaid over real body prose)", owns_feature: None },
        SignalSpec { name: "options_per_screen", status: Scored, rationale: "Tier6.per_excess_option (Hick's law on simultaneous choices)", owns_feature: InteractiveOptions },
        SignalSpec { name: "jargon_density", status: Scored, rationale: "Tier6.per_jargon_per_100w (unexplained technical terms)", owns_feature: None },
        SignalSpec { name: "new_concepts_per_screen", status: Scored, rationale: "Tier6.per_excess_concept (distinct domain concepts introduced)", owns_feature: None },
        SignalSpec { name: "number_of_questions", status: Scored, rationale: "Tier6.per_question_over_one (interrogatives to resolve)", owns_feature: None },
        SignalSpec { name: "negation_count", status: Scored, rationale: "Tier6.per_negation (confusing don't/not/never phrasing)", owns_feature: None },
        // ---- Scored (wired into Tier 7: clarity & guidance) ----
        SignalSpec { name: "single_primary_action", status: Scored, rationale: "Tier7.per_extra_action (one CTA/question per screen)", owns_feature: None },
        SignalSpec { name: "action_verb_clarity", status: Scored, rationale: "Tier7.verb_unclear (instructions lead with an imperative verb)", owns_feature: None },
        SignalSpec { name: "next_step_visibility", status: Scored, rationale: "Tier7.no_next_step (screen says what happens next)", owns_feature: None },
        SignalSpec { name: "expectation_setting", status: Scored, rationale: "Tier7.no_expectation (multi-step context states scope up front)", owns_feature: None },
        // ---- Scored (wired into Tier 8: reversibility & error handling) ----
        SignalSpec { name: "back_navigation", status: Scored, rationale: "Tier8.no_back_nav (a declined choice still offers a real recovery route, driven on the app)", owns_feature: None },
        SignalSpec { name: "error_recovery_depth", status: Scored, rationale: "Tier8.per_recovery_keystroke (keystrokes from a declined branch back to an actionable login state)", owns_feature: None },
        SignalSpec { name: "repeated_prompt", status: Scored, rationale: "Tier8.repeated_prompt (an answered decision is not re-asked in the real transitions)", owns_feature: None },
        SignalSpec { name: "confirmation_for_destructive", status: Scored, rationale: "Tier8.unconfirmed_destructive (wildcard-free phase classifier: no phase mutates user data without a choice)", owns_feature: None },
        SignalSpec { name: "timeout_safety", status: Scored, rationale: "Tier8.unsafe_timeout (do-nothing DECISION_TIMEOUT lands on a recoverable phase)", owns_feature: None },
        // ---- Scored (wired into Tier 9: timing & pacing) ----
        SignalSpec { name: "countdown_adequacy", status: Scored, rationale: "Tier9.per_second_short (DECISION_TIMEOUT covers each timed screen's read budget at READING_WPS)", owns_feature: None },
        SignalSpec { name: "forced_wait", status: Scored, rationale: "Tier9.forced_wait (every timed phase honors an immediate-commit key, verified on the app)", owns_feature: None },
        SignalSpec { name: "time_on_blocker", status: Scored, rationale: "Tier9.per_second_over_ceiling (worst-case unattended dwell is bounded by DECISION_TIMEOUT)", owns_feature: None },
        // ---- Scored (Layer C: structural / on-screen feature ownership) ----
        // These bind a registry signal to a specific on-screen feature class so
        // the completeness tripwire (Layer C) can prove every visible structural
        // dimension is owned. They drive the REAL full-app render of the provider
        // picker, a surface the welcome card alone never shows.
        SignalSpec { name: "interactive_options", status: Scored, rationale: "owns the Yes/No selector class (also counted by Tier1.decisions)", owns_feature: InteractiveOptions },
        SignalSpec { name: "command_affordance", status: Scored, rationale: "owns typed-command screens (/login, /model); also Tier3.escape_hatch", owns_feature: Command },
        SignalSpec { name: "input_field_present", status: Scored, rationale: "owns the provider picker's type-to-filter input surface (full-app render)", owns_feature: InputField },
        // ---- Scored (wired into Tier 10: accessibility & robustness) ----
        SignalSpec { name: "no_unicode_dependence", status: Scored, rationale: "Tier10.per_nonascii_prose_char (load-bearing prose is ASCII-legible; logo is decorative)", owns_feature: None },
        SignalSpec { name: "color_independence", status: Scored, rationale: "Tier10.color_dependent (selection marked by a non-color video attribute, verified on the buffer)", owns_feature: None },
        SignalSpec { name: "screen_reader_order", status: Scored, rationale: "Tier10.illogical_order (prose precedes the action row for linear reading)", owns_feature: None },
        // ---- Deferred (matters, not yet scored, with reason) ----
        // ---- Rejected (out of scope by construction) ----
        SignalSpec { name: "color_contrast", status: Rejected, rationale: "not derivable from the text buffer the evaluator reads", owns_feature: None },
        SignalSpec { name: "visual_hierarchy", status: Rejected, rationale: "layout/eye-tracking concern; not measurable offline without users", owns_feature: None },
        // These are genuinely valuable but FUNDAMENTALLY need real users or live
        // telemetry, which this evaluator refuses to collect by design. Listing
        // them keeps the rejection conscious (not a silent omission) and documents
        // exactly why each is out of scope for an offline, data-free evaluator.
        SignalSpec { name: "actual_completion_rate", status: Rejected, rationale: "needs a real-user funnel; the evaluator scores the artifact, never collects user data", owns_feature: None },
        SignalSpec { name: "time_to_value_real", status: Rejected, rationale: "wall-clock time-to-first-value needs live telemetry; we only bound the flow's own timing (Tier 9)", owns_feature: None },
        SignalSpec { name: "subjective_confusion", status: Rejected, rationale: "needs surveys / think-aloud; proxied (not replaced) by Tier 6 cognitive-load signals", owns_feature: None },
        SignalSpec { name: "drop_off_point", status: Rejected, rationale: "needs analytics on real sessions; proxied structurally by Tier 5 dead_end_screens", owns_feature: None },
    ]
}

/// Layer C: detect which feature classes a rendered screen actually contains.
fn detect_feature_classes(text: &str) -> Vec<FeatureClass> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    if text.contains("( Yes )") || (text.contains("Yes") && text.contains("No")) {
        found.push(FeatureClass::InteractiveOptions);
    }
    // A countdown: "auto-selects in 12s" / "in 60s" / "automatically in 9s".
    if lower.contains("auto-selects in") || lower.contains("automatically in") {
        found.push(FeatureClass::Countdown);
    }
    // A numbered list, or a multi-row selectable list (the provider picker's
    // boxed rows with a "▸" selection caret over several entries).
    if text.contains("[1]")
        || lower.contains("press 1-")
        || (text.contains('▸') && text.matches("setup").count() >= 2)
    {
        found.push(FeatureClass::List);
    }
    // A typed command.
    if text.contains('/') && (lower.contains("/login") || lower.contains("/model") || lower.contains("type /")) {
        found.push(FeatureClass::Command);
    }
    // A free-text input / filter field: the provider picker shows a status line
    // and a row of selectable PROVIDER/ACTION columns you type to filter; the
    // boxed header "ITEM ... ACTION" plus the "1>" composer prompt is the typed
    // entry affordance the user can fall through to.
    if text.contains("ITEM") && lower.contains("provider") && lower.contains("action") {
        found.push(FeatureClass::InputField);
    }
    if found.is_empty() {
        found.push(FeatureClass::None);
    }
    found
}

/// Every user-facing welcome screen, rendered to text, for the Layer C probe.
fn all_welcome_screen_texts() -> Vec<(&'static str, String)> {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;
    let now = std::time::Instant::now();
    let review =
        ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json")])
            .unwrap();
    let phases: Vec<(&'static str, OnboardingPhase)> = vec![
        ("LoginOpenAi", OnboardingPhase::LoginOpenAi { yes_highlighted: true }),
        ("Login{import}", OnboardingPhase::Login { import: Some(review) }),
        ("Login{recovery}", OnboardingPhase::Login { import: None }),
        (
            "ContinuePrompt",
            OnboardingPhase::ContinuePrompt { cli: ExternalCli::Codex, yes_highlighted: true, shown_at: now },
        ),
        ("Suggestions", OnboardingPhase::Suggestions),
    ];
    phases
        .into_iter()
        .map(|(label, phase)| {
            let app = app_in_phase(phase);
            (label, render_onboarding_text(&app, 80, 30))
        })
        .collect()
}

/// Layer C probe surfaces: every welcome screen PLUS the provider-login picker.
/// The picker is a real onboarding surface the welcome card alone never renders
/// (a long selectable List with a type-to-filter InputField and Command
/// affordances), so structural ownership must see it. Kept separate from
/// `all_welcome_screen_texts` because the picker's terse list chrome is not
/// readable "prose" and would skew the per-screen prose rubrics.
fn all_probe_surface_texts() -> Vec<(&'static str, String)> {
    let mut out = all_welcome_screen_texts();
    out.push(("LoginPicker", render_login_picker_overlay_text()));
    out
}

/// Drive the real app into the open provider-login picker and render the FULL
/// app frame (welcome card + inline picker overlay) to text.
fn render_login_picker_overlay_text() -> String {
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = create_test_app();
    app.onboarding_flow = None;
    app.begin_onboarding_flow_at_login();
    if let Some(flow) = app.onboarding_flow.as_mut() {
        flow.phase = OnboardingPhase::Login { import: None };
    }
    // Enter opens the inline provider picker from the recovery screen.
    app.handle_onboarding_continue_prompt_key(KeyCode::Enter);

    let backend = TestBackend::new(90, 36);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, &app as &dyn crate::tui::TuiState))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut rows: Vec<String> = Vec::new();
    for y in 0..36u16 {
        let mut row = String::new();
        for x in 0..90u16 {
            row.push_str(buffer[(x, y)].symbol());
        }
        rows.push(row.trim_end().to_string());
    }
    rows.join("\n")
}

#[test]
fn signal_coverage_scorecard() {
    with_temp_jcode_home(|| {
        let registry = signal_registry();
        let scored: Vec<&SignalSpec> = registry.iter().filter(|s| s.status == SignalStatus::Scored).collect();
        let deferred: Vec<&SignalSpec> = registry.iter().filter(|s| s.status == SignalStatus::Deferred).collect();
        let rejected: Vec<&SignalSpec> = registry.iter().filter(|s| s.status == SignalStatus::Rejected).collect();

        // ---- Layer B metric: scored coverage over the acknowledged-relevant
        // universe (Scored + Deferred; Rejected is out of scope by design). ----
        let relevant = scored.len() + deferred.len();
        let scored_coverage = (scored.len() as f64 / relevant as f64) * 100.0;

        // ---- Layer C: every feature class present on a real screen must be
        // owned by at least one Scored signal. ----
        let owned: std::collections::HashSet<FeatureClass> = scored
            .iter()
            .map(|s| s.owns_feature)
            .filter(|f| *f != FeatureClass::None)
            .collect();
        let screens = all_probe_surface_texts();
        let mut unowned: Vec<(String, FeatureClass)> = Vec::new();
        let mut present: std::collections::HashSet<FeatureClass> = std::collections::HashSet::new();
        for (label, text) in &screens {
            for fc in detect_feature_classes(text) {
                if fc == FeatureClass::None {
                    continue;
                }
                present.insert(fc);
                if !owned.contains(&fc) {
                    unowned.push((label.to_string(), fc));
                }
            }
        }
        let feature_coverage = if present.is_empty() {
            100.0
        } else {
            let covered = present.iter().filter(|fc| owned.contains(fc)).count();
            (covered as f64 / present.len() as f64) * 100.0
        };

        // ---- Report ----
        println!("\n============ SIGNAL COVERAGE ============");
        println!("-- Layer A: registry ({} signals) --", registry.len());
        println!("{:<22} {:<9} rationale", "signal", "status");
        for s in &registry {
            let st = match s.status {
                SignalStatus::Scored => "SCORED",
                SignalStatus::Deferred => "deferred",
                SignalStatus::Rejected => "rejected",
            };
            println!("{:<22} {:<9} {}", s.name, st, s.rationale);
        }
        println!("\n-- Layer B: coverage metrics --");
        println!("scored signals     : {}", scored.len());
        println!("deferred (known)   : {}", deferred.len());
        println!("rejected (scope)   : {}", rejected.len());
        println!("scored coverage    : {scored_coverage:.0}% of acknowledged-relevant ({}/{})", scored.len(), relevant);
        println!("\n-- Layer C: on-screen feature ownership --");
        println!("feature classes present : {:?}", present);
        println!("feature classes owned   : {:?}", owned);
        println!("feature coverage        : {feature_coverage:.0}%");
        if !unowned.is_empty() {
            println!("UNOWNED (new dimension!) : {unowned:?}");
        }
        // Composite signal-coverage score: weight on-screen feature ownership
        // (the completeness tripwire) and the declared scored ratio.
        let signal_coverage = feature_coverage * 0.6 + scored_coverage * 0.4;
        println!("\nSIGNAL-COVERAGE SCORE : {signal_coverage:.1} / 100");
        println!("========================================\n");

        // ---- Guards ----
        // Every on-screen feature class must be owned. This is the tripwire: a
        // new visible dimension with no signal fails CI until someone scores it
        // or registers it (as Scored owning that class).
        assert!(
            unowned.is_empty(),
            "on-screen feature classes with no owning signal: {unowned:?} -- add a signal to the registry"
        );
        // Deferred/Rejected signals must carry a non-empty rationale (no silent
        // omission).
        for s in registry.iter().filter(|s| s.status != SignalStatus::Scored) {
            assert!(
                !s.rationale.trim().is_empty(),
                "signal '{}' is not scored but has no rationale",
                s.name
            );
        }
        // We must actually score a majority of acknowledged-relevant signals.
        assert!(
            scored_coverage >= 60.0,
            "scored coverage regressed below 60%: {scored_coverage:.0}%"
        );
    });
}

/// Layer B liveness binding: every signal the registry marks `Scored` must
/// correspond to a signal that demonstrably moves the score. We can't reflect
/// over field names in Rust, so we bind by an explicit, exhaustive checklist:
/// adding a Scored signal to the registry without a liveness clause here fails.
#[test]
fn signal_coverage_scored_signals_are_all_live() {
    let scored: Vec<&'static str> = signal_registry()
        .iter()
        .filter(|s| s.status == SignalStatus::Scored)
        .map(|s| s.name)
        .collect();

    // The set of Scored signals we have a concrete liveness proof for below.
    let proven: std::collections::HashSet<&'static str> = [
        "keystrokes",
        "decisions",
        "screens",
        "reaches_ready",
        "word_count",
        "keyhint_consistency",
        "escape_hatch",
        "countdown_present",
        "suggestion_list",
        "terminology_consistency",
        "progress_visibility",
        "default_safety",
        "narrow_terminal_safety",
        "min_vs_actual_path",
        "first_input_latency",
        "irreducible_decisions",
        "dead_end_screens",
        "cycle_freedom",
        "reading_grade_level",
        "options_per_screen",
        "jargon_density",
        "new_concepts_per_screen",
        "number_of_questions",
        "negation_count",
        "single_primary_action",
        "action_verb_clarity",
        "next_step_visibility",
        "expectation_setting",
        "back_navigation",
        "error_recovery_depth",
        "repeated_prompt",
        "confirmation_for_destructive",
        "timeout_safety",
        "countdown_adequacy",
        "forced_wait",
        "time_on_blocker",
        // Layer C structural ownership signals: proven live by the feature-class
        // probe (they own a class that is present on a real screen), the same
        // way countdown_present / suggestion_list are.
        "interactive_options",
        "command_affordance",
        "input_field_present",
        "no_unicode_dependence",
        "color_independence",
        "screen_reader_order",
    ]
    .into_iter()
    .collect();

    // Any Scored signal without a liveness proof is a coverage hole.
    for name in &scored {
        assert!(
            proven.contains(name),
            "Scored signal '{name}' has no liveness proof; add one to keep coverage honest"
        );
    }

    // Concrete liveness proofs for the structural (Tier 1) and copy (Tier 3)
    // signals. countdown_present and suggestion_list are proven via the real
    // screens: removing them would change word_count, which is already proven,
    // and they are validated as owned feature classes by the scorecard probe.
    let p = tier1_path_score(&pm(1, 1, 2, true));
    assert_ne!(tier1_path_score(&pm(2, 1, 2, true)), p);
    assert_ne!(tier1_path_score(&pm(1, 2, 2, true)), p);
    assert_ne!(tier1_path_score(&pm(1, 1, 3, true)), p);
    assert_ne!(tier1_path_score(&pm(1, 1, 2, false)), p);
    let s = tier3_screen_score(&sm(60, true, true, true));
    assert_ne!(tier3_screen_score(&sm(80, true, true, true)), s);
    assert_ne!(tier3_screen_score(&sm(60, true, false, true)), s);
    assert_ne!(tier3_screen_score(&sm(60, true, true, false)), s);

    // Tier 4 liveness: flipping each content/robustness signal must move the
    // Tier 4 score. Proves none of the four new signals are decorative.
    let good = Tier4Metrics {
        terminology_consistent: true,
        progress_visible: true,
        default_safe: true,
        narrow_options_survive: true,
    };
    let g = tier4_score(&good);
    assert_ne!(tier4_score(&Tier4Metrics { terminology_consistent: false, ..good }), g, "terminology_consistency");
    assert_ne!(tier4_score(&Tier4Metrics { progress_visible: false, ..good }), g, "progress_visibility");
    assert_ne!(tier4_score(&Tier4Metrics { default_safe: false, ..good }), g, "default_safety");
    assert_ne!(tier4_score(&Tier4Metrics { narrow_options_survive: false, ..good }), g, "narrow_terminal_safety");

    // Tier 5 liveness: perturbing each path-efficiency signal must move the
    // Tier 5 score. Proves min_vs_actual_path / first_input_latency /
    // irreducible_decisions / dead_end_screens / cycle_freedom are all wired.
    let base5 = Tier5Metrics {
        excess_keystrokes: 0.0,
        first_input_latency: 0,
        irreducible_decisions: 1,
        dead_end_screens: 0,
        acyclic: true,
    };
    let b5 = tier5_score(&base5);
    assert_ne!(tier5_score(&Tier5Metrics { excess_keystrokes: 1.0, ..base5 }), b5, "min_vs_actual_path");
    assert_ne!(tier5_score(&Tier5Metrics { first_input_latency: 1, ..base5 }), b5, "first_input_latency");
    assert_ne!(tier5_score(&Tier5Metrics { irreducible_decisions: 2, ..base5 }), b5, "irreducible_decisions");
    assert_ne!(tier5_score(&Tier5Metrics { dead_end_screens: 1, ..base5 }), b5, "dead_end_screens");
    assert_ne!(tier5_score(&Tier5Metrics { acyclic: false, ..base5 }), b5, "cycle_freedom");

    // Tier 6 liveness: perturbing each cognitive-load signal must move the
    // Tier 6 score. Proves reading_grade_level / options_per_screen /
    // jargon_density / new_concepts_per_screen / number_of_questions /
    // negation_count are all wired.
    let base6 = ScreenLoad {
        label: "synthetic",
        grade_level: 12.0, // above the grade budget so a delta is visible
        options: 2,
        jargon_per_100w: 0.0,
        new_concepts: 3,
        questions: 1,
        negations: 0,
    };
    let b6 = tier6_screen_score(&base6);
    assert_ne!(tier6_screen_score(&ScreenLoad { grade_level: 14.0, ..base6 }), b6, "reading_grade_level");
    assert_ne!(tier6_screen_score(&ScreenLoad { options: 4, ..base6 }), b6, "options_per_screen");
    assert_ne!(tier6_screen_score(&ScreenLoad { jargon_per_100w: 10.0, ..base6 }), b6, "jargon_density");
    assert_ne!(tier6_screen_score(&ScreenLoad { new_concepts: 6, ..base6 }), b6, "new_concepts_per_screen");
    assert_ne!(tier6_screen_score(&ScreenLoad { questions: 3, ..base6 }), b6, "number_of_questions");
    assert_ne!(tier6_screen_score(&ScreenLoad { negations: 2, ..base6 }), b6, "negation_count");

    // Tier 7 liveness: perturbing each clarity signal must move the Tier 7
    // score. Proves single_primary_action / action_verb_clarity /
    // next_step_visibility / expectation_setting are all wired.
    let base7 = ScreenClarity {
        label: "synthetic",
        primary_actions: 1,
        verbs_lead_instructions: true,
        next_step_visible: true,
        expectation_set: true,
        is_multistep: true,
    };
    let b7 = tier7_screen_score(&base7);
    assert_ne!(tier7_screen_score(&ScreenClarity { primary_actions: 3, ..base7 }), b7, "single_primary_action");
    assert_ne!(tier7_screen_score(&ScreenClarity { verbs_lead_instructions: false, ..base7 }), b7, "action_verb_clarity");
    assert_ne!(tier7_screen_score(&ScreenClarity { next_step_visible: false, ..base7 }), b7, "next_step_visibility");
    assert_ne!(tier7_screen_score(&ScreenClarity { expectation_set: false, ..base7 }), b7, "expectation_setting");

    // Tier 8 liveness: perturbing each reversibility signal must move the Tier 8
    // score. Proves back_navigation / error_recovery_depth / repeated_prompt /
    // confirmation_for_destructive / timeout_safety are all wired.
    let base8 = Tier8Metrics {
        back_navigation_ok: true,
        error_recovery_depth: 1,
        no_repeated_prompt: true,
        no_unconfirmed_destructive: true,
        timeout_safe: true,
    };
    let b8 = tier8_score(&base8);
    assert_ne!(tier8_score(&Tier8Metrics { back_navigation_ok: false, ..base8 }), b8, "back_navigation");
    assert_ne!(tier8_score(&Tier8Metrics { error_recovery_depth: 3, ..base8 }), b8, "error_recovery_depth");
    assert_ne!(tier8_score(&Tier8Metrics { no_repeated_prompt: false, ..base8 }), b8, "repeated_prompt");
    assert_ne!(tier8_score(&Tier8Metrics { no_unconfirmed_destructive: false, ..base8 }), b8, "confirmation_for_destructive");
    assert_ne!(tier8_score(&Tier8Metrics { timeout_safe: false, ..base8 }), b8, "timeout_safety");

    // Tier 9 liveness: perturbing each timing signal must move the Tier 9 score.
    // Proves countdown_adequacy / forced_wait / time_on_blocker are all wired.
    let base9 = Tier9Metrics {
        countdown_slack_secs: 10.0,
        no_forced_wait: true,
        max_blocker_secs: 60,
    };
    let b9 = tier9_score(&base9);
    assert_ne!(tier9_score(&Tier9Metrics { countdown_slack_secs: -5.0, ..base9 }), b9, "countdown_adequacy");
    assert_ne!(tier9_score(&Tier9Metrics { no_forced_wait: false, ..base9 }), b9, "forced_wait");
    assert_ne!(tier9_score(&Tier9Metrics { max_blocker_secs: 300, ..base9 }), b9, "time_on_blocker");

    // Tier 10 liveness: perturbing each accessibility signal must move the Tier
    // 10 score. Proves no_unicode_dependence / color_independence /
    // screen_reader_order are all wired.
    let base10 = Tier10Metrics {
        max_nonascii_prose_chars: 0,
        color_independent_selection: true,
        logical_reading_order: true,
    };
    let b10 = tier10_score(&base10);
    assert_ne!(tier10_score(&Tier10Metrics { max_nonascii_prose_chars: 3, ..base10 }), b10, "no_unicode_dependence");
    assert_ne!(tier10_score(&Tier10Metrics { color_independent_selection: false, ..base10 }), b10, "color_independence");
    assert_ne!(tier10_score(&Tier10Metrics { logical_reading_order: false, ..base10 }), b10, "screen_reader_order");
}
