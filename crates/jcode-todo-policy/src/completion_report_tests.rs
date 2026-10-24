use super::*;

#[test]
fn uncapped_and_preserves_every_todo_field() {
    let todos: Vec<_> = (0..12)
        .map(|i| TodoItem {
            id: format!("id{i}"),
            content: format!("Task {i}"),
            status: "completed".into(),
            priority: "high".into(),
            group: Some("lane".into()),
            details: Some(format!("Full details {i}")),
            confidence: Some(ConfidenceState::Verified),
            completion_confidence: Some(ConfidenceState::Validated),
            confidence_history: vec![ConfidenceState::Speculative, ConfidenceState::Verified],
            assigned_to: Some("worker".into()),
            blocked_by: vec!["dependency".into()],
            waiting_on: vec!["background-task".into()],
            stage: Some(2),
        })
        .collect();
    let report = render_completion_report(&todos, &TodoPlan::default(), &[]);
    for i in 0..12 {
        assert!(report.contains(&format!("Full details {i}")));
        assert!(report.contains(&format!("Task {i}")));
    }
    for value in [
        "Todos (12)",
        "worker",
        "dependency",
        "**Stage:** 2",
        "**Completion confidence:** validated",
        "speculative → verified",
    ] {
        assert!(report.contains(value), "{value}");
    }
    assert!(!report.contains("all checks passed"));
}

#[test]
fn improved_histories_remain_prominent_and_all_goal_fields_render() {
    let plan = TodoPlan {
        user_intention: Some("Deliver outcome".into()),
        understands_user_intent: Some(IntentUnderstanding::Complete),
        understands_user_intent_history: vec![
            IntentUnderstanding::Uncertain,
            IntentUnderstanding::Complete,
        ],
    };
    let goal = TodoGoal {
        group: Some("lane".into()),
        feedback_loop: Some("Run acceptance workflow".into()),
        closed_feedback_loop: Some(FeedbackLoopState::Closed),
        closed_feedback_loop_history: vec![FeedbackLoopState::Absent, FeedbackLoopState::Closed],
        feedback_loop_relevance: Some(FeedbackLoopRelevance::AcceptanceAligned),
        feedback_loop_relevance_history: vec![
            FeedbackLoopRelevance::Synthetic,
            FeedbackLoopRelevance::AcceptanceAligned,
        ],
        feedback_loop_coverage: Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths),
        feedback_loop_coverage_history: vec![
            FeedbackLoopCoverage::Narrow,
            FeedbackLoopCoverage::EdgeAndIntegrationPaths,
        ],
        feedback_loop_traceability: Some(FeedbackLoopTraceability::Complete),
        feedback_loop_traceability_history: vec![
            FeedbackLoopTraceability::Unmapped,
            FeedbackLoopTraceability::Complete,
        ],
        delivery_state: Some(DeliveryState::OutcomeDelivered),
        delivery_state_history: vec![DeliveryState::ChangeMade, DeliveryState::OutcomeDelivered],
        difficulty: Some(Difficulty::Involved),
        autonomy: Some(Autonomy::NecessaryFollowthrough),
        iteration_maturity: Some(IterationMaturity::OutcomeReached),
        stopping_evidence: Some("Measured result".into()),
    };
    let report = render_completion_report(&[], &plan, &[goal]);
    let earlier = report
        .split("## Earlier weak states")
        .nth(1)
        .unwrap()
        .split("## Plan")
        .next()
        .unwrap();
    for value in [
        "uncertain",
        "absent",
        "synthetic",
        "narrow",
        "unmapped",
        "change&#95;made",
    ] {
        assert!(earlier.contains(value), "{value}");
    }
    for value in [
        "Deliver outcome",
        "Measured result",
        "Difficulty (descriptive)",
        "Autonomy (descriptive)",
        "Iteration maturity",
        "Delivery state history",
        "Feedback loop traceability history",
    ] {
        assert!(report.contains(value), "{value}");
    }
    assert!(report.contains("None identified in the recorded data"));
}

#[test]
fn current_weaknesses_use_difficulty_thresholds() {
    let goal = TodoGoal {
        difficulty: Some(Difficulty::Involved),
        feedback_loop_relevance: Some(FeedbackLoopRelevance::Representative),
        feedback_loop_coverage: Some(FeedbackLoopCoverage::MainPaths),
        feedback_loop_traceability: Some(FeedbackLoopTraceability::Partial),
        iteration_maturity: Some(IterationMaturity::BudgetExhausted),
        ..Default::default()
    };
    let report = render_completion_report(&[], &TodoPlan::default(), &[goal]);
    let current = report.split("## Earlier").next().unwrap();
    for value in [
        "relevance weak or missing",
        "coverage weak or missing",
        "traceability weak or missing",
        "required stopping evidence missing",
        "delivery state weak or missing",
        "intent understanding weak or missing",
    ] {
        assert!(current.contains(value), "{value}");
    }
}

#[test]
fn blocked_acceptance_and_exhausted_constraints_stay_prominent() {
    let report = render_completion_report(
        &[],
        &TodoPlan::default(),
        &[TodoGoal {
            difficulty: Some(Difficulty::Routine),
            feedback_loop_relevance: Some(FeedbackLoopRelevance::AcceptanceBlocked),
            iteration_maturity: Some(IterationMaturity::ConstraintsExhausted),
            stopping_evidence: Some("Service unavailable".into()),
            ..Default::default()
        }],
    );
    let current = report.split("## Earlier").next().unwrap();
    assert!(current.contains("acceptance path blocked"));
    assert!(current.contains("stopped at constraints&#95;exhausted"));
}

#[test]
fn missing_cancelled_unicode_and_hostile_markdown_are_display_only() {
    let todo = TodoItem {
        content: "日本語 🦀\n# fake [link](javascript:bad) <script> *bold*".into(),
        id: "x`|".into(),
        status: "cancelled".into(),
        group: Some("组\n## header".into()),
        ..Default::default()
    };
    let report = render_completion_report(&[todo], &TodoPlan::default(), &[]);
    assert!(report.contains("日本語 🦀&#10;&#35;"));
    assert!(report.contains("&#60;script&#62;"));
    assert!(report.contains("**Status:** cancelled"));
    assert!(report.contains("not recorded"));
    assert!(report.contains("goal assessment missing"));
    assert!(!report.contains("completion confidence weak or missing"));
    assert!(!report.contains("\n# fake"));
    assert!(!report.contains("[link]"));
    assert!(!report.contains("[auto]"));
    assert_eq!(
        report,
        render_completion_report(
            &[TodoItem {
                content: "日本語 🦀\n# fake [link](javascript:bad) <script> *bold*".into(),
                id: "x`|".into(),
                status: "cancelled".into(),
                group: Some("组\n## header".into()),
                ..Default::default()
            }],
            &TodoPlan::default(),
            &[]
        )
    );
}
