// Integration tests for the first-run onboarding flow control logic.

use super::onboarding_flow::{ExternalCli, OnboardingFlow, OnboardingPhase};

#[derive(Clone)]
struct QualityFirstOpenAiProvider {
    model: std::sync::Arc<std::sync::RwLock<String>>,
}

#[async_trait::async_trait]
impl Provider for QualityFirstOpenAiProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("QualityFirstOpenAiProvider")
    }

    fn name(&self) -> &str {
        "OpenAI"
    }

    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![
            crate::provider::ModelRoute {
                model: jcode_provider_core::DEFAULT_CLAUDE_MODEL.to_string(),
                provider: "Anthropic".to_string(),
                api_method: "claude-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "gpt-5.1".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-api-key".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: jcode_provider_core::DEFAULT_OPENAI_MODEL.to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-api-key".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            },
        ]
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let bare = model.rsplit_once(':').map_or(model, |(_, bare)| bare);
        *self.model.write().unwrap() = bare.to_string();
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn quality_first_openai_test_app() -> App {
    let provider: Arc<dyn Provider> = Arc::new(QualityFirstOpenAiProvider {
        model: std::sync::Arc::new(std::sync::RwLock::new("gpt-5.1".to_string())),
    });
    let runtime = tokio::runtime::Runtime::new().expect("registry runtime");
    let registry = runtime.block_on(crate::tool::Registry::new(provider.clone()));
    App::new_for_test_harness(provider, registry)
}

fn onboarding_test_app() -> App {
    let mut app = create_test_app();
    // Force the flow on regardless of the on-disk new-user heuristic.
    app.onboarding_flow = Some(OnboardingFlow::begin());
    app
}

#[test]
fn onboarding_strongest_model_only_runs_without_explicit_defaults() {
    with_temp_jcode_home(|| {
        let previous_explicit = std::env::var_os("JCODE_INITIAL_PROVIDER_EXPLICIT");
        crate::env::remove_var("JCODE_INITIAL_PROVIDER_EXPLICIT");

        let mut app = onboarding_test_app();
        assert!(app.onboarding_should_prefer_strongest_model());

        let mut config = crate::config::Config::load();
        config.provider.default_model = Some("claude-fable-5".to_string());
        config.save().expect("save explicit model default");
        assert!(!app.onboarding_should_prefer_strongest_model());

        config.provider.default_model = None;
        config.provider.default_provider = Some("openai".to_string());
        config.save().expect("save explicit provider default");
        assert!(!app.onboarding_should_prefer_strongest_model());

        config.provider.default_provider = None;
        config.save().expect("clear explicit defaults");
        crate::env::set_var("JCODE_INITIAL_PROVIDER_EXPLICIT", "1");
        assert!(!app.onboarding_should_prefer_strongest_model());

        app.onboarding_auto_model_selection_active
            .store(true, std::sync::atomic::Ordering::Release);
        app.onboarding_finish();
        assert!(
            !app.onboarding_auto_model_selection_active
                .load(std::sync::atomic::Ordering::Acquire),
            "finishing onboarding must cancel a delayed catalog selection"
        );

        if let Some(value) = previous_explicit {
            crate::env::set_var("JCODE_INITIAL_PROVIDER_EXPLICIT", value);
        } else {
            crate::env::remove_var("JCODE_INITIAL_PROVIDER_EXPLICIT");
        }
    });
}

#[test]
fn onboarding_begins_and_advances_past_model_select() {
    let mut app = create_test_app();
    app.onboarding_flow = None;
    app.begin_onboarding_flow();
    // `begin_onboarding_flow` immediately advances past the legacy ModelSelect
    // phase into the action-only start choice.
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::StartChoice { .. })
    ));
    // begin is idempotent: a second call does not reset the phase.
    app.begin_onboarding_flow();
    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::StartChoice { .. })
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
fn login_welcome_kind_shows_import_checkbox_list() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::OnboardingWelcomeKind;
    use crate::tui::app::onboarding_flow::ImportReview;

    let mut app = create_test_app();
    app.onboarding_flow = None;
    app.begin_onboarding_flow_at_login();
    // Inject a multi-select import list as if external logins were detected at
    // startup.
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
            assert_eq!(prompt.rows.len(), 2);
            assert_eq!(prompt.rows[0].provider_summary, "OpenAI/Codex");
            assert_eq!(prompt.rows[0].source_name, "Codex auth.json");
            // Every login is pre-checked by default.
            assert!(prompt.rows.iter().all(|r| r.checked));
            assert_eq!(prompt.checked_count, 2);
            assert_eq!(prompt.cursor, 0);
        }
        other => panic!("expected Login welcome with import prompt, got {other:?}"),
    }
}

