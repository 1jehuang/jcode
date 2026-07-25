// Tests for the onboarding simulator (Alt+5 reset, Cmd+5 toggle, and
// `/onboarding-sim`).
//
// `include!`d into `crate::tui::app::tests`, so it shares the `create_test_app`
// harness and the onboarding type imports from the sibling includes.

use crossterm::event::{KeyCode, KeyModifiers};

#[test]
fn onboarding_sim_starts_steps_and_exits() {
    let mut app = create_test_app();
    assert!(!app.onboarding_sim_active());

    // Cmd+5 starts the simulator on the first screen.
    app.handle_key(KeyCode::Char('5'), KeyModifiers::SUPER)
        .unwrap();
    assert!(app.onboarding_sim_active(), "Cmd+5 should start the sim");
    assert!(
        app.onboarding_welcome_active(),
        "sim should render the onboarding welcome screen"
    );
    // First screen is the LoginOpenAi prompt.
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::LoginOpenAi { .. })
    ));

    // Tab advances to the import-review screen.
    app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::Login { import: Some(_) })
    ));

    // Esc exits and clears all onboarding state.
    app.handle_key(KeyCode::Esc, KeyModifiers::NONE).unwrap();
    assert!(!app.onboarding_sim_active(), "Esc should stop the sim");
    assert!(app.onboarding_phase().is_none());
}

#[test]
fn onboarding_sim_highlight_toggles_without_real_action() {
    let mut app = create_test_app();
    app.start_onboarding_simulator();

    // On the LoginOpenAi screen, 'l' previews the "No" highlight and 'h' the
    // "Yes" highlight. Neither triggers a real login (no overlay opens).
    app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE)
        .unwrap();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::LoginOpenAi {
            yes_highlighted: false
        })
    ));
    app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE)
        .unwrap();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::LoginOpenAi {
            yes_highlighted: true
        })
    ));
    assert!(
        app.onboarding_sim_active(),
        "previewing highlight should not exit the sim"
    );

    app.stop_onboarding_simulator();
    assert!(!app.onboarding_sim_active());
}

#[test]
fn onboarding_sim_advancing_past_last_screen_exits() {
    let mut app = create_test_app();
    app.start_onboarding_simulator();
    // Step forward many times; once we run off the end the sim exits cleanly.
    for _ in 0..20 {
        if !app.onboarding_sim_active() {
            break;
        }
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
    }
    assert!(
        !app.onboarding_sim_active(),
        "stepping past the last screen should exit the sim"
    );
}

#[test]
fn alt_5_resets_onboarding_sim_to_a_pristine_first_screen() {
    let mut app = create_test_app();
    app.start_onboarding_simulator();
    app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
    assert_eq!(app.onboarding_sim, Some(1));

    // Seed every transient field that could make a restarted simulation look
    // like a half-completed or failed real onboarding attempt.
    app.onboarding_import_in_progress = Some(std::time::Instant::now());
    app.onboarding_import_error = Some("stale import error".to_string());
    app.onboarding_import_failed_provider = Some("stale-provider".to_string());
    app.onboarding_pending_model_validation = Some(
        crate::tui::app::onboarding_flow::OnboardingPendingValidation::new(
            "stale-session".to_string(),
        ),
    );
    app.onboarding_auto_model_selection_active
        .store(true, std::sync::atomic::Ordering::Release);
    app.help_scroll = Some(4);
    app.model_status_scroll = Some(2);
    app.copy_selection_mode = true;
    app.copy_selection_dragging = true;

    app.handle_key(KeyCode::Char('5'), KeyModifiers::ALT)
        .unwrap();

    assert_eq!(app.onboarding_sim, Some(0));
    assert!(app.onboarding_preview_mode);
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::LoginOpenAi {
            yes_highlighted: true
        })
    ));
    assert!(app.onboarding_import_in_progress.is_none());
    assert!(app.onboarding_import_error.is_none());
    assert!(app.onboarding_import_failed_provider.is_none());
    assert!(app.onboarding_pending_model_validation.is_none());
    assert!(app.help_scroll.is_none());
    assert!(app.model_status_scroll.is_none());
    assert!(!app.copy_selection_mode);
    assert!(!app.copy_selection_dragging);
    assert!(
        !app
            .onboarding_auto_model_selection_active
            .load(std::sync::atomic::Ordering::Acquire)
    );
}

#[test]
fn altgr_5_does_not_start_onboarding_simulator() {
    let mut app = create_test_app();
    app.handle_key(
        KeyCode::Char('5'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    )
    .unwrap();
    assert!(!app.onboarding_sim_active());
}

#[test]
fn onboarding_sim_includes_telemetry_settings_screen() {
    let mut app = create_test_app();
    app.start_onboarding_simulator();

    // Tab through screens until the telemetry settings page shows. The sim's
    // catalog is small, so a bounded walk is enough and it fails loudly if the
    // screen was dropped from the catalog.
    let mut found = false;
    for _ in 0..12 {
        if let Some(OnboardingPhase::Login {
            import: Some(review),
        }) = app.onboarding_phase()
            && review.telemetry.is_some()
        {
            found = true;
            break;
        }
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();
    }
    assert!(found, "sim should include a telemetry settings screen");

    // Down/Up preview the three options without persisting anything.
    use crate::tui::app::onboarding_flow::TelemetryLevel;
    app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
    match app.onboarding_phase() {
        Some(OnboardingPhase::Login {
            import: Some(review),
        }) => assert_eq!(review.telemetry, Some(TelemetryLevel::NoContent)),
        other => panic!("expected telemetry screen, got {other:?}"),
    }
    app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
    match app.onboarding_phase() {
        Some(OnboardingPhase::Login {
            import: Some(review),
        }) => assert_eq!(review.telemetry, Some(TelemetryLevel::Everything)),
        other => panic!("expected telemetry screen, got {other:?}"),
    }
}

#[test]
fn onboarding_sim_summary_arrows_preview_the_three_pills() {
    use crate::tui::app::onboarding_flow::SummaryPill;
    let mut app = create_test_app();
    app.start_onboarding_simulator();
    // Screen 2 is the read-only import summary.
    app.handle_key(KeyCode::Tab, KeyModifiers::NONE).unwrap();

    let pill = |app: &App| match app.onboarding_phase() {
        Some(OnboardingPhase::Login {
            import: Some(review),
        }) => review.summary_pill,
        other => panic!("expected import summary, got {other:?}"),
    };
    assert_eq!(pill(&app), SummaryPill::Continue);
    app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
    assert_eq!(pill(&app), SummaryPill::ImportLess);
    app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
    assert_eq!(pill(&app), SummaryPill::Telemetry);
    app.handle_key(KeyCode::Left, KeyModifiers::NONE).unwrap();
    assert_eq!(pill(&app), SummaryPill::ImportLess);
}
