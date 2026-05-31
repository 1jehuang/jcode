// Integration tests for the first-run onboarding flow control logic.

use super::onboarding_flow::{ExternalCli, OnboardingFlow, OnboardingPhase};

fn onboarding_test_app() -> App {
    let mut app = create_test_app();
    // Force the flow on regardless of the on-disk new-user heuristic.
    app.onboarding_flow = Some(OnboardingFlow::begin());
    app
}

#[test]
fn onboarding_begins_at_model_select() {
    let mut app = create_test_app();
    app.onboarding_flow = None;
    app.begin_onboarding_flow();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::ModelSelect)
    ));
    // begin is idempotent: a second call does not reset the phase.
    if let Some(flow) = app.onboarding_flow.as_mut() {
        flow.phase = OnboardingPhase::Suggestions;
    }
    app.begin_onboarding_flow();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::Suggestions)
    ));
}

#[test]
fn onboarding_can_begin_at_login_phase() {
    let mut app = create_test_app();
    app.onboarding_flow = None;
    app.begin_onboarding_flow_at_login();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::Login { .. })
    ));
    // begin_at_login is idempotent: a second call does not reset the phase.
    if let Some(flow) = app.onboarding_flow.as_mut() {
        flow.phase = OnboardingPhase::Suggestions;
    }
    app.begin_onboarding_flow_at_login();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::Suggestions)
    ));
}

#[test]
fn login_welcome_kind_shows_first_import_candidate() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::OnboardingWelcomeKind;
    use crate::tui::app::onboarding_flow::ImportReview;

    let mut app = create_test_app();
    app.onboarding_flow = None;
    app.begin_onboarding_flow_at_login();
    // Inject a per-candidate import walkthrough as if external logins were
    // detected at startup.
    let review = ImportReview::new(vec![
        ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
        ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
    ])
    .unwrap();
    if let Some(flow) = app.onboarding_flow.as_mut() {
        flow.phase = OnboardingPhase::Login {
            import: Some(review),
        };
    }
    match app.onboarding_welcome_kind() {
        OnboardingWelcomeKind::Login { import: Some(prompt) } => {
            assert_eq!(prompt.provider_summary, "OpenAI/Codex");
            assert_eq!(prompt.source_name, "Codex auth.json");
            assert_eq!(prompt.position, 1);
            assert_eq!(prompt.total, 2);
            assert!(prompt.yes_highlighted);
        }
        other => panic!("expected Login welcome with import prompt, got {other:?}"),
    }
}

#[test]
fn import_review_walks_candidates_and_collects_approvals() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    let mut review = ImportReview::new(vec![
        ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
        ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
        ExternalAuthReviewCandidate::fixture("Gemini", "Gemini CLI"),
    ])
    .unwrap();
    assert_eq!(review.position(), 1);
    assert_eq!(review.total(), 3);

    // Candidate 1: approve (Yes is default).
    assert!(!review.commit_current());
    // Candidate 2: decline.
    review.set_yes(false);
    assert!(!review.commit_current());
    // Candidate 3: approve. Now finished.
    review.set_yes(true);
    assert!(review.commit_current());

    assert_eq!(review.approved, vec![0, 2]);
}

#[test]
fn import_review_highlight_navigation() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    let mut review =
        ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("Cursor", "Cursor")]).unwrap();
    assert!(review.yes_highlighted);
    review.toggle();
    assert!(!review.yes_highlighted);
    review.set_yes(true);
    assert!(review.yes_highlighted);
}

#[test]
fn login_phase_advances_to_telemetry_consent_then_model_select() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { .. })
        ));
        // After login we ask for telemetry consent first.
        app.onboarding_after_login();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::TelemetryConsent {
                yes_highlighted: false,
                ..
            })
        ));
        // onboarding_after_login is a no-op once we're past the Login phase.
        app.onboarding_after_login();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::TelemetryConsent { .. })
        ));
        // Declining advances to model select and does not opt in.
        app.onboarding_answer_telemetry_consent(false);
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::ModelSelect)
        ));
        assert!(!crate::telemetry::content_sharing_enabled());
    });
}

