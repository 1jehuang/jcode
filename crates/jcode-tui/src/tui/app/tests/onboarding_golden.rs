// Golden state-space walker for the first-run onboarding welcome screen.
//
// This renders every onboarding phase to an offscreen TestBackend and captures
// the exact text the user sees. It serves two purposes:
//
//   1. A reviewable, deterministic dump of every onboarding screen (run with
//      `--nocapture` to read them), so we can verify every word of copy without
//      manually walking the live flow.
//   2. Regression guards on the exact wording / option layout of each phase.
//
// To see all rendered screens:
//   cargo test -p jcode-tui onboarding_golden -- --nocapture

// NOTE: This file is `include!`d into `crate::tui::app::tests`, which already
// imports `ExternalCli`, `OnboardingFlow`, and `OnboardingPhase` via the
// sibling `onboarding_flow.rs` include. To avoid duplicate-import errors we
// reference types through fully-qualified paths / local aliases below instead
// of adding module-level `use` statements.

/// Render the onboarding welcome screen for `app` into a fixed-size buffer and
/// return the visible text, one line per row, trailing blank rows trimmed.
fn render_onboarding_text(app: &App, width: u16, height: u16) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            crate::tui::ui::draw_onboarding_welcome_for_tests(frame, app, area);
        })
        .unwrap();

    let buffer = terminal.backend().buffer().clone();
    let mut rows: Vec<String> = Vec::with_capacity(height as usize);
    for y in 0..height {
        let mut row = String::new();
        for x in 0..width {
            row.push_str(buffer[(x, y)].symbol());
        }
        rows.push(row.trim_end().to_string());
    }
    while rows.last().map(|r| r.is_empty()).unwrap_or(false) {
        rows.pop();
    }
    rows.join("\n")
}

/// Force the app into a specific onboarding phase, bypassing the on-disk
/// new-user heuristic.
fn app_in_phase(phase: OnboardingPhase) -> App {
    let mut app = create_test_app();
    let mut flow = OnboardingFlow::begin();
    flow.phase = phase;
    app.onboarding_flow = Some(flow);
    app
}

fn dump(title: &str, text: &str) {
    println!("\n========== {title} ==========");
    println!("{text}");
    println!("==========================================");
}