#[test]
fn import_review_collects_checked_logins() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    let mut review = ImportReview::new(vec![
        ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
        ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
        ExternalAuthReviewCandidate::fixture("Gemini", "Gemini CLI"),
    ])
    .unwrap();
    // The default is the summary screen with Continue preselected.
    assert!(!review.choosing);
    assert!(review.continue_focused);
    assert_eq!(review.total(), 3);
    // All pre-checked: the default action imports everything.
    assert_eq!(review.approved_indices(), vec![0, 1, 2]);
    assert_eq!(review.checked_count(), 3);

    // Switch to the checkbox list and uncheck the middle login (cursor on row 1).
    review.enter_choose_mode();
    assert!(review.choosing);
    assert_eq!(review.position(), 1);
    review.cursor_down();
    review.toggle_current();
    assert_eq!(review.approved_indices(), vec![0, 2]);
    assert_eq!(review.checked_count(), 2);
}

#[test]
fn import_review_cursor_navigation_wraps() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    let mut review = ImportReview::new(vec![
        ExternalAuthReviewCandidate::fixture("Cursor", "Cursor"),
        ExternalAuthReviewCandidate::fixture("Gemini", "Gemini CLI"),
    ])
    .unwrap();
    review.enter_choose_mode();
    assert_eq!(review.position(), 1);
    assert!(!review.continue_focused);
    review.cursor_down();
    assert_eq!(review.position(), 2);
    assert!(!review.continue_focused);
    // Down past the last row lands on the navigable Continue pill.
    review.cursor_down();
    assert!(review.continue_focused);
    // Down again wraps from Continue to the first row.
    review.cursor_down();
    assert!(!review.continue_focused);
    assert_eq!(review.position(), 1);
    // Up from the first row lands on the Continue pill.
    review.cursor_up();
    assert!(review.continue_focused);
    // Up from Continue lands on the last row.
    review.cursor_up();
    assert!(!review.continue_focused);
    assert_eq!(review.position(), 2);
    // Toggling the current row flips just that row.
    assert!(review.current_checked());
    review.toggle_current();
    assert!(!review.current_checked());
    // Toggling while Continue is focused is a no-op (no row changes).
    review.cursor_down(); // back onto Continue
    assert!(review.continue_focused);
    let before = review.approved_indices();
    review.toggle_current();
    assert_eq!(review.approved_indices(), before);
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
        // straight through model selection into the first-run start choice and
        // leave content sharing off.
        app.onboarding_after_login();
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::ModelSelect)
                | Some(OnboardingPhase::Suggestions)
                | Some(OnboardingPhase::StartChoice { .. })
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
                yes_highlighted: true
            }
        ));
    });
}

#[test]
fn login_openai_no_finishes_onboarding_with_login_hint() {
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
        let before = app.display_messages().len();
        // 'n' exits onboarding straight to the normal screen (no flaky inline
        // provider picker) and tells the user to run /login when ready.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
        // No inline picker is opened.
        assert!(app.inline_interactive_state.is_none());
        // Onboarding is finished (Done phase is inactive, so the accessor
        // reports no active phase).
        assert!(app.onboarding_phase().is_none());
        assert!(!app.onboarding_flow_active());
        // A system message guides the user to /login.
        let messages = app.display_messages();
        assert_eq!(messages.len(), before + 1, "exactly one guidance message");
        assert!(
            messages.last().unwrap().content.contains("/login"),
            "guidance message should mention /login: {:?}",
            messages.last().unwrap().content
        );
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
fn pending_login_entry_is_not_intercepted_by_onboarding_login_phase() {
    // Regression for the OpenRouter (and any API-key provider) login loop:
    // after selecting a provider during onboarding, the Login phase stays
    // active while the user types their API key. Pressing Enter to submit the
    // key must NOT be intercepted by the onboarding welcome-screen handler
    // (which would re-open the provider picker), and key characters must not be
    // swallowed as Yes/No navigation.
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        // Simulate having chosen OpenRouter: the picker closed and a pending
        // API-key login prompt is now active.
        app.inline_interactive_state = None;
        app.start_login_provider(crate::provider_catalog::resolve_login_provider("openrouter").unwrap());
        assert!(app.pending_login.is_some());
        assert!(app.inline_interactive_state.is_none());

        // Enter must fall through to the normal input/pending-login handler
        // instead of re-opening the provider picker.
        assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(app.inline_interactive_state.is_none());
        // Letters that double as Yes/No navigation must also fall through.
        assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::Char('y')));
        assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
    });
}

