#[test]
fn auto_poke_stays_armed_when_a_turn_has_no_todos() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.auto_poke_incomplete_todos = true;
        app.auto_poke_default_on = true;

        assert!(
            !app.schedule_auto_poke_followup_if_needed(),
            "no todos means nothing to poke about this turn"
        );
        assert!(
            app.auto_poke_incomplete_todos,
            "a todo-free turn must not disable the default-on feature"
        );

        // Later work with incomplete todos must still be poked.
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Finish the thing".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                confidence: Some(crate::todo::ConfidenceState::from_legacy_score(80)),
                ..Default::default()
            }],
        )
        .expect("save incomplete todo");

        assert!(
            app.schedule_auto_poke_followup_if_needed(),
            "incomplete todos on a later turn must still schedule a poke"
        );
    });
}

#[test]
fn auto_poke_does_not_repeat_until_incomplete_todos_change() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.auto_poke_incomplete_todos = true;
        let pending = |content: &str| crate::todo::TodoItem {
            id: "todo-1".to_string(),
            content: content.to_string(),
            status: "pending".to_string(),
            priority: "high".to_string(),
            confidence: Some(crate::todo::ConfidenceState::from_legacy_score(80)),
            ..Default::default()
        };

        crate::todo::save_todos(&app.session.id, &[pending("Wait for worker")]).expect("save");
        assert!(app.schedule_auto_poke_followup_if_needed());

        // Simulate dispatch and completion of the automatically poked turn.
        app.queued_messages.clear();
        app.pending_queued_dispatch = false;
        assert!(
            !app.schedule_auto_poke_followup_if_needed(),
            "an unchanged list must not consume another model turn"
        );

        crate::todo::save_todos(&app.session.id, &[pending("Review worker result")])
            .expect("update");
        assert!(
            app.schedule_auto_poke_followup_if_needed(),
            "changing the todo list must re-arm the automatic nudge"
        );

        app.queued_messages.clear();
        app.pending_queued_dispatch = false;
        crate::todo::save_todos(&app.session.id, &[]).expect("finish cycle");
        assert!(!app.schedule_auto_poke_followup_if_needed());
        crate::todo::save_todos(&app.session.id, &[pending("Review worker result")])
            .expect("start equivalent new cycle");
        assert!(
            app.schedule_auto_poke_followup_if_needed(),
            "an equivalent todo in a new cycle must receive one fresh nudge"
        );
    });
}