#[test]
fn onboarding_golden_walks_every_phase() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    let width = 80u16;
    let height = 30u16;

    // 1. No detected imports: "Log in to OpenAI?" Yes/No prompt.
    {
        let app = app_in_phase(OnboardingPhase::LoginOpenAi {
            yes_highlighted: true,
        });
        let text = render_onboarding_text(&app, width, height);
        dump("LoginOpenAi (no imports)", &text);
        // Lean prompt: just the question + the Yes/No lozenge pills. The Esc hint
        // already covers the "skip / log in later" path, so no extra prose.
        assert!(text.contains("Log in to OpenAI?"), "{text}");
        assert!(text.contains("Yes") && text.contains("No"), "{text}");
        assert!(
            text.contains("\u{25D6} Yes \u{25D7}") && text.contains("\u{25D6} No \u{25D7}"),
            "yes/no lozenge pills: {text}"
        );
        // The redundant "Choose No to skip" line was removed.
        assert!(
            !text.contains("Choose \"No\" to skip"),
            "redundant skip line should be gone: {text}"
        );
    }

    // 1b. Recovery fallback: bare Login phase with no import (import declined or
    // failed) points the user at the provider picker.
    {
        let app = app_in_phase(OnboardingPhase::Login { import: None });
        let text = render_onboarding_text(&app, width, height);
        dump("Login (no imports, recovery)", &text);
        assert!(text.contains("First, log in to get started."), "{text}");
        assert!(
            text.contains("Press Enter to pick who to log in with"),
            "{text}"
        );
    }

    // 2. Login with detected imports: the default SUMMARY screen. It lists
    // everything we detected read-only and lands focus on a preselected
    // import action, with a Jcode subscription alternative and secondary
    // import/telemetry controls beside it.
    {
        let review = ImportReview::new(vec![
            ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
            ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
        ])
        .unwrap();
        let app = app_in_phase(OnboardingPhase::Login {
            import: Some(review),
        });
        let text = render_onboarding_text(&app, width, height);
        dump("Login (import summary, 2 logins)", &text);
        // The headline states how many logins were found.
        assert!(
            text.contains("We found 2 existing logins:"),
            "summary headline: {text}"
        );
        // Every detected login is listed with a checkmark (read-only summary).
        assert!(text.contains("OpenAI/Codex"), "provider 1: {text}");
        assert!(text.contains("Codex auth.json"), "source 1: {text}");
        assert!(text.contains("Claude"), "provider 2: {text}");
        assert!(text.contains('✓'), "detected checkmark: {text}");
        // The primary actions explicitly offer import or a Jcode subscription.
        assert!(text.contains("Import"), "import pill label: {text}");
        assert!(
            text.contains("Jcode subscription"),
            "subscription pill label: {text}"
        );
        assert!(text.contains("Import less"), "import-less pill: {text}");
        assert!(text.contains("Telemetry"), "telemetry pill label: {text}");
        assert!(
            text.contains("$10 to $20 inference, $20 to $40; then provider API prices"),
            "subscription allowance and overage pricing: {text}"
        );
        assert!(
            text.contains("Scales through Solo"),
            "offer should apply through the Solo plan: {text}"
        );
        assert!(
            text.contains('\u{25D6}') && text.contains('\u{25D7}'),
            "pill rounded end caps: {text}"
        );
        // The summary is read-only: no per-row choice circles yet.
        assert!(!text.contains('●'), "no choice circles on summary: {text}");
    }

    // 2b. Choose mode: the per-login checkbox list (opened via the "Choose
    // what to import" pill) still renders the labeled two-column list.
    {
        let mut review = ImportReview::new(vec![
            ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
            ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
        ])
        .unwrap();
        review.enter_choose_mode();
        let app = app_in_phase(OnboardingPhase::Login {
            import: Some(review),
        });
        let text = render_onboarding_text(&app, width, height);
        dump("Login (import choose mode, 2 logins)", &text);
        // The section is labeled "Import:" (lean header; the list itself shows
        // how many and which logins were found).
        assert!(text.contains("Import:"), "import label: {text}");
        // Both logins are listed at once, each with a Yes/No choice.
        assert!(text.contains("OpenAI/Codex"), "provider 1: {text}");
        assert!(text.contains("Codex auth.json"), "source 1: {text}");
        assert!(text.contains("Claude"), "provider 2: {text}");
        // A Yes/No header sits above the per-login circle columns, with the
        // filled circle marking the current (pre-selected: Yes) choice.
        assert!(text.contains("Yes") && text.contains("No"), "yes/no header: {text}");
        assert!(text.contains('●'), "filled choice circle: {text}");
        assert!(text.contains('○'), "hollow choice circle: {text}");
        // A navigable "Continue" pill sits above the list (between the label and
        // the rows) so the user can reach the commit action by arrowing out of
        // the list. It is drawn as a real lozenge: half-circle end caps (◖ ◗)
        // around the label.
        assert!(text.contains("Continue"), "continue pill label: {text}");
        assert!(
            text.contains('\u{25D6}') && text.contains('\u{25D7}'),
            "continue pill rounded end caps: {text}"
        );
    }

    // 2c. A single detected login still renders the summary + one row.
    {
        let review =
            ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("Cursor", "Cursor")])
                .unwrap();
        let app = app_in_phase(OnboardingPhase::Login {
            import: Some(review),
        });
        let text = render_onboarding_text(&app, width, height);
        dump("Login (import summary, single login)", &text);
        assert!(
            text.contains("We found 1 existing login:"),
            "singular headline: {text}"
        );
        assert!(text.contains("Cursor"), "single login row: {text}");
        assert!(text.contains("Import"), "import pill: {text}");
        assert!(
            text.contains("Jcode subscription"),
            "subscription pill: {text}"
        );
    }

    // 4. Continue prompt (resume an external session).
    {
        let app = app_in_phase(OnboardingPhase::ContinuePrompt {
            cli: ExternalCli::Codex,
            yes_highlighted: true,
            shown_at: std::time::Instant::now(),
        });
        let text = render_onboarding_text(&app, width, height);
        dump("ContinuePrompt (Codex)", &text);
        assert!(
            text.contains("Continue where you left off in Codex?"),
            "continue prompt: {text}"
        );
        assert!(
            text.contains("\u{25D6} Yes \u{25D7}") && text.contains("\u{25D6} No \u{25D7}"),
            "continue prompt Yes/No lozenge pills: {text}"
        );
        assert!(
            text.contains("Opens the resume menu automatically in"),
            "resume-menu hint: {text}"
        );
    }

    // 5. Suggestions (resting state).
    {
        let app = app_in_phase(OnboardingPhase::Suggestions);
        let text = render_onboarding_text(&app, width, height);
        dump("Suggestions", &text);
        assert!(text.contains("Welcome to jcode onboarding"), "{text}");
    }
}

