// Integration tests for the first-run onboarding flow control logic.

use super::onboarding_flow::{ExternalCli, OnboardingFlow, OnboardingPhase};

fn onboarding_test_app() -> App {
    let mut app = create_test_app();
    // Force the flow on regardless of the on-disk new-user heuristic.
    app.onboarding_flow = Some(OnboardingFlow::begin());
    app
}

#[test]
fn onboarding_begins_and_advances_past_model_select() {
    let mut app = create_test_app();
    app.onboarding_flow = None;
    app.begin_onboarding_flow();
    // `begin_onboarding_flow` immediately advances past the legacy ModelSelect
    // phase: with no external transcripts to resume it lands on the
    // suggestion-card (new-session) screen rather than blocking on a picker.
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::Suggestions)
    ));
    // begin is idempotent: a second call does not reset the phase.
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
        Some(OnboardingPhase::Login { .. }) | Some(OnboardingPhase::LoginOpenAi { .. })
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
        OnboardingWelcomeKind::Login { import: Some(prompt), .. } => {
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
fn login_phase_advances_to_model_select_without_telemetry_prompt() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        // Force the bare Login phase (the recovery/import path) so we exercise
        // onboarding_after_login directly regardless of host logins.
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { .. })
        ));
        // After login we no longer ask a telemetry-consent question; we advance
        // straight to model selection and leave content sharing off.
        app.onboarding_after_login();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::ModelSelect) | Some(OnboardingPhase::Suggestions)
        ));
        assert!(!crate::telemetry::content_sharing_enabled());
    });
}

#[test]
fn login_openai_phase_is_default_when_no_imports() {
    use crate::tui::OnboardingWelcomeKind;
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        // Fresh temp home has no importable logins, so begin_at_login lands on
        // the "Log in to OpenAI?" Yes/No prompt (not the bare provider picker).
        app.begin_onboarding_flow_at_login();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::LoginOpenAi {
                yes_highlighted: true
            })
        ));
        assert!(matches!(
            app.onboarding_welcome_kind(),
            OnboardingWelcomeKind::LoginOpenAi {
                yes_highlighted: true,
                ..
            }
        ));
    });
}

#[test]
fn login_openai_no_opens_provider_picker() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::LoginOpenAi {
                yes_highlighted: true,
            };
        }
        assert!(app.inline_interactive_state.is_none());
        // 'n' opens the full provider picker so the user can choose another.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
        assert!(app.inline_interactive_state.is_some());
    });
}

#[test]
fn login_openai_arrows_toggle_highlight() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::LoginOpenAi {
                yes_highlighted: true,
            };
        }
        // Right highlights No, Left highlights Yes; nothing commits yet.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Right));
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::LoginOpenAi {
                yes_highlighted: false
            })
        ));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Left));
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::LoginOpenAi {
                yes_highlighted: true
            })
        ));
        assert!(app.inline_interactive_state.is_none());
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
fn no_external_transcripts_lands_on_suggestions_without_autosubmit() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::ContinuePrompt {
                cli: ExternalCli::Codex,
                yes_highlighted: true,
                shown_at: std::time::Instant::now(),
            };
        }
        // Temp home has no Codex transcripts, so opening the picker should land
        // the user on the clean new-session suggestion cards rather than
        // auto-submitting a "search for my last session" turn.
        app.onboarding_open_transcript_picker(&[ExternalCli::Codex]);
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Suggestions)
        ));
        assert!(app.session_picker_overlay.is_none());
        // It must NOT have queued/dispatched an agent turn.
        assert!(!app.pending_queued_dispatch);
        assert!(app.queued_messages.is_empty());
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
fn onboarding_picker_shows_both_codex_and_claude_transcripts() {
    use std::fs;
    with_temp_jcode_home(|| {
        // Seed one Codex transcript and one Claude Code transcript under the
        // sandbox-aware external home ($JCODE_HOME/external/...), mirroring a
        // user who is logged into BOTH CLIs.
        let home = std::env::var_os("JCODE_HOME").expect("JCODE_HOME");
        let external = std::path::Path::new(&home).join("external");

        let codex_dir = external.join(".codex/sessions/2026/04/05");
        fs::create_dir_all(&codex_dir).expect("codex dir");
        fs::write(
            codex_dir.join("rollout-2026-04-05T19-00-00-codextest.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-04-05T19:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019d-codex-both\",\"timestamp\":\"2026-04-05T18:59:00Z\",\"cwd\":\"/tmp/codex-demo\",\"source\":\"cli\"}}\n",
                "{\"timestamp\":\"2026-04-05T19:00:03Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"CODEX_MARKER fix the widget\"}]}}\n",
            ),
        )
        .expect("write codex transcript");

        let claude_dir = external.join(".claude/projects/demo-project");
        fs::create_dir_all(&claude_dir).expect("claude dir");
        fs::write(
            claude_dir.join("claude-session-both.jsonl"),
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"CLAUDE_MARKER fix the flaky test\"}]}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n"
            ),
        )
        .expect("write claude transcript");

        let mut app = onboarding_test_app();
        // Open the combined picker for BOTH detected CLIs.
        app.onboarding_open_transcript_picker(&[ExternalCli::Codex, ExternalCli::ClaudeCode]);

        // The picker overlay should be up with both CLIs' sessions visible
        // (not just one).
        let picker_cell = app
            .session_picker_overlay
            .as_ref()
            .expect("picker overlay should be open");
        let picker = picker_cell.borrow();
        assert!(
            picker.visible_session_count() >= 2,
            "combined picker should list both CLIs' sessions, got {}",
            picker.visible_session_count()
        );

        let mut saw_codex = false;
        let mut saw_claude = false;
        for session in picker.visible_session_iter_for_test() {
            match session.source {
                jcode_tui_session_picker::SessionSource::Codex => saw_codex = true,
                jcode_tui_session_picker::SessionSource::ClaudeCode => saw_claude = true,
                _ => {}
            }
        }
        assert!(saw_codex, "Codex session should be present in combined picker");
        assert!(
            saw_claude,
            "Claude Code session should be present in combined picker"
        );
    });
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
            Some(OnboardingPhase::Login { .. }) | Some(OnboardingPhase::LoginOpenAi { .. })
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

