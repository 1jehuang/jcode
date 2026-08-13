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

/// Render the onboarding screen and return its styled offscreen buffer.
fn render_onboarding_buffer(
    app: &App,
    width: u16,
    height: u16,
) -> ratatui::buffer::Buffer {
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
    terminal.backend().buffer().clone()
}

fn svg_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn svg_color(color: ratatui::style::Color, fallback: &str) -> String {
    use ratatui::style::Color;
    match color {
        Color::Reset => fallback.to_string(),
        Color::Black => "#101419".to_string(),
        Color::Red => "#e06c75".to_string(),
        Color::Green => "#98c379".to_string(),
        Color::Yellow => "#e5c07b".to_string(),
        Color::Blue => "#61afef".to_string(),
        Color::Magenta => "#c678dd".to_string(),
        Color::Cyan => "#56b6c2".to_string(),
        Color::Gray => "#abb2bf".to_string(),
        Color::DarkGray => "#5c6370".to_string(),
        Color::LightRed => "#ff7b86".to_string(),
        Color::LightGreen => "#b4e88a".to_string(),
        Color::LightYellow => "#f2d18b".to_string(),
        Color::LightBlue => "#7fc1ff".to_string(),
        Color::LightMagenta => "#dc8cf2".to_string(),
        Color::LightCyan => "#70d7e2".to_string(),
        Color::White => "#f2f4f8".to_string(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(index) => {
            // The onboarding palette uses named/RGB colors today. Keep indexed
            // colors deterministic if one is introduced later.
            let level = 48u8.saturating_add(index.saturating_mul(207) / 255);
            format!("#{level:02x}{level:02x}{level:02x}")
        }
    }
}

/// Serialize a ratatui TestBackend buffer as a standalone terminal-like SVG.
/// This keeps screenshot generation headless and, importantly, renders the
/// exact same state model and widget tree as the interactive onboarding flow.
fn onboarding_buffer_svg(buffer: &ratatui::buffer::Buffer) -> String {
    use ratatui::style::Modifier;
    use std::fmt::Write;

    const CELL_W: u32 = 12;
    const CELL_H: u32 = 24;
    const FONT_SIZE: u32 = 18;
    const DEFAULT_BG: &str = "#0b0f14";
    const DEFAULT_FG: &str = "#d8dee9";

    let area = buffer.area;
    let pixel_width = u32::from(area.width) * CELL_W;
    let pixel_height = u32::from(area.height) * CELL_H;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{pixel_width}" height="{pixel_height}" viewBox="0 0 {pixel_width} {pixel_height}"><rect width="100%" height="100%" fill="{DEFAULT_BG}"/><g font-family="JetBrains Mono, DejaVu Sans Mono, monospace" font-size="{FONT_SIZE}px">"#
    );

    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buffer[(x, y)];
            let bg = svg_color(cell.bg, DEFAULT_BG);
            if bg != DEFAULT_BG {
                let _ = write!(
                    svg,
                    r#"<rect x="{}" y="{}" width="{CELL_W}" height="{CELL_H}" fill="{bg}"/>"#,
                    u32::from(x) * CELL_W,
                    u32::from(y) * CELL_H
                );
            }
            let symbol = cell.symbol();
            if symbol.is_empty() || symbol == " " {
                continue;
            }
            let fg = svg_color(cell.fg, DEFAULT_FG);
            let weight = if cell.modifier.contains(Modifier::BOLD) {
                "bold"
            } else {
                "normal"
            };
            let opacity = if cell.modifier.contains(Modifier::DIM) {
                "0.62"
            } else {
                "1"
            };
            let decoration = if cell.modifier.contains(Modifier::UNDERLINED) {
                "underline"
            } else {
                "none"
            };
            let _ = write!(
                svg,
                r#"<text x="{}" y="{}" fill="{fg}" font-weight="{weight}" opacity="{opacity}" text-decoration="{decoration}" xml:space="preserve">{}</text>"#,
                u32::from(x) * CELL_W,
                u32::from(y) * CELL_H + 19,
                svg_escape(symbol)
            );
        }
    }
    svg.push_str("</g></svg>");
    svg
}

