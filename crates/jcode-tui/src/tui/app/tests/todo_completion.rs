fn completion_report_messages(app: &App) -> Vec<&DisplayMessage> {
    app.display_messages
        .iter()
        .filter(|message| message.title.as_deref() == Some("Todo completion"))
        .collect()
}

fn completion_report_todos(status: &str) -> Vec<crate::todo::TodoItem> {
    (0..12)
        .map(|index| crate::todo::TodoItem {
            id: format!("report-{index}"),
            content: format!("Report task {index}"),
            status: status.into(),
            priority: "high".into(),
            completion_confidence: Some(crate::todo::ConfidenceState::Validated),
            details: Some(format!("Evidence for task {index}")),
            ..Default::default()
        })
        .collect()
}

#[test]
fn todo_completion_report_is_full_nonblocking_and_independent_of_poke() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.auto_poke_incomplete_todos = false;
        app.auto_poke_default_on = false;
        let session_id = app.session.id.clone();
        assert!(!app.refresh_completed_todo_report(true));
        let mut todos = completion_report_todos("pending");
        crate::todo::save_todos(&session_id, &todos).unwrap();
        assert!(!app.refresh_completed_todo_report(true));
        todos = completion_report_todos("completed");
        crate::todo::save_todos(&session_id, &todos).unwrap();
        crate::todo::save_plan(
            &session_id,
            &crate::todo::TodoPlan {
                user_intention: Some("Show all evidence without another agent turn".into()),
                understands_user_intent: Some(crate::todo::IntentUnderstanding::Clear),
                understands_user_intent_history: vec![
                    crate::todo::IntentUnderstanding::Uncertain,
                    crate::todo::IntentUnderstanding::Clear,
                ],
            },
        )
        .unwrap();
        crate::todo::save_goals(
            &session_id,
            &[crate::todo::TodoGoal {
                feedback_loop: Some("Real acceptance workflow is unavailable".into()),
                feedback_loop_relevance: Some(
                    crate::todo::FeedbackLoopRelevance::AcceptanceBlocked,
                ),
                iteration_maturity: Some(crate::todo::IterationMaturity::ConstraintsExhausted),
                stopping_evidence: Some("External service unavailable".into()),
                ..Default::default()
            }],
        )
        .unwrap();

        assert!(app.refresh_completed_todo_report(true));
        let reports = completion_report_messages(&app);
        assert_eq!(reports.len(), 1);
        let report = (*reports[0]).clone();
        let content = &report.content;
        for expected in [
            "Report task 0",
            "Report task 11",
            "Evidence for task 11",
            "acceptance&#95;blocked",
            "uncertain",
            "External service unavailable",
            "Show all evidence without another agent turn",
        ] {
            assert!(content.contains(expected), "missing {expected}: {content}");
        }
        assert!(app.queued_messages.is_empty());
        assert!(!app.pending_queued_dispatch);
        assert!(!app.refresh_completed_todo_report(true));
        assert_eq!(completion_report_messages(&app).len(), 1);

        // The actual renderer keeps the entire report at narrow terminal widths.
        for width in [20, 40, 80] {
            let lines = crate::tui::ui::render_system_message(
                &report,
                width,
                crate::config::DiffDisplayMode::Off,
            );
            let plain = lines
                .iter()
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let normalized = plain.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(normalized.contains("Report task 11"), "{plain}");
            if width >= 40 {
                assert!(plain.contains("acceptance_blocked"), "{plain}");
            }
            for line in &lines {
                assert!(line.width() <= width as usize, "width {width}: {line}");
            }
        }
    });
}

#[test]
fn todo_completion_report_refreshes_evidence_and_preserves_previous_cycles() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let session_id = app.session.id.clone();
        let mut todos = completion_report_todos("completed");
        crate::todo::save_todos(&session_id, &todos).unwrap();
        assert!(app.refresh_completed_todo_report(true));
        todos[0].details = Some("Additional acceptance evidence".into());
        crate::todo::save_todos(&session_id, &todos).unwrap();
        assert!(app.refresh_completed_todo_report(true));
        assert_eq!(completion_report_messages(&app).len(), 1);
        assert!(
            completion_report_messages(&app)[0]
                .content
                .contains("Additional acceptance evidence")
        );

        todos[0].status = "in_progress".into();
        crate::todo::save_todos(&session_id, &todos).unwrap();
        assert!(!app.refresh_completed_todo_report(true));
        todos[0].status = "completed".into();
        todos[0].content = "Second cycle task".into();
        crate::todo::save_todos(&session_id, &todos).unwrap();
        assert!(app.refresh_completed_todo_report(true));
        let reports = completion_report_messages(&app);
        assert_eq!(reports.len(), 2);
        assert!(!reports[0].content.contains("Second cycle task"));
        assert!(reports[1].content.contains("Second cycle task"));
        assert!(app.queued_messages.is_empty());
    });
}

#[test]
fn todo_completion_report_uses_remote_session_and_runs_before_other_gates() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let wrapper = app.session.id.clone();
        crate::todo::save_todos(&wrapper, &completion_report_todos("pending")).unwrap();
        let remote = "session_completion_report_remote";
        app.is_remote = true;
        app.remote_session_id = Some(remote.into());
        crate::todo::save_todos(remote, &completion_report_todos("completed")).unwrap();
        app.auto_poke_default_on = false;
        app.auto_poke_incomplete_todos = false;
        assert!(!app.schedule_auto_poke_followup_if_needed());
        assert_eq!(completion_report_messages(&app).len(), 1);
        assert!(app.queued_messages.is_empty());
    });
}