#[test]
fn openrouter_key_typed_through_full_key_path_does_not_reopen_picker() {
    // End-to-end regression for the OpenRouter login loop, driven through the
    // real production key dispatch (`handle_key`) instead of calling the
    // onboarding helper directly. This reproduces exactly what the user does:
    // they are mid-onboarding (Login phase still active), a pending API-key
    // login prompt is showing, and they type a key like "sk-or-..." then press
    // Enter. Before the fix, the onboarding welcome handler intercepted the
    // typed characters (y/n/h/l/j/k as Yes/No nav) and Enter (re-opening the
    // provider picker), creating the infinite loop. Now every keystroke must
    // flow to the input buffer and Enter must submit the key.
    use crossterm::event::{KeyCode, KeyModifiers};

    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        // Simulate having chosen OpenRouter from the picker: the picker is closed
        // and a pending API-key login prompt is active.
        app.inline_interactive_state = None;
        app.start_login_provider(
            crate::provider_catalog::resolve_login_provider("openrouter").unwrap(),
        );
        assert!(app.pending_login.is_some());
        assert!(app.inline_interactive_state.is_none());

        // Type a fake key. It deliberately contains characters that doubled as
        // Yes/No navigation in the buggy code path (k, n, l, y) to prove they
        // are no longer swallowed.
        let key = "sk-or-key-no-loop";
        for ch in key.chars() {
            app.handle_key(KeyCode::Char(ch), KeyModifiers::NONE).unwrap();
            // The picker must never re-open while typing.
            assert!(
                app.inline_interactive_state.is_none(),
                "picker re-opened while typing '{ch}'"
            );
        }
        assert_eq!(app.input, key, "every typed character must reach the input buffer");

        // Pressing Enter submits the key to the pending-login handler instead of
        // re-opening the provider picker (the old loop).
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
        assert!(
            app.pending_login.is_none(),
            "Enter must consume the pending login, not bounce back to the picker"
        );
        assert!(
            app.inline_interactive_state.is_none(),
            "Enter must not re-open the provider picker"
        );
        assert!(app.input.is_empty(), "input buffer should clear after submit");

        // Crucially: the key must actually be *persisted*, not just "not loop".
        // It is written to $JCODE_HOME/config/jcode/openrouter.env and exported
        // to OPENROUTER_API_KEY so the provider can authenticate.
        let env_file = crate::storage::app_config_dir().unwrap().join("openrouter.env");
        let contents = std::fs::read_to_string(&env_file)
            .unwrap_or_else(|e| panic!("openrouter.env should exist at {env_file:?}: {e}"));
        assert!(
            contents.contains(&format!("OPENROUTER_API_KEY={key}")),
            "saved env file must contain the typed key, got:\n{contents}"
        );
        assert_eq!(
            std::env::var("OPENROUTER_API_KEY").ok().as_deref(),
            Some(key),
            "key must be exported to the process env for immediate use"
        );
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
        app.onboarding_handle_login_failed(Some("Auto import failed: token expired".to_string()));
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
        let mut review = ImportReview::new(vec![ExternalAuthReviewCandidate::fixture(
            "OpenAI/Codex",
            "Codex auth.json",
        )])
        .unwrap();
        // Start in choose mode: this test exercises the per-login decline path.
        review.enter_choose_mode();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login {
                import: Some(review),
            };
        }
        // Uncheck the only login ("n"), then commit with Enter. With nothing
        // checked we don't spawn an import, the list clears, and the card falls
        // back to the manual-login prompt.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
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
        // It either opened the picker (StartChoice) or fell back depending on
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
fn onboarding_start_choice_is_action_only_and_defaults_to_review() {
    let mut app = onboarding_test_app();
    if let Some(flow) = app.onboarding_flow.as_mut() {
        flow.phase = OnboardingPhase::ModelSelect;
    }

    app.onboarding_after_model_select();

    assert!(matches!(
        app.onboarding_phase(),
        Some(OnboardingPhase::StartChoice { .. })
    ));
    assert_eq!(app.session_picker_mode, SessionPickerMode::Onboarding);
    let picker = app
        .session_picker_overlay
        .as_ref()
        .expect("start choice picker")
        .borrow();
    assert_eq!(picker.visible_session_count(), 0);
    assert!(picker.onboarding_review_recent_project_highlighted());
    assert!(!picker.onboarding_start_new_highlighted());
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
        // Parallel tests can leak credential env vars (ANTHROPIC_API_KEY etc.),
        // which legitimately routes the fresh install through the credentialed
        // post-login path instead. Either way, the flow must have *started*:
        // scaffolding messages must not be mistaken for real activity.
        assert!(
            !app.display_messages.is_empty(),
            "precondition: scaffolding messages present"
        );
        assert!(app.onboarding_startup_checked);
        assert!(
            app.onboarding_flow_active(),
            "scaffolding-only sessions must still enter first-run onboarding"
        );
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
fn local_post_import_validation_waits_for_model_activation() {
    use crate::tui::app::onboarding_flow::OnboardingPendingValidation;
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = false;
        app.auth_catalog_refresh_pending = true;
        app.onboarding_pending_model_validation = Some(
            OnboardingPendingValidation::awaiting_catalog_refresh(app.session.id.clone(), 0),
        );

        assert!(
            !app.onboarding_pending_validation_ready_to_fire(),
            "Continue must not validate the stale pre-import model"
        );

        app.auth_catalog_refresh_pending = false;
        assert!(
            app.onboarding_pending_validation_ready_to_fire(),
            "validation should start once local provider/model activation finishes"
        );
    });
}