#[test]
fn telemetry_consent_opt_in_persists_and_advances() {
    with_temp_jcode_home(|| {
        // Ensure base telemetry isn't globally disabled in the test env, so the
        // content-sharing opt-in is observable.
        let saved = (
            std::env::var_os("JCODE_NO_TELEMETRY"),
            std::env::var_os("DO_NOT_TRACK"),
        );
        crate::env::remove_var("JCODE_NO_TELEMETRY");
        crate::env::remove_var("DO_NOT_TRACK");

        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::TelemetryConsent {
                yes_highlighted: false,
                shown_at: std::time::Instant::now(),
            };
        }
        // Right highlights Yes, Enter commits -> opt in.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Right));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::ModelSelect)
        ));
        assert!(crate::telemetry::content_sharing_enabled());
        // Re-running the flow and declining clears the opt-in.
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::TelemetryConsent {
                yes_highlighted: true,
                shown_at: std::time::Instant::now(),
            };
        }
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
        assert!(!crate::telemetry::content_sharing_enabled());

        if let Some(v) = saved.0 {
            crate::env::set_var("JCODE_NO_TELEMETRY", v);
        }
        if let Some(v) = saved.1 {
            crate::env::set_var("DO_NOT_TRACK", v);
        }
    });
}

#[test]
fn telemetry_consent_y_key_opts_in() {
    with_temp_jcode_home(|| {
        let saved = (
            std::env::var_os("JCODE_NO_TELEMETRY"),
            std::env::var_os("DO_NOT_TRACK"),
        );
        crate::env::remove_var("JCODE_NO_TELEMETRY");
        crate::env::remove_var("DO_NOT_TRACK");

        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::TelemetryConsent {
                yes_highlighted: false,
                shown_at: std::time::Instant::now(),
            };
        }
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('y')));
        assert!(crate::telemetry::content_sharing_enabled());
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::ModelSelect)
        ));

        if let Some(v) = saved.0 {
            crate::env::set_var("JCODE_NO_TELEMETRY", v);
        }
        if let Some(v) = saved.1 {
            crate::env::set_var("DO_NOT_TRACK", v);
        }
    });
}

#[test]
fn import_review_decision_timer_counts_down_and_times_out() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::{DECISION_TIMEOUT, ImportReview};

    let mut review =
        ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("Cursor", "Cursor")]).unwrap();
    // Fresh review: a full timeout's worth of seconds remain and it hasn't
    // timed out yet.
    assert!(review.seconds_remaining() <= DECISION_TIMEOUT.as_secs());
    assert!(!review.timed_out());
    // Force the clock past the timeout.
    review.shown_at = std::time::Instant::now() - (DECISION_TIMEOUT + std::time::Duration::from_secs(1));
    assert_eq!(review.seconds_remaining(), 0);
    assert!(review.timed_out());
}

#[test]
fn login_phase_enter_opens_login_picker() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        // Force the no-detected-imports case so this test exercises the manual
        // login fallback regardless of any external logins on the host. (The
        // import walkthrough has its own dedicated tests.)
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        assert!(app.inline_interactive_state.is_none());
        // Enter from the welcome screen opens the interactive login picker.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(app.inline_interactive_state.is_some());
        // With a picker already open, Enter is no longer consumed by onboarding
        // so the picker can commit the selection.
        assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
    });
}

#[test]
fn import_failure_resets_login_to_manual_prompt() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        // Simulate the walkthrough having approved a candidate and kicked off an
        // import (the per-candidate sub-state is cleared once the import spawns).
        let review =
            ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("Cursor", "Cursor")])
                .unwrap();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login {
                import: Some(review),
            };
        }
        // The async import later fails -> handle_login_failed must reset the
        // Login phase to the clean manual-login prompt so the welcome card stops
        // fighting the error message / donut.
        app.onboarding_handle_login_failed();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { import: None })
        ));
        // Still in Login: Enter opens the manual login picker so the user can
        // recover.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(app.inline_interactive_state.is_some());
    });
}

#[test]
fn import_review_decline_all_falls_back_to_manual_login() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        let review = ImportReview::new(vec![ExternalAuthReviewCandidate::fixture(
            "OpenAI/Codex",
            "Codex auth.json",
        )])
        .unwrap();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login {
                import: Some(review),
            };
        }
        // Decline the only candidate ("No" then Enter). With nothing approved we
        // don't spawn an import, the walkthrough clears, and the card falls back
        // to the manual-login prompt.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { import: None })
        ));
        // Still in Login: Enter now opens the manual login picker.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(app.inline_interactive_state.is_some());
    });
}

