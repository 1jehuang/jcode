#[test]
fn test_improve_mode_persists_in_session_file() {
    with_temp_jcode_home(|| {
        let mut session = crate::session::Session::create(None, None);
        session.improve_mode = Some(crate::session::SessionImproveMode::ImprovePlan);
        // Untouched sessions are not persisted (783c979a0); give it a real turn.
        session.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: "improve this".to_string(),
                cache_control: None,
            }],
        );
        let session_id = session.id.clone();
        session.save().expect("save session");

        let loaded = crate::session::Session::load(&session_id).expect("load session");
        assert_eq!(
            loaded.improve_mode,
            Some(crate::session::SessionImproveMode::ImprovePlan)
        );
    });
}

#[test]
fn test_refactor_command_starts_refactor_loop() {
    let mut app = create_test_app();
    app.input = "/refactor".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, Some(ImproveMode::RefactorRun));
    assert_eq!(
        app.session.improve_mode,
        Some(crate::session::SessionImproveMode::RefactorRun)
    );
    assert!(app.is_processing());

    let msg = app
        .session
        .messages
        .last()
        .expect("missing refactor prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("You are entering refactor mode for this repository")
                && text.contains("use the `swarm` tool with `action=spawn` exactly once")
    ));

    let display = app
        .display_messages()
        .last()
        .expect("missing refactor launch notice");
    assert!(display.content.contains("Starting refactor loop"));
}

#[test]
fn test_plan_command_is_plan_only_and_presents_plan_card() {
    let mut app = create_test_app();
    app.input = "/plan add a compact message mode".to_string();
    app.submit_input();

    // /plan is a one-shot, not a resumable improve/refactor loop.
    assert_eq!(app.improve_mode, None);
    assert!(app.is_processing());

    let msg = app.session.messages.last().expect("missing plan prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("You are entering planning mode")
                && text.contains("Do NOT implement anything yet")
                && text.contains("```plan")
                && text.contains("`todo`")
                && text.contains("Goal: add a compact message mode")
    ));

    let display = app
        .display_messages()
        .last()
        .expect("missing plan launch notice");
    assert!(
        display
            .content
            .contains("Planning add a compact message mode")
    );
}

#[test]
fn test_plan_command_without_goal_plans_current_focus() {
    let mut app = create_test_app();
    app.input = "/plan".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, None);
    assert!(app.is_processing());

    let msg = app.session.messages.last().expect("missing plan prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("You are entering planning mode")
                && text.contains("currently in focus in this session")
    ));
}

#[test]
fn test_refactor_plan_command_is_plan_only_and_accepts_focus() {
    let mut app = create_test_app();
    app.input = "/refactor plan command parsing".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, Some(ImproveMode::RefactorPlan));
    assert_eq!(
        app.session.improve_mode,
        Some(crate::session::SessionImproveMode::RefactorPlan)
    );
    assert!(app.is_processing());

    let msg = app
        .session
        .messages
        .last()
        .expect("missing refactor plan prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("refactor planning mode")
                && text.contains("This is plan-only mode")
                && text.contains("Focus area: command parsing")
    ));
}

#[test]
fn test_refactor_status_summarizes_current_todos() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[
                crate::todo::TodoItem {
                    lane_after: None,
                    lane_owner: None,
                    stage: None,
                    details: None,
                    group: None,
                    id: "one".to_string(),
                    content: "Split giant module".to_string(),
                    status: "in_progress".to_string(),
                    priority: "high".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                    confidence: Some(crate::todo::ConfidenceState::from_legacy_score(76)),
                    completion_confidence: None,
                    confidence_history: Vec::new(),
                    waiting_reason: None,
                    requires_user_auth: None,
                    waiting_on: Vec::new(),
                },
                crate::todo::TodoItem {
                    lane_after: None,
                    lane_owner: None,
                    stage: None,
                    details: None,
                    group: None,
                    id: "two".to_string(),
                    content: "Run review subagent".to_string(),
                    status: "completed".to_string(),
                    priority: "medium".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                    confidence: None,
                    completion_confidence: None,
                    confidence_history: Vec::new(),
                    waiting_reason: None,
                    requires_user_auth: None,
                    waiting_on: Vec::new(),
                },
            ],
        )
        .expect("save todos");

        app.improve_mode = Some(ImproveMode::RefactorRun);
        app.input = "/refactor status".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing refactor status");
        assert!(msg.content.contains("Refactor status"));
        assert!(
            msg.content
                .contains("1 incomplete · 1 completed · 0 cancelled")
        );
        assert!(msg.content.contains("Split giant module"));
        assert!(msg.content.contains("confidence plausible"));
    });
}

