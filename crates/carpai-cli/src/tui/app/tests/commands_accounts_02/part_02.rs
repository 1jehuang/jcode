#[test]
fn test_improve_mode_persists_in_session_file() {
    with_temp_jcode_home(|| {
        let mut session = crate::session::Session::create(None, None);
        session.improve_mode = Some(crate::session::SessionImproveMode::ImprovePlan);
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
                && text.contains("use the `subagent` tool exactly once")
    ));

    let display = app
        .display_messages()
        .last()
        .expect("missing refactor launch notice");
    assert!(display.content.contains("Starting refactor loop"));
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
                    id: "one".to_string(),
                    content: "Split giant module".to_string(),
                    status: "in_progress".to_string(),
                    priority: "high".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
                crate::todo::TodoItem {
                    id: "two".to_string(),
                    content: "Run review subagent".to_string(),
                    status: "completed".to_string(),
                    priority: "medium".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
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
                id: "resume1".to_string(),
                content: "Extract review prompt builder".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
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
