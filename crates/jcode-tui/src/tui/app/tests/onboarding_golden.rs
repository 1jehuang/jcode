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

    // 1. Login with no detected imports.
    {
        let app = app_in_phase(OnboardingPhase::Login { import: None });
        let text = render_onboarding_text(&app, width, height);
        dump("Login (no imports)", &text);
        assert!(text.contains("First, log in to get started."), "{text}");
        assert!(text.contains("Press Enter to choose a provider."), "{text}");
    }

    // 2. Login with detected imports (per-candidate review).
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
        dump("Login (import review, candidate 1/2)", &text);
        assert!(text.contains("We found 2 existing logins."), "count: {text}");
        assert!(text.contains("Login 1 of 2"), "position: {text}");
        assert!(text.contains("OpenAI/Codex"), "provider: {text}");
        assert!(text.contains("Codex auth.json"), "source: {text}");
        assert!(text.contains("Yes"), "yes option: {text}");
        assert!(text.contains("No"), "no option: {text}");
        assert!(
            text.contains("Left/right or h/l to move, Enter or Space to choose (y / n also work)."),
            "keys hint: {text}"
        );
        assert!(text.contains("Auto-selects in"), "countdown: {text}");
    }

    // 2b. Singular phrasing for a single detected login.
    {
        let review =
            ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("Cursor", "Cursor")])
                .unwrap();
        let app = app_in_phase(OnboardingPhase::Login {
            import: Some(review),
        });
        let text = render_onboarding_text(&app, width, height);
        dump("Login (import review, single login)", &text);
        assert!(
            text.contains("We found 1 existing login."),
            "singular count: {text}"
        );
        assert!(text.contains("Login 1 of 1"), "{text}");
    }

    // 3. Telemetry consent.
    {
        let app = app_in_phase(OnboardingPhase::TelemetryConsent {
            shown_at: std::time::Instant::now(),
            yes_highlighted: false,
        });
        let text = render_onboarding_text(&app, width, height);
        dump("TelemetryConsent", &text);
        assert!(text.contains("Help improve jcode?"), "{text}");
        assert!(
            text.contains("Share your prompts and transcripts so we can improve the product."),
            "{text}"
        );
        assert!(
            text.contains("This is optional and off by default. You can change it later."),
            "{text}"
        );
        assert!(text.contains("Declines automatically in"), "countdown: {text}");
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
            text.contains("Yes") && text.contains("No"),
            "continue prompt Yes/No selector: {text}"
        );
        assert!(
            text.contains("Left/right or h/l to move"),
            "continue prompt movement hint: {text}"
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