#[test]
fn test_refactor_resume_uses_saved_mode_and_current_todos() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.session.improve_mode = Some(crate::session::SessionImproveMode::RefactorRun);
        app.session.save().expect("save session");
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                lane_after: None,
                lane_owner: None,
                stage: None,
                details: None,
                group: None,
                id: "resume1".to_string(),
                content: "Extract review prompt builder".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
                waiting_reason: None,
                requires_user_auth: None,
                waiting_on: Vec::new(),
            }],
        )
        .expect("save todos");

        app.input = "/refactor resume".to_string();
        app.submit_input();

        assert_eq!(app.improve_mode, Some(ImproveMode::RefactorRun));
        assert_eq!(
            app.session.improve_mode,
            Some(crate::session::SessionImproveMode::RefactorRun)
        );
        assert!(app.is_processing());

        let msg = app
            .session
            .messages
            .last()
            .expect("missing refactor resume prompt");
        assert!(matches!(
            &msg.content[0],
            ContentBlock::Text { text, .. }
                if text.contains("Resume refactor mode")
                    && text.contains("Extract review prompt builder")
        ));
    });
}

#[test]
fn test_fix_resets_provider_session() {
    let mut app = create_test_app();
    app.provider_session_id = Some("provider-session".to_string());
    app.session.provider_session_id = Some("provider-session".to_string());
    app.last_stream_error = Some("Stream error: context window exceeded".to_string());

    app.input = "/fix".to_string();
    app.submit_input();

    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());

    let msg = app
        .display_messages()
        .last()
        .expect("missing /fix response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Fix Results"));
    assert!(msg.content.contains("Reset provider session resume state"));
}

#[test]
fn test_turn_error_restores_prompt_to_input() {
    let mut app = create_test_app();
    // Simulate a submitted prompt that started a turn.
    app.last_submitted_input = Some("explain this bug".to_string());
    app.input.clear();

    app.handle_turn_error("Token refresh needed");

    // The typed prompt should be restored to the input box so it is not lost.
    assert_eq!(app.input, "explain this bug");
    // And the saved copy is consumed once restored.
    assert!(app.last_submitted_input.is_none());
}

#[test]
fn test_turn_error_does_not_clobber_new_input() {
    let mut app = create_test_app();
    app.last_submitted_input = Some("old prompt".to_string());
    // User already started typing a new prompt.
    app.input = "new prompt".to_string();

    app.handle_turn_error("Token refresh needed");

    // We must not overwrite text the user already started.
    assert_eq!(app.input, "new prompt");
}

