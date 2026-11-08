use super::*;

#[test]
fn test_improve_command_starts_improvement_loop() {
    let mut app = create_test_app();
    app.input = "/improve".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, Some(ImproveMode::ImproveRun));
    assert_eq!(
        app.session.improve_mode,
        Some(crate::session::SessionImproveMode::ImproveRun)
    );
    assert!(app.is_processing());

    let msg = app.session.messages.last().expect("missing improve prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("You are entering improvement mode for this repository")
                && text.contains("write a concise ranked todo list using `todo`")
    ));

    let display = app
        .display_messages()
        .last()
        .expect("missing improve launch notice");
    assert!(display.content.contains("Starting improvement loop"));
}

#[test]
fn test_improve_plan_command_is_plan_only_and_accepts_focus() {
    let mut app = create_test_app();
    app.input = "/improve plan startup performance".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, Some(ImproveMode::ImprovePlan));
    assert_eq!(
        app.session.improve_mode,
        Some(crate::session::SessionImproveMode::ImprovePlan)
    );
    assert!(app.is_processing());

    let msg = app
        .session
        .messages
        .last()
        .expect("missing improve plan prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("improvement planning mode")
                && text.contains("This is plan-only mode")
                && text.contains("Focus area: startup performance")
    ));
}

#[test]
fn test_improve_status_summarizes_current_todos() {
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
                    content: "Profile startup path".to_string(),
                    status: "in_progress".to_string(),
                    priority: "high".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                    confidence: Some(crate::todo::ConfidenceState::from_legacy_score(82)),
                    completion_confidence: None,
                    confidence_history: Vec::new(),
                    external_blocker: None,
                    completely_blocked: None,
                    waiting_on: Vec::new(),
                },
                crate::todo::TodoItem {
                    lane_after: None,
                    lane_owner: None,
                    stage: None,
                    details: None,
                    group: None,
                    id: "two".to_string(),
                    content: "Add regression test".to_string(),
                    status: "completed".to_string(),
                    priority: "medium".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                    confidence: None,
                    completion_confidence: None,
                    confidence_history: Vec::new(),
                    external_blocker: None,
                    completely_blocked: None,
                    waiting_on: Vec::new(),
                },
            ],
        )
        .expect("save todos");

        app.improve_mode = Some(ImproveMode::ImproveRun);
        app.input = "/improve status".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing improve status");
        assert!(msg.content.contains("Improve status"));
        assert!(
            msg.content
                .contains("1 incomplete · 1 completed · 0 cancelled")
        );
        assert!(msg.content.contains("Profile startup path"));
        assert!(msg.content.contains("confidence plausible"));
    });
}

#[test]
fn test_improve_stop_without_active_run_reports_idle() {
    let mut app = create_test_app();
    app.session.improve_mode = None;
    app.input = "/improve stop".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing improve stop idle message");
    assert!(msg.content.contains("No active improve loop to stop"));
}

#[test]
fn test_improve_stop_queues_stop_prompt_and_clears_mode() {
    let mut app = create_test_app();
    app.improve_mode = Some(ImproveMode::ImproveRun);
    app.session.improve_mode = Some(crate::session::SessionImproveMode::ImproveRun);
    app.input = "/improve stop".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, None);
    assert_eq!(app.session.improve_mode, None);
    assert!(app.is_processing());

    let msg = app
        .session
        .messages
        .last()
        .expect("missing improve stop prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("Stop improvement mode after the current safe point")
    ));
}

#[test]
fn test_improve_resume_requires_saved_mode() {
    let mut app = create_test_app();
    app.input = "/improve resume".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing improve resume idle message");
    assert!(msg.content.contains("No saved improve run found"));
}

#[test]
fn test_improve_resume_uses_saved_mode_and_current_todos() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.session.improve_mode = Some(crate::session::SessionImproveMode::ImproveRun);
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
                content: "Refactor command parsing".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
                external_blocker: None,
                completely_blocked: None,
                waiting_on: Vec::new(),
            }],
        )
        .expect("save todos");

        app.input = "/improve resume".to_string();
        app.submit_input();

        assert_eq!(app.improve_mode, Some(ImproveMode::ImproveRun));
        assert_eq!(
            app.session.improve_mode,
            Some(crate::session::SessionImproveMode::ImproveRun)
        );
        assert!(app.is_processing());

        let msg = app
            .session
            .messages
            .last()
            .expect("missing improve resume prompt");
        assert!(matches!(
            &msg.content[0],
            ContentBlock::Text { text, .. }
                if text.contains("Resume improvement mode")
                    && text.contains("Refactor command parsing")
        ));
    });
}