#[test]
fn startup_check_skips_user_with_established_session_history() {
    with_temp_jcode_home(|| {
        // A low/missing launch_count alone must NOT classify someone as a new
        // user when their jcode home has a substantial native session history
        // (e.g. setup_hints.json was reset or lost). Seed >=10 native session
        // files in the temp home.
        let sessions_dir = crate::storage::jcode_dir()
            .expect("jcode dir")
            .join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        for i in 0..10 {
            std::fs::write(
                sessions_dir.join(format!("session_test_{i:02}.json")),
                "{}",
            )
            .expect("write session file");
        }

        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.onboarding_startup_checked = false;

        app.maybe_begin_onboarding_flow_on_startup();

        assert!(app.onboarding_startup_checked);
        assert!(
            app.onboarding_flow.is_none(),
            "established users (many native sessions) must never re-onboard"
        );
    });
}

#[test]
fn startup_check_imported_transcripts_do_not_count_as_history() {
    with_temp_jcode_home(|| {
        // Imported Codex/Claude transcripts exist on genuinely fresh installs
        // that chose to import history; they must not suppress onboarding.
        let sessions_dir = crate::storage::jcode_dir()
            .expect("jcode dir")
            .join("sessions");
        std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");
        for i in 0..20 {
            std::fs::write(
                sessions_dir.join(format!("imported_codex_{i:02}.json")),
                "{}",
            )
            .expect("write imported file");
        }

        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.onboarding_startup_checked = false;

        app.maybe_begin_onboarding_flow_on_startup();

        assert!(app.onboarding_startup_checked);
        assert!(
            app.onboarding_flow.is_some(),
            "imported transcripts alone should still onboard a fresh install"
        );
    });
}

// ---------------------------------------------------------------------------
// Liveness: a first-run user can never be permanently stranded.
//
// The dangerous failure mode is a phase whose only exit depends on an external
// async event (a `LoginCompleted` bus message) that might never arrive. These
// tests prove that from every reachable phase there is *always* a forward path
// using only inputs the user is guaranteed to have: a key press, or the passage
// of time via the tick watchdog. No test here depends on an async event firing.
// ---------------------------------------------------------------------------

/// A phase is a "safe resting/exit state" if the user is no longer trapped by
/// the guided flow: onboarding finished (`None`/`Done`), they reached a ready
/// surface (`Suggestions`/`StartChoice`), or an interactive picker overlay is
/// open for them to act in.
fn onboarding_state_is_escapable(app: &App) -> bool {
    use crate::tui::app::onboarding_flow::OnboardingPhase;
    if app.inline_interactive_state.is_some() || app.session_picker_overlay.is_some() {
        return true;
    }
    match app.onboarding_phase() {
        None => true, // flow finished / inactive
        Some(OnboardingPhase::Suggestions) => true,
        Some(OnboardingPhase::StartChoice { .. }) => true,
        Some(OnboardingPhase::Done) => true,
        _ => false,
    }
}