#[test]
fn test_account_picker_includes_all_configured_credentials_even_excluded_and_manual() {
    with_temp_jcode_home(|| {
        let _env_guard = EnvRestoreGuard::capture([
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
            "COPILOT_GITHUB_TOKEN",
            "GROQ_API_KEY",
        ]);
        for key in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OPENROUTER_API_KEY"] {
            crate::env::set_var(key, "picker-test-secret-api-key");
        }
        crate::env::set_var("COPILOT_GITHUB_TOKEN", "picker-test-secret-device-token");
        crate::env::set_var("GROQ_API_KEY", "picker-test-secret-compatible-key");
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "work".into(),
            access_token: "picker-test-secret-oauth-access".into(),
            refresh_token: "picker-test-secret-oauth-refresh".into(),
            id_token: None,
            account_id: None,
            expires_at: Some(chrono::Utc::now().timestamp_millis() + 3_600_000),
            email: None,
        })
        .unwrap();
        crate::auth::google::save_tokens(&crate::auth::google::GoogleTokens {
            access_token: "picker-test-secret-google-access".into(),
            refresh_token: "picker-test-secret-google-refresh".into(),
            expires_at: chrono::Utc::now().timestamp_millis() + 3_600_000,
            tier: crate::auth::google::GmailAccessTier::Full,
            email: None,
            services: vec![crate::auth::google::GoogleService::Gmail],
            credentials: None,
        })
        .unwrap();
        crate::auth::account_pool::AccountPool {
            members: vec![],
            excluded: vec![
                "openai-api".into(),
                "copilot".into(),
                "google".into(),
                "groq".into(),
            ],
            ..Default::default()
        }
        .save()
        .unwrap();
        crate::auth::AuthStatus::invalidate_cache();

        let mut app = create_test_app();
        app.input = "/account".to_string();
        app.submit_input();
        let picker = app.inline_interactive_state.as_ref().unwrap();
        for provider_id in [
            "openai-api",
            "anthropic-api",
            "openrouter",
            "copilot",
            "google",
            "groq",
        ] {
            let entry = picker
                .entries
                .iter()
                .find(|entry| {
                    matches!(
                        entry.action,
                        crate::tui::PickerAction::Login(provider) if provider.id == provider_id
                    )
                })
                .unwrap_or_else(|| panic!("missing configured {provider_id}"));
            assert_eq!(entry.options[0].api_method, "configured");
        }
        assert!(
            !picker
                .entries
                .iter()
                .any(|entry| entry.name == "account center")
        );
        assert!(!picker.entries.iter().any(|entry| matches!(entry.action,
            crate::tui::PickerAction::Login(provider) if provider.id == "auto-import"
        )));
        assert!(!format!("{:?}", picker.entries).contains("picker-test-secret"));
        let status = crate::auth::AuthStatus::check_fast();
        for entry in &picker.entries {
            if let crate::tui::PickerAction::Login(provider) = entry.action {
                assert!(
                    status.assessment_for_provider(provider).is_configured(),
                    "unconfigured provider {} should not be offered",
                    provider.id
                );
            }
        }
        let api_row = picker
            .entries
            .iter()
            .position(|entry| {
                matches!(entry.action,
                    crate::tui::PickerAction::Login(provider) if provider.id == "openai-api"
                )
            })
            .unwrap();
        let new_row = picker
            .entries
            .iter()
            .position(|entry| entry.name == "new Claude account")
            .unwrap();
        assert!(
            api_row < new_row,
            "credentials must precede management actions"
        );

        // A local shared-daemon client must retain its local credential
        // inventory, even though it uses the same wire client as SSH.
        app.is_remote = true;
        for (name, credential, expected) in [
            (
                "OpenAI",
                Some(jcode_provider_core::ResolvedCredential::ApiKey),
                "openai-api",
            ),
            ("Groq", None, "groq"),
        ] {
            app.remote_provider_name = Some(name.into());
            app.remote_resolved_credential = credential;
            app.open_account_picker(None);
            let picker = app.inline_interactive_state.as_ref().unwrap();
            let selected = &picker.entries[picker.selected];
            assert!(selected.is_current);
            assert!(matches!(selected.action,
                crate::tui::PickerAction::Login(provider) if provider.id == expected
            ));
        }
        app.is_remote = false;

        for (scope, expected, unexpected) in [
            ("claude", "anthropic-api", "openai-api"),
            ("openai", "openai-api", "anthropic-api"),
        ] {
            app.open_account_picker(Some(scope));
            let entries = &app.inline_interactive_state.as_ref().unwrap().entries;
            assert!(entries.iter().any(|entry| matches!(entry.action,
                crate::tui::PickerAction::Login(provider) if provider.id == expected
            )));
            assert!(!entries.iter().any(|entry| matches!(entry.action,
                crate::tui::PickerAction::Login(provider) if provider.id == unexpected
            )));
            assert!(!entries.iter().any(|entry| entry.name == "account center"));
        }
    });
}