#[test]
fn startup_check_skips_selfdev_canary_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.onboarding_startup_checked = false;
        // Self-dev / canary sessions (e.g. the niri `jcode self-dev` hotkey) take
        // a launch path that never bumps `launch_count`, so without this guard the
        // new-user heuristic would re-onboard on every spawn.
        app.session.is_canary = true;

        app.maybe_begin_onboarding_flow_on_startup();

        assert!(app.onboarding_startup_checked);
        assert!(
            app.onboarding_flow.is_none(),
            "self-dev/canary sessions must never auto-start onboarding"
        );
    });
}

#[test]
fn model_validation_success_appends_single_ready_line() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    let before = app.display_messages().len();

    let consumed = app.handle_onboarding_model_validated(crate::bus::OnboardingModelValidated {
        session_id,
        model_label: "GPT-5.5 (low)".to_string(),
        provider_key: Some("openai".to_string()),
        ok: true,
        detail: None,
    });

    assert!(consumed);
    let messages = app.display_messages();
    assert_eq!(messages.len(), before + 1, "exactly one summary block");
    let line = &messages.last().unwrap().content;
    assert!(line.contains("Ready to use"), "has a ready section: {line:?}");
    assert!(
        line.contains("GPT-5.5 (low) (default)"),
        "names the default model: {line:?}"
    );
    assert!(
        line.contains('\u{2713}'),
        "marks ready rows with a check: {line:?}"
    );
}

#[test]
fn model_validation_failure_appends_single_warning_line_with_detail() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    let before = app.display_messages().len();

    let consumed = app.handle_onboarding_model_validated(crate::bus::OnboardingModelValidated {
        session_id,
        model_label: "Claude Opus 4.8".to_string(),
        provider_key: Some("anthropic".to_string()),
        ok: false,
        detail: Some("timed out after 30s".to_string()),
    });

    assert!(consumed);
    let messages = app.display_messages();
    assert_eq!(messages.len(), before + 1, "exactly one summary block");
    let line = &messages.last().unwrap().content;
    assert!(
        line.contains("Needs attention"),
        "has an attention section: {line:?}"
    );
    assert!(
        line.contains("Claude Opus 4.8 (default)"),
        "names the default model: {line:?}"
    );
    assert!(line.contains("timed out after 30s"), "includes detail: {line:?}");
    assert!(line.contains("/model"), "offers a way out: {line:?}");
    assert!(
        line.contains('\u{2715}'),
        "marks attention rows with a cross: {line:?}"
    );
}