#[test]
fn liveness_every_login_phase_has_a_single_keypress_exit() {
    use crate::tui::app::onboarding_flow::OnboardingPhase;
    with_temp_jcode_home(|| {
        // Each interactive Login-family phase must leave itself after exactly one
        // decisive key, with no dependence on an async event. We use the "skip /
        // decline" key, which is always synchronous (it never spawns an import).
        let cases: Vec<(&str, OnboardingPhase, KeyCode)> = vec![
            // OpenAI prompt: "n" declines and finishes onboarding immediately.
            (
                "LoginOpenAi",
                OnboardingPhase::LoginOpenAi {
                    yes_highlighted: true,
                },
                KeyCode::Char('n'),
            ),
            // Recovery fallback: Enter opens the provider picker overlay.
            (
                "Login{import:None}",
                OnboardingPhase::Login { import: None },
                KeyCode::Enter,
            ),
        ];
        for (label, phase, key) in cases {
            let mut app = create_test_app();
            app.onboarding_flow = None;
            app.begin_onboarding_flow_at_login();
            if let Some(flow) = app.onboarding_flow.as_mut() {
                flow.phase = phase;
            }
            assert!(
                !onboarding_state_is_escapable(&app),
                "{label}: precondition - should start trapped in the flow"
            );
            let consumed = app.handle_onboarding_continue_prompt_key(key);
            assert!(consumed, "{label}: the exit key must be consumed");
            assert!(
                onboarding_state_is_escapable(&app),
                "{label}: one key press must reach an escapable state"
            );
        }
    });
}

#[test]
fn liveness_import_review_decline_all_then_enter_escapes() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::{ImportReview, OnboardingPhase};
    with_temp_jcode_home(|| {
        // The import list is the richest interactive phase. Declining every login
        // ("n") then committing (Enter) must never spawn an async import (so it
        // can't hang) and must land on the recovery screen, from which a final
        // Enter opens the provider picker. Whole path is synchronous.
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        let mut review = ImportReview::new(vec![
            ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
            ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
        ])
        .unwrap();
        // Start in choose mode: this liveness path declines each login row.
        review.enter_choose_mode();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login {
                import: Some(review),
            };
        }
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Down));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('n')));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        // No async import was spawned (declined all), so we are not stuck on the
        // progress screen; we are on the recovery screen.
        assert!(app.onboarding_import_in_progress.is_none());
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { import: None })
        ));
        // Final Enter opens the provider picker -> escapable.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(
            onboarding_state_is_escapable(&app),
            "recovery screen + Enter must open the provider picker"
        );
    });
}

#[test]
fn liveness_stuck_import_is_recovered_by_the_tick_watchdog() {
    use crate::tui::app::onboarding_flow::OnboardingPhase;
    with_temp_jcode_home(|| {
        // Simulate the dangerous state: the import was committed (progress screen
        // showing) but its `LoginCompleted` event never arrived. The flow sits in
        // Login{import:None} with `onboarding_import_in_progress` set. Without the
        // watchdog the user is stranded forever. We backdate the start time past
        // the watchdog window and assert a single tick recovers the flow into the
        // failure-aware recovery screen (which has a guaranteed keypress exit).
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        // Enter the "importing" wait, backdated so the watchdog fires immediately.
        app.onboarding_import_in_progress =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(120));
        app.onboarding_import_error = None;

        // Precondition: with the import flag set and no error yet, the screen
        // shows progress and offers no keypress exit.
        assert!(app.onboarding_import_in_progress.is_some());

        let changed = app.onboarding_tick();
        assert!(changed, "watchdog tick should change state");
        // Recovered: no longer "importing", and an error is set so the recovery
        // screen explains what happened.
        assert!(
            app.onboarding_import_in_progress.is_none(),
            "watchdog must clear the stuck import progress flag"
        );
        assert!(
            app.onboarding_import_error.is_some(),
            "watchdog recovery must surface a failure reason to the user"
        );
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { import: None })
        ));
        // And from there a single Enter still reaches the provider picker.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(onboarding_state_is_escapable(&app));
    });
}