fn write_onboarding_svg(
    output_dir: &std::path::Path,
    filename: &str,
    app: &App,
    width: u16,
    height: u16,
) {
    let buffer = render_onboarding_buffer(app, width, height);
    std::fs::write(output_dir.join(filename), onboarding_buffer_svg(&buffer)).unwrap();
}

/// Render the FULL app frame (welcome card, overlays, transcript, composer)
/// exactly as the live TUI draws it, and write it as an SVG artifact.
fn write_full_frame_svg(
    output_dir: &std::path::Path,
    filename: &str,
    app: &App,
    width: u16,
    height: u16,
) {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, app as &dyn crate::tui::TuiState))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    std::fs::write(output_dir.join(filename), onboarding_buffer_svg(&buffer)).unwrap();
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
        text.contains("Share full transcripts (30-day retention)"),
        "option 1: {text}"
    );
    assert!(
        text.contains("Includes prompts, model responses, reasoning, code, and tool"),
        "caption 1: {text}"
    );
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

/// Generate a reviewable image for every state in the onboarding graph
/// (`onboarding_graph.rs`), rendered through the exact widget tree the live
/// flow uses. Welcome-card states render via the onboarding welcome layout;
/// picker-overlay and session states render the FULL app frame via
/// `ui::draw`. Ignored during normal test runs because it writes artifacts.
/// `scripts/capture_onboarding.sh` is the supported entry point.
#[test]
#[ignore = "artifact generator; run scripts/capture_onboarding.sh"]
fn onboarding_import_happy_path_images() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::{ImportReview, SummaryPill};

    let output_dir = std::env::var_os("JCODE_ONBOARDING_SCREENSHOT_DIR")
        .map(std::path::PathBuf::from)
        .expect("JCODE_ONBOARDING_SCREENSHOT_DIR must be set");
    std::fs::create_dir_all(&output_dir).unwrap();

    let width = 110;
    let height = 38;

    let many_candidates = || {
        vec![
            ExternalAuthReviewCandidate::fixture(
                "OpenRouter/API-key providers",
                "OpenCode auth.json",
            ),
            ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "pi auth.json"),
            ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
            ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
            ExternalAuthReviewCandidate::fixture("Gemini", "Gemini CLI"),
            ExternalAuthReviewCandidate::fixture(
                "GitHub Copilot",
                "GitHub Copilot CLI hosts.json",
            ),
            ExternalAuthReviewCandidate::fixture("Cursor", "Cursor auth.json"),
        ]
    };

    // ---- login_openai: fresh install, nothing importable detected ----
    {
        let app = app_in_phase(OnboardingPhase::LoginOpenAi {
            yes_highlighted: true,
        });
        write_onboarding_svg(&output_dir, "login-openai.svg", &app, width, height);
    }

    // ---- login_import: detected-logins summary, Import preselected ----
    {
        let mut review = ImportReview::new(many_candidates()).unwrap();
        review.focus_summary_pill(SummaryPill::Continue);
        let app = app_in_phase(OnboardingPhase::Login {
            import: Some(review),
        });
        write_onboarding_svg(&output_dir, "login-import.svg", &app, width, height);
    }

    // ---- login_import (choose mode): per-login yes/no checkbox list ----
    {
        let mut review = ImportReview::new(many_candidates()).unwrap();
        review.enter_choose_mode();
        let app = app_in_phase(OnboardingPhase::Login {
            import: Some(review),
        });
        write_onboarding_svg(&output_dir, "login-import-choose.svg", &app, width, height);
    }

    // ---- login_import (telemetry sub-page) ----
    {
        let mut review = ImportReview::new(many_candidates()).unwrap();
        review.focus_summary_pill(SummaryPill::Telemetry);
        review.open_telemetry();
        let app = app_in_phase(OnboardingPhase::Login {
            import: Some(review),
        });
        write_onboarding_svg(
            &output_dir,
            "login-import-telemetry.svg",
            &app,
            width,
            height,
        );
    }

    // ---- import committed, async import running (transient progress card) ----
    {
        let mut app = app_in_phase(OnboardingPhase::Login { import: None });
        app.onboarding_import_in_progress = Some(std::time::Instant::now());
        write_onboarding_svg(&output_dir, "login-importing.svg", &app, width, height);
    }

    // ---- login_recovery: nothing imported, manual provider pick ----
    {
        let app = app_in_phase(OnboardingPhase::Login { import: None });
        write_onboarding_svg(&output_dir, "login-recovery.svg", &app, width, height);
    }

    // ---- login_failed: classified failure with actionable recovery ----
    {
        let mut app = app_in_phase(OnboardingPhase::Login { import: None });
        app.onboarding_import_error =
            Some("the OAuth flow did not complete".to_string());
        write_onboarding_svg(&output_dir, "login-failed.svg", &app, width, height);
    }

    // ---- cred_rejected: a permanently rejected saved credential ----
    {
        let mut app = app_in_phase(OnboardingPhase::Login { import: None });
        app.onboarding_import_error = Some("the saved credential was rejected".to_string());
        write_onboarding_svg(&output_dir, "cred-rejected.svg", &app, width, height);
    }

    // ---- continue_prompt (legacy): resume an external CLI session ----
    {
        let app = app_in_phase(OnboardingPhase::ContinuePrompt {
            cli: ExternalCli::Codex,
            yes_highlighted: true,
            shown_at: std::time::Instant::now(),
        });
        write_onboarding_svg(&output_dir, "continue-prompt.svg", &app, width, height);
    }

    // ---- start_choice: the action-only picker overlay (full frame) ----
    {
        let mut app = create_test_app();
        app.onboarding_preview_mode = true;
        app.onboarding_flow = Some(OnboardingFlow {
            phase: OnboardingPhase::StartChoice {
                shown_at: std::time::Instant::now(),
            },
        });
        app.onboarding_open_start_choice();
        write_full_frame_svg(&output_dir, "start-choice.svg", &app, width, height);
    }

    // ---- suggestions: the resting new-session welcome with prompt cards ----
    // The suggestion cards only render when some provider is authenticated
    // (otherwise the welcome collapses to "Log in to get started"), so give
    // the app a synthetic API-key credential for these two session renders.
    crate::env::set_var("OPENROUTER_API_KEY", "sk-or-fixture-for-screenshots");
    crate::auth::AuthStatus::invalidate_cached_status();
    {
        let mut app = create_test_app();
        app.onboarding_preview_mode = true;
        app.onboarding_flow = Some(OnboardingFlow {
            phase: OnboardingPhase::Suggestions,
        });
        write_full_frame_svg(&output_dir, "suggestions.svg", &app, width, height);
    }

    // ---- done (review turn): the suggested architecture review accepted ----
    {
        // The full frame includes the "Updates" box when the machine running
        // the generator has unseen changelog entries, which makes the artifact
        // depend on developer-local state. Force it empty for determinism.
        crate::tui::ui::header::set_unseen_changelog_entries_override_for_tests(Some(Vec::new()));
        // The git info widget would otherwise capture the live ahead/behind and
        // dirty counts of the repo the generator runs in. Pin it to a clean
        // fixture branch. (The version label is compile-time build meta, which
        // is deterministic for a given checkout and expected to advance.)
        crate::tui::app::helpers::seed_git_info_cache_for_tests(Some(
            crate::tui::info_widget::GitInfo {
                branch: "main".to_string(),
                modified: 0,
                staged: 0,
                untracked: 0,
                ahead: 0,
                behind: 0,
                dirty_files: Vec::new(),
            },
        ));
        let mut app = create_test_app();
        // The header shows a randomly drawn session mascot ("client: Goat 🐐"),
        // which would make the artifact differ run to run. Pin it.
        app.session.short_name = Some("sauropod".to_string());
        let prompt = App::onboarding_recent_project_review_prompt(std::path::Path::new(
            "~/projects/my-app",
        ));
        app.push_display_message(DisplayMessage::user(prompt));
        app.is_processing = true;
        write_full_frame_svg(&output_dir, "review-turn.svg", &app, width, height);
        crate::tui::ui::header::set_unseen_changelog_entries_override_for_tests(None);
    }
    crate::env::remove_var("OPENROUTER_API_KEY");
    crate::auth::AuthStatus::invalidate_cached_status();

    println!("wrote onboarding images to {}", output_dir.display());
}