#[test]
fn completed_cycle_rearms_auto_poke_only_when_default_on() {
    with_temp_jcode_home(|| {
        let completed = |id: &str| crate::todo::TodoItem {
            id: id.to_string(),
            content: "Done".to_string(),
            status: "completed".to_string(),
            priority: "high".to_string(),
            confidence: Some(crate::todo::ConfidenceState::from_legacy_score(100)),
            completion_confidence: Some(crate::todo::ConfidenceState::from_legacy_score(100)),
            confidence_history: vec![
                crate::todo::ConfidenceState::Validated,
                crate::todo::ConfidenceState::Verified,
            ],
            ..Default::default()
        };

        let mut app = create_test_app();
        app.auto_poke_incomplete_todos = true;
        app.auto_poke_default_on = true;
        crate::todo::save_todos(&app.session.id, &[completed("todo-1")]).expect("save");
        crate::todo::save_goals(
            &app.session.id,
            &[crate::todo::TodoGoal {
                delivery_state: Some(crate::todo::DeliveryState::WorkflowValidated),
                autonomy: Some(crate::todo::Autonomy::NecessaryFollowthrough),
                iteration_maturity: Some(crate::todo::IterationMaturity::OutcomeReached),
                feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
                feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
                feedback_loop_traceability: Some(crate::todo::FeedbackLoopTraceability::Complete),
                ..Default::default()
            }],
        )
        .expect("save passing goal");
        assert!(app.schedule_auto_poke_followup_if_needed());
        assert_eq!(
            app.queued_messages,
            vec![crate::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE.to_string()]
        );
        app.queued_messages.clear();
        app.pending_queued_dispatch = false;
        assert!(!app.schedule_auto_poke_followup_if_needed());
        assert!(
            app.auto_poke_incomplete_todos,
            "default-on auto-poke should cover the next batch of work too"
        );

        // The final handoff must stay latched across a client reload too.
        app.save_input_for_reload(&app.session.id);
        let restored = App::restore_input_for_reload(&app.session.id)
            .expect("final handoff should survive reload");
        let mut reloaded = create_test_app();
        reloaded.session.id = app.session.id.clone();
        reloaded.auto_poke_incomplete_todos = true;
        reloaded.auto_poke_default_on = true;
        reloaded.apply_restored_reload_input(restored);
        assert_eq!(
            reloaded.final_response_todo_fingerprint,
            app.final_response_todo_fingerprint
        );
        assert!(reloaded.todo_final_response_requested);
        app = reloaded;

        // An overdue timed review alone must not restart finished work.
        let review_path = crate::storage::jcode_dir()
            .expect("jcode home")
            .join("todos")
            .join(format!("{}-review-state.json", app.session.id));
        crate::storage::write_json_fast(
            &review_path,
            &serde_json::json!({
                "cycle_started_at": "2020-01-01T00:00:00Z",
                "review_delivered": false,
            }),
        )
        .expect("age review");

        // A late observation must not resurrect gates after the final answer,
        // even when the scheduler is called repeatedly with the same todo state.
        crate::todo::append_gate_observations(
            &app.session.id,
            &[crate::todo::GateObservation {
                kind: crate::todo::GateObservationKind::IntentUnderstanding,
                group: None,
                state: Some("uncertain".to_string()),
            }],
        )
        .expect("record late observation");
        for _ in 0..3 {
            assert!(!app.schedule_auto_poke_followup_if_needed());
            assert!(app.queued_messages.is_empty());
            assert!(!app.pending_queued_dispatch);
        }

        // Rewriting identical data is not a new cycle either.
        crate::todo::save_todos(&app.session.id, &[completed("todo-1")]).expect("save");
        assert!(!app.schedule_auto_poke_followup_if_needed());

        assert!(
            crate::todo::take_long_session_review_if_due(&app.session.id)
                .expect("the final-response guard must not consume the timed review")
        );

        // A real todo change re-arms the quality checks and final handoff.
        let mut changed = completed("todo-1");
        changed.completion_confidence = Some(crate::todo::ConfidenceState::Plausible);
        crate::todo::save_todos(&app.session.id, &[changed]).expect("save changed todo");
        assert!(app.schedule_auto_poke_followup_if_needed());
        assert!(app.queued_messages[0].starts_with(crate::todo::TODO_GATE_DIGEST_PREFIX));
        assert!(!app.todo_final_response_requested);
        app.queued_messages.clear();
        app.pending_queued_dispatch = false;
        assert!(app.schedule_auto_poke_followup_if_needed());
        assert!(
            app.queued_messages[0].starts_with(crate::todo::TODO_COMPLETION_CONTINUATION_MESSAGE)
        );
        app.queued_messages.clear();
        app.pending_queued_dispatch = false;
        crate::todo::save_todos(&app.session.id, &[completed("todo-1")]).expect("save validated");
        assert!(app.schedule_auto_poke_followup_if_needed());
        assert_eq!(
            app.queued_messages,
            vec![crate::todo::TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE.to_string()]
        );

        // An explicit /poke off must stick.
        let mut app = create_test_app();
        app.auto_poke_incomplete_todos = true;
        app.auto_poke_default_on = true;
        crate::tui::app::commands::disable_auto_poke(&mut app);
        crate::todo::save_todos(&app.session.id, &[completed("todo-2")]).expect("save");
        crate::todo::save_goals(
            &app.session.id,
            &[crate::todo::TodoGoal {
                delivery_state: Some(crate::todo::DeliveryState::WorkflowValidated),
                autonomy: Some(crate::todo::Autonomy::NecessaryFollowthrough),
                iteration_maturity: Some(crate::todo::IterationMaturity::OutcomeReached),
                feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
                feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
                feedback_loop_traceability: Some(crate::todo::FeedbackLoopTraceability::Complete),
                ..Default::default()
            }],
        )
        .expect("save passing goal");
        app.auto_poke_incomplete_todos = true; // pretend a stale arm survived
        app.todo_final_response_requested = true; // final handoff already delivered
        assert!(!app.schedule_auto_poke_followup_if_needed());
        assert!(
            !app.auto_poke_incomplete_todos,
            "/poke off must not be undone by the default-on re-arm"
        );
    });
}

#[test]
fn peer_auto_poke_is_shown_and_suppresses_duplicate_local_poke() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.auto_poke_incomplete_todos = true;
        app.auto_poke_default_on = true;
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Send the email".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                confidence: Some(crate::todo::ConfidenceState::from_legacy_score(80)),
                ..Default::default()
            }],
        )
        .expect("save incomplete todo");

        let poke = crate::todo::build_auto_poke_message(1);
        app.handle_peer_user_message(&poke);

        assert!(
            app.display_messages()
                .iter()
                .any(|msg| msg.role == "system" && msg.content.contains("1 incomplete todo")),
            "a poke sent by another window must be shown here too"
        );
        assert!(
            !app.display_messages().iter().any(|msg| msg.role == "user"),
            "a peer poke must not render as a user prompt"
        );
        assert!(
            !app.schedule_auto_poke_followup_if_needed(),
            "this window must not re-poke the same open work a peer already poked"
        );
    });
}