#[test]
fn model_validation_auth_failure_offers_login_fix() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();

    let consumed = app.handle_onboarding_model_validated(crate::bus::OnboardingModelValidated {
        session_id,
        model_label: "Claude Opus 4.8".to_string(),
        provider_key: Some("anthropic".to_string()),
        ok: false,
        detail: Some(
            "Anthropic API error (401 Unauthorized): Invalid authentication credentials"
                .to_string(),
        ),
    });

    assert!(consumed);
    let messages = app.display_messages();
    let line = &messages.last().unwrap().content;
    // Auth failures should point the user at /login to re-authenticate, while
    // still offering /model as an alternative.
    assert!(line.contains("/login"), "auth failure offers /login: {line:?}");
    assert!(line.contains("/model"), "still offers /model: {line:?}");
}

#[test]
fn model_validation_ignores_stale_session_result() {
    let mut app = create_test_app();
    let before = app.display_messages().len();

    let consumed = app.handle_onboarding_model_validated(crate::bus::OnboardingModelValidated {
        session_id: "some-other-session".to_string(),
        model_label: "GPT-5.5".to_string(),
        provider_key: Some("openai".to_string()),
        ok: true,
        detail: None,
    });

    assert!(!consumed, "stale result is not consumed");
    assert_eq!(
        app.display_messages().len(),
        before,
        "stale result appends nothing"
    );
}

#[test]
fn remote_post_login_validation_waits_for_catalog_refresh() {
    use crate::tui::app::onboarding_flow::OnboardingPendingValidation;
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        // Simulate the state right after a remote login: a pending validation
        // armed to wait for the catalog generation to advance past 3.
        app.remote_model_catalog_generation = 3;
        app.onboarding_pending_model_validation = Some(
            OnboardingPendingValidation::awaiting_catalog_refresh(app.session.id.clone(), 3),
        );

        // Catalog hasn't refreshed yet (generation unchanged): not ready to fire.
        assert!(!app.onboarding_pending_validation_ready_to_fire());

        // The server pushes the post-login catalog (generation advances): now
        // the validation is ready to fire with the freshly-selected model.
        app.remote_model_catalog_generation = 4;
        assert!(app.onboarding_pending_validation_ready_to_fire());
    });
}

#[test]
fn login_welcome_kind_carries_not_found_rows() {
    use crate::external_auth::{AuthSearchTarget, ExternalAuthReviewCandidate};
    use crate::tui::OnboardingWelcomeKind;
    use crate::tui::app::onboarding_flow::ImportReview;

    let mut app = create_test_app();
    app.onboarding_flow = None;
    app.begin_onboarding_flow_at_login();

    // Arm a one-candidate import walkthrough and inject a not-found list.
    let review =
        ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("Cursor", "Cursor")]).unwrap();
    if let Some(flow) = app.onboarding_flow.as_mut() {
        flow.phase = OnboardingPhase::Login {
            import: Some(review),
        };
        flow.login_not_found = vec![
            AuthSearchTarget {
                family: "codex",
                label: "Codex".to_string(),
                path: "~/.codex/auth.json".to_string(),
                present: false,
            },
            AuthSearchTarget {
                family: "gemini_cli",
                label: "Gemini CLI".to_string(),
                path: "~/.gemini/oauth_creds.json".to_string(),
                present: false,
            },
        ];
    }

    match app.onboarding_welcome_kind() {
        OnboardingWelcomeKind::Login { not_found, .. } => {
            let labels: Vec<&str> = not_found.iter().map(|r| r.label.as_str()).collect();
            assert_eq!(labels, vec!["Codex", "Gemini CLI"]);
        }
        other => panic!("expected Login welcome, got {other:?}"),
    }
}

#[test]
fn not_found_panel_scrolls_with_pgdn_and_clamps() {
    use crate::external_auth::AuthSearchTarget;
    use crate::tui::app::onboarding_flow::OnboardingFlow;

    let mut app = create_test_app();
    // LoginOpenAi phase (no detected imports) with a long not-found list so the
    // panel overflows its visible window and becomes scrollable. 12 rows with a
    // 5-row visible window gives a max scroll offset of 7.
    let targets: Vec<AuthSearchTarget> = (0..12)
        .map(|i| AuthSearchTarget {
            family: "x",
            label: format!("Source {i}"),
            path: format!("~/path/{i}"),
            present: false,
        })
        .collect();
    let mut flow = OnboardingFlow::begin_at_login_with_not_found(None, targets);
    flow.phase = OnboardingPhase::LoginOpenAi {
        yes_highlighted: true,
    };
    app.onboarding_flow = Some(flow);
    assert_eq!(app.onboarding_notfound_scroll, 0);

    // PgDn scrolls down by the step (3).
    assert!(app.handle_onboarding_continue_prompt_key(KeyCode::PageDown));
    assert_eq!(app.onboarding_notfound_scroll, 3);
    assert!(app.handle_onboarding_continue_prompt_key(KeyCode::PageDown));
    assert_eq!(app.onboarding_notfound_scroll, 6);

    // Next PgDn clamps to the max offset (12 rows - 5 visible = 7).
    assert!(app.handle_onboarding_continue_prompt_key(KeyCode::PageDown));
    assert_eq!(app.onboarding_notfound_scroll, 7);

    // Already at the bottom: PgDn no longer consumes the key (falls through).
    assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::PageDown));

    // PgUp scrolls back up and eventually clamps at 0.
    assert!(app.handle_onboarding_continue_prompt_key(KeyCode::PageUp));
    assert_eq!(app.onboarding_notfound_scroll, 4);
    assert!(app.handle_onboarding_continue_prompt_key(KeyCode::PageUp));
    assert!(app.handle_onboarding_continue_prompt_key(KeyCode::PageUp));
    assert_eq!(app.onboarding_notfound_scroll, 0);
    assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::PageUp));
}

