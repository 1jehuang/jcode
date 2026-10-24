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
fn todo_completion_report_saves_full_evidence_but_renders_compactly_without_poke() {
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
        todos[11].details = Some(format!(
            "Evidence for task 11 {} FINAL_EVIDENCE_MARKER",
            "long evidence with Unicode 測試 and punctuation * _ < > &. ".repeat(80)
        ));
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
                feedback_loop_relevance_history: vec![
                    crate::todo::FeedbackLoopRelevance::Indirect,
                    crate::todo::FeedbackLoopRelevance::AcceptanceBlocked,
                ],
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
        let snapshot: serde_json::Value = serde_json::from_str(content).unwrap();
        assert_eq!(snapshot["todo_completion_version"], 1);
        for expected in [
            "Report task 0",
            "Report task 11",
            "Evidence for task 11",
            "acceptance_blocked",
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

        // Descriptive assessments alone never fabricate gate findings.
        assert_eq!(snapshot["observations"], serde_json::json!([]));
        // The snapshot retains all evidence, but the chat view only shows an
        // inline intent header and the finished task dots at every width.
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
            assert_eq!(lines.len(), 2, "{plain}");
            assert_eq!(lines[1].to_string().trim(), "●".repeat(12), "{plain}");
            if width == 80 {
                assert_eq!(
                    lines[0].to_string(),
                    "Todo completion  Show all evidence without another agent turn"
                );
            }
            // Removing wrap whitespace lets us assert every detail even when
            // an individual word has to be split at a narrow terminal width.
            let unwrapped: String = plain.chars().filter(|ch| !ch.is_whitespace()).collect();
            for unexpected in [
                "Report task",
                "Evidence for task",
                "Intent clear",
                "FINAL_EVIDENCE_MARKER",
                "Earlier weak states",
                "feedback_loop_relevance was indirect",
                "intent understanding was uncertain",
                "Current weaknesses, limitations, or missing assessments",
                "Disabled gates that would have fired",
            ] {
                let unexpected: String = unexpected
                    .chars()
                    .filter(|ch| !ch.is_whitespace())
                    .collect();
                assert!(
                    !unwrapped.contains(&unexpected),
                    "unexpected {unexpected}: {plain}"
                );
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
        let observations = vec![crate::todo::GateObservation {
            kind: crate::todo::GateObservationKind::IntentUnderstanding,
            group: None,
            state: Some("uncertain".into()),
        }];
        crate::todo::save_gate_observations(&session_id, &observations).unwrap();
        crate::todo::save_plan(
            &session_id,
            &crate::todo::TodoPlan {
                understands_user_intent: Some(crate::todo::IntentUnderstanding::Clear),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(app.refresh_completed_todo_report(true));
        assert_eq!(completion_report_messages(&app).len(), 1);
        assert!(!app.refresh_completed_todo_report(true));
        // Observation-only late writes must invalidate the fingerprint, and
        // repeated observations must retain the digest's count and wording.
        crate::todo::save_gate_observations(
            &session_id,
            &[observations[0].clone(), observations[0].clone()],
        )
        .unwrap();
        assert!(app.refresh_completed_todo_report(true));
        assert_eq!(completion_report_messages(&app).len(), 1);
        let first_report = completion_report_messages(&app)[0].clone();
        let first_render = crate::tui::ui::render_system_message(
            &first_report,
            40,
            crate::config::DiffDisplayMode::Off,
        );

        let plain = first_render
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let normalized = plain.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(normalized.contains("Disabled gates that would have fired"));
        assert!(normalized.contains("• Intent understanding"));
        assert!(!normalized.contains("started this work without understanding"));
        assert!(!normalized.contains("flagged 2 times this turn"));
        assert!(!plain.contains("[auto]"));
        assert!(!normalized.contains("Do not reply or wait for the user"));
        let snapshot: serde_json::Value = serde_json::from_str(&first_report.content).unwrap();
        assert_eq!(snapshot["observations"].as_array().unwrap().len(), 2);
        crate::todo::clear_gate_observations(&session_id).unwrap();
        assert!(!app.refresh_completed_todo_report(true));
        assert_eq!(
            completion_report_messages(&app)[0].content,
            first_report.content
        );
        crate::todo::save_plan(
            &session_id,
            &crate::todo::TodoPlan {
                understands_user_intent: Some(crate::todo::IntentUnderstanding::Uncertain),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(app.refresh_completed_todo_report(true));
        let updated = completion_report_messages(&app)[0].clone();
        let text = crate::tui::ui::render_system_message(
            &updated,
            200,
            crate::config::DiffDisplayMode::Off,
        )
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join(" ");
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(text.contains("• Intent understanding"));
        assert!(!text.contains("never became solid"));
        assert!(!text.contains("flagged 2 times this turn"));
        // This updated snapshot is the immutable record for the earlier cycle.
        let first_render = crate::tui::ui::render_system_message(
            &updated,
            40,
            crate::config::DiffDisplayMode::Off,
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
        assert!(reports[1].content.contains("\"observations\":[]"));
        assert_eq!(
            crate::tui::ui::render_system_message(
                reports[0],
                40,
                crate::config::DiffDisplayMode::Off,
            ),
            first_render,
            "historic report must not read the current todo file"
        );
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
