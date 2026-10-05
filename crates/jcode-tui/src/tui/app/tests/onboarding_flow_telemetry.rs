// Onboarding flow: telemetry settings page and recent-project prefetch.

#[test]
fn telemetry_pill_opens_settings_page_and_commits_choice() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::{ImportReview, TelemetryLevel};

    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        let review =
            ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json")])
                .unwrap();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login {
                import: Some(review),
            };
        }

        // Right twice: Subscription -> Import less -> Telemetry settings.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Right));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Right));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));

        // The page opens defaulted to "Send everything".
        match app.onboarding_phase() {
            Some(OnboardingPhase::Login {
                import: Some(review),
            }) => assert_eq!(review.telemetry, Some(TelemetryLevel::Everything)),
            other => panic!("expected telemetry page open, got {other:?}"),
        }
        // The import countdown is paused while the page is open, so the screen
        // cannot commit the import out from under the user.
        assert!(!app.onboarding_flow.as_ref().unwrap().decision_timed_out());

        // Enter commits "Send everything": usage on, content sharing on.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        if !crate::telemetry::opt_out_forced_by_env() {
            assert!(crate::telemetry::is_enabled());
            assert!(crate::telemetry::content_sharing_enabled());
        }
        let no_telemetry_marker = std::path::Path::new(
            &std::env::var_os("JCODE_HOME").expect("temporary JCODE_HOME"),
        )
        .join("no_telemetry");
        assert!(
            !no_telemetry_marker.exists(),
            "Send everything must remove the persisted opt-out marker"
        );
        // We are back on the summary screen with the import still pending.
        match app.onboarding_phase() {
            Some(OnboardingPhase::Login {
                import: Some(review),
            }) => {
                assert!(review.telemetry.is_none());
                assert!(!review.choosing);
            }
            other => panic!("expected import summary, got {other:?}"),
        }
        assert!(app.onboarding_import_in_progress.is_none());
    });
}

#[test]
fn telemetry_page_send_nothing_disables_telemetry_and_esc_goes_back() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        let review =
            ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json")])
                .unwrap();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login {
                import: Some(review),
            };
        }

        // t is the direct shortcut onto the telemetry page; Esc returns without
        // changing anything and keeps onboarding active.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('t')));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Esc));
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { import: Some(_) })
        ));
        if !crate::telemetry::opt_out_forced_by_env() {
            assert!(crate::telemetry::is_enabled());
        }

        // Reopen, walk down to "Send nothing", commit.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('t')));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Down));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Down));
        // In the dependency build used by this crate, telemetry-core is not
        // compiled with cfg(test), so the in-app opt-out event would otherwise
        // use the real delivery path. Keep the UI preconditions above free of
        // inherited opt-out env, then force opt-out only for the commit action:
        // telemetry-core sees delivery blocked by env while still writing the
        // no_telemetry marker that this test verifies after the guard drops.
        let delivery_block = EnvRestoreGuard::set("JCODE_NO_TELEMETRY", "1");
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        drop(delivery_block);
        assert!(!crate::telemetry::is_enabled());
        assert!(!crate::telemetry::content_sharing_enabled());
        let no_telemetry_marker = std::path::Path::new(
            &std::env::var_os("JCODE_HOME").expect("temporary JCODE_HOME"),
        )
        .join("no_telemetry");
        assert!(
            no_telemetry_marker.exists(),
            "Send nothing must persist the opt-out marker"
        );
    });
}

#[test]
fn start_choice_prefetches_recent_project_so_enter_does_not_block() {
    let mut app = onboarding_test_app();
    assert!(
        app.onboarding_recent_project_prefetch.is_none(),
        "no prefetch before the start choice is shown"
    );

    app.onboarding_open_start_choice();

    let slot = app
        .onboarding_recent_project_prefetch
        .clone()
        .expect("opening the start choice should warm the recent-project lookup");

    // Wait briefly for the background scan; the action must not depend on it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if slot.lock().expect("prefetch slot").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        slot.lock().expect("prefetch slot").is_some(),
        "prefetch should resolve in the background"
    );

    // Opening the choice twice must not spawn a second scan.
    app.onboarding_open_start_choice();
    assert!(
        std::sync::Arc::ptr_eq(
            &slot,
            app.onboarding_recent_project_prefetch
                .as_ref()
                .expect("prefetch retained")
        ),
        "the warm prefetch should be reused"
    );

    // The resolved path is still the repository the session runs in.
    assert_eq!(
        app.onboarding_recent_project_path(),
        crate::import::repo_ranking::resolve_git_root(std::path::Path::new(
            app.session
                .working_dir
                .as_deref()
                .expect("test session working dir")
        ))
    );
}