#[test]
fn not_found_panel_no_scroll_when_list_fits() {
    use crate::external_auth::AuthSearchTarget;
    use crate::tui::app::onboarding_flow::OnboardingFlow;

    let mut app = create_test_app();
    // Only 3 rows: fits within the visible window, so PgDn is a no-op and the
    // key falls through (not consumed) for other handlers.
    let targets: Vec<AuthSearchTarget> = (0..3)
        .map(|i| AuthSearchTarget {
            family: "x",
            label: format!("Source {i}"),
            path: format!("~/path/{i}"),
            present: false,
        })
        .collect();
    let mut flow = OnboardingFlow::begin_at_login_with_not_found(None, targets);
    flow.phase = OnboardingPhase::LoginOpenAi {
        yes_highlighted: true,
    };
    app.onboarding_flow = Some(flow);
    assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::PageDown));
    assert_eq!(app.onboarding_notfound_scroll, 0);
}

#[test]
fn scrollwm_optin_gate_logic() {
    use super::onboarding_flow_control::scrollwm_optin_should_offer;
    // Offered only on macOS, local, not installed, not answered.
    assert!(scrollwm_optin_should_offer(true, false, false, false));
    // Suppressed when already installed.
    assert!(!scrollwm_optin_should_offer(true, false, true, false));
    // Suppressed when already answered.
    assert!(!scrollwm_optin_should_offer(true, false, false, true));
    // Suppressed in remote sessions.
    assert!(!scrollwm_optin_should_offer(true, true, false, false));
    // Suppressed off macOS.
    assert!(!scrollwm_optin_should_offer(false, false, false, false));
}

#[test]
fn scrollwm_optin_enter_defaults_to_no() {
    let mut app = onboarding_test_app();
    app.onboarding_enter_scrollwm_optin();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::ScrollWmOptIn {
            yes_highlighted: false,
            ..
        })
    ));
}

#[test]
fn scrollwm_optin_no_advances_to_suggestions_and_persists() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        app.onboarding_enter_scrollwm_optin();
        app.onboarding_answer_scrollwm_optin(false);
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Suggestions)
        ));
        // The answer is persisted so we never re-ask.
        assert!(crate::setup_hints::SetupHintsState::load().scrollwm_optin_answered);
    });
}

#[test]
fn scrollwm_optin_yes_sets_running_progress() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        app.onboarding_enter_scrollwm_optin();
        app.onboarding_answer_scrollwm_optin(true);
        // Stays on the opt-in card with a Running progress line; the install
        // runs in the background and is resolved via the Bus event.
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::ScrollWmOptIn { .. })
        ));
        let state = crate::setup_hints::SetupHintsState::load();
        assert!(state.scrollwm_optin_answered);
        assert!(state.scrollwm_install_started);
    });
}

#[test]
fn scrollwm_install_completed_advances_to_suggestions() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        app.onboarding_enter_scrollwm_optin();
        app.onboarding_answer_scrollwm_optin(true);
        let session_id = app.session.id.clone();
        let consumed = app.handle_scrollwm_install_completed(
            crate::bus::ScrollWmInstallCompleted {
                session_id,
                ok: true,
                detail: None,
            },
        );
        assert!(consumed);
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Suggestions)
        ));
    });
}

#[test]
fn scrollwm_optin_key_yes_no_toggle_and_skip() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        app.onboarding_enter_scrollwm_optin();
        // Left highlights Yes; Right highlights No.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Left));
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::ScrollWmOptIn {
                yes_highlighted: true,
                ..
            })
        ));
        // 'n' skips and advances to suggestions.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Suggestions)
        ));
    });
}