#[test]
fn answering_no_on_continue_prompt_shows_suggestions() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: std::time::Instant::now(),
            };
        }
        app.onboarding_answer_continue(false);
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Suggestions)
        ));
        // No session picker overlay opened on the "No" path.
        assert!(app.session_picker_overlay.is_none());
    });
}

#[test]
fn continue_prompt_key_y_consumes_and_advances() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::ClaudeCode,
                yes_highlighted: true,
                shown_at: std::time::Instant::now(),
            };
        }
        // 'Y' is consumed by the onboarding handler.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('Y')));
        // It either opened the picker (TranscriptPick) or fell back depending on
        // whether transcripts exist in the temp home; either way it leaves
        // ContinuePrompt.
        assert!(!matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::ContinuePrompt { .. })
        ));
    });
}

#[test]
fn continue_prompt_key_ignored_when_not_in_phase() {
    let mut app = create_test_app();
    app.onboarding_flow = None;
    assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::Char('y')));
}

#[test]
fn no_external_transcripts_falls_back_to_session_search() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: std::time::Instant::now(),
            };
        }
        // Temp home has no Codex transcripts, so opening the picker should fall
        // back to the session-search prompt and finish the flow.
        app.onboarding_open_transcript_picker(ExternalCli::Codex);
        assert!(matches!(
            app.onboarding_phase(),
            None | Some(OnboardingPhase::Done)
        ));
        assert!(app.session_picker_overlay.is_none());
        // The fallback announced it's finding and continuing the latest session.
        assert!(
            app.display_messages()
                .iter()
                .any(|m| m.content.contains("find and continue"))
        );
    });
}

#[test]
fn onboarding_picker_mode_carries_cli() {
    let mode = SessionPickerMode::Onboarding {
        cli: ExternalCli::ClaudeCode,
    };
    assert!(matches!(mode, SessionPickerMode::Onboarding { .. }));
    assert_ne!(mode, SessionPickerMode::Resume);
}

#[test]
fn startup_check_skips_when_session_already_has_activity() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.onboarding_startup_checked = false;
        // Simulate a resumed session with a real user message.
        app.push_display_message(DisplayMessage::user("what does this repo do?".to_string()));

        app.maybe_begin_onboarding_flow_on_startup();

        // Settled, non-empty state: guard is committed and no flow starts.
        assert!(app.onboarding_startup_checked);
        assert!(app.onboarding_flow.is_none());
    });
}

#[test]
fn startup_check_ignores_synthetic_scaffolding_messages() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.onboarding_startup_checked = false;
        // Fresh sessions still carry a synthetic system-reminder (role=user) and
        // assorted system scaffolding. These must not count as real activity.
        app.push_display_message(DisplayMessage::user(
            "<system-reminder>\n# Session Context\nDate: 2026-05-30".to_string(),
        ));
        app.push_display_message(DisplayMessage::system("Switched to model: x".to_string()));

        app.maybe_begin_onboarding_flow_on_startup();

        // The guard must not be tripped by scaffolding alone. In a temp home with
        // no working credentials the flow begins at the in-TUI Login phase (the
        // fresh-install path no longer logs in at the CLI before the TUI).
        assert!(
            !app.display_messages.is_empty(),
            "precondition: scaffolding messages present"
        );
        assert!(app.onboarding_startup_checked);
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { .. })
        ));
    });
}

#[test]
fn startup_check_skips_when_input_is_present() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.onboarding_startup_checked = false;
        app.input = "restored draft".to_string();

        app.maybe_begin_onboarding_flow_on_startup();

        assert!(app.onboarding_startup_checked);
        assert!(app.onboarding_flow.is_none());
    });
}

#[test]
fn startup_check_is_noop_once_committed() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.onboarding_startup_checked = true;

        app.maybe_begin_onboarding_flow_on_startup();

        // Already committed: never touches the flow.
        assert!(app.onboarding_flow.is_none());
    });
}