/// Comprehensive state-space walkthrough that also covers the async-wait and
/// failure screens the basic golden walk omits (the "Importing your logins..."
/// progress screen and the failure-aware recovery screen), and enforces polish
/// invariants on every guided screen:
///
///   * It always renders the welcome title + tagline (no blank/garbled card).
///   * Every guided screen advertises the universal Esc escape hatch, so the
///     user can always see a way out (the liveness guarantee, made visible).
///   * The failure screen states what went wrong AND the concrete next step.
///
/// Run with `--nocapture` to eyeball every screen, including the edge states.
#[test]
fn onboarding_golden_walks_failure_and_async_states() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    let width = 80u16;
    let height = 32u16;

    // Helper: assert the shared polish invariants for a guided screen.
    let assert_guided_polish = |title: &str, text: &str| {
        assert!(
            text.contains("Welcome to jcode onboarding"),
            "{title}: must render the welcome title\n{text}"
        );
        assert!(
            text.contains("Esc to skip onboarding"),
            "{title}: every guided screen must advertise the Esc escape hatch\n{text}"
        );
    };

    // (a) Import committed, async LoginCompleted not yet arrived: progress card.
    {
        let mut app = app_in_phase(OnboardingPhase::Login { import: None });
        app.onboarding_import_in_progress = Some(std::time::Instant::now());
        let text = render_onboarding_text(&app, width, height);
        dump("Login (importing in progress)", &text);
        assert!(
            text.contains("Importing your logins"),
            "progress headline: {text}"
        );
        assert!(
            text.contains("Hang tight"),
            "progress reassurance: {text}"
        );
        // The progress screen must NOT show the manual-login recovery copy.
        assert!(
            !text.contains("Press Enter to pick who to log in with"),
            "progress screen must not tell the user to log in again: {text}"
        );
        assert_guided_polish("Login (importing in progress)", &text);
    }

    // (b) Import failed: failure-aware recovery card with reason + next step.
    {
        let mut app = app_in_phase(OnboardingPhase::Login { import: None });
        app.onboarding_import_error =
            Some("the saved credential was rejected".to_string());
        let text = render_onboarding_text(&app, width, height);
        dump("Login (import failed, recovery)", &text);
        assert!(
            text.contains("We couldn't import those logins."),
            "failure headline: {text}"
        );
        assert!(
            text.contains("the saved credential was rejected"),
            "failure reason must be shown verbatim: {text}"
        );
        assert!(
            text.contains("you can log in directly"),
            "failure must offer a concrete recovery: {text}"
        );
        assert!(
            text.contains("Press Enter to choose a provider"),
            "failure must state the exact next key: {text}"
        );
        assert_guided_polish("Login (import failed, recovery)", &text);
    }

    // (c) The import list, recovery, OpenAI prompt, and continue prompt must all
    // advertise the Esc escape hatch (polish invariant across guided screens).
    {
        let review = ImportReview::new(vec![ExternalAuthReviewCandidate::fixture(
            "OpenAI/Codex",
            "Codex auth.json",
        )])
        .unwrap();
        let app = app_in_phase(OnboardingPhase::Login {
            import: Some(review),
        });
        let text = render_onboarding_text(&app, width, height);
        dump("Login (import list, Esc hint)", &text);
        assert_guided_polish("Login (import list)", &text);
    }
    {
        let app = app_in_phase(OnboardingPhase::LoginOpenAi {
            yes_highlighted: true,
        });
        let text = render_onboarding_text(&app, width, height);
        assert_guided_polish("LoginOpenAi", &text);
    }
    {
        let app = app_in_phase(OnboardingPhase::ContinuePrompt {
            cli: ExternalCli::Codex,
            yes_highlighted: true,
            shown_at: std::time::Instant::now(),
        });
        let text = render_onboarding_text(&app, width, height);
        assert_guided_polish("ContinuePrompt", &text);
    }
}

/// Golden render of the "Telemetry settings" sub-page reached from the import
/// summary. Three stacked options with dim consequence captions, defaulting to
/// "Send everything".
#[test]
fn onboarding_golden_telemetry_settings_page() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    let mut review = ImportReview::new(vec![ExternalAuthReviewCandidate::fixture(
        "OpenAI/Codex",
        "Codex auth.json",
    )])
    .unwrap();
    review.open_telemetry();
    let app = app_in_phase(OnboardingPhase::Login {
        import: Some(review),
    });
    let text = render_onboarding_text(&app, 80, 34);
    dump("Telemetry settings page", &text);
    assert!(text.contains("Telemetry settings"), "title: {text}");
    assert!(
        text.contains("Send everything, including prompts"),
        "option 1: {text}"
    );
    assert!(text.contains("Helps jcode the most"), "caption 1: {text}");
    assert!(
        text.contains("No prompts or transcripts"),
        "option 2: {text}"
    );
    assert!(text.contains("Send nothing"), "option 3: {text}");
    assert!(text.contains("/telemetry"), "later-change hint: {text}");
    // The import summary is hidden while the sub-page is open.
    assert!(
        !text.contains("We found 1 existing login"),
        "summary hidden: {text}"
    );
}
