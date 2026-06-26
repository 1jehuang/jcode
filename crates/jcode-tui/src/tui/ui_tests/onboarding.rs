use super::*;
use ratatui::backend::TestBackend;
use ratatui::{Terminal, layout::Rect};

/// Render the onboarding welcome screen for the given state at the given size
/// and return the flattened text of the whole buffer.
fn render_onboarding(state: &TestState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            crate::tui::ui::onboarding::draw_onboarding_welcome(frame, state, area);
        })
        .expect("failed to draw onboarding");

    let buf = terminal.backend().buffer();
    let mut lines = Vec::with_capacity(height as usize);
    for y in 0..height {
        let mut line = String::with_capacity(width as usize);
        for x in 0..width {
            line.push_str(buf[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

fn onboarding_state() -> TestState {
    TestState {
        onboarding_preview: true,
        suggestions: vec![
            ("Log in to get started".to_string(), "/login".to_string()),
            (
                "Build a small CLI tool".to_string(),
                "build a CLI".to_string(),
            ),
        ],
        ..Default::default()
    }
}

#[test]
fn onboarding_welcome_shows_telemetry_title_and_suggestions() {
    let state = onboarding_state();
    let text = render_onboarding(&state, 80, 30);

    assert!(
        text.contains("anonymous usage statistics"),
        "telemetry notice should be rendered:\n{text}"
    );
    assert!(
        text.contains("JCODE_NO_TELEMETRY=1"),
        "telemetry opt-out hint should be rendered:\n{text}"
    );
    assert!(
        text.contains("Welcome to jcode onboarding"),
        "welcome title should be rendered:\n{text}"
    );
    assert!(
        text.contains("Log in to get started"),
        "login suggestion should be rendered:\n{text}"
    );
    assert!(
        text.contains("Build a small CLI tool"),
        "secondary suggestion should be rendered:\n{text}"
    );
    assert!(
        text.contains("Press 1-2 or type anything to start"),
        "numeric hint should reflect suggestion count:\n{text}"
    );
}

#[test]
fn onboarding_welcome_login_suggestion_shows_typed_command() {
    let state = onboarding_state();
    let text = render_onboarding(&state, 80, 30);
    assert!(
        text.contains("(type /login)"),
        "login suggestion should hint the slash command:\n{text}"
    );
}

#[test]
fn onboarding_welcome_renders_on_tiny_area_without_panicking() {
    // Below the donut/full-treatment threshold: should fall back gracefully.
    // The title may be truncated at narrow widths, so only assert its prefix.
    let state = onboarding_state();
    let text = render_onboarding(&state, 20, 5);
    assert!(
        text.contains("Welcome to jcode"),
        "minimal fallback should still show the title:\n{text}"
    );
}

#[test]
fn onboarding_welcome_centers_within_tall_area() {
    // A tall area should leave blank padding above the telemetry header.
    let state = onboarding_state();
    let text = render_onboarding(&state, 80, 40);
    let first_nonblank = text
        .lines()
        .position(|line| !line.trim().is_empty())
        .expect("expected some content");
    assert!(
        first_nonblank > 0,
        "content should be vertically padded from the top:\n{text}"
    );
}

#[test]
fn onboarding_login_card_renders_searched_not_found_panel() {
    use crate::tui::{NotFoundRow, OnboardingWelcomeKind};

    let not_found = vec![
        NotFoundRow {
            label: "Codex".to_string(),
            path: "~/.codex/auth.json".to_string(),
        },
        NotFoundRow {
            label: "Cursor".to_string(),
            path: "~/.cursor/auth.json".to_string(),
        },
    ];
    let state = TestState {
        onboarding_preview: true,
        onboarding_welcome_kind: Some(OnboardingWelcomeKind::LoginOpenAi {
            yes_highlighted: true,
            not_found,
            not_found_scroll: 0,
        }),
        ..Default::default()
    };
    let text = render_onboarding(&state, 80, 40);
    assert!(
        text.contains("Searched, not found"),
        "should render the not-found header:\n{text}"
    );
    assert!(
        text.contains("Codex") && text.contains("Cursor"),
        "should list the absent sources:\n{text}"
    );
}

#[test]
fn onboarding_not_found_panel_shows_scroll_affordance_when_overflowing() {
    use crate::tui::{NotFoundRow, OnboardingWelcomeKind};

    let not_found: Vec<NotFoundRow> = (0..9)
        .map(|i| NotFoundRow {
            label: format!("Source {i}"),
            path: format!("~/path/{i}"),
        })
        .collect();
    let state = TestState {
        onboarding_preview: true,
        onboarding_welcome_kind: Some(OnboardingWelcomeKind::LoginOpenAi {
            yes_highlighted: true,
            not_found,
            not_found_scroll: 0,
        }),
        ..Default::default()
    };
    let text = render_onboarding(&state, 80, 44);
    assert!(
        text.contains("more") && text.contains("scroll"),
        "overflowing panel should show a scroll affordance:\n{text}"
    );
}



#[test]
fn onboarding_scrollwm_optin_card_renders_decision_and_progress() {
    use crate::tui::{OnboardingWelcomeKind, ScrollWmInstallProgress};

    // Decision state: shows the pitch + Yes/No + countdown.
    let decision = TestState {
        onboarding_preview: true,
        onboarding_welcome_kind: Some(OnboardingWelcomeKind::ScrollWmOptIn {
            yes_highlighted: false,
            seconds_left: 60,
            progress: None,
        }),
        ..Default::default()
    };
    let text = render_onboarding(&decision, 80, 34);
    assert!(text.contains("Set up ScrollWM?"), "pitch title:\n{text}");
    assert!(text.contains("Accessibility"), "permission note:\n{text}");
    assert!(
        text.contains("Yes") && text.contains("No"),
        "Yes/No row:\n{text}"
    );
    assert!(text.contains("Skips automatically"), "countdown:\n{text}");

    // Running state: shows the install progress line, no Yes/No countdown.
    let running = TestState {
        onboarding_preview: true,
        onboarding_welcome_kind: Some(OnboardingWelcomeKind::ScrollWmOptIn {
            yes_highlighted: false,
            seconds_left: 60,
            progress: Some(ScrollWmInstallProgress::Running),
        }),
        ..Default::default()
    };
    let text = render_onboarding(&running, 80, 34);
    assert!(
        text.contains("Installing ScrollWM"),
        "running progress line:\n{text}"
    );
    assert!(
        !text.contains("Skips automatically"),
        "countdown should be gone while installing:\n{text}"
    );
}