#[test]
fn liveness_esc_always_exits_onboarding_from_every_guided_phase() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::{ImportReview, OnboardingPhase};
    with_temp_jcode_home(|| {
        // The universal escape hatch: from ANY guided pre-ready phase, a single
        // Esc must leave onboarding to the normal screen. This is the strongest
        // liveness guarantee - it doesn't matter how the flow got wedged, Esc
        // always works. We cover every interactive/transient phase, including the
        // async "importing" wait (where Esc must abandon the in-flight import).
        let make_import = || {
            ImportReview::new(vec![ExternalAuthReviewCandidate::fixture(
                "OpenAI/Codex",
                "Codex auth.json",
            )])
            .unwrap()
        };
        let phases: Vec<(&str, OnboardingPhase, bool)> = vec![
            (
                "LoginOpenAi",
                OnboardingPhase::LoginOpenAi {
                    yes_highlighted: true,
                },
                false,
            ),
            (
                "Login{import:Some}",
                OnboardingPhase::Login {
                    import: Some(make_import()),
                },
                false,
            ),
            (
                "Login{import:None} recovery",
                OnboardingPhase::Login { import: None },
                false,
            ),
            // The async "importing" wait: import committed, LoginCompleted not yet
            // arrived. Esc must still bail out cleanly.
            (
                "Login importing wait",
                OnboardingPhase::Login { import: None },
                true,
            ),
            ("ModelSelect", OnboardingPhase::ModelSelect, false),
            (
                "ContinuePrompt",
                OnboardingPhase::ContinuePrompt {
                    cli: ExternalCli::Codex,
                    yes_highlighted: true,
                    shown_at: std::time::Instant::now(),
                },
                false,
            ),
        ];
        for (label, phase, importing) in phases {
            let mut app = create_test_app();
            app.onboarding_flow = None;
            app.begin_onboarding_flow_at_login();
            if let Some(flow) = app.onboarding_flow.as_mut() {
                flow.phase = phase;
            }
            if importing {
                app.onboarding_import_in_progress = Some(std::time::Instant::now());
            }
            assert!(
                !onboarding_state_is_escapable(&app),
                "{label}: precondition - should start trapped in the flow"
            );
            let consumed = app.handle_onboarding_continue_prompt_key(KeyCode::Esc);
            assert!(consumed, "{label}: Esc must be consumed");
            assert!(
                onboarding_state_is_escapable(&app),
                "{label}: Esc must reach an escapable state"
            );
            // Esc must not leave a stale import-progress flag spinning.
            assert!(
                app.onboarding_import_in_progress.is_none(),
                "{label}: Esc must clear any in-flight import progress"
            );
        }
    });
}

#[test]
fn import_failure_reason_is_cleaned_and_capitalized() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::{ImportReview, OnboardingPhase};
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        // Must be in the Login phase for the failure handler to apply.
        let review =
            ImportReview::new(vec![ExternalAuthReviewCandidate::fixture("Cursor", "Cursor")])
                .unwrap();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login {
                import: Some(review),
            };
        }
        // A multi-line markdown failure message with marker noise and a
        // lowercase first word, mimicking the importer's render_markdown output.
        let raw = "**Logins imported**\n\nthe token has expired\n- \u{2715} Cursor (from Cursor): bad";
        app.onboarding_handle_login_failed(Some(raw.to_string()));
        let shown = app
            .onboarding_import_error
            .as_deref()
            .expect("failure reason should be recorded");
        // Markdown bold headers and the "Logins imported" line are stripped; the
        // first meaningful line is kept, marker trimmed, first letter uppercased.
        assert!(!shown.contains("**"), "markdown bold stripped: {shown}");
        assert!(!shown.contains('\u{2715}'), "marker glyph stripped: {shown}");
        assert!(
            shown.starts_with("The token has expired"),
            "first meaningful line kept + capitalized: {shown}"
        );
    });
}

#[test]
fn import_failure_h_key_prepares_agent_repair_brief() {
    use crate::tui::app::onboarding_flow::OnboardingPhase;
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        // Simulate a failed import that recorded a reason.
        app.onboarding_import_error = Some("the saved credential was rejected".to_string());
        app.onboarding_import_failed_provider = Some("openai".to_string());
        let before = app.display_messages.len();

        // H on the failure screen prepares the agent repair brief.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Char('H')));

        // A brief was pushed into the transcript with the agent-runnable
        // commands and the failure reason, so it works even without a clipboard.
        assert!(app.display_messages.len() > before, "brief message pushed");
        let brief = app
            .display_messages
            .iter()
            .rev()
            .find(|m| m.content.contains("Agent repair brief"))
            .map(|m| m.content.clone())
            .expect("repair brief message");
        assert!(brief.contains("jcode auth-test --provider openai --json"), "{brief}");
        assert!(brief.contains("--api-key-stdin"), "{brief}");
        assert!(brief.contains("the saved credential was rejected"), "{brief}");
        // The brief was also persisted to a stable path a helper agent can read.
        let brief_path = crate::tui::app::onboarding_repair::repair_brief_path()
            .expect("repair brief path");
        assert!(brief_path.exists(), "brief file should be written: {brief_path:?}");
        let on_disk = std::fs::read_to_string(&brief_path).expect("read brief file");
        assert!(on_disk.contains("jcode auth-test --provider openai --json"), "{on_disk}");
        assert!(brief.contains(&brief_path.display().to_string()), "brief cites its own path");
        // Staying on the recovery screen, Enter still opens the provider picker.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(app.inline_interactive_state.is_some());
    });
}

