// Tests for the onboarding rehearsal (Alt+5 restart, Cmd+5 toggle, and
// `/onboarding-sim`): the real first-run flow, run as if jcode were new.
//
// `include!`d into `crate::tui::app::tests`, so it shares the `create_test_app`
// harness and the onboarding type imports from the sibling includes.

use crossterm::event::{KeyCode, KeyModifiers};

/// Put a Gemini CLI login where the detector looks (`$JCODE_HOME/external`).
fn write_fake_gemini_cli_login() -> std::path::PathBuf {
    let path = crate::storage::user_home_path(".gemini/oauth_creds.json").unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, br#"{"access_token":"x","refresh_token":"y"}"#).unwrap();
    path
}

fn import_rows(app: &App) -> Vec<String> {
    match app.onboarding_phase() {
        Some(OnboardingPhase::Login {
            import: Some(review),
        }) => review
            .candidates
            .iter()
            .map(|c| c.provider_summary().to_string())
            .collect(),
        other => panic!("expected the import screen, got {other:?}"),
    }
}

#[test]
fn rehearsal_starts_the_real_first_run_flow_and_exits() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        assert!(!app.onboarding_sim_active());

        app.handle_key(KeyCode::Char('5'), KeyModifiers::SUPER)
            .unwrap();
        assert!(app.onboarding_sim_active(), "Cmd+5 should start it");
        assert!(app.onboarding_welcome_active());
        // Nothing to import in an empty home, so the real flow asks about
        // OpenAI, exactly like a brand-new user with no other tools.
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::LoginOpenAi {
                yes_highlighted: true
            })
        ));

        // Keys reach the real handlers: l moves the highlight to "No".
        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE)
            .unwrap();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::LoginOpenAi {
                yes_highlighted: false
            })
        ));

        app.handle_key(KeyCode::Char('5'), KeyModifiers::SUPER)
            .unwrap();
        assert!(!app.onboarding_sim_active(), "Cmd+5 should stop it");
        assert!(app.onboarding_phase().is_none());
    });
}

#[test]
fn rehearsal_finds_logins_jcode_already_trusts() {
    with_temp_jcode_home(|| {
        write_fake_gemini_cli_login();
        crate::auth::gemini::trust_cli_auth_for_future_use().unwrap();
        // A real launch on this set-up machine offers nothing: jcode already
        // trusts the Gemini login.
        assert!(
            crate::external_auth::pending_external_auth_review_candidates()
                .unwrap()
                .is_empty()
        );

        // The rehearsal sees what a brand-new install would find.
        let mut app = create_test_app();
        app.start_onboarding_simulator();
        assert_eq!(import_rows(&app), vec!["Gemini".to_string()]);
    });
}

#[test]
fn rehearsal_keys_drive_the_real_import_summary() {
    use crate::tui::app::onboarding_flow::SummaryPill;
    with_temp_jcode_home(|| {
        write_fake_gemini_cli_login();
        let mut app = create_test_app();
        app.start_onboarding_simulator();

        let pill = |app: &App| match app.onboarding_phase() {
            Some(OnboardingPhase::Login {
                import: Some(review),
            }) => review.summary_pill,
            other => panic!("expected import summary, got {other:?}"),
        };
        assert_eq!(pill(&app), SummaryPill::Continue);
        app.handle_key(KeyCode::Right, KeyModifiers::NONE).unwrap();
        assert_eq!(pill(&app), SummaryPill::ImportLess);
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        assert_eq!(pill(&app), SummaryPill::Telemetry);

        // Enter on "Import less" opens the real checkbox list.
        app.handle_key(KeyCode::Up, KeyModifiers::NONE).unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { import: Some(review) }) if review.choosing
        ));
    });
}

#[test]
fn rehearsal_telemetry_choice_is_not_saved() {
    with_temp_jcode_home(|| {
        write_fake_gemini_cli_login();
        let mut app = create_test_app();
        app.start_onboarding_simulator();
        let before = crate::tui::app::onboarding_flow::TelemetryLevel::current();

        // Open telemetry settings, pick "Send nothing".
        app.handle_key(KeyCode::Char('t'), KeyModifiers::NONE)
            .unwrap();
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();

        assert_eq!(
            crate::tui::app::onboarding_flow::TelemetryLevel::current(),
            before
        );
        let marker = std::path::Path::new(&std::env::var_os("JCODE_HOME").unwrap())
            .join("no_telemetry");
        assert!(!marker.exists(), "rehearsal must not write telemetry markers");
    });
}

#[test]
fn alt_5_restarts_the_rehearsal_from_a_pristine_first_screen() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.start_onboarding_simulator();
        // Move off the default highlight so a restart is observable.
        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE)
            .unwrap();

        // Seed every transient field that could make a restart look like a
        // half-completed or failed onboarding attempt.
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

        assert!(app.onboarding_sim_active());
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
            !app.onboarding_auto_model_selection_active
                .load(std::sync::atomic::Ordering::Acquire)
        );
    });
}

#[test]
fn altgr_5_does_not_start_onboarding_rehearsal() {
    let mut app = create_test_app();
    app.handle_key(
        KeyCode::Char('5'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    )
    .unwrap();
    assert!(!app.onboarding_sim_active());
}