#[test]
fn import_failure_h_key_is_inert_without_a_recorded_error() {
    use crate::tui::app::onboarding_flow::OnboardingPhase;
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login { import: None };
        }
        // Recovery screen reached by declining all (no error reason recorded):
        // H must NOT be intercepted, so normal input handling can use it.
        app.onboarding_import_error = None;
        assert!(!app.handle_onboarding_continue_prompt_key(KeyCode::Char('H')));
    });
}

#[test]
fn import_summary_defaults_to_continue_and_enter_imports_all() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
        let review = ImportReview::new(vec![
            ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json"),
            ExternalAuthReviewCandidate::fixture("Claude", "Claude Code"),
        ])
        .unwrap();
        // The summary screen is the default and lands on Continue.
        assert!(!review.choosing);
        assert!(review.continue_focused);
        if let Some(flow) = app.onboarding_flow.as_mut() {
            flow.phase = OnboardingPhase::Login {
                import: Some(review),
            };
        }
        // Enter on the preselected Continue commits the whole import: the list
        // clears. On a live runtime the async import is marked in-flight; the
        // test harness has no tokio runtime, so the graceful fallback lands on
        // the recovery screen instead (never a panic, never a stuck screen).
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        assert!(matches!(
            app.onboarding_phase(),
            Some(OnboardingPhase::Login { import: None })
        ));
        assert!(
            app.onboarding_import_in_progress.is_some() || app.onboarding_import_error.is_some(),
            "Continue must either start the import or fail it gracefully"
        );
    });
}

#[test]
fn import_continue_reaches_ready_quality_first_openai_model() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    with_temp_jcode_home(|| {
        let legacy_auth = crate::auth::codex::legacy_auth_file_path().expect("legacy auth path");
        std::fs::create_dir_all(legacy_auth.parent().expect("legacy auth parent"))
            .expect("create legacy auth dir");
        std::fs::write(
            legacy_auth,
            r#"{"OPENAI_API_KEY":"sk-onboarding-test"}"#,
        )
        .expect("seed importable Codex key");
        crate::auth::AuthStatus::invalidate_cache();

        // App construction performs synchronous runtime-backed setup, so build
        // it before entering the async test runtime to avoid nested `block_on`.
        let mut app = quality_first_openai_test_app();
        let runtime = tokio::runtime::Runtime::new().expect("test runtime");
        runtime.block_on(async {
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

            let mut bus_rx = crate::bus::Bus::global().subscribe();
            assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));

            let login = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if let Ok(crate::bus::BusEvent::LoginCompleted(login)) = bus_rx.recv().await {
                        break login;
                    }
                }
            })
            .await
            .expect("import completion event");
            assert!(
                login.success,
                "Continue should complete the approved import: {}",
                login.message
            );
            assert_eq!(
                login.provider, "openai-api",
                "import completion must preserve the concrete provider route"
            );
            assert!(
                app.onboarding_should_prefer_strongest_model(),
                "first-run import without explicit defaults should use global ranking"
            );

            app.handle_login_completed(login);
            assert!(matches!(
                app.onboarding_phase(),
                Some(OnboardingPhase::StartChoice { .. })
            ));
            let (model, provider_key) = tokio::time::timeout(
                std::time::Duration::from_secs(4),
                async {
                    loop {
                        match bus_rx.recv().await {
                            Ok(crate::bus::BusEvent::ProviderModelActivated {
                                model,
                                provider_key,
                                ..
                            }) => break (model, provider_key),
                            Ok(crate::bus::BusEvent::AuthCatalogRefreshReady) => {
                                app.finish_auth_catalog_refresh();
                            }
                            _ => {}
                        }
                    }
                },
            )
            .await
            .expect("strongest model activation event");

            assert_eq!(model, jcode_provider_core::DEFAULT_OPENAI_MODEL);
            assert_eq!(provider_key.as_deref(), Some("openai-api"));
        });
    });
}

#[test]
fn import_summary_choose_pill_opens_checkbox_list() {
    use crate::external_auth::ExternalAuthReviewCandidate;
    use crate::tui::app::onboarding_flow::ImportReview;

    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.onboarding_flow = None;
        app.begin_onboarding_flow_at_login();
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
        // Arrow to the "Choose what to import" pill, then commit it.
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Down));
        assert!(app.handle_onboarding_continue_prompt_key(KeyCode::Enter));
        // Now in choose mode: the checkbox list with the cursor on row 1 and
        // nothing imported yet.
        match app.onboarding_phase() {
            Some(OnboardingPhase::Login {
                import: Some(review),
            }) => {
                assert!(review.choosing);
                assert!(!review.continue_focused);
                assert_eq!(review.cursor, 0);
                assert_eq!(review.checked_count(), 2);
            }
            other => panic!("expected choose-mode import review, got {other:?}"),
        }
        assert!(app.onboarding_import_in_progress.is_none());

        // The welcome snapshot reports choose mode so the renderer switches.
        match app.onboarding_welcome_kind() {
            crate::tui::OnboardingWelcomeKind::Login {
                import: Some(prompt),
                ..
            } => assert!(prompt.choosing),
            other => panic!("expected Login welcome with import prompt, got {other:?}"),
        }
    });
}

#[test]
fn recent_project_review_prompt_is_bounded_read_only_and_requires_approval() {
    let repository = std::path::Path::new("/home/example/projects/demo");
    let prompt = App::onboarding_recent_project_review_prompt(repository);

    assert_eq!(
        prompt,
        "Find the most critical architecture problems in the repository at \"/home/example/projects/demo\". Do not fix them yet, and ask me whether I want them fixed once you find them."
    );
}

#[test]
fn preparing_recent_project_review_finishes_onboarding_and_seeds_the_first_turn() {
    let mut app = onboarding_test_app();
    let repository = app
        .onboarding_recent_project_path()
        .expect("test session should start in a Git repository");
    let expected = App::onboarding_recent_project_review_prompt(&repository);

    assert!(app.onboarding_prepare_recent_project_review());

    assert!(!app.onboarding_flow_active());
    assert_eq!(app.input, expected);
    assert_eq!(app.cursor_pos, app.input.len());
}

#[test]
fn starting_recent_project_review_runs_as_a_visible_local_turn() {
    let mut app = onboarding_test_app();
    let repository = app
        .onboarding_recent_project_path()
        .expect("test session should start in a Git repository");
    let expected = App::onboarding_recent_project_review_prompt(&repository);

    app.onboarding_start_recent_project_review();

    assert!(!app.onboarding_flow_active());
    assert!(app.pending_turn, "local review should start a local turn");
    assert!(app.is_processing, "local review should enter Sending");
    assert!(app.queued_messages.is_empty());
    assert_eq!(
        app.session.messages.last().and_then(|message| {
            message.content.iter().find_map(|block| match block {
                crate::message::ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
        }),
        Some(expected.as_str())
    );
}

#[test]
fn starting_recent_project_review_queues_remote_turn_without_stuck_sending() {
    let mut app = onboarding_test_app();
    app.is_remote = true;
    let repository = app
        .onboarding_recent_project_path()
        .expect("remote session should provide its working directory");
    let expected = App::onboarding_recent_project_review_prompt(&repository);

    app.onboarding_start_recent_project_review();

    assert!(!app.onboarding_flow_active());
    assert!(
        !app.pending_turn,
        "remote review must not set the local pending-turn flag"
    );
    assert!(
        !app.is_processing,
        "remote review must stay idle until the remote queue dispatches"
    );
    assert!(app.input.is_empty());
    assert_eq!(
        app.queued_messages,
        vec![expected]
    );
}

#[test]
fn recent_project_review_falls_back_cleanly_when_no_repo_is_known() {
    let mut app = onboarding_test_app();
    app.is_remote = true;
    app.session.working_dir = dirs::home_dir().map(|path| path.to_string_lossy().into_owned());

    app.onboarding_start_recent_project_review();

    assert!(!app.pending_turn);
    assert!(app.queued_messages.is_empty());
    assert!(matches!(app.onboarding_phase(), Some(OnboardingPhase::Suggestions)));
    assert!(app.status_notice.as_ref().is_some_and(|(notice, _)| {
        notice.contains("No recent Git repository found")
    }));
}
